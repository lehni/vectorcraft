//! Drawing on a canvas, as the browser's `CanvasRenderingContext2D` does, with vello_cpu: paths
//! (`moveTo` … `roundRect`, on the context and on `Path2D`s), `fill`, `stroke`, `clip`,
//! `isPointInPath` and `isPointInStroke`, the rectangles, text (`fillText`,
//! `strokeText`, with the app's fonts), images (`drawImage` of a canvas), transforms,
//! `save`/`restore`, line styles and dashes, colours, gradients and patterns, `globalAlpha`,
//! `globalCompositeOperation`, shadows, and `filter`'s `blur()` and `drop-shadow()` (its other
//! functions are kept but change nothing: vello_cpu has no colour filters yet).
//!
//! A canvas collects what it draws in a vello_cpu scene and renders it onto its pixels only when
//! they're read ([`Surface::flush`]): drawing many paths costs one render. As the standard has it,
//! a path's points are transformed when they're added, a stroke's width by the transform when it's
//! stroked, and gradients and patterns are in the coordinates of the transform when they paint.
//! The context's properties (`fillStyle`, `lineWidth`, …) are the canvas's own JavaScript
//! properties; `save()` keeps them with the transform, clip and dashes.

use std::sync::Arc;

use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_cpu::color::Rgba8;
use vello_cpu::kurbo::{Affine, Arc as ArcSegment, BezPath, Cap, Join, PathEl, Point, Rect, Shape, Stroke, Vec2};
use vello_cpu::peniko::{self, BlendMode, Color, ColorStop, Compose, Extend, Fill, ImageQuality, ImageSampler, Mix};
use vello_cpu::{Pixmap, RenderContext, RenderSettings, Resources};

use crate::{Call, Error, Value};

/// A context property: its name, the key its value is kept under, its value on a new canvas, and
/// the values it takes; others are ignored, as browsers ignore them. `save()` keeps them.
pub(crate) struct Property {
    pub name: &'static str,
    pub key: &'static str,
    pub initial: Initial,
    check: Check,
}

/// CSS Compositing's composite and blend modes, but `plus-darker`, which vello_cpu doesn't have.
const COMPOSITE_OPERATIONS: &[&str] = &[
    "clear",
    "plus-lighter",
    "normal",
    "source-over",
    "source-in",
    "source-out",
    "source-atop",
    "destination-over",
    "destination-in",
    "destination-out",
    "destination-atop",
    "lighter",
    "copy",
    "xor",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "color-dodge",
    "color-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
    "hue",
    "saturation",
    "color",
    "luminosity",
];

const fn property(name: &'static str, key: &'static str, initial: Initial, check: Check) -> Property {
    Property { name, key, initial, check }
}

pub(crate) const PROPERTIES: [Property; 26] = [
    property("fillStyle", "_fillStyle", Initial::Text("#000000"), Check::Style),
    property("strokeStyle", "_strokeStyle", Initial::Text("#000000"), Check::Style),
    property("lineWidth", "_lineWidth", Initial::Number(1.0), Check::Positive),
    property("lineCap", "_lineCap", Initial::Text("butt"), Check::Keyword(&["butt", "round", "square"])),
    property("lineJoin", "_lineJoin", Initial::Text("miter"), Check::Keyword(&["round", "bevel", "miter"])),
    property("miterLimit", "_miterLimit", Initial::Number(10.0), Check::Positive),
    property("lineDashOffset", "_lineDashOffset", Initial::Number(0.0), Check::Finite),
    property("globalAlpha", "_globalAlpha", Initial::Number(1.0), Check::Fraction),
    property("globalCompositeOperation", "_globalCompositeOperation", Initial::Text("source-over"), Check::Keyword(COMPOSITE_OPERATIONS)),
    property("font", "_font", Initial::Text(crate::canvas::DEFAULT_FONT), Check::Font),
    property("textAlign", "_textAlign", Initial::Text("start"), Check::Keyword(&["start", "end", "left", "right", "center"])),
    property(
        "textBaseline",
        "_textBaseline",
        Initial::Text("alphabetic"),
        Check::Keyword(&["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"]),
    ),
    property("direction", "_direction", Initial::Text("inherit"), Check::Keyword(&["ltr", "rtl", "inherit"])),
    property("imageSmoothingEnabled", "_imageSmoothingEnabled", Initial::Bool(true), Check::Bool),
    property("imageSmoothingQuality", "_imageSmoothingQuality", Initial::Text("low"), Check::Keyword(&["low", "medium", "high"])),
    property("shadowBlur", "_shadowBlur", Initial::Number(0.0), Check::NonNegative),
    property("shadowColor", "_shadowColor", Initial::Text("rgba(0, 0, 0, 0)"), Check::Color),
    property("shadowOffsetX", "_shadowOffsetX", Initial::Number(0.0), Check::Finite),
    property("shadowOffsetY", "_shadowOffsetY", Initial::Number(0.0), Check::Finite),
    property("filter", "_filter", Initial::Text("none"), Check::Filter),
    property("letterSpacing", "_letterSpacing", Initial::Text("0px"), Check::Length),
    property("wordSpacing", "_wordSpacing", Initial::Text("0px"), Check::Length),
    property("fontKerning", "_fontKerning", Initial::Text("auto"), Check::Keyword(&["auto", "normal", "none"])),
    property(
        "fontStretch",
        "_fontStretch",
        Initial::Text("normal"),
        Check::Keyword(&[
            "ultra-condensed",
            "extra-condensed",
            "condensed",
            "semi-condensed",
            "normal",
            "semi-expanded",
            "expanded",
            "extra-expanded",
            "ultra-expanded",
        ]),
    ),
    property(
        "fontVariantCaps",
        "_fontVariantCaps",
        Initial::Text("normal"),
        Check::Keyword(&["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"]),
    ),
    property(
        "textRendering",
        "_textRendering",
        Initial::Text("auto"),
        Check::Keyword(&["auto", "optimizeSpeed", "optimizeLegibility", "geometricPrecision"]),
    ),
];

