//! The document as Paper.js sees it through its host: one record per child of a container
//! (as columns), and each item's own content as Paper.js JSON (`importJSON` form).
//!
//! Layers, groups, paths, compound paths and text have Paper.js classes. Every other kind of object
//! is a `HostItem`: Paper.js knows its bounds and attributes, never its content. VectorCraft has no
//! per-object matrix, so records leave their matrix out (identity), and boxes are in document
//! coordinates, but for text: its transform is its matrix, and its boxes are in its own space.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use vectorcraft_color::{BlendMode, Color, GradientGeom, GradientKind, Paint};
use vectorcraft_doc::{AnchorRef, Document, LineCap, LineJoin, Node, NodeId, NodeKind, Scaling, Selection, TextKind, TextObject};
use vectorcraft_geom::{Affine, PathData, Rect};

use crate::text;

/// The Paper.js classes of the records' `types`.
pub const TYPE_NAMES: &[&str] = &["Layer", "Group", "Path", "CompoundPath", "HostItem", "PointText", "AreaText", "PathText"];

const LAYER: u8 = 0;
const GROUP: u8 = 1;
const PATH: u8 = 2;
const COMPOUND_PATH: u8 = 3;
const HOST_ITEM: u8 = 4;
const POINT_TEXT: u8 = 5;
const AREA_TEXT: u8 = 6;
const PATH_TEXT: u8 = 7;

/// Record flags, as Paper.js reads them.
pub mod flags {
    pub const SELECTED: u16 = 1;
    pub const VISIBLE: u16 = 2;
    pub const LOCKED: u16 = 4;
    pub const STROKE_SCALING: u16 = 8;
    /// For `project.invalidate()`: the item's own content changed.
    pub const CONTENT_CHANGED: u16 = 16;
    /// For `project.invalidate()`: the container's child list changed.
    pub const CHILDREN_CHANGED: u16 = 32;
    pub const CLIP_MASK: u16 = 64;
    pub const GUIDE: u16 = 128;
    pub const EMPTY: u16 = 256;
}

/// Records of a container's children, bottom first, as the columns Paper.js reads.
#[derive(Debug, Default, PartialEq)]
pub struct Columns {
    pub keys: Vec<f64>,
    pub types: Vec<u8>,
    pub flags: Vec<u16>,
    /// Twelve numbers per record: bounds, stroke bounds and rough bounds, each `x, y, width,
    /// height`.
    pub boxes: Vec<f64>,
    /// Names by record index.
    pub names: Vec<(usize, String)>,
    /// Matrices by record index, `[a, b, c, d, tx, ty]`: text's, whose boxes are in its own space.
    pub matrices: Vec<(usize, [f64; 6])>,
    /// For `project.invalidate()`: the keys of the definitions that changed.
    pub definitions: Vec<String>,
}

impl Columns {
    /// Is there nothing to invalidate?
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.definitions.is_empty()
    }

    /// Adds the record of `node`.
    pub fn push(&mut self, node: &Node, selection: &Selection) {
        let (kind, mut flags) = match &node.kind {
            NodeKind::Layer { .. } => (LAYER, 0),
            NodeKind::Group { .. } => (GROUP, 0),
            NodeKind::Path { clipping, guide, .. } => (PATH, if *clipping { flags::CLIP_MASK } else { 0 } | if *guide { flags::GUIDE } else { 0 }),
            NodeKind::Compound { .. } => (COMPOUND_PATH, 0),
            NodeKind::Text(t) => (
                match t.kind {
                    TextKind::Point => POINT_TEXT,
                    TextKind::Area { .. } => AREA_TEXT,
                    TextKind::OnPath { .. } => PATH_TEXT,
                },
                0,
            ),
            _ => (HOST_ITEM, 0),
        };
        flags |= flags::STROKE_SCALING;
        if node.visible {
            flags |= flags::VISIBLE;
        }
        if node.locked {
            flags |= flags::LOCKED;
        }
        if selection.objects.contains(&node.id) {
            flags |= flags::SELECTED;
        }
        // Text has a matrix in Paper.js, its transform, so its boxes are in its own space:
        let text = match &node.kind {
            NodeKind::Text(t) => Some(t),
            _ => None,
        };
        if let Some(t) = text
            && t.xf != Affine::IDENTITY
        {
            self.matrices.push((self.keys.len(), t.xf.as_coeffs()));
        }
        let bounds = match text {
            Some(t) => text_bounds(t),
            None => node.geometric_bounds(),
        };
        if bounds.is_none() {
            flags |= flags::EMPTY;
        }
        let stroke = if text.is_some() { bounds } else { node.visual_bounds().or(bounds) };
        let rough = if text.is_some() { bounds } else { rough_bounds(node).or(stroke) };
        for rect in [bounds, stroke, rough] {
            let rect = rect.unwrap_or(Rect::ZERO);
            self.boxes.extend([rect.x0, rect.y0, rect.width(), rect.height()]);
        }
        if let Some(name) = &node.name {
            self.names.push((self.keys.len(), name.clone()));
        }
        // Keys are node ids: below 2^53 in every document VectorCraft makes (`MAX_NEXT_ID`).
        self.keys.push(node.id.0 as f64);
        self.types.push(kind);
        self.flags.push(flags);
    }

    /// The records of the children of `parent` (`None`: the layers), or `None` when it holds none.
    pub fn of_children(doc: &Document, parent: Option<NodeId>, selection: &Selection) -> Option<Self> {
        let children = doc.children(parent)?;
        let mut columns = Self::default();
        for child in children {
            columns.push(child, selection);
        }
        Some(columns)
    }
}

