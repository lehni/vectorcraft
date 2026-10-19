//! PaperCraft scripts in the app: running ones keep running through what the user does.

use serde_json::json;
use vectorcraft_engine::Session;

use crate::VectorcraftApp;

/// An app with one rectangle.
fn app() -> (VectorcraftApp, u64) {
    let mut app = VectorcraftApp::new(Session::new(), Default::default());
    app.run("file.new", json!({})).unwrap();
    let id = app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 30, "height": 20})).unwrap()["id"].as_u64().unwrap();
    (app, id)
}

/// An app with one rectangle, running a script that moves it 10 pt right on every frame.
fn running() -> (VectorcraftApp, u64, tempfile_path::Path) {
    let (mut app, id) = app();
    let path = tempfile_path::Path::new("var item = project.activeLayer.firstChild\nfunction onFrame() { item.position += [10, 0] }");
    let result = app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert_eq!(result["running"], true);
    (app, id, path)
}

fn pointer(app: &mut VectorcraftApp, kind: vectorcraft_tools::PointerKind, x: f64) -> bool {
    let ev = vectorcraft_tools::PointerEvent { kind, pos: vectorcraft_geom::Point::new(x, 100.0), mods: Default::default(), pressure: 1.0 };
    crate::scripts::pointer(app, &ev)
}

fn children(app: &VectorcraftApp) -> usize {
    app.session.doc().unwrap().doc.layers[0].children().unwrap().len()
}

fn left(app: &VectorcraftApp, id: u64) -> f64 {
    app.session.doc().unwrap().doc.node(vectorcraft_doc::NodeId(id)).unwrap().geometric_bounds().unwrap().x0
}

/// A script file that is removed when dropped.
mod tempfile_path {
    pub struct Path(pub std::path::PathBuf);

    impl Path {
        pub fn new(source: &str) -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = &N;
            let path =
                std::env::temp_dir().join(format!("papercraft-{}-{}.js", std::process::id(), n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::write(&path, source).unwrap();
            Self(path)
        }
    }

    impl Drop for Path {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// While the user drags, a running script goes on: its changes go beneath the drag
/// (`document.patch` during an interaction), and the drag wins where both change the same object.
#[test]
fn a_running_script_goes_on_through_an_interaction() {
    let (mut app, id, _path) = running();
    let ctx = egui::Context::default();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(left(&app, id), 20.0, "a frame moves it");
    // The user drags something else: the script keeps moving the rectangle.
    app.session.begin_interaction("Move").unwrap();
    crate::scripts::frame(&mut app, &ctx, 2.0);
    assert!(app.scripts.is_running(), "the script still runs: {}", app.ui.status);
    assert_eq!(left(&app, id), 30.0, "and moves it during the drag");
    // The user drags the rectangle itself: their move wins over the script's.
    app.session.preview("object.move", &json!({"ids": [id], "dx": 100, "dy": 0})).unwrap();
    crate::scripts::frame(&mut app, &ctx, 3.0);
    assert_eq!(left(&app, id), 130.0, "the drag wins on the object it moves");
    app.session.commit_interaction().unwrap();
    crate::scripts::frame(&mut app, &ctx, 4.0);
    assert_eq!(left(&app, id), 140.0, "and the script goes on from there");
}

#[test]
fn a_tool_script_draws_with_the_mouse_one_undo_step_per_drag() {
    use vectorcraft_tools::PointerKind::{Down, Drag, Up};
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("tool.minDistance = 10\nfunction onMouseDrag(event) { new Path.Circle({ center: event.point, radius: 3 }) }");
    let result = app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert_eq!(result["tool"], true);
    assert!(app.scripts.active_tool().is_some(), "a tool script becomes the active tool");
    let steps = app.session.doc().unwrap().history.undo.len();
    for (kind, x) in [(Down, 0.0), (Drag, 15.0), (Drag, 30.0), (Drag, 45.0), (Up, 45.0)] {
        assert!(pointer(&mut app, kind, x), "the tool script has the event");
    }
    assert_eq!(children(&app), 4, "three drag events drew three circles");
    assert_eq!(app.session.doc().unwrap().history.undo.len(), steps + 1, "the drag is one undo step");
    for (kind, x) in [(Down, 0.0), (Drag, 15.0), (Up, 15.0)] {
        pointer(&mut app, kind, x);
    }
    assert_eq!(app.session.doc().unwrap().history.undo.len(), steps + 2, "the next drag is a step of its own");
    // Choosing one of the app's tools gives the canvas back to it.
    app.select_tool("selection");
    assert!(!pointer(&mut app, Down, 0.0));
    assert!(app.scripts.tool().is_some(), "the tool script stays assigned to the PaperCraft Tool");
    // The PaperCraft Tool's button makes it the active tool again.
    app.scripts.select_tool();
    assert!(pointer(&mut app, Down, 0.0));
}