/// The values a property takes.
#[derive(Clone, Copy)]
enum Check {
    /// A finite number above 0.
    Positive,
    /// A finite number.
    Finite,
    /// A finite number, 0 or more.
    NonNegative,
    /// A finite number from 0 to 1.
    Fraction,
    /// Anything, as a boolean.
    Bool,
    /// One of these keywords.
    Keyword(&'static [&'static str]),
    /// A CSS `font` shorthand.
    Font,
    /// A CSS length.
    Length,
    /// A CSS colour.
    Color,
    /// A CSS colour, a `CanvasGradient` or a `CanvasPattern`.
    Style,
    /// A CSS `filter`.
    Filter,
}

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

/// `get(name)`, with the context as `this`: its property `name`.
pub(crate) fn get_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let property = property_named(call, args)?;
    stored(call, this, property.key)
}

/// `set(name, value)`, with the context as `this`: keeps the value as the property has it, when it
/// takes it.
pub(crate) fn set_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let property = property_named(call, args)?;
    let Value::Js(context) = this else {
        return Err(Error::Script("not a canvas".into()));
    };
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    if let Some(kept) = checked(call, property.check, value)? {
        call.set(context, property.key, &kept)?;
    }
    Ok(Value::Undefined)
}

fn property_named<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<&'static Property, Error> {
    let name = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    PROPERTIES.iter().find(|p| p.name == name).ok_or_else(|| Error::Type(format!("Unknown property: '{name}'.")))
}

/// `value` as a property that takes `check` keeps it, or `None` when it doesn't take it.
fn checked<H: Clone>(call: &mut dyn Call<H>, check: Check, value: Value<H>) -> Result<Option<Value<H>>, Error> {
    let number = |call: &mut dyn Call<H>, value: &Value<H>| call.to_number(value);
    Ok(match check {
        Check::Positive => Some(number(call, &value)?).filter(|n| n.is_finite() && *n > 0.0).map(Value::Number),
        Check::Finite => Some(number(call, &value)?).filter(|n| n.is_finite()).map(Value::Number),
        Check::NonNegative => Some(number(call, &value)?).filter(|n| n.is_finite() && *n >= 0.0).map(Value::Number),
        Check::Fraction => Some(number(call, &value)?).filter(|n| (0.0..=1.0).contains(n)).map(Value::Number),
        Check::Bool => Some(Value::Bool(crate::truthy(&value))),
        Check::Keyword(keywords) => {
            let text = call.to_string(&value)?;
            keywords.contains(&text.as_str()).then_some(Value::String(text))
        }
        Check::Font => {
            let text = call.to_string(&value)?;
            crate::canvas::is_font(&text).then_some(Value::String(text))
        }
        Check::Length => {
            let text = call.to_string(&value)?;
            css_length(&text).is_some().then_some(Value::String(text))
        }
        Check::Color => match &value {
            Value::String(css) => color_value(css),
            _ => None,
        },
        Check::Style => match &value {
            Value::Js(object)
                if matches!(call.get(object, "_gradient")?, Value::String(_)) || matches!(call.get(object, "_image")?, Value::Js(_)) =>
            {
                Some(value.clone())
            }
            Value::String(css) => color_value(css),
            _ => None,
        },
        Check::Filter => {
            let text = call.to_string(&value)?;
            parse_filter(&text).is_some().then_some(Value::String(text))
        }
    })
}

/// A CSS length in pixels, `em` and `rem` taken as 10 pixels, the default font's size.
fn css_length(text: &str) -> Option<f64> {
    let text = text.trim();
    if text == "0" {
        return Some(0.0);
    }
    let units = [
        ("px", 1.0),
        ("em", 10.0),
        ("rem", 10.0),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 101.6),
    ];
    units.iter().find_map(|(unit, scale)| text.strip_suffix(unit)?.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * scale))
}