impl Columns {
    /// The records `project.invalidate()` needs to bring a project that reads `old` (with
    /// `old_selection`) up to `new`: every changed object's fresh record, flagged when its own
    /// content changed or, for layers and groups, its child list. Subtrees whose nodes are shared
    /// between the two documents are skipped, so the cost follows what changed, not the document.
    pub fn of_changes(old: &Document, old_selection: &Selection, new: &Document, new_selection: &Selection) -> Self {
        let mut columns = Self::default();
        // Objects whose selection changed get a fresh record even where their node didn't change.
        let mut reselected: BTreeSet<NodeId> = old_selection.objects.iter().chain(&new_selection.objects).copied().collect();
        reselected.retain(|id| old_selection.objects.contains(id) != new_selection.objects.contains(id));
        let reanchored: BTreeSet<NodeId> = old_selection
            .anchors
            .keys()
            .chain(new_selection.anchors.keys())
            .filter(|id| old_selection.anchors.get(id) != new_selection.anchors.get(id))
            .copied()
            .collect();
        let changed_threads = changed_threads(old, new);
        let mut walk = Walk {
            columns: &mut columns,
            selection: new_selection,
            reselected: &reselected,
            reanchored: &reanchored,
            changed_threads: &changed_threads,
        };
        walk.children(&old.layers, &new.layers);
        let names: BTreeSet<&str> = old.patterns.iter().chain(&new.patterns).map(|p| p.name.as_str()).collect();
        columns.definitions = names.into_iter().filter(|name| old.pattern(name) != new.pattern(name)).map(|name| format!("pattern/{name}")).collect();
        // Frames of those threads in subtrees that didn't change, which the walk skips.
        for id in &changed_threads {
            if !columns.keys.contains(&(id.0 as f64))
                && let Some(node) = new.node(*id)
            {
                columns.push(node, new_selection);
                if let Some(last) = columns.flags.last_mut() {
                    *last |= flags::CONTENT_CHANGED;
                }
            }
        }
        columns
    }
}

/// The frames of the threads of which a frame changed between `old` and `new`, or that changed
/// themselves: each frame's record holds the whole story, so they all change with it.
fn changed_threads(old: &Document, new: &Document) -> BTreeSet<NodeId> {
    let changed = |id: &NodeId| match (old.node(*id), new.node(*id)) {
        (Some(a), Some(b)) => !std::ptr::eq(a, b),
        (a, b) => a.is_some() != b.is_some(),
    };
    let mut frames = BTreeSet::new();
    for thread in old.text_threads.iter().chain(&new.text_threads) {
        let same = old.text_threads.contains(thread) && new.text_threads.contains(thread);
        if !same || thread.iter().any(changed) {
            frames.extend(thread.iter().copied());
        }
    }
    frames
}

/// [`Columns::of_changes`] in progress.
struct Walk<'a> {
    columns: &'a mut Columns,
    selection: &'a Selection,
    reselected: &'a BTreeSet<NodeId>,
    reanchored: &'a BTreeSet<NodeId>,
    changed_threads: &'a BTreeSet<NodeId>,
}

