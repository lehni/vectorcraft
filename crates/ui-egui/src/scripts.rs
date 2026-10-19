//! Window › PaperCraft: PaperScript files (`.js`) run on the active document with PaperCraft. A
//! script sees the document as a Paper.js project, and what it changes comes back as one undo
//! step. A script with `onFrame` or an interval keeps running, driven by the app's frames, until
//! it stops or Stop Running Scripts stops it; the whole run is one undo step while nothing else
//! edits the document in between. Before each frame the script's project catches up with edits
//! made outside it.
//!
//! `ui.runScript {path?}` runs a script (without a path it asks for the file);
//! `ui.stopScripts` stops the running ones. The script's view is the first artboard, so
//! `view.bounds` is its size. The last line a script prints with `console.log` shows in the
//! status bar.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_doc::{Document, NodeId};
use vectorcraft_papercraft::{Engine, PlatformEngine, Runtime, Scope};

use crate::{FilePick, VectorcraftApp};

/// Script files.
const EXTS: &[&str] = &["js"];

/// The scripts that keep running.
#[derive(Default)]
pub struct Scripts(Vec<Running>);

struct Running {
    /// The file name, for messages.
    name: String,
    runtime: Runtime<PlatformEngine>,
    scope: Scope<<PlatformEngine as Engine>::Handle>,
    /// The app's clock when it started, in seconds.
    started: f64,
    /// The document after the script's last change: while it is still the document, the next
    /// change adds to the same undo step.
    last: Option<Arc<Document>>,
}

impl Scripts {
    pub fn is_running(&self) -> bool {
        !self.0.is_empty()
    }
}

/// `ui.runScript`: run the script at `path`, or the file the user picks.
pub(crate) fn run(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    if app.services.open_async.is_some() {
        return Err("Running scripts needs the desktop app".into());
    }
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => path.to_owned(),
        None => {
            let pick = FilePick { filters: vec![("Scripts", EXTS)], ..Default::default() };
            crate::picks::open(app, &pick).ok_or("cancelled")?
        }
    };
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    let size = doc.artboards.first().map_or((800.0, 600.0), |a| (a.rect.width(), a.rect.height()));

    let mut runtime = Runtime::<PlatformEngine>::new().map_err(|e| e.to_string())?;
    let scope = runtime.create_scope(size.0, size.1).map_err(|e| e.to_string())?;
    runtime.bind(&scope, doc, selection).map_err(|e| e.to_string())?;
    let selected = runtime.selected_ids(&scope).map_err(|e| e.to_string())?;
    let result = runtime.execute(&scope, &source);
    let mut script = Running { name, runtime, scope, started: app.last_time, last: None };
    // What the script changed before an error still goes back, as in a browser.
    let changed = write_back(app, &mut script, &selected)?;
    if let Err(e) = result {
        return Err(format!("{}: {e}", script.name));
    }
    show_output(app, &mut script);
    let running = script.runtime.is_running();
    if running {
        app.scripts.0.push(script);
    }
    Ok(json!({ "changed": changed, "running": running }))
}

/// `ui.stopScripts`: stop every running script.
pub(crate) fn stop(app: &mut VectorcraftApp) -> Result<Value, String> {
    let stopped = app.scripts.0.len();
    app.scripts.0.clear();
    Ok(json!({ "stopped": stopped }))
}

/// Drives the running scripts, once per frame: their timers and `onFrame` handlers run, and what
/// they changed goes back to the document.
pub(crate) fn frame(app: &mut VectorcraftApp, ctx: &egui::Context, now: f64) {
    if app.scripts.0.is_empty() {
        return;
    }
    let mut scripts = std::mem::take(&mut app.scripts.0);
    scripts.retain_mut(|script| {
        let result = catch_up(app, script).and_then(|selected| {
            script.runtime.run_timers((now - script.started) * 1000.0).map_err(|e| e.to_string())?;
            write_back(app, script, &selected)
        });
        show_output(app, script);
        match result {
            Ok(_) => script.runtime.is_running(),
            Err(e) => {
                app.status(format!("{}: {e}", script.name));
                false
            }
        }
    });
    // Scripts started by a script's frame join the list.
    scripts.append(&mut app.scripts.0);
    app.scripts.0 = scripts;
    if app.scripts.is_running() {
        ctx.request_repaint();
    }
}

/// Brings the script's project up to the document, after edits made outside it, and returns the
/// objects selected in it.
fn catch_up(app: &VectorcraftApp, script: &mut Running) -> Result<BTreeSet<NodeId>, String> {
    let state = app.session.doc().map_err(|e| e.to_string())?;
    script.runtime.update(&script.scope, state.doc.clone(), state.selection.clone()).map_err(|e| e.to_string())?;
    script.runtime.selected_ids(&script.scope).map_err(|e| e.to_string())
}

/// Applies what the script changed since the last call, adding to its undo step while nothing else
/// changed the document since, and the objects it selected or deselected since `selected`.
/// Returns whether the document changed.
fn write_back(app: &mut VectorcraftApp, script: &mut Running, selected: &BTreeSet<NodeId>) -> Result<bool, String> {
    let changed = write_patch(app, script)?;
    let now = script.runtime.selected_ids(&script.scope).map_err(|e| e.to_string())?;
    if &now != selected {
        let state = app.session.doc().map_err(|e| e.to_string())?;
        let mut ids: Vec<NodeId> = state.selection.objects.iter().copied().filter(|id| !selected.contains(id) || now.contains(id)).collect();
        ids.extend(now.iter().filter(|id| !selected.contains(id)));
        app.run("select.set", json!({ "ids": ids.iter().map(|id| id.0).collect::<Vec<_>>() }))?;
    }
    Ok(changed)
}

/// Applies the patch of what the script changed since the last call. Returns whether there was one.
fn write_patch(app: &mut VectorcraftApp, script: &mut Running) -> Result<bool, String> {
    let Some(patch) = script.runtime.take_patch(&script.scope).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let mut params = patch.params.clone();
    let current = app.session.doc().map_err(|e| e.to_string())?.doc.clone();
    if script.last.as_ref().is_some_and(|last| Arc::ptr_eq(last, &current))
        && let Some(params) = params.as_object_mut()
    {
        params.insert("extend".into(), json!(true));
    }
    let result = app.run("document.patch", params)?;
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    script.last = Some(doc.clone());
    script.runtime.apply_ids(patch, &result, doc, selection).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Shows the last line the script printed in the status bar.
fn show_output(app: &mut VectorcraftApp, script: &mut Running) {
    if let Some(line) = script.runtime.take_output().pop() {
        app.status(line);
    }
}
