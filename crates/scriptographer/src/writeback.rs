//! Paper.js's change log as a `document.patch`: removed items are removed, new items inserted,
//! changed items replaced, and a container whose children changed gets its new order.
//!
//! A changed path goes back through the node it came from: only what a script changed is written,
//! so what Paper.js can't show (more fills and strokes, gradient midpoints, corner and smooth anchors)
//! stays as it was.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value as Json, json};
use vectorcraft_color::{BlendMode, Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::{Appearance, Document, LayerColor, LineCap, LineJoin, Node, NodeId, NodeKind, TextWrap};
use vectorcraft_geom::{Anchor, AnchorKind, PathData, Point, SubPath};

use crate::binding::{Patch, node, number};
use crate::host::item_json;
use crate::text;
use crate::{Call, Error, Value, method, object};

/// Paper.js's `ChangeFlag`s.
mod change {
    pub const CHILDREN: u32 = 0x2;
    pub const INSERTION: u32 = 0x4;
    /// Everything that changes an item's own content or attributes.
    pub const CONTENT: u32 = 0x8 | 0x10 | 0x20 | 0x40 | 0x80 | 0x100 | 0x200 | 0x800;
}

/// One entry of the change log.
pub(crate) struct Change<H> {
    item: H,
    flags: u32,
    key: Option<NodeId>,
    inserted: bool,
    class: String,
}

impl<H: Clone> Change<H> {
    pub(crate) fn read(call: &mut dyn Call<H>, item: H, flags: u32) -> Result<Self, Error> {
        let key = match call.get(&item, "key")? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => Some(NodeId(n as u64)),
            _ => None,
        };
        let inserted = matches!(method(call, &item, "isInserted", &[])?, Value::Bool(true));
        let class = match call.get(&item, "className")? {
            Value::String(class) => class,
            _ => String::new(),
        };
        Ok(Self { item, flags, key, inserted, class })
    }
}

/// The patch for `changes`, made against `doc`, or `None` when they change nothing.
pub(crate) fn patch<H: Clone>(call: &mut dyn Call<H>, doc: &Arc<Document>, changes: Vec<Change<H>>) -> Result<Option<Patch<H>>, Error> {
    let mut remove = Vec::new();
    let mut replace = Vec::new();
    let mut news = News { insert: Vec::new(), inserted: Vec::new(), new: HashMap::new(), order: Vec::new() };
    let mut parents = Vec::new();
    let (mut replaced_ids, mut texts) = (Vec::new(), Vec::new());
    for change in &changes {
        match change.key {
            Some(id) if !change.inserted => {
                if change.flags & change::INSERTION != 0 {
                    remove.push(json!(id.0));
                }
            }
            Some(id) => {
                if change.flags & change::CONTENT != 0 {
                    replace.push(replaced(call, doc, node(doc, id)?, change)?);
                    replaced_ids.push(id);
                    if is_text(&change.class) {
                        texts.push(change.item.clone());
                    }
                }
                if change.flags & change::CHILDREN != 0 {
                    parents.push((id, change.item.clone()));
                }
            }
            // A new item inside another new item goes in with that one.
            None if change.inserted => {
                let parent = call.get(&change.item, "parent")?;
                let parent_key = match &parent {
                    Value::Js(parent) => match call.get(parent, "key")? {
                        Value::Number(n) => Some(n as u64),
                        _ if is_project(call, parent)? => None,
                        _ => continue,
                    },
                    _ => None,
                };
                let index = number(call.get(&change.item, "index")?) as u64;
                news.add(call, &change.item, json!(parent_key), Some(index))?;
            }
            None => {}
        }
    }
    // The items linked with changed text go with it, whether Paper.js reports them or not.
    let mut at = 0;
    while let Some(item) = texts.get(at).cloned() {
        at += 1;
        for linked in story_items(call, &item)? {
            if let Value::Number(key) = call.get(&linked, "key")?
                && !replaced_ids.contains(&NodeId(key as u64))
            {
                let change = Change::read(call, linked.clone(), change::CONTENT)?;
                replace.push(replaced(call, doc, node(doc, NodeId(key as u64))?, &change)?);
                replaced_ids.push(NodeId(key as u64));
                texts.push(linked);
            }
        }
    }
    let threads = threads(call, doc, &texts, &news)?;
    for (id, item) in parents {
        let children = object(call, &item, "children")?;
        let count = number(call.get(&children, "length")?) as usize;
        let mut slots = Vec::with_capacity(count);
        for i in 0..count {
            let child = object(call, &children, &i.to_string())?;
            match call.get(&child, "key")? {
                Value::Number(key) => slots.push(json!(key as u64)),
                _ => {
                    let paper_id = number(call.get(&child, "id")?) as u64;
                    let k = news.new.get(&paper_id).ok_or_else(|| Error::Script("a new item is missing from the change log".into()))?;
                    slots.push(json!({ "new": k }));
                }
            }
        }
        news.order.push(json!({ "parent": id.0, "children": slots }));
    }
    let mut params = Map::new();
    let News { insert, inserted, order, .. } = news;
    for (key, list) in [("remove", remove), ("insert", insert), ("replace", replace), ("order", order), ("threads", threads)] {
        if !list.is_empty() {
            params.insert(key.into(), Json::Array(list));
        }
    }
    if params.is_empty() {
        return Ok(None);
    }
    params.insert("label".into(), json!("Script"));
    Ok(Some(Patch::new(Json::Object(params), inserted)))
}

