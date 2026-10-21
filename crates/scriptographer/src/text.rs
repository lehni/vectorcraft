//! Text objects as Paper.js text items, and back: point type is a `PointText`, area type an
//! `AreaText` and type on a path a `PathText`, with its runs and paragraphs in the form of
//! `exportJSON()`, and its layout by VectorCraft's text crate in the form of the host's
//! `layOut()`. Paper.js knows only part of VectorCraft's type controls, so reading text back starts
//! from the object it was read from, and only what the script changed is written.
//!
//! The frames of a thread are linked text items, which share one story: each one's JSON names the
//! thread's frames (`linked`), the story comes once for all of them ([`story_view`], the host's
//! `getStory()`), and each one is laid out from where the one before it ends. A story a script
//! changed goes back whole into its first item ([`written`]), and the items it links make the
//! thread, for the document to flow the story across them again.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json, json};
use vectorcraft_color::Paint;
use vectorcraft_doc::{
    CharPosition, CharStyle, Composer, Document, Justify, NodeId, NodeKind, ParaDirection, ParaStyle, PathAlign, ScriptMetrics, TabAlign, TabStop,
    TextKind, TextObject, TextRun, VerticalAlign, text::INLINE_CHAR,
};
use vectorcraft_geom::{Affine, PathData, Point};

use crate::Error;
use crate::host::{color, path_json};
use crate::writeback::{color as parse_color, path_data};

/// The frames of the thread `id` is a frame of, in order.
pub fn thread(doc: &Document, id: NodeId) -> Option<&[NodeId]> {
    doc.text_threads.iter().find(|thread| thread.contains(&id)).map(Vec::as_slice)
}

fn text_of(doc: &Document, id: NodeId) -> Option<&TextObject> {
    match doc.node(id).map(|node| &node.kind) {
        Some(NodeKind::Text(t)) => Some(t),
        _ => None,
    }
}

/// `t`, the text object `id`, as scripts see it: a frame of a thread with the thread's whole story,
/// which its frames share.
pub fn story_view(doc: &Document, id: NodeId, t: &TextObject) -> TextObject {
    let mut view = t.clone();
    if let Some(thread) = thread(doc, id) {
        let frames: Vec<&TextObject> = thread.iter().filter_map(|frame| text_of(doc, *frame)).collect();
        view.runs = vectorcraft_text::thread::story(&frames);
        view.set_paragraph_styles(vectorcraft_text::thread::story_paras(&frames, &[]));
    }
    view
}

/// The keys of the frames of the thread `id` is a frame of, for its JSON's `linked`.
pub fn linked(doc: &Document, id: NodeId) -> Option<Json> {
    thread(doc, id).map(|thread| json!(thread.iter().map(|frame| frame.0).collect::<Vec<_>>()))
}

/// Where a text item is in its story, as the script left it: on its own, or one of linked items.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Alone,
    First,
    Later,
}

/// The text object a script left as `edited` is written back as. `t` is the one in the document
/// (`None` for a new item) and `view` what the script read ([`story_view`]). An item on its own
/// keeps its story. Linked items share theirs: when the script changed it, the first one gets all of
/// it and the others none, for the document to flow it across them again, as after any edit; else
/// each one keeps its share.
pub fn written(t: Option<&TextObject>, view: Option<&TextObject>, mut edited: TextObject, link: Link) -> TextObject {
    if link == Link::Alone {
        return edited;
    }
    if let (Some(t), Some(view)) = (t, view)
        && edited.runs == view.runs
        && edited.paragraph_styles() == view.paragraph_styles()
    {
        edited.runs = t.runs.clone();
        edited.set_paragraph_styles(t.paragraph_styles());
        return edited;
    }
    if link == Link::Later {
        let paragraphs = edited.paragraph_styles();
        let end = vectorcraft_text::edit::runs_len(&edited.runs);
        let style = vectorcraft_text::edit::style_at(&edited.runs, end);
        edited.runs = vec![TextRun { text: String::new(), style, inline: None }];
        // The story's last paragraph style, which the frames before it end with.
        edited.set_paragraph_styles(paragraphs.last().cloned().into_iter().collect());
    }
    edited
}

/// The Paper.js class of `t`.
pub fn class(t: &TextObject) -> &'static str {
    match t.kind {
        TextKind::Point => "PointText",
        TextKind::Area { .. } => "AreaText",
        TextKind::OnPath { .. } => "PathText",
    }
}

