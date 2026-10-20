//! Drawing on a canvas, as the browser's `CanvasRenderingContext2D` does, with vello_cpu: paths
//! (`moveTo` … `roundRect`), `fill`, `stroke`, `clip`, the rectangles, text (`fillText`,
//! `strokeText`, with the app's fonts), images (`drawImage` of a canvas), transforms,
//! `save`/`restore`, line styles and dashes, colours, gradients and patterns, `globalAlpha` and
//! `globalCompositeOperation`.
//!
//! A canvas collects what it draws in a vello_cpu scene and renders it onto its pixels only when
//! they're read ([`Surface::flush`]): drawing many paths costs one render. As the standard has it,
//! a path's points are transformed when they're added, a stroke's width by the transform when it's
//! stroked, and gradients and patterns are in the coordinates of the transform when they paint.
//! The context's properties (`fillStyle`, `lineWidth`, …) are the canvas's own JavaScript
//! properties; `save()` keeps them with the transform, clip and dashes.

use std::str::FromStr;
use std::sync::Arc;

use vello_cpu::kurbo::{Affine, Arc as ArcSegment, BezPath, Cap, Join, PathEl, Point, Rect, Shape, Stroke, Vec2};
use vello_cpu::peniko::{self, BlendMode, Color, ColorStop, Compose, Extend, Fill, ImageQuality, ImageSampler, Mix};
use vello_cpu::{Pixmap, RenderContext, Resources};

use crate::{Call, Error, Value};

/// The context's properties `save()` keeps, with their values on a new canvas.
pub(crate) const PROPERTIES: [(&str, Initial); 18] = [
    ("fillStyle", Initial::Text("#000000")),
    ("strokeStyle", Initial::Text("#000000")),
    ("lineWidth", Initial::Number(1.0)),
    ("lineCap", Initial::Text("butt")),
    ("lineJoin", Initial::Text("miter")),
    ("miterLimit", Initial::Number(10.0)),
    ("lineDashOffset", Initial::Number(0.0)),
    ("globalAlpha", Initial::Number(1.0)),
    ("globalCompositeOperation", Initial::Text("source-over")),
    ("font", Initial::Text(crate::canvas::DEFAULT_FONT)),
    ("textAlign", Initial::Text("start")),
    ("textBaseline", Initial::Text("alphabetic")),
    ("direction", Initial::Text("inherit")),
    ("imageSmoothingEnabled", Initial::Bool(true)),
    ("imageSmoothingQuality", Initial::Text("low")),
    ("shadowBlur", Initial::Number(0.0)),
    ("shadowColor", Initial::Text("rgba(0, 0, 0, 0)")),
    ("letterSpacing", Initial::Text("0px")),
];

#[derive(Clone, Copy)]
pub(crate) enum Initial {
    Text(&'static str),
    Number(f64),
    Bool(bool),
}

impl Initial {
    pub(crate) fn value<H>(self) -> Value<H> {
        match self {
            Initial::Text(text) => Value::String(text.into()),
            Initial::Number(n) => Value::Number(n),
            Initial::Bool(b) => Value::Bool(b),
        }
    }
}

/// A canvas's pixels and drawing state.
pub(crate) struct Surface<H> {
    pub pixmap: Pixmap,
    /// What's drawn since the pixels were last rendered.
    scene: Option<RenderContext>,
    /// The current path, in pixels.
    path: BezPath,
    transform: Affine,
    /// The clip paths, in pixels; drawing is inside all of them.
    clips: Vec<(BezPath, Fill)>,
    dash: Vec<f64>,
    saved: Vec<Saved<H>>,
}

/// What `save()` keeps.
struct Saved<H> {
    transform: Affine,
    clips: Vec<(BezPath, Fill)>,
    dash: Vec<f64>,
    properties: Vec<Value<H>>,
}

impl<H> Surface<H> {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            pixmap: Pixmap::new(width, height),
            scene: None,
            path: BezPath::new(),
            transform: Affine::IDENTITY,
            clips: Vec::new(),
            dash: Vec::new(),
            saved: Vec::new(),
        }
    }

    /// Renders what's drawn onto the pixels.
    pub(crate) fn flush(&mut self) {
        if let Some(mut scene) = self.scene.take() {
            scene.flush();
            scene.render(&mut self.pixmap, &mut Resources::default());
        }
    }

    /// Cleared to `width` × `height`, its state reset, as setting a canvas's size does.
    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        *self = Self::new(width, height);
    }

    /// The scene, which starts from the pixels as they are: composite operations other than
    /// `source-over` need what's there.
    fn scene(&mut self) -> &mut RenderContext {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let pixmap = &self.pixmap;
        self.scene.get_or_insert_with(|| {
            let mut scene = RenderContext::new(width, height);
            // Pixel for pixel: unscaled, sampled at the pixels' centres.
            let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::Low, alpha: 1.0 };
            scene.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(Arc::new(pixmap.clone())), sampler });
            scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
            scene
        })
    }

    /// Draws with `draw` inside the clips, with `alpha` and `blend`.
    fn draw(&mut self, alpha: f32, blend: BlendMode, draw: impl FnOnce(&mut RenderContext)) {
        if self.pixmap.width() == 0 || self.pixmap.height() == 0 {
            return;
        }
        let clips = self.clips.clone();
        let scene = self.scene();
        for (clip, rule) in &clips {
            scene.set_fill_rule(*rule);
            scene.push_clip_layer(clip);
        }
        let layered = alpha < 1.0 || blend != BlendMode::default();
        if layered {
            scene.push_layer(None, Some(blend), Some(alpha), None, None);
        }
        draw(scene);
        if layered {
            scene.pop_layer();
        }
        for _ in &clips {
            scene.pop_layer();
        }
    }
}

