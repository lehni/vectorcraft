//! `OffscreenCanvas` for scripts outside a browser, with pixels: a vello_cpu pixmap per canvas,
//! kept in Rust and freed when the script drops the canvas (through a `FinalizationRegistry`,
//! where the engine has one). A canvas is its own 2D context (`getContext('2d')` returns it), with
//! `fillRect`, `clearRect`, `getImageData`, `putImageData`, `createImageData` and the `ImageData`
//! class, `convertToBlob()` (PNG), and text measuring (`font`, `measureText()`) with the app's fonts
//! and shaping (VectorCraft's text crate, harfrust). It draws as [`drawing`] describes.
//!
//! Paper.js resolves CSS colours with it (it fills a pixel with the colour and reads it back), and
//! measures `PointText` with it.

use std::sync::Arc;

use vello_cpu::Pixmap;
use vello_cpu::color::PremulRgba8;
use vello_cpu::kurbo::BezPath;

use crate::bitmap;
use crate::drawing::{self, Surface};
use crate::path2d;

use crate::{Call, Engine, Error, HostFunction, Value, define_accessor, method, object};

/// The largest side of a canvas, and the most pixels one has, as browsers cap them.
pub(crate) const SIDE: f64 = 16384.0;
pub(crate) const AREA: f64 = 16384.0 * 16384.0 / 4.0;

/// What canvases share: their prototype, `ImageData`'s, the realm's `Object`,
/// `Uint8ClampedArray` and `FinalizationRegistry` (when it has one, to free a dropped canvas's
/// pixels), and the pixels of every canvas and `ImageBitmap`, by its number; and the same for
/// `Path2D`s.
pub(crate) struct Canvases<H> {
    image_data: H,
    gradient: H,
    pub object: H,
    clamped: H,
    pub promise: H,
    pub registry: Option<H>,
    pub surfaces: Vec<Option<Surface<H>>>,
    pub bitmap_prototype: H,
    pub path_registry: Option<H>,
    pub paths: Vec<Option<BezPath>>,
}

/// Keeps `item` in the first free slot, and gives back its number.
pub(crate) fn store<T>(slots: &mut Vec<Option<T>>, item: T) -> usize {
    match slots.iter().position(Option::is_none) {
        Some(free) => {
            if let Some(slot) = slots.get_mut(free) {
                *slot = Some(item);
            }
            free
        }
        None => {
            slots.push(Some(item));
            slots.len() - 1
        }
    }
}