/// The properties of `t` as Paper.js's `exportJSON()` writes them, for `props`: the matrix, the
/// text path and options of its kind, its runs and its paragraphs.
pub fn properties(t: &TextObject, props: &mut Map<String, Json>) {
    props.insert("matrix".into(), json!(t.xf.as_coeffs()));
    if t.vertical {
        props.insert("orientation".into(), json!("vertical"));
    }
    match &t.kind {
        TextKind::Point => {}
        TextKind::Area { frame } => {
            props.insert("textPath".into(), path_json(frame));
            let area = &t.area;
            props.insert("columns".into(), json!(area.columns));
            props.insert("rows".into(), json!(area.rows));
            props.insert("gutter".into(), json!(area.gutter));
            props.insert("inset".into(), json!(area.inset));
            props.insert(
                "verticalAlign".into(),
                json!(match area.vertical_align {
                    VerticalAlign::Center => "center",
                    VerticalAlign::Bottom => "bottom",
                    // Paper.js has no vertical justification: its lines sit at the top.
                    _ => "top",
                }),
            );
        }
        TextKind::OnPath { path, .. } => {
            props.insert("textPath".into(), path_json(path));
            let length = path.length();
            if let Some((start, end)) = t.kind.path_span() {
                props.insert("startOffset".into(), json!(start * length));
                props.insert("endOffset".into(), json!(end * length));
            }
            props.insert(
                "alignment".into(),
                json!(match t.path_align {
                    PathAlign::Ascender => "ascender",
                    PathAlign::Descender => "descender",
                    PathAlign::Center => "center",
                    PathAlign::Baseline => "baseline",
                }),
            );
            props.insert("spacing".into(), json!(t.path_spacing));
        }
    }
    story_properties(t, props);
}

/// The story of `t` as Paper.js's `exportJSON()` writes it, for `props`: its runs and its
/// paragraphs.
pub fn story_properties(t: &TextObject, props: &mut Map<String, Json>) {
    let runs: Vec<Json> = t
        .runs
        .iter()
        .zip(run_properties(t))
        .map(|(run, mut run_props)| {
            run_props.insert("content".into(), json!(run.text));
            Json::Object(run_props)
        })
        .collect();
    props.insert("runs".into(), Json::Array(runs));
    let paragraphs: Vec<Json> = t.paragraph_styles().iter().map(|p| Json::Object(para_properties(p))).collect();
    props.insert("paragraphs".into(), Json::Array(paragraphs));
}

/// The character properties of the runs of `t`, each with the auto leading of the paragraph it
/// starts in.
fn run_properties(t: &TextObject) -> Vec<Map<String, Json>> {
    let text = t.plain_text();
    t.runs
        .iter()
        .scan(0, |start, run| {
            let paragraph = text.get(..*start).map_or(0, |before| before.matches('\n').count());
            *start += run.text.len();
            Some(char_properties(&run.style, t.para_at(paragraph)))
        })
        .collect()
}