/// The surface of the canvas `this` is, for `f`.
fn surface<H: Clone, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Surface<H>) -> R) -> Result<R, Error> {
    crate::canvas::with_surface(call, this, f)
}

fn number<H>(args: &[Value<H>], i: usize) -> f64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n,
        _ => f64::NAN,
    }
}

/// `args[from..from + N]` as numbers, when they are all finite (else the call does nothing, as
/// in browsers).
fn finite<const N: usize, H>(args: &[Value<H>], from: usize) -> Option<[f64; N]> {
    let mut out = [0.0; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = number(args, from + i);
    }
    out.iter().all(|n| n.is_finite()).then_some(out)
}

/// A property of the context `this` is.
fn property<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Value<H>, Error> {
    match this {
        Value::Js(context) => call.get(context, key),
        _ => Err(Error::Script("not a canvas".into())),
    }
}

fn text_property<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<String, Error> {
    match property(call, this, key)? {
        Value::String(text) => Ok(text),
        other => call.to_string(&other),
    }
}

fn number_property<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, default: f64) -> Result<f64, Error> {
    Ok(match property(call, this, key)? {
        Value::Number(n) if n.is_finite() => n,
        _ => default,
    })
}

// Styles.

/// `fillStyle` or `strokeStyle` (`key`), as the canvas keeps it: a colour in its normalized form,
/// or a gradient or pattern.
pub(crate) fn get_style<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Value<H>, Error> {
    property(call, this, key)
}

/// Setting `fillStyle` or `strokeStyle` (kept in `key`): a CSS colour, a `CanvasGradient` or a
/// `CanvasPattern`; anything else is ignored, as in browsers.
pub(crate) fn set_style<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, value: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let Value::Js(context) = this else {
        return Err(Error::Script("not a canvas".into()));
    };
    let kept = match value {
        Some(Value::Js(object))
            if matches!(call.get(object, "_gradient")?, Value::String(_)) || matches!(call.get(object, "_image")?, Value::Js(_)) =>
        {
            Some(Value::Js(object.clone()))
        }
        Some(Value::String(css)) => svgtypes::Color::from_str(css).ok().map(|c| Value::String(serialize(c))),
        _ => None,
    };
    if let Some(kept) = kept {
        call.set(context, key, &kept)?;
    }
    Ok(Value::Undefined)
}

/// A colour as browsers give a canvas style back: `#rrggbb`, or `rgba(r, g, b, a)` when it's not
/// opaque.
fn serialize(c: svgtypes::Color) -> String {
    if c.alpha == 255 {
        format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue)
    } else {
        let alpha = (f64::from(c.alpha) / 255.0 * 1000.0).round() / 1000.0;
        format!("rgba({}, {}, {}, {alpha})", c.red, c.green, c.blue)
    }
}

// Paths.

pub(crate) fn begin_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| s.path = BezPath::new())?;
    Ok(Value::Undefined)
}

pub(crate) fn close_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| {
        if !s.path.elements().is_empty() {
            s.path.close_path();
        }
    })?;
    Ok(Value::Undefined)
}

/// The current point of a path, where a subpath starts or the last segment ends.
fn current(path: &BezPath) -> Option<Point> {
    let mut start = None;
    let mut last = None;
    for element in path.elements() {
        match element {
            PathEl::MoveTo(p) => {
                start = Some(*p);
                last = Some(*p);
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => last = Some(*p),
            PathEl::ClosePath => last = start,
        }
    }
    last
}

/// Adds `points` (in user space) as a line, or a move when the path has no point yet.
fn line_to(s: &mut Surface<impl Sized>, p: Point) {
    let p = s.transform * p;
    if current(&s.path).is_some() {
        s.path.line_to(p);
    } else {
        s.path.move_to(p);
    }
}

pub(crate) fn move_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y]) = finite(args, 0) {
        surface(call, this, |s| {
            let p = s.transform * Point::new(x, y);
            s.path.move_to(p);
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn line_to_method<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y]) = finite(args, 0) {
        surface(call, this, |s| line_to(s, Point::new(x, y)))?;
    }
    Ok(Value::Undefined)
}

/// Starts a subpath at `p` (user space) when there is no current point.
fn ensure_start(s: &mut Surface<impl Sized>, p: Point) {
    if current(&s.path).is_none() {
        let p = s.transform * p;
        s.path.move_to(p);
    }
}

