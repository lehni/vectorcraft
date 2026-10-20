//! Scriptographer scripts in the app: running ones keep running through what the user does.

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

/// A pointer event on the canvas, sent as at the end of a frame.
fn pointer(app: &mut VectorcraftApp, kind: vectorcraft_tools::PointerKind, x: f64) -> bool {
    let ev = vectorcraft_tools::PointerEvent { kind, pos: vectorcraft_geom::Point::new(x, 100.0), mods: Default::default(), pressure: 1.0 };
    let taken = crate::scripts::pointer(app, &ev);
    crate::scripts::flush_pointer(app, app.last_time, 0.0);
    taken
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
            let path = std::env::temp_dir().join(format!(
                "scriptographer-{}-{}.js",
                std::process::id(),
                n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
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
    assert!(app.scripts.tool().is_some(), "the tool script stays assigned to the Scriptographer Tool");
    // The Scriptographer Tool's button makes it the active tool again.
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

/// There is one Scriptographer Tool button: running a tool script replaces the one it stood for.
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

/// The Scriptographer Tool is global: it draws in whichever document is active, and stays when a
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
    assert_eq!(app.scripts.tool(), Some(tool.0.as_path()), "it stays the Scriptographer Tool's");
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

/// While the Scriptographer Tool is the active tool, the canvas shows its pencil.
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

/// A tool script with key handlers gets the plain keys typed on the canvas; keys with Cmd stay the
/// app's shortcuts.
#[test]
fn a_tool_script_gets_the_plain_keys() {
    let (mut app, _) = app();
    let tool = tempfile_path::Path::new("function onKeyDown(event) { console.log(event.key, event.character) }");
    app.run("ui.runScript", json!({ "path": tool.0.to_string_lossy() })).unwrap();
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let events = vec![
        key(egui::Key::A, egui::Modifiers::SHIFT),
        key(egui::Key::V, egui::Modifiers::COMMAND),
        key(egui::Key::ArrowLeft, egui::Modifiers::NONE),
    ];
    let ctx = egui::Context::default();
    let mut left = Vec::new();
    let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        crate::scripts::keys(&mut app, ui.ctx());
        left = ui.input(|i| i.events.clone());
    });
    out.textures_delta.clear();
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log, ["a A", "left "]);
    assert_eq!(left, [key(egui::Key::V, egui::Modifiers::COMMAND)], "Cmd+V stays the app's");
}

/// A script that asks with `Dialog.prompt` waits for the answer while the app goes on, then works
/// with the values given.
#[test]
fn a_script_waits_for_its_prompt() {
    let (mut app, id) = app();
    let path = tempfile_path::Path::new(
        "var values = Dialog.prompt('Move', { dx: { type: 'number', label: 'Distance', value: 10 } })\n\
         if (values) project.activeLayer.firstChild.position.x += values.dx",
    );
    let result = app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert_eq!(result["started"], true, "the app goes on while the script waits");
    let ctx = egui::Context::default();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    let dialog = app.ui.dialog.as_mut().expect("the prompt shows");
    assert_eq!(dialog.kind, crate::dialogs::script::KIND);
    assert_eq!(dialog.fields["values"], json!({"dx": 10}));
    dialog.fields.insert("values".into(), json!({"dx": 50}));
    crate::dialogs::confirm(&mut app).unwrap();
    crate::scripts::frame(&mut app, &ctx, 2.0);
    assert_eq!(left(&app, id), 60.0, "the script moved it by the distance given");
    assert!(!app.scripts.is_running());
}

/// A prompt closed with Cancel returns null; a dialog waits while another dialog is open.
#[test]
fn a_cancelled_prompt_and_one_that_waits_its_turn() {
    let (mut app, id) = app();
    crate::dialogs::confirm::tell(&mut app, "Hello", "");
    let path = tempfile_path::Path::new("if (Dialog.prompt('Move', { dx: { value: 10 } })) project.activeLayer.firstChild.position.x += 100");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(app.ui.dialog.as_ref().unwrap().kind, crate::dialogs::confirm::MESSAGE, "the open dialog stays");
    crate::dialogs::cancel(&mut app);
    crate::scripts::frame(&mut app, &ctx, 2.0);
    assert_eq!(app.ui.dialog.as_ref().unwrap().kind, crate::dialogs::script::KIND, "then the prompt shows");
    crate::dialogs::cancel(&mut app);
    crate::scripts::frame(&mut app, &ctx, 3.0);
    assert_eq!(left(&app, id), 10.0, "cancelled: nothing moved");
    assert!(!app.scripts.is_running());
    assert!(app.scripts.log().iter().all(|l| !l.error));
}

