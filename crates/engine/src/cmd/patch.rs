//! `document.patch`: many node-level changes as one undo step, in the document's own JSON (the
//! form `document.node` returns). For callers that keep their own copy of the art and write back
//! what changed, such as scripts, where one command per change would not scale.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_doc::{Document, Node, NodeId, NodeKind};

use super::{CommandSpec, bad, cmd, has_doc};
use crate::{Result, Session};

const CMD: &str = "document.patch";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "document.patch",
        "Patch Document",
        [],
        None,
        "{label?: undo step name (default \"Edit\"), extend?: true adds to the newest undo step instead of recording one (for a caller whose last edit that step is), remove?: [ids], insert?: [{parent: id|null|{new: k} (null: the layer list; {new: k}: the k-th insert, an earlier one), index?: position among the parent's children (bottom first; default: on top), node: object JSON as document.node returns it (its ids are replaced with new ones)}], replace?: [object JSON with its id (a container sent without its children keeps them)], order?: [{parent: id|null|{new: k}, children: [id|{new: k}]}] (the parent's children become exactly these, bottom first; listed objects from elsewhere move here; {new: k} places the k-th insert), keep?: {objects: [ids], lists: [parent id|null]} (what changed since the caller's copy of the document, which the patch leaves out)} applied in that order, as one undo step; during an interaction (a drag) it applies beneath it, leaving out the objects the interaction changed too → {ids: [the new id of each insert], skipped?: [the objects left out]}",
        has_doc,
        patch
    )]
}

/// One place an object ends up in an `order` list: an existing object, or the k-th insert.
enum Slot {
    Old(NodeId),
    New(usize),
}

/// Where an insert or an order list goes: the layer list, an existing object, or the k-th insert.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    Layers,
    Old(NodeId),
    New(usize),
}

/// A parsed patch.
struct Ops {
    remove: Vec<NodeId>,
    inserts: Vec<(Parent, Option<usize>, Node)>,
    /// Each object, and whether it keeps its children.
    replaces: Vec<(Node, bool)>,
    orders: Vec<(Parent, Vec<Slot>)>,
}

fn patch(s: &mut Session, p: &Value) -> Result<Value> {
    let label = p.get("label").and_then(Value::as_str).unwrap_or("Edit").to_owned();
    let mut ops = parse(p)?;
    let keep = p.get("keep").map(Touched::parse).transpose()?;
    if s.in_interaction() {
        return beneath_interaction(s, &mut ops, keep.unwrap_or_default()).map(|(ids, skipped)| json!({ "ids": ids, "skipped": skipped }));
    }
    let skipped = keep.map(|keep| leave_out(&mut ops, &keep));
    let extending = p.get("extend").and_then(Value::as_bool).unwrap_or(false) && s.extend_newest_undo_step();
    let ids = s.edit(&label, |doc, _| apply(doc, &ops));
    if extending {
        s.end_undo_group(false);
    }
    Ok(match skipped {
        Some(skipped) => json!({ "ids": ids?, "skipped": skipped }),
        None => json!({ "ids": ids? }),
    })
}