/// Sets `OffscreenCanvas` and `ImageData` on `global`, unless it has them.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let methods: Vec<(&str, HostFunction<E::Handle>)> = vec![
        ("getContext", |_, this, _| Ok(this.clone())),
        ("getImageData", get_image_data),
        ("putImageData", put_image_data),
        ("createImageData", create_image_data),
        ("measureText", measure_text),
        ("convertToBlob", convert_to_blob),
        ("beginPath", drawing::begin_path),
        ("closePath", drawing::close_path::<_, drawing::Context>),
        ("moveTo", drawing::move_to::<_, drawing::Context>),
        ("lineTo", drawing::line_to::<_, drawing::Context>),
        ("bezierCurveTo", drawing::bezier_curve_to::<_, drawing::Context>),
        ("quadraticCurveTo", drawing::quadratic_curve_to::<_, drawing::Context>),
        ("arc", drawing::arc::<_, drawing::Context>),
        ("arcTo", drawing::arc_to::<_, drawing::Context>),
        ("ellipse", drawing::ellipse::<_, drawing::Context>),
        ("rect", drawing::rect::<_, drawing::Context>),
        ("roundRect", drawing::round_rect::<_, drawing::Context>),
        ("fill", drawing::fill),
        ("stroke", drawing::stroke),
        ("clip", drawing::clip),
        ("isPointInPath", drawing::is_point_in_path),
        ("isPointInStroke", drawing::is_point_in_stroke),
        ("fillRect", drawing::fill_rect),
        ("strokeRect", drawing::stroke_rect),
        ("clearRect", drawing::clear_rect),
        ("fillText", drawing::fill_text),
        ("strokeText", drawing::stroke_text),
        ("drawImage", drawing::draw_image),
        ("translate", drawing::translate),
        ("rotate", drawing::rotate),
        ("scale", drawing::scale),
        ("transform", drawing::transform),
        ("setTransform", drawing::set_transform),
        ("resetTransform", drawing::reset_transform),
        ("getTransform", drawing::get_transform),
        ("save", drawing::save),
        ("restore", drawing::restore),
        ("reset", drawing::reset),
        ("setLineDash", drawing::set_line_dash),
        ("getLineDash", drawing::get_line_dash),
        ("createLinearGradient", |call, _, args| drawing::create_gradient(call, "linear", args, 4)),
        ("createRadialGradient", |call, _, args| drawing::create_gradient(call, "radial", args, 6)),
        ("createConicGradient", |call, _, args| drawing::create_gradient(call, "conic", args, 3)),
        ("createPattern", drawing::create_pattern),
    ];
    let methods = methods.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let path_methods: Vec<(&str, HostFunction<E::Handle>)> = vec![
        ("closePath", drawing::close_path::<_, drawing::Path2D>),
        ("moveTo", drawing::move_to::<_, drawing::Path2D>),
        ("lineTo", drawing::line_to::<_, drawing::Path2D>),
        ("bezierCurveTo", drawing::bezier_curve_to::<_, drawing::Path2D>),
        ("quadraticCurveTo", drawing::quadratic_curve_to::<_, drawing::Path2D>),
        ("arc", drawing::arc::<_, drawing::Path2D>),
        ("arcTo", drawing::arc_to::<_, drawing::Path2D>),
        ("ellipse", drawing::ellipse::<_, drawing::Path2D>),
        ("rect", drawing::rect::<_, drawing::Path2D>),
        ("roundRect", drawing::round_rect::<_, drawing::Path2D>),
        ("addPath", path2d::add_path),
    ];
    let path_methods = path_methods.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let path_constructor = engine.constructor("Path2D", path2d::construct)?;
    let release_path = engine.function("release", path2d::release)?;
    let width = engine.function("width", |call, this, _| Ok(Value::Number(f64::from(with_pixmap(call, this, |p| p.width())?))))?;
    let height = engine.function("height", |call, this, _| Ok(Value::Number(f64::from(with_pixmap(call, this, |p| p.height())?))))?;
    let set_width = engine.function("width", |call, this, args| resize(call, this, args.first(), None))?;
    let set_height = engine.function("height", |call, this, args| resize(call, this, None, args.first()))?;
    let release = engine.function("release", release)?;
    let bitmap_width = engine.function("width", |call, this, _| bitmap::side(call, this, true))?;
    let bitmap_height = engine.function("height", |call, this, _| bitmap::side(call, this, false))?;
    let bitmap_close = engine.function("close", bitmap::close)?;
    let bitmap_constructor = engine.constructor("ImageBitmap", |_, _, _| Err(Error::Type("Illegal constructor".into())))?;
    let create_bitmap = engine.function("createImageBitmap", bitmap::create)?;
    let mut properties = Vec::with_capacity(drawing::PROPERTIES.len());
    for (property, (get, set)) in drawing::PROPERTIES.iter().zip(drawing::accessors()) {
        properties.push((property.name, engine.function(property.name, get)?, engine.function(property.name, set)?));
    }
    let constructor = engine.constructor("OffscreenCanvas", offscreen_canvas)?;
    let image_data = engine.constructor("ImageData", construct_image_data)?;
    let add_color_stop = engine.function("addColorStop", drawing::add_color_stop)?;
    engine.with_call(|call| {
        if !matches!(call.get(global, "OffscreenCanvas")?, Value::Undefined) {
            return Ok(());
        }
        let prototype = call.new_object()?;
        for (name, function) in methods {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        define_accessor(call, global, &prototype, "width", width, Some(set_width))?;
        for (name, get, set) in properties {
            define_accessor(call, global, &prototype, name, get, Some(set))?;
        }
        define_accessor(call, global, &prototype, "height", height, Some(set_height))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(global, "OffscreenCanvas", &Value::Js(constructor))?;
        crate::tag(call, global, &prototype, "OffscreenCanvas")?;
        let gradient = call.new_object()?;
        crate::tag(call, global, &gradient, "CanvasGradient")?;
        call.set(&gradient, "addColorStop", &Value::Js(add_color_stop))?;
        let image_data_prototype = call.new_object()?;
        call.set(&image_data, "prototype", &Value::Js(image_data_prototype.clone()))?;
        call.set(global, "ImageData", &Value::Js(image_data))?;
        crate::tag(call, global, &image_data_prototype, "ImageData")?;
        let path_prototype = call.new_object()?;
        for (name, function) in path_methods {
            call.set(&path_prototype, name, &Value::Js(function))?;
        }
        call.set(&path_constructor, "prototype", &Value::Js(path_prototype.clone()))?;
        call.set(global, "Path2D", &Value::Js(path_constructor))?;
        crate::tag(call, global, &path_prototype, "Path2D")?;
        let bitmap_prototype = call.new_object()?;
        call.set(&bitmap_prototype, "close", &Value::Js(bitmap_close))?;
        define_accessor(call, global, &bitmap_prototype, "width", bitmap_width, None)?;
        define_accessor(call, global, &bitmap_prototype, "height", bitmap_height, None)?;
        call.set(&bitmap_constructor, "prototype", &Value::Js(bitmap_prototype.clone()))?;
        call.set(global, "ImageBitmap", &Value::Js(bitmap_constructor))?;
        crate::tag(call, global, &bitmap_prototype, "ImageBitmap")?;
        call.set(global, "createImageBitmap", &Value::Js(create_bitmap))?;
        let registry = finalization_registry(call, global, release)?;
        let path_registry = finalization_registry(call, global, release_path)?;
        call.host()?.borrow_mut().canvases = Some(Canvases {
            image_data: image_data_prototype,
            gradient,
            object: object(call, global, "Object")?,
            clamped: object(call, global, "Uint8ClampedArray")?,
            promise: object(call, global, "Promise")?,
            registry,
            surfaces: Vec::new(),
            bitmap_prototype,
            path_registry,
            paths: Vec::new(),
        });
        Ok(())
    })
}