/// Sets the properties of `context` to their values on a new canvas, as `reset()` and resizing do.
pub(crate) fn reset_properties<H>(call: &mut dyn Call<H>, context: &H) -> Result<(), Error> {
    for p in &PROPERTIES {
        call.set(context, p.key, &p.initial.value())?;
    }
    Ok(())
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
            // On the script's thread: the app's renderer turns on vello_cpu's threads, which a
            // canvas doesn't need, and with which filters don't work.
            let settings = RenderSettings { num_threads: 0, ..RenderSettings::default() };
            let mut scene = RenderContext::new_with(width, height, settings);
            // Pixel for pixel: unscaled, sampled at the pixels' centres.
            let sampler = ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::Low, alpha: 1.0 };
            scene.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(Arc::new(pixmap.clone())), sampler });
            scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
            scene
        })
    }

    /// Draws with `draw` inside the clips, as `compositing` has it.
    fn draw(&mut self, compositing: &Compositing, draw: impl FnOnce(&mut RenderContext)) {
        let (width, height) = (f64::from(self.pixmap.width()), f64::from(self.pixmap.height()));
        if width == 0.0 || height == 0.0 {
            return;
        }
        let filters: Vec<Filter> = compositing.effects.iter().filter_map(|e| e.filter(width, height)).collect();
        let clips = self.clips.clone();
        let scene = self.scene();
        // Clips and filters are in pixels.
        scene.reset_transform();
        for (clip, rule) in &clips {
            scene.set_fill_rule(*rule);
            scene.push_clip_layer(clip);
        }
        let layered = compositing.alpha < 1.0 || compositing.blend != BlendMode::default() || !filters.is_empty();
        if layered {
            scene.push_layer(None, Some(compositing.blend), Some(compositing.alpha), None, None);
        }
        // The innermost layer's filter applies first.
        for filter in filters.iter().rev() {
            scene.push_filter_layer(filter.clone());
        }
        draw(scene);
        for _ in &filters {
            scene.pop_layer();
        }
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

/// The value the context `this` is keeps under `key`.
fn stored<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Value<H>, Error> {
    match this {
        Value::Js(context) => call.get(context, key),
        _ => Err(Error::Script("not a canvas".into())),
    }
}

fn text_property<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<String, Error> {
    match stored(call, this, key)? {
        Value::String(text) => Ok(text),
        other => call.to_string(&other),
    }
}

fn number_property<H>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, default: f64) -> Result<f64, Error> {
    Ok(match stored(call, this, key)? {
        Value::Number(n) if n.is_finite() => n,
        _ => default,
    })
}

// Styles.

/// A CSS colour as the canvas keeps it.
fn color_value<H>(css: &str) -> Option<Value<H>> {
    css_color(css).map(|c| Value::String(serialize(c)))
}

/// A colour as browsers give a canvas style back: `#rrggbb`, or `rgba(r, g, b, a)` when it's not
/// opaque.
fn serialize(c: Color) -> String {
    let Rgba8 { r, g, b, a } = c.to_rgba8();
    if a == 255 {
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    // The fewest decimals that give the same 8 bits back, as CSS serializes alpha.
    let exact = f64::from(a) / 255.0;
    let alpha =
        [100.0, 1000.0].into_iter().map(|scale| (exact * scale).round() / scale).find(|alpha| (alpha * 255.0).round() as u8 == a).unwrap_or(exact);
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// A CSS colour (CSS Color 4: names, hex, `rgb()`, `hsl()`, `hwb()`, `lab()`, `oklch()`,
/// `color()` …) in sRGB, rounded to 8 bits a channel as a canvas keeps it; `currentcolor` is
/// black, as for a canvas without an element.
pub(crate) fn css_color(css: &str) -> Option<Color> {
    let css = css.trim();
    if css.eq_ignore_ascii_case("currentcolor") {
        return Some(Color::BLACK);
    }
    let color = vello_cpu::color::parse_color(css).ok()?.to_alpha_color::<vello_cpu::color::Srgb>();
    let Rgba8 { r, g, b, a } = color.to_rgba8();
    Some(Color::from_rgba8(r, g, b, a))
}

// Paths.

/// A path being built: its points go through `transform` as they're added.
pub(crate) struct Pen<'a> {
    pub path: &'a mut BezPath,
    pub transform: Affine,
}

/// What the path methods build: a canvas's current path, in pixels through the context's
/// transform, or a `Path2D`'s, as given.
pub(crate) trait Target {
    fn with<H: Clone, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Pen) -> R) -> Result<R, Error>;
}

/// The path methods on a canvas's context.
pub(crate) struct Context;

impl Target for Context {
    fn with<H: Clone, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Pen) -> R) -> Result<R, Error> {
        surface(call, this, |s| f(&mut Pen { path: &mut s.path, transform: s.transform }))
    }
}

/// The path methods on a `Path2D`.
pub(crate) struct Path2D;

impl Target for Path2D {
    fn with<H: Clone, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Pen) -> R) -> Result<R, Error> {
        crate::path2d::with_path(call, this, |path| f(&mut Pen { path, transform: Affine::IDENTITY }))
    }
}