/// A text object from a Paper.js text item's JSON, starting from `before`, the object the item was
/// read from: what Paper.js has no form for stays, and only what differs from what `before` reads
/// as is written.
pub fn text_object(json: &Json, before: Option<&TextObject>) -> Result<TextObject, Error> {
    let props = json.get(1).and_then(Json::as_object).ok_or_else(|| Error::Script("not an item's JSON".into()))?;
    let mut t = before.cloned().unwrap_or_else(|| TextObject::point(Point::ZERO, "", CharStyle::default()));
    let mut old = Map::new();
    if let Some(before) = before {
        properties(before, &mut old);
    }
    // The settings of area type and type on a path, which Paper.js leaves out where they are its
    // defaults:
    let settings = completed(props, settings_defaults());
    let changed = |key: &str| settings.get(key).filter(|v| Some(*v) != old.get(key));
    t.xf = matrix(props.get("matrix"));
    t.vertical = props.get("orientation").and_then(Json::as_str) == Some("vertical");
    let text_path = |before: Option<&PathData>| props.get("textPath").map(|p| path_data(p, before)).transpose();
    match json.get(0).and_then(Json::as_str) {
        Some("AreaText") => {
            let before = match &t.kind {
                TextKind::Area { frame } => Some(frame),
                _ => None,
            };
            let frame = text_path(before)?.unwrap_or_default();
            t.kind = TextKind::Area { frame };
            let area = &mut t.area;
            if let Some(n) = changed("columns").and_then(Json::as_u64) {
                area.columns = usize::try_from(n).unwrap_or(1).clamp(1, 100);
            }
            if let Some(n) = changed("rows").and_then(Json::as_u64) {
                area.rows = usize::try_from(n).unwrap_or(1).clamp(1, 100);
            }
            if let Some(gutter) = changed("gutter").and_then(finite) {
                area.gutter = gutter;
            }
            if let Some(inset) = changed("inset").and_then(finite) {
                area.inset = inset;
            }
            if let Some(align) = changed("verticalAlign").and_then(Json::as_str) {
                area.vertical_align = match align {
                    "center" => VerticalAlign::Center,
                    "bottom" => VerticalAlign::Bottom,
                    _ => VerticalAlign::Top,
                };
            }
        }
        Some("PathText") => {
            let before = match &t.kind {
                TextKind::OnPath { path, .. } => Some(path),
                _ => None,
            };
            let path = text_path(before)?.unwrap_or_default();
            // Paper.js measures the offsets along the path, VectorCraft in its fractions:
            let length = path.length();
            let fraction = |offset: f64| if length > 0.0 { offset / length } else { 0.0 };
            let (start, end) = match &t.kind {
                TextKind::OnPath { start, end, .. } => (*start, *end),
                _ => (0.0, None),
            };
            let start = changed("startOffset").and_then(finite).map_or(start, fraction);
            let end = match changed("endOffset") {
                Some(Json::Null) => None,
                Some(offset) => finite(offset).map(fraction).or(end),
                None => end,
            };
            t.kind = TextKind::OnPath { path, start, end };
            if let Some(align) = changed("alignment").and_then(Json::as_str) {
                t.path_align = match align {
                    "ascender" => PathAlign::Ascender,
                    "descender" => PathAlign::Descender,
                    "center" => PathAlign::Center,
                    _ => PathAlign::Baseline,
                };
            }
            if let Some(spacing) = changed("spacing").and_then(finite) {
                t.path_spacing = spacing;
            }
        }
        _ => t.kind = TextKind::Point,
    }
    t.runs = match before {
        Some(before) => runs(props, &before.runs, &run_properties(before)),
        None => runs(props, &[], &[]),
    };
    let old_paras = before.map(TextObject::paragraph_styles).unwrap_or_default();
    let paras = paragraphs(props, &old_paras, t.paragraph_count());
    t.set_paragraph_styles(paras);
    t.normalize_paras();
    Ok(t)
}

/// The runs of a text item's JSON: those of `runs`, or one of `content` with the item's own
/// character properties. Each starts from the old run that read as it does, or else from the one
/// at its place, and inline objects keep their art in order.
fn runs(props: &Map<String, Json>, old: &[TextRun], old_props: &[Map<String, Json>]) -> Vec<TextRun> {
    let items: Vec<&Map<String, Json>> = match props.get("runs").and_then(Json::as_array) {
        Some(runs) => runs.iter().filter_map(Json::as_object).collect(),
        None => vec![props],
    };
    let mut arts = old.iter().filter_map(|run| run.inline.clone());
    let mut runs = Vec::new();
    let mut offset = 0;
    for item in items {
        let content = item.get("content").and_then(Json::as_str).unwrap_or_default();
        let now = completed(item, char_defaults());
        let index = old_props.iter().position(|p| *p == now).or_else(|| at_offset(old, offset)).unwrap_or(0);
        let mut style = old.get(index).map(|run| run.style.clone()).unwrap_or_default();
        if let Some(was) = old_props.get(index) {
            apply_char(&mut style, &now, was);
        } else {
            apply_char(&mut style, &now, &Map::new());
        }
        offset += content.len();
        // Each inline object is a run of its own:
        for (i, part) in content.split(INLINE_CHAR).enumerate() {
            if i > 0 {
                match arts.next() {
                    Some(art) => runs.push(TextRun::inline(art, style.clone())),
                    None => runs.push(TextRun::new(INLINE_CHAR.to_string(), style.clone())),
                }
            }
            if !part.is_empty() {
                runs.push(TextRun::new(part, style.clone()));
            }
        }
    }
    if runs.is_empty() {
        runs.push(TextRun::new("", old.first().map(|run| run.style.clone()).unwrap_or_default()));
    }
    runs
}

/// The index of the old run that holds byte `offset` of the old text.
fn at_offset(old: &[TextRun], offset: usize) -> Option<usize> {
    let mut start = 0;
    old.iter().position(|run| {
        start += run.text.len();
        offset < start
    })
}