/// A `FinalizationRegistry` that calls `release`, when the realm has the class.
fn finalization_registry<H: Clone>(call: &mut dyn Call<H>, global: &H, release: H) -> Result<Option<H>, Error> {
    Ok(match call.get(global, "FinalizationRegistry")? {
        Value::Js(class) => match call.construct(&class, &[Value::Js(release)])? {
            Value::Js(registry) => Some(registry),
            _ => None,
        },
        _ => None,
    })
}

/// A canvas's or image's side from a script: whole, within the caps.
fn side<H>(value: Option<&Value<H>>) -> Result<u16, Error> {
    match value {
        Some(Value::Number(n)) => pixels(*n),
        None | Some(Value::Undefined) => Ok(0),
        _ => Err(Error::Script("not a size".into())),
    }
}

fn pixels(n: f64) -> Result<u16, Error> {
    if n.is_finite() && (0.0..=SIDE).contains(&n) {
        Ok(n.trunc() as u16)
    } else {
        Err(Error::Script(format!("a canvas can't be {n} pixels wide or high")))
    }
}

/// `new OffscreenCanvas(width, height)`: transparent black, with a context's defaults.
fn offscreen_canvas<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (width, height) = (side(args.first())?, side(args.get(1))?);
    if f64::from(width) * f64::from(height) > AREA {
        return Err(Error::Script(format!("a canvas can't have {width} × {height} pixels")));
    }
    let registry = {
        let host = call.host()?;
        let host = host.borrow();
        host.canvases.as_ref().ok_or_else(|| Error::Script("OffscreenCanvas is not set up".into()))?.registry.clone()
    };
    // `this`, which `new` made from `new.target.prototype`: a subclass's instances stay its own.
    let Value::Js(canvas) = this else {
        return Err(Error::Type("OffscreenCanvas needs 'new'".into()));
    };
    let canvas = canvas.clone();
    let number = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        let canvases = host.canvases.as_mut().ok_or_else(|| Error::Script("OffscreenCanvas is not set up".into()))?;
        store(&mut canvases.surfaces, Surface::new(width, height))
    };
    call.set(&canvas, "_canvas", &Value::Number(number as f64))?;
    if let Some(registry) = registry {
        method(call, &registry, "register", &[Value::Js(canvas.clone()), Value::Number(number as f64)])?;
    }
    call.set(&canvas, "canvas", &Value::Js(canvas.clone()))?;
    drawing::reset_properties(call, &canvas)?;
    Ok(Value::Js(canvas))
}

