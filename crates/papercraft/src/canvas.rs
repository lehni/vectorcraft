//! A minimal `OffscreenCanvas`, for what Paper.js needs a canvas for without a DOM: resolving
//! CSS colours (`new Color('red')`: Paper.js paints a pixel with the colour as `fillStyle` and
//! reads it back), and measuring text (`ctx.font`, `ctx.measureText()`) for `PointText`, with the
//! app's fonts and shaping (VectorCraft's text crate, harfrust).
//!
//! The canvas holds no pixels yet. It is its own 2D context, `getImageData()` returns the current
//! `fillStyle` as the pixel, and the drawing calls do nothing. Rasters and drawing come with
//! VectorCraft's renderer behind it.

use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;

use crate::{Call, Engine, Error, Host, HostFunction, Value};

/// Sets `OffscreenCanvas` on `global`, and the methods every canvas gets on `host`.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle, host: &Rc<RefCell<Host<E::Handle>>>) -> Result<(), Error> {
    let methods: [(&str, HostFunction<E::Handle>); 7] = [
        ("getContext", get_context),
        ("getImageData", get_image_data),
        ("measureText", measure_text),
        ("fillRect", nothing),
        ("clearRect", nothing),
        ("save", nothing),
        ("restore", nothing),
    ];
    let mut handles = Vec::with_capacity(methods.len());
    for (name, method) in methods {
        handles.push((name, engine.function(name, method)?));
    }
    host.borrow_mut().canvas_methods = handles;
    let constructor = engine.constructor("OffscreenCanvas", offscreen_canvas)?;
    engine.with_call(|call| call.set(global, "OffscreenCanvas", &Value::Js(constructor)))
}

/// `new OffscreenCanvas(width, height)`.
fn offscreen_canvas<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let canvas = call.new_object()?;
    let size = |index: usize| match args.get(index) {
        Some(Value::Number(size)) => *size,
        _ => 0.0,
    };
    call.set(&canvas, "width", &Value::Number(size(0)))?;
    call.set(&canvas, "height", &Value::Number(size(1)))?;
    call.set(&canvas, "canvas", &Value::Js(canvas.clone()))?;
    call.set(&canvas, "fillStyle", &Value::String("#000000".into()))?;
    call.set(&canvas, "font", &Value::String(DEFAULT_FONT.into()))?;
    let methods = call.host()?.borrow().canvas_methods.clone();
    for (name, method) in methods {
        call.set(&canvas, name, &Value::Js(method))?;
    }
    Ok(Value::Js(canvas))
}

/// `canvas.getContext('2d')`: the canvas itself.
fn get_context<H: Clone>(_call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    Ok(this.clone())
}

/// `context.getImageData()`: one pixel of the current `fillStyle`, `{ data: [r, g, b, a] }`. A
/// colour CSS doesn't know is transparent black, as on a canvas cleared to `rgba(0,0,0,0)`.
fn get_image_data<H>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(context) = this else {
        return Err(Error::Script("getImageData: not called on a canvas".into()));
    };
    let color = match call.get(context, "fillStyle")? {
        Value::String(style) => svgtypes::Color::from_str(&style).ok(),
        _ => None,
    };
    let color = color.unwrap_or(svgtypes::Color::new_rgba(0, 0, 0, 0));
    let pixel: Vec<_> = [color.red, color.green, color.blue, color.alpha].into_iter().map(|c| Value::Number(f64::from(c))).collect();
    let data = call.new_array(&pixel)?;
    let image = call.new_object()?;
    call.set(&image, "data", &Value::Js(data))?;
    Ok(Value::Js(image))
}

/// A context's font until a script sets one, as in browsers.
const DEFAULT_FONT: &str = "10px sans-serif";

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
    let font = match call.get(context, "font")? {
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
struct Measured {
    width: f64,
    ink_left: f64,
    ink_right: f64,
    ink_ascent: f64,
    ink_descent: f64,
    ascent: f64,
    descent: f64,
}

/// `text` laid out on one line in `font`, as the app lays out point type.
fn measure(text: &str, font: &Font) -> Measured {
    use vectorcraft_geom::Shape as _;
    let style = vectorcraft_doc::CharStyle { font_family: font.family(), font_style: font.style().into(), size: font.size, ..Default::default() };
    let object = vectorcraft_doc::TextObject::point(vectorcraft_geom::Point::ZERO, &text.replace(['\n', '\r'], " "), style);
    let layout = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), &object);
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

fn nothing<H>(_call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    Ok(Value::Undefined)
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