impl Walk<'_> {
    fn children(&mut self, old: &[Arc<Node>], new: &[Arc<Node>]) {
        // Children are usually in the same places: look them up by id only where they aren't.
        let by_id: std::collections::HashMap<NodeId, &Arc<Node>> = old.iter().map(|n| (n.id, n)).collect();
        for (i, node) in new.iter().enumerate() {
            let before = old.get(i).filter(|o| o.id == node.id).or_else(|| by_id.get(&node.id).copied());
            if let Some(before) = before {
                self.node(before, node);
            }
        }
    }

    fn node(&mut self, old: &Arc<Node>, new: &Arc<Node>) {
        let selected = self.reselected.contains(&new.id);
        let reanchored = self.reanchored.contains(&new.id) || self.changed_threads.contains(&new.id);
        if Arc::ptr_eq(old, new) && !selected && !reanchored {
            return;
        }
        let mut changed = 0;
        match (&old.kind, &new.kind) {
            (NodeKind::Layer { children: a, .. }, NodeKind::Layer { children: b, .. })
            | (NodeKind::Group { children: a, .. }, NodeKind::Group { children: b, .. }) => {
                if !a.iter().map(|c| c.id).eq(b.iter().map(|c| c.id)) {
                    changed |= flags::CHILDREN_CHANGED;
                }
                if item_json(old, self.selection) != item_json(new, self.selection) {
                    changed |= flags::CONTENT_CHANGED;
                }
                self.children(a, b);
            }
            _ if !Arc::ptr_eq(old, new) || reanchored => changed |= flags::CONTENT_CHANGED,
            _ => {}
        }
        self.columns.push(new, self.selection);
        if let Some(last) = self.columns.flags.last_mut() {
            *last |= changed;
        }
    }
}

/// The bounds of text in its own space, as its matrix places them.
fn text_bounds(t: &TextObject) -> Option<Rect> {
    match &t.kind {
        TextKind::Area { frame } => frame.bounds(),
        TextKind::OnPath { path, .. } => path.bounds(),
        TextKind::Point => t.cached_bounds,
    }
}

/// The box Paper.js rejects hit tests with: the control points, grown by as far as strokes reach.
fn rough_bounds(node: &Node) -> Option<Rect> {
    let outset = node.appearance.outset();
    let control = match &node.kind {
        NodeKind::Path { path, .. } => path.control_bounds(),
        NodeKind::Compound { children, .. } => children.iter().filter_map(|c| c.path_data()?.control_bounds()).reduce(|a, b| a.union(b)),
        _ => return node.reach_bounds(),
    };
    control.map(|r| r.inflate(outset, outset))
}

/// The item's own content as Paper.js JSON: a container's children come from its records instead,
/// and the record owns name, visibility, locking and selection.
pub fn item_json(node: &Node, selection: &Selection) -> Value {
    match &node.kind {
        NodeKind::Layer { .. } => json!(["Layer", attributes(node)]),
        NodeKind::Group { clip, .. } => {
            let mut props = attributes(node);
            if *clip {
                props.insert("clipped".into(), json!(true));
            }
            json!(["Group", props])
        }
        NodeKind::Path { path, .. } => {
            let mut props = attributes(node);
            style(node, &mut props);
            let anchors = selection.anchors.get(&node.id);
            match path.subpaths.as_slice() {
                [subpath] => {
                    props.insert("segments".into(), segments(path, 0, anchors));
                    props.insert("closed".into(), json!(subpath.closed));
                    json!(["Path", props])
                }
                // A path of several subpaths is a compound path to Paper.js.
                _ => {
                    props.insert("children".into(), subpaths(path, anchors));
                    json!(["CompoundPath", props])
                }
            }
        }
        NodeKind::Compound { children, .. } => {
            let mut props = attributes(node);
            style(node, &mut props);
            let paths: Vec<Value> = children
                .iter()
                .filter_map(|c| {
                    let path = c.path_data()?;
                    Some(subpaths(path, selection.anchors.get(&c.id)))
                })
                .flat_map(|v| v.as_array().cloned().unwrap_or_default())
                .collect();
            props.insert("children".into(), Value::Array(paths));
            json!(["CompoundPath", props])
        }
        NodeKind::Text(t) => {
            let mut props = attributes(node);
            // Text keeps its matrix, as Paper.js's does:
            props.insert("applyMatrix".into(), json!(false));
            let (t, first) = text::unfolded(t, &node.appearance);
            text::properties(&t, &first, &mut props);
            json!([text::class(&t), props])
        }
        _ => json!(["HostItem", attributes(node)]),
    }
}

