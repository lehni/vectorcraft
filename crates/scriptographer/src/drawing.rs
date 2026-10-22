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
//! The context's properties (`fillStyle`, `lineWidth`, …) are kept with its surface, as the
//! script gave them; `save()` keeps them with the transform, clip and dashes. Each canvas, path,
//! gradient and pattern is a slot of the runtime's (`crate::canvas::Canvases`), which its object in
//! `js/src/canvas.js` names by its number: the functions here get it as `this`.

use std::sync::Arc;

use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_cpu::kurbo::{Affine, Arc as ArcSegment, BezPath, Cap, Join, ParamCurveNearest, PathEl, Point, Rect, Shape, Stroke, Vec2};
use vello_cpu::peniko::{self, BlendMode, Color, ColorStop, Compose, Extend, Fill, ImageQuality, ImageSampler, Mix};
use vello_cpu::{Pixmap, RenderContext, RenderSettings, Resources};

use crate::{Call, Error, Value};

/// A context property: its name, its value on a new canvas, and the values it takes; others are
/// ignored, as browsers ignore them. `save()` keeps them.
pub(crate) struct Property {
    pub name: &'static str,
    initial: Initial,
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

const fn property(name: &'static str, initial: Initial, check: Check) -> Property {
    Property { name, initial, check }
}

pub(crate) const PROPERTIES: [Property; 27] = [
    property("fillStyle", Initial::Text("#000000"), Check::Style),
    property("strokeStyle", Initial::Text("#000000"), Check::Style),
    property("lineWidth", Initial::Number(1.0), Check::Positive),
    property("lineCap", Initial::Text("butt"), Check::Keyword(&["butt", "round", "square"])),
    property("lineJoin", Initial::Text("miter"), Check::Keyword(&["round", "bevel", "miter"])),
    property("miterLimit", Initial::Number(10.0), Check::Positive),
    property("lineDashOffset", Initial::Number(0.0), Check::Finite),
    property("globalAlpha", Initial::Number(1.0), Check::Fraction),
    property("globalCompositeOperation", Initial::Text("source-over"), Check::Keyword(COMPOSITE_OPERATIONS)),
    property("font", Initial::Text(crate::canvas::DEFAULT_FONT), Check::Font),
    property("textAlign", Initial::Text("start"), Check::Keyword(&["start", "end", "left", "right", "center"])),
    property("textBaseline", Initial::Text("alphabetic"), Check::Keyword(&["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"])),
    property("direction", Initial::Text("inherit"), Check::Keyword(&["ltr", "rtl", "inherit"])),
    property("lang", Initial::Text("inherit"), Check::Text),
    property("imageSmoothingEnabled", Initial::Bool(true), Check::Bool),
    property("imageSmoothingQuality", Initial::Text("low"), Check::Keyword(&["low", "medium", "high"])),
    property("shadowBlur", Initial::Number(0.0), Check::NonNegative),
    property("shadowColor", Initial::Text("rgba(0, 0, 0, 0)"), Check::Color),
    property("shadowOffsetX", Initial::Number(0.0), Check::Finite),
    property("shadowOffsetY", Initial::Number(0.0), Check::Finite),
    property("filter", Initial::Text("none"), Check::Filter),
    property("letterSpacing", Initial::Text("0px"), Check::Length),
    property("wordSpacing", Initial::Text("0px"), Check::Length),
    property("fontKerning", Initial::Text("auto"), Check::Keyword(&["auto", "normal", "none"])),
    property(
        "fontStretch",
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
        Initial::Text("normal"),
        Check::Keyword(&["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"]),
    ),
    property("textRendering", Initial::Text("auto"), Check::Keyword(&["auto", "optimizeSpeed", "optimizeLegibility", "geometricPrecision"])),
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
    /// Any text.
    Text,
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

/// `get(name)`: the context's property `name`, as the script gave it.
pub(crate) fn get_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let property = property_named(call, args)?;
    property_value(call, this, property.name)
}

/// `set(name, value, slot)`: keeps the value as the property has it, when it takes it; a
/// `fillStyle` or `strokeStyle` that is a gradient or a pattern comes with its slot.
pub(crate) fn set_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let property = property_named(call, args)?;
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    let (kept, slot) = match (property.check, args.get(2)) {
        (Check::Style, Some(Value::Number(slot))) => (Some(value), Some(*slot as usize)),
        (check, _) => (checked(call, check, value)?, None),
    };
    if let Some(kept) = kept {
        surface(call, this, |s| s.properties.set(property.name, kept, slot))?;
    }
    Ok(Value::Undefined)
}