/// The new items of a patch: each one an insert, in its parent's place, the parents first.
struct News<H> {
    insert: Vec<Json>,
    /// The items, in the order of their inserts, which get the inserts' ids.
    inserted: Vec<H>,
    /// New items by Paper.js id: their index among the inserts.
    new: HashMap<u64, usize>,
    order: Vec<Json>,
}

impl<H: Clone> News<H> {
    /// Inserts `item` into `parent` (an id, `null` or `{new: k}`) at `index`, and a group's or
    /// layer's new children after it. The children it holds, new or already in the document, are
    /// its order list, so objects a script moved into a new group keep their ids.
    fn add(&mut self, call: &mut dyn Call<H>, item: &H, parent: Json, index: Option<u64>) -> Result<(), Error> {
        let k = self.insert.len();
        let mut json = export(call, item)?;
        let container = matches!(json.get(0).and_then(Json::as_str), Some("Group" | "Layer"));
        if container && let Some(props) = json.get_mut(1).and_then(Json::as_object_mut) {
            props.remove("children");
        }
        let mut node = new_node(&json)?;
        if let NodeKind::Text(t) = &mut node.kind {
            let link = link(call, item)?;
            **t = text::written(None, None, (**t).clone(), link);
        }
        let mut entry = json!({ "parent": parent, "node": node_json(&node)? });
        if let Some(index) = index {
            entry["index"] = json!(index);
        }
        self.insert.push(entry);
        self.inserted.push(item.clone());
        self.new.insert(number(call.get(item, "id")?) as u64, k);
        if !container {
            return Ok(());
        }
        let children = object(call, item, "children")?;
        let count = number(call.get(&children, "length")?) as usize;
        let mut slots = Vec::with_capacity(count);
        for i in 0..count {
            let child = object(call, &children, &i.to_string())?;
            match call.get(&child, "key")? {
                Value::Number(key) => slots.push(json!(key as u64)),
                _ => {
                    slots.push(json!({ "new": self.insert.len() }));
                    self.add(call, &child, json!({ "new": k }), None)?;
                }
            }
        }
        if !slots.is_empty() {
            self.order.push(json!({ "parent": { "new": k }, "children": slots }));
        }
        Ok(())
    }
}

fn is_text(class: &str) -> bool {
    matches!(class, "PointText" | "AreaText" | "PathText")
}

/// The text items of the story of the text item `item`, in order.
fn story_items<H: Clone>(call: &mut dyn Call<H>, item: &H) -> Result<Vec<H>, Error> {
    let story = object(call, item, "story")?;
    let items = object(call, &story, "textItems")?;
    let count = number(call.get(&items, "length")?) as usize;
    (0..count).map(|i| object(call, &items, &i.to_string())).collect()
}

