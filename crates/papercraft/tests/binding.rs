#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Scripts on a real document: Paper.js reads it through its item source, and what a script
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
