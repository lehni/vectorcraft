//! The document binding: a scope's project reads the document through Paper.js's item source,
//! loading only what scripts touch, and its changes go back as one `document.patch`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value as Json;
use vectorcraft_doc::{Document, Node, NodeId, Selection};

use crate::source::{Columns, TYPE_NAMES, item_json};
use crate::writeback::{self, Change};
use crate::{Call, Engine, Error, Runtime, Scope, Value, handle, method, object};

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
    /// Makes the document the item source of `scope`'s project, and tracks the project's changes.
    pub fn bind(&mut self, scope: &Scope<E::Handle>, doc: Arc<Document>, selection: Selection) -> Result<(), Error> {
        let selected = selection.objects.clone();
        self.host.borrow_mut().bound = Some(Bound { doc, selection });
        let get_children = self.engine.function("getChildren", get_children)?;
        let get_item = self.engine.function("getItem", get_item)?;
        let get_ancestor_keys = self.engine.function("getAncestorKeys", get_ancestor_keys)?;
        self.engine.with_call(|call| {
            let source = call.new_object()?;
            call.set(&source, "getChildren", &Value::Js(get_children))?;
            call.set(&source, "getItem", &Value::Js(get_item))?;
            call.set(&source, "getAncestorKeys", &Value::Js(get_ancestor_keys))?;
            let project = object(call, &scope.0, "project")?;
            method(call, &project, "setSource", &[Value::Js(source)])?;
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
            let bound = host.borrow();
            let doc = bound.bound.as_ref().map(|b| &b.doc).ok_or_else(|| Error::Script("no document is bound".into()))?;
            writeback::patch(call, doc, list)
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
    pub fn apply_ids(&mut self, patch: Patch<E::Handle>, result: &Json, doc: Arc<Document>, selection: Selection) -> Result<(), Error> {
        if let Some(bound) = self.host.borrow_mut().bound.as_mut() {
            *bound = Bound { doc, selection };
        }
        let ids = result["ids"].as_array().cloned().unwrap_or_default();
        self.engine.with_call(|call| {
            for (item, id) in patch.inserted.iter().zip(ids) {
                if let Some(id) = id.as_f64() {
                    call.set(item, "key", &Value::Number(id))?;
                }
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