/// Where the text item `item` is in its story.
fn link<H: Clone>(call: &mut dyn Call<H>, item: &H) -> Result<text::Link, Error> {
    let items = story_items(call, item)?;
    let Some(first) = items.first().filter(|_| items.len() > 1) else {
        return Ok(text::Link::Alone);
    };
    let first = number(call.get(first, "id")?);
    Ok(if number(call.get(item, "id")?) == first { text::Link::First } else { text::Link::Later })
}

/// The patch's `threads`: the stories of the text items `texts` and of the new text items, each
/// its items in order (keys, or `{new: k}`), where that differs from the document's threads.
fn threads<H: Clone>(call: &mut dyn Call<H>, doc: &Document, texts: &[H], news: &News<H>) -> Result<Vec<Json>, Error> {
    let mut items = texts.to_vec();
    for item in &news.inserted {
        if matches!(call.get(item, "className")?, Value::String(class) if is_text(&class)) {
            items.push(item.clone());
        }
    }
    let (mut threads, mut seen) = (Vec::new(), HashSet::new());
    for item in items {
        let linked = story_items(call, &item)?;
        let Some(first) = linked.first() else { continue };
        if !seen.insert(number(call.get(first, "id")?) as u64) {
            continue;
        }
        let (mut slots, mut keys) = (Vec::new(), Vec::new());
        for item in &linked {
            match call.get(item, "key")? {
                Value::Number(key) => {
                    slots.push(json!(key as u64));
                    keys.push(Some(NodeId(key as u64)));
                }
                _ => {
                    let paper_id = number(call.get(item, "id")?) as u64;
                    let k = news.new.get(&paper_id).ok_or_else(|| Error::Script("linked text that isn't in the project can't be written".into()))?;
                    slots.push(json!({ "new": k }));
                    keys.push(None);
                }
            }
        }
        let unchanged = match keys.first().copied().flatten().and_then(|key| text::thread(doc, key)) {
            Some(thread) => keys.len() == thread.len() && keys.iter().zip(thread).all(|(key, frame)| *key == Some(*frame)),
            None => keys.len() < 2,
        };
        if !unchanged {
            threads.push(Json::Array(slots));
        }
    }
    Ok(threads)
}

fn is_project<H>(call: &mut dyn Call<H>, item: &H) -> Result<bool, Error> {
    Ok(matches!(call.get(item, "className")?, Value::String(c) if c == "Project"))
}

/// The item's JSON, as `exportJSON()` gives it, with the gradients it shares through its
/// dictionary in place.
fn export<H: Clone>(call: &mut dyn Call<H>, item: &H) -> Result<Json, Error> {
    let json: Json = match method(call, item, "exportJSON", &[])? {
        Value::String(json) => serde_json::from_str(&json).map_err(|e| Error::Script(format!("exportJSON(): {e}")))?,
        _ => return Err(Error::Script("exportJSON() didn't return a string".into())),
    };
    Ok(match json.as_array().map(Vec::as_slice) {
        Some([Json::Array(dictionary), item]) if dictionary.first().and_then(Json::as_str) == Some("dictionary") => {
            let definitions = dictionary.get(1).and_then(Json::as_object).cloned().unwrap_or_default();
            resolved(item, &definitions)
        }
        _ => json,
    })
}

/// `json` with each reference to a definition, `["#id"]`, replaced by it.
fn resolved(json: &Json, definitions: &Map<String, Json>) -> Json {
    match json {
        Json::Array(list) => match list.as_slice() {
            [Json::String(id)] => definitions.get(id).cloned().unwrap_or_else(|| json.clone()),
            _ => Json::Array(list.iter().map(|json| resolved(json, definitions)).collect()),
        },
        Json::Object(map) => Json::Object(map.iter().map(|(key, json)| (key.clone(), resolved(json, definitions))).collect()),
        _ => json.clone(),
    }
}

fn node_json(node: &Node) -> Result<Json, Error> {
    serde_json::to_value(node).map_err(|e| Error::Script(format!("writing an object: {e}")))
}