fn property_named<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<&'static Property, Error> {
    let name = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    PROPERTIES.iter().find(|p| p.name == name).ok_or_else(|| Error::Type(format!("Unknown property: '{name}'.")))
}

/// The context's properties, as [`PROPERTIES`] lists them, and the slots of a `fillStyle` and
/// `strokeStyle` that are a gradient or a pattern.
#[derive(Clone)]
pub(crate) struct Properties<H> {
    values: Vec<Value<H>>,
    styles: [Option<usize>; 2],
}

impl<H> Properties<H> {
    /// As on a new canvas.
    fn new() -> Self {
        Self { values: PROPERTIES.iter().map(|p| p.initial.value()).collect(), styles: [None; 2] }
    }

    fn index(name: &str) -> Option<usize> {
        PROPERTIES.iter().position(|p| p.name == name)
    }

    /// The slot of the style in `name`, when it's a gradient or a pattern.
    fn style(&self, name: &str) -> Option<usize> {
        self.styles.get(usize::from(name == "strokeStyle")).copied().flatten()
    }

    fn value(&self, name: &str) -> Value<H>
    where
        H: Clone,
    {
        Self::index(name).and_then(|i| self.values.get(i)).cloned().unwrap_or(Value::Undefined)
    }

    fn set(&mut self, name: &str, value: Value<H>, slot: Option<usize>) {
        if let Some(stored) = Self::index(name).and_then(|i| self.values.get_mut(i)) {
            *stored = value;
        }
        if let ("fillStyle" | "strokeStyle", Some(style)) = (name, self.styles.get_mut(usize::from(name == "strokeStyle"))) {
            *style = slot;
        }
    }
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
            crate::canvas::font_css(&text).map(Value::String)
        }
        Check::Length => {
            let text = call.to_string(&value)?.trim().to_ascii_lowercase();
            css_length(&text, 10.0).is_some().then_some(Value::String(text))
        }
        // Gradients and patterns come with their slot; anything else is text.
        Check::Color | Check::Style => crate::css_color::serialized(&call.to_string(&value)?).map(Value::String),
        Check::Text => Some(Value::String(call.to_string(&value)?)),
        Check::Filter => {
            let text = call.to_string(&value)?;
            parse_filter(&text).is_some().then_some(Value::String(text))
        }
    })
}

/// A CSS length in pixels, an `em` the font's size `em`, a `rem` the default font's 10 pixels.
pub(crate) fn css_length(text: &str, em: f64) -> Option<f64> {
    let text = text.trim().to_ascii_lowercase();
    if text == "0" {
        return Some(0.0);
    }
    // `ex` and `ch` as half an em, `ic` as an em, as CSS has them without the font's own.
    let units = [
        ("px", 1.0),
        ("em", em),
        ("rem", 10.0),
        ("ex", em / 2.0),
        ("ch", em / 2.0),
        ("ic", em),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 101.6),
    ];
    units.iter().find_map(|(unit, scale)| text.strip_suffix(unit)?.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * scale))
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
    properties: Properties<H>,
    saved: Vec<Saved<H>>,
    /// A context's `alpha: false`: the pixels are composited onto black, as they come.
    opaque: bool,
}