pub(crate) fn bezier_curve_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x1, y1, x2, y2, x, y]) = finite(args, 0) {
        surface(call, this, |s| {
            ensure_start(s, Point::new(x1, y1));
            let t = s.transform;
            s.path.curve_to(t * Point::new(x1, y1), t * Point::new(x2, y2), t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn quadratic_curve_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x1, y1, x, y]) = finite(args, 0) {
        surface(call, this, |s| {
            ensure_start(s, Point::new(x1, y1));
            let t = s.transform;
            s.path.quad_to(t * Point::new(x1, y1), t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        surface(call, this, |s| {
            let t = s.transform;
            s.path.move_to(t * Point::new(x, y));
            s.path.line_to(t * Point::new(x + w, y));
            s.path.line_to(t * Point::new(x + w, y + h));
            s.path.line_to(t * Point::new(x, y + h));
            s.path.close_path();
            s.path.move_to(t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

/// An elliptical arc around `center` with `radii`, rotated by `rotation`, from `start` to `end`
/// (radians, clockwise in y-down space unless `counterclockwise`), joined to the current point
/// by a line, as `arc()` and `ellipse()` add it.
fn add_arc(s: &mut Surface<impl Sized>, center: Point, radii: Vec2, rotation: f64, start: f64, end: f64, counterclockwise: bool) {
    use std::f64::consts::TAU;
    let mut sweep = end - start;
    if counterclockwise {
        if sweep <= -TAU || (start - end).rem_euclid(TAU) == 0.0 && start != end {
            sweep = -TAU;
        } else {
            sweep = -((start - end).rem_euclid(TAU));
        }
    } else if sweep >= TAU || (end - start).rem_euclid(TAU) == 0.0 && start != end {
        sweep = TAU;
    } else {
        sweep = (end - start).rem_euclid(TAU);
    }
    let arc = ArcSegment::new(center, radii, start, sweep, rotation);
    let t = s.transform;
    let mut first = true;
    for element in arc.path_elements(0.1) {
        match element {
            PathEl::MoveTo(p) => {
                if first {
                    let p = t * p;
                    if current(&s.path).is_some() {
                        s.path.line_to(p);
                    } else {
                        s.path.move_to(p);
                    }
                    first = false;
                }
            }
            PathEl::LineTo(p) => s.path.line_to(t * p),
            PathEl::QuadTo(a, p) => s.path.quad_to(t * a, t * p),
            PathEl::CurveTo(a, b, p) => s.path.curve_to(t * a, t * b, t * p),
            PathEl::ClosePath => {}
        }
    }
}

pub(crate) fn arc<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, r, start, end]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if r < 0.0 {
        return Err(Error::Script("IndexSizeError: arc: the radius is negative".into()));
    }
    let counterclockwise = matches!(args.get(5), Some(Value::Bool(true)));
    surface(call, this, |s| add_arc(s, Point::new(x, y), Vec2::new(r, r), 0.0, start, end, counterclockwise))?;
    Ok(Value::Undefined)
}

pub(crate) fn ellipse<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, rx, ry, rotation, start, end]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if rx < 0.0 || ry < 0.0 {
        return Err(Error::Script("IndexSizeError: ellipse: a radius is negative".into()));
    }
    let counterclockwise = matches!(args.get(7), Some(Value::Bool(true)));
    surface(call, this, |s| add_arc(s, Point::new(x, y), Vec2::new(rx, ry), rotation, start, end, counterclockwise))?;
    Ok(Value::Undefined)
}

/// `arcTo(x1, y1, x2, y2, radius)`: a line towards `(x1, y1)`, then an arc of `radius` tangent to
/// it and to the line on to `(x2, y2)`.
pub(crate) fn arc_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x1, y1, x2, y2, r]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if r < 0.0 {
        return Err(Error::Script("IndexSizeError: arcTo: the radius is negative".into()));
    }
    surface(call, this, |s| {
        let (p1, p2) = (Point::new(x1, y1), Point::new(x2, y2));
        // The current point, back in user space.
        let Some(p0) = current(&s.path).map(|p| s.transform.inverse() * p) else {
            let p = s.transform * p1;
            s.path.move_to(p);
            return;
        };
        let (v1, v2) = ((p0 - p1).normalize(), (p2 - p1).normalize());
        let cross = v1.cross(v2);
        if r == 0.0 || !cross.is_finite() || cross.abs() < 1e-12 || p0 == p1 || p1 == p2 {
            line_to(s, p1);
            return;
        }
        let angle = v1.dot(v2).clamp(-1.0, 1.0).acos();
        let distance = r / (angle / 2.0).tan();
        let (t1, t2) = (p1 + v1 * distance, p1 + v2 * distance);
        let center_direction = (v1 + v2).normalize();
        let center = p1 + center_direction * (r / (angle / 2.0).sin());
        let (start, end) = ((t1 - center).atan2(), (t2 - center).atan2());
        // The arc turns the way the corner does.
        add_arc(s, center, Vec2::new(r, r), 0.0, start, end, cross > 0.0);
    })?;
    Ok(Value::Undefined)
}

/// `roundRect(x, y, w, h, radii)`: a rectangle with rounded corners, the radii a number or a list
/// of one to four (top-left, top-right, bottom-right, bottom-left, as CSS has them).
pub(crate) fn round_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, w, h]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    let radii: Vec<f64> = match args.get(4) {
        Some(Value::Number(r)) => vec![*r],
        Some(Value::Js(list)) => {
            let count = match call.get(list, "length")? {
                Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
                _ => 0,
            };
            (0..count.min(4))
                .map(|i| call.get(list, &i.to_string()).map(|v| if let Value::Number(n) = v { n } else { 0.0 }))
                .collect::<Result<_, _>>()?
        }
        _ => vec![0.0],
    };
    if radii.iter().any(|r| !r.is_finite() || *r < 0.0) {
        return Err(Error::Script("RangeError: roundRect: a radius is negative".into()));
    }
    let [tl, tr, br, bl] = match radii.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => [0.0; 4],
    };
    // Radii that don't fit shrink together, as the standard says.
    let fit = [(tl + tr) / w.abs(), (bl + br) / w.abs(), (tl + bl) / h.abs(), (tr + br) / h.abs()]
        .into_iter()
        .filter(|f| f.is_finite())
        .fold(1.0f64, f64::max);
    let [tl, tr, br, bl] = [tl / fit, tr / fit, br / fit, bl / fit];
    let rounded = vello_cpu::kurbo::RoundedRect::from_rect(Rect::new(x, y, x + w, y + h).abs(), (tl, tr, br, bl));
    surface(call, this, |s| {
        let t = s.transform;
        s.path.extend(t * rounded.to_path(0.1));
        s.path.move_to(t * Point::new(x, y));
    })?;
    Ok(Value::Undefined)
}

