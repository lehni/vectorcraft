//! The document binding: a scope's project reads the document through Paper.js's host interface,
//! loading only what scripts touch, and its changes go back as one `document.patch`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value as Json;
use vectorcraft_doc::{Document, Node, NodeId, Selection};

use crate::host::{Columns, TYPE_NAMES, flags, item_json};
use crate::writeback::{self, Change};
use crate::{Call, Engine, Error, Runtime, Scope, Value, define_accessor, handle, method, object};

/// The document a runtime's project reads from.
pub(crate) struct Bound {
    pub doc: Arc<Document>,
    pub selection: Selection,
}

/// The changes a script made, as `document.patch` params, and the new items waiting for the ids
/// that patch gives them.
pub struct Patch<H> {
    pub params: Json,
    inserted: Vec<H>,
}

impl<E: Engine> Runtime<E> {
    /// Makes the document the host of `scope`'s project, and tracks the project's changes.
    /// `project.artboards` lists the document's artboards and `project.activeArtboard` is the
    /// active one: VectorCraft's, on top of Paper.js.
    pub fn bind(&mut self, scope: &Scope<E::Handle>, doc: Arc<Document>, selection: Selection) -> Result<(), Error> {
        let selected = selection.objects.clone();
        self.host.borrow_mut().bound = Some(Bound { doc, selection });
        let get_children = self.engine.function("getChildren", get_children)?;
        let get_item = self.engine.function("getItem", get_item)?;
        let get_ancestor_keys = self.engine.function("getAncestorKeys", get_ancestor_keys)?;
        let artboards = self.engine.function("artboards", artboards)?;
        let active_artboard = self.engine.function("activeArtboard", active_artboard)?;
        let global = self.engine.global()?;
        let paper = &self.paper;
        self.engine.with_call(|call| {
            let rectangle = object(call, paper, "Rectangle")?;
            call.host()?.borrow_mut().rectangle = Some(rectangle);
            let host = call.new_object()?;
            call.set(&host, "getChildren", &Value::Js(get_children))?;
            call.set(&host, "getItem", &Value::Js(get_item))?;
            call.set(&host, "getAncestorKeys", &Value::Js(get_ancestor_keys))?;
            let project = object(call, &scope.0, "project")?;
            method(call, &project, "setHost", &[Value::Js(host)])?;
            define_accessor(call, &global, &project, "artboards", artboards, None)?;
            define_accessor(call, &global, &project, "activeArtboard", active_artboard, None)?;
            call.set(&project, "trackChanges", &Value::Bool(true))?;
            register(call, &project, &selected)
        })
    }

    /// The objects selected in `scope`'s project, as the script left them.
    pub fn selected_ids(&mut self, scope: &Scope<E::Handle>) -> Result<BTreeSet<NodeId>, Error> {
        self.engine.with_call(|call| {
            let project = object(call, &scope.0, "project")?;
            let items = object(call, &project, "selectedItems")?;
            let count = number(call.get(&items, "length")?) as usize;
            let mut ids = BTreeSet::new();
            for i in 0..count {
                let item = object(call, &items, &i.to_string())?;
                if let Value::Number(key) = call.get(&item, "key")? {
                    ids.insert(NodeId(key as u64));
                }
            }
            Ok(ids)
        })
    }