/// A frame of the scripts' palettes: the texts they show, where.
fn palettes_frame(app: &mut VectorcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let mut out =
        ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| crate::scripts::palettes(app, ui.ctx()));
    out.textures_delta.clear();
    crate::tests_removeanchors::shapes_text(&out.shapes.iter().map(|c| c.shape.clone()).collect::<Vec<_>>())
}

/// A palette stays open while the script runs: its button runs the script's handler, and what the
/// handler sets in `values` shows in the palette.
#[test]
fn a_palette_runs_its_buttons_and_shows_the_values() {
    let (mut app, id) = app();
    let path = tempfile_path::Path::new(
        "var values = { size: 10 }\n\
         new Palette('Circles', {\n\
             size: { type: 'number', label: 'Size' },\n\
             move: { type: 'button', value: 'Move', onClick: function() { project.activeLayer.firstChild.position.x += values.size; values.size = 25 } }\n\
         }, values)",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::scripts::frame(&mut app, &ctx, 0.5);
    assert!(!app.scripts.is_running(), "no timers");
    assert!(app.scripts.running_paths().contains(&path.0), "but it stays, with its palette");
    // An area shows from its second frame, once it knows its size.
    palettes_frame(&mut app, &ctx, vec![]);
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    let at = crate::tests_removeanchors::at(&texts, "Move");
    for s in ["Circles", "Size:", "10"] {
        assert!(texts.iter().any(|(t, _)| t == s), "{s} in {texts:?}");
    }
    let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at), press(true)]);
    palettes_frame(&mut app, &ctx, vec![press(false)]);
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert_eq!(left(&app, id), 20.0, "the button moved it by the size");
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    assert!(texts.iter().any(|(t, _)| t == "25"), "the new size shows: {texts:?}");
}

/// A palette dragged by its title bar opens there again the next time the script runs.
#[test]
fn a_palette_opens_where_the_user_put_it() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("new Palette('Circles', { size: { value: 1 } })");
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let title = |app: &mut VectorcraftApp| {
        palettes_frame(app, &ctx, vec![]);
        crate::tests_removeanchors::at(&palettes_frame(app, &ctx, vec![]), "Circles")
    };
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let from = title(&mut app);
    let to = from + egui::vec2(100.0, 50.0);
    let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(from), press(from, true)]);
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(from + egui::vec2(50.0, 25.0))]);
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(to)]);
    palettes_frame(&mut app, &ctx, vec![press(to, false)]);
    assert!(app.prefs_changed, "the place is kept");
    app.run("ui.stopScripts", json!({})).unwrap();
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let again = title(&mut app);
    assert!((again - to).length() < 1.0, "{again:?} where it was put, {to:?}");
}

/// The scripts folder is listed again when the app comes to the front: a script added elsewhere
/// meanwhile shows.
#[test]
fn the_scripts_folder_is_listed_again_when_the_app_comes_to_the_front() {
    let (mut app, _) = app();
    let folder = TempFolder::new("listed");
    std::fs::write(folder.0.join("a.js"), "").unwrap();
    let folders = [folder.script_folder("Mine")];
    let ctx = egui::Context::default();
    let focus = |app: &mut VectorcraftApp, focused| {
        let mut out = ctx.run_ui(egui::RawInput { focused, ..Default::default() }, |ui| crate::scripts::frame(app, ui.ctx(), 0.0));
        out.textures_delta.clear();
    };
    let scripts = |app: &mut VectorcraftApp| match &app.scripts.tree(&folders, false)[0].kind {
        crate::scripts::EntryKind::Folder(children) => children.len(),
        _ => 0,
    };
    focus(&mut app, true);
    assert_eq!(scripts(&mut app), 1);
    std::fs::write(folder.0.join("b.js"), "").unwrap();
    assert_eq!(scripts(&mut app), 1, "listed once");
    focus(&mut app, false);
    focus(&mut app, true);
    assert_eq!(scripts(&mut app), 2, "back in front, listed again");
}

/// A folder for a test, removed when dropped.
struct TempFolder(std::path::PathBuf);