/// Frees the pixels of the canvas numbered `args[0]`, which the script dropped.
fn release<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Number(number)) = args.first()
        && let Some(canvases) = call.host()?.borrow_mut().canvases.as_mut()
        && let Some(slot) = canvases.surfaces.get_mut(*number as usize)
    {
        *slot = None;
    }
    Ok(Value::Undefined)
}

/// The number of the canvas `this` is.
fn number<H>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<usize, Error> {
    let Value::Js(canvas) = this else {
        return Err(Error::Script("not a canvas".into()));
    };
    match call.get(canvas, "_canvas")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => Ok(n as usize),
        _ => Err(Error::Script("not a canvas".into())),
    }
}

/// `f` with the surface of the canvas `this` is.
pub(crate) fn with_surface<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Surface<H>) -> R) -> Result<R, Error> {
    let number = number(call, this)?;
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let surface = host
        .canvases
        .as_mut()
        .and_then(|c| c.surfaces.get_mut(number))
        .and_then(Option::as_mut)
        .ok_or_else(|| Error::Script("the canvas is gone".into()))?;
    Ok(f(surface))
}

/// `f` with the pixels of the canvas `this` is, what was drawn rendered onto them.
fn with_pixmap<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Pixmap) -> R) -> Result<R, Error> {
    with_surface(call, this, |surface| {
        surface.flush();
        f(&mut surface.pixmap)
    })
}

/// The pixels of `image`, a canvas or an `ImageBitmap`, as they are now.
pub(crate) fn image_pixels<H>(call: &mut dyn Call<H>, image: &Value<H>) -> Result<Arc<Pixmap>, Error> {
    if let Value::Js(object) = image
        && matches!(call.get(object, "_canvas")?, Value::Number(n) if n < 0.0)
    {
        return Err(Error::Dom("InvalidStateError", "the image bitmap is closed".into()));
    }
    with_pixmap(call, image, |pixmap| Arc::new(pixmap.clone())).map_err(|_| Error::Type("the image isn't a canvas".into()))
}

/// A new `CanvasGradient`.
pub(crate) fn new_gradient<H: Clone>(call: &mut dyn Call<H>) -> Result<H, Error> {
    let (object_class, prototype) = {
        let host = call.host()?;
        let host = host.borrow();
        let canvases = host.canvases.as_ref().ok_or_else(|| Error::Script("OffscreenCanvas is not set up".into()))?;
        (canvases.object.clone(), canvases.gradient.clone())
    };
    match method(call, &object_class, "create", &[Value::Js(prototype)])? {
        Value::Js(made) => Ok(made),
        _ => Err(Error::Script("Object.create made nothing".into())),
    }
}

/// Setting `canvas.width` or `height`: the canvas is cleared to that size and its state reset, as
/// in browsers.
fn resize<H>(call: &mut dyn Call<H>, this: &Value<H>, width: Option<&Value<H>>, height: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let (width, height) = (width.map(|w| side(Some(w))).transpose()?, height.map(|h| side(Some(h))).transpose()?);
    let resized = with_surface(call, this, |surface| {
        let (width, height) = (width.unwrap_or(surface.pixmap.width()), height.unwrap_or(surface.pixmap.height()));
        let fits = f64::from(width) * f64::from(height) <= AREA;
        if fits {
            surface.resize(width, height);
        }
        fits
    })?;
    if let (true, Value::Js(context)) = (resized, this) {
        drawing::reset_properties(call, context)?;
    }
    Ok(Value::Undefined)
}

/// A rectangle from a script's numbers, as whole pixels inside `width` × `height`: `(x0, y0, x1,
/// y1)`, empty when it misses. Negative sizes extend left or up, as in browsers.
fn pixel_rect(x: f64, y: f64, w: f64, h: f64, width: u16, height: u16) -> (usize, usize, usize, usize) {
    if ![x, y, w, h].iter().all(|n| n.is_finite()) {
        return (0, 0, 0, 0);
    }
    let (x0, x1) = if w < 0.0 { (x + w, x) } else { (x, x + w) };
    let (y0, y1) = if h < 0.0 { (y + h, y) } else { (y, y + h) };
    let clamp = |v: f64, max: u16| v.round().clamp(0.0, f64::from(max)) as usize;
    (clamp(x0, width), clamp(y0, height), clamp(x1, width), clamp(y1, height))
}

