//! `document.patch`: node-level changes in the document's own JSON, as one undo step.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use vectorcraft_doc::NodeId;
use vectorcraft_engine::Session;

fn id(v: &Value) -> u64 {
    v["id"].as_u64().unwrap()
}

/// A document with one layer holding three rectangles a, b, c (bottom first).
fn three_rectangles() -> (Session, u64, [u64; 3]) {
    let mut s = Session::new();
    s.execute("file.new", &json!({})).unwrap();
    let mut rectangle = |x| id(&s.execute("shape.rectangle", &json!({"x": x, "y": 10, "width": 30, "height": 30})).unwrap());
    let ids = [rectangle(10), rectangle(50), rectangle(90)];
    let layer = s.active().unwrap().doc.layers[0].id.0;
    (s, layer, ids)
}

fn children(s: &Session, parent: u64) -> Vec<u64> {
    s.active().unwrap().doc.node(NodeId(parent)).unwrap().children().unwrap().iter().map(|c| c.id.0).collect()
}

fn node(s: &mut Session, id: u64) -> Value {
    s.execute("document.node", &json!({"id": id})).unwrap()
}

fn document(s: &mut Session) -> Value {
    s.execute("document.json", &json!({})).unwrap()
}

#[test]
fn inserts_replaces_orders_and_removes_as_one_undo_step() {
    let (mut s, layer, [a, b, c]) = three_rectangles();
    let before = document(&mut s);

    // A copy of a, moved 100 to the right, as a new object; b's first anchor moved; c removed.
    let mut copy = node(&mut s, a);
    for anchor in copy["kind"]["path"]["subpaths"][0]["anchors"].as_array_mut().unwrap() {
        anchor["p"][0] = json!(anchor["p"][0].as_f64().unwrap() + 100.0);
    }
    let mut moved = node(&mut s, b);
    moved["kind"]["path"]["subpaths"][0]["anchors"][0]["p"] = json!([0.0, 0.0]);
    let result = s
        .execute(
            "document.patch",
            &json!({
                "label": "Script",
                "insert": [{"parent": layer, "node": copy}],
                "replace": [moved],
                "order": [{"parent": layer, "children": [{"new": 0}, b, a]}],
                "remove": [c],
            }),
        )
        .unwrap();
    let new = result["ids"][0].as_u64().unwrap();
    assert!(![a, b, c].contains(&new), "the insert gets a new id");
    assert_eq!(children(&s, layer), [new, b, a], "the new object is placed by {{new: 0}}");
    let p0 = &s.active().unwrap().doc.node(NodeId(b)).unwrap().path_data().unwrap().subpaths[0].anchors[0].p;
    assert_eq!((p0.x, p0.y), (0.0, 0.0));
    let new_x = s.active().unwrap().doc.node(NodeId(new)).unwrap().geometric_bounds().unwrap().x0;
    assert_eq!(new_x, 110.0);

    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(document(&mut s), before, "one undo step undoes the whole patch");
}

#[test]
fn a_container_sent_without_children_keeps_them() {
    let (mut s, layer, [a, b, _]) = three_rectangles();
    s.execute("select.set", &json!({"ids": [a, b]})).unwrap();
    let group = id(&s.execute("object.group", &json!({})).unwrap());
    let mut renamed = node(&mut s, group);
    renamed["name"] = json!("Pair");
    renamed["kind"].as_object_mut().unwrap().remove("children");
    s.execute("document.patch", &json!({"replace": [renamed]})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.node(NodeId(group)).unwrap().name.as_deref(), Some("Pair"));
    assert_eq!(children(&s, group), [a, b]);
    assert!(children(&s, layer).contains(&group));
}

#[test]
fn order_moves_objects_from_other_parents() {
    let (mut s, layer, [a, b, c]) = three_rectangles();
    s.execute("select.set", &json!({"ids": [a, b]})).unwrap();
    let group = id(&s.execute("object.group", &json!({})).unwrap());
    // c moves into the group, between a and b.
    s.execute("document.patch", &json!({"order": [{"parent": group, "children": [a, c, b]}]})).unwrap();
    assert_eq!(children(&s, group), [a, c, b]);
    assert_eq!(children(&s, layer), [group]);
}