fn parse(p: &Value) -> Result<Ops> {
    let remove = list(p, "remove")?.iter().map(|v| node_id(v, "remove")).collect::<Result<Vec<_>>>()?;
    let inserts = list(p, "insert")?
        .iter()
        .enumerate()
        .map(|(k, entry)| {
            // A new parent is an insert before this one.
            let parent = parent(entry, k)?;
            let index = match entry.get("index") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    Some(v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(CMD, "insert index must be a non-negative integer"))?)
                }
            };
            let node = entry.get("node").ok_or_else(|| bad(CMD, "each insert needs a node"))?;
            Ok((parent, index, parse_node(node.clone(), "insert")?))
        })
        .collect::<Result<Vec<_>>>()?;
    let replaces = list(p, "replace")?
        .iter()
        .map(|node| {
            let keep = keeps_children(node);
            Ok((parse_node(with_empty_children(node, keep), "replace")?, keep))
        })
        .collect::<Result<Vec<_>>>()?;
    let orders = list(p, "order")?
        .iter()
        .map(|entry| {
            let children = entry.get("children").and_then(Value::as_array).ok_or_else(|| bad(CMD, "each order entry needs a children array"))?;
            let slots = children
                .iter()
                .map(|c| match c.get("new") {
                    Some(k) => k
                        .as_u64()
                        .and_then(|k| usize::try_from(k).ok())
                        .filter(|k| *k < inserts.len())
                        .map(Slot::New)
                        .ok_or_else(|| bad(CMD, "{new: k} must name an insert by its index")),
                    None => node_id(c, "order").map(Slot::Old),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((parent(entry, inserts.len())?, slots))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Ops { remove, inserts, replaces, orders })
}

/// Applies `ops` to `doc`. Returns the new ids of the inserts.
fn apply(doc: &mut Document, ops: &Ops) -> Result<Vec<u64>> {
    doc.remove_many(&ops.remove).map_err(|e| bad(CMD, format!("remove: {e}")))?;
    // New objects get new ids and go in at once, an order list then puts them in their place.
    let mut fresh = Vec::with_capacity(ops.inserts.len());
    for (parent, index, node) in &ops.inserts {
        let parent = resolve(*parent, &fresh)?;
        check_place(doc, parent, node)?;
        let node = doc.reid(node);
        fresh.push(node.id);
        doc.insert(parent, index.unwrap_or(usize::MAX), node).map_err(|e| bad(CMD, format!("insert: {e}")))?;
    }
    for (node, keep) in &ops.replaces {
        replace(doc, node.clone(), *keep)?;
    }
    // Text comes without its layout, which is a cache, so lay out what came:
    let mut text: Vec<NodeId> = ops.replaces.iter().map(|(node, _)| node.id).collect();
    text.extend(&fresh);
    super::typecmd::refresh_bounds_of(doc, &text);
    let orders = ops
        .orders
        .iter()
        .map(|(parent, slots)| {
            let slots = slots.iter().map(|slot| resolve(Parent::from(slot), &fresh)?.ok_or_else(|| bad(CMD, "order: no such object")));
            Ok((resolve(*parent, &fresh)?, slots.collect::<Result<Vec<_>>>()?))
        })
        .collect::<Result<Vec<(Option<NodeId>, Vec<NodeId>)>>>()?;
    // Every listed object moves to its parent first, so the lists can come in any order.
    for (parent, slots) in &orders {
        gather(doc, *parent, slots)?;
    }
    for (parent, slots) in &orders {
        order(doc, *parent, slots)?;
    }
    Ok(fresh.iter().map(|id| id.0).collect())
}

/// The object `parent` names, with `fresh` the ids of the inserts so far; `None`: the layer list.
fn resolve(parent: Parent, fresh: &[NodeId]) -> Result<Option<NodeId>> {
    match parent {
        Parent::Layers => Ok(None),
        Parent::Old(id) => Ok(Some(id)),
        Parent::New(k) => fresh.get(k).copied().map(Some).ok_or_else(|| bad(CMD, "{new: k} must name an earlier insert")),
    }
}

impl From<&Slot> for Parent {
    fn from(slot: &Slot) -> Self {
        match slot {
            Slot::Old(id) => Parent::Old(*id),
            Slot::New(k) => Parent::New(*k),
        }
    }
}

/// A patch while an interaction (a drag, a dialog's preview) is in progress applies beneath it:
/// to the document the interaction started from, so its next preview keeps the patch, and to the
/// preview shown now. What the interaction changed wins: the patch leaves out the objects whose
/// node it changed and the parents whose children it changed. It records no undo step of its own:
/// undoing the interaction goes back to the document with the patch.
/// Returns the new ids, and the objects whose changes it left out, with those in `keep`.
fn beneath_interaction(s: &mut Session, ops: &mut Ops, mut keep: Touched) -> Result<(Vec<u64>, Vec<u64>)> {
    let st = s.doc_mut()?;
    let Some(it) = st.interaction.as_mut() else {
        return Err(bad(CMD, "no interaction in progress"));
    };
    keep.children(None, &it.doc.layers, &st.doc.layers);
    let skipped = leave_out(ops, &keep);
    // The same new ids in both documents: both count on from the higher of their counters. Both
    // change, or neither.
    let next = it.doc.peek_next_id().max(st.doc.peek_next_id());
    let (mut base, mut preview) = (it.doc.clone(), st.doc.clone());
    for doc in [&mut base, &mut preview] {
        Arc::make_mut(doc).reserve_ids(next);
    }
    let ids = apply(Arc::make_mut(&mut base), ops)?;
    if apply(Arc::make_mut(&mut preview), ops)? != ids {
        return Err(bad(CMD, "the interaction's preview gave the new objects other ids"));
    }
    it.doc = base;
    st.doc = preview;
    st.revision += 1;
    Ok((ids, skipped))
}

/// Leaves the objects and child lists in `touched` out of `ops`. Returns the objects left out.
fn leave_out(ops: &mut Ops, touched: &Touched) -> Vec<u64> {
    let mut skipped: Vec<u64> =
        ops.remove.iter().chain(ops.replaces.iter().map(|(n, _)| &n.id)).filter(|id| touched.nodes.contains(id)).map(|id| id.0).collect();
    ops.remove.retain(|id| !touched.nodes.contains(id));
    ops.replaces.retain(|(node, _)| !touched.nodes.contains(&node.id));
    // A parent whose children the interaction changed keeps them; the new objects the patch
    // placed there go on top instead.
    let touches = |parent: &Parent| match parent {
        Parent::Layers => touched.lists.contains(&None),
        Parent::Old(id) => touched.lists.contains(&Some(*id)),
        Parent::New(_) => false,
    };
    let unordered: Vec<(Parent, Vec<Slot>)> = ops.orders.extract_if(.., |(parent, _)| touches(parent)).collect();
    skipped.extend(unordered.iter().filter_map(|(parent, _)| match parent {
        Parent::Old(id) => Some(id.0),
        _ => None,
    }));
    for slot in unordered.iter().flat_map(|(_, slots)| slots) {
        if let Slot::New(k) = slot
            && let Some(insert) = ops.inserts.get_mut(*k)
        {
            insert.1 = None;
        }
    }
    skipped
}

/// What changed between two versions of a document, as `document.patch`'s `keep` takes it: for a
/// caller whose copy of the document is `before`, to leave out of its patch what others changed
/// since.
pub fn changes_between(before: &Document, now: &Document) -> Value {
    let mut touched = Touched::default();
    touched.children(None, &before.layers, &now.layers);
    let mut objects: Vec<u64> = touched.nodes.iter().map(|id| id.0).collect();
    let mut lists: Vec<Option<u64>> = touched.lists.iter().map(|id| id.map(|id| id.0)).collect();
    objects.sort_unstable();
    lists.sort_unstable();
    json!({ "objects": objects, "lists": lists })
}

/// What changed between two versions of a document: the nodes whose node is not the same
/// (leaves; containers count only for their child lists), and the parents (`None`: the layer
/// list) whose children are not the same.
#[derive(Default)]
struct Touched {
    nodes: HashSet<NodeId>,
    lists: HashSet<Option<NodeId>>,
}

impl Touched {
    /// `keep`'s `{objects, lists}`.
    fn parse(keep: &Value) -> Result<Self> {
        let nodes = list(keep, "objects")?.iter().map(|v| node_id(v, "keep objects")).collect::<Result<_>>()?;
        let lists =
            list(keep, "lists")?.iter().map(|v| if v.is_null() { Ok(None) } else { node_id(v, "keep lists").map(Some) }).collect::<Result<_>>()?;
        Ok(Self { nodes, lists })
    }

    fn children(&mut self, parent: Option<NodeId>, before: &[Arc<Node>], now: &[Arc<Node>]) {
        if !before.iter().map(|n| n.id).eq(now.iter().map(|n| n.id)) {
            self.lists.insert(parent);
        }
        let by_id: HashMap<NodeId, &Arc<Node>> = before.iter().map(|n| (n.id, n)).collect();
        for node in now {
            match by_id.get(&node.id) {
                Some(old) if Arc::ptr_eq(old, node) => {}
                Some(old) => match (old.children(), node.children()) {
                    (Some(a), Some(b)) => self.children(Some(node.id), a, b),
                    _ => {
                        self.nodes.insert(node.id);
                    }
                },
                None => {
                    self.nodes.insert(node.id);
                }
            }
        }
    }
}

fn list<'a>(p: &'a Value, key: &str) -> Result<&'a [Value]> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(bad(CMD, format!("{key} must be an array"))),
    }
}