/// The node `original` with the item's changes: content for paths and compound paths, attributes
/// for everything. Containers send no children, so they keep theirs. Text is written as
/// [`text::written`] says, by where it is in its story.
fn replaced<H: Clone>(call: &mut dyn Call<H>, doc: &Document, original: &Node, change: &Change<H>) -> Result<Json, Error> {
    let mut node = original.clone();
    let before = item_json(original, &Default::default());
    match change.class.as_str() {
        "Path" | "CompoundPath" => {
            let json = export(call, &change.item)?;
            apply(&mut node, &json, &before)?;
        }
        "PointText" | "AreaText" | "PathText" => {
            let json = export(call, &change.item)?;
            if let NodeKind::Text(t) = &original.kind {
                let view = text::story_view(doc, original.id, t);
                let (seen, shown) = text::unfolded(&view, &original.appearance);
                let (mut edited, first) = text::text_object(&json, Some((&seen, &shown)))?;
                let mut appearance = (*original.appearance).clone();
                text::fold(&mut edited, &first, shown.contains(&true), &mut appearance);
                node.appearance = appearance.into();
                let link = link(call, &change.item)?;
                node.kind = NodeKind::Text(Box::new(text::written(Some(t), Some(&view), edited, link)));
            }
            let props = json.get(1).and_then(Json::as_object).cloned().unwrap_or_default();
            apply_attributes(&mut node, &props, &before[1]);
        }
        _ => {
            let props = attributes(call, &change.item)?;
            apply_attributes(&mut node, &props, &before[1]);
        }
    }
    node.name = match call.get(&change.item, "name")? {
        Value::String(name) if !name.is_empty() => Some(name),
        _ => None,
    };
    node.visible = matches!(call.get(&change.item, "visible")?, Value::Bool(true));
    node.locked = matches!(call.get(&change.item, "locked")?, Value::Bool(true));
    let mut json = node_json(&node)?;
    if let Some(key) = original.children().map(|_| children_key(&node)) {
        // Sent without its children, a container keeps the ones it has.
        if let Some(kind) = json.get_mut("kind").and_then(Json::as_object_mut) {
            kind.remove(key);
        }
    }
    Ok(json)
}

fn children_key(node: &Node) -> &'static str {
    match node.kind {
        NodeKind::Envelope { .. } => "content",
        NodeKind::Repeat(_) => "source",
        _ => "children",
    }
}

/// The attributes of an item Paper.js doesn't export whole: opacity, blend mode, clipping.
fn attributes<H: Clone>(call: &mut dyn Call<H>, item: &H) -> Result<Map<String, Json>, Error> {
    let mut props = Map::new();
    if let Value::Number(opacity) = call.get(item, "opacity")? {
        props.insert("opacity".into(), json!(opacity));
    }
    if let Value::String(mode) = call.get(item, "blendMode")? {
        props.insert("blendMode".into(), json!(mode));
    }
    if let Value::Js(wrap) = call.get(item, "textWrap")? {
        props.insert(
            "textWrap".into(),
            json!({ "offset": number(call.get(&wrap, "offset")?), "invert": matches!(call.get(&wrap, "invert")?, Value::Bool(true)) }),
        );
    }
    Ok(props)
}

/// Applies a path or compound path's Paper.js JSON to `node`, where it differs from `before`, the
/// JSON the item was read with.
pub fn apply(node: &mut Node, json: &Json, before: &Json) -> Result<(), Error> {
    let props = json.get(1).and_then(Json::as_object).ok_or_else(|| Error::Script("not an item's JSON".into()))?;
    let before_props = before.get(1).cloned().unwrap_or(Json::Null);
    let path = path_data(json, node.path_data())?;
    // A live shape whose geometry a script changed is a plain path, as after any path edit.
    if let NodeKind::Path { path: p, live, .. } = &mut node.kind
        && *p != path
    {
        *p = path;
        *live = None;
    }
    let mut appearance = (*node.appearance).clone();
    apply_style(&mut appearance, props, &before_props);
    node.appearance = appearance.into();
    apply_attributes(node, props, &before_props);
    Ok(())
}