#[test]
fn a_failing_patch_changes_nothing() {
    let (mut s, layer, [a, b, c]) = three_rectangles();
    let before = document(&mut s);
    let failures = [
        // b is left out of its parent's new order.
        json!({"remove": [c], "order": [{"parent": layer, "children": [a]}]}),
        // A path can't go in the layer list.
        json!({"insert": [{"parent": null, "node": node(&mut s, a)}]}),
        // No such object.
        json!({"remove": [a, 999_999]}),
        json!({"replace": [{"id": 999_999, "kind": {"type": "group", "children": []}}]}),
        // {new: k} past the inserts.
        json!({"order": [{"parent": layer, "children": [a, b, c, {"new": 0}]}]}),
        // Not an object.
        json!({"insert": [{"parent": layer, "node": {"kind": {"type": "nonsense"}}}]}),
    ];
    for params in failures {
        assert!(s.execute("document.patch", &params).is_err(), "{params}");
        assert_eq!(document(&mut s), before, "{params}");
    }
}

#[test]
fn extend_adds_to_the_newest_undo_step() {
    let (mut s, _, [a, b, _]) = three_rectangles();
    let before = document(&mut s);
    let steps = s.active().unwrap().history.undo.len();
    s.execute("document.patch", &json!({"label": "Script", "remove": [a]})).unwrap();
    s.execute("document.patch", &json!({"label": "Script", "remove": [b], "extend": true})).unwrap();
    assert_eq!(s.active().unwrap().history.undo.len(), steps + 1, "the second patch extends the first one's step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(document(&mut s), before, "one undo undoes both");
}

/// A patch during an interaction (a script's animation frame while the user drags) applies
/// beneath it: the interaction's next preview keeps it, and what the interaction changed wins.
#[test]
fn a_patch_during_an_interaction_applies_beneath_it() {
    let (mut s, layer, [a, b, c]) = three_rectangles();
    let x0 = |s: &Session, id: u64| s.active().unwrap().doc.node(NodeId(id)).unwrap().geometric_bounds().unwrap().x0;
    // The user drags a 5 pt to the right.
    s.begin_interaction("Move").unwrap();
    s.preview("object.move", &json!({"ids": [a], "dx": 5, "dy": 0})).unwrap();
    // A script moves a and b 100 pt right, and adds an object on top.
    let moved = |s: &mut Session, id: u64| {
        let mut n = node(s, id);
        for anchor in n["kind"]["path"]["subpaths"][0]["anchors"].as_array_mut().unwrap() {
            anchor["p"][0] = json!(anchor["p"][0].as_f64().unwrap() + 100.0);
        }
        n
    };
    let (moved_a, moved_b, copy) = (moved(&mut s, a), moved(&mut s, b), node(&mut s, b));
    let result = s
        .execute(
            "document.patch",
            &json!({"replace": [moved_a, moved_b], "insert": [{"parent": layer, "node": copy}], "order": [{"parent": layer, "children": [a, b, c, {"new": 0}]}]}),
        )
        .unwrap();
    let new = result["ids"][0].as_u64().unwrap();
    assert_eq!((x0(&s, a), x0(&s, b)), (15.0, 150.0), "the drag wins on a; the script moves b at once");
    // The drag goes on: its preview starts again from the snapshot, which has the script's changes.
    s.preview("object.move", &json!({"ids": [a], "dx": 20, "dy": 0})).unwrap();
    assert_eq!((x0(&s, a), x0(&s, b)), (30.0, 150.0));
    assert!(children(&s, layer).contains(&new), "the new object stays too");
    s.commit_interaction().unwrap();
    assert_eq!((x0(&s, a), x0(&s, b)), (30.0, 150.0));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((x0(&s, a), x0(&s, b)), (10.0, 150.0), "undoing the drag keeps the script's changes");
}

/// A caller whose copy of the document is older says what changed since (`keep`): its patch
/// leaves those objects out, so the newer change stays.
#[test]
fn keep_leaves_out_what_changed_since_the_callers_copy() {
    let (mut s, layer, [a, b, _]) = three_rectangles();
    let copy = s.active().unwrap().doc.clone();
    s.execute("object.move", &json!({"ids": [a], "dx": 100, "dy": 0})).unwrap();
    let keep = vectorcraft_engine::cmd::patch::changes_between(&copy, &s.active().unwrap().doc);
    assert_eq!(keep, json!({"objects": [a], "lists": []}));
    let mut old_a = serde_json::to_value(copy.node(NodeId(a)).unwrap()).unwrap();
    old_a["name"] = json!("script");
    let mut old_b = serde_json::to_value(copy.node(NodeId(b)).unwrap()).unwrap();
    old_b["name"] = json!("script");
    let result = s.execute("document.patch", &json!({"replace": [old_a, old_b], "keep": keep})).unwrap();
    assert_eq!(result["skipped"], json!([a]));
    assert_eq!(node(&mut s, b)["name"], "script", "the rest applies");
    assert_ne!(node(&mut s, a)["name"], "script", "the newer move stays");
    assert_eq!(children(&s, layer).len(), 3);
}
