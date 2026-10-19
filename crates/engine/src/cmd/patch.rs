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
        "{label?: undo step name (default \"Edit\"), extend?: true adds to the newest undo step instead of recording one (for a caller whose last edit that step is), remove?: [ids], insert?: [{parent: id|null (null: the layer list), index?: position among the parent's children (bottom first; default: on top), node: object JSON as document.node returns it (its ids are replaced with new ones)}], replace?: [object JSON with its id (a container sent without its children keeps them)], order?: [{parent: id|null, children: [id|{new: k}]}] (the parent's children become exactly these, bottom first; listed objects from elsewhere move here; {new: k} places the k-th insert)} applied in that order, as one undo step; during an interaction (a drag) it applies beneath it, leaving out the objects the interaction changed → {ids: [the new id of each insert], skipped?: [the objects left out]}",
        has_doc,
        patch
    )]
}

/// One place an object ends up in an `order` list: an existing object, or the k-th insert.
enum Slot {
    Old(NodeId),
    New(usize),
}

/// A parsed patch.
struct Ops {
    remove: Vec<NodeId>,
    inserts: Vec<(Option<NodeId>, Option<usize>, Node)>,
    /// Each object, and whether it keeps its children.
    replaces: Vec<(Node, bool)>,
    orders: Vec<(Option<NodeId>, Vec<Slot>)>,
}

fn patch(s: &mut Session, p: &Value) -> Result<Value> {
    let label = p.get("label").and_then(Value::as_str).unwrap_or("Edit").to_owned();
    let mut ops = parse(p)?;
    if s.in_interaction() {
        return beneath_interaction(s, &mut ops).map(|(ids, skipped)| json!({ "ids": ids, "skipped": skipped }));
    }
    let extending = p.get("extend").and_then(Value::as_bool).unwrap_or(false) && s.extend_newest_undo_step();
    let ids = s.edit(&label, |doc, _| apply(doc, &ops));
    if extending {
        s.end_undo_group(false);
    }
    Ok(json!({ "ids": ids? }))
}

