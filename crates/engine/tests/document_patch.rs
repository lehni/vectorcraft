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
    assert_eq!(keep, json!({"objects": [a], "lists": [], "patterns": [], "symbols": []}));
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

/// A new group gets its new and its existing children in one patch: an insert or an order list
/// names it as `{new: k}`. Existing objects keep their ids.
#[test]
fn new_parents_take_new_and_existing_children() {
    let (mut s, layer, [a, b, c]) = three_rectangles();
    let mut group = node(&mut s, layer);
    group["kind"] = json!({"type": "group", "children": []});
    group["name"] = json!("Group");
    let copy = node(&mut s, a);
    let result = s
        .execute(
            "document.patch",
            &json!({
                "insert": [{"parent": layer, "node": group}, {"parent": {"new": 0}, "node": copy}],
                "order": [
                    {"parent": layer, "children": [a, {"new": 0}]},
                    {"parent": {"new": 0}, "children": [b, {"new": 1}, c]},
                ],
            }),
        )
        .unwrap();
    let ids: Vec<u64> = result["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(children(&s, layer), [a, ids[0]]);
    assert_eq!(children(&s, ids[0]), [b, ids[1], c], "b and c moved in, keeping their ids");
    let again = node(&mut s, a);
    assert!(s.execute("document.patch", &json!({"insert": [{"parent": {"new": 0}, "node": again}]})).is_err(), "only an earlier insert");
}

/// Area type frames `a` (with more text than it holds), `b` and `c` (empty), side by side, and the
/// layer they're on.
fn three_frames() -> (Session, u64, [u64; 3]) {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 800, "height": 400})).unwrap();
    let text = "Text in an area breaks into lines that fit it. ".repeat(12);
    let mut frame = |x, text: &str| {
        id(&s.execute("text.create", &json!({"x": x, "y": 10, "text": text, "size": 12, "area": {"width": 100, "height": 120}})).unwrap())
    };
    let ids = [frame(10, &text), frame(150, ""), frame(290, "")];
    let layer = s.active().unwrap().doc.layers[0].id.0;
    (s, layer, ids)
}

fn threads(s: &Session) -> Vec<Vec<u64>> {
    s.active().unwrap().doc.text_threads.iter().map(|t| t.iter().map(|id| id.0).collect()).collect()
}

fn text(s: &Session, id: u64) -> String {
    match &s.active().unwrap().doc.node(NodeId(id)).unwrap().kind {
        vectorcraft_doc::NodeKind::Text(t) => t.plain_text(),
        _ => panic!("{id} isn't text"),
    }
}

/// `threads` makes each list of frames one thread, which the threads they were in lose; a list of
/// one frame takes it out of its thread. The stories flow across the frames again, in one undo step.
#[test]
fn threads_link_frames_and_take_them_out() {
    let (mut s, _, [a, b, c]) = three_frames();
    let story = text(&s, a);
    s.execute("document.patch", &json!({"threads": [[a, b]]})).unwrap();
    assert_eq!(threads(&s), [[a, b]]);
    assert!(!text(&s, b).is_empty(), "the story flows on into b");
    assert_eq!(text(&s, a) + &text(&s, b), story);

    // c takes a's place in front of b: a's thread loses it.
    s.execute("document.patch", &json!({"threads": [[c, b]]})).unwrap();
    assert_eq!(threads(&s), [[c, b]]);

    s.execute("document.patch", &json!({"threads": [[b]]})).unwrap();
    assert!(threads(&s).is_empty(), "a thread of one frame is none");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(threads(&s), [[c, b]], "one undo step");
}

/// A frame inserted by the same patch can be threaded, as `{new: k}`.
#[test]
fn threads_take_new_frames() {
    let (mut s, layer, [a, b, _]) = three_frames();
    let copy = node(&mut s, b);
    let result = s.execute("document.patch", &json!({"insert": [{"parent": layer, "node": copy}], "threads": [[a, {"new": 0}]]})).unwrap();
    let new = result["ids"][0].as_u64().unwrap();
    assert_eq!(threads(&s), [[a, new]]);
    assert!(!text(&s, new).is_empty(), "the story flows into the new frame");
}