/// What `save()` keeps.
struct Saved<H> {
    transform: Affine,
    clips: Vec<(BezPath, Fill)>,
    dash: Vec<f64>,
    properties: Properties<H>,
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
            properties: Properties::new(),
            saved: Vec::new(),
            opaque: false,
        }
    }

    /// Renders what's drawn onto the pixels.
    pub(crate) fn flush(&mut self) {
        if let Some(mut scene) = self.scene.take() {
            scene.flush();
            scene.render(&mut self.pixmap, &mut Resources::default());
            self.keep_opaque();
        }
    }

    /// Cleared to `width` × `height`, its state reset, as setting a canvas's size does.
    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        let opaque = self.opaque;
        *self = Self::new(width, height);
        self.set_opaque(opaque);
    }

    /// Its pixels cleared, and what's drawn but not rendered dropped; its state as it is.
    pub(crate) fn clear(&mut self) {
        self.scene = None;
        self.pixmap = Pixmap::new(self.pixmap.width(), self.pixmap.height());
        self.keep_opaque();
    }

    /// Opaque from now on, or not: an opaque canvas starts black.
    pub(crate) fn set_opaque(&mut self, opaque: bool) {
        self.opaque = opaque;
        self.keep_opaque();
    }

    /// An opaque canvas's pixels composited onto black: as they're premultiplied, at full alpha.
    pub(crate) fn keep_opaque(&mut self) {
        if self.opaque {
            for [_, _, _, alpha] in self.pixmap.data_as_u8_slice_mut().as_chunks_mut::<4>().0 {
                *alpha = 255;
            }
        }
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
    /// `draw` draws with its transforms after the one it's given: the shadow's offset, then none.
    fn draw(&mut self, compositing: &Compositing, mut draw: impl FnMut(&mut RenderContext, Affine)) {
        let (width, height) = (f64::from(self.pixmap.width()), f64::from(self.pixmap.height()));
        if width == 0.0 || height == 0.0 {
            return;
        }
        let filters: Vec<Filter> = compositing.effects.iter().filter_map(|e| e.filter(width, height)).collect();
        let shadow = compositing.shadow.filter(Shadow::shows);
        let clips = self.clips.clone();
        let scene = self.scene();
        // Clips and filters are in pixels.
        scene.reset_transform();
        for (clip, rule) in &clips {
            scene.set_fill_rule(*rule);
            scene.push_clip_layer(clip);
        }
        // The innermost layer's filter applies first.
        let mut filtered = |scene: &mut RenderContext, offset| {
            for filter in filters.iter().rev() {
                scene.push_filter_layer(filter.clone());
            }
            draw(scene, offset);
            for _ in &filters {
                scene.pop_layer();
            }
        };
        if let Some(shadow) = shadow {
            // Composited as the drawing is, the drawing moved, in the shadow's colour where it
            // covers, then blurred.
            scene.push_layer(None, Some(compositing.blend), Some(compositing.alpha), None, None);
            if shadow.blur > 0.0 {
                let blur = FilterPrimitive::GaussianBlur { std_deviation: shadow.blur.min(MAX_BLUR) as f32, edge_mode: EdgeMode::None };
                scene.push_filter_layer(Filter::from_primitive(blur));
            }
            filtered(scene, Affine::translate(shadow.offset));
            scene.reset_transform();
            scene.reset_paint_transform();
            scene.push_layer(None, Some(BlendMode::new(Mix::Normal, Compose::SrcIn)), None, None, None);
            scene.set_paint(shadow.color);
            scene.fill_rect(&Rect::new(0.0, 0.0, width, height));
            scene.pop_layer();
            if shadow.blur > 0.0 {
                scene.pop_layer();
            }
            scene.pop_layer();
        }
        let layered = compositing.alpha < 1.0 || compositing.blend != BlendMode::default() || !filters.is_empty();
        if layered {
            scene.push_layer(None, Some(compositing.blend), Some(compositing.alpha), None, None);
        }
        filtered(scene, Affine::IDENTITY);
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

/// The property `name` of the context `this` is.
pub(crate) fn property_value<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, name: &str) -> Result<Value<H>, Error> {
    surface(call, this, |s| s.properties.value(name))
}

fn text_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, name: &str) -> Result<String, Error> {
    match property_value(call, this, name)? {
        Value::String(text) => Ok(text),
        other => call.to_string(&other),
    }
}

fn number_property<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, name: &str, default: f64) -> Result<f64, Error> {
    Ok(match property_value(call, this, name)? {
        Value::Number(n) if n.is_finite() => n,
        _ => default,
    })
}

// Styles.

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

