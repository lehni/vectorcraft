#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Scripts on a real document: Paper.js reads it through its host interface, and what a script
//! changes goes back through `document.patch` as one undo step.

use serde_json::json;
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Node, NodeId};
use vectorcraft_engine::Session;
use vectorcraft_papercraft::{Engine, Runtime};

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
fn frame<E: Engine>(s: &mut Session, runtime: &mut Runtime<E>, scope: &vectorcraft_papercraft::Scope<E::Handle>, now: f64) {
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
             console.log(project.activeArtboard.name, project.activeArtboard.bounds.width, document.activeArtboard.name)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        ["Artboard 1 { x: 0, y: 0, width: 400, height: 300 }", "Artboard 2 { x: 500, y: 0, width: 200, height: 100 }", "Artboard 2 200 Artboard 2"]
    );
}

fn tool_scripts_get_pointer_events<E: Engine>() {
    use vectorcraft_papercraft::{Modifiers, MouseEvent};
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
    use vectorcraft_papercraft::Modifiers;
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

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn reads_the_document() {
                super::reads_the_document::<$engine>();
            }

            #[test]
            fn writes_changes_back_as_one_undo_step() {
                super::writes_changes_back_as_one_undo_step::<$engine>();
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
engine_tests!(boa, vectorcraft_papercraft::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_papercraft::V8);

#[test]
fn zz_after_delete() {
    use vectorcraft_papercraft::{Modifiers, MouseEvent, V8};
    let (mut s, a, b) = document();
    let st = s.active().unwrap();
    let mut runtime = Runtime::<V8>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, st.doc.clone(), st.selection.clone()).unwrap();
    runtime.execute(&scope, "var worm\nfunction onMouseDown(e) { worm = new Path(); worm.add(e.point); console.log('down', worm.parent && worm.parent.key, worm.isInserted(), project.layers.length) }\nfunction onMouseDrag(e) { worm.add(e.point) }").unwrap();
    let step = |s: &mut Session, runtime: &mut Runtime<V8>, ev, x: f64| {
        let st = s.active().unwrap();
        runtime.update(&scope, st.doc.clone(), st.selection.clone()).unwrap();
        runtime.mouse_event(&scope, ev, (x, 50.0), Modifiers::default(), 0.0).unwrap();
        if let Some(patch) = runtime.take_patch(&scope).unwrap() {
            println!("patch {}", patch.params);
            let result = s.execute("document.patch", &patch.params).unwrap();
            let st = s.active().unwrap();
            runtime.apply_ids(&scope, patch, &result, st.doc.clone(), st.selection.clone(), false).unwrap();
        } else {
            println!("no patch");
        }
    };
    // Touch the existing objects first, as the hover and the earlier worms did.
    runtime.execute(&scope, "console.log(project.activeLayer.children.length)").unwrap();
    s.execute("select.set", &json!({"ids": [a.0, b.0]})).unwrap();
    step(&mut s, &mut runtime, MouseEvent::Move, 5.0);
    s.execute("edit.clear", &json!({})).unwrap();
    step(&mut s, &mut runtime, MouseEvent::Move, 6.0);
    step(&mut s, &mut runtime, MouseEvent::Down, 10.0);
    step(&mut s, &mut runtime, MouseEvent::Move, 30.0);
    println!("{:?}", runtime.take_output());
}