// Transforms.

fn set_transform_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(Affine) -> Affine) -> Result<Value<H>, Error> {
    surface(call, this, |s| {
        let next = f(s.transform);
        if next.as_coeffs().iter().all(|c| c.is_finite()) {
            s.transform = next;
        }
    })?;
    Ok(Value::Undefined)
}

pub(crate) fn translate<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    match finite(args, 0) {
        Some([x, y]) => set_transform_to(call, this, |t| t * Affine::translate((x, y))),
        None => Ok(Value::Undefined),
    }
}

pub(crate) fn rotate<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    match finite(args, 0) {
        Some([angle]) => set_transform_to(call, this, |t| t * Affine::rotate(angle)),
        None => Ok(Value::Undefined),
    }
}

pub(crate) fn scale<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    match finite(args, 0) {
        Some([x, y]) => set_transform_to(call, this, |t| t * Affine::scale_non_uniform(x, y)),
        None => Ok(Value::Undefined),
    }
}

pub(crate) fn transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    match finite(args, 0) {
        Some(m) => set_transform_to(call, this, |t| t * Affine::new(m)),
        None => Ok(Value::Undefined),
    }
}

/// `setTransform(a, b, c, d, e, f)`, or `setTransform(matrix)` with `a` … `f`, or none: the
/// identity.
pub(crate) fn set_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let m = match args.first() {
        None | Some(Value::Undefined) => Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        Some(Value::Js(matrix)) => {
            let mut m = [0.0; 6];
            for (slot, key) in m.iter_mut().zip(["a", "b", "c", "d", "e", "f"]) {
                *slot = match call.get(matrix, key)? {
                    Value::Number(n) => n,
                    _ => f64::NAN,
                };
            }
            m.iter().all(|n| n.is_finite()).then_some(m)
        }
        _ => finite(args, 0),
    };
    match m {
        Some(m) => set_transform_to(call, this, |_| Affine::new(m)),
        None => Ok(Value::Undefined),
    }
}

pub(crate) fn reset_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    set_transform_to(call, this, |_| Affine::IDENTITY)
}

/// `getTransform()`: `{ a, b, c, d, e, f }`, as a `DOMMatrix` has them.
pub(crate) fn get_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let m = surface(call, this, |s| s.transform.as_coeffs())?;
    let matrix = call.new_object()?;
    for (key, value) in ["a", "b", "c", "d", "e", "f"].into_iter().zip(m) {
        call.set(&matrix, key, &Value::Number(value))?;
    }
    call.set(&matrix, "is2D", &Value::Bool(true))?;
    Ok(Value::Js(matrix))
}

// State.

pub(crate) fn save<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut properties = Vec::with_capacity(PROPERTIES.len());
    for (key, _) in PROPERTIES {
        properties.push(property(call, this, key)?);
    }
    surface(call, this, |s| {
        let saved = Saved { transform: s.transform, clips: s.clips.clone(), dash: s.dash.clone(), properties };
        s.saved.push(saved);
    })?;
    Ok(Value::Undefined)
}

pub(crate) fn restore<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let saved = surface(call, this, |s| {
        let saved = s.saved.pop()?;
        s.transform = saved.transform;
        s.clips = saved.clips;
        s.dash = saved.dash;
        Some(saved.properties)
    })?;
    if let (Some(properties), Value::Js(context)) = (saved, this) {
        for ((key, _), value) in PROPERTIES.iter().zip(properties) {
            call.set(context, key, &value)?;
        }
    }
    Ok(Value::Undefined)
}

