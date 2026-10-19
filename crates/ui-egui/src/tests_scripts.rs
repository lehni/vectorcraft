//! PaperCraft scripts in the app: running ones keep running through what the user does.

use serde_json::json;
use vectorcraft_engine::Session;

use crate::VectorcraftApp;

/// An app with one rectangle, running a script that moves it 10 pt right on every frame.
fn running() -> (VectorcraftApp, u64, tempfile_path::Path) {
    let mut app = VectorcraftApp::new(Session::new(), Default::default());
    app.run("file.new", json!({})).unwrap();
    let id = app.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 30, "height": 20})).unwrap()["id"].as_u64().unwrap();
    let path = tempfile_path::Path::new("var item = project.activeLayer.firstChild\nfunction onFrame() { item.position += [10, 0] }");
    let result = app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert_eq!(result["running"], true);
    (app, id, path)
}

fn left(app: &VectorcraftApp, id: u64) -> f64 {
    app.session.doc().unwrap().doc.node(vectorcraft_doc::NodeId(id)).unwrap().geometric_bounds().unwrap().x0
}

/// A script file that is removed when dropped.
mod tempfile_path {
    pub struct Path(pub std::path::PathBuf);

    impl Path {
        pub fn new(source: &str) -> Self {
            let n = std::sync::atomic::AtomicUsize::new(0);
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

#[test]
fn a_running_script_waits_out_an_interaction() {
    let (mut app, id, _path) = running();
    let ctx = egui::Context::default();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(left(&app, id), 20.0, "a frame moves it");
    // The user starts dragging: frames wait, and the script keeps running.
    app.session.begin_interaction("Move").unwrap();
    crate::scripts::frame(&mut app, &ctx, 2.0);
    assert!(app.scripts.is_running(), "the script still runs: {}", app.ui.status);
    app.session.commit_interaction().unwrap();
    crate::scripts::frame(&mut app, &ctx, 3.0);
    assert_eq!(left(&app, id), 30.0, "and moves it again once the drag is over");
}