/// Opacity and blend mode, where they differ from `before`.
fn apply_attributes(node: &mut Node, props: &Map<String, Json>, before: &Json) {
    let changed = |key: &str| props.get(key).filter(|v| Some(*v) != before.get(key));
    if let Some(opacity) = changed("opacity").and_then(Json::as_f64) {
        node.opacity = opacity.clamp(0.0, 1.0) as f32;
    }
    if let Some(mode) = changed("blendMode").and_then(Json::as_str) {
        node.blend = blend_mode(mode);
    }
    // A text wrap that isn't set is left out of the JSON:
    let wrap = props.get("textWrap").unwrap_or(&Json::Null);
    if wrap != before.get("textWrap").unwrap_or(&Json::Null) {
        node.extras_mut().wrap = wrap.as_object().map(|w| TextWrap {
            offset: w.get("offset").and_then(Json::as_f64).filter(|n| n.is_finite()).unwrap_or(6.0),
            invert: w.get("invert").and_then(Json::as_bool).unwrap_or(false),
        });
    }
}

/// The top fill and stroke and their order, where they differ from `before`.
fn apply_style(appearance: &mut Appearance, props: &Map<String, Json>, before: &Json) {
    let changed = |key: &str| {
        let now = props.get(key).unwrap_or(&Json::Null);
        (now != before.get(key).unwrap_or(&Json::Null)).then_some(now)
    };
    if let Some(fill) = changed_paint(props.get("fillColor"), before.get("fillColor")) {
        appearance.set_fill(fill);
    }
    if let Some(stroke) = changed_paint(props.get("strokeColor"), before.get("strokeColor")) {
        appearance.set_stroke(stroke);
    }
    if let Some(order) = changed("paintOrder") {
        appearance.set_stroke_first(order.as_str() == Some("stroke"));
    }
    let Some(stroke) = appearance.stroke_mut() else {
        return;
    };
    if let Some(width) = changed("strokeWidth").and_then(Json::as_f64) {
        stroke.width = width.max(0.0);
    }
    if let Some(cap) = changed("strokeCap").and_then(Json::as_str) {
        stroke.cap = match cap {
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => LineCap::Butt,
        };
    }
    if let Some(join) = changed("strokeJoin").and_then(Json::as_str) {
        stroke.join = match join {
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
    }
    if let Some(limit) = changed("miterLimit").and_then(Json::as_f64) {
        stroke.miter_limit = limit.max(1.0);
    }
}

/// The paint of a Paper.js colour `now`, when it isn't the one of `before`, the colour it was read
/// as, beyond the precision of Paper.js's JSON: what Paper.js can't show stays while it's unchanged.
pub(crate) fn changed_paint(now: Option<&Json>, before: Option<&Json>) -> Option<Paint> {
    let read = |json: Option<&Json>| json.map_or(Paint::None, paint);
    let now = read(now);
    let json = |paint: &Paint| serde_json::to_value(paint).unwrap_or_default();
    (!is_close(&json(&now), &json(&read(before)))).then_some(now)
}

/// Are `a` and `b` the same, but for numbers within the precision of Paper.js's JSON?
fn is_close(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64().zip(y.as_f64()).is_some_and(|(x, y)| (x - y).abs() <= 1e-4 * x.abs().max(1.0)),
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| is_close(x, y)),
        (Json::Object(x), Json::Object(y)) => x.len() == y.len() && x.iter().all(|(key, x)| y.get(key).is_some_and(|y| is_close(x, y))),
        _ => a == b,
    }
}

/// A Paper.js colour as a paint: a solid colour, a gradient (see [`gradient`]) or none.
fn paint(json: &Json) -> Paint {
    match gradient(json) {
        Some(gradient) => Paint::Gradient(Box::new(gradient)),
        None => color(json).map_or(Paint::None, |(color, _)| Paint::solid(color)),
    }
}

