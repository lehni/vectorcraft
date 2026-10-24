//! Live effects as Paper.js has them: an object's own effects are its item's `effects`, each
//! `{ type, parameters, visible }`, and VectorCraft's geometry effects are Paper.js effect types
//! whose `apply()` draws what the app draws.

use serde_json::{Map, Value as Json, json};
use vectorcraft_doc::{Document, Effect, Node};

use crate::Error;
use crate::host::tree_json;

/// An effect Paper.js has a type of its own for: its VectorCraft id, its Paper.js type, and the
/// parameters named differently, VectorCraft's name first.
struct Name {
    id: &'static str,
    paper: &'static str,
    parameters: &'static [(&'static str, &'static str)],
}

const NAMES: &[Name] = &[
    Name { id: "path.offsetPath", paper: "offset", parameters: &[("offset", "distance"), ("joins", "join")] },
    Name { id: "path.outlineStroke", paper: "outline", parameters: &[] },
];

/// An effect as Paper.js JSON.
pub(crate) fn effect_json(effect: &Effect) -> Json {
    let parameters: Map<String, Json> =
        effect.params.as_object().into_iter().flatten().map(|(key, value)| (renamed(&effect.id, key).into(), value.clone())).collect();
    let mut json = json!({ "type": paper_type(&effect.id), "parameters": parameters });
    // Paper.js writes the visibility of hidden effects only:
    if !effect.visible {
        json["visible"] = json!(false);
    }
    json
}

/// An effect from its Paper.js JSON. None without a type.
pub(crate) fn effect(json: &Json) -> Option<Effect> {
    let kind = json.get("type")?.as_str()?;
    let name = NAMES.iter().find(|name| name.paper == kind);
    let ours = |key: &str| name.and_then(|name| name.parameters.iter().find(|(_, theirs)| *theirs == key)).map_or(key, |(ours, _)| *ours).to_string();
    let parameters = json.get("parameters").and_then(Json::as_object).into_iter().flatten();
    Some(Effect {
        id: name.map_or(kind, |name| name.id).into(),
        params: Json::Object(parameters.map(|(key, value)| (ours(key), value.clone())).collect()),
        visible: json.get("visible").and_then(Json::as_bool).unwrap_or(true),
    })
}

/// The Paper.js types of the app's geometry effects and of the installed effect plug-ins, which
/// [`apply`] draws, with their defaults.
pub(crate) fn types() -> impl Iterator<Item = (&'static str, Map<String, Json>)> {
    vectorcraft_effects::effect_catalog()
        .into_iter()
        .chain(vectorcraft_effects::plugin_effects())
        .filter(|info| vectorcraft_effects::is_geometry(info.id))
        .map(|info| (paper_type(info.id), defaults(info.id)))
}

/// The Paper.js type of the effect `id`.
fn paper_type(id: &str) -> &str {
    NAMES.iter().find(|name| name.id == id).map_or(id, |name| name.paper)
}

/// The Paper.js name of the parameter `key` of the effect `id`.
fn renamed<'a>(id: &str, key: &'a str) -> &'a str {
    let name = NAMES.iter().find(|name| name.id == id);
    name.and_then(|name| name.parameters.iter().find(|(ours, _)| *ours == key)).map_or(key, |(_, theirs)| *theirs)
}

/// The app's defaults of the effect `id`'s parameters, by their Paper.js names.
fn defaults(id: &str) -> Map<String, Json> {
    let defaults = vectorcraft_effects::effect_info(id).and_then(|info| info.defaults.as_object().cloned()).unwrap_or_default();
    defaults.into_iter().map(|(key, value)| (renamed(id, &key).into(), value)).collect()
}

/// The art drawn with the effect as the app draws it, as Paper.js JSON, in `doc`, whose symbols
/// its symbol items show. None when the effect leaves it as it is.
pub(crate) fn apply(doc: &Document, mut art: Node, effect: &Json) -> Result<Option<Json>, Error> {
    let effect = self::effect(effect).ok_or_else(|| Error::Script("an effect without a type".into()))?;
    let mut appearance = (*art.appearance).clone();
    appearance.effects = vec![effect];
    art.appearance = appearance.into();
    Ok(vectorcraft_effects::preview::drawn_art(&doc.symbol_boxes(), &art).and_then(|art| tree_json(&art)))
}