fn numbers<H>(args: &[Value<H>]) -> [f64; 4] {
    let n = |i: usize| match args.get(i) {
        Some(Value::Number(n)) => *n,
        _ => f64::NAN,
    };
    [n(0), n(1), n(2), n(3)]
}

/// `[r, g, b, a]` premultiplied.
pub(crate) fn premultiply([r, g, b, a]: [u8; 4]) -> PremulRgba8 {
    let p = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
    PremulRgba8 { r: p(r), g: p(g), b: p(b), a }
}

/// `[r, g, b, a]` of a premultiplied pixel.
fn unpremultiply(p: PremulRgba8) -> [u8; 4] {
    if p.a == 0 {
        return [0, 0, 0, 0];
    }
    let u = |c: u8| ((u16::from(c) * 255 + u16::from(p.a) / 2) / u16::from(p.a)).min(255) as u8;
    [u(p.r), u(p.g), u(p.b), p.a]
}

/// An `ImageData` of `width` × `height` with `bytes` (RGBA, not premultiplied).
/// An `ImageData` of `width` × `height` `bytes`: `into`, the constructor's `this`, else a new one.
fn image_data<H: Clone>(call: &mut dyn Call<H>, into: &Value<H>, width: u16, height: u16, bytes: &[u8]) -> Result<Value<H>, Error> {
    let (object_class, prototype, clamped) = {
        let host = call.host()?;
        let host = host.borrow();
        let canvases = host.canvases.as_ref().ok_or_else(|| Error::Script("ImageData is not set up".into()))?;
        (canvases.object.clone(), canvases.image_data.clone(), canvases.clamped.clone())
    };
    let image = match into {
        Value::Js(image) => image.clone(),
        _ => crate::handle(method(call, &object_class, "create", &[Value::Js(prototype)])?, "ImageData")?,
    };
    let array = call.new_uint8_array(bytes)?;
    let buffer = call.get(&array, "buffer")?;
    let data = call.construct(&clamped, &[buffer])?;
    for (key, value) in [
        ("width", Value::Number(f64::from(width))),
        ("height", Value::Number(f64::from(height))),
        ("data", data),
        ("colorSpace", Value::String("srgb".into())),
    ] {
        call.set(&image, key, &value)?;
    }
    Ok(Value::Js(image))
}

/// `new ImageData(width, height)` or `new ImageData(data, width[, height])`.
fn construct_image_data<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    match args.first() {
        Some(Value::Js(data)) => {
            let bytes = call.bytes(data)?;
            let width = side(args.get(1))?;
            let rows = bytes.len() / 4 / usize::from(width.max(1));
            let height = match args.get(2) {
                Some(Value::Number(_)) => side(args.get(2))?,
                _ => u16::try_from(rows).map_err(|_| Error::Script("ImageData: too many rows".into()))?,
            };
            if width == 0 || bytes.len() != usize::from(width) * usize::from(height) * 4 {
                return Err(Error::Script("ImageData: the data isn't width × height × 4 bytes".into()));
            }
            image_data(call, this, width, height, &bytes)
        }
        _ => {
            let (width, height) = (side(args.first())?, side(args.get(1))?);
            if f64::from(width) * f64::from(height) > AREA {
                return Err(Error::Script("ImageData: too large".into()));
            }
            image_data(call, this, width, height, &vec![0; usize::from(width) * usize::from(height) * 4])
        }
    }
}

/// `createImageData(width, height)` or `createImageData(imageData)`: transparent black.
fn create_image_data<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (width, height) = match args.first() {
        Some(Value::Js(other)) => {
            let (w, h) = (call.get(other, "width")?, call.get(other, "height")?);
            (side(Some(&w))?, side(Some(&h))?)
        }
        _ => (side(args.first())?, side(args.get(1))?),
    };
    construct_image_data(call, &Value::Undefined, &[Value::Number(f64::from(width)), Value::Number(f64::from(height))])
}