/// `roundRect(x, y, w, h, ...radii)`: a rectangle with elliptical corners, the radii the
/// horizontal and vertical ones of its top-left, top-right, bottom-right and bottom-left corners
/// as it shows (`js/src/canvas.js` reads them as the standard has them). It's drawn from `(x, y)`
/// towards `x + w`, then `y + h`, so a negative width or height draws it the other way round;
/// radii that don't fit shrink together.
pub(crate) fn round_rect<H: Clone, T: Target>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some([x, y, w, h, radii @ ..]) = finite::<12, H>(args, 0) else {
        return Ok(Value::Undefined);
    };
    let corners: [Vec2; 4] = [0, 2, 4, 6].map(|i| Vec2::new(radii[i], radii[i + 1]));
    let [ul, ur, lr, ll] = corners;
    let fit = [(ul.x + ur.x) / w.abs(), (ll.x + lr.x) / w.abs(), (ul.y + ll.y) / h.abs(), (ur.y + lr.y) / h.abs()]
        .into_iter()
        .filter(|f| f.is_finite())
        .fold(1.0f64, f64::max);
    // The corners as drawn, each with its radii, as the rectangle shows them.
    let points = [Point::new(x, y), Point::new(x + w, y), Point::new(x + w, y + h), Point::new(x, y + h)];
    let radius = |p: Point| {
        let (left, top) = (p.x == x.min(x + w), p.y == y.min(y + h));
        let r = match (left, top) {
            (true, true) => ul,
            (false, true) => ur,
            (false, false) => lr,
            (true, false) => ll,
        };
        r / fit
    };
    // The sides' directions, from each corner to the next; a quarter ellipse's control points, as
    // a cubic Bézier curve draws one.
    let (sx, sy) = (if w < 0.0 { -1.0 } else { 1.0 }, if h < 0.0 { -1.0 } else { 1.0 });
    let sides = [Vec2::new(sx, 0.0), Vec2::new(0.0, sy), Vec2::new(-sx, 0.0), Vec2::new(0.0, -sy)];
    const K: f64 = 0.552_284_749_830_793_4;
    T::with(call, this, |pen| {
        let t = pen.transform;
        // Each corner's ends, along the side it's reached by and the side it leaves on: the
        // horizontal radius along a horizontal side.
        let ends = |i: usize| {
            let (corner, r) = (points[i], radius(points[i]));
            let (incoming, outgoing) = (sides[(i + 3) % 4], sides[i]);
            let (r_in, r_out) = if i.is_multiple_of(2) { (r.y, r.x) } else { (r.x, r.y) };
            (corner - incoming * r_in, incoming * r_in * K, corner + outgoing * r_out, outgoing * r_out * K)
        };
        let (_, _, start, _) = ends(0);
        pen.path.move_to(t * start);
        for i in [1, 2, 3, 0] {
            let (from, from_handle, to, to_handle) = ends(i);
            pen.path.line_to(t * from);
            pen.path.curve_to(t * (from + from_handle), t * (to - to_handle), t * to);
        }
        pen.path.close_path();
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
    surface(call, this, |s| {
        let saved = Saved { transform: s.transform, clips: s.clips.clone(), dash: s.dash.clone(), properties: s.properties.clone() };
        s.saved.push(saved);
    })?;
    Ok(Value::Undefined)
}

pub(crate) fn restore<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| {
        if let Some(saved) = s.saved.pop() {
            s.transform = saved.transform;
            s.clips = saved.clips;
            s.dash = saved.dash;
            s.properties = saved.properties;
        }
    })?;
    Ok(Value::Undefined)
}

/// `reset()`: as a new canvas, its pixels cleared.
pub(crate) fn reset<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    surface(call, this, |s| {
        let (width, height) = (s.pixmap.width(), s.pixmap.height());
        s.resize(width, height);
    })?;
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
#[derive(Clone)]
enum Style {
    Color(Color),
    /// A gradient, in its coordinates through the transform.
    Gradient(peniko::Gradient, Affine),
    Pattern(Pattern),
}

/// An image that paints, in its coordinates through `transform`, repeated across and down or
/// not, and nothing beyond it where it isn't.
#[derive(Clone)]
pub(crate) struct Pattern {
    pub pixels: Arc<Pixmap>,
    pub repeat: (bool, bool),
    pub transform: Affine,
}

