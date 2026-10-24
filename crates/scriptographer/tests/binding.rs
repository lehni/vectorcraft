#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Scripts on a real document: Paper.js reads it through its host interface, and what a script
//! changes goes back through `document.patch` as one undo step.

use std::sync::Arc;

use serde_json::json;
use vectorcraft_color::{Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::{Node, NodeId, NodeKind};
use vectorcraft_engine::Session;
use vectorcraft_geom::{Affine, Point, Rect};
use vectorcraft_scriptographer::{Engine, Runtime};

/// A session with one layer holding two rectangles, a (bottom) and b.
fn document() -> (Session, NodeId, NodeId) {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 400, "height": 300})).unwrap();
    let mut rectangle =
        |x| NodeId(s.execute("shape.rectangle", &json!({"x": x, "y": 10, "width": 30, "height": 20})).unwrap()["id"].as_u64().unwrap());
    let (a, b) = (rectangle(10), rectangle(50));
    (s, a, b)
}

/// Runs `script` on the session's document and applies what it changed, as the app does.
fn run<E: Engine>(s: &mut Session, script: &str) -> Vec<String> {
    let st = s.active().unwrap();
    let (doc, selection) = (st.doc.clone(), st.selection.clone());
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, doc, selection).unwrap();
    runtime.execute(&scope, script).unwrap();
    if let Some(patch) = runtime.take_patch(&scope).unwrap() {
        s.execute("document.patch", &patch.params).unwrap();
    }
    runtime.take_output()
}

fn node(s: &Session, id: NodeId) -> Node {
    s.active().unwrap().doc.node(id).unwrap().clone()
}

fn reads_the_document<E: Engine>() {
    let (mut s, a, b) = document();
    let output = run::<E>(
        &mut s,
        "var layer = project.activeLayer\n\
         console.log(layer.children.length, layer.firstChild.key, layer.lastChild.key)\n\
         console.log(layer.firstChild.className, layer.firstChild.segments.length, layer.firstChild.bounds)",
    );
    assert_eq!(output, [format!("2 {} {}", a.0, b.0), "Path 4 { x: 10, y: 10, width: 30, height: 20 }".to_string()]);
}

fn writes_changes_back_as_one_undo_step<E: Engine>() {
    let (mut s, a, b) = document();
    let before = s.execute("document.json", &json!({})).unwrap();
    run::<E>(
        &mut s,
        "var layer = project.activeLayer\n\
         layer.firstChild.fillColor = 'red'\n\
         layer.lastChild.position += [100, 0]\n\
         new Path.Circle({ center: [200, 150], radius: 10, fillColor: 'blue' })",
    );
    let fill = |n: &Node| n.appearance.fill_paint();
    assert_eq!(fill(&node(&s, a)), Paint::solid(Color::rgb(1.0, 0.0, 0.0)));
    assert_eq!(node(&s, b).geometric_bounds().unwrap().x0, 150.0);
    assert!(node(&s, b).live_shape().is_none(), "a moved live rectangle is a plain path, as after any path edit");
    let children = s.active().unwrap().doc.layers[0].children().unwrap().clone();
    assert_eq!(children.len(), 3, "the circle is added on top");
    let circle = children.last().unwrap();
    assert_eq!(fill(circle), Paint::solid(Color::rgb(0.0, 0.0, 1.0)));
    let bounds = circle.geometric_bounds().unwrap();
    assert!((bounds.x0 - 190.0).abs() < 1e-9 && (bounds.y1 - 160.0).abs() < 1e-9, "{bounds:?}");
    // What Paper.js can't show stays: the rectangle a keeps its stroke.
    assert_eq!(node(&s, a).appearance.stroke_paint(), Paint::solid(Color::BLACK));

    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.execute("document.json", &json!({})).unwrap(), before, "one undo step undoes the script");
}

/// `paintOrder` is the order of the top fill and stroke, both ways.
fn paint_order_is_the_order_of_fill_and_stroke<E: Engine>() {
    let (mut s, a, _) = document();
    run::<E>(
        &mut s,
        "project.activeLayer.firstChild.paintOrder = 'stroke'\n\
         new Path.Circle({ center: [200, 150], radius: 10, fillColor: 'red', strokeColor: 'black', paintOrder: 'stroke' })",
    );
    let circle = s.active().unwrap().doc.layers[0].children().unwrap().last().unwrap().clone();
    assert!(node(&s, a).appearance.is_stroke_first() && circle.appearance.is_stroke_first());
    let output = run::<E>(&mut s, "var a = project.activeLayer.firstChild\nconsole.log(a.paintOrder)\na.paintOrder = 'fill'");
    assert_eq!(output, ["stroke"]);
    assert!(!node(&s, a).appearance.is_stroke_first());
    assert!(matches!(node(&s, a).appearance.items.last(), Some(vectorcraft_doc::AppearanceItem::Stroke(_))), "the stroke is back on top");
}

fn removes_and_reorders<E: Engine>() {
    let (mut s, a, b) = document();
    run::<E>(&mut s, "var layer = project.activeLayer\nlayer.firstChild.bringToFront()");
    let ids: Vec<_> = s.active().unwrap().doc.layers[0].children().unwrap().iter().map(|c| c.id).collect();
    assert_eq!(ids, [b, a]);
    run::<E>(&mut s, "project.activeLayer.lastChild.remove()");
    let ids: Vec<_> = s.active().unwrap().doc.layers[0].children().unwrap().iter().map(|c| c.id).collect();
    assert_eq!(ids, [b]);
}

fn untouched_scripts_change_nothing<E: Engine>() {
    let (mut s, _, _) = document();
    let before = s.execute("document.json", &json!({})).unwrap();
    let steps = s.active().unwrap().history.undo.len();
    run::<E>(&mut s, "console.log(project.activeLayer.bounds)");
    assert_eq!(s.execute("document.json", &json!({})).unwrap(), before);
    assert_eq!(s.active().unwrap().history.undo.len(), steps, "reading makes no undo step");
}

fn on_frame_keeps_changing_the_document<E: Engine>() {
    let (mut s, a, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "var item = project.activeLayer.firstChild\nfunction onFrame(event) { item.position += [10, 0] }").unwrap();
    assert!(runtime.is_running(), "onFrame keeps the script running");
    for now in [20.0, 40.0, 60.0] {
        frame(&mut s, &mut runtime, &scope, now);
    }
    assert_eq!(node(&s, a).geometric_bounds().unwrap().x0, 40.0, "three frames moved it by 30");
}