fn node_id(v: &Value, key: &str) -> Result<NodeId> {
    v.as_u64().map(NodeId).ok_or_else(|| bad(CMD, format!("{key}: object ids are non-negative integers")))
}

/// An entry's `parent`: an object's id, `null` for the layer list, or `{new: k}` for the k-th
/// insert, one of the first `inserts`.
fn parent(entry: &Value, inserts: usize) -> Result<Parent> {
    match entry.get("parent") {
        None | Some(Value::Null) => Ok(Parent::Layers),
        Some(v) => match v.get("new") {
            Some(k) => k
                .as_u64()
                .and_then(|k| usize::try_from(k).ok())
                .filter(|k| *k < inserts)
                .map(Parent::New)
                .ok_or_else(|| bad(CMD, "parent {new: k} must name an earlier insert")),
            None => node_id(v, "parent").map(Parent::Old),
        },
    }
}

fn parse_node(json: Value, key: &str) -> Result<Node> {
    serde_json::from_value(json).map_err(|e| bad(CMD, format!("{key}: {e}")))
}

/// The key a container kind keeps its children under, from the kind's JSON.
fn children_key(kind: &Value) -> Option<&'static str> {
    match kind.get("type")?.as_str()? {
        "layer" | "group" | "compound" | "blend" | "intertwine" => Some("children"),
        "envelope" => Some("content"),
        "repeat" => Some("source"),
        _ => None,
    }
}