    /// Takes the changes `scope`'s project logged since the last call, as a patch of the bound
    /// document, or `None` when nothing changed.
    pub fn take_patch(&mut self, scope: &Scope<E::Handle>) -> Result<Option<Patch<E::Handle>>, Error> {
        let host = self.host.clone();
        self.engine.with_call(|call| {
            let project = object(call, &scope.0, "project")?;
            let changes = handle(method(call, &project, "takeChanges", &[])?, "takeChanges()")?;
            let count = number(call.get(&changes, "length")?);
            let mut list = Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                let entry = object(call, &changes, &i.to_string())?;
                let item = object(call, &entry, "item")?;
                let flags = number(call.get(&entry, "flags")?) as u32;
                list.push(Change::read(call, item, flags)?);
            }
            // Not borrowed while the write-back calls into the script, which may need the host.
            let doc = host.borrow().bound.as_ref().map(|b| b.doc.clone()).ok_or_else(|| Error::Script("no document is bound".into()))?;
            writeback::patch(call, &doc, list)
        })
    }

    /// Brings `scope`'s project up to `doc` and `selection`, as they are after edits made outside
    /// the script: the items of the objects that changed reload from the document the next time
    /// the script touches them. Objects that didn't change keep their items, and so does
    /// everything when the document is the one the project already reads.
    pub fn update(&mut self, scope: &Scope<E::Handle>, doc: Arc<Document>, selection: Selection) -> Result<(), Error> {
        let selected = selection.objects.clone();
        let changes = {
            let mut host = self.host.borrow_mut();
            let Some(bound) = host.bound.as_mut() else {
                return Err(Error::Script("no document is bound".into()));
            };
            if Arc::ptr_eq(&bound.doc, &doc) && bound.selection == selection {
                return Ok(());
            }
            let changes = Columns::of_changes(&bound.doc, &bound.selection, &doc, &selection);
            *bound = Bound { doc, selection };
            changes
        };
        if changes.keys.is_empty() {
            return Ok(());
        }
        self.engine.with_call(|call| {
            let columns = columns_object(call, &changes)?;
            let project = object(call, &scope.0, "project")?;
            method(call, &project, "invalidate", &[Value::Js(columns)])?;
            register(call, &project, &selected)
        })
    }

    /// Gives the new items of an applied patch the ids `document.patch` returned, so later changes
    /// to them replace them instead of adding them again, and makes the patched document the one
    /// the project reads: what the script changed itself is never reloaded.
    ///
    /// The objects the patch left out (`skipped`: a drag in progress changed them, and wins) load
    /// again from the document, so the script goes on from what the drag made of them. When the
    /// document changed in other ways too since the project last caught up with it (`behind`: the
    /// patch was applied later), everything that changed loads again.
    pub fn apply_ids(
        &mut self,
        scope: &Scope<E::Handle>,
        patch: Patch<E::Handle>,
        result: &Json,
        doc: Arc<Document>,
        selection: Selection,
        behind: bool,
    ) -> Result<(), Error> {
        let mut reload = Columns::default();
        let mut host = self.host.borrow_mut();
        let bound = host.bound.as_mut().ok_or_else(|| Error::Script("no document is bound".into()))?;
        if behind {
            reload = Columns::of_changes(&bound.doc, &bound.selection, &doc, &selection);
        } else {
            for id in result["skipped"].as_array().into_iter().flatten().filter_map(Json::as_u64) {
                if let Some(node) = doc.node(NodeId(id)) {
                    reload.push(node, &selection);
                    let changed = if node.children().is_some() { flags::CHILDREN_CHANGED } else { flags::CONTENT_CHANGED };
                    if let Some(last) = reload.flags.last_mut() {
                        *last |= changed;
                    }
                }
            }
        }
        *bound = Bound { doc, selection };
        drop(host);
        let ids = result["ids"].as_array().cloned().unwrap_or_default();
        self.engine.with_call(|call| {
            for (item, id) in patch.inserted.iter().zip(ids) {
                if let Some(id) = id.as_f64() {
                    call.set(item, "key", &Value::Number(id))?;
                }
            }
            if !reload.keys.is_empty() {
                let columns = columns_object(call, &reload)?;
                let project = object(call, &scope.0, "project")?;
                method(call, &project, "invalidate", &[Value::Js(columns)])?;
            }
            Ok(())
        })
    }
}

impl<H> Patch<H> {
    pub(crate) fn new(params: Json, inserted: Vec<H>) -> Self {
        Self { params, inserted }
    }
}

/// Loads the selected objects' items, so the project's selection is complete even inside
/// containers the script hasn't touched.
fn register<H: Clone>(call: &mut dyn Call<H>, project: &H, selected: &[NodeId]) -> Result<(), Error> {
    for id in selected {
        method(call, project, "getItemByKey", &[Value::Number(id.0 as f64)])?;
    }
    Ok(())
}

/// A number, or 0.
pub(crate) fn number<H>(value: Value<H>) -> f64 {
    match value {
        Value::Number(n) if n.is_finite() => n,
        _ => 0.0,
    }
}

/// The node id in a key argument; `None` for `null`, the project.
fn key<H>(args: &[Value<H>]) -> Option<NodeId> {
    match args.first() {
        Some(Value::Number(n)) if n.is_finite() && *n >= 0.0 => Some(NodeId(*n as u64)),
        _ => None,
    }
}

impl<E: Engine> Runtime<E> {
    /// Makes artboard `index` of the document `project.activeArtboard`.
    pub fn set_artboard(&mut self, index: usize) {
        self.host.borrow_mut().artboard = index;
    }
}