fn scripts_without_on_frame_stop<E: Engine>() {
    let (s, _, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "project.activeLayer.firstChild.position += [10, 0]").unwrap();
    runtime.run_timers(20.0).unwrap();
    assert!(!runtime.is_running(), "once the view has updated, nothing is left to run");
}

/// One frame as the app runs it: the project catches up with the document, the timers run, and
/// what they changed goes back.
fn frame<E: Engine>(s: &mut Session, runtime: &mut Runtime<E>, scope: &vectorcraft_scriptographer::Scope<E::Handle>, now: f64) {
    let st = s.active().unwrap();
    runtime.update(scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.run_timers(now).unwrap();
    if let Some(patch) = runtime.take_patch(scope).unwrap() {
        let result = s.execute("document.patch", &patch.params).unwrap();
        let st = s.active().unwrap();
        runtime.apply_ids(scope, patch, &result, st.doc.clone(), st.selection.clone(), false).unwrap();
    }
}

fn edits_made_while_a_script_runs_stay<E: Engine>() {
    let (mut s, a, b) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime
        .execute(&scope, "var layer = project.activeLayer, item = layer.firstChild\nfunction onFrame(event) { item.position += [10, 0]; console.log(layer.children.length) }")
        .unwrap();
    frame(&mut s, &mut runtime, &scope, 20.0);
    // Edits outside the script, between frames: a's fill and size, and b removed.
    s.execute("paint.setFill", &json!({"ids": [a.0], "color": "#00ff00"})).unwrap();
    s.execute("object.transform.scale", &json!({"ids": [a.0], "uniform": 200})).unwrap_or_else(|_| {
        s.execute("path.setAnchors", &json!({"id": a.0, "subpaths": [{"anchors": [{"x": 20, "y": 10}, {"x": 80, "y": 10}, {"x": 80, "y": 50}, {"x": 20, "y": 50}], "closed": true}]})).unwrap()
    });
    s.execute("edit.clear", &json!({"ids": [b.0]})).unwrap();
    let width = node(&s, a).geometric_bounds().unwrap().width();
    let x = node(&s, a).geometric_bounds().unwrap().x0;
    frame(&mut s, &mut runtime, &scope, 40.0);
    let a_now = node(&s, a);
    assert_eq!(a_now.appearance.fill_paint(), Paint::solid(Color::rgb(0.0, 1.0, 0.0)), "the new fill stays");
    let bounds = a_now.geometric_bounds().unwrap();
    assert!((bounds.width() - width).abs() < 1e-9, "the new size stays: {bounds:?}");
    assert!((bounds.x0 - (x + 10.0)).abs() < 1e-9, "and the script still moves it: {bounds:?}");
    assert_eq!(runtime.take_output().last().map(String::as_str), Some("1"), "the script sees b removed");
}

/// A patch applied after the user edited the document meanwhile (the app went on while the script
/// worked): the user's edit stays, and the script sees it.
fn a_late_patch_leaves_what_changed_meanwhile<E: Engine>() {
    let (mut s, a, b) = document();
    let st = s.active().unwrap();
    let base = st.doc.clone();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, base.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "var a = project.activeLayer.firstChild\nconsole.log(a.nextSibling.segments[0].point.x)\na.position += [10, 0]").unwrap();
    assert_eq!(runtime.take_output(), ["50"], "the script read b");
    let patch = runtime.take_patch(&scope).unwrap().unwrap();
    // Meanwhile, the user moves b.
    s.execute("object.move", &json!({"ids": [b.0], "dx": 100, "dy": 0})).unwrap();
    let mut params = patch.params.clone();
    params["keep"] = vectorcraft_engine::cmd::patch::changes_between(&base, &s.active().unwrap().doc);
    let result = s.execute("document.patch", &params).unwrap();
    let st = s.active().unwrap();
    runtime.apply_ids(&scope, patch, &result, st.doc.clone(), st.selection.clone(), true).unwrap();
    assert_eq!(node(&s, a).geometric_bounds().unwrap().x0, 20.0, "the script's move of a applies");
    assert_eq!(node(&s, b).geometric_bounds().unwrap().x0, 150.0, "the user's move of b stays");
    runtime.execute(&scope, "console.log(project.activeLayer.lastChild.segments[0].point.x)").unwrap();
    assert_eq!(runtime.take_output(), ["150"], "and the script sees it");
}

fn selection_goes_both_ways<E: Engine>() {
    let (mut s, a, b) = document();
    s.execute("select.set", &json!({"ids": [a.0]})).unwrap();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "console.log(project.selectedItems.length, project.selectedItems[0].key)").unwrap();
    assert_eq!(runtime.take_output(), [format!("1 {}", a.0)]);
    runtime.execute(&scope, "var layer = project.activeLayer\nlayer.firstChild.selected = false\nlayer.lastChild.selected = true").unwrap();
    assert_eq!(runtime.selected_ids(&scope).unwrap().into_iter().collect::<Vec<_>>(), [b]);
}

fn artboards_and_the_active_one<E: Engine>() {
    let (mut s, _, _) = document();
    s.execute("artboard.new", &json!({"x": 500, "y": 0, "width": 200, "height": 100})).unwrap();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.set_artboard(1);
    runtime
        .execute(
            &scope,
            "project.artboards.forEach(function(a) { console.log(a.name, a.bounds) })\n\
             console.log(project.activeArtboard.name, project.activeArtboard.bounds.width)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        ["Artboard 1 { x: 0, y: 0, width: 400, height: 300 }", "Artboard 2 { x: 500, y: 0, width: 200, height: 100 }", "Artboard 2 200"]
    );
}