impl TempFolder {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("scriptographer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn script_folder(&self, name: &str) -> crate::state::ScriptFolder {
        crate::state::ScriptFolder { name: name.into(), path: self.0.to_string_lossy().into_owned(), visible: true }
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The panel lists the shown script folders, each a root under its name, in their order.
#[test]
fn script_folders_are_the_roots_of_the_tree() {
    let (mut app, _) = app();
    let (a, b, hidden) = (TempFolder::new("roots-a"), TempFolder::new("roots-b"), TempFolder::new("roots-hidden"));
    let mut folders = vec![b.script_folder("Second"), a.script_folder("First"), hidden.script_folder("Hidden")];
    folders[2].visible = false;
    let names: Vec<String> = app.scripts.tree(&folders, false).iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["Second", "First"]);
}

/// New Script asks for the name in a save dialog, in the selected folder, suggesting the first free
/// Untitled N.js; New Folder asks in a dialog. Both select what they made.
#[test]
fn new_scripts_and_folders_are_named_by_the_user() {
    let (mut app, _) = app();
    let folder = TempFolder::new("new");
    app.ui.scriptographer_folders = vec![folder.script_folder("Mine")];
    let asked = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (seen, into) = (asked.clone(), folder.0.clone());
    app.services.pick_save = Some(Box::new(move |pick: &crate::FilePick| {
        *seen.borrow_mut() = Some((pick.name.clone(), pick.folder.clone()));
        Some(into.join("Spiral.js").to_string_lossy().into_owned())
    }));
    let r = app.run("ui.newScript", json!({})).unwrap();
    assert_eq!(*asked.borrow(), Some(("Untitled 1.js".into(), Some(folder.0.to_string_lossy().into_owned()))));
    assert_eq!(r["path"], folder.0.join("Spiral.js").to_string_lossy().as_ref());
    assert!(folder.0.join("Spiral.js").is_file());
    assert_eq!(app.scripts.take_created(), Some(folder.0.join("Spiral.js")), "the panel selects it");
    let r = app.run("ui.newScriptFolder", json!({})).unwrap();
    assert_eq!(r["pending"], crate::dialogs::script_folders::NEW_FOLDER);
    let dialog = app.ui.dialog.as_mut().unwrap();
    assert_eq!(dialog.fields["name"], "Untitled 1");
    dialog.fields.insert("name".into(), json!("Tools"));
    crate::dialogs::confirm(&mut app).unwrap();
    assert!(folder.0.join("Tools").is_dir());
    assert!(app.ui.dialog.is_none());
    assert!(app.run("ui.newScriptFolder", json!({ "name": "Tools" })).is_err(), "it exists");
}

/// Script Folders edits the list: OK keeps it, and the panel lists it.
#[test]
fn script_folders_dialog_keeps_the_list() {
    let (mut app, _) = app();
    let (a, b) = (TempFolder::new("dialog-a"), TempFolder::new("dialog-b"));
    app.ui.scriptographer_folders = vec![a.script_folder("A"), b.script_folder("B")];
    app.run("ui.scriptFolders", json!({})).unwrap();
    let dialog = app.ui.dialog.as_mut().unwrap();
    assert_eq!(dialog.kind, crate::dialogs::script_folders::KIND);
    let mut renamed = b.script_folder("Bee");
    renamed.visible = false;
    dialog.fields.insert("folders".into(), json!([renamed, a.script_folder("A")]));
    crate::dialogs::confirm(&mut app).unwrap();
    assert_eq!(app.ui.scriptographer_folders, [renamed, a.script_folder("A")]);
    assert!(app.prefs_changed);
}

/// New items a script makes take the style new art takes in the app, as in Scriptographer.
#[test]
fn new_items_take_the_apps_paint() {
    let (mut app, _) = app();
    app.session.paint.fill = vectorcraft_color::Paint::solid(vectorcraft_color::Color::rgb(1.0, 0.0, 0.0));
    app.session.paint.stroke_width = 3.0;
    let path = tempfile_path::Path::new("new Path.Circle({ center: [50, 50], radius: 10 })");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let layer = &app.session.doc().unwrap().doc.layers[0];
    let circle = layer.children().unwrap().last().unwrap();
    assert_eq!(circle.appearance.fill_paint(), vectorcraft_color::Paint::solid(vectorcraft_color::Color::rgb(1.0, 0.0, 0.0)));
    assert_eq!(circle.appearance.stroke().map(|s| s.width), Some(3.0));
}

/// As in Scriptographer, running a script stops the ones that run, unless they set
/// `script.keepAlive`, and `script.file` is the script's file.
#[test]
fn a_script_stops_the_others_unless_they_keep_alive() {
    let (mut app, _) = app();
    let kept = tempfile_path::Path::new("script.keepAlive = true\nfunction onFrame() {}");
    let other = tempfile_path::Path::new("function onFrame() {}");
    let next = tempfile_path::Path::new("console.log(script.file == null, script.directory != null)");
    for path in [&kept, &other, &next] {
        app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    }
    // The last one has run once its view updated.
    crate::scripts::frame(&mut app, &egui::Context::default(), 0.5);
    assert_eq!(app.scripts.running_paths(), std::collections::BTreeSet::from([kept.0.clone()]));
    assert!(app.scripts.log().iter().any(|l| l.text == "false true"), "script.file and script.directory");
}

/// The `__init__` scripts of the scripts folders run by themselves once a document is open, and
/// don't show in the panel.
#[test]
fn init_scripts_run_by_themselves() {
    let (mut app, _) = app();
    let folder = TempFolder::new("init");
    std::fs::create_dir_all(folder.0.join("Tools")).unwrap();
    std::fs::write(folder.0.join("Tools").join("__init__.js"), "script.keepAlive = true\nconsole.log('ready')").unwrap();
    std::fs::write(folder.0.join("a.js"), "").unwrap();
    app.ui.scriptographer_folders = vec![folder.script_folder("Mine")];
    crate::scripts::frame(&mut app, &egui::Context::default(), 0.0);
    assert!(app.scripts.log().iter().any(|l| l.text == "ready"));
    crate::scripts::frame(&mut app, &egui::Context::default(), 0.1);
    assert_eq!(app.scripts.log().iter().filter(|l| l.text == "ready").count(), 1, "once");
    let crate::scripts::EntryKind::Folder(children) = &app.scripts.tree(&app.ui.scriptographer_folders.clone(), false)[0].kind else { panic!() };
    let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Tools", "a.js"], "and the Tools folder shows no __init__");
}

/// `script.preferences` keeps what a script puts there for its next run, as in Scriptographer.
#[test]
fn script_preferences_last_from_one_run_to_the_next() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("console.log(script.preferences.count || 0)\nscript.preferences.count = (script.preferences.count || 0) + 1");
    for _ in 0..2 {
        app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    }
    let counts: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(counts, ["0", "1"]);
    assert_eq!(app.ui.scriptographer_preferences[path.0.to_string_lossy().as_ref()], json!({"count": 2}));
    assert!(app.prefs_changed, "kept with the app's settings");
}