/// The paragraph styles of a text item's JSON, `count` of them: those of `paragraphs`, or the
/// item's own paragraph properties for all, each from the old one at its place.
fn paragraphs(props: &Map<String, Json>, old: &[ParaStyle], count: usize) -> Vec<ParaStyle> {
    let items: Vec<&Map<String, Json>> =
        props.get("paragraphs").and_then(Json::as_array).map_or_else(Vec::new, |list| list.iter().filter_map(Json::as_object).collect());
    (0..count.max(1))
        .map(|i| {
            let item = items.get(i).copied().unwrap_or(props);
            let mut style = old.get(i).or(old.last()).cloned().unwrap_or_default();
            let was = para_properties(&style);
            apply_para(&mut style, &completed(item, para_defaults()), &was);
            style
        })
        .collect()
}

/// The character properties of `style` as Paper.js has them. Auto leading is the paragraph's.
fn char_properties(style: &CharStyle, paragraph: &ParaStyle) -> Map<String, Json> {
    let mut props = Map::new();
    let (weight, slant) = font_face(&style.font_style);
    props.insert("fontFamily".into(), json!(style.font_family));
    props.insert("fontWeight".into(), if weight == 400 { json!("normal") } else { json!(weight) });
    props.insert("fontStyle".into(), json!(slant));
    props.insert("fontSize".into(), json!(style.size));
    props.insert(
        "lineHeight".into(),
        match style.leading {
            Some(leading) => json!(format!("{leading}px")),
            None => {
                let factor = paragraph.justification.auto_leading / 100.0;
                if (factor - 1.2).abs() < 1e-9 { json!("auto") } else { json!(factor) }
            }
        },
    );
    props.insert("letterTracking".into(), json!(style.tracking));
    props.insert("letterKerning".into(), json!(style.kerning));
    let (h, v) = (style.h_scale / 100.0, style.v_scale / 100.0);
    props.insert("letterScaling".into(), if h == v { json!(h) } else { json!([h, v]) });
    props.insert("letterRotation".into(), json!(style.rotation));
    props.insert("baselineShift".into(), json!(style.baseline_shift));
    props.insert(
        "baselinePosition".into(),
        json!(match style.position {
            CharPosition::Normal => "normal",
            CharPosition::Superscript(_) => "superscript",
            CharPosition::Subscript(_) => "subscript",
        }),
    );
    props.insert(
        "capitals".into(),
        json!(if style.all_caps {
            "all"
        } else if style.small_caps.is_some() {
            "small"
        } else {
            "normal"
        }),
    );
    props.insert("underline".into(), json!(style.underline));
    props.insert("strikethrough".into(), json!(style.strikethrough));
    props.insert("features".into(), if style.features.is_empty() { Json::Null } else { json!(style.features) });
    props.insert("fontVariations".into(), if style.font_axes.is_empty() { Json::Null } else { json!(style.font_axes) });
    props.insert("fillColor".into(), color(&style.fill).unwrap_or(Json::Null));
    props.insert("strokeColor".into(), color(&style.stroke).unwrap_or(Json::Null));
    props.insert("strokeWidth".into(), json!(style.stroke_width));
    props
}