impl Pattern {
    /// Where it paints, in its coordinates, when it isn't repeated both ways.
    fn bounds(&self) -> Option<Rect> {
        // Far enough to be beyond any canvas, whatever the transform.
        const FAR: f64 = 1e9;
        let span = |repeat, size: u16| if repeat { (-FAR, FAR) } else { (0.0, f64::from(size)) };
        let ((x0, x1), (y0, y1)) = (span(self.repeat.0, self.pixels.width()), span(self.repeat.1, self.pixels.height()));
        (self.repeat != (true, true)).then(|| Rect::new(x0, y0, x1, y1))
    }
}

/// What a gradient's or a pattern's slot holds: a gradient as its stops are added, or a pattern's
/// image as it was when the pattern was made.
pub(crate) enum Paint {
    /// `modern` once a stop is in a syntax other than the legacy one, which makes CSS interpolate
    /// the gradient in Oklab.
    Gradient {
        kind: &'static str,
        at: Vec<f64>,
        stops: Vec<ColorStop>,
        modern: bool,
    },
    Pattern(Pattern),
}

/// The style in the property `name`, `None` when it paints nothing.
fn style<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, name: &str) -> Result<Option<Style>, Error> {
    let (value, slot) = surface(call, this, |s| (s.properties.value(name), s.properties.style(name)))?;
    match (slot, value) {
        (Some(slot), _) => crate::canvas::with_paint(call, slot, |paint| match paint {
            Paint::Gradient { kind, at, stops, modern } => {
                gradient(kind, at, stops, *modern).map(|(gradient, transform)| Style::Gradient(gradient, transform))
            }
            Paint::Pattern(pattern) => Some(Style::Pattern(pattern.clone())),
        })
        .map(Option::flatten),
        (None, Value::String(css)) => Ok(crate::css_color::color(&css).map(Style::Color)),
        _ => Ok(None),
    }
}

/// The gradient of `kind` at `at`, with `stops` in the order of their offsets, interpolated in
/// Oklab when they're `modern`, and the transform of its coordinates; none when it paints nothing:
/// without stops, or a linear gradient from a point to itself or a radial one between the same
/// circle.
fn gradient(kind: &str, at: &[f64], stops: &[ColorStop], modern: bool) -> Option<(peniko::Gradient, Affine)> {
    if stops.is_empty() {
        return None;
    }
    let mut stops = stops.to_vec();
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    let (gradient, transform) = match (kind, at) {
        ("linear", [x0, y0, x1, y1]) if (x0, y0) != (x1, y1) => (peniko::Gradient::new_linear((*x0, *y0), (*x1, *y1)), Affine::IDENTITY),
        ("radial", [x0, y0, r0, x1, y1, r1]) if (x0, y0, r0) != (x1, y1, r1) => {
            (peniko::Gradient::new_two_point_radial((*x0, *y0), *r0 as f32, (*x1, *y1), *r1 as f32), Affine::IDENTITY)
        }
        // vello_cpu's sweeps start at angles from 0, so the start angle turns the gradient.
        ("conic", [angle, x, y]) => {
            (peniko::Gradient::new_sweep((*x, *y), 0.0, std::f32::consts::TAU), Affine::rotate_about(*angle, Point::new(*x, *y)))
        }
        _ => return None,
    };
    // The canvas interpolates colours with their alpha apart, not premultiplied.
    let gradient = peniko::Gradient { interpolation_alpha_space: peniko::InterpolationAlphaSpace::Unpremultiplied, ..gradient };
    let gradient = if modern { gradient.with_interpolation_cs(vello_cpu::color::ColorSpaceTag::Oklab) } else { gradient };
    Some((gradient.with_stops(stops.as_slice()), transform))
}

/// How a drawing lands on the canvas.
struct Compositing {
    /// `globalAlpha`.
    alpha: f32,
    /// `globalCompositeOperation`.
    blend: BlendMode,
    /// `filter`'s, applied in order.
    effects: Vec<Effect>,
    /// The shadow, drawn first.
    shadow: Option<Shadow>,
}