/// A tool script hears when the user picks its tool and another one, as Paper.js's onActivate and
/// onDeactivate; `ui.scriptTool` picks it.
#[test]
fn a_tool_script_hears_when_its_tool_is_picked() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new(
        "function onActivate() { console.log('activate') }\nfunction onDeactivate() { console.log('deactivate') }\nfunction onMouseDown() {}",
    );
    let ctx = egui::Context::default();
    let log = |app: &VectorcraftApp| app.scripts.log().iter().map(|l| l.text.clone()).collect::<Vec<_>>();
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    crate::scripts::frame(&mut app, &ctx, 0.1);
    assert_eq!(log(&app), ["activate"], "assigned, it is the active tool");
    app.select_tool("rectangle");
    crate::scripts::frame(&mut app, &ctx, 0.2);
    assert_eq!(log(&app), ["activate", "deactivate"]);
    assert_eq!(app.run("ui.scriptTool", json!({})).unwrap()["active"], true);
    crate::scripts::frame(&mut app, &ctx, 0.3);
    assert_eq!(log(&app), ["activate", "deactivate", "activate"]);
}

/// A script's app handlers run when the app comes to the front or goes behind another, and when
/// the user stops the running scripts.
#[test]
fn app_handlers_run_on_app_events() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new(
        "script.keepAlive = true\n\
         function onAppActivate() { console.log('front') }\n\
         function onAppDeactivate() { console.log('behind') }\n\
         function onStop() { console.log('stop') }\n\
         function onFrame() {}",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    let frame = |app: &mut VectorcraftApp, focused| {
        let mut out = ctx.run_ui(egui::RawInput { focused, ..Default::default() }, |ui| crate::scripts::frame(app, ui.ctx(), 0.0));
        out.textures_delta.clear();
    };
    frame(&mut app, true);
    frame(&mut app, false);
    app.run("ui.stopScripts", json!({})).unwrap();
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log, ["front", "behind", "stop"]);
}