fn parse(p: &Value) -> Result<Ops> {
    let remove = list(p, "remove")?.iter().map(|v| node_id(v, "remove")).collect::<Result<Vec<_>>>()?;
    let inserts = list(p, "insert")?
        .iter()
        .map(|entry| {
            let parent = parent_id(entry)?;
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
            Ok((parent_id(entry)?, slots))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Ops { remove, inserts, replaces, orders })
}

/// Applies `ops` to `doc`. Returns the new ids of the inserts.
fn apply(doc: &mut Document, ops: &Ops) -> Result<Vec<u64>> {
    doc.remove_many(&ops.remove).map_err(|e| bad(CMD, format!("remove: {e}")))?;
    let mut placed = vec![false; ops.inserts.len()];
    ops.orders.iter().flat_map(|(_, slots)| slots).for_each(|slot| {
        if let Slot::New(k) = slot
            && let Some(p) = placed.get_mut(*k)
        {
            *p = true;
        }
    });
    // New objects get new ids; those an order list places wait for it.
    let mut fresh = Vec::with_capacity(ops.inserts.len());
    let mut pending: HashMap<usize, Arc<Node>> = HashMap::new();
    for (k, (parent, index, node)) in ops.inserts.iter().enumerate() {
        check_place(doc, *parent, node)?;
        let node = doc.reid(node);
        fresh.push(node.id.0);
        if placed.get(k).copied().unwrap_or(false) {
            pending.insert(k, Arc::new(node));
        } else {
            doc.insert(*parent, index.unwrap_or(usize::MAX), node).map_err(|e| bad(CMD, format!("insert: {e}")))?;
        }
    }
    for (node, keep) in &ops.replaces {
        replace(doc, node.clone(), *keep)?;
    }
    for (parent, slots) in &ops.orders {
        order(doc, *parent, slots, &mut pending)?;
    }
    if !pending.is_empty() {
        return Err(bad(CMD, "an insert placed by {new: k} was listed twice"));
    }
    Ok(fresh)
}

/// A patch while an interaction (a drag, a dialog's preview) is in progress applies beneath it:
/// to the document the interaction started from, so its next preview keeps the patch, and to the
/// preview shown now. What the interaction changed wins: the patch leaves out the objects whose
/// node it changed and the parents whose children it changed. It records no undo step of its own:
/// undoing the interaction goes back to the document with the patch.
/// Returns the new ids, and the objects whose changes it left out.
fn beneath_interaction(s: &mut Session, ops: &mut Ops) -> Result<(Vec<u64>, Vec<u64>)> {
    let st = s.doc_mut()?;
    let Some(it) = st.interaction.as_mut() else {
        return Err(bad(CMD, "no interaction in progress"));
    };
    let touched = Touched::between(&it.doc, &st.doc);
    let mut skipped: Vec<u64> =
        ops.remove.iter().chain(ops.replaces.iter().map(|(n, _)| &n.id)).filter(|id| touched.nodes.contains(id)).map(|id| id.0).collect();
    ops.remove.retain(|id| !touched.nodes.contains(id));
    ops.replaces.retain(|(node, _)| !touched.nodes.contains(&node.id));
    // A parent whose children the interaction changed keeps them; the new objects the patch
    // placed there go on top instead.
    let unordered: Vec<(Option<NodeId>, Vec<Slot>)> = ops.orders.extract_if(.., |(parent, _)| touched.lists.contains(parent)).collect();
    skipped.extend(unordered.iter().filter_map(|(parent, _)| parent.map(|p| p.0)));
    for slot in unordered.iter().flat_map(|(_, slots)| slots) {
        if let Slot::New(k) = slot
            && let Some(insert) = ops.inserts.get_mut(*k)
        {
            insert.1 = None;
        }
    }
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

/// What an interaction changed between the document it started from and its preview: the nodes
/// whose node is not the same (leaves; containers count only for their child lists), and the
/// parents (`None`: the layer list) whose children are not the same.
struct Touched {
    nodes: HashSet<NodeId>,
    lists: HashSet<Option<NodeId>>,
}

impl Touched {
    fn between(before: &Document, now: &Document) -> Self {
        let mut touched = Self { nodes: HashSet::new(), lists: HashSet::new() };
        touched.children(None, &before.layers, &now.layers);
        touched
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

fn parent_id(entry: &Value) -> Result<Option<NodeId>> {
    match entry.get("parent") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => node_id(v, "parent").map(Some),
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

fn order(doc: &mut Document, parent: Option<NodeId>, slots: &[Slot], pending: &mut HashMap<usize, Arc<Node>>) -> Result<()> {
    // Objects listed from other parents move here first.
    let here: HashSet<NodeId> = doc.children(parent).ok_or_else(|| bad(CMD, "order: the parent holds no objects"))?.iter().map(|c| c.id).collect();
    for slot in slots {
        if let Slot::Old(id) = slot
            && !here.contains(id)
        {
            doc.move_node(*id, parent, usize::MAX).map_err(|e| bad(CMD, format!("order: {e}")))?;
        }
    }
    let children = doc.children_mut(parent).map_err(|e| bad(CMD, format!("order: {e}")))?;
    let mut by_id: HashMap<NodeId, Arc<Node>> = std::mem::take(children).into_iter().map(|c| (c.id, c)).collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for slot in slots {
        let node = match slot {
            Slot::Old(id) => by_id.remove(id).ok_or_else(|| bad(CMD, format!("order: object {id} is listed twice")))?,
            Slot::New(k) => pending.remove(k).ok_or_else(|| bad(CMD, "order: an insert is listed twice"))?,
        };
        ordered.push(node);
    }
    if let Some(id) = by_id.keys().next() {
        return Err(bad(CMD, format!("order: object {id} is in the parent but not listed: remove or move it in the same patch")));
    }
    *children = ordered;
    Ok(())
}