/// A Paper.js gradient colour, `['gradient', ['Gradient', stops, radial], origin, destination,
/// highlight, aspect]`, as a gradient placed where it is. Stops are `[color, offset]`, spread
/// evenly where they have no offset.
fn gradient(json: &Json) -> Option<GradientPaint> {
    let (kind, list) = color_list(json)?;
    let [gradient, points @ ..] = list else {
        return None;
    };
    if kind != "gradient" {
        return None;
    }
    let gradient = classless(gradient.as_array()?, "Gradient");
    let stops = gradient.first().and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default();
    let last = stops.len().saturating_sub(1).max(1) as f64;
    let stops = stops
        .iter()
        .enumerate()
        .filter_map(|(i, stop)| {
            let stop = classless(stop.as_array()?, "GradientStop");
            let (color, opacity) = color(stop.first()?)?;
            let offset = stop.get(1).and_then(Json::as_f64).filter(|n| n.is_finite()).unwrap_or(i as f64 / last);
            Some(GradientStop { opacity, ..GradientStop::new(offset.clamp(0.0, 1.0) as f32, color) })
        })
        .collect();
    let point = |i: usize| {
        let xy = classless(points.get(i)?.as_array()?, "Point");
        match xy {
            [x, y] => Some(Point::new(x.as_f64().filter(|n| n.is_finite())?, y.as_f64().filter(|n| n.is_finite())?)),
            _ => None,
        }
    };
    let aspect = points.get(3).and_then(Json::as_f64).filter(|a| a.is_finite() && *a > 0.0).unwrap_or(1.0);
    let geom = GradientGeom { start: point(0)?, end: point(1)?, aspect, focal: point(2) };
    let kind = if gradient.get(1).and_then(Json::as_bool) == Some(true) { GradientKind::Radial } else { GradientKind::Linear };
    Some(GradientPaint { gradient: Gradient::new(kind, stops), geom: Some(geom), angle: geom.angle_deg(), swatch: None, freeform: None })
}

/// A Paper.js solid colour and its alpha: `[r, g, b, alpha?]` or `['gray', g, alpha?]`,
/// `['rgb', …]`, `['hsb', …]`, `['hsl', …]`.
fn color(json: &Json) -> Option<(Color, f32)> {
    let (kind, values) = color_list(json)?;
    let v = |i: usize| values.get(i).and_then(Json::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    let alpha = |i: usize| values.get(i).map_or(1.0, |_| v(i));
    match kind {
        "rgb" => Some((Color::rgb(v(0), v(1), v(2)), alpha(3))),
        "gray" => Some((Color::Gray { k: 1.0 - v(0) }, alpha(1))),
        "hsb" | "hsl" => {
            let hue = values.first().and_then(Json::as_f64).unwrap_or(0.0).rem_euclid(360.0) as f32;
            let [r, g, b] = if kind == "hsb" { hsb_to_rgb(hue, v(1), v(2)) } else { hsl_to_rgb(hue, v(1), v(2)) };
            Some((Color::rgb(r, g, b), alpha(3)))
        }
        _ => None,
    }
}

/// A Paper.js colour's type and components, also with its class in front, `['Color', …]`, as text
/// runs write them. Without a type, it's RGB.
fn color_list(json: &Json) -> Option<(&str, &[Json])> {
    let list = classless(json.as_array()?, "Color");
    Some(match list.split_first() {
        Some((Json::String(kind), values)) => (kind.as_str(), values),
        _ => ("rgb", list),
    })
}

/// `list` without `class` in front.
fn classless<'a>(list: &'a [Json], class: &str) -> &'a [Json] {
    match list.split_first() {
        Some((Json::String(name), rest)) if name == class => rest,
        _ => list,
    }
}

fn hsb_to_rgb(h: f32, s: f32, b: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h / 60.0) % 6.0;
        b - b * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn blend_mode(mode: &str) -> BlendMode {
    BlendMode::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(&mode.replace('-', " "))).unwrap_or(BlendMode::Normal)
}

/// The geometry of a path or compound path's JSON. Anchors keep the kind they had in `before`
/// where the subpath has as many as before; new ones are smooth where their handles line up.
pub(crate) fn path_data(json: &Json, before: Option<&PathData>) -> Result<PathData, Error> {
    let props = json.get(1).and_then(Json::as_object).ok_or_else(|| Error::Script("not an item's JSON".into()))?;
    let subpaths: Vec<&Map<String, Json>> = match json.get(0).and_then(Json::as_str) {
        Some("CompoundPath") => props
            .get("children")
            .and_then(Json::as_array)
            .map(|children| children.iter().filter_map(|c| c.get(1).and_then(Json::as_object)).collect())
            .unwrap_or_default(),
        _ => vec![props],
    };
    let subpaths = subpaths
        .into_iter()
        .enumerate()
        .map(|(i, props)| {
            let kinds = before.and_then(|b| b.subpaths.get(i)).map(|s| s.anchors.iter().map(|a| a.kind).collect::<Vec<_>>());
            let segments = props.get("segments").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default();
            let anchors: Vec<Anchor> = segments
                .iter()
                .enumerate()
                .map(|(j, s)| {
                    let kind = kinds.as_ref().filter(|k| k.len() == segments.len()).and_then(|k| k.get(j).copied());
                    anchor(s, kind)
                })
                .collect();
            SubPath::new(anchors, props.get("closed").and_then(Json::as_bool).unwrap_or(false))
        })
        .collect();
    Ok(PathData::new(subpaths))
}