/// The character properties of `now` that differ from `was` (what `style` read as) on `style`.
fn apply_char(style: &mut CharStyle, now: &Map<String, Json>, was: &Map<String, Json>) {
    let changed = |key: &str| now.get(key).filter(|v| Some(*v) != was.get(key));
    if let Some(family) = changed("fontFamily").and_then(Json::as_str) {
        style.font_family = family.to_owned();
    }
    if changed("fontWeight").is_some() || changed("fontStyle").is_some() {
        let weight = now.get("fontWeight").map_or(400, css_weight);
        let slant = now.get("fontStyle").and_then(Json::as_str).unwrap_or("normal");
        style.font_style = style_name(weight, slant);
    }
    if let Some(size) = changed("fontSize").and_then(finite).filter(|s| *s > 0.0) {
        style.size = size;
    }
    if let Some(line_height) = changed("lineHeight") {
        style.leading = fixed_leading(line_height, style.size);
    }
    if let Some(tracking) = changed("letterTracking").and_then(finite) {
        style.tracking = tracking;
    }
    if let Some(kerning) = changed("letterKerning") {
        style.kerning = finite(kerning);
    }
    if let Some(scaling) = changed("letterScaling") {
        let (h, v) = match scaling {
            Json::Array(list) => (list.first().and_then(finite).unwrap_or(1.0), list.get(1).and_then(finite).unwrap_or(1.0)),
            other => {
                let s = finite(other).unwrap_or(1.0);
                (s, s)
            }
        };
        style.h_scale = h * 100.0;
        style.v_scale = v * 100.0;
    }
    if let Some(rotation) = changed("letterRotation").and_then(finite) {
        style.rotation = rotation;
    }
    if let Some(shift) = changed("baselineShift").and_then(finite) {
        style.baseline_shift = shift;
    }
    if let Some(position) = changed("baselinePosition").and_then(Json::as_str) {
        style.position = match position {
            "superscript" => CharPosition::Superscript(ScriptMetrics::default()),
            "subscript" => CharPosition::Subscript(ScriptMetrics::default()),
            _ => CharPosition::Normal,
        };
    }
    if let Some(capitals) = changed("capitals").and_then(Json::as_str) {
        style.all_caps = capitals == "all";
        style.small_caps = (capitals == "small").then(|| style.small_caps.unwrap_or(70.0));
    }
    if let Some(underline) = changed("underline").and_then(Json::as_bool) {
        style.underline = underline;
    }
    if let Some(strikethrough) = changed("strikethrough").and_then(Json::as_bool) {
        style.strikethrough = strikethrough;
    }
    if let Some(features) = changed("features") {
        style.features = features.as_array().map_or_else(Vec::new, |list| list.iter().filter_map(Json::as_str).map(str::to_owned).collect());
    }
    if let Some(variations) = changed("fontVariations") {
        style.font_axes =
            variations.as_object().map_or_else(BTreeMap::new, |axes| axes.iter().filter_map(|(tag, v)| Some((tag.clone(), finite(v)?))).collect());
    }
    if let Some(fill) = changed("fillColor") {
        style.fill = parse_color(fill).map_or(Paint::None, Paint::solid);
    }
    if let Some(stroke) = changed("strokeColor") {
        style.stroke = parse_color(stroke).map_or(Paint::None, Paint::solid);
    }
    if let Some(width) = changed("strokeWidth").and_then(finite) {
        style.stroke_width = width.max(0.0);
    }
}

/// The paragraph properties of `style` as Paper.js has them.
fn para_properties(style: &ParaStyle) -> Map<String, Json> {
    let mut props = Map::new();
    let (justification, last) = match style.justify {
        Justify::Auto => ("start", "left"),
        Justify::Left => ("left", "left"),
        Justify::Center => ("center", "left"),
        Justify::Right => ("right", "left"),
        Justify::JustifyLeft => ("justify", "left"),
        Justify::JustifyCenter => ("justify", "center"),
        Justify::JustifyRight => ("justify", "right"),
        Justify::JustifyAll => ("justify", "all"),
    };
    props.insert("justification".into(), json!(justification));
    props.insert("justifyLast".into(), json!(last));
    // VectorCraft's indents are left and right, Paper.js's at the start and end of the lines:
    let rtl = style.direction == Some(ParaDirection::RightToLeft);
    let (start, end) = if rtl { (style.right_indent, style.left_indent) } else { (style.left_indent, style.right_indent) };
    props.insert("startIndent".into(), json!(start));
    props.insert("endIndent".into(), json!(end));
    props.insert("firstLineIndent".into(), json!(style.first_line_indent));
    props.insert("spaceBefore".into(), json!(style.space_before));
    props.insert("spaceAfter".into(), json!(style.space_after));
    let h = &style.hyphenation;
    props.insert(
        "hyphenation".into(),
        if style.hyphenate {
            json!({ "wordsLongerThan": h.words_longer_than, "afterFirst": h.after_first, "beforeLast": h.before_last,
                "limit": h.hyphen_limit, "zone": h.zone, "capitalized": h.capitalized })
        } else {
            json!(false)
        },
    );
    props.insert(
        "composer".into(),
        json!(match style.composer {
            Composer::SingleLine => "line",
            Composer::EveryLine => "paragraph",
        }),
    );
    let tabs: Vec<Json> = style
        .tabs
        .iter()
        .map(|tab| {
            let align = match tab.align {
                TabAlign::Left => "left",
                TabAlign::Center => "center",
                TabAlign::Right => "right",
                TabAlign::Decimal => "decimal",
            };
            json!({ "position": tab.position, "align": align, "leader": tab.leader })
        })
        .collect();
    props.insert("tabs".into(), if tabs.is_empty() { Json::Null } else { Json::Array(tabs) });
    props.insert(
        "direction".into(),
        match style.direction {
            Some(ParaDirection::LeftToRight) => json!("ltr"),
            Some(ParaDirection::RightToLeft) => json!("rtl"),
            None => Json::Null,
        },
    );
    props
}