pub(crate) fn begin_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| s.path = BezPath::new())?;
    Ok(Value::Undefined)
}

pub(crate) fn close_path<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    T::with(call, this, |pen| {
        if !pen.path.elements().is_empty() {
            pen.path.close_path();
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

impl Pen<'_> {
    /// Adds a line to `p`, or a move when the path has no point yet.
    fn line_to(&mut self, p: Point) {
        let p = self.transform * p;
        if current(self.path).is_some() {
            self.path.line_to(p);
        } else {
            self.path.move_to(p);
        }
    }

    /// Starts a subpath at `p` when there is no current point.
    fn ensure_start(&mut self, p: Point) {
        if current(self.path).is_none() {
            self.path.move_to(self.transform * p);
        }
    }

    /// An elliptical arc around `center` with `radii`, rotated by `rotation`, from `start` to
    /// `end` (radians, clockwise in y-down space unless `counterclockwise`), joined to the current
    /// point by a line, as `arc()` and `ellipse()` add it.
    fn arc(&mut self, center: Point, radii: Vec2, rotation: f64, start: f64, end: f64, counterclockwise: bool) {
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
        let t = self.transform;
        let mut first = true;
        for element in arc.path_elements(0.1) {
            match element {
                PathEl::MoveTo(p) => {
                    if first {
                        self.line_to(p);
                        first = false;
                    }
                }
                PathEl::LineTo(p) => self.path.line_to(t * p),
                PathEl::QuadTo(a, p) => self.path.quad_to(t * a, t * p),
                PathEl::CurveTo(a, b, p) => self.path.curve_to(t * a, t * b, t * p),
                PathEl::ClosePath => {}
            }
        }
    }
}

pub(crate) fn move_to<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y]) = finite(args, 0) {
        T::with(call, this, |pen| pen.path.move_to(pen.transform * Point::new(x, y)))?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn line_to<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y]) = finite(args, 0) {
        T::with(call, this, |pen| pen.line_to(Point::new(x, y)))?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn bezier_curve_to<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x1, y1, x2, y2, x, y]) = finite(args, 0) {
        T::with(call, this, |pen| {
            pen.ensure_start(Point::new(x1, y1));
            let t = pen.transform;
            pen.path.curve_to(t * Point::new(x1, y1), t * Point::new(x2, y2), t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn quadratic_curve_to<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x1, y1, x, y]) = finite(args, 0) {
        T::with(call, this, |pen| {
            pen.ensure_start(Point::new(x1, y1));
            let t = pen.transform;
            pen.path.quad_to(t * Point::new(x1, y1), t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn rect<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        T::with(call, this, |pen| {
            let t = pen.transform;
            pen.path.move_to(t * Point::new(x, y));
            pen.path.line_to(t * Point::new(x + w, y));
            pen.path.line_to(t * Point::new(x + w, y + h));
            pen.path.line_to(t * Point::new(x, y + h));
            pen.path.close_path();
            pen.path.move_to(t * Point::new(x, y));
        })?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn arc<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, r, start, end]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if r < 0.0 {
        return Err(Error::Dom("IndexSizeError", "arc: the radius is negative".into()));
    }
    let counterclockwise = matches!(args.get(5), Some(Value::Bool(true)));
    T::with(call, this, |pen| pen.arc(Point::new(x, y), Vec2::new(r, r), 0.0, start, end, counterclockwise))?;
    Ok(Value::Undefined)
}

pub(crate) fn ellipse<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, rx, ry, rotation, start, end]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if rx < 0.0 || ry < 0.0 {
        return Err(Error::Dom("IndexSizeError", "ellipse: a radius is negative".into()));
    }
    let counterclockwise = matches!(args.get(7), Some(Value::Bool(true)));
    T::with(call, this, |pen| pen.arc(Point::new(x, y), Vec2::new(rx, ry), rotation, start, end, counterclockwise))?;
    Ok(Value::Undefined)
}

/// `arcTo(x1, y1, x2, y2, radius)`: a line towards `(x1, y1)`, then an arc of `radius` tangent to
/// it and to the line on to `(x2, y2)`.
pub(crate) fn arc_to<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x1, y1, x2, y2, r]) = finite(args, 0) else {
        return Ok(Value::Undefined);
    };
    if r < 0.0 {
        return Err(Error::Dom("IndexSizeError", "arcTo: the radius is negative".into()));
    }
    T::with(call, this, |pen| {
        let (p1, p2) = (Point::new(x1, y1), Point::new(x2, y2));
        // The current point, back in the coordinates the points come in.
        let Some(p0) = current(pen.path).map(|p| pen.transform.inverse() * p) else {
            pen.path.move_to(pen.transform * p1);
            return;
        };
        let (v1, v2) = ((p0 - p1).normalize(), (p2 - p1).normalize());
        let cross = v1.cross(v2);
        if r == 0.0 || !cross.is_finite() || cross.abs() < 1e-12 || p0 == p1 || p1 == p2 {
            pen.line_to(p1);
            return;
        }
        let angle = v1.dot(v2).clamp(-1.0, 1.0).acos();
        let distance = r / (angle / 2.0).tan();
        let (t1, t2) = (p1 + v1 * distance, p1 + v2 * distance);
        let center_direction = (v1 + v2).normalize();
        let center = p1 + center_direction * (r / (angle / 2.0).sin());
        let (start, end) = ((t1 - center).atan2(), (t2 - center).atan2());
        // The arc turns the way the corner does.
        pen.arc(center, Vec2::new(r, r), 0.0, start, end, cross > 0.0);
    })?;
    Ok(Value::Undefined)
}