/// A segment, `[x, y]` or `[[x, y], handleIn, handleOut, selection?]` with relative handles.
fn anchor(segment: &Json, kind: Option<AnchorKind>) -> Anchor {
    let point = |v: Option<&Json>| {
        let v = v.and_then(Json::as_array);
        let c = |i: usize| v.and_then(|v| v.get(i)).and_then(Json::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
        Point::new(c(0), c(1))
    };
    let (p, h_in, h_out) = match segment.get(0) {
        Some(Json::Array(_)) => {
            let p = point(segment.get(0));
            let (i, o) = (point(segment.get(1)), point(segment.get(2)));
            (p, Point::new(p.x + i.x, p.y + i.y), Point::new(p.x + o.x, p.y + o.y))
        }
        _ => {
            let p = point(Some(segment));
            (p, p, p)
        }
    };
    let kind = kind.unwrap_or_else(|| {
        let (a, b) = (h_in - p, h_out - p);
        let lined_up = a.hypot() > 0.0 && b.hypot() > 0.0 && (a.x * b.y - a.y * b.x).abs() <= 1e-9 * a.hypot() * b.hypot() && a.dot(b) < 0.0;
        if lined_up { AnchorKind::Smooth } else { AnchorKind::Corner }
    });
    Anchor { p, h_in, h_out, kind }
}

/// A new node from a Paper.js item's JSON: layers, groups, paths and compound paths.
pub fn new_node(json: &Json) -> Result<Node, Error> {
    let class = json.get(0).and_then(Json::as_str).unwrap_or_default();
    let props = json.get(1).and_then(Json::as_object).ok_or_else(|| Error::Script("not an item's JSON".into()))?;
    let children = || -> Result<Vec<Arc<Node>>, Error> {
        props.get("children").and_then(Json::as_array).into_iter().flatten().map(|c| new_node(c).map(Arc::new)).collect()
    };
    let id = NodeId(0);
    let mut node = match class {
        "Path" | "CompoundPath" => {
            let mut node = Node::path(id, path_data(json, None)?, Appearance::basic(Paint::None, Paint::None, 1.0));
            apply(&mut node, json, &Json::Null)?;
            node
        }
        "PointText" | "AreaText" | "PathText" => {
            let (mut t, first) = text::text_object(json, None)?;
            let mut appearance = Appearance::default();
            text::fold(&mut t, &first, false, &mut appearance);
            let mut node = Node::new(id, NodeKind::Text(Box::new(t)));
            node.appearance = appearance.into();
            node
        }
        "Group" => {
            let mut node = Node::group(id, children()?);
            if let NodeKind::Group { clip, .. } = &mut node.kind {
                *clip = props.get("clipped").and_then(Json::as_bool).unwrap_or(false);
            }
            node
        }
        "Layer" => {
            let mut node = Node::layer(id, props.get("name").and_then(Json::as_str).unwrap_or("Layer"), LayerColor::Preset(0));
            if let Some(children_list) = node.children_mut() {
                *children_list = children()?;
            }
            node
        }
        other => return Err(Error::Script(format!("a new {other} can't be added to the document yet"))),
    };
    apply_attributes(&mut node, props, &Json::Null);
    node.name = props.get("name").and_then(Json::as_str).map(str::to_owned).or(node.name);
    node.visible = props.get("visible").and_then(Json::as_bool).unwrap_or(true);
    node.locked = props.get("locked").and_then(Json::as_bool).unwrap_or(false);
    Ok(node)
}