fn tool_scripts_get_pointer_events<E: Engine>() {
    use vectorcraft_scriptographer::{Modifiers, MouseEvent};
    let (s, _, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime
        .execute(
            &scope,
            "tool.minDistance = 10\n\
             function onMouseDown(event) { console.log('down', event.point.x, event.modifiers.shift) }\n\
             function onMouseDrag(event) { console.log('drag', Math.round(event.velocity.x)); new Path.Circle({ center: event.point, radius: 2 }) }\n\
             function onMouseUp(event) { console.log('up', event.point.x, event.time, Math.round(event.elapsed * 1000)) }",
        )
        .unwrap();
    assert!(runtime.has_tool(&scope).unwrap());
    assert!(!runtime.has_key_handlers(&scope).unwrap(), "a tool for the mouse only");
    let shift = Modifiers { shift: true, ..Default::default() };
    runtime.mouse_event(&scope, MouseEvent::Down, (0.0, 0.0), shift, 0.0).unwrap();
    // Closer than minDistance: no drag event; the next one's velocity spans both moves.
    runtime.mouse_event(&scope, MouseEvent::Move, (5.0, 0.0), Modifiers::default(), 0.01).unwrap();
    runtime.mouse_event(&scope, MouseEvent::Move, (20.0, 0.0), Modifiers::default(), 0.02).unwrap();
    runtime.mouse_event(&scope, MouseEvent::Up, (20.0, 0.0), Modifiers::default(), 0.03).unwrap();
    // A move after the button went up is no drag.
    runtime.mouse_event(&scope, MouseEvent::Move, (40.0, 0.0), Modifiers::default(), 0.04).unwrap();
    // The document is larger than the view: a press outside it is a press too, also after a move
    // there.
    runtime.mouse_event(&scope, MouseEvent::Move, (-60.0, 500.0), Modifiers::default(), 0.06).unwrap();
    runtime.mouse_event(&scope, MouseEvent::Down, (-50.0, 500.0), Modifiers::default(), 0.07).unwrap();
    runtime.mouse_event(&scope, MouseEvent::Up, (-50.0, 500.0), Modifiers::default(), 0.08).unwrap();
    assert_eq!(runtime.take_output(), ["down 0 true", "drag 1000", "up 20 0.03 10", "down -50 false", "up -50 0.08 10"]);
    let patch = runtime.take_patch(&scope).unwrap().unwrap();
    assert_eq!(patch.params["insert"].as_array().unwrap().len(), 1, "one drag event drew one circle");
}

fn tool_scripts_get_key_events<E: Engine>() {
    use vectorcraft_scriptographer::Modifiers;
    let (s, _, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime
        .execute(
            &scope,
            "function onKeyDown(event) { console.log('down', event.key, event.character, event.modifiers.shift) }\n\
             function onKeyUp(event) { console.log('up', event.key) }",
        )
        .unwrap();
    assert!(runtime.has_key_handlers(&scope).unwrap());
    let shift = Modifiers { shift: true, ..Default::default() };
    runtime.key_event(&scope, true, "a", Some("A"), shift).unwrap();
    runtime.key_event(&scope, false, "a", None, shift).unwrap();
    runtime.key_event(&scope, true, "left", None, Modifiers::default()).unwrap();
    assert_eq!(runtime.take_output(), ["down a A true", "up a", "down left  false"]);
}

fn scripts_without_handlers_have_no_tool<E: Engine>() {
    let (s, _, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "console.log(project.activeLayer.children.length)").unwrap();
    assert!(!runtime.has_tool(&scope).unwrap());
}

/// A path the script made and the document then gave an id is reloaded like any other when the
/// user changes it (`invalidate()` on an item that was never a shell).
fn edits_to_objects_a_script_made_reach_it<E: Engine>() {
    let (mut s, _, _) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime
        .execute(&scope, "var circle = new Path.Circle({ center: [200, 150], radius: 10 })\nfunction onFrame() { circle.position += [1, 0]; console.log(circle.bounds.width) }")
        .unwrap();
    // The circle goes to the document and gets its id.
    let patch = runtime.take_patch(&scope).unwrap().unwrap();
    let result = s.execute("document.patch", &patch.params).unwrap();
    let circle = result["ids"][0].as_u64().unwrap();
    let st = s.active().unwrap();
    runtime.apply_ids(&scope, patch, &result, st.doc.clone(), st.selection.clone(), false).unwrap();
    // The user makes it twice as wide.
    s.execute("path.setAnchors", &json!({"id": circle, "subpaths": [{"anchors": [{"x": 180, "y": 150}, {"x": 200, "y": 140}, {"x": 220, "y": 150}, {"x": 200, "y": 160}], "closed": true}]})).unwrap();
    frame(&mut s, &mut runtime, &scope, 20.0);
    assert_eq!(runtime.take_output().last().map(String::as_str), Some("40"), "the script sees the user's change");
}

/// An item already in the document that the script moves into a new group goes there, and the
/// items in the new group are the document's objects from then on: later changes reach them.
fn items_moved_into_a_new_group<E: Engine>() {
    let (mut s, a, b) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    let write_back = |s: &mut Session, runtime: &mut Runtime<E>| {
        let patch = runtime.take_patch(&scope).unwrap().unwrap();
        let result = s.execute("document.patch", &patch.params).unwrap();
        let st = s.active().unwrap();
        runtime.apply_ids(&scope, patch, &result, st.doc.clone(), st.selection.clone(), false).unwrap();
    };
    runtime.execute(&scope, "new Path.Circle({ center: [200, 150], radius: 10 })").unwrap();
    write_back(&mut s, &mut runtime);
    runtime.execute(&scope, "var circle = project.activeLayer.lastChild\nvar group = new Group([circle])\ngroup.addChild(circle.clone())").unwrap();
    write_back(&mut s, &mut runtime);
    runtime.execute(&scope, "project.activeLayer.lastChild.lastChild.position += [100, 0]").unwrap();
    write_back(&mut s, &mut runtime);
    let layer = &s.active().unwrap().doc.layers[0];
    let ids: Vec<NodeId> = layer.children().unwrap().iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 3, "a, b and the group: the circle moved into it");
    assert_eq!(&ids[..2], [a, b]);
    let group = layer.children().unwrap()[2].children().unwrap();
    let left: Vec<f64> = group.iter().map(|c| c.geometric_bounds().unwrap().x0.round()).collect();
    assert_eq!(left, [190.0, 290.0], "the clone, a new item in the new group, moved later");
}

/// Gives `id` the fill `paint`, as the app's Gradient panel does.
fn fill(s: &mut Session, id: NodeId, paint: Paint) {
    let mut node = node(s, id);
    let mut appearance = (*node.appearance).clone();
    appearance.set_fill(paint);
    node.appearance = appearance.into();
    s.execute("document.patch", &json!({ "replace": [node] })).unwrap();
}

fn gradient(kind: GradientKind, geom: Option<GradientGeom>) -> Paint {
    let mut stops = vec![GradientStop::new(0.0, Color::rgb(1.0, 0.0, 0.0)), GradientStop::new(1.0, Color::rgb(0.0, 0.0, 1.0))];
    stops[1].opacity = 0.5;
    Paint::Gradient(Box::new(GradientPaint { gradient: Gradient::new(kind, stops), geom, angle: 0.0, swatch: None, freeform: None }))
}

/// Linear and radial gradients read as Paper.js gradient colours, placed where they are, or where
/// they fit their object.
fn reads_gradients<E: Engine>() {
    let (mut s, a, b) = document();
    fill(&mut s, a, gradient(GradientKind::Linear, None));
    let geom = GradientGeom { start: Point::new(65.0, 20.0), end: Point::new(75.0, 20.0), aspect: 0.5, focal: Some(Point::new(67.0, 21.0)) };
    fill(&mut s, b, gradient(GradientKind::Radial, Some(geom)));
    let output = run::<E>(
        &mut s,
        "for (var item of project.activeLayer.children) {\n\
             var color = item.fillColor\n\
             console.log(color.type, color.gradient.radial, color.origin, color.destination, color.highlight, color.aspect)\n\
             console.log(color.gradient.stops.map(function(stop) { return stop.color.toCSS() + ' ' + stop.offset }).join(', '))\n\
         }",
    );
    assert_eq!(
        output,
        [
            "gradient false { x: 10, y: 20 } { x: 40, y: 20 } undefined 1",
            "rgb(255,0,0) 0, rgba(0,0,255,0.5) 1",
            "gradient true { x: 65, y: 20 } { x: 75, y: 20 } { x: 67, y: 21 } 0.5",
            "rgb(255,0,0) 0, rgba(0,0,255,0.5) 1",
        ]
    );
}

/// What Paper.js can't show stays while a script leaves a gradient alone: a gradient that fits its
/// object still fits it, and keeps its midpoints.
fn untouched_gradients_stay<E: Engine>() {
    let (mut s, a, _) = document();
    let mut paint = gradient(GradientKind::Linear, None);
    if let Paint::Gradient(g) = &mut paint {
        // A colour that Paper.js's JSON rounds:
        g.gradient.stops[0].color = Color::rgb(1.0 / 3.0, 0.0, 0.0);
        g.gradient.stops[0].midpoint = 0.3;
    }
    fill(&mut s, a, paint.clone());
    run::<E>(&mut s, "project.activeLayer.firstChild.segments[1].point.x += 20");
    assert_eq!(node(&s, a).geometric_bounds().unwrap().x1, 60.0, "the script changed the path");
    assert_eq!(node(&s, a).appearance.fill_paint(), paint);
}

/// A gradient a script makes is placed where the script puts it, and moves with its object.
fn writes_gradients<E: Engine>() {
    let (mut s, a, b) = document();
    fill(&mut s, b, gradient(GradientKind::Linear, None));
    run::<E>(
        &mut s,
        "var layer = project.activeLayer\n\
         layer.firstChild.fillColor = { gradient: { stops: [['yellow', 0.2], [new Color(0, 0, 0, 0.25), 0.8]], radial: true },\n\
             origin: [20, 20], destination: [30, 20], highlight: [22, 21] }\n\
         layer.lastChild.position += [100, 0]",
    );
    let Paint::Gradient(g) = node(&s, a).appearance.fill_paint() else { panic!("a gradient") };
    assert_eq!(g.gradient.kind, GradientKind::Radial);
    let stops: Vec<_> = g.gradient.stops.iter().map(|s| (s.offset, s.color, s.opacity)).collect();
    assert_eq!(stops, [(0.2, Color::rgb(1.0, 1.0, 0.0), 1.0), (0.8, Color::rgb(0.0, 0.0, 0.0), 0.25)]);
    let geom = GradientGeom { start: Point::new(20.0, 20.0), end: Point::new(30.0, 20.0), aspect: 1.0, focal: Some(Point::new(22.0, 21.0)) };
    assert_eq!(g.geom, Some(geom));
    let Paint::Gradient(g) = node(&s, b).appearance.fill_paint() else { panic!("a gradient") };
    let geom = g.geom.expect("a moved gradient is placed, as after any move");
    assert_eq!((geom.start, geom.end), (Point::new(150.0, 20.0), Point::new(180.0, 20.0)));
}

/// The pattern "Tiles", made of a copy of `a`, its tile 40 × 20 around it, filling `a` at its place
/// and `b` moved by 3.
fn patterns(s: &mut Session, a: NodeId, b: NodeId) {
    s.execute("object.pattern.make", &json!({"ids": [a.0], "name": "Tiles", "width": 40, "height": 20, "edit": false})).unwrap();
    fill(s, a, Paint::Pattern { pattern: "Tiles".into(), xf: Affine::IDENTITY });
    fill(s, b, Paint::Pattern { pattern: "Tiles".into(), xf: Affine::translate((3.0, 0.0)) });
}

/// Patterns read as pattern colours whose definition all their objects share, its art in pattern
/// space, where its tile starts at the origin.
fn reads_patterns<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    let output = run::<E>(
        &mut s,
        "var a = project.activeLayer.firstChild.fillColor, b = project.activeLayer.lastChild.fillColor\n\
         console.log(a.type, a.pattern === b.pattern, a.pattern.tile, a.pattern.item.firstChild.bounds)\n\
         console.log(a.matrix, b.matrix)",
    );
    assert_eq!(
        output,
        ["pattern true { x: 0, y: 0, width: 40, height: 20 } { x: 5, y: 0, width: 30, height: 20 }", "[[1, 0, 0], [0, 1, 0]] [[1, 0, 3], [0, 1, 0]]",]
    );
}