/// The paragraph properties of `now` that differ from `was` (what `style` read as) on `style`.
fn apply_para(style: &mut ParaStyle, now: &Map<String, Json>, was: &Map<String, Json>) {
    let changed = |key: &str| now.get(key).filter(|v| Some(*v) != was.get(key));
    if let Some(direction) = changed("direction") {
        style.direction = match direction.as_str() {
            Some("rtl") => Some(ParaDirection::RightToLeft),
            Some("ltr") => Some(ParaDirection::LeftToRight),
            _ => None,
        };
    }
    if changed("justification").is_some() || changed("justifyLast").is_some() {
        let last = now.get("justifyLast").and_then(Json::as_str).unwrap_or("left");
        style.justify = match now.get("justification").and_then(Json::as_str).unwrap_or("left") {
            "start" => Justify::Auto,
            "center" => Justify::Center,
            "right" => Justify::Right,
            // VectorCraft knows 'end' only through the paragraph's direction:
            "end" if style.direction == Some(ParaDirection::RightToLeft) => Justify::Left,
            "end" => Justify::Right,
            "justify" => match last {
                "center" => Justify::JustifyCenter,
                "right" | "end" => Justify::JustifyRight,
                "all" => Justify::JustifyAll,
                _ => Justify::JustifyLeft,
            },
            _ => Justify::Left,
        };
    }
    let rtl = style.direction == Some(ParaDirection::RightToLeft);
    if let Some(start) = changed("startIndent").and_then(finite) {
        *if rtl { &mut style.right_indent } else { &mut style.left_indent } = start;
    }
    if let Some(end) = changed("endIndent").and_then(finite) {
        *if rtl { &mut style.left_indent } else { &mut style.right_indent } = end;
    }
    if let Some(indent) = changed("firstLineIndent").and_then(finite) {
        style.first_line_indent = indent;
    }
    if let Some(space) = changed("spaceBefore").and_then(finite) {
        style.space_before = space;
    }
    if let Some(space) = changed("spaceAfter").and_then(finite) {
        style.space_after = space;
    }
    if let Some(hyphenation) = changed("hyphenation") {
        style.hyphenate = !matches!(hyphenation, Json::Bool(false) | Json::Null);
        if let Some(options) = hyphenation.as_object() {
            let h = &mut style.hyphenation;
            let count = |key: &str, value: &mut u32| {
                if let Some(n) = options.get(key).and_then(Json::as_u64) {
                    *value = u32::try_from(n).unwrap_or(u32::MAX);
                }
            };
            count("wordsLongerThan", &mut h.words_longer_than);
            count("afterFirst", &mut h.after_first);
            count("beforeLast", &mut h.before_last);
            count("limit", &mut h.hyphen_limit);
            if let Some(zone) = options.get("zone").and_then(finite) {
                h.zone = zone;
            }
            if let Some(capitalized) = options.get("capitalized").and_then(Json::as_bool) {
                h.capitalized = capitalized;
            }
        }
    }
    if let Some(composer) = changed("composer").and_then(Json::as_str) {
        style.composer = if composer == "line" { Composer::SingleLine } else { Composer::EveryLine };
    }
    if let Some(tabs) = changed("tabs") {
        style.tabs = tabs.as_array().map_or_else(Vec::new, |list| {
            list.iter()
                .filter_map(|tab| {
                    let position = tab.get("position").and_then(finite)?;
                    let align = match tab.get("align").and_then(Json::as_str) {
                        Some("center") => TabAlign::Center,
                        Some("right") => TabAlign::Right,
                        Some("decimal") => TabAlign::Decimal,
                        _ => TabAlign::Left,
                    };
                    let leader = tab.get("leader").and_then(Json::as_str).unwrap_or_default().to_owned();
                    Some(TabStop { position, align, leader, align_on: '.' })
                })
                .collect()
        });
    }
}