/// `reset()`: as a new canvas, its pixels cleared.
pub(crate) fn reset<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| {
        let (width, height) = (s.pixmap.width(), s.pixmap.height());
        s.resize(width, height);
    })?;
    if let Value::Js(context) = this {
        for (key, default) in PROPERTIES {
            call.set(context, key, &default.value())?;
        }
    }
    Ok(Value::Undefined)
}

pub(crate) fn set_line_dash<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(list)) = args.first() else {
        return Ok(Value::Undefined);
    };
    let count = match call.get(list, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => (n as usize).min(1024),
        _ => 0,
    };
    let mut dash = Vec::with_capacity(count);
    for i in 0..count {
        match call.get(list, &i.to_string())? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => dash.push(n),
            // A list with anything else is ignored, as in browsers.
            _ => return Ok(Value::Undefined),
        }
    }
    // An odd list repeats, as in browsers.
    if dash.len() % 2 == 1 {
        dash.extend(dash.clone());
    }
    surface(call, this, |s| s.dash = dash)?;
    Ok(Value::Undefined)
}

pub(crate) fn get_line_dash<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let dash = surface(call, this, |s| s.dash.clone())?;
    let values: Vec<_> = dash.into_iter().map(Value::Number).collect();
    Ok(Value::Js(call.new_array(&values)?))
}

// Painting.

/// A `fillStyle` or `strokeStyle`: a colour, or a gradient or pattern object.
enum Style {
    Color(Color),
    Gradient(peniko::Gradient),
    Pattern(Arc<Pixmap>, Extend, Extend),
}

/// The style in property `key`, `None` when it paints nothing.
fn style<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Option<Style>, Error> {
    match property(call, this, key)? {
        Value::String(css) => Ok(svgtypes::Color::from_str(&css).ok().map(|c| Style::Color(Color::from_rgba8(c.red, c.green, c.blue, c.alpha)))),
        Value::Js(object) => {
            if let Value::String(kind) = call.get(&object, "_gradient")? {
                return gradient(call, &object, &kind).map(|g| g.map(Style::Gradient));
            }
            if let Value::Js(image) = call.get(&object, "_image")? {
                let repetition = match call.get(&object, "_repetition")? {
                    Value::String(r) => r,
                    _ => "repeat".into(),
                };
                let (x, y) = match repetition.as_str() {
                    "repeat-x" => (Extend::Repeat, Extend::Pad),
                    "repeat-y" => (Extend::Pad, Extend::Repeat),
                    "no-repeat" => (Extend::Pad, Extend::Pad),
                    _ => (Extend::Repeat, Extend::Repeat),
                };
                let pixels = crate::canvas::image_pixels(call, &Value::Js(image))?;
                return Ok(Some(Style::Pattern(pixels, x, y)));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// The gradient a `CanvasGradient` object stands for.
fn gradient<H: Clone>(call: &mut dyn Call<H>, object: &H, kind: &str) -> Result<Option<peniko::Gradient>, Error> {
    let numbers = |call: &mut dyn Call<H>, key: &str| -> Result<Vec<f64>, Error> {
        let Value::Js(list) = call.get(object, key)? else {
            return Ok(Vec::new());
        };
        let count = match call.get(&list, "length")? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => (n as usize).min(4096),
            _ => 0,
        };
        (0..count).map(|i| call.get(&list, &i.to_string()).map(|v| if let Value::Number(n) = v { n } else { f64::NAN })).collect()
    };
    let at = numbers(call, "_at")?;
    let offsets = numbers(call, "_offsets")?;
    let Value::Js(colors) = call.get(object, "_colors")? else {
        return Ok(None);
    };
    let mut stops = Vec::with_capacity(offsets.len());
    for (i, offset) in offsets.iter().enumerate() {
        let css = match call.get(&colors, &i.to_string())? {
            Value::String(css) => css,
            _ => continue,
        };
        if let Ok(c) = svgtypes::Color::from_str(&css) {
            stops.push(ColorStop::from((*offset as f32, Color::from_rgba8(c.red, c.green, c.blue, c.alpha))));
        }
    }
    if stops.is_empty() {
        return Ok(None);
    }
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    let gradient = match (kind, at.as_slice()) {
        ("linear", [x0, y0, x1, y1]) => peniko::Gradient::new_linear((*x0, *y0), (*x1, *y1)),
        ("radial", [x0, y0, r0, x1, y1, r1]) => peniko::Gradient::new_two_point_radial((*x0, *y0), *r0 as f32, (*x1, *y1), *r1 as f32),
        ("conic", [angle, x, y]) => peniko::Gradient::new_sweep((*x, *y), *angle as f32, (*angle + std::f64::consts::TAU) as f32),
        _ => return Ok(None),
    };
    Ok(Some(gradient.with_stops(stops.as_slice())))
}

/// `globalAlpha` and `globalCompositeOperation`, as vello_cpu has them.
fn compositing<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<(f32, BlendMode), Error> {
    let alpha = number_property(call, this, "globalAlpha", 1.0)?.clamp(0.0, 1.0) as f32;
    let operation = text_property(call, this, "globalCompositeOperation")?;
    let (mix, compose) = match operation.as_str() {
        "source-in" => (Mix::Normal, Compose::SrcIn),
        "source-out" => (Mix::Normal, Compose::SrcOut),
        "source-atop" => (Mix::Normal, Compose::SrcAtop),
        "destination-over" => (Mix::Normal, Compose::DestOver),
        "destination-in" => (Mix::Normal, Compose::DestIn),
        "destination-out" => (Mix::Normal, Compose::DestOut),
        "destination-atop" => (Mix::Normal, Compose::DestAtop),
        "lighter" => (Mix::Normal, Compose::Plus),
        "copy" => (Mix::Normal, Compose::Copy),
        "xor" => (Mix::Normal, Compose::Xor),
        "multiply" => (Mix::Multiply, Compose::SrcOver),
        "screen" => (Mix::Screen, Compose::SrcOver),
        "overlay" => (Mix::Overlay, Compose::SrcOver),
        "darken" => (Mix::Darken, Compose::SrcOver),
        "lighten" => (Mix::Lighten, Compose::SrcOver),
        "color-dodge" => (Mix::ColorDodge, Compose::SrcOver),
        "color-burn" => (Mix::ColorBurn, Compose::SrcOver),
        "hard-light" => (Mix::HardLight, Compose::SrcOver),
        "soft-light" => (Mix::SoftLight, Compose::SrcOver),
        "difference" => (Mix::Difference, Compose::SrcOver),
        "exclusion" => (Mix::Exclusion, Compose::SrcOver),
        "hue" => (Mix::Hue, Compose::SrcOver),
        "saturation" => (Mix::Saturation, Compose::SrcOver),
        "color" => (Mix::Color, Compose::SrcOver),
        "luminosity" => (Mix::Luminosity, Compose::SrcOver),
        _ => (Mix::Normal, Compose::SrcOver),
    };
    Ok((alpha, BlendMode::new(mix, compose)))
}

/// Sets `style` as the paint, its coordinates those of `transform`.
fn paint(scene: &mut RenderContext, style: Style, transform: Affine, smooth: bool) {
    match style {
        Style::Color(color) => {
            scene.reset_paint_transform();
            scene.set_paint(color);
        }
        Style::Gradient(gradient) => {
            scene.set_paint_transform(transform);
            scene.set_paint(gradient);
        }
        Style::Pattern(pixels, x_extend, y_extend) => {
            let quality = if smooth { ImageQuality::Medium } else { ImageQuality::Low };
            let sampler = ImageSampler { x_extend, y_extend, quality, alpha: 1.0 };
            scene.set_paint_transform(transform);
            scene.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(pixels), sampler });
        }
    }
}

/// The fill rule a `fill()` or `clip()` names: `"nonzero"` (the default) or `"evenodd"`.
fn fill_rule<H>(args: &[Value<H>]) -> Fill {
    if args.iter().any(|a| matches!(a, Value::String(r) if r == "evenodd")) { Fill::EvenOdd } else { Fill::NonZero }
}

/// Fills `path` (in pixels) with the style in `key`.
fn fill_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, path: Option<BezPath>, rule: Fill) -> Result<(), Error> {
    let Some(style) = style(call, this, key)? else {
        return Ok(());
    };
    let (alpha, blend) = compositing(call, this)?;
    let smooth = !matches!(property(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        let path = path.unwrap_or_else(|| s.path.clone());
        s.draw(alpha, blend, |scene| {
            scene.reset_transform();
            scene.set_fill_rule(rule);
            paint(scene, style, transform, smooth);
            scene.fill_path(&path);
        });
    })
}