/// `getImageData(x, y, width, height)`: the pixels there, not premultiplied; outside the canvas
/// transparent black.
fn get_image_data<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let [x, y, w, h] = numbers(args);
    if !(w.is_finite() && h.is_finite()) || w == 0.0 || h == 0.0 {
        return Err(Error::Script("getImageData: the width and height must not be 0".into()));
    }
    let (sw, sh) = (pixels(w.abs())?, pixels(h.abs())?);
    let (left, top) = ((if w < 0.0 { x + w } else { x }).round(), (if h < 0.0 { y + h } else { y }).round());
    let bytes = with_pixmap(call, this, |pixmap| {
        let mut bytes = vec![0u8; usize::from(sw) * usize::from(sh) * 4];
        for row in 0..usize::from(sh) {
            for column in 0..usize::from(sw) {
                let (px, py) = (left + column as f64, top + row as f64);
                if px >= 0.0 && py >= 0.0 && px < f64::from(pixmap.width()) && py < f64::from(pixmap.height()) {
                    let pixel = unpremultiply(pixmap.sample(px as u16, py as u16));
                    let at = (row * usize::from(sw) + column) * 4;
                    if let Some(target) = bytes.get_mut(at..at + 4) {
                        target.copy_from_slice(&pixel);
                    }
                }
            }
        }
        bytes
    })?;
    image_data(call, &Value::Undefined, sw, sh, &bytes)
}

/// `putImageData(imageData, x, y[, dirtyX, dirtyY, dirtyWidth, dirtyHeight])`: the pixels replace
/// those there, unblended, as in browsers.
fn put_image_data<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(image)) = args.first() else {
        return Err(Error::Script("putImageData: not an ImageData".into()));
    };
    let (w, h) = (call.get(image, "width")?, call.get(image, "height")?);
    let (width, height) = (side(Some(&w))?, side(Some(&h))?);
    let Value::Js(data) = call.get(image, "data")? else {
        return Err(Error::Script("putImageData: not an ImageData".into()));
    };
    let bytes = call.bytes(&data)?;
    let [dx, dy, ..] = numbers(args.get(1..).unwrap_or_default());
    let dirty = numbers(args.get(3..).unwrap_or_default());
    let (x0, y0, x1, y1) = if dirty.iter().all(|n| n.is_finite()) {
        pixel_rect(dirty[0], dirty[1], dirty[2], dirty[3], width, height)
    } else {
        (0, 0, usize::from(width), usize::from(height))
    };
    if !(dx.is_finite() && dy.is_finite()) {
        return Ok(Value::Undefined);
    }
    let (dx, dy) = (dx.round() as i64, dy.round() as i64);
    with_pixmap(call, this, |pixmap| {
        for row in y0..y1 {
            for column in x0..x1 {
                let (px, py) = (dx + column as i64, dy + row as i64);
                if px < 0 || py < 0 || px >= i64::from(pixmap.width()) || py >= i64::from(pixmap.height()) {
                    continue;
                }
                let at = (row * usize::from(width) + column) * 4;
                if let Some(&[r, g, b, a]) = bytes.get(at..at + 4) {
                    pixmap.set_pixel(px as u16, py as u16, premultiply([r, g, b, a]));
                }
            }
        }
    })?;
    Ok(Value::Undefined)
}

/// `convertToBlob()`: a Promise of the canvas as a PNG `Blob`.
fn convert_to_blob<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let png = with_pixmap(call, this, |pixmap| pixmap.clone().into_png())?.map_err(|e| Error::Script(format!("convertToBlob: {e}")))?;
    #[cfg(not(target_arch = "wasm32"))]
    let blob = crate::fetch::new_blob(call, &png, "image/png")?;
    #[cfg(target_arch = "wasm32")]
    let blob = Value::Js(call.new_uint8_array(&png)?);
    let promise =
        call.host()?.borrow().canvases.as_ref().map(|c| c.promise.clone()).ok_or_else(|| Error::Script("OffscreenCanvas is not set up".into()))?;
    method(call, &promise, "resolve", &[blob])
}

/// Whether `css` is a CSS `font` shorthand a context takes.
pub(crate) fn is_font(css: &str) -> bool {
    Font::parse(css).is_some()
}

/// A context's font until a script sets one, as in browsers.
pub(crate) const DEFAULT_FONT: &str = "10px sans-serif";