/// Only area type threads, and a frame is in one thread: a patch that says otherwise changes
/// nothing.
#[test]
fn threads_take_area_type_once() {
    let (mut s, _, [a, b, c]) = three_frames();
    let point = id(&s.execute("text.create", &json!({"x": 10, "y": 300, "text": "Point"})).unwrap());
    let before = document(&mut s);
    assert!(s.execute("document.patch", &json!({"threads": [[a, point]]})).is_err());
    assert!(s.execute("document.patch", &json!({"threads": [[a, b], [b, c]]})).is_err());
    assert_eq!(document(&mut s), before);
}

/// Rectangle a as a pattern "Tiles" 40 × 20, of grid tiles with Size Tile to Art off.
fn tiles() -> (Session, [u64; 3]) {
    let (mut s, _, ids) = three_rectangles();
    s.execute("object.pattern.make", &json!({"ids": [ids[0]], "name": "Tiles", "width": 40, "height": 20, "edit": false})).unwrap();
    (s, ids)
}

fn tile(s: &Session, name: &str) -> [f64; 4] {
    let tile = s.active().unwrap().doc.pattern(name).unwrap().tile;
    [tile.x0, tile.y0, tile.width(), tile.height()]
}

#[test]
fn patterns_are_made_with_their_swatch() {
    let (mut s, _, [a, ..]) = three_rectangles();
    let art = node(&mut s, a);
    s.execute("document.patch", &json!({"patterns": [{"name": "Dots", "art": [art]}]})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.pattern("Dots").unwrap().art.len(), 1);
    assert_ne!(doc.pattern("Dots").unwrap().art[0].id.0, a, "the art gets new ids");
    assert!(doc.swatch_name_taken("Dots"), "as Make does, it gets a swatch");
    assert_eq!(tile(&s, "Dots"), [10.0, 10.0, 30.0, 30.0], "without a tile, the art's bounds");
}

#[test]
fn patterns_keep_what_isnt_given() {
    let (mut s, [_, b, _]) = tiles();
    s.execute("pattern.options", &json!({"name": "Tiles", "tileType": "brickByRow", "hSpacing": 4})).unwrap();
    let before = document(&mut s);
    let art = node(&mut s, b);
    s.execute("document.patch", &json!({"patterns": [{"name": "Tiles", "art": [art]}]})).unwrap();
    let def = s.active().unwrap().doc.pattern("Tiles").unwrap().clone();
    assert_eq!(def.art[0].geometric_bounds().unwrap().x0, 50.0, "the art is b's");
    assert_eq!((def.tile_type.id(), def.h_spacing), ("brickByRow", 4.0));
    s.execute("document.patch", &json!({"patterns": [{"name": "Tiles", "tile": [0, 0, 60, 30]}]})).unwrap();
    assert_eq!(tile(&s, "Tiles"), [0.0, 0.0, 60.0, 30.0]);
    assert_eq!(s.active().unwrap().doc.pattern("Tiles").unwrap().art, def.art, "the art stays");
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(document(&mut s), before);
}

/// A tile given is the caller's own, so Size Tile to Art turns off; art alone refits the tile.
#[test]
fn patterns_sized_to_their_art_follow_it() {
    let (mut s, [_, b, c]) = tiles();
    s.execute("pattern.options", &json!({"name": "Tiles", "sizeTileToArt": true})).unwrap();
    let art = node(&mut s, b);
    s.execute("document.patch", &json!({"patterns": [{"name": "Tiles", "art": [art]}]})).unwrap();
    assert_eq!(tile(&s, "Tiles"), [50.0, 10.0, 30.0, 30.0], "fitted to the new art");
    let art = node(&mut s, c);
    s.execute("document.patch", &json!({"patterns": [{"name": "Tiles", "tile": [0, 0, 60, 30], "art": [art]}]})).unwrap();
    assert_eq!(tile(&s, "Tiles"), [0.0, 0.0, 60.0, 30.0]);
    assert!(!s.active().unwrap().doc.pattern("Tiles").unwrap().size_tile_to_art);
}