/// Show in Folder shows a script or a folder in the system's file manager.
#[test]
fn scripts_and_folders_show_in_their_folder() {
    let (mut app, _) = app();
    let shown = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = shown.clone();
    app.services.reveal = Some(Box::new(move |path: &str| {
        seen.borrow_mut().push(path.to_owned());
        Ok(())
    }));
    for path in ["/scripts/Tools", "/scripts/Tools/Clouds.js"] {
        app.run("ui.revealScript", json!({ "path": path })).unwrap();
    }
    assert_eq!(*shown.borrow(), ["/scripts/Tools", "/scripts/Tools/Clouds.js"]);
}

/// A palette closed with its × opens again when its script runs again, and a new palette asks for
/// the frame it shows in (an area shows from its second one).
#[test]
fn a_closed_palette_opens_again() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("new Palette('Clouds', { size: { value: 1 } })\nfunction onMouseDown() {}");
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let shown = |app: &mut VectorcraftApp| {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| crate::scripts::palettes(app, ui.ctx()));
        out.textures_delta.clear();
        out.viewport_output.values().any(|v| v.repaint_delay.is_zero())
    };
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert!(shown(&mut app), "the next frame is asked for");
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    let title = crate::tests_removeanchors::at(&texts, "Clouds");
    let layer = ctx.layer_id_at(title).unwrap();
    let area = ctx.memory(|m| m.area_rect(layer.id)).unwrap();
    let close = egui::pos2(area.right() - 13.0, title.y);
    let press = |pressed| egui::Event::PointerButton { pos: close, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(close), press(true)]);
    palettes_frame(&mut app, &ctx, vec![press(false)]);
    crate::scripts::frame(&mut app, &ctx, 0.5);
    assert!(!palettes_frame(&mut app, &ctx, vec![]).iter().any(|(t, _)| t == "Clouds"), "closed");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    assert!(shown(&mut app), "the next frame is asked for again");
    assert!(palettes_frame(&mut app, &ctx, vec![]).iter().any(|(t, _)| t == "Clouds"), "open again");
}

/// The console's input is a multi-line field: Return goes to a new line and runs the line it ends,
/// and the text stays.
#[test]
fn the_console_runs_the_line_return_ends() {
    let (mut app, _) = app();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let frame = |app: &mut VectorcraftApp, events: Vec<egui::Event>| {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| {
            crate::panels::scriptographer_console::show(app, ui);
        });
        out.textures_delta.clear();
    };
    let field = egui::pos2(40.0, 15.0);
    let press = |pressed| egui::Event::PointerButton { pos: field, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, vec![]);
    frame(&mut app, vec![egui::Event::PointerMoved(field), press(true)]);
    frame(&mut app, vec![press(false)]);
    let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    frame(&mut app, vec![egui::Event::Text("console.log(6 * 7)".into()), enter]);
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log, ["> console.log(6 * 7)", "42"]);
}

/// A tool script gets every mouse move the system reports, as in a browser, not one per frame: its
/// drags follow the mouse in even steps. They get times spread over the frame, by the script's
/// clock.
#[test]
fn a_tool_script_gets_every_mouse_move() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("function onMouseDrag(event) { console.log('drag', Math.round(event.time * 1000)) }");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    let mut time = 0.97;
    let mut frame = |app: &mut VectorcraftApp, events: Vec<egui::Event>| {
        time += 0.03;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(time), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| crate::canvas::show(app, ui));
        out.textures_delta.clear();
    };
    let at = |x: f32| egui::pos2(x, 300.0);
    let press = |x, pressed| egui::Event::PointerButton { pos: at(x), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, vec![]);
    frame(&mut app, vec![egui::Event::PointerMoved(at(300.0)), press(300.0, true)]);
    frame(&mut app, vec![egui::Event::PointerMoved(at(310.0)), egui::Event::PointerMoved(at(320.0)), egui::Event::PointerMoved(at(330.0))]);
    let drags: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).filter(|l| l.starts_with("drag")).collect();
    assert_eq!(drags, ["drag 1040", "drag 1050", "drag 1060"], "one drag per move, 10 ms apart");
}