/// A definition as Paper.js JSON, by its key: `pattern/<name>` is a pattern, its art as one group
/// in pattern space, where its tile starts at the origin, as the pattern's paints place it. A
/// pattern the document doesn't have is empty, so the colours that use it keep it. None for any
/// other key.
pub fn definition_json(doc: &Document, key: &str) -> Option<Value> {
    let name = key.strip_prefix("pattern/")?;
    Some(match doc.pattern(name) {
        Some(pattern) => {
            let tile = pattern.tile;
            let to_pattern = Affine::translate(-tile.origin().to_vec2());
            let children: Vec<Value> = pattern
                .art
                .iter()
                .filter_map(|node| {
                    let mut node = (**node).clone();
                    node.transform(to_pattern, Scaling { patterns: true, ..Scaling::default() });
                    tree_json(&node)
                })
                .collect();
            json!([
                "PatternDefinition",
                ["Group", { "applyMatrix": true, "children": children }],
                { "tile": ["Rectangle", 0, 0, tile.width(), tile.height()] }
            ])
        }
        None => json!(["PatternDefinition", ["Group", {}]]),
    })
}

/// An object and all it holds as Paper.js JSON, for the art of definitions, which has no records.
/// Kinds Paper.js has no class for are left out.
fn tree_json(node: &Node) -> Option<Value> {
    if !matches!(node.kind, NodeKind::Group { .. } | NodeKind::Path { .. } | NodeKind::Compound { .. } | NodeKind::Text(_)) {
        return None;
    }
    let mut json = item_json(node, &Selection::default());
    if let Some(props) = json.get_mut(1).and_then(Value::as_object_mut) {
        if let NodeKind::Group { children, .. } = &node.kind {
            props.insert("children".into(), children.iter().filter_map(|child| tree_json(child)).collect());
        }
        if let Some(name) = &node.name {
            props.insert("name".into(), json!(name));
        }
        if !node.visible {
            props.insert("visible".into(), json!(false));
        }
    }
    Some(json)
}

/// A path as Paper.js `Path` JSON, or `CompoundPath` JSON with several subpaths: a text's path.
pub(crate) fn path_json(path: &PathData) -> Value {
    match path.subpaths.as_slice() {
        [subpath] => json!(["Path", { "applyMatrix": true, "segments": segments(path, 0, None), "closed": subpath.closed }]),
        _ => json!(["CompoundPath", { "applyMatrix": true, "children": subpaths(path, None) }]),
    }
}

/// The attributes every item has in its JSON.
fn attributes(node: &Node) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("applyMatrix".into(), json!(true));
    if node.opacity < 1.0 {
        props.insert("opacity".into(), json!(node.opacity));
    }
    if node.blend != BlendMode::Normal {
        props.insert("blendMode".into(), json!(blend_mode(node.blend)));
    }
    if let Some(wrap) = &node.extras().wrap {
        props.insert("textWrap".into(), json!({ "offset": wrap.offset, "invert": wrap.invert }));
    }
    props
}

/// The topmost fill and stroke, as Paper.js's single fill and stroke, and their order.
fn style(node: &Node, props: &mut Map<String, Value>) {
    let appearance = &node.appearance;
    // Gradients that fit the object fit the boxes the renderer gives them:
    let bounds = node.geometric_bounds().unwrap_or(Rect::ZERO);
    if let Some(color) = appearance.fill().and_then(|f| color(&f.paint, Some(bounds))) {
        props.insert("fillColor".into(), color);
    }
    if let Some(stroke) = appearance.stroke()
        && let Some(color) = color(&stroke.paint, Some(stroke.paint_bounds(bounds)))
    {
        props.insert("strokeColor".into(), color);
        props.insert("strokeWidth".into(), json!(stroke.width));
        props.insert(
            "strokeCap".into(),
            json!(match stroke.cap {
                LineCap::Butt => "butt",
                LineCap::Round => "round",
                LineCap::Square => "square",
            }),
        );
        props.insert(
            "strokeJoin".into(),
            json!(match stroke.join {
                LineJoin::Miter => "miter",
                LineJoin::Round => "round",
                LineJoin::Bevel => "bevel",
            }),
        );
        props.insert("miterLimit".into(), json!(stroke.miter_limit));
        if let Some(dash) = &stroke.dash {
            props.insert("dashArray".into(), json!(dash.pattern));
            props.insert("dashOffset".into(), json!(dash.offset));
        }
        if appearance.is_stroke_first() {
            props.insert("paintOrder".into(), json!("stroke"));
        }
    }
}