/// The host's `layOut()` result for `t`, from `start` (a UTF-16 index) in its story: where its
/// visible text ends, and its lines with their pieces, in text space, which is the item's own, as
/// Paper.js's `Project#host` describes them. Positions and indices are those of VectorCraft's
/// layout, so scripts see the lines the document shows.
pub fn layout(t: &TextObject, start: usize) -> Json {
    let story = t.plain_text();
    // A frame of a thread: the story from where the frame before it ends.
    let (t, offset) = if start > 0 {
        let byte = byte_at(&story, start);
        let mut from = t.clone();
        from.runs = vectorcraft_text::edit::slice_runs(&t.runs, byte, story.len());
        let first = story.get(..byte).map_or(0, |before| before.matches('\n').count());
        let count = story.get(byte..).map_or(0, |after| after.matches('\n').count()) + 1;
        from.set_paragraph_styles(t.paragraph_styles().get(first..first + count).map(<[ParaStyle]>::to_vec).unwrap_or_default());
        (std::borrow::Cow::Owned(from), utf16(&story, byte))
    } else {
        (std::borrow::Cow::Borrowed(t), 0)
    };
    let t = t.as_ref();
    let layout = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), t);
    let text = t.plain_text();
    let index = |byte: usize| offset + utf16(&text, byte);
    let styles: Vec<&CharStyle> = t.runs.iter().map(|run| &run.style).collect();
    let leading = |run: usize| styles.get(run).map_or(0.0, |s| s.leading.unwrap_or(s.size * t.para.justification.auto_leading / 100.0));
    let lines: Vec<Json> = layout
        .lines
        .iter()
        .map(|line| {
            let glyphs = layout.glyphs.get(line.glyph_start..line.glyph_end).unwrap_or_default();
            // Type on a path is one line with its glyphs turned along it, placed where they are:
            let baseline = if layout.on_path { 0.0 } else { line.baseline };
            let mut pieces: Vec<Json> = Vec::new();
            let mut group: Vec<&vectorcraft_text::PositionedGlyph> = Vec::new();
            let flush = |group: &mut Vec<&vectorcraft_text::PositionedGlyph>, pieces: &mut Vec<Json>| {
                let (Some(first), Some(last)) = (group.first(), group.last()) else {
                    return;
                };
                let size = styles.get(first.run).map_or(0.0, |s| s.size);
                let mut piece = json!({
                    "start": index(first.byte), "end": index(last.byte + last.len),
                    "x": first.origin.x, "y": first.origin.y - baseline, "dx": 0,
                    "width": group.iter().map(|g| g.advance).sum::<f64>(),
                    "rotation": first.angle.to_degrees(), "size": size,
                    "ascent": line.ascent, "descent": line.descent, "lineHeight": leading(first.run),
                });
                // A hyphen added where a line breaks has no text of its own:
                if first.len == 0 {
                    piece["content"] = json!("-");
                }
                pieces.push(piece);
                group.clear();
            };
            for glyph in glyphs {
                let joins = group.last().is_some_and(|last| {
                    !layout.on_path && last.run == glyph.run && last.len > 0 && glyph.len > 0 && last.byte + last.len == glyph.byte
                });
                if !joins {
                    flush(&mut group, &mut pieces);
                }
                group.push(glyph);
            }
            flush(&mut group, &mut pieces);
            let line_height = glyphs.iter().map(|g| leading(g.run)).fold(0.0, f64::max);
            json!({
                "start": index(line.start), "end": index(line.end),
                "x": line.x0, "y": baseline, "width": line.x1 - line.x0,
                "lineHeight": line_height, "ascent": line.ascent, "descent": line.descent,
                "pieces": pieces,
            })
        })
        .collect();
    // Where the next frame of a thread starts, as the document splits stories.
    let end = index(vectorcraft_text::thread::fitted(&layout, &text));
    json!({ "end": end, "lines": lines })
}

