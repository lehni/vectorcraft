//! The canvases of `OffscreenCanvas` outside a browser (`js/src/canvas.js`), with pixels: a
//! vello_cpu pixmap and drawing state per canvas, in a slot the canvas's object names, which it
//! frees when the script drops the canvas. A canvas is its own 2D context, with `fillRect`,
//! `clearRect`, `getImageData`, `putImageData`, `convertToBlob()` (PNG), and text measuring
//! (`font`, `measureText()`) with the app's fonts and shaping (VectorCraft's text crate,
//! harfrust). It draws as [`drawing`] describes.
//!
//! Paper.js resolves CSS colours with it (it fills a pixel with the colour and reads it back), and
//! measures `PointText` with it.

use std::sync::Arc;

use vello_cpu::Pixmap;
use vello_cpu::color::PremulRgba8;
use vello_cpu::kurbo::BezPath;

use crate::bitmap;
use crate::drawing::{self, Paint, Surface};
use crate::path2d;

use crate::{Call, Engine, Error, HostFunction, Value};

/// The largest side of a canvas, and the most pixels one has, as browsers cap them.
pub(crate) const SIDE: f64 = 16384.0;
pub(crate) const AREA: f64 = 16384.0 * 16384.0 / 4.0;

/// The slots of every canvas and `ImageBitmap` (their pixels and drawing state), `Path2D` and
/// gradient or pattern, by their numbers, which their objects in `js/src/canvas.js` keep.
pub(crate) struct Canvases<H> {
    pub surfaces: Vec<Option<Surface<H>>>,
    pub paths: Vec<Option<BezPath>>,
    pub paints: Vec<Option<Paint>>,
}

impl<H> Default for Canvases<H> {
    fn default() -> Self {
        Self { surfaces: Vec::new(), paths: Vec::new(), paints: Vec::new() }
    }
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

/// The natives `js/src/canvas.js` makes its classes of, each with the slot it's for as `this`:
/// `create(width, height)`, a canvas's slot, `width()`, `height()`, `resize(width, height)`,
/// `toPNG()`, the context's `properties` with `get(name)` and `set(name, value, slot)`, and its
/// `methods`; `createGradient(kind, …)`, `addColorStop()` and `createPattern(image, repetition)`;
/// `createPath(source)` and the `pathMethods` of a `Path2D`; `createImageBitmap(source, …)`; and
/// `release(kind, slot)` for the slots the script dropped.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let methods: Vec<(&str, HostFunction<E::Handle>)> = vec![
        ("getImageData", get_image_data),
        ("putImageData", put_image_data),
        ("measureText", measure_text),
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
    ];
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
    let natives: Vec<(&str, HostFunction<E::Handle>)> = vec![
        ("create", create),
        ("width", |call, this, _| Ok(Value::Number(f64::from(with_pixmap(call, this, |p| p.width())?)))),
        ("height", |call, this, _| Ok(Value::Number(f64::from(with_pixmap(call, this, |p| p.height())?)))),
        // `undefined` for the side that stays.
        ("resize", |call, this, args| {
            let given = |n: usize| args.get(n).filter(|side| !matches!(side, Value::Undefined));
            resize(call, this, given(0), given(1))
        }),
        ("toPNG", to_png),
        ("get", drawing::get_property),
        ("set", drawing::set_property),
        ("createGradient", |call, _, args| {
            let (kind, count) = match args.first() {
                Some(Value::String(kind)) if kind == "linear" => ("linear", 4),
                Some(Value::String(kind)) if kind == "radial" => ("radial", 6),
                _ => ("conic", 3),
            };
            drawing::create_gradient(call, kind, args.get(1..).unwrap_or_default(), count)
        }),
        ("addColorStop", drawing::add_color_stop),
        ("createPattern", drawing::create_pattern),
        ("createPath", path2d::construct),
        ("createImageBitmap", bitmap::create),
        ("release", release),
    ];
    let functions = |engine: &mut E, list: Vec<(&'static str, HostFunction<E::Handle>)>| {
        list.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()
    };
    let (methods, path_methods, mut natives) = (functions(engine, methods)?, functions(engine, path_methods)?, functions(engine, natives)?);
    let objects = engine.with_call(|call| {
        let object = |call: &mut dyn Call<E::Handle>, list: Vec<(&'static str, E::Handle)>| -> Result<E::Handle, Error> {
            let object = call.new_object()?;
            for (name, function) in list {
                call.set(&object, name, &Value::Js(function))?;
            }
            Ok(object)
        };
        let names: Vec<_> = drawing::PROPERTIES.iter().map(|p| Value::String(p.name.into())).collect();
        call.host()?.borrow_mut().canvases = Some(Canvases::default());
        Ok([("methods", object(call, methods)?), ("pathMethods", object(call, path_methods)?), ("properties", call.new_array(&names)?)])
    })?;
    natives.extend(objects);
    Ok(natives)
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

/// `create(width, height)`: the slot of a new canvas, transparent black, with a context's
/// defaults.
fn create<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (width, height) = (side(args.first())?, side(args.get(1))?);
    if f64::from(width) * f64::from(height) > AREA {
        return Err(Error::Script(format!("a canvas can't have {width} × {height} pixels")));
    }
    with_canvases(call, |canvases| Value::Number(store(&mut canvases.surfaces, Surface::new(width, height)) as f64))
}

