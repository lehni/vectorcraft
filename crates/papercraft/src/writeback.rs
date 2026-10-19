//! Paper.js's change log as a `document.patch`: removed items are removed, new items inserted,
//! changed items replaced, and a container whose children changed gets its new order.
//!
//! A changed path goes back through the node it came from: only what a script changed is written,
//! so what Paper.js can't show (more fills and strokes, gradients, corner and smooth anchors)
//! stays as it was.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value as Json, json};
use vectorcraft_color::{BlendMode, Color, Paint};
use vectorcraft_doc::{Appearance, Document, LayerColor, LineCap, LineJoin, Node, NodeId, NodeKind};
use vectorcraft_geom::{Anchor, AnchorKind, PathData, Point, SubPath};

use crate::binding::{Patch, node, number};
use crate::source::item_json;
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
    /// Paper.js's own id, to recognise new items in their parents' child lists.
    id: f64,
    key: Option<NodeId>,
    inserted: bool,
    class: String,
}

impl<H: Clone> Change<H> {
    pub(crate) fn read(call: &mut dyn Call<H>, item: H, flags: u32) -> Result<Self, Error> {
        let id = number(call.get(&item, "id")?);
        let key = match call.get(&item, "key")? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => Some(NodeId(n as u64)),
            _ => None,
        };
        let inserted = matches!(method(call, &item, "isInserted", &[])?, Value::Bool(true));
        let class = match call.get(&item, "className")? {
            Value::String(class) => class,
            _ => String::new(),
        };
        Ok(Self { item, flags, id, key, inserted, class })
    }
}

/// The patch for `changes`, made against `doc`, or `None` when they change nothing.
pub(crate) fn patch<H: Clone>(call: &mut dyn Call<H>, doc: &Arc<Document>, changes: Vec<Change<H>>) -> Result<Option<Patch<H>>, Error> {
    let mut remove = Vec::new();
    let mut replace = Vec::new();
    let mut order = Vec::new();
    let mut insert = Vec::new();
    let mut inserted = Vec::new();
    // New items by Paper.js id: their index among the inserts.
    let mut new: HashMap<u64, usize> = HashMap::new();
    let mut parents = Vec::new();
    for change in &changes {
        match change.key {
            Some(id) if !change.inserted => {
                if change.flags & change::INSERTION != 0 {
                    remove.push(json!(id.0));
                }
            }
            Some(id) => {
                if change.flags & change::CONTENT != 0 {
                    replace.push(replaced(call, node(doc, id)?, change)?);
                }
                if change.flags & change::CHILDREN != 0 {
                    parents.push((id, change.item.clone()));
                }
            }
            // A new item inside another new item comes with that one's JSON.
            None if change.inserted => {
                let parent = call.get(&change.item, "parent")?;
                let parent_is_new = match &parent {
                    Value::Js(parent) => matches!(call.get(parent, "key")?, Value::Null | Value::Undefined) && !is_project(call, parent)?,
                    _ => false,
                };
                if parent_is_new {
                    continue;
                }
                let parent_key = match &parent {
                    Value::Js(parent) => match call.get(parent, "key")? {
                        Value::Number(n) => Some(n as u64),
                        _ => None,
                    },
                    _ => None,
                };
                let json = export(call, &change.item)?;
                let node = new_node(&json)?;
                let index = number(call.get(&change.item, "index")?) as u64;
                new.insert(change.id as u64, insert.len());
                insert.push(json!({ "parent": parent_key, "index": index, "node": node_json(&node)? }));
                inserted.push(change.item.clone());
            }
            None => {}
        }
    }
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
                    let k = new.get(&paper_id).ok_or_else(|| Error::Script("a new item is missing from the change log".into()))?;
                    slots.push(json!({ "new": k }));
                }
            }
        }
        order.push(json!({ "parent": id.0, "children": slots }));
    }
    let mut params = Map::new();
    for (key, list) in [("remove", remove), ("insert", insert), ("replace", replace), ("order", order)] {
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

fn is_project<H>(call: &mut dyn Call<H>, item: &H) -> Result<bool, Error> {
    Ok(matches!(call.get(item, "className")?, Value::String(c) if c == "Project"))
}

/// The item's JSON, as `exportJSON()` gives it.
fn export<H: Clone>(call: &mut dyn Call<H>, item: &H) -> Result<Json, Error> {
    match method(call, item, "exportJSON", &[])? {
        Value::String(json) => serde_json::from_str(&json).map_err(|e| Error::Script(format!("exportJSON(): {e}"))),
        _ => Err(Error::Script("exportJSON() didn't return a string".into())),
    }
}

fn node_json(node: &Node) -> Result<Json, Error> {
    serde_json::to_value(node).map_err(|e| Error::Script(format!("writing an object: {e}")))
}

/// The node `original` with the item's changes: content for paths and compound paths, attributes
/// for everything. Containers send no children, so they keep theirs.
fn replaced<H: Clone>(call: &mut dyn Call<H>, original: &Node, change: &Change<H>) -> Result<Json, Error> {
    let mut node = original.clone();
    let before = item_json(original, &Default::default());
    match change.class.as_str() {
        "Path" | "CompoundPath" => {
            let json = export(call, &change.item)?;
            apply(&mut node, &json, &before)?;
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
    Ok(props)
}

/// Applies a path or compound path's Paper.js JSON to `node`, where it differs from `before`, the
/// JSON the item was read with.
pub fn apply(node: &mut Node, json: &Json, before: &Json) -> Result<(), Error> {
    let props = json.get(1).and_then(Json::as_object).ok_or_else(|| Error::Script("not an item's JSON".into()))?;
    let before_props = before.get(1).cloned().unwrap_or(Json::Null);
    let path = path_data(json, node.path_data())?;
    if let NodeKind::Path { path: p, .. } = &mut node.kind {
        *p = path;
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
}

/// The top fill and stroke, where they differ from `before`.
fn apply_style(appearance: &mut Appearance, props: &Map<String, Json>, before: &Json) {
    let changed = |key: &str| {
        let now = props.get(key).unwrap_or(&Json::Null);
        (now != before.get(key).unwrap_or(&Json::Null)).then_some(now)
    };
    if let Some(fill) = changed("fillColor") {
        appearance.set_fill(color(fill).map_or(Paint::None, Paint::solid));
    }
    if let Some(stroke) = changed("strokeColor") {
        appearance.set_stroke(color(stroke).map_or(Paint::None, Paint::solid));
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

/// A Paper.js colour: `[r, g, b, alpha?]` or `['gray', g]`, `['rgb', …]`, `['hsb', …]`,
/// `['hsl', …]`. Gradients have no VectorCraft form here yet.
fn color(json: &Json) -> Option<Color> {
    let list = json.as_array()?;
    let (kind, values) = match list.first()?.as_str() {
        Some(kind) => (kind, &list[1..]),
        None => ("rgb", &list[..]),
    };
    let v = |i: usize| values.get(i).and_then(Json::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    match kind {
        "rgb" => Some(Color::rgb(v(0), v(1), v(2))),
        "gray" => Some(Color::Gray { k: 1.0 - v(0) }),
        "hsb" | "hsl" => {
            let hue = values.first().and_then(Json::as_f64).unwrap_or(0.0).rem_euclid(360.0) as f32;
            let [r, g, b] = if kind == "hsb" { hsb_to_rgb(hue, v(1), v(2)) } else { hsl_to_rgb(hue, v(1), v(2)) };
            Some(Color::rgb(r, g, b))
        }
        _ => None,
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
fn path_data(json: &Json, before: Option<&PathData>) -> Result<PathData, Error> {
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