/// The stroke the context's line properties describe, in user space.
fn line<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, dash: &[f64]) -> Result<Option<Stroke>, Error> {
    let width = number_property(call, this, "lineWidth", 1.0)?;
    if width <= 0.0 {
        return Ok(None);
    }
    let cap = match text_property(call, this, "lineCap")?.as_str() {
        "round" => Cap::Round,
        "square" => Cap::Square,
        _ => Cap::Butt,
    };
    let join = match text_property(call, this, "lineJoin")?.as_str() {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    };
    let miter = number_property(call, this, "miterLimit", 10.0)?.max(0.0);
    let offset = number_property(call, this, "lineDashOffset", 0.0)?;
    let mut stroke = Stroke::new(width).with_caps(cap).with_join(join).with_miter_limit(miter);
    if !dash.is_empty() && dash.iter().any(|d| *d > 0.0) {
        stroke = stroke.with_dashes(offset, dash.iter().copied());
    }
    Ok(Some(stroke))
}

/// Strokes `path` (in pixels) with the style in `key`, its line in the current user space.
fn stroke_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, path: Option<BezPath>) -> Result<(), Error> {
    let Some(style) = style(call, this, "strokeStyle")? else {
        return Ok(());
    };
    let dash = surface(call, this, |s| s.dash.clone())?;
    let Some(stroke) = line(call, this, &dash)? else {
        return Ok(());
    };
    let (alpha, blend) = compositing(call, this)?;
    let smooth = !matches!(property(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        // A transform that flattens everything strokes nothing.
        if transform.determinant().abs() < 1e-12 {
            return;
        }
        let path = transform.inverse() * path.unwrap_or_else(|| s.path.clone());
        s.draw(alpha, blend, |scene| {
            scene.set_transform(transform);
            scene.set_stroke(stroke);
            paint(scene, style, Affine::IDENTITY, smooth);
            scene.stroke_path(&path);
        });
    })
}

