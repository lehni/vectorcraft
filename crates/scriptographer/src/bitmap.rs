//! `ImageBitmap` and `createImageBitmap()` for scripts outside a browser: decoded images (PNG,
//! JPEG, GIF, WebP, BMP, TIFF, TGA from a `Blob`), or copies of an `ImageData`, a canvas or
//! another bitmap, cropped, flipped and resized as asked, to draw with `drawImage()` and
//! `createPattern()`. A bitmap keeps its pixels as a canvas does, in a slot of its own, so it reads
//! as one; it has no context, and `close()` frees it.

use std::sync::Arc;

use vello_cpu::kurbo::{Affine, Rect, Shape as _};
use vello_cpu::peniko::{Extend, ImageQuality, ImageSampler};
use vello_cpu::{Pixmap, RenderContext, RenderSettings, Resources};

use crate::canvas::{AREA, SIDE, premultiply, store};
use crate::drawing::Surface;
use crate::{Call, Error, Value, method};

/// `createImageBitmap(image[, sx, sy, sw, sh][, options])`: a Promise of an `ImageBitmap` of
/// `image` (a `Blob`, an `ImageData`, a canvas or an `ImageBitmap`), cropped to the rectangle,
/// then flipped (`imageOrientation: 'flipY'`) and resized (`resizeWidth`, `resizeHeight`,
/// `resizeQuality`) as the options say.
pub(crate) fn create<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(source)) = args.first() else {
        return Err(Error::Type("createImageBitmap: not an image".into()));
    };
    let pixels = pixels(call, source)?;
    let (crop, options) = match args.get(1) {
        Some(Value::Number(_)) => {
            let [sx, sy, sw, sh] = [1, 2, 3, 4].map(|i| match args.get(i) {
                Some(Value::Number(n)) if n.is_finite() => *n,
                _ => f64::NAN,
            });
            if sw == 0.0 || sh == 0.0 || [sx, sy, sw, sh].iter().any(|n| n.is_nan()) {
                return Err(Error::Range("createImageBitmap: the crop is empty".into()));
            }
            // A negative size extends left or up, as in browsers.
            (Rect::new(sx, sy, sx + sw, sy + sh).abs(), args.get(5))
        }
        other => (Rect::new(0.0, 0.0, f64::from(pixels.width()), f64::from(pixels.height())), other),
    };
    let option = |call: &mut dyn Call<H>, key: &str| match options {
        Some(Value::Js(options)) => call.get(options, key),
        _ => Ok(Value::Undefined),
    };
    let size = |value: Value<H>| match value {
        Value::Number(n) if (1.0..=SIDE).contains(&n) => Ok(Some(n.trunc())),
        Value::Undefined => Ok(None),
        _ => Err(Error::Dom("InvalidStateError", "createImageBitmap: the size is out of range".into())),
    };
    let (resize_width, resize_height) = (size(option(call, "resizeWidth")?)?, size(option(call, "resizeHeight")?)?);
    // One given size keeps the crop's proportions.
    let (width, height) = match (resize_width, resize_height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, (w * crop.height() / crop.width()).ceil()),
        (None, Some(h)) => ((h * crop.width() / crop.height()).ceil(), h),
        (None, None) => (crop.width(), crop.height()),
    };
    if !(1.0..=SIDE).contains(&width) || !(1.0..=SIDE).contains(&height) || width * height > AREA {
        return Err(Error::Dom("InvalidStateError", "createImageBitmap: the bitmap would be too large".into()));
    }
    let flip = matches!(option(call, "imageOrientation")?, Value::String(o) if o == "flipY");
    let smooth = !matches!(option(call, "resizeQuality")?, Value::String(q) if q == "pixelated");
    let (width, height) = (width as u16, height as u16);
    let mut transform =
        Affine::scale_non_uniform(f64::from(width) / crop.width(), f64::from(height) / crop.height()) * Affine::translate((-crop.x0, -crop.y0));
    if flip {
        transform = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, f64::from(height)]) * transform;
    }
    let pixmap = if transform == Affine::IDENTITY && (width, height) == (pixels.width(), pixels.height()) {
        Arc::unwrap_or_clone(pixels)
    } else {
        render(pixels, width, height, transform, smooth)
    };
    let bitmap = new_bitmap(call, pixmap)?;
    let promise = call.host()?.borrow().canvases.as_ref().map(|c| c.promise.clone()).ok_or_else(not_set_up)?;
    method(call, &promise, "resolve", &[bitmap])
}

fn not_set_up() -> Error {
    Error::Script("ImageBitmap is not set up".into())
}

