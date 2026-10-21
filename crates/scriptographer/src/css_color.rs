//! CSS colours as the canvas takes them and gives them back: CSS Color 4 as the color crate parses
//! it, `currentcolor` (black, as for a canvas without an element), the system colours, and CSS
//! Color 5's `color-mix()` and relative colours (`rgb(from red g r b)`: channel keywords, numbers,
//! percentages and `none`, but no `calc()`).

use vello_cpu::color::{ColorSpaceTag, DynamicColor, HueDirection, Rgba8, Srgb, parse_color};
use vello_cpu::peniko::Color;

/// The colour of `css` in sRGB, rounded to 8 bits a channel as a canvas keeps it.
pub(crate) fn color(css: &str) -> Option<Color> {
    CssColor::parse(css).map(CssColor::srgb)
}

/// `css` as a canvas gives its colour back: in the legacy syntax `#rrggbb`, or `rgba(r, g, b, a)`
/// when it's not opaque; else as CSS writes the colour, in its space (`color(srgb 0.5 0 0.5)`).
pub(crate) fn serialized(css: &str) -> Option<String> {
    let parsed = CssColor::parse(css)?;
    Some(if parsed.legacy { legacy(parsed.color) } else { modern(parsed.color) })
}

/// A colour, and whether it was written in the legacy syntax: a name, hex, `rgb()`, `hsl()` or
/// `hwb()`.
#[derive(Clone, Copy)]
pub(crate) struct CssColor {
    color: DynamicColor,
    pub legacy: bool,
}

impl CssColor {
    pub(crate) fn parse(css: &str) -> Option<Self> {
        let css = css.trim().to_ascii_lowercase();
        let legacy = |color| Some(Self { color, legacy: true });
        if css == "currentcolor" {
            return legacy(DynamicColor::from_alpha_color(Color::BLACK));
        }
        if let Some(hex) = system_color(&css) {
            return legacy(parse_color(hex).ok()?);
        }
        if let Some(arguments) = arguments(&css, "color-mix") {
            return mix(arguments);
        }
        if let Some(relative) = relative(&css) {
            return relative;
        }
        let name = css.split('(').next().unwrap_or_default();
        Some(Self { color: parse_color(&css).ok()?, legacy: !matches!(name, "color" | "lab" | "lch" | "oklab" | "oklch") })
    }

    /// The colour in sRGB, rounded to 8 bits a channel as a canvas keeps it.
    pub(crate) fn srgb(self) -> Color {
        let Rgba8 { r, g, b, a } = self.color.to_alpha_color::<Srgb>().to_rgba8();
        Color::from_rgba8(r, g, b, a)
    }
}

/// `color-mix(in <space> [<direction> hue], <color> [<p>], <color> [<p>])`, as CSS Color 5 mixes.
fn mix(arguments: &str) -> Option<CssColor> {
    let parts = split(arguments, ',');
    let [method, first, second] = parts.as_slice() else {
        return None;
    };
    let words: Vec<&str> = method.split_whitespace().collect();
    let (space, direction) = match words.as_slice() {
        ["in", space] => (space, HueDirection::Shorter),
        ["in", space, direction, "hue"] => (space, hue_direction(direction)?),
        _ => return None,
    };
    let space = space_tag(space)?;
    let ((first, p1), (second, p2)) = (mixed(first)?, mixed(second)?);
    let (p1, p2) = match (p1, p2) {
        (None, None) => (0.5, 0.5),
        (Some(p1), None) => (p1, 1.0 - p1),
        (None, Some(p2)) => (1.0 - p2, p2),
        (Some(p1), Some(p2)) => (p1, p2),
    };
    let sum = p1 + p2;
    if sum <= 0.0 {
        return None;
    }
    let mut color = first.interpolate(second, space, direction).eval((p2 / sum) as f32);
    // Less than all of them together is that much transparent.
    if sum < 1.0 {
        color.components[3] *= sum as f32;
    }
    Some(CssColor { color, legacy: false })
}