/// A shadow, which the canvas draws under a drawing as a drawing of its own: the drawing moved by
/// `offset` (in pixels), in `color`, blurred with the standard deviation `blur`.
#[derive(Clone, Copy, Debug)]
struct Shadow {
    offset: Vec2,
    blur: f64,
    color: Color,
}

impl Shadow {
    /// Whether it shows: in a colour, and blurred or moved.
    fn shows(&self) -> bool {
        self.color.components[3] > 0.0 && (self.blur > 0.0 || self.offset != Vec2::ZERO)
    }
}

/// What `filter` does to a drawing, in pixels, whatever the transform.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Effect {
    /// A Gaussian blur of this standard deviation.
    Blur(f64),
    /// `drop-shadow()`: a shadow under the drawing, offset by `dx`, `dy`, blurred with the standard
    /// deviation `blur`.
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
                let color = |word: &str| crate::css_color::color(word);
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
    let effects = parse_filter(&text_property(call, this, "filter")?).unwrap_or_default();
    let shadow = match crate::css_color::color(&text_property(call, this, "shadowColor")?) {
        Some(color) => {
            let offset = Vec2::new(number_property(call, this, "shadowOffsetX", 0.0)?, number_property(call, this, "shadowOffsetY", 0.0)?);
            // As for CSS shadows, the blur's standard deviation is half the `shadowBlur`.
            let blur = number_property(call, this, "shadowBlur", 0.0)?.max(0.0) / 2.0;
            Some(Shadow { offset, blur, color })
        }
        None => None,
    };
    Ok(Compositing { alpha, blend: BlendMode::new(mix, compose), effects, shadow })
}

/// Sets `style` as the paint, its coordinates those of `transform`.
fn paint(scene: &mut RenderContext, style: Style, transform: Affine, smooth: bool) {
    match style {
        Style::Color(color) => {
            scene.reset_paint_transform();
            scene.set_paint(color);
        }
        Style::Gradient(gradient, local) => {
            scene.set_paint_transform(transform * local);
            scene.set_paint(gradient);
        }
        Style::Pattern(pattern) => {
            let quality = if smooth { ImageQuality::Medium } else { ImageQuality::Low };
            let extend = |repeat| if repeat { Extend::Repeat } else { Extend::Pad };
            let sampler = ImageSampler { x_extend: extend(pattern.repeat.0), y_extend: extend(pattern.repeat.1), quality, alpha: 1.0 };
            scene.set_paint_transform(transform * pattern.transform);
            scene.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(pattern.pixels), sampler });
        }
    }
}