#[test]
fn failing_definitions_change_nothing() {
    let (mut s, [a, ..]) = tiles();
    s.execute("swatch.new", &json!({"name": "Red", "color": "#ff0000"})).unwrap();
    let art = node(&mut s, a);
    let before = document(&mut s);
    let failures = [
        // A new pattern needs its art.
        json!({"patterns": [{"name": "Dots", "tile": [0, 0, 10, 10]}]}),
        // Its swatch's name is taken.
        json!({"patterns": [{"name": "Red", "art": [art]}]}),
        json!({"patterns": [{"name": "Tiles", "tile": [0, 0, 0, 10]}]}),
        json!({"patterns": [{"tile": [0, 0, 10, 10]}]}),
        json!({"symbols": [{"name": "Box"}]}),
        json!({"symbols": [{"name": " ", "art": art}]}),
    ];
    for params in failures {
        assert!(s.execute("document.patch", &params).is_err(), "{params}");
        assert_eq!(document(&mut s), before, "{params}");
    }
}

#[test]
fn a_pattern_being_edited_is_refused() {
    let (mut s, [_, b, _]) = tiles();
    s.execute("object.pattern.edit", &json!({"name": "Tiles"})).unwrap();
    let art = node(&mut s, b);
    let error = s.execute("document.patch", &json!({"patterns": [{"name": "Tiles", "art": [art]}]})).unwrap_err();
    assert!(error.to_string().contains("being edited"), "{error}");
}

/// Unlike Redefine Symbol, the art goes in as it is, in the symbol's space, and the instances
/// keep their transforms.
#[test]
fn symbols_take_art_as_it_is() {
    let (mut s, _, [a, b, _]) = three_rectangles();
    s.execute("symbol.new", &json!({"name": "Box", "ids": [a]})).unwrap();
    let instance = |s: &Session| {
        let mut found = None;
        s.active().unwrap().doc.walk(|n| {
            if let vectorcraft_doc::NodeKind::SymbolInstance { xf, .. } = &n.kind {
                found = Some(*xf);
            }
        });
        found.unwrap()
    };
    let xf = instance(&s);
    let art = node(&mut s, b);
    s.execute("document.patch", &json!({"symbols": [{"name": "Box", "art": art}, {"name": "Fresh", "art": art}]})).unwrap();
    let doc = &s.active().unwrap().doc;
    for name in ["Box", "Fresh"] {
        let bounds = doc.symbol(name).unwrap().art.geometric_bounds().unwrap();
        assert_eq!([bounds.x0, bounds.y0, bounds.width(), bounds.height()], [50.0, 10.0, 30.0, 30.0], "{name}");
    }
    assert_eq!(instance(&s), xf);
    let list = s.execute("symbol.list", &json!({})).unwrap();
    assert_eq!(list["symbols"][1]["size"], json!([20.0, 20.0]), "a new symbol's natural size is unset, so its art places at 100%");
}

/// `keep` names the patterns and symbols that changed since the caller's copy, and the patch
/// leaves them out.
#[test]
fn keep_leaves_out_definitions_changed_since_the_callers_copy() {
    let (mut s, [a, b, _]) = tiles();
    s.execute("symbol.new", &json!({"name": "Box", "ids": [a]})).unwrap();
    let copy = s.active().unwrap().doc.clone();
    s.execute("pattern.options", &json!({"name": "Tiles", "width": 80})).unwrap();
    let keep = vectorcraft_engine::cmd::patch::changes_between(&copy, &s.active().unwrap().doc);
    assert_eq!((&keep["patterns"], &keep["symbols"]), (&json!(["Tiles"]), &json!([])));
    let art = node(&mut s, b);
    let result = s
        .execute(
            "document.patch",
            &json!({"patterns": [{"name": "Tiles", "tile": [0, 0, 10, 10]}], "symbols": [{"name": "Box", "art": art}], "keep": keep}),
        )
        .unwrap();
    assert_eq!(result["skippedPatterns"], json!(["Tiles"]));
    assert!(result.get("skippedSymbols").is_none());
    assert_eq!(tile(&s, "Tiles")[2], 80.0, "the newer change stays");
    assert_eq!(s.active().unwrap().doc.symbol("Box").unwrap().art.geometric_bounds().unwrap().x0, 50.0, "the rest applies");
}