/// The styles new art takes in the app, as a Paper.js project has them: `currentStyle`, what new
/// items a script makes get, as in Scriptographer, where they took the document's current style,
/// and `currentTextStyle`, what new text gets instead: a fill of `text_fill` without a stroke, as
/// the Type tool's new type has. Gradients that fit their object have no art to fit yet.
pub fn current_style(fill: &Paint, stroke: &Paint, stroke_width: f64, text_fill: &Paint) -> Value {
    json!({
        "currentStyle": { "fillColor": color(fill, None), "strokeColor": color(stroke, None), "strokeWidth": stroke_width },
        "currentTextStyle": { "fillColor": color(text_fill, None), "strokeColor": null },
    })
}

/// A paint as a Paper.js colour: a solid colour, a linear or radial gradient, placed where it is or
/// fitted to `bounds`, or a pattern, its definition by key and its matrix the paint's. Freeform
/// gradients have no Paper.js form.
pub(crate) fn color(paint: &Paint, bounds: Option<Rect>) -> Option<Value> {
    match paint {
        Paint::Solid { color, .. } => Some(solid(*color, 1.0)),
        Paint::Gradient(g) if g.gradient.kind != GradientKind::Freeform => {
            let geom = match g.geom {
                Some(geom) => geom,
                None => GradientGeom::fit(g.gradient.kind, bounds?, g.angle),
            };
            let stops: Vec<Value> = g.gradient.stops.iter().map(|s| json!([solid(s.color, s.opacity), s.offset])).collect();
            let point = |p: vectorcraft_geom::Point| json!([p.x, p.y]);
            Some(json!([
                "gradient",
                ["Gradient", stops, g.gradient.kind == GradientKind::Radial],
                point(geom.start),
                point(geom.end),
                geom.focal.map(point),
                geom.aspect
            ]))
        }
        Paint::Pattern { pattern, xf } => Some(json!(["pattern", [format!("@pattern/{pattern}")], xf.as_coeffs()])),
        _ => None,
    }
}

/// A colour as a Paper.js colour, with `alpha` when it isn't opaque.
fn solid(color: Color, alpha: f32) -> Value {
    let mut value = match color {
        Color::Rgb { r, g, b } => json!([r, g, b]),
        // VectorCraft's grey is ink (0 = white), Paper.js's is lightness (0 = black).
        Color::Gray { k } => json!(["gray", 1.0 - k]),
        // Paper.js has no CMYK or Lab: they show as their display colour.
        _ => {
            let [r, g, b] = color.to_rgb();
            json!([r, g, b])
        }
    };
    if alpha < 1.0
        && let Some(list) = value.as_array_mut()
    {
        list.push(json!(alpha));
    }
    value
}

fn blend_mode(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "normal",
        BlendMode::Darken => "darken",
        BlendMode::Multiply => "multiply",
        BlendMode::ColorBurn => "color-burn",
        BlendMode::Lighten => "lighten",
        BlendMode::Screen => "screen",
        BlendMode::ColorDodge => "color-dodge",
        BlendMode::Overlay => "overlay",
        BlendMode::SoftLight => "soft-light",
        BlendMode::HardLight => "hard-light",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
    }
}

/// The subpaths of `path` as Paper.js `Path` JSON, for a compound path's children.
fn subpaths(path: &PathData, anchors: Option<&BTreeSet<AnchorRef>>) -> Value {
    let paths: Vec<Value> = (0..path.subpaths.len())
        .map(|i| {
            let closed = path.subpaths.get(i).is_some_and(|s| s.closed);
            json!(["Path", { "applyMatrix": true, "segments": segments(path, i, anchors), "closed": closed }])
        })
        .collect();
    Value::Array(paths)
}

/// Subpath `index` as Paper.js segments, `[point, handleIn, handleOut, selection?]` with handles
/// relative to their point.
fn segments(path: &PathData, index: usize, anchors: Option<&BTreeSet<AnchorRef>>) -> Value {
    let Some(subpath) = path.subpaths.get(index) else {
        return json!([]);
    };
    let segments: Vec<Value> = subpath
        .anchors
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let point = json!([a.p.x, a.p.y]);
            let handle_in = json!([a.h_in.x - a.p.x, a.h_in.y - a.p.y]);
            let handle_out = json!([a.h_out.x - a.p.x, a.h_out.y - a.p.y]);
            // Segment selection: bit 1 is the point.
            if anchors.is_some_and(|set| set.contains(&(index, i))) {
                json!([point, handle_in, handle_out, 1])
            } else {
                json!([point, handle_in, handle_out])
            }
        })
        .collect();
    Value::Array(segments)
}