/// `context.measureText(text)`: a `TextMetrics` of `text` in the context's `font`: its advance
/// (`width`), its ink around the start of its baseline (`actualBoundingBox…`) and the font's
/// ascent and descent (`fontBoundingBox…`).
fn measure_text<H>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(context) = this else {
        return Err(Error::Script("measureText: not called on a canvas".into()));
    };
    let text = match args.first() {
        Some(text) => call.to_string(text)?,
        None => String::new(),
    };
    let font = match call.get(context, "_font")? {
        Value::String(font) => font,
        _ => DEFAULT_FONT.to_owned(),
    };
    let measured = measure(&text, &Font::parse(&font).unwrap_or_default());
    let metrics = call.new_object()?;
    for (key, value) in [
        ("width", measured.width),
        ("actualBoundingBoxLeft", measured.ink_left),
        ("actualBoundingBoxRight", measured.ink_right),
        ("actualBoundingBoxAscent", measured.ink_ascent),
        ("actualBoundingBoxDescent", measured.ink_descent),
        ("fontBoundingBoxAscent", measured.ascent),
        ("fontBoundingBoxDescent", measured.descent),
    ] {
        call.set(&metrics, key, &Value::Number(value))?;
    }
    Ok(Value::Js(metrics))
}

/// A CSS `font` shorthand as the canvas reads it: `[style] [variant] [weight] size[/line-height]
/// family[, family…]`.
#[derive(Debug, PartialEq)]
struct Font {
    families: Vec<String>,
    bold: bool,
    italic: bool,
    /// In CSS pixels, which are Paper.js's points.
    size: f64,
}

impl Default for Font {
    fn default() -> Self {
        Self { families: vec!["sans-serif".into()], bold: false, italic: false, size: 10.0 }
    }
}

impl Font {
    /// `font`, or `None` when it isn't a font shorthand, which leaves a canvas's font as it was in
    /// browsers.
    fn parse(font: &str) -> Option<Self> {
        let (mut bold, mut italic) = (false, false);
        let mut rest = font.trim();
        loop {
            let (word, after) = rest.split_once(char::is_whitespace)?;
            let lower = word.to_ascii_lowercase();
            match lower.as_str() {
                "italic" | "oblique" => italic = true,
                "bold" | "bolder" => bold = true,
                "normal" | "lighter" | "small-caps" | "condensed" | "expanded" => {}
                weight if weight.parse::<u16>().is_ok_and(|w| (1..=1000).contains(&w)) => bold = weight.parse::<u16>().is_ok_and(|w| w >= 600),
                _ => {
                    let size = Self::size(word.split('/').next()?)?;
                    let families =
                        after.split(',').map(|f| f.trim().trim_matches(['"', '\'']).to_owned()).filter(|f| !f.is_empty()).collect::<Vec<_>>();
                    return (!families.is_empty()).then_some(Self { families, bold, italic, size });
                }
            }
            rest = after.trim_start();
        }
    }

    /// A CSS font size in pixels.
    fn size(size: &str) -> Option<f64> {
        let number = |s: &str| s.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0);
        if let Some(px) = size.strip_suffix("px") {
            number(px)
        } else if let Some(pt) = size.strip_suffix("pt") {
            number(pt).map(|pt| pt * 96.0 / 72.0)
        } else if let Some(em) = size.strip_suffix("em") {
            number(em).map(|em| em * 10.0)
        } else if let Some(percent) = size.strip_suffix('%') {
            number(percent).map(|p| p / 10.0)
        } else {
            None
        }
    }

    /// The font style VectorCraft's font database knows.
    fn style(&self) -> &'static str {
        match (self.bold, self.italic) {
            (true, true) => "Bold Italic",
            (true, false) => "Bold",
            (false, true) => "Italic",
            (false, false) => "Regular",
        }
    }

    /// The first of the families that is installed, CSS's generic families as typical faces for
    /// them; the app's default family when none is.
    fn family(&self) -> String {
        let db = vectorcraft_text::FontDb::global();
        let installed = |family: &str| db.resolve(family, self.style()).is_some_and(|(_, m)| m != vectorcraft_text::FontMatch::Missing);
        for family in &self.families {
            let candidates: &[&str] = match family.to_ascii_lowercase().as_str() {
                "sans-serif" | "system-ui" => &["Helvetica", "Arial", "Helvetica Neue", "Liberation Sans", "DejaVu Sans", "Source Sans 3"],
                "serif" => &["Times", "Times New Roman", "Georgia", "Liberation Serif", "DejaVu Serif"],
                "monospace" => &["Menlo", "Courier New", "Consolas", "Liberation Mono", "DejaVu Sans Mono", "Source Code Pro"],
                "cursive" => &["Apple Chancery", "Comic Sans MS", "URW Chancery L"],
                "fantasy" => &["Papyrus", "Impact"],
                _ => &[],
            };
            if candidates.is_empty() {
                if installed(family) {
                    return family.clone();
                }
            } else if let Some(found) = candidates.iter().find(|c| installed(c)) {
                return (*found).to_owned();
            }
        }
        vectorcraft_doc::CharStyle::default().font_family
    }
}

