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

/// The user drags another object with the Selection tool while an animation runs, the way the
/// canvas sends the drag: the animation goes on.
#[test]
fn an_animation_goes_on_while_the_user_moves_another_object() {
    use vectorcraft_tools::PointerKind::{Down, Drag, Up};
    let (mut app, id, _path) = running();
    let other = app.run("shape.rectangle", json!({"x": 200, "y": 200, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap();
    app.select_tool("selection");
    let ctx = egui::Context::default();
    let view = app.view_info();
    let mut now = 1.0;
    for (kind, x) in [(Down, 220.0), (Drag, 230.0), (Drag, 240.0), (Drag, 250.0), (Up, 250.0)] {
        let ev = vectorcraft_tools::PointerEvent { kind, pos: vectorcraft_geom::Point::new(x, 220.0), mods: Default::default(), pressure: 1.0 };
        crate::canvas::dispatch(&mut app, &ev, view);
        crate::scripts::frame(&mut app, &ctx, now);
        now += 1.0;
        assert!(app.scripts.is_running(), "after {kind:?}: {}", app.ui.status);
    }
    assert_eq!(left(&app, other), 230.0, "the user's drag moved the other object");
    assert_eq!(left(&app, id), 60.0, "and five frames moved the animated one");
}

/// The user drags the animated object itself: their drag wins while it lasts, and the animation
/// goes on from where they left it.
#[test]
fn an_animation_goes_on_after_the_user_moves_its_object() {
    use vectorcraft_tools::PointerKind::{Down, Drag, Up};
    let (mut app, id, _path) = running();
    app.select_tool("selection");
    let ctx = egui::Context::default();
    let view = app.view_info();
    let mut now = 1.0;
    for (kind, x) in [(Down, 20.0), (Drag, 30.0), (Drag, 60.0), (Up, 60.0)] {
        let ev = vectorcraft_tools::PointerEvent { kind, pos: vectorcraft_geom::Point::new(x, 20.0), mods: Default::default(), pressure: 1.0 };
        crate::canvas::dispatch(&mut app, &ev, view);
        crate::scripts::frame(&mut app, &ctx, now);
        now += 1.0;
        assert!(app.scripts.is_running(), "after {kind:?}: {}", app.ui.status);
    }
    let after_drag = left(&app, id);
    crate::scripts::frame(&mut app, &ctx, now);
    assert!(app.scripts.is_running(), "{}", app.ui.status);
    assert_eq!(left(&app, id), after_drag + 10.0, "the animation goes on from where the drag left it");
}

/// A tool script that draws a circle where the mouse goes down.
const TOOL: &str = "function onMouseDown(event) { new Path.Circle({ center: event.point, radius: 5 }) }";

/// There is one PaperCraft Tool button: running a tool script replaces the one it stood for.
#[test]
fn a_tool_script_replaces_the_previous_one() {
    let (mut app, _) = app();
    let (first, second) = (tempfile_path::Path::new(TOOL), tempfile_path::Path::new(TOOL));
    for path in [&first, &second] {
        app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    }
    assert_eq!(app.scripts.running_paths(), std::collections::BTreeSet::from([second.0.clone()]), "only the second one is loaded");
    assert_eq!(app.scripts.tool(), Some(second.0.as_path()));
}

/// A script runs on the document it started on: it waits while another document is active, and
/// stops when its document closes.
#[test]
fn scripts_stop_when_their_document_closes() {
    let (mut app, id, _path) = running();
    let ctx = egui::Context::default();
    app.run("file.new", json!({})).unwrap();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(children(&app), 0, "the script doesn't reach the other document");
    app.run("file.close", json!({})).unwrap();
    crate::scripts::frame(&mut app, &ctx, 2.0);
    assert_eq!(left(&app, id), 20.0, "back on its document, it goes on");
    // Closed with Don't Save.
    app.run("file.close", json!({})).unwrap();
    app.ui.dialog.as_mut().unwrap().fields.insert("discard".into(), json!(true));
    crate::dialogs::confirm(&mut app).unwrap();
    crate::scripts::frame(&mut app, &ctx, 3.0);
    assert!(app.scripts.running_paths().is_empty(), "it stopped with its document");
    assert!(app.scripts.log().iter().all(|l| !l.error), "without errors");
}

/// The PaperCraft Tool is global: it draws in whichever document is active, and stays when a
/// document closes.
#[test]
fn the_tool_works_in_every_document() {
    let (mut app, _) = app();
    let tool = tempfile_path::Path::new(TOOL);
    app.run("ui.runScript", json!({ "path": tool.0.to_string_lossy() })).unwrap();
    app.run("file.new", json!({})).unwrap();
    let ctx = egui::Context::default();
    for _ in 0..2 {
        assert!(pointer(&mut app, vectorcraft_tools::PointerKind::Down, 50.0));
        pointer(&mut app, vectorcraft_tools::PointerKind::Up, 50.0);
    }
    assert_eq!(children(&app), 2, "it draws in the other document, run there once");
    app.run("file.close", json!({})).unwrap();
    app.ui.dialog.as_mut().unwrap().fields.insert("discard".into(), json!(true));
    crate::dialogs::confirm(&mut app).unwrap();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(app.scripts.tool(), Some(tool.0.as_path()), "it stays the PaperCraft Tool's");
    assert!(pointer(&mut app, vectorcraft_tools::PointerKind::Down, 50.0));
    assert_eq!(children(&app), 2, "and draws in the first document");
    assert!(app.scripts.log().iter().all(|l| !l.error));
}

/// A script that loops forever doesn't hold up the app: it goes on on its thread, and Stop
/// Running Scripts ends it.
#[test]
fn an_endless_script_goes_on_alone_until_stopped() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("while (true) {}");
    let result = app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert_eq!(result["started"], true, "the app went on");
    crate::scripts::frame(&mut app, &egui::Context::default(), 1.0);
    assert!(app.scripts.is_running(), "the script still works");
    app.run("ui.stopScripts", json!({})).unwrap();
    assert!(!app.scripts.is_running());
}

/// While the PaperCraft Tool is the active tool, the canvas shows its pencil.
#[test]
fn the_tool_has_its_own_cursor() {
    let (mut app, _) = app();
    assert_eq!(crate::scripts::cursor(&app), None);
    let tool = tempfile_path::Path::new(TOOL);
    app.run("ui.runScript", json!({ "path": tool.0.to_string_lossy() })).unwrap();
    assert_eq!(crate::scripts::cursor(&app), Some(vectorcraft_tools::Cursor::Script));
    app.session.prefs.use_precise_cursors = true;
    assert_eq!(crate::scripts::cursor(&app), Some(vectorcraft_tools::Cursor::Crosshair), "Use Precise Cursors");
    app.scripts.deselect_tool();
    assert_eq!(crate::scripts::cursor(&app), None, "the app's tool has its own");
}