/// `project.artboards`: the document's artboards, in order, each as `{ name, bounds }`.
fn artboards<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let boards = with_bound(call, |doc, _| doc.artboards.iter().map(|a| (a.name.clone(), a.rect)).collect::<Vec<_>>())?;
    let list = boards.into_iter().map(|(name, rect)| artboard(call, name, rect).map(Value::Js)).collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Js(call.new_array(&list)?))
}

/// `project.activeArtboard`: the active one of `project.artboards` (the first when there is no
/// such artboard), `null` without artboards.
fn active_artboard<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let index = call.host()?.borrow().artboard;
    match with_bound(call, |doc, _| doc.artboards.get(index).or(doc.artboards.first()).map(|a| (a.name.clone(), a.rect)))? {
        Some((name, rect)) => Ok(Value::Js(artboard(call, name, rect)?)),
        None => Ok(Value::Null),
    }
}

/// An artboard as scripts see it: `{ name, bounds }`.
fn artboard<H: Clone>(call: &mut dyn Call<H>, name: String, rect: vectorcraft_geom::Rect) -> Result<H, Error> {
    let rectangle = call.host()?.borrow().rectangle.clone().ok_or_else(|| Error::Script("no document is bound".into()))?;
    let board = call.new_object()?;
    call.set(&board, "name", &Value::String(name))?;
    let args = [rect.x0, rect.y0, rect.width(), rect.height()].map(Value::Number);
    let bounds = call.construct(&rectangle, &args)?;
    call.set(&board, "bounds", &bounds)?;
    Ok(board)
}

/// Runs `f` with the bound document and selection.
fn with_bound<H, R>(call: &mut dyn Call<H>, f: impl FnOnce(&Document, &Selection) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let host = host.borrow();
    let bound = host.bound.as_ref().ok_or_else(|| Error::Script("no document is bound".into()))?;
    Ok(f(&bound.doc, &bound.selection))
}

/// `source.getChildren(key)`: the records of a container's children, as columns.
fn get_children<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let parent = key(args);
    let columns = with_bound(call, |doc, selection| Columns::of_children(doc, parent, selection))?.unwrap_or_default();
    Ok(Value::Js(columns_object(call, &columns)?))
}

/// `columns` as the object Paper.js reads them from.
fn columns_object<H: Clone>(call: &mut dyn Call<H>, columns: &Columns) -> Result<H, Error> {
    let result = call.new_object()?;
    let keys = call.new_float64_array(&columns.keys)?;
    let types = call.new_uint8_array(&columns.types)?;
    let type_names: Vec<_> = TYPE_NAMES.iter().map(|n| Value::String((*n).to_owned())).collect();
    let type_names = call.new_array(&type_names)?;
    let flags = call.new_uint16_array(&columns.flags)?;
    let boxes = call.new_float64_array(&columns.boxes)?;
    let names = call.new_object()?;
    for (index, name) in &columns.names {
        call.set(&names, &index.to_string(), &Value::String(name.clone()))?;
    }
    for (key, value) in [("keys", keys), ("types", types), ("typeNames", type_names), ("flags", flags), ("boxes", boxes), ("names", names)] {
        call.set(&result, key, &Value::Js(value))?;
    }
    Ok(result)
}

/// `source.getItem(key)`: the item's own content as Paper.js JSON.
fn get_item<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let id = key(args).ok_or_else(|| Error::Script("getItem: no key".into()))?;
    let json = with_bound(call, |doc, selection| doc.node(id).map(|node| item_json(node, selection)))?;
    let json = json.ok_or_else(|| Error::Script(format!("getItem: no object {id}")))?;
    Ok(Value::String(json.to_string()))
}

/// `source.getAncestorKeys(key)`: the keys from the layer down to the item's parent.
fn get_ancestor_keys<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let id = key(args).ok_or_else(|| Error::Script("getAncestorKeys: no key".into()))?;
    let mut ancestry = with_bound(call, |doc, _| doc.ancestry(id))?.unwrap_or_default();
    ancestry.pop();
    let keys: Vec<_> = ancestry.iter().map(|id| Value::Number(id.0 as f64)).collect();
    Ok(Value::Js(call.new_array(&keys)?))
}

/// The node `id` of `doc`, for write-back.
pub(crate) fn node(doc: &Document, id: NodeId) -> Result<&Node, Error> {
    doc.node(id).ok_or_else(|| Error::Script(format!("no object {id}")))
}