/// A colour of `color-mix()`, and its share when it's given one, as a fraction.
fn mixed(text: &str) -> Option<(DynamicColor, Option<f64>)> {
    let share = |word: &str| word.strip_suffix('%')?.parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p)).map(|p| p / 100.0);
    match split(text, ' ').as_slice() {
        [color] => Some((CssColor::parse(color)?.color, None)),
        [color, p] | [p, color] if share(p).is_some() => Some((CssColor::parse(color)?.color, share(p))),
        _ => None,
    }
}

/// A relative colour, `<function>(from <color> <channels> [/ <alpha>])`: its channels keywords of
/// the origin colour's in the function's space, numbers, percentages or `none`. None when `css`
/// isn't one; `Some(None)` when it is, but invalid.
fn relative(css: &str) -> Option<Option<CssColor>> {
    let (function, arguments) =
        ["rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color"].iter().find_map(|f| Some((*f, arguments(css, f)?)))?;
    let arguments = arguments.replace('/', " / ");
    let words = split(&arguments, ' ');
    let ["from", origin, rest @ ..] = words.as_slice() else {
        return None;
    };
    Some((|| {
        let origin = CssColor::parse(origin)?.color;
        let (space, rest) = match (function, rest) {
            ("color", [space, rest @ ..]) => (Some(*space), rest),
            (_, rest) => (None, rest),
        };
        let (tag, keywords, scale) = match (function, space) {
            ("rgb" | "rgba", _) => (ColorSpaceTag::Srgb, ["r", "g", "b"], 255.0),
            ("hsl" | "hsla", _) => (ColorSpaceTag::Hsl, ["h", "s", "l"], 1.0),
            ("hwb", _) => (ColorSpaceTag::Hwb, ["h", "w", "b"], 1.0),
            ("lab", _) => (ColorSpaceTag::Lab, ["l", "a", "b"], 1.0),
            ("lch", _) => (ColorSpaceTag::Lch, ["l", "c", "h"], 1.0),
            ("oklab", _) => (ColorSpaceTag::Oklab, ["l", "a", "b"], 1.0),
            ("oklch", _) => (ColorSpaceTag::Oklch, ["l", "c", "h"], 1.0),
            (_, Some(space @ ("xyz" | "xyz-d50" | "xyz-d65"))) => (space_tag(space)?, ["x", "y", "z"], 1.0),
            (_, Some(space)) => (space_tag(space)?, ["r", "g", "b"], 1.0),
            _ => return None,
        };
        let [c0, c1, c2, origin_alpha] = origin.convert(tag).components;
        let channels = [c0, c1, c2];
        let value = |word: &str| -> Option<String> {
            if let Some(i) = keywords.iter().position(|k| *k == word) {
                Some(number(f64::from(channels.get(i).copied().unwrap_or_default()) * scale))
            } else if word == "alpha" {
                Some(number(f64::from(origin_alpha)))
            } else if word == "none" || word.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')) {
                Some(word.to_owned())
            } else {
                None
            }
        };
        let (values, alpha) = match rest {
            [a, b, c] => ([a, b, c], "alpha"),
            [a, b, c, "/", alpha] => ([a, b, c], *alpha),
            _ => return None,
        };
        let values = [value(values[0])?, value(values[1])?, value(values[2])?, value(alpha)?].join(" ");
        let values = values.rsplit_once(' ').map(|(channels, alpha)| format!("{channels} / {alpha}"))?;
        let css = match space {
            Some(space) => format!("color({space} {values})"),
            None => format!("{}({values})", function.trim_end_matches('a')),
        };
        Some(CssColor { color: parse_color(&css).ok()?, legacy: false })
    })())
}