/// Whether `node` is a container sent without its children, which keeps the ones it has.
fn keeps_children(node: &Value) -> bool {
    let kind = &node["kind"];
    children_key(kind).is_some_and(|key| kind.get(key).is_none())
}

/// `node` with an empty children list where it has none, so it parses; the kept children go in
/// after.
fn with_empty_children(node: &Value, keep: bool) -> Value {
    let mut node = node.clone();
    if keep
        && let Some(key) = children_key(&node["kind"])
        && let Some(kind) = node.get_mut("kind").and_then(Value::as_object_mut)
    {
        kind.insert(key.into(), json!([]));
    }
    node
}

/// Layers hold the document's art: only a layer goes in the layer list, as the layer list does.
fn check_place(doc: &Document, parent: Option<NodeId>, node: &Node) -> Result<()> {
    let layer = matches!(node.kind, NodeKind::Layer { .. });
    match parent {
        None if !layer => Err(bad(CMD, "only layers go in the layer list (parent null)")),
        Some(p) if doc.node(p).is_none() => Err(bad(CMD, format!("no object {p}"))),
        _ => Ok(()),
    }
}

fn replace(doc: &mut Document, mut node: Node, keep: bool) -> Result<()> {
    let id = node.id;
    let parent = doc.parent_of(id);
    check_place(doc, parent, &node)?;
    let old = doc.node_mut(id).ok_or_else(|| bad(CMD, format!("replace: no object {id}")))?;
    if keep {
        let kept = old.children_mut().map(std::mem::take).unwrap_or_default();
        match node.children_mut() {
            Some(children) => *children = kept,
            None if kept.is_empty() => {}
            None => return Err(bad(CMD, format!("replace: object {id} has children and its new kind can't hold them"))),
        }
    }
    *old = node;
    Ok(())
}

/// Moves the objects in `slots` that are elsewhere into `parent`.
fn gather(doc: &mut Document, parent: Option<NodeId>, slots: &[NodeId]) -> Result<()> {
    let here: HashSet<NodeId> = doc.children(parent).ok_or_else(|| bad(CMD, "order: the parent holds no objects"))?.iter().map(|c| c.id).collect();
    for id in slots {
        if !here.contains(id) {
            doc.move_node(*id, parent, usize::MAX).map_err(|e| bad(CMD, format!("order: {e}")))?;
        }
    }
    Ok(())
}

/// Makes `parent`'s children exactly `slots`, gathered there.
fn order(doc: &mut Document, parent: Option<NodeId>, slots: &[NodeId]) -> Result<()> {
    let children = doc.children_mut(parent).map_err(|e| bad(CMD, format!("order: {e}")))?;
    let mut by_id: HashMap<NodeId, Arc<Node>> = std::mem::take(children).into_iter().map(|c| (c.id, c)).collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for id in slots {
        ordered.push(by_id.remove(id).ok_or_else(|| bad(CMD, format!("order: object {id} is listed twice")))?);
    }
    if let Some(id) = by_id.keys().next() {
        return Err(bad(CMD, format!("order: object {id} is in the parent but not listed: remove or move it in the same patch")));
    }
    *children = ordered;
    Ok(())
}