/// `release(kind, slot)`: frees the slot of a `'surface'`, `'path'` or `'paint'` the script
/// dropped, or closed.
fn release<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let kind = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let slot = call.to_number(args.get(1).unwrap_or(&Value::Undefined))?;
    if slot.is_finite() && slot >= 0.0 {
        let slot = slot as usize;
        with_canvases(call, |canvases| match kind.as_str() {
            "surface" => free(&mut canvases.surfaces, slot),
            "path" => free(&mut canvases.paths, slot),
            _ => free(&mut canvases.paints, slot),
        })?;
    }
    Ok(Value::Undefined)
}

fn free<T>(slots: &mut [Option<T>], slot: usize) {
    if let Some(slot) = slots.get_mut(slot) {
        *slot = None;
    }
}

/// `f` with the slots.
pub(crate) fn with_canvases<H, R>(call: &mut dyn Call<H>, f: impl FnOnce(&mut Canvases<H>) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let mut host = host.borrow_mut();
    host.canvases.as_mut().map(f).ok_or_else(|| Error::Script("OffscreenCanvas is not set up".into()))
}

/// The slot `value` is.
pub(crate) fn slot<H>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<usize, Error> {
    match call.to_number(value)? {
        n if n.is_finite() && n >= 0.0 => Ok(n as usize),
        _ => Err(Error::Type("not a canvas's slot".into())),
    }
}

/// `f` with the surface of the canvas `this` is.
pub(crate) fn with_surface<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Surface<H>) -> R) -> Result<R, Error> {
    let slot = slot(call, this)?;
    with_canvases(call, |canvases| canvases.surfaces.get_mut(slot).and_then(Option::as_mut).map(f))?
        .ok_or_else(|| Error::Script("the canvas is gone".into()))
}

/// `f` with the gradient or pattern in `slot`; `None` when it's gone.
pub(crate) fn with_paint<H, R>(call: &mut dyn Call<H>, slot: usize, f: impl FnOnce(&mut Paint) -> R) -> Result<Option<R>, Error> {
    with_canvases(call, |canvases| canvases.paints.get_mut(slot).and_then(Option::as_mut).map(f))
}

/// The slot of a new gradient or pattern.
pub(crate) fn store_paint<H>(call: &mut dyn Call<H>, paint: Paint) -> Result<Value<H>, Error> {
    with_canvases(call, |canvases| Value::Number(store(&mut canvases.paints, paint) as f64))
}

/// `f` with the pixels of the canvas `this` is, what was drawn rendered onto them.
fn with_pixmap<H, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Pixmap) -> R) -> Result<R, Error> {
    with_surface(call, this, |surface| {
        surface.flush();
        f(&mut surface.pixmap)
    })
}

/// The pixels of `image`, the slot of a canvas or an `ImageBitmap`, as they are now.
pub(crate) fn image_pixels<H>(call: &mut dyn Call<H>, image: &Value<H>) -> Result<Arc<Pixmap>, Error> {
    with_pixmap(call, image, |pixmap| Arc::new(pixmap.clone())).map_err(|_| Error::Type("the image isn't a canvas".into()))
}

/// Setting `canvas.width` or `height`: the canvas is cleared to that size and its state reset, as
/// in browsers.
fn resize<H>(call: &mut dyn Call<H>, this: &Value<H>, width: Option<&Value<H>>, height: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let (width, height) = (width.map(|w| side(Some(w))).transpose()?, height.map(|h| side(Some(h))).transpose()?);
    with_surface(call, this, |surface| {
        let (width, height) = (width.unwrap_or(surface.pixmap.width()), height.unwrap_or(surface.pixmap.height()));
        if f64::from(width) * f64::from(height) <= AREA {
            surface.resize(width, height);
        }
    })?;
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

/// A new `ImageData` of `width` × `height` `bytes` (RGBA, not premultiplied).
fn image_data<H: Clone>(call: &mut dyn Call<H>, width: u16, height: u16, bytes: &[u8]) -> Result<Value<H>, Error> {
    let array = call.new_uint8_array(bytes)?;
    let buffer = call.get(&array, "buffer")?;
    let clamped = crate::global_class(call, "Uint8ClampedArray")?;
    let data = call.construct(&clamped, &[buffer])?;
    let class = crate::global_class(call, "ImageData")?;
    call.construct(&class, &[data, Value::Number(f64::from(width)), Value::Number(f64::from(height))])
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
    image_data(call, sw, sh, &bytes)
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

/// `toPNG()`: the canvas as PNG bytes, for `convertToBlob()`.
fn to_png<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let png = with_pixmap(call, this, |pixmap| pixmap.clone().into_png())?.map_err(|e| Error::Script(format!("convertToBlob: {e}")))?;
    Ok(Value::Js(call.new_uint8_array(&png)?))
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
fn measure_text<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = match args.first() {
        Some(text) => call.to_string(text)?,
        None => String::new(),
    };
    let font = match drawing::property_value(call, this, "font")? {
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