/// Where the platform knows the system's times of the pointer events, a tool script gets them:
/// the newest ones for its events, as the system may have seen more; else the frame's time.
#[test]
fn a_tool_script_gets_the_systems_times() {
    use crate::canvas::PointerTimes;
    use vectorcraft_tools::PointerKind::{Down, Drag};
    let (mut app, _) = app();
    let path = tempfile_path::Path::new(
        "function onMouseDown(event) { console.log('down', Math.round(event.time * 1000)) }\n\
         function onMouseDrag(event) { console.log('drag', Math.round(event.time * 1000)) }",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    app.pointer_times = PointerTimes { downs: vec![1.0], ..Default::default() };
    pointer(&mut app, Down, 0.0);
    app.pointer_times = PointerTimes { moves: vec![0.9, 1.012], ..Default::default() };
    pointer(&mut app, Drag, 10.0);
    app.pointer_times = PointerTimes::default();
    pointer(&mut app, Drag, 20.0);
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log, ["down 1000", "drag 1012", "drag 0"]);
}

/// A script's `project.activeArtboard` is the artboard active in its document's view.
#[test]
fn a_script_knows_the_active_artboard() {
    let (mut app, _) = app();
    app.run("artboard.new", json!({"x": 500, "y": 0, "width": 200, "height": 100})).unwrap();
    app.view_mut().unwrap().artboard = 1;
    let path = tempfile_path::Path::new("console.log(project.activeArtboard.name, project.artboards.length)");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log, ["Artboard 2 2"]);
}

/// The console shows when a script prints, as Scriptographer's did.
#[test]
fn the_console_shows_when_a_script_prints() {
    let (mut app, _) = app();
    let shown = |app: &VectorcraftApp| crate::menus::checked(app, "window.panel", &json!({ "panel": crate::panels::scriptographer_console::ID }));
    assert_ne!(shown(&app), Some(true));
    let quiet = tempfile_path::Path::new("var a = 1");
    app.run("ui.runScript", json!({ "path": quiet.0.to_string_lossy() })).unwrap();
    assert_ne!(shown(&app), Some(true), "a script that prints nothing leaves it");
    let loud = tempfile_path::Path::new("print('hello')");
    app.run("ui.runScript", json!({ "path": loud.0.to_string_lossy() })).unwrap();
    assert_eq!(shown(&app), Some(true));
}

/// `Dialog.openFile` shows the app's file picker, starting where the script says, with its file
/// types; the path picked answers it, and a picker closed without one answers null.
#[test]
fn a_script_picks_a_file() {
    let (mut app, _) = app();
    let started = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (seen, mut answers) = (started.clone(), vec![None, Some("/tmp/picked.svg".to_owned())]);
    app.services.pick_open = Some(Box::new(move |pick: &crate::FilePick| {
        seen.borrow_mut().push((pick.folder.clone(), pick.name.clone(), pick.filters.clone()));
        answers.pop().flatten()
    }));
    let path = tempfile_path::Path::new(
        "print((await Dialog.openFile({ types: { SVG: 'svg', 'All Files': '*' }, path: '/tmp/start.svg' })).path)\nprint(await Dialog.openFile())",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    for frame in 1..6 {
        crate::scripts::frame(&mut app, &ctx, f64::from(frame));
    }
    let types = crate::FilePick::filters([("SVG", &["svg"][..]), ("All Files", &[])]);
    assert_eq!(*started.borrow(), [(Some("/tmp".to_owned()), "start.svg".to_owned(), types), (None, String::new(), Vec::new())]);
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).filter(|l| !l.ends_with("stopped")).collect();
    assert_eq!(log, ["/tmp/picked.svg", "null"]);
}

/// A number with `units` shows its points in that unit, or with the unit's sign; with `steppers`
/// it has arrows.
#[test]
fn palette_numbers_show_their_units() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new(
        "new Palette('Units', {\n\
             width: { type: 'number', label: 'Width', units: 'millimeter', value: 72 },\n\
             angle: { type: 'number', label: 'Angle', units: 'degree', value: 45, steppers: true },\n\
             share: { type: 'number', label: 'Share', units: 'percent', value: 50 }\n\
         })",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::scripts::frame(&mut app, &ctx, 0.5);
    palettes_frame(&mut app, &ctx, vec![]);
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    for s in ["25.4 mm", "45°", "50%"] {
        assert!(texts.iter().any(|(t, _)| t == s), "{s} in {texts:?}");
    }
}