/// Draws with `style`, its coordinates those of `transform` after the scene's, inside where it
/// paints: a pattern that isn't repeated both ways only over its image.
fn painted(scene: &mut RenderContext, style: &Style, transform: Affine, smooth: bool, draw: impl FnOnce(&mut RenderContext)) {
    let bounds = match style {
        Style::Pattern(pattern) => pattern.bounds().map(|bounds| transform * pattern.transform * bounds.to_path(0.1)),
        _ => None,
    };
    if let Some(bounds) = &bounds {
        scene.push_clip_layer(bounds);
    }
    paint(scene, style.clone(), transform, smooth);
    draw(scene);
    if bounds.is_some() {
        scene.pop_layer();
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
    let smooth = !matches!(property_value(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        s.draw(&compositing, |scene, offset| {
            scene.set_transform(offset);
            scene.set_fill_rule(rule);
            painted(scene, &style, transform, smooth, |scene| scene.fill_path(&path));
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

/// Strokes `path` (in pixels) with `strokeStyle`, its line in the current user space.
fn stroke_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, path: BezPath) -> Result<(), Error> {
    let Some(style) = style(call, this, "strokeStyle")? else {
        return Ok(());
    };
    let dash = surface(call, this, |s| s.dash.clone())?;
    let Some(stroke) = line(call, this, &dash)? else {
        return Ok(());
    };
    let compositing = compositing(call, this)?;
    let smooth = !matches!(property_value(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
    surface(call, this, |s| {
        let transform = s.transform;
        // A transform that flattens everything strokes nothing.
        if transform.determinant().abs() < 1e-12 {
            return;
        }
        let path = transform.inverse() * path;
        s.draw(&compositing, |scene, offset| {
            scene.set_transform(offset * transform);
            scene.set_stroke(stroke.clone());
            painted(scene, &style, Affine::IDENTITY, smooth, |scene| scene.stroke_path(&path));
        });
    })
}

/// The path whose slot `fill()`, `stroke()`, `clip()` and the point tests take first (`null` for
/// the current path), in pixels through the context's transform, and the arguments after it.
fn path_argument<'a, H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &'a [Value<H>]) -> Result<(BezPath, &'a [Value<H>]), Error> {
    let (first, rest) = args.split_first().unwrap_or((&Value::Null, &[]));
    let path = match crate::path2d::path_of(call, first)? {
        Some(path) => surface(call, this, |s| s.transform * path)?,
        None => surface(call, this, |s| s.path.clone())?,
    };
    Ok((path, rest))
}

/// `fill(path, fillRule)`.
pub(crate) fn fill<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (path, args) = path_argument(call, this, args)?;
    fill_path(call, this, "fillStyle", path, fill_rule(args))?;
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
    Ok(Value::Bool(contains(&path, Point::new(x, y), fill_rule(args))))
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
    Ok(Value::Bool(contains(&outline, inverse * Point::new(x, y), Fill::NonZero)))
}

/// Whether `point` is inside `path` as it fills, by `rule`: its open subpaths closed, and points on
/// its edges inside, as the canvas has them.
fn contains(path: &BezPath, point: Point, rule: Fill) -> bool {
    let mut closed = BezPath::new();
    let close = |closed: &mut BezPath| {
        if closed.elements().last().is_some_and(|el| !matches!(el, PathEl::ClosePath)) {
            closed.push(PathEl::ClosePath);
        }
    };
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) {
            close(&mut closed);
        }
        closed.push(*el);
    }
    close(&mut closed);
    let winding = closed.winding(point);
    let inside = match rule {
        Fill::EvenOdd => winding % 2 != 0,
        Fill::NonZero => winding != 0,
    };
    inside || closed.segments().any(|segment| segment.nearest(point, 1e-9).distance_sq < 1e-12)
}

/// A rectangle in user space, as a closed path in pixels.
fn rect_path<H>(s: &Surface<H>, x: f64, y: f64, w: f64, h: f64) -> BezPath {
    s.transform * Rect::new(x, y, x + w, y + h).to_path(0.1)
}

pub(crate) fn fill_rect<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some([x, y, w, h]) = finite(args, 0) {
        let path = surface(call, this, |s| rect_path(s, x, y, w, h))?;
        fill_path(call, this, "fillStyle", path, Fill::NonZero)?;
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
            let clearing = Compositing { alpha: 1.0, blend: BlendMode::new(Mix::Normal, Compose::DestOut), effects: Vec::new(), shadow: None };
            s.draw(&clearing, |scene, offset| {
                scene.set_transform(offset);
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
    let style = crate::canvas::text_style(call, this)?;
    let (outline, measured) = crate::canvas::outline(&text, &style);
    let (dx, dy) = measured.anchor(&style);
    // `maxWidth` squeezes the text to fit, as in browsers; one that isn't more than 0 draws none.
    let squeeze = match args.get(3) {
        Some(_) => match number(args, 3) {
            max if max > 0.0 => (max / measured.width).min(1.0),
            _ => return Ok(None),
        },
        None => 1.0,
    };
    let place = Affine::translate((x, y)) * Affine::scale_non_uniform(squeeze, 1.0) * Affine::translate((-dx, dy));
    surface(call, this, |s| Some(s.transform * place * outline))
}

pub(crate) fn fill_text<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(path) = text_path(call, this, args)? {
        fill_path(call, this, "fillStyle", path, Fill::NonZero)?;
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
    // Negative sizes give the same rectangles, not flipped images.
    let (source, destination) = (source.abs(), destination.abs());
    if source.width() == 0.0 || source.height() == 0.0 || destination.width() == 0.0 || destination.height() == 0.0 {
        return Ok(Value::Undefined);
    }
    let compositing = compositing(call, this)?;
    let smooth = !matches!(property_value(call, this, "imageSmoothingEnabled")?, Value::Bool(false));
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
        s.draw(&compositing, |scene, offset| {
            scene.set_transform(offset);
            scene.set_fill_rule(Fill::NonZero);
            let pattern = Pattern { pixels: pixels.clone(), repeat: (false, false), transform: Affine::IDENTITY };
            paint(scene, Style::Pattern(pattern), transform * image_to_user, smooth);
            scene.fill_path(&path);
        });
    })?;
    Ok(Value::Undefined)
}

/// `createLinearGradient(x0, y0, x1, y1)`, `createRadialGradient(x0, y0, r0, x1, y1, r1)` or
/// `createConicGradient(angle, x, y)`: the slot of a gradient, its stops added by `addColorStop()`.
pub(crate) fn create_gradient<H: Clone>(call: &mut dyn Call<H>, kind: &'static str, args: &[Value<H>], count: usize) -> Result<Value<H>, Error> {
    let at: Vec<f64> = (0..count).map(|i| number(args, i)).collect();
    if at.iter().any(|n| !n.is_finite()) {
        return Err(Error::Type(format!("create{kind}Gradient: not finite")));
    }
    if let ("radial", [_, _, r0, _, _, r1]) = (kind, at.as_slice())
        && (*r0 < 0.0 || *r1 < 0.0)
    {
        return Err(Error::Dom("IndexSizeError", "createRadialGradient: a radius is negative".into()));
    }
    crate::canvas::store_paint(call, Paint::Gradient { kind, at, stops: Vec::new(), modern: false })
}

/// `addColorStop(offset, color)`, of the gradient `this` is.
pub(crate) fn add_color_stop<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let offset = number(args, 0);
    if !(0.0..=1.0).contains(&offset) {
        return Err(Error::Dom("IndexSizeError", "addColorStop: the offset isn't between 0 and 1".into()));
    }
    let css = match args.get(1) {
        Some(color) => call.to_string(color)?,
        None => String::new(),
    };
    let Some(color) = crate::css_color::CssColor::parse(&css) else {
        return Err(Error::Dom("SyntaxError", format!("addColorStop: '{css}' isn't a colour")));
    };
    let slot = call.to_number(this)?;
    crate::canvas::with_paint(call, slot as usize, |paint| {
        if let Paint::Gradient { stops, modern, .. } = paint {
            stops.push(ColorStop::from((offset as f32, color.srgb())));
            *modern |= !color.legacy;
        }
    })?;
    Ok(Value::Undefined)
}

/// `createPattern(image, repetition)`: the slot of a pattern of a canvas's or bitmap's pixels.
pub(crate) fn create_pattern<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(image) = args.first() else {
        return Err(Error::Type("createPattern: no image".into()));
    };
    // The image's pixels as they are now.
    let pixels = crate::canvas::image_pixels(call, image)?;
    // `null` is no text, as WebIDL's `[LegacyNullToEmptyString]` has it.
    let repetition = match args.get(1) {
        None | Some(Value::Null) => String::new(),
        Some(r) => call.to_string(r)?,
    };
    let repeat = match repetition.as_str() {
        "" | "repeat" => (true, true),
        "repeat-x" => (true, false),
        "repeat-y" => (false, true),
        "no-repeat" => (false, false),
        _ => return Err(Error::Dom("SyntaxError", format!("createPattern: '{repetition}' isn't a repetition"))),
    };
    crate::canvas::store_paint(call, Paint::Pattern(Pattern { pixels, repeat, transform: Affine::IDENTITY }))
}

/// `setTransform(a, b, c, d, e, f)` of the pattern `this` is: its coordinates from then on; one
/// that isn't finite changes nothing.
pub(crate) fn set_pattern_transform<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(m) = finite::<6, H>(args, 0) {
        let slot = call.to_number(this)?;
        crate::canvas::with_paint(call, slot as usize, |paint| {
            if let Paint::Pattern(pattern) = paint {
                pattern.transform = Affine::new(m);
            }
        })?;
    }
    Ok(Value::Undefined)
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