/// The pixels of a `Blob`'s image, an `ImageData`, a canvas or an `ImageBitmap`.
fn pixels<H: Clone>(call: &mut dyn Call<H>, source: &H) -> Result<Arc<Pixmap>, Error> {
    #[cfg(not(target_arch = "wasm32"))]
    if crate::fetch::is_blob(call, source)? {
        let bytes = crate::fetch::blob_bytes(call, &Value::Js(source.clone()))?;
        return decode(&bytes).map(Arc::new).map_err(|e| Error::Dom("InvalidStateError", format!("createImageBitmap: {e}")));
    }
    if let Value::Js(data) = call.get(source, "data")? {
        let (w, h) = (call.get(source, "width")?, call.get(source, "height")?);
        let (Value::Number(w), Value::Number(h)) = (w, h) else {
            return Err(Error::Type("createImageBitmap: not an ImageData".into()));
        };
        let bytes = call.bytes(&data)?;
        if !(w >= 0.0 && h >= 0.0 && w <= SIDE && h <= SIDE) || bytes.len() as f64 != w * h * 4.0 {
            return Err(Error::Type("createImageBitmap: not an ImageData".into()));
        }
        let data = bytes.as_chunks::<4>().0.iter().map(|p| premultiply(*p)).collect();
        return Ok(Arc::new(Pixmap::from_parts(data, w as u16, h as u16)));
    }
    crate::canvas::image_pixels(call, &Value::Js(source.clone()))
}

/// The image in `bytes`, in any format the app reads, within the size a canvas may have.
#[cfg(not(target_arch = "wasm32"))]
fn decode(bytes: &[u8]) -> Result<Pixmap, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(SIDE as u32);
    limits.max_image_height = Some(SIDE as u32);
    limits.max_alloc = Some((AREA * 8.0) as u64);
    reader.limits(limits);
    let rgba = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (width, height) = (u16::try_from(rgba.width()).map_err(|e| e.to_string())?, u16::try_from(rgba.height()).map_err(|e| e.to_string())?);
    if f64::from(width) * f64::from(height) > AREA {
        return Err("the image is too large".into());
    }
    let data = rgba.pixels().map(|p| premultiply(p.0)).collect();
    Ok(Pixmap::from_parts(data, width, height))
}

/// `pixels` drawn into a `width` × `height` pixmap through `transform`, transparent where they
/// don't reach.
fn render(pixels: Arc<Pixmap>, width: u16, height: u16, transform: Affine, smooth: bool) -> Pixmap {
    let source = Rect::new(0.0, 0.0, f64::from(pixels.width()), f64::from(pixels.height()));
    let settings = RenderSettings { num_threads: 0, ..RenderSettings::default() };
    let mut scene = RenderContext::new_with(width, height, settings);
    let quality = if smooth { ImageQuality::Medium } else { ImageQuality::Low };
    let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality, alpha: 1.0 };
    scene.set_paint_transform(transform);
    scene.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(pixels), sampler });
    scene.fill_path(&(transform * source.to_path(0.1)));
    scene.flush();
    let mut pixmap = Pixmap::new(width, height);
    scene.render(&mut pixmap, &mut Resources::default());
    pixmap
}

/// A new `ImageBitmap` of `pixmap`.
fn new_bitmap<H: Clone>(call: &mut dyn Call<H>, pixmap: Pixmap) -> Result<Value<H>, Error> {
    let (object_class, prototype, registry) = {
        let host = call.host()?;
        let host = host.borrow();
        let canvases = host.canvases.as_ref().ok_or_else(not_set_up)?;
        (canvases.object.clone(), canvases.bitmap_prototype.clone(), canvases.registry.clone())
    };
    let Value::Js(bitmap) = method(call, &object_class, "create", &[Value::Js(prototype)])? else {
        return Err(Error::Script("Object.create made nothing".into()));
    };
    let number = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        let canvases = host.canvases.as_mut().ok_or_else(not_set_up)?;
        let mut surface = Surface::new(0, 0);
        surface.pixmap = pixmap;
        store(&mut canvases.surfaces, surface)
    };
    call.set(&bitmap, "_canvas", &Value::Number(number as f64))?;
    if let Some(registry) = registry {
        method(call, &registry, "register", &[Value::Js(bitmap.clone()), Value::Number(number as f64)])?;
    }
    Ok(Value::Js(bitmap))
}

/// `bitmap.width` or `height` (`horizontal`): 0 once it's closed.
pub(crate) fn side<H>(call: &mut dyn Call<H>, this: &Value<H>, horizontal: bool) -> Result<Value<H>, Error> {
    let side = crate::canvas::with_surface(call, this, |s| if horizontal { s.pixmap.width() } else { s.pixmap.height() }).unwrap_or(0);
    Ok(Value::Number(f64::from(side)))
}

/// `bitmap.close()`: frees its pixels; drawing it throws from then on.
pub(crate) fn close<H>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(bitmap) = this else {
        return Ok(Value::Undefined);
    };
    if let Value::Number(number) = call.get(bitmap, "_canvas")? {
        if let Some(slot) = call.host()?.borrow_mut().canvases.as_mut().and_then(|c| c.surfaces.get_mut(number as usize)) {
            *slot = None;
        }
        call.set(bitmap, "_canvas", &Value::Number(-1.0))?;
    }
    Ok(Value::Undefined)
}