/// The document's patterns can be given to other objects, and move with the objects a script
/// moves. A script's own `PatternDefinition` becomes a new pattern.
fn writes_patterns<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    run::<E>(
        &mut s,
        "var layer = project.activeLayer, a = layer.firstChild, b = layer.lastChild\n\
         a.position += [100, 0]\n\
         new Path.Circle({ center: [200, 150], radius: 10, fillColor: { pattern: a.fillColor.pattern } })\n\
         b.fillColor = { pattern: new PatternDefinition(new Path.Rectangle({ point: [0, 0], size: [5, 5], insert: false })) }",
    );
    let pattern = |xf: Affine| Paint::Pattern { pattern: "Tiles".into(), xf };
    assert_eq!(node(&s, a).appearance.fill_paint(), pattern(Affine::translate((100.0, 0.0))));
    assert_eq!(node(&s, b).appearance.fill_paint(), Paint::Pattern { pattern: "New Pattern".into(), xf: Affine::IDENTITY });
    let circle = s.active().unwrap().doc.layers[0].children().unwrap().last().unwrap().clone();
    assert_eq!(circle.appearance.fill_paint(), pattern(Affine::IDENTITY));
    let new = s.active().unwrap().doc.pattern("New Pattern").unwrap().clone();
    assert_eq!((new.tile, new.art[0].geometric_bounds().unwrap()), (Rect::new(0.0, 0.0, 5.0, 5.0), Rect::new(0.0, 0.0, 5.0, 5.0)));
    assert!(s.active().unwrap().doc.swatch_name_taken("New Pattern"), "it gets a swatch");
}