/// What `measureText` reports, in pixels.
#[derive(Debug, Default)]
pub(crate) struct Measured {
    pub width: f64,
    ink_left: f64,
    ink_right: f64,
    ink_ascent: f64,
    ink_descent: f64,
    pub ascent: f64,
    pub descent: f64,
}

/// `text` laid out on one line in `font`, as the app lays out point type.
fn layout(text: &str, font: &Font) -> vectorcraft_text::TextLayout {
    let style = vectorcraft_doc::CharStyle { font_family: font.family(), font_style: font.style().into(), size: font.size, ..Default::default() };
    let object = vectorcraft_doc::TextObject::point(vectorcraft_geom::Point::ZERO, &text.replace(['\n', '\r'], " "), style);
    vectorcraft_text::layout(vectorcraft_text::FontDb::global(), &object)
}

/// The outlines of `text` in the CSS `font`, the start of its baseline at the origin, and its
/// measurements: what `fillText` draws.
pub(crate) fn outline(text: &str, font: &str) -> (vectorcraft_geom::BezPath, Measured) {
    let font = Font::parse(font).unwrap_or_default();
    let laid = layout(text, &font);
    let measured = measure_layout(&laid);
    let start = laid.glyphs.first().map_or(0.0, |g| g.origin.x);
    let baseline = laid.lines.first().map_or(0.0, |l| l.baseline);
    let mut outline = vectorcraft_geom::BezPath::new();
    for glyph in &laid.glyphs {
        outline.extend(glyph.outline.iter());
    }
    (vectorcraft_geom::Affine::translate((-start, -baseline)) * outline, measured)
}

fn measure(text: &str, font: &Font) -> Measured {
    measure_layout(&layout(text, font))
}

fn measure_layout(layout: &vectorcraft_text::TextLayout) -> Measured {
    use vectorcraft_geom::Shape as _;
    let Some(line) = layout.lines.first() else {
        return Measured::default();
    };
    let width = layout.glyphs.iter().map(|g| g.advance).sum();
    // Ink: the outlines' bounds, around the start of the baseline (y grows downwards).
    let ink = layout.glyphs.iter().filter(|g| !g.outline.elements().is_empty()).map(|g| g.outline.bounding_box()).reduce(|a, b| a.union(b));
    let start = layout.glyphs.first().map_or(0.0, |g| g.origin.x);
    let mut measured = Measured { width, ascent: line.ascent, descent: line.descent, ..Default::default() };
    if let Some(ink) = ink {
        measured.ink_left = start - ink.x0;
        measured.ink_right = ink.x1 - start;
        measured.ink_ascent = line.baseline - ink.y0;
        measured.ink_descent = ink.y1 - line.baseline;
    }
    measured
}

#[cfg(test)]
mod tests {
    use super::Font;

    #[test]
    fn font_shorthands_parse_as_browsers_read_them() {
        let font = |families: &[&str], bold, italic, size| Font { families: families.iter().map(|f| (*f).to_owned()).collect(), bold, italic, size };
        assert_eq!(Font::parse("10px sans-serif"), Some(font(&["sans-serif"], false, false, 10.0)));
        assert_eq!(Font::parse("italic bold 12pt \"Source Sans 3\", serif"), Some(font(&["Source Sans 3", "serif"], true, true, 16.0)));
        assert_eq!(Font::parse("300 2em/1.5 'Helvetica Neue'"), Some(font(&["Helvetica Neue"], false, false, 20.0)));
        assert_eq!(Font::parse("700 50% monospace"), Some(font(&["monospace"], true, false, 5.0)));
        assert_eq!(Font::parse("bold"), None, "no size or family");
        assert_eq!(Font::parse("12px"), None, "no family");
        assert_eq!(Font::parse("big Helvetica"), None, "not a size");
    }
}