pub(crate) fn fill<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    fill_path(call, this, "fillStyle", None, fill_rule(args))?;
    Ok(Value::Undefined)
}

pub(crate) fn stroke<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    stroke_path(call, this, None)?;
    Ok(Value::Undefined)
}

/// `clip(fillRule)`: drawing is inside the current path from then on, until `restore()`.
pub(crate) fn clip<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let rule = fill_rule(args);
    surface(call, this, |s| {
        let path = s.path.clone();
        s.clips.push((path, rule));
    })?;
    Ok(Value::Undefined)
}

/// `isPointInPath(x, y, fillRule)`: whether the point (in pixels) is inside the current path.
pub(crate) fn is_point_in_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y]) = finite(args, 0) else {
        return Ok(Value::Bool(false));
    };
    let rule = fill_rule(args);
    let inside = surface(call, this, |s| {
        let winding = s.path.winding(Point::new(x, y));
        match rule {
            Fill::EvenOdd => winding % 2 != 0,
            Fill::NonZero => winding != 0,
        }
    })?;
    Ok(Value::Bool(inside))
}

/// A rectangle in user space, as a closed path in pixels.
fn rect_path<H>(s: &Surface<H>, x: f64, y: f64, w: f64, h: f64) -> BezPath {
    s.transform * Rect::new(x, y, x + w, y + h).to_path(0.1)
}

pub(crate) fn fill_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        let path = surface(call, this, |s| rect_path(s, x, y, w, h))?;
        fill_path(call, this, "fillStyle", Some(path), Fill::NonZero)?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn stroke_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        let path = surface(call, this, |s| rect_path(s, x, y, w, h))?;
        stroke_path(call, this, Some(path))?;
    }
    Ok(Value::Undefined)
}

/// `clearRect(x, y, w, h)`: transparent black there, inside the clips.
pub(crate) fn clear_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        surface(call, this, |s| {
            let path = rect_path(s, x, y, w, h);
            // Removing an opaque rectangle's coverage leaves only it transparent.
            s.draw(1.0, BlendMode::new(Mix::Normal, Compose::DestOut), |scene| {
                scene.reset_transform();
                scene.set_fill_rule(Fill::NonZero);
                scene.set_paint(Color::BLACK);
                scene.fill_path(&path);
            });
        })?;
    }
    Ok(Value::Undefined)
}

// Text.

/// The outlines of `text` at `(x, y)` in user space, placed by `textAlign` and `textBaseline`,
/// in pixels.
fn text_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Option<BezPath>, Error> {
    let text = match args.first() {
        Some(text) => call.to_string(text)?,
        None => return Ok(None),
    };
    let Some([x, y]) = finite(args, 1) else {
        return Ok(None);
    };
    let font = text_property(call, this, "font")?;
    let align = text_property(call, this, "textAlign")?;
    let baseline = text_property(call, this, "textBaseline")?;
    let (outline, measured) = crate::canvas::outline(&text, &font);
    let dx = match align.as_str() {
        "center" => -measured.width / 2.0,
        "right" | "end" => -measured.width,
        _ => 0.0,
    };
    let dy = match baseline.as_str() {
        "top" | "hanging" => measured.ascent,
        "middle" => (measured.ascent - measured.descent) / 2.0,
        "bottom" | "ideographic" => -measured.descent,
        _ => 0.0,
    };
    // `maxWidth` squeezes the text to fit, as in browsers.
    let squeeze = match finite(args, 3) {
        Some([max]) if max > 0.0 && measured.width > max => max / measured.width,
        _ => 1.0,
    };
    let place = Affine::translate((x, y)) * Affine::scale_non_uniform(squeeze, 1.0) * Affine::translate((dx, dy));
    surface(call, this, |s| Some(s.transform * place * outline))
}

pub(crate) fn fill_text<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(path) = text_path(call, this, args)? {
        fill_path(call, this, "fillStyle", Some(path), Fill::NonZero)?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn stroke_text<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(path) = text_path(call, this, args)? {
        stroke_path(call, this, Some(path))?;
    }
    Ok(Value::Undefined)
}

// Images.