/// What a script changes in a pattern goes back to it: its art in pattern space, where the tile
/// keeps its origin, and the tile's new size.
fn writes_changed_patterns<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    let before = s.active().unwrap().doc.pattern("Tiles").unwrap().clone();
    run::<E>(
        &mut s,
        "var pattern = project.activeLayer.firstChild.fillColor.pattern\n\
         pattern.item.firstChild.fillColor = 'red'\n\
         pattern.tile = [0, 0, 80, 40]",
    );
    let after = s.active().unwrap().doc.pattern("Tiles").unwrap().clone();
    assert_eq!(after.tile, Rect::from_origin_size(before.tile.origin(), (80.0, 40.0)));
    assert_eq!(after.art[0].geometric_bounds(), before.art[0].geometric_bounds(), "the art stays where it was");
    assert_eq!(after.art[0].appearance.fill_paint(), Paint::solid(Color::rgb(1.0, 0.0, 0.0)));
    assert_eq!(node(&s, a).appearance.fill_paint(), Paint::Pattern { pattern: "Tiles".into(), xf: Affine::IDENTITY }, "its objects keep it");
}

/// A pattern that changes in the document changes in the definition the script holds.
fn changed_patterns_reload<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "globalThis.pattern = project.activeLayer.firstChild.fillColor.pattern").unwrap();
    let mut doc = (*st.doc).clone();
    doc.patterns[0].tile = vectorcraft_geom::Rect::new(0.0, 0.0, 50.0, 25.0);
    runtime.update(&scope, Arc::new(doc), st.selection.clone()).unwrap();
    runtime.execute(&scope, "console.log(pattern === project.activeLayer.firstChild.fillColor.pattern, pattern.tile)").unwrap();
    assert_eq!(runtime.take_output(), ["true { x: 0, y: 0, width: 50, height: 25 }"]);
}

/// The symbol "Box" made of `a`, which becomes its instance, and a second instance at 200, 150.
fn symbols(s: &mut Session, a: NodeId) -> (NodeId, NodeId) {
    let id = |v: serde_json::Value| NodeId(v["id"].as_u64().unwrap());
    let first = id(s.execute("symbol.new", &json!({"name": "Box", "ids": [a.0]})).unwrap());
    let second = id(s.execute("symbol.place", &json!({"name": "Box", "x": 200, "y": 150})).unwrap());
    (first, second)
}

/// Symbol instances are `SymbolItem`s, measured by their symbol's art with its strokes, as
/// Paper.js measures them, whose definition all its instances share.
fn reads_symbols<E: Engine>() {
    let (mut s, a, _) = document();
    let (first, second) = symbols(&mut s, a);
    let output = run::<E>(
        &mut s,
        &format!(
            "var first = project.getItemByKey({}), second = project.getItemByKey({})\n\
             console.log(first.className, first.definition === second.definition, first.definition.item.className)\n\
             console.log(first.bounds, second.bounds.center)",
            first.0, second.0
        ),
    );
    assert_eq!(
        output,
        // The art of 20 × 20 with its stroke of 1, stretched to the rectangle's 30 × 20:
        ["SymbolItem true Path", "{ x: 9.25, y: 9.5, width: 31.5, height: 21 } { x: 200, y: 150 }"]
    );
}

/// What a script does to an instance goes back to it, and new instances of the document's symbols
/// are added to it. A transform the script leaves alone stays exactly as it was.
fn writes_symbols<E: Engine>() {
    let (mut s, a, _) = document();
    let (first, second) = symbols(&mut s, a);
    let mut turned = node(&s, second);
    let xf = Affine::rotate(0.123456789).then_translate((200.0, 150.0).into());
    turned.kind = NodeKind::SymbolInstance { symbol: "Box".into(), xf };
    s.execute("document.patch", &json!({ "replace": [turned] })).unwrap();
    run::<E>(
        &mut s,
        &format!(
            "var first = project.getItemByKey({}), second = project.getItemByKey({})\n\
             first.position += [100, 0]\n\
             second.opacity = 0.5\n\
             new SymbolItem(first.definition, [50, 200])",
            first.0, second.0
        ),
    );
    assert_eq!(node(&s, first).geometric_bounds().unwrap().x0, 110.0);
    assert_eq!(node(&s, second).kind, NodeKind::SymbolInstance { symbol: "Box".into(), xf });
    assert_eq!(node(&s, second).opacity, 0.5);
    let added = s.active().unwrap().doc.layers[0].children().unwrap().last().unwrap().clone();
    let NodeKind::SymbolInstance { symbol, xf } = &added.kind else { panic!("an instance") };
    assert_eq!((symbol.as_str(), xf.translation()), ("Box", (50.0, 200.0).into()));
}