/// A palette's menu entries are in its menu, not in its body: picking one runs its `onSelect`.
#[test]
fn a_palettes_menu_runs_its_entries() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new(
        "new Palette('Tools', {\n\
             size: { type: 'number', label: 'Size', value: 1 },\n\
             about: { label: 'About Tools', onSelect: function() { print('about') } }\n\
         })",
    );
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::scripts::frame(&mut app, &ctx, 0.5);
    palettes_frame(&mut app, &ctx, vec![]);
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    assert!(!texts.iter().any(|(t, _)| t.contains("About Tools")), "not in the body: {texts:?}");
    // The menu button sits left of the × at the title bar's right end.
    let area = ctx.memory(|m| m.layer_ids().filter(|l| l.order == egui::Order::Middle).find_map(|l| m.area_rect(l.id))).unwrap();
    let menu = egui::pos2(area.right() - 33.0, area.top() + crate::floating::STRIP / 2.0);
    let press = |at, pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(menu), press(menu, true)]);
    palettes_frame(&mut app, &ctx, vec![press(menu, false)]);
    let texts = palettes_frame(&mut app, &ctx, vec![]);
    let entry = texts.iter().find(|(t, _)| t.contains("About Tools")).map(|(_, r)| r.center()).expect("the menu shows the entry");
    palettes_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(entry), press(entry, true)]);
    palettes_frame(&mut app, &ctx, vec![press(entry, false)]);
    crate::scripts::frame(&mut app, &ctx, 1.0);
    assert!(app.scripts.log().iter().any(|l| l.text == "about"), "{:?}", app.scripts.log().iter().map(|l| &l.text).collect::<Vec<_>>());
}

/// Double-clicking the Scriptographer Tool's button calls its script's `onEditOptions()`, as
/// Scriptographer's tool buttons did.
#[test]
fn double_clicking_the_tool_button_edits_its_options() {
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("function onMouseDown(event) {}\nfunction onEditOptions() { print('options') }");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    let ctx = egui::Context::default();
    crate::scripts::frame(&mut app, &ctx, 0.5);
    let mut time = 1.0;
    let mut frame = |app: &mut VectorcraftApp, events: Vec<egui::Event>| {
        time += 0.05;
        let input = egui::RawInput { time: Some(time), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| crate::scripts::tool_button(app, ui, 20.0));
        out.textures_delta.clear();
    };
    let at = egui::pos2(20.0, 15.0);
    let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut app, vec![egui::Event::PointerMoved(at)]);
    for _ in 0..2 {
        frame(&mut app, vec![press(true)]);
        frame(&mut app, vec![press(false)]);
    }
    let log: Vec<&str> = app.scripts.log().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(log.iter().filter(|l| **l == "options").count(), 1, "{log:?}");
}

/// What a tool script marks `removeOnDrag()` goes again at its next drag, from the document too,
/// as Scriptographer's tools used it for a preview that follows the mouse.
#[test]
fn a_tool_scripts_preview_goes_at_the_next_drag() {
    use vectorcraft_tools::PointerKind::{Down, Drag, Up};
    let (mut app, _) = app();
    let path = tempfile_path::Path::new("function onMouseDrag(event) { new Path.Line(event.downPoint, event.point).removeOnDrag() }");
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    for (kind, x) in [(Down, 0.0), (Drag, 15.0), (Drag, 30.0), (Drag, 45.0), (Up, 45.0)] {
        pointer(&mut app, kind, x);
    }
    assert_eq!(children(&app), 2, "the rectangle and the last line");
}