/// The arguments of the function `name` in `css`, which the end of `css` closes too.
fn arguments<'a>(css: &'a str, name: &str) -> Option<&'a str> {
    let arguments = css.strip_prefix(name)?.strip_prefix('(')?;
    Some(arguments.strip_suffix(')').unwrap_or(arguments))
}

/// `text` split at `separator` outside parentheses, trimmed, without empty parts.
fn split(text: &str, separator: char) -> Vec<&str> {
    let (mut parts, mut depth, mut start) = (Vec::new(), 0, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == separator && depth == 0 => {
                parts.push(text.get(start..i).unwrap_or_default());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default());
    parts.into_iter().map(str::trim).filter(|part| !part.is_empty()).collect()
}

/// The colour space CSS names `name`.
fn space_tag(name: &str) -> Option<ColorSpaceTag> {
    Some(match name {
        "srgb" => ColorSpaceTag::Srgb,
        "srgb-linear" => ColorSpaceTag::LinearSrgb,
        "display-p3" => ColorSpaceTag::DisplayP3,
        "a98-rgb" => ColorSpaceTag::A98Rgb,
        "prophoto-rgb" => ColorSpaceTag::ProphotoRgb,
        "rec2020" => ColorSpaceTag::Rec2020,
        "xyz" | "xyz-d65" => ColorSpaceTag::XyzD65,
        "xyz-d50" => ColorSpaceTag::XyzD50,
        "hsl" => ColorSpaceTag::Hsl,
        "hwb" => ColorSpaceTag::Hwb,
        "lab" => ColorSpaceTag::Lab,
        "lch" => ColorSpaceTag::Lch,
        "oklab" => ColorSpaceTag::Oklab,
        "oklch" => ColorSpaceTag::Oklch,
        _ => return None,
    })
}

fn hue_direction(name: &str) -> Option<HueDirection> {
    Some(match name {
        "shorter" => HueDirection::Shorter,
        "longer" => HueDirection::Longer,
        "increasing" => HueDirection::Increasing,
        "decreasing" => HueDirection::Decreasing,
        _ => return None,
    })
}

/// `color` in the legacy syntax: `#rrggbb`, or `rgba(r, g, b, a)` when it's not opaque.
fn legacy(color: DynamicColor) -> String {
    let Rgba8 { r, g, b, a } = color.to_alpha_color::<Srgb>().to_rgba8();
    if a == 255 {
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    // The fewest decimals that give the same 8 bits back, as CSS serializes alpha.
    let exact = f64::from(a) / 255.0;
    let alpha =
        [100.0, 1000.0].into_iter().map(|scale| (exact * scale).round() / scale).find(|alpha| (alpha * 255.0).round() as u8 == a).unwrap_or(exact);
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// `color` as CSS writes it in its space: `color(srgb 0.5 0 0.5)`, `lab(50 20 -30 / 0.5)`; a
/// colour in `hsl` or `hwb` as sRGB.
fn modern(color: DynamicColor) -> String {
    let color = match color.cs {
        ColorSpaceTag::Hsl | ColorSpaceTag::Hwb => color.convert(ColorSpaceTag::Srgb),
        _ => color,
    };
    let function = match color.cs {
        ColorSpaceTag::Lab => "lab(",
        ColorSpaceTag::Lch => "lch(",
        ColorSpaceTag::Oklab => "oklab(",
        ColorSpaceTag::Oklch => "oklch(",
        ColorSpaceTag::LinearSrgb => "color(srgb-linear ",
        ColorSpaceTag::DisplayP3 => "color(display-p3 ",
        ColorSpaceTag::A98Rgb => "color(a98-rgb ",
        ColorSpaceTag::ProphotoRgb => "color(prophoto-rgb ",
        ColorSpaceTag::Rec2020 => "color(rec2020 ",
        ColorSpaceTag::XyzD50 => "color(xyz-d50 ",
        ColorSpaceTag::XyzD65 => "color(xyz-d65 ",
        _ => "color(srgb ",
    };
    let missing = color.flags.missing();
    let [c0, c1, c2, alpha] = color.components;
    let component = |i: usize, c: f32| if missing.contains(i) { "none".to_owned() } else { number(f64::from(c)) };
    let alpha = if alpha < 1.0 { format!(" / {}", number(f64::from(alpha))) } else { String::new() };
    format!("{function}{} {} {}{alpha})", component(0, c0), component(1, c1), component(2, c2))
}

/// `n` as browsers write a colour's numbers: to six significant digits, without trailing zeros.
fn number(n: f64) -> String {
    let digits = if n == 0.0 { 1 } else { n.abs().log10().floor() as i32 + 1 };
    let decimals = (6 - digits).clamp(0, 6) as usize;
    let text = format!("{n:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    // No negative zero:
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// The value of the system colour `name`, as browsers have them in a light colour scheme; the
/// deprecated ones as CSS Color 4 maps them.
fn system_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "canvas" | "activecaption" | "appworkspace" | "background" | "inactivecaption" | "infobackground" | "menu" | "scrollbar" | "window" => {
            "#ffffff"
        }
        "canvastext" | "captiontext" | "infotext" | "menutext" | "windowtext" | "buttontext" | "fieldtext" | "highlighttext" | "marktext" => {
            "#000000"
        }
        "linktext" => "#0000ee",
        "visitedtext" => "#551a8b",
        "activetext" => "#ff0000",
        "buttonface" | "buttonhighlight" | "buttonshadow" | "threedface" => "#efefef",
        "buttonborder" | "activeborder" | "inactiveborder" | "threeddarkshadow" | "threedhighlight" | "threedlightshadow" | "threedshadow"
        | "windowframe" => "#767676",
        "field" => "#ffffff",
        "highlight" => "#b5d5ff",
        "selecteditem" | "accentcolor" => "#0075ff",
        "selecteditemtext" | "accentcolortext" => "#ffffff",
        "mark" => "#ffff00",
        "graytext" | "inactivecaptiontext" => "#808080",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::serialized;

    #[test]
    fn colors_serialize_as_the_canvas_gives_them_back() {
        for (css, expected) in [
            ("red", "#ff0000"),
            ("rgba(255, 0, 0, 0.5)", "rgba(255, 0, 0, 0.5)"),
            ("hsl(120 100% 50%)", "#00ff00"),
            ("CurrentColor", "#000000"),
            ("ButtonText", "#000000"),
            ("color(srgb 0.25 0.5 0.75 / 0.5)", "color(srgb 0.25 0.5 0.75 / 0.5)"),
            ("color-mix(in srgb, red, blue)", "color(srgb 0.5 0 0.5)"),
            ("color-mix(in srgb, red 25%, blue)", "color(srgb 0.25 0 0.75)"),
            ("color-mix(in srgb, red 20%, blue 20%)", "color(srgb 0.5 0 0.5 / 0.4)"),
            ("color-mix(in hsl longer hue, red, blue)", "color(srgb 0 1 0)"),
            ("rgb(from red g r b)", "color(srgb 0 1 0)"),
            ("rgb(from red r g b / 50%)", "color(srgb 1 0 0 / 0.5)"),
            ("hsl(from red calc(h + 120) s l)", ""),
            ("color(from color(srgb 0.25 0.5 0.75 / 0.5) srgb r g b / alpha)", "color(srgb 0.25 0.5 0.75 / 0.5)"),
            ("lab(from red l a b)", "lab(54.2905 80.8049 69.891)"),
        ] {
            assert_eq!(serialized(css).unwrap_or_default(), expected, "{css}");
        }
        for invalid in ["color-mix(red, blue)", "color-mix(in nowhere, red, blue)", "color-mix(in srgb, red 0%, blue 0%)", "rgb(from red g r)"] {
            assert_eq!(serialized(invalid), None, "{invalid}");
        }
    }
}