/// `roundRect(x, y, w, h, radii)`: a rectangle with rounded corners, the radii a number or a list
/// of one to four (top-left, top-right, bottom-right, bottom-left, as CSS has them).
pub(crate) fn round_rect<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
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
        return Err(Error::Range("roundRect: a radius is negative".into()));
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
    T::with(call, this, |pen| {
        let t = pen.transform;
        pen.path.extend(t * rounded.to_path(0.1));
        pen.path.move_to(t * Point::new(x, y));
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
        Some(Value::Number(_)) => finite(args, 0),
        // A `DOMMatrix2DInit`, the identity without one.
        init => Some(matrix_coefficients(call, init)?).filter(|m| m.iter().all(|n| n.is_finite())),
    };
    match m {
        Some(m) => set_transform_to(call, this, |_| Affine::new(m)),
        None => Ok(Value::Undefined),
    }
}

pub(crate) fn reset_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    set_transform_to(call, this, |_| Affine::IDENTITY)
}

/// `getTransform()`: a `DOMMatrix`.
pub(crate) fn get_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let m = surface(call, this, |s| s.transform.as_coeffs())?;
    let values = call.new_float64_array(&m)?;
    let class = crate::global_class(call, "DOMMatrix")?;
    call.construct(&class, &[Value::Js(values)])
}

/// `a` … `f` of a `DOMMatrix2DInit`, as `DOMMatrix.fromMatrix()` checks and completes it.
pub(crate) fn matrix_coefficients<H: Clone>(call: &mut dyn Call<H>, init: Option<&Value<H>>) -> Result<[f64; 6], Error> {
    let class = crate::global_class(call, "DOMMatrix")?;
    let matrix = crate::handle(crate::method(call, &class, "fromMatrix", &[init.cloned().unwrap_or(Value::Undefined)])?, "DOMMatrix")?;
    let mut m = [0.0; 6];
    for (value, key) in m.iter_mut().zip(["a", "b", "c", "d", "e", "f"]) {
        let number = call.get(&matrix, key)?;
        *value = call.to_number(&number)?;
    }
    Ok(m)
}

// State.