/// A pattern sized to its art stays so when a script changes only its art: the tile follows it.
fn patterns_sized_to_art_follow_it<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    s.execute("pattern.options", &json!({"name": "Tiles", "sizeTileToArt": true})).unwrap();
    run::<E>(&mut s, "project.activeLayer.firstChild.fillColor.pattern.item.firstChild.scale(2)");
    let pattern = s.active().unwrap().doc.pattern("Tiles").unwrap().clone();
    assert!(pattern.size_tile_to_art);
    assert_eq!(pattern.tile.size(), pattern.art[0].geometric_bounds().unwrap().size());
}

/// What a script changes in a symbol's art goes back to it, in the symbol's space: its instances
/// keep their transforms.
fn writes_changed_symbols<E: Engine>() {
    let (mut s, a, _) = document();
    let (first, second) = symbols(&mut s, a);
    let xfs = |s: &Session| [first, second].map(|id| node(s, id).kind);
    let before = xfs(&s);
    run::<E>(&mut s, "var item = project.symbolDefinitions[0].item\nitem.scale(2, [0, 0])\nitem.position += [5, 0]");
    let art = s.active().unwrap().doc.symbol("Box").unwrap().art.geometric_bounds().unwrap();
    // The symbol's art, the 20 × 20 square around its origin, doubled and moved:
    assert_eq!(art, Rect::new(-15.0, -20.0, 25.0, 20.0));
    assert_eq!(xfs(&s), before);
    let output = run::<E>(&mut s, "console.log(project.symbolDefinitions[0].item.bounds)");
    assert_eq!(output, ["{ x: -15, y: -20, width: 40, height: 40 }"], "read again, it stays where it is");
}

/// A script's own symbols become the document's, named as the script names them, else "New
/// Symbol", numbered where the document has the name.
fn writes_new_symbols<E: Engine>() {
    let (mut s, a, _) = document();
    symbols(&mut s, a);
    run::<E>(
        &mut s,
        "var brick = new SymbolDefinition(new Path.Rectangle({ point: [10, 10], size: [20, 10], fillColor: 'red' }))\n\
         brick.name = 'Box'\n\
         new SymbolItem(brick, [200, 50])\n\
         new SymbolItem(new SymbolDefinition(new Path.Circle([0, 0], 5)), [50, 50])",
    );
    let doc = &s.active().unwrap().doc;
    let names: Vec<&str> = doc.symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Box", "Box 2", "New Symbol"]);
    assert_eq!(doc.symbol("Box 2").unwrap().art.geometric_bounds().unwrap(), Rect::new(-10.0, -5.0, 10.0, 5.0), "Paper.js centres it");
    let added: Vec<_> = doc.layers[0].children().unwrap().iter().rev().take(2).map(|n| n.kind.clone()).collect();
    assert_eq!(
        added,
        [
            NodeKind::SymbolInstance { symbol: "New Symbol".into(), xf: Affine::translate((50.0, 50.0)) },
            NodeKind::SymbolInstance { symbol: "Box 2".into(), xf: Affine::translate((200.0, 50.0)) },
        ]
    );
}

/// `project.symbolDefinitions` and `project.patternDefinitions` list the document's definitions,
/// and the script's own that its items use.
fn lists_definitions<E: Engine>() {
    let (mut s, a, b) = document();
    patterns(&mut s, a, b);
    symbols(&mut s, b);
    let output = run::<E>(
        &mut s,
        "new SymbolItem(new SymbolDefinition(new Path.Circle([0, 0], 5)), [50, 50])\n\
         console.log(project.symbolDefinitions.map(d => d.key + ' ' + d.name).join(), project.patternDefinitions.map(d => d.name).join())",
    );
    assert_eq!(output, ["symbol/Box Box,null null Tiles"]);
}

/// A new definition whose patch failed reloads from the document, which doesn't have it: it's
/// empty, as a new item of a failed patch is gone.
fn failed_new_definitions_reload<E: Engine>() {
    let (s, ..) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime
        .execute(&scope, "globalThis.definition = new SymbolDefinition(new Path.Circle([0, 0], 5))\nnew SymbolItem(definition, [50, 50])")
        .unwrap();
    let patch = runtime.take_patch(&scope).unwrap().unwrap();
    runtime.apply_ids(&scope, patch, &json!({}), st.doc.clone(), st.selection.clone(), true).unwrap();
    runtime.execute(&scope, "console.log(definition.key, definition.item.isEmpty())").unwrap();
    assert_eq!(runtime.take_output(), ["symbol/New Symbol true"]);
}

/// Gives `id` the effect `effect` with `params`, its own, or on its appearance item `item`.
fn effect(s: &mut Session, id: NodeId, effect: &str, params: serde_json::Value, item: Option<u64>) {
    s.execute("effect.apply", &json!({ "effect": effect, "params": params, "ids": [id.0], "item": item })).unwrap();
}