/// `drawImage(image, dx, dy)`, `(image, dx, dy, dw, dh)` or `(image, sx, sy, sw, sh, dx, dy, dw,
/// dh)`, the image a canvas.
pub(crate) fn draw_image<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(image) = args.first() else {
        return Err(Error::Script("TypeError: drawImage: no image".into()));
    };
    let pixels = crate::canvas::image_pixels(call, image)?;
    let (iw, ih) = (f64::from(pixels.width()), f64::from(pixels.height()));
    let (source, destination) = match args.len() {
        3 => match finite(args, 1) {
            Some([dx, dy]) => (Rect::new(0.0, 0.0, iw, ih), Rect::new(dx, dy, dx + iw, dy + ih)),
            None => return Ok(Value::Undefined),
        },
        5 => match finite(args, 1) {
            Some([dx, dy, dw, dh]) => (Rect::new(0.0, 0.0, iw, ih), Rect::new(dx, dy, dx + dw, dy + dh)),
            None => return Ok(Value::Undefined),
        },
        _ => match finite(args, 1) {
            Some([sx, sy, sw, sh, dx, dy, dw, dh]) => (Rect::new(sx, sy, sx + sw, sy + sh), Rect::new(dx, dy, dx + dw, dy + dh)),
            None => return Ok(Value::Undefined),
        },
    };
    if source.width() == 0.0 || source.height() == 0.0 || destination.width() == 0.0 || destination.height() == 0.0 {
        return Ok(Value::Undefined);
    }
    let (alpha, blend) = compositing(call, this)?;
    let smooth = !matches!(property(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
    // The source rectangle, clipped to the image, maps onto the destination rectangle.
    let scale = Vec2::new(destination.width() / source.width(), destination.height() / source.height());
    let visible = source.intersect(Rect::new(0.0, 0.0, iw, ih));
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return Ok(Value::Undefined);
    }
    let shown = Rect::new(
        destination.x0 + (visible.x0 - source.x0) * scale.x,
        destination.y0 + (visible.y0 - source.y0) * scale.y,
        destination.x0 + (visible.x1 - source.x0) * scale.x,
        destination.y0 + (visible.y1 - source.y0) * scale.y,
    );
    let image_to_user = Affine::translate((destination.x0, destination.y0))
        * Affine::scale_non_uniform(scale.x, scale.y)
        * Affine::translate((-source.x0, -source.y0));
    surface(call, this, |s| {
        let transform = s.transform;
        let path = transform * shown.to_path(0.1);
        s.draw(alpha, blend, |scene| {
            scene.reset_transform();
            scene.set_fill_rule(Fill::NonZero);
            paint(scene, Style::Pattern(pixels, Extend::Pad, Extend::Pad), transform * image_to_user, smooth);
            scene.fill_path(&path);
        });
    })?;
    Ok(Value::Undefined)
}

/// `createLinearGradient(x0, y0, x1, y1)`, `createRadialGradient(x0, y0, r0, x1, y1, r1)` or
/// `createConicGradient(angle, x, y)`: a `CanvasGradient`, its stops added by `addColorStop()`.
pub(crate) fn create_gradient<H: Clone>(call: &mut dyn Call<H>, kind: &str, args: &[Value<H>], count: usize) -> Result<Value<H>, Error> {
    let at: Vec<Value<H>> = (0..count).map(|i| Value::Number(number(args, i))).collect();
    if at.iter().any(|v| matches!(v, Value::Number(n) if !n.is_finite())) {
        return Err(Error::Script(format!("TypeError: create{kind}Gradient: not finite")));
    }
    let gradient = crate::canvas::new_gradient(call)?;
    let at = call.new_array(&at)?;
    let (offsets, colors) = (call.new_array(&[])?, call.new_array(&[])?);
    for (key, value) in
        [("_gradient", Value::String(kind.into())), ("_at", Value::Js(at)), ("_offsets", Value::Js(offsets)), ("_colors", Value::Js(colors))]
    {
        call.set(&gradient, key, &value)?;
    }
    Ok(Value::Js(gradient))
}

/// `gradient.addColorStop(offset, color)`.
pub(crate) fn add_color_stop<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(gradient) = this else {
        return Err(Error::Script("not a CanvasGradient".into()));
    };
    let offset = number(args, 0);
    if !(0.0..=1.0).contains(&offset) {
        return Err(Error::Script("IndexSizeError: addColorStop: the offset isn't between 0 and 1".into()));
    }
    let color = match args.get(1) {
        Some(color) => call.to_string(color)?,
        None => String::new(),
    };
    if svgtypes::Color::from_str(&color).is_err() {
        return Err(Error::Script(format!("SyntaxError: addColorStop: '{color}' isn't a colour")));
    }
    for (key, value) in [("_offsets", Value::Number(offset)), ("_colors", Value::String(color))] {
        if let Value::Js(list) = call.get(gradient, key)? {
            crate::method(call, &list, "push", &[value])?;
        }
    }
    Ok(Value::Undefined)
}

/// `createPattern(image, repetition)`: a `CanvasPattern` of a canvas.
pub(crate) fn create_pattern<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(image) = args.first() else {
        return Err(Error::Script("TypeError: createPattern: no image".into()));
    };
    // The image's pixels as they are now.
    crate::canvas::image_pixels(call, image)?;
    let repetition = match args.get(1) {
        None | Some(Value::Null | Value::Undefined) => "repeat".to_owned(),
        Some(r) => call.to_string(r)?,
    };
    let repetition = if repetition.is_empty() { "repeat".to_owned() } else { repetition };
    if !matches!(repetition.as_str(), "repeat" | "repeat-x" | "repeat-y" | "no-repeat") {
        return Err(Error::Script(format!("SyntaxError: createPattern: '{repetition}' isn't a repetition")));
    }
    let pattern = call.new_object()?;
    call.set(&pattern, "_image", image)?;
    call.set(&pattern, "_repetition", &Value::String(repetition))?;
    Ok(Value::Js(pattern))
}