pub(crate) fn save<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut properties = Vec::with_capacity(PROPERTIES.len());
    for p in &PROPERTIES {
        properties.push(stored(call, this, p.key)?);
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
        for (p, value) in PROPERTIES.iter().zip(properties) {
            call.set(context, p.key, &value)?;
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
        reset_properties(call, context)?;
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
    match stored(call, this, key)? {
        Value::String(css) => Ok(css_color(&css).map(Style::Color)),
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
        if let Some(color) = css_color(&css) {
            stops.push(ColorStop::from((*offset as f32, color)));
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

/// How a drawing lands on the canvas.
struct Compositing {
    /// `globalAlpha`.
    alpha: f32,
    /// `globalCompositeOperation`.
    blend: BlendMode,
    /// `filter`'s, then the shadow, applied in order.
    effects: Vec<Effect>,
}

/// What `filter` and the shadow do to a drawing, in pixels, whatever the transform.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Effect {
    /// A Gaussian blur of this standard deviation.
    Blur(f64),
    /// A shadow under the drawing, offset by `dx`, `dy`, blurred with the standard deviation `blur`.
    Shadow { dx: f64, dy: f64, blur: f64, color: Color },
}

/// The largest blur, as a standard deviation in pixels: a wider one would make vello_cpu filter
/// a layer far larger than the canvas.
const MAX_BLUR: f64 = 128.0;

impl Effect {
    /// vello_cpu's filter for it on a canvas of `width` × `height`, none when it changes nothing.
    fn filter(self, width: f64, height: f64) -> Option<Filter> {
        let primitive = match self {
            Effect::Blur(blur) if blur > 0.0 => FilterPrimitive::GaussianBlur { std_deviation: blur.min(MAX_BLUR) as f32, edge_mode: EdgeMode::None },
            Effect::Shadow { dx, dy, blur, color } if color.components[3] > 0.0 && (blur > 0.0 || dx != 0.0 || dy != 0.0) => {
                let blur = blur.min(MAX_BLUR);
                // Farther off, the shadow misses the canvas.
                if dx.abs() > width + 3.0 * blur || dy.abs() > height + 3.0 * blur {
                    return None;
                }
                FilterPrimitive::DropShadow { dx: dx as f32, dy: dy as f32, std_deviation: blur as f32, color, edge_mode: EdgeMode::None }
            }
            _ => return None,
        };
        Some(Filter::from_primitive(primitive))
    }
}

/// The effects of a CSS `filter` value, in order; `None` when it isn't one. Functions vello_cpu
/// can't do yet (`brightness()`, `url()`, …) are valid and do nothing.
fn parse_filter(css: &str) -> Option<Vec<Effect>> {
    let mut rest = css.trim();
    let mut effects = Vec::new();
    if rest == "none" {
        return Some(effects);
    }
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let (name, after) = rest.split_once('(')?;
        let (arguments, after) = closing(after)?;
        let arguments = words(arguments);
        match name.trim() {
            "blur" => match arguments.as_slice() {
                [] => {}
                [radius] => effects.push(Effect::Blur(length(radius).filter(|r| *r >= 0.0)?)),
                _ => return None,
            },
            "drop-shadow" => {
                // The colour comes first or last.
                let color = |word: &str| css_color(word);
                let (color, lengths) = match arguments.split_first().zip(arguments.split_last()) {
                    Some(((first, rest), _)) if color(first).is_some() => (color(first), rest),
                    Some((_, (last, rest))) if color(last).is_some() => (color(last), rest),
                    _ => (None, arguments.as_slice()),
                };
                let lengths: Vec<f64> = lengths.iter().map(|l| length(l)).collect::<Option<_>>()?;
                let (dx, dy, blur) = match lengths.as_slice() {
                    [dx, dy] => (*dx, *dy, 0.0),
                    [dx, dy, blur] if *blur >= 0.0 => (*dx, *dy, *blur),
                    _ => return None,
                };
                effects.push(Effect::Shadow { dx, dy, blur: blur / 2.0, color: color.unwrap_or(Color::BLACK) });
            }
            "brightness" | "contrast" | "grayscale" | "hue-rotate" | "invert" | "opacity" | "saturate" | "sepia" | "url" => {}
            _ => return None,
        }
        rest = after.trim_start();
    }
    Some(effects)
}

/// The text up to the `)` closing a function, and the text after it.
fn closing(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some((text.get(..i)?, text.get(i + 1..)?)),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The space-separated words of `text`, a function's parentheses kept in its word.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let (mut depth, mut start) = (0, None);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(from) = start.take() {
                    words.extend(text.get(from..i));
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(from) = start {
        words.extend(text.get(from..));
    }
    words
}

/// A CSS length in pixels: `12px`, or `0`.
fn length(word: &str) -> Option<f64> {
    let number = match word.strip_suffix("px") {
        Some(number) => number,
        None if word == "0" => word,
        None => return None,
    };
    number.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// How drawing lands on the canvas now: `globalAlpha` and `globalCompositeOperation` as vello_cpu
/// has them, `filter` and the shadow.
fn compositing<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Compositing, Error> {
    let alpha = number_property(call, this, "_globalAlpha", 1.0)?.clamp(0.0, 1.0) as f32;
    let operation = text_property(call, this, "_globalCompositeOperation")?;
    let (mix, compose) = match operation.as_str() {
        "source-in" => (Mix::Normal, Compose::SrcIn),
        "source-out" => (Mix::Normal, Compose::SrcOut),
        "source-atop" => (Mix::Normal, Compose::SrcAtop),
        "destination-over" => (Mix::Normal, Compose::DestOver),
        "destination-in" => (Mix::Normal, Compose::DestIn),
        "destination-out" => (Mix::Normal, Compose::DestOut),
        "destination-atop" => (Mix::Normal, Compose::DestAtop),
        "lighter" => (Mix::Normal, Compose::Plus),
        "plus-lighter" => (Mix::Normal, Compose::PlusLighter),
        "clear" => (Mix::Normal, Compose::Clear),
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
    let mut effects = parse_filter(&text_property(call, this, "_filter")?).unwrap_or_default();
    if let Some(color) = css_color(&text_property(call, this, "_shadowColor")?) {
        let dx = number_property(call, this, "_shadowOffsetX", 0.0)?;
        let dy = number_property(call, this, "_shadowOffsetY", 0.0)?;
        // As for CSS shadows, the blur's standard deviation is half the `shadowBlur`.
        let blur = number_property(call, this, "_shadowBlur", 0.0)?.max(0.0) / 2.0;
        effects.push(Effect::Shadow { dx, dy, blur, color });
    }
    Ok(Compositing { alpha, blend: BlendMode::new(mix, compose), effects })
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
fn fill_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, path: BezPath, rule: Fill) -> Result<(), Error> {
    let Some(style) = style(call, this, key)? else {
        return Ok(());
    };
    let compositing = compositing(call, this)?;
    let smooth = !matches!(stored(call, this, "_imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        s.draw(&compositing, |scene| {
            scene.reset_transform();
            scene.set_fill_rule(rule);
            paint(scene, style, transform, smooth);
            scene.fill_path(&path);
        });
    })
}

/// The stroke the context's line properties describe, in user space.
fn line<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, dash: &[f64]) -> Result<Option<Stroke>, Error> {
    let width = number_property(call, this, "_lineWidth", 1.0)?;
    if width <= 0.0 {
        return Ok(None);
    }
    let cap = match text_property(call, this, "_lineCap")?.as_str() {
        "round" => Cap::Round,
        "square" => Cap::Square,
        _ => Cap::Butt,
    };
    let join = match text_property(call, this, "_lineJoin")?.as_str() {
        "round" => Join::Round,
        "bevel" => Join::Bevel,
        _ => Join::Miter,
    };
    let miter = number_property(call, this, "_miterLimit", 10.0)?.max(0.0);
    let offset = number_property(call, this, "_lineDashOffset", 0.0)?;
    let mut stroke = Stroke::new(width).with_caps(cap).with_join(join).with_miter_limit(miter);
    if !dash.is_empty() && dash.iter().any(|d| *d > 0.0) {
        stroke = stroke.with_dashes(offset, dash.iter().copied());
    }
    Ok(Some(stroke))
}

/// Strokes `path` (in pixels) with `strokeStyle`, its line in the current user space.
fn stroke_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, path: BezPath) -> Result<(), Error> {
    let Some(style) = style(call, this, "_strokeStyle")? else {
        return Ok(());
    };
    let dash = surface(call, this, |s| s.dash.clone())?;
    let Some(stroke) = line(call, this, &dash)? else {
        return Ok(());
    };
    let compositing = compositing(call, this)?;
    let smooth = !matches!(stored(call, this, "_imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        // A transform that flattens everything strokes nothing.
        if transform.determinant().abs() < 1e-12 {
            return;
        }
        let path = transform.inverse() * path;
        s.draw(&compositing, |scene| {
            scene.set_transform(transform);
            scene.set_stroke(stroke);
            paint(scene, style, Affine::IDENTITY, smooth);
            scene.stroke_path(&path);
        });
    })
}