/// An object's own effects are its item's: Offset Path and Outline Stroke as Paper.js's types, with
/// all their parameters, the others by their ids.
fn reads_effects<E: Engine>() {
    let (mut s, a, _) = document();
    effect(&mut s, a, "path.offsetPath", json!({ "offset": 5 }), None);
    effect(&mut s, a, "distort.zigZag", json!({ "size": 3, "ridges": 2 }), None);
    let output = run::<E>(
        &mut s,
        &format!(
            "var effects = project.getItemByKey({}).effects\n\
             effects.forEach(function(effect) {{ console.log(effect.type, JSON.stringify(effect.parameters), effect.visible) }})",
            a.0
        ),
    );
    assert_eq!(output[0], r#"offset {"distance":5,"join":"miter","miterLimit":4} true"#);
    assert_eq!(output[1].split(' ').next(), Some("distort.zigZag"));
}

/// `previewBounds` measures what the app's geometry effects draw, those Paper.js doesn't have
/// too, and leaves raster effects out.
fn preview_bounds_follow_the_apps_effects<E: Engine>() {
    let (mut s, a, b) = document();
    effect(&mut s, a, "path.offsetPath", json!({ "offset": 5 }), None);
    effect(&mut s, b, "distort.transform", json!({ "moveH": 10 }), None);
    effect(&mut s, b, "stylize.dropShadow", json!({}), None);
    let output = run::<E>(
        &mut s,
        &format!("console.log(project.getItemByKey({}).previewBounds)\nconsole.log(project.getItemByKey({}).previewBounds)", a.0, b.0),
    );
    // Both with the half of their 1 pt stroke outside:
    assert_eq!(output, ["{ x: 4.5, y: 4.5, width: 41, height: 31 }", "{ x: 59.5, y: 9.5, width: 31, height: 21 }"]);
}

/// Objects Paper.js has no class for, such as blends, are measured with their effects too.
fn preview_bounds_of_objects_without_a_class<E: Engine>() {
    let (mut s, a, b) = document();
    let blend = NodeId(s.execute("object.blend.make", &json!({ "ids": [a.0, b.0], "steps": 2 })).unwrap()["id"].as_u64().unwrap());
    effect(&mut s, blend, "distort.transform", json!({ "moveH": 10 }), None);
    let output = run::<E>(
        &mut s,
        &format!("var item = project.getItemByKey({})\nconsole.log(item.className, item.strokeBounds, item.previewBounds)", blend.0),
    );
    assert_eq!(output, ["HostItem { x: 9.5, y: 9.5, width: 71, height: 21 } { x: 19.5, y: 9.5, width: 71, height: 21 }"]);
}

/// Installed effect plug-ins draw what scripts' effects draw too.
fn preview_bounds_of_plugin_effects<E: Engine>() {
    let (mut s, a, _) = document();
    // An effect that draws a 30 × 20 triangle at the origin:
    let path = vectorcraft_geom::PathData::single(vectorcraft_geom::SubPath::polyline(
        &[Point::new(0.0, 0.0), Point::new(30.0, 0.0), Point::new(15.0, 20.0)],
        true,
    ));
    let triangle = json!({ "objects": [{ "path": path }] });
    let manifest = vectorcraft_plugins::wat::manifest("test.scripts.triangle", "effect", "{}");
    let wasm = wat::parse_str(vectorcraft_plugins::wat::returning(&manifest, &triangle.to_string())).unwrap();
    s.execute("plugin.install", &json!({ "dataBase64": vectorcraft_format::base64_encode(&wasm) })).unwrap();
    effect(&mut s, a, "plugin.test.scripts.triangle", json!({}), None);
    let output = run::<E>(&mut s, &format!("var bounds = project.getItemByKey({}).previewBounds\nconsole.log(bounds.width, bounds.height)", a.0));
    // As the app measures it with Use Preview Bounds, the triangle's stroke and its miters included:
    let app = vectorcraft_effects::preview::bounds_of(&s.active().unwrap().doc, &[a], true).unwrap();
    let measured: Vec<f64> = output[0].split(' ').map(|n| n.parse().unwrap()).collect();
    assert!((measured[0] - app.width()).abs() < 1e-3 && (measured[1] - app.height()).abs() < 1e-3, "{measured:?} {app:?}");
}

/// Effects a script changes, adds or removes go back as the object's own, by the app's names, and
/// the effects on its fills and strokes stay.
fn writes_effects<E: Engine>() {
    let (mut s, a, _) = document();
    effect(&mut s, a, "path.offsetPath", json!({ "offset": 5 }), None);
    effect(&mut s, a, "distort.roughen", json!({ "size": 2 }), Some(0));
    run::<E>(
        &mut s,
        &format!(
            "var item = project.getItemByKey({})\n\
             item.effects[0].parameters.distance = 8\n\
             item.addEffect({{ type: 'distort.transform', parameters: {{ moveH: 3 }} }})",
            a.0
        ),
    );
    let n = node(&s, a);
    let own: Vec<_> = n.appearance.effects.iter().map(|e| (e.id.as_str(), e.params.clone())).collect();
    assert_eq!(own[0], ("path.offsetPath", json!({ "offset": 8, "joins": "miter", "miterLimit": 4 })));
    // A new effect has the app's defaults for what the script doesn't set:
    assert_eq!((own[1].0, &own[1].1["moveH"], &own[1].1["scaleH"]), ("distort.transform", &json!(3), &json!(100)));
    let item = n.appearance.items.iter().find(|item| !item_effects(item).is_empty()).expect("the fill keeps its effect");
    assert_eq!(item_effects(item)[0].id, "distort.roughen");
}

fn item_effects(item: &vectorcraft_doc::AppearanceItem) -> &[vectorcraft_doc::Effect] {
    match item {
        vectorcraft_doc::AppearanceItem::Fill(f) => &f.effects,
        vectorcraft_doc::AppearanceItem::Stroke(s) => &s.effects,
    }
}

/// Groups' effects go back too, which Paper.js doesn't export with their content.
fn writes_effects_of_groups<E: Engine>() {
    let (mut s, a, b) = document();
    s.execute("select.set", &json!({ "ids": [a.0, b.0] })).unwrap();
    let group = NodeId(s.execute("object.group", &json!({})).unwrap()["id"].as_u64().unwrap());
    run::<E>(&mut s, &format!("project.getItemByKey({}).addEffect({{ type: 'offset', parameters: {{ distance: 3 }} }})", group.0));
    let effects: Vec<_> = node(&s, group).appearance.effects.iter().map(|e| (e.id.clone(), e.params["offset"].clone())).collect();
    assert_eq!(effects, [("path.offsetPath".to_string(), json!(3))]);
}

/// Effects a script doesn't change stay as they are, parameters of any name included, when it
/// changes something else of their object.
fn untouched_effects_stay<E: Engine>() {
    let (mut s, a, _) = document();
    effect(&mut s, a, "path.offsetPath", json!({ "offset": 5 }), None);
    effect(&mut s, a, "pixelate.mezzotint", json!({ "type": "coarseDots" }), None);
    let before = node(&s, a).appearance.effects.clone();
    run::<E>(&mut s, &format!("var item = project.getItemByKey({})\nitem.opacity = 0.5\nitem.effects[1].visible = true", a.0));
    let n = node(&s, a);
    assert_eq!(n.opacity, 0.5);
    assert_eq!(n.appearance.effects, before);
}

/// A script's new items take their effects into the document.
fn new_items_have_effects<E: Engine>() {
    let (mut s, ..) = document();
    run::<E>(
        &mut s,
        "new Path.Rectangle({ point: [0, 0], size: [10, 10], fillColor: 'red', effects: [{ type: 'offset', parameters: { distance: 2 } }] })",
    );
    let doc = &s.active().unwrap().doc;
    let added = doc.layers[0].children().unwrap().last().unwrap();
    let effects: Vec<_> = added.appearance.effects.iter().map(|e| (e.id.as_str(), e.params.clone())).collect();
    assert_eq!(effects, [("path.offsetPath", json!({ "offset": 2, "joins": "miter", "miterLimit": 4 }))]);
}

/// A symbol whose art changes in the document changes in the definition the script holds, and its
/// instances and what holds them are measured again.
fn changed_symbols_reload<E: Engine>() {
    let (mut s, a, _) = document();
    let (first, _) = symbols(&mut s, a);
    let st = s.active().unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, &format!("globalThis.first = project.getItemByKey({})\nglobalThis.definition = first.definition", first.0)).unwrap();
    let mut doc = (*st.doc).clone();
    let mut art = (*doc.symbols[0].art).clone();
    art.transform(Affine::scale(2.0), false);
    doc.symbols[0].art = Arc::new(art);
    runtime.update(&scope, Arc::new(doc), st.selection.clone()).unwrap();
    runtime
        .execute(&scope, "console.log(definition === first.definition, first.bounds.width, first.bounds.height, project.activeLayer.bounds)")
        .unwrap();
    // The art of 40 × 40 with its stroke of 1, stretched by 1.5, and the layer measured again
    // around both instances, now 60 × 40 (without strokes) at their centres, 25, 20 and 200, 150:
    assert_eq!(runtime.take_output(), ["true 61.5 41 { x: -5, y: 0, width: 235, height: 170 }"]);
}

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn reads_the_document() {
                super::reads_the_document::<$engine>();
            }

            #[test]
            fn reads_gradients() {
                super::reads_gradients::<$engine>();
            }

            #[test]
            fn untouched_gradients_stay() {
                super::untouched_gradients_stay::<$engine>();
            }

            #[test]
            fn writes_gradients() {
                super::writes_gradients::<$engine>();
            }

            #[test]
            fn reads_patterns() {
                super::reads_patterns::<$engine>();
            }

            #[test]
            fn writes_patterns() {
                super::writes_patterns::<$engine>();
            }

            #[test]
            fn changed_patterns_reload() {
                super::changed_patterns_reload::<$engine>();
            }

            #[test]
            fn reads_symbols() {
                super::reads_symbols::<$engine>();
            }

            #[test]
            fn writes_symbols() {
                super::writes_symbols::<$engine>();
            }

            #[test]
            fn changed_symbols_reload() {
                super::changed_symbols_reload::<$engine>();
            }

            #[test]
            fn writes_changed_patterns() {
                super::writes_changed_patterns::<$engine>();
            }

            #[test]
            fn patterns_sized_to_art_follow_it() {
                super::patterns_sized_to_art_follow_it::<$engine>();
            }

            #[test]
            fn writes_changed_symbols() {
                super::writes_changed_symbols::<$engine>();
            }

            #[test]
            fn writes_new_symbols() {
                super::writes_new_symbols::<$engine>();
            }

            #[test]
            fn reads_effects() {
                super::reads_effects::<$engine>();
            }

            #[test]
            fn preview_bounds_follow_the_apps_effects() {
                super::preview_bounds_follow_the_apps_effects::<$engine>();
            }

            #[test]
            fn preview_bounds_of_objects_without_a_class() {
                super::preview_bounds_of_objects_without_a_class::<$engine>();
            }

            #[test]
            fn preview_bounds_of_plugin_effects() {
                super::preview_bounds_of_plugin_effects::<$engine>();
            }

            #[test]
            fn writes_effects() {
                super::writes_effects::<$engine>();
            }

            #[test]
            fn writes_effects_of_groups() {
                super::writes_effects_of_groups::<$engine>();
            }

            #[test]
            fn untouched_effects_stay() {
                super::untouched_effects_stay::<$engine>();
            }

            #[test]
            fn new_items_have_effects() {
                super::new_items_have_effects::<$engine>();
            }

            #[test]
            fn lists_definitions() {
                super::lists_definitions::<$engine>();
            }

            #[test]
            fn failed_new_definitions_reload() {
                super::failed_new_definitions_reload::<$engine>();
            }

            #[test]
            fn writes_changes_back_as_one_undo_step() {
                super::writes_changes_back_as_one_undo_step::<$engine>();
            }

            #[test]
            fn paint_order_is_the_order_of_fill_and_stroke() {
                super::paint_order_is_the_order_of_fill_and_stroke::<$engine>();
            }

            #[test]
            fn removes_and_reorders() {
                super::removes_and_reorders::<$engine>();
            }

            #[test]
            fn on_frame_keeps_changing_the_document() {
                super::on_frame_keeps_changing_the_document::<$engine>();
            }

            #[test]
            fn a_late_patch_leaves_what_changed_meanwhile() {
                super::a_late_patch_leaves_what_changed_meanwhile::<$engine>();
            }

            #[test]
            fn scripts_without_on_frame_stop() {
                super::scripts_without_on_frame_stop::<$engine>();
            }

            #[test]
            fn edits_made_while_a_script_runs_stay() {
                super::edits_made_while_a_script_runs_stay::<$engine>();
            }

            #[test]
            fn selection_goes_both_ways() {
                super::selection_goes_both_ways::<$engine>();
            }

            #[test]
            fn artboards_and_the_active_one() {
                super::artboards_and_the_active_one::<$engine>();
            }

            #[test]
            fn tool_scripts_get_pointer_events() {
                super::tool_scripts_get_pointer_events::<$engine>();
            }

            #[test]
            fn items_moved_into_a_new_group() {
                super::items_moved_into_a_new_group::<$engine>();
            }

            #[test]
            fn tool_scripts_get_key_events() {
                super::tool_scripts_get_key_events::<$engine>();
            }

            #[test]
            fn scripts_without_handlers_have_no_tool() {
                super::scripts_without_handlers_have_no_tool::<$engine>();
            }

            #[test]
            fn edits_to_objects_a_script_made_reach_it() {
                super::edits_to_objects_a_script_made_reach_it::<$engine>();
            }

            #[test]
            fn untouched_scripts_change_nothing() {
                super::untouched_scripts_change_nothing::<$engine>();
            }
        }
    };
}

#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_scriptographer::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_scriptographer::V8);
