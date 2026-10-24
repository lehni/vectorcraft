//! Effects that scripts define (Scriptographer's `Effect`s), `script.<type>`, in Effect ›
//! Scriptographer while their script runs. The app never runs a script to draw one: the script
//! keeps what it drew in the effect's parameters, `_output`, with the fingerprint of what it drew
//! it from, `_input`. The effect draws that output while its input is the same, and otherwise the
//! geometry as it is, as a missing plug-in does.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde_json::{Map, Value};
use vectorcraft_doc::Node;
use vectorcraft_geom::{PathData, Shape};
use vectorcraft_plugins::Manifest;

use crate::{GeomContext, apply_geometry_with};

/// The prefix of script effects' ids.
pub const PREFIX: &str = "script.";
/// The parameter that holds what the script drew, as [`PathData`].
pub const OUTPUT: &str = "_output";
/// The parameter that holds the [`fingerprint`] of what the script drew it from.
pub const INPUT: &str = "_input";

static EFFECTS: RwLock<Vec<Arc<Manifest>>> = RwLock::new(Vec::new());
static REVISION: AtomicU64 = AtomicU64::new(0);

/// The script effect's type in `id` (`script.wobble` → `wobble`).
pub fn script_type(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX)
}

/// The effect id of the script effect `kind`.
pub fn effect_id(kind: &str) -> String {
    format!("{PREFIX}{kind}")
}

/// Adds the script effect `manifest` describes, its id being its type, in place of one with the
/// same type.
pub fn register(manifest: Manifest) {
    let mut effects = EFFECTS.write().unwrap_or_else(|e| e.into_inner());
    effects.retain(|m| m.id != manifest.id);
    effects.push(Arc::new(manifest));
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Removes the script effect of type `kind`, once its script ends.
pub fn unregister(kind: &str) {
    let mut effects = EFFECTS.write().unwrap_or_else(|e| e.into_inner());
    let count = effects.len();
    effects.retain(|m| m.id != kind);
    if effects.len() != count {
        REVISION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The script effects running scripts define, in the order they were defined.
pub fn list() -> Vec<Arc<Manifest>> {
    EFFECTS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The manifest of the script effect `id`, while its script runs.
pub fn manifest(id: &str) -> Option<Arc<Manifest>> {
    let kind = script_type(id)?;
    EFFECTS.read().unwrap_or_else(|e| e.into_inner()).iter().find(|m| m.id == kind).cloned()
}

/// Changes whenever a script effect is defined or removed.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// The fingerprint of a script effect's input: its geometry and its parameters, as 16 hex digits.
pub fn fingerprint(path: &PathData, params: &Value) -> String {
    let mut h = FNV_BASIS;
    for subpath in &path.subpaths {
        h = fnv1a(h, &[u8::from(subpath.closed)]);
        for anchor in &subpath.anchors {
            for p in [anchor.p, anchor.h_in, anchor.h_out] {
                h = fnv1a(h, &p.x.to_bits().to_le_bytes());
                h = fnv1a(h, &p.y.to_bits().to_le_bytes());
            }
        }
    }
    // Keys are sorted, as serde_json keeps them; what the script keeps itself doesn't count.
    let given: Map<String, Value> =
        params.as_object().into_iter().flatten().filter(|(key, _)| !key.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
    h = fnv1a(h, Value::Object(given).to_string().as_bytes());
    format!("{h:016x}")
}

/// What the script effect with `params` draws of `path`: its output while `path` is its input.
pub(crate) fn apply(params: &Value, path: &PathData) -> Option<PathData> {
    let input = params.get(INPUT)?.as_str()?;
    if input != fingerprint(path, params) {
        return None;
    }
    serde_json::from_value(params.get(OUTPUT)?.clone()).ok()
}

/// The input of one of an object's script effects: where it is in the object's effects, the
/// geometry it draws from, and that geometry's [`fingerprint`] with its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub index: usize,
    pub path: PathData,
    pub fingerprint: String,
}

/// The inputs of the script effects of path or compound path `n`, as the app draws them: the
/// geometry after the effects before each. A hidden one isn't drawn, so it has none.
pub fn inputs(n: &Node) -> Vec<Input> {
    let Some((path, _)) = crate::reshape::leaf_geometry(n) else {
        return vec![];
    };
    let bounds = path.to_bezpath().bounding_box();
    let ctx = GeomContext::of(n);
    let effects = &n.appearance.effects;
    effects
        .iter()
        .enumerate()
        .filter(|(_, e)| e.visible && script_type(&e.id).is_some())
        .map(|(index, e)| {
            let path = apply_geometry_with(effects.get(..index).unwrap_or_default(), &path, bounds, &ctx);
            let fingerprint = fingerprint(&path, &crate::merged_params(&e.id, &e.params));
            Input { index, path, fingerprint }
        })
        .collect()
}

/// The FNV-1a offset basis: the hash of no bytes.
const FNV_BASIS: u64 = 0xcbf29ce484222325;

/// The 64-bit FNV-1a hash `h` of some bytes, carried on over `bytes`: stable across builds, as
/// documents keep it.
fn fnv1a(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
}