/// The `Path2D` that `fill()`, `stroke()`, `clip()` and the point tests take first, in pixels
/// through the context's transform, and the arguments after it; the current path without one.
fn path_argument<'a, H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &'a [Value<H>]) -> Result<(BezPath, &'a [Value<H>]), Error> {
    if let Some((first, rest)) = args.split_first()
        && let Some(path) = crate::path2d::path_of(call, first)?
    {
        let transform = surface(call, this, |s| s.transform)?;
        return Ok((transform * path, rest));
    }
    Ok((surface(call, this, |s| s.path.clone())?, args))
}

/// `fill(path, fillRule)`.
pub(crate) fn fill<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, args) = path_argument(call, this, args)?;
    fill_path(call, this, "_fillStyle", path, fill_rule(args))?;
    Ok(Value::Undefined)
}

/// `stroke(path)`.
pub(crate) fn stroke<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, _) = path_argument(call, this, args)?;
    stroke_path(call, this, path)?;
    Ok(Value::Undefined)
}

/// `clip(path, fillRule)`: drawing is inside the path from then on, until `restore()`.
pub(crate) fn clip<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, args) = path_argument(call, this, args)?;
    let rule = fill_rule(args);
    surface(call, this, |s| s.clips.push((path, rule)))?;
    Ok(Value::Undefined)
}

/// `isPointInPath(path, x, y, fillRule)`: whether the point (in pixels) is inside the path.
pub(crate) fn is_point_in_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, args) = path_argument(call, this, args)?;
    let Some([x, y]) = finite(args, 0) else {
        return Ok(Value::Bool(false));
    };
    let winding = path.winding(Point::new(x, y));
    let inside = match fill_rule(args) {
        Fill::EvenOdd => winding % 2 != 0,
        Fill::NonZero => winding != 0,
    };
    Ok(Value::Bool(inside))
}

/// `isPointInStroke(path, x, y)`: whether the point (in pixels) is on the path's stroke, with the
/// context's line.
pub(crate) fn is_point_in_stroke<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, args) = path_argument(call, this, args)?;
    let (transform, dash) = surface(call, this, |s| (s.transform, s.dash.clone()))?;
    let (Some([x, y]), Some(line)) = (finite(args, 0), line(call, this, &dash)?) else {
        return Ok(Value::Bool(false));
    };
    // In user space, where the line has its width.
    if transform.determinant().abs() < 1e-12 {
        return Ok(Value::Bool(false));
    }
    let inverse = transform.inverse();
    let outline = vello_cpu::kurbo::stroke(inverse * path, &line, &vello_cpu::kurbo::StrokeOpts::default(), 0.01);
    Ok(Value::Bool(outline.winding(inverse * Point::new(x, y)) != 0))
}

/// A rectangle in user space, as a closed path in pixels.
fn rect_path<H>(s: &Surface<H>, x: f64, y: f64, w: f64, h: f64) -> BezPath {
    s.transform * Rect::new(x, y, x + w, y + h).to_path(0.1)
}