/// The byte of `text` at UTF-16 index `index`, as Paper.js counts, or its end for an index past it.
fn byte_at(text: &str, index: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= index {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// The UTF-16 index of byte `byte` of `text`, as Paper.js counts.
fn utf16(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text.get(..byte).map_or(0, |s| s.encode_utf16().count())
}

/// A Paper.js matrix, `[a, b, c, d, tx, ty]`, or the identity.
fn matrix(json: Option<&Json>) -> Affine {
    let values: Vec<f64> = json.and_then(Json::as_array).map_or_else(Vec::new, |list| list.iter().filter_map(finite).collect());
    match values.as_slice() {
        [a, b, c, d, e, f] => Affine::new([*a, *b, *c, *d, *e, *f]),
        _ => Affine::IDENTITY,
    }
}

fn finite(json: &Json) -> Option<f64> {
    json.as_f64().filter(|n| n.is_finite())
}

/// `props` with Paper.js's defaults for the keys of `defaults` it leaves out, as its JSON omits
/// them.
fn completed(props: &Map<String, Json>, defaults: Json) -> Map<String, Json> {
    let Json::Object(defaults) = defaults else {
        return Map::new();
    };
    defaults.into_iter().map(|(key, default)| (key.clone(), props.get(&key).cloned().unwrap_or(default))).collect()
}

/// Paper.js's defaults for the settings of area and path text.
fn settings_defaults() -> Json {
    json!({
        "columns": 1, "rows": 1, "gutter": 18, "inset": 0, "verticalAlign": "top",
        "startOffset": 0, "endOffset": null, "alignment": "baseline", "spacing": 0,
    })
}

/// Paper.js's defaults for the character properties of text.
fn char_defaults() -> Json {
    json!({
        "fontFamily": "sans-serif", "fontWeight": "normal", "fontStyle": "normal", "fontSize": 12, "lineHeight": "auto",
        "letterTracking": 0, "letterKerning": null, "letterScaling": 1, "letterRotation": 0, "baselineShift": 0,
        "baselinePosition": "normal", "capitals": "normal", "underline": false, "strikethrough": false, "features": null,
        "fontVariations": null, "fillColor": [0, 0, 0], "strokeColor": null, "strokeWidth": 1,
    })
}

/// Paper.js's defaults for the paragraph properties of text.
fn para_defaults() -> Json {
    json!({
        "justification": "left", "justifyLast": "left", "startIndent": 0, "endIndent": 0, "firstLineIndent": 0, "spaceBefore": 0,
        "spaceAfter": 0, "hyphenation": false, "composer": "paragraph", "tabs": null, "direction": null,
    })
}

/// The CSS weight and style of a font's style name, e.g. "Bold Italic" is 700 and italic.
fn font_face(name: &str) -> (u16, &'static str) {
    let name = name.to_ascii_lowercase().replace(['-', '_', ' '], "");
    let weight = WEIGHTS.iter().find(|(word, _)| name.contains(word)).map_or(400, |(_, weight)| *weight);
    let slant = if name.contains("italic") {
        "italic"
    } else if name.contains("oblique") {
        "oblique"
    } else {
        "normal"
    };
    (weight, slant)
}

/// Style-name words and their CSS weights, those that contain others first.
const WEIGHTS: [(&str, u16); 12] = [
    ("extralight", 200),
    ("ultralight", 200),
    ("semibold", 600),
    ("demibold", 600),
    ("extrabold", 800),
    ("ultrabold", 800),
    ("thin", 100),
    ("light", 300),
    ("medium", 500),
    ("bold", 700),
    ("black", 900),
    ("heavy", 900),
];

/// A CSS font weight: a number, or 'normal', 'bold' and the like.
fn css_weight(json: &Json) -> u16 {
    match json {
        Json::Number(n) => n.as_f64().map_or(400, |w| w.clamp(1.0, 1000.0) as u16),
        Json::String(s) => match s.as_str() {
            "bold" | "bolder" => 700,
            "lighter" => 300,
            other => other.parse().unwrap_or(400),
        },
        _ => 400,
    }
}

/// A style name for a CSS weight and style, as fonts name their faces: "Bold Italic", "Italic",
/// "Regular".
fn style_name(weight: u16, slant: &str) -> String {
    let weight = match weight {
        0..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    };
    let slant = match slant {
        "italic" => "Italic",
        "oblique" => "Oblique",
        _ => "",
    };
    match (weight, slant) {
        ("", "") => "Regular".to_owned(),
        (w, "") => w.to_owned(),
        ("", s) => s.to_owned(),
        (w, s) => format!("{w} {s}"),
    }
}

/// A Paper.js line height as a fixed leading in points, or `None` for automatic leading: a length
/// with units, as Paper.js reads them ('px' is a point here), is fixed, a factor of the size
/// automatic.
fn fixed_leading(json: &Json, size: f64) -> Option<f64> {
    let text = json.as_str()?.trim();
    let number = |suffix: &str| text.strip_suffix(suffix)?.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0);
    number("px").or_else(|| number("pt").map(|pt| pt * 4.0 / 3.0)).or_else(|| number("em").map(|em| em * size))
}
