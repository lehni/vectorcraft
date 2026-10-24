use serde_json::{Value, json};
use vectorcraft_doc::{Appearance, Effect, Node, NodeId};
use vectorcraft_geom::{PathData, Rect, Shape, shapes};
use vectorcraft_plugins::Manifest;

use super::*;

fn manifest(kind: &str) -> Manifest {
    let json = json!({
        "id": kind, "name": "Wobble", "kind": "effect",
        "params": {"amount": {"type": "number", "min": 0, "max": 100, "default": 5, "label": "Amount"}}
    });
    Manifest::parse(json.to_string().as_bytes()).unwrap()
}

fn object(effects: Vec<Effect>) -> Node {
    let appearance = Appearance { effects, ..Appearance::default() };
    Node::path(NodeId(7), shapes::rectangle(Rect::new(0.0, 0.0, 100.0, 50.0)), appearance)
}

fn effect(params: Value) -> Effect {
    Effect { id: "script.wobble".into(), params, visible: true }
}

/// The object's effect at `index` with `output` kept for the input it has now.
fn kept(mut n: Node, index: usize, output: &PathData) -> Node {
    let input = script::inputs(&n).into_iter().find(|i| i.index == index).unwrap();
    let mut appearance = (*n.appearance).clone();
    let params = &mut appearance.effects[index].params;
    params[script::INPUT] = json!(input.fingerprint);
    params[script::OUTPUT] = serde_json::to_value(output).unwrap();
    n.appearance = appearance.into();
    n
}

fn drawn(n: &Node) -> PathData {
    let path = n.path_data().unwrap().clone();
    apply_geometry_with(&n.appearance.effects, &path, path.bounds().unwrap(), &GeomContext::of(n))
}

#[test]
fn a_script_effect_draws_what_its_script_kept_for_the_same_input() {
    let output = shapes::ellipse(Rect::new(0.0, 0.0, 30.0, 30.0));
    let n = kept(object(vec![effect(json!({"amount": 5}))]), 0, &output);
    assert_eq!(drawn(&n), output);
    // The renderer's route, through kurbo paths:
    let path = n.path_data().unwrap().to_bezpath();
    assert_eq!(apply_geometry_bez(&n.appearance.effects, &path, path.bounding_box(), &GeomContext::of(&n)), output.to_bezpath());
}

#[test]
fn a_script_effect_draws_the_geometry_once_its_input_changed() {
    let output = shapes::ellipse(Rect::new(0.0, 0.0, 30.0, 30.0));
    let n = kept(object(vec![effect(json!({"amount": 5}))]), 0, &output);
    let geometry = shapes::rectangle(Rect::new(10.0, 0.0, 110.0, 50.0));
    let mut moved = n.clone();
    moved.kind = Node::path(n.id, geometry.clone(), Appearance::default()).kind;
    assert_eq!(drawn(&moved), geometry);
    let mut changed = n.clone();
    let mut appearance = (*n.appearance).clone();
    appearance.effects[0].params["amount"] = json!(6);
    changed.appearance = appearance.into();
    assert_eq!(&drawn(&changed), n.path_data().unwrap());
    // Nor does one its script never drew:
    let fresh = object(vec![effect(json!({"amount": 5}))]);
    assert_eq!(&drawn(&fresh), fresh.path_data().unwrap());
}

#[test]
fn a_script_effects_input_is_the_geometry_after_the_effects_before_it() {
    let offset = Effect { id: "path.offsetPath".into(), params: json!({"offset": 10}), visible: true };
    let n = object(vec![offset.clone(), effect(json!({}))]);
    let inputs = script::inputs(&n);
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].index, 1);
    let path = n.path_data().unwrap();
    assert_eq!(inputs[0].path, apply_geometry(&[offset], path, path.bounds().unwrap()));
    // What it keeps for that input, the app draws after the offset:
    let output = shapes::ellipse(Rect::new(0.0, 0.0, 30.0, 30.0));
    assert_eq!(drawn(&kept(n, 1, &output)), output);
    // A hidden one has none:
    let mut hidden = effect(json!({}));
    hidden.visible = false;
    assert!(script::inputs(&object(vec![hidden])).is_empty());
}

#[test]
fn script_effects_are_listed_while_their_script_runs() {
    let before = script::revision();
    script::register(manifest("listed"));
    assert_ne!(script::revision(), before);
    let info = effect_info("script.listed").unwrap();
    assert_eq!((info.label, info.menu), ("Wobble…", &["Effect", "Scriptographer"][..]));
    assert!(script_effects().iter().any(|e| e.id == "script.listed"));
    assert_eq!(new_effect("script.listed", &json!({})).unwrap().params, json!({"amount": 5.0}));
    assert_eq!(effect_manifest("script.listed").unwrap().name, "Wobble");
    script::unregister("listed");
    assert!(effect_info("script.listed").is_none());
    assert!(new_effect("script.listed", &json!({})).is_none());
    // Its effects stay geometry effects, which draw what they kept:
    assert!(is_geometry("script.listed"));
}