pub(crate) fn fill_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        let path = surface(call, this, |s| rect_path(s, x, y, w, h))?;
        fill_path(call, this, "_fillStyle", path, Fill::NonZero)?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn stroke_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        let path = surface(call, this, |s| rect_path(s, x, y, w, h))?;
        stroke_path(call, this, path)?;
    }
    Ok(Value::Undefined)
}

/// `clearRect(x, y, w, h)`: transparent black there, inside the clips.
pub(crate) fn clear_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        surface(call, this, |s| {
            let path = rect_path(s, x, y, w, h);
            // Removing an opaque rectangle's coverage leaves only it transparent.
            let clearing = Compositing { alpha: 1.0, blend: BlendMode::new(Mix::Normal, Compose::DestOut), effects: Vec::new() };
            s.draw(&clearing, |scene| {
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
    let font = text_property(call, this, "_font")?;
    let align = text_property(call, this, "_textAlign")?;
    let baseline = text_property(call, this, "_textBaseline")?;
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
        fill_path(call, this, "_fillStyle", path, Fill::NonZero)?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn stroke_text<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(path) = text_path(call, this, args)? {
        stroke_path(call, this, path)?;
    }
    Ok(Value::Undefined)
}

// Images.

/// `drawImage(image, dx, dy)`, `(image, dx, dy, dw, dh)` or `(image, sx, sy, sw, sh, dx, dy, dw,
/// dh)`, the image a canvas.
pub(crate) fn draw_image<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(image) = args.first() else {
        return Err(Error::Type("drawImage: no image".into()));
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
    let compositing = compositing(call, this)?;
    let smooth = !matches!(stored(call, this, "_imageSmoothingEnabled")?, Value::Bool(false));
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
        s.draw(&compositing, |scene| {
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
        return Err(Error::Type(format!("create{kind}Gradient: not finite")));
    }
    let gradient = crate::canvas::new_style(call, "CanvasGradient")?;
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
        return Err(Error::Dom("IndexSizeError", "addColorStop: the offset isn't between 0 and 1".into()));
    }
    let color = match args.get(1) {
        Some(color) => call.to_string(color)?,
        None => String::new(),
    };
    if css_color(&color).is_none() {
        return Err(Error::Dom("SyntaxError", format!("addColorStop: '{color}' isn't a colour")));
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
        return Err(Error::Type("createPattern: no image".into()));
    };
    // The image's pixels as they are now.
    crate::canvas::image_pixels(call, image)?;
    let repetition = match args.get(1) {
        None | Some(Value::Null | Value::Undefined) => "repeat".to_owned(),
        Some(r) => call.to_string(r)?,
    };
    let repetition = if repetition.is_empty() { "repeat".to_owned() } else { repetition };
    if !matches!(repetition.as_str(), "repeat" | "repeat-x" | "repeat-y" | "no-repeat") {
        return Err(Error::Dom("SyntaxError", format!("createPattern: '{repetition}' isn't a repetition")));
    }
    let pattern = crate::canvas::new_style(call, "CanvasPattern")?;
    call.set(&pattern, "_image", image)?;
    call.set(&pattern, "_repetition", &Value::String(repetition))?;
    Ok(Value::Js(pattern))
}

#[cfg(test)]
mod tests {
    use super::{Effect, parse_filter};
    use vello_cpu::peniko::Color;

    #[test]
    fn filters_parse_as_browsers_read_them() {
        let shadow = |dx, dy, blur, color| Effect::Shadow { dx, dy, blur, color };
        assert_eq!(parse_filter("none"), Some(vec![]));
        assert_eq!(parse_filter(" blur(3px) "), Some(vec![Effect::Blur(3.0)]));
        assert_eq!(parse_filter("blur()"), Some(vec![]));
        assert_eq!(parse_filter("drop-shadow(1px 2px)"), Some(vec![shadow(1.0, 2.0, 0.0, Color::BLACK)]));
        assert_eq!(parse_filter("drop-shadow(rgb(255, 0, 0) 1px 0 4px)"), Some(vec![shadow(1.0, 0.0, 2.0, Color::from_rgb8(255, 0, 0))]));
        assert_eq!(
            parse_filter("drop-shadow(1px 2px 4px red) blur(1px)"),
            Some(vec![shadow(1.0, 2.0, 2.0, Color::from_rgb8(255, 0, 0)), Effect::Blur(1.0)])
        );
        // Valid, though vello_cpu can't do them yet.
        assert_eq!(parse_filter("sepia(1) blur(2px) url(#f)"), Some(vec![Effect::Blur(2.0)]));
        for invalid in ["", "blur(-1px)", "blur(1em)", "blur(2px", "drop-shadow(1px)", "drop-shadow(1px 2px -3px)", "glow(1px)", "blur(1px) x"] {
            assert_eq!(parse_filter(invalid), None, "{invalid}");
        }
    }
}