/// A click into the Scriptographer panel's tree gives it the arrow keys, Return and ⌘E, as
/// Scriptographer's: they walk the tree, open folders and run scripts instead of nudging the
/// selection; Escape gives them back.
#[test]
fn the_panel_walks_its_tree_with_the_keys() {
    use crate::panels::scriptographer::ID;
    use egui::{Key, Modifiers};
    let (mut app, id) = app();
    app.run("select.set", json!({ "ids": [id] })).unwrap();
    let folder = TempFolder::new("keys");
    for name in ["a.js", "b.js"] {
        std::fs::write(folder.0.join(name), format!("print('{name}')")).unwrap();
    }
    app.ui.scriptographer_folders = vec![folder.script_folder("Scripts")];
    app.run("window.panel", json!({ "panel": ID })).unwrap();
    let ctx = egui::Context::default();
    let state = |key: &str| egui::Id::new(("panel-state", key));
    ctx.data_mut(|d| {
        d.insert_temp(state("scriptographer-open"), std::collections::BTreeSet::from([folder.0.clone()]));
        d.insert_temp(state("scriptographer-selected"), Some(folder.0.join("a.js")));
    });
    app.key_panel = Some(ID);
    let key = |app: &mut VectorcraftApp, key: Key, modifiers: Modifiers| {
        let events = vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }];
        let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| crate::shortcuts::handle(app, ui.ctx()));
        out.textures_delta.clear();
        ctx.data(|d| d.get_temp::<Option<std::path::PathBuf>>(state("scriptographer-selected"))).flatten()
    };
    let before = left(&app, id);
    assert_eq!(key(&mut app, Key::ArrowDown, Modifiers::NONE), Some(folder.0.join("b.js")));
    assert_eq!(left(&app, id), before, "the arrows don't nudge the selection meanwhile");
    assert_eq!(key(&mut app, Key::ArrowLeft, Modifiers::NONE), Some(folder.0.clone()), "a script's folder");
    key(&mut app, Key::ArrowLeft, Modifiers::NONE);
    assert!(ctx.data(|d| d.get_temp::<std::collections::BTreeSet<std::path::PathBuf>>(state("scriptographer-open"))).unwrap().is_empty(), "closed");
    key(&mut app, Key::Enter, Modifiers::NONE);
    key(&mut app, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(key(&mut app, Key::E, Modifiers::COMMAND), Some(folder.0.join("a.js")), "Return opened it again");
    assert!(app.scripts.log().iter().any(|l| l.text == "a.js"), "⌘E ran it");
    key(&mut app, Key::Escape, Modifiers::NONE);
    assert_eq!(app.key_panel, None);
    key(&mut app, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(left(&app, id), before + 1.0, "the arrows nudge again");
}

/// A click on a row of the Scriptographer panel gives it the keys; a click elsewhere gives them back.
#[test]
fn a_click_into_the_panel_gives_it_the_keys() {
    let (mut app, _) = app();
    let folder = TempFolder::new("focus");
    app.ui.scriptographer_folders = vec![folder.script_folder("Scripts")];
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let frame = |app: &mut VectorcraftApp, events: Vec<egui::Event>| {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0));
        let mut out =
            ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| crate::panels::scriptographer::show(app, ui));
        out.textures_delta.clear();
        crate::tests_removeanchors::shapes_text(&out.shapes.iter().map(|c| c.shape.clone()).collect::<Vec<_>>())
    };
    let texts = frame(&mut app, vec![]);
    let row = crate::tests_removeanchors::at(&texts, "Scripts");
    let click = |at| {
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        [vec![egui::Event::PointerMoved(at), press(true)], vec![press(false)]]
    };
    for events in click(row) {
        frame(&mut app, events);
    }
    assert_eq!(app.key_panel, Some(crate::panels::scriptographer::ID));
    // The bottom bar, outside the tree.
    for events in click(egui::pos2(150.0, 395.0)) {
        frame(&mut app, events);
    }
    assert_eq!(app.key_panel, None);
}

/// A script reads and writes the system clipboard through the app, with no dialog: its text goes
/// out as a text field copies it.
#[test]
fn scripts_read_and_write_the_clipboard() {
    let (mut app, _) = app();
    app.services.clipboard_read = Some(Box::new(|| Some("from the clipboard".into())));
    let path = tempfile_path::Path::new(
        "navigator.clipboard.readText().then(text => {\n\
             new Path.Circle({ center: [0, 0], radius: 5, name: text })\n\
             return navigator.clipboard.writeText('from the script')\n\
         })",
    );
    let ctx = egui::Context::default();
    app.run("ui.runScript", json!({ "path": path.0.to_string_lossy() })).unwrap();
    for frame in 1..20 {
        crate::scripts::frame(&mut app, &ctx, f64::from(frame));
        if children(&app) == 2 && !app.scripts.is_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let named = app.session.doc().unwrap().doc.layers[0].children().unwrap().iter().any(|n| n.name.as_deref() == Some("from the clipboard"));
    assert!(named, "the script named a path for the clipboard's text: {}", app.ui.status);
    let copied = ctx.output(|o| o.commands.iter().find_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t.clone()) } else { None }));
    assert_eq!(copied.as_deref(), Some("from the script"));
}
