//! PaperCraft: PaperScript files (`.js`) run on the active document. A script sees the document
//! as a Paper.js project, and what it changes comes back as one undo step. A script with `onFrame`
//! or an interval keeps running, driven by the app's frames, until it stops or Stop Running
//! Scripts stops it; the whole run is one undo step while nothing else edits the document in
//! between. Before each frame the script's project catches up with edits made outside it.
//!
//! `ui.runScript {path?}` runs a script (without a path it asks for the file),
//! `ui.stopScripts` stops the running ones and `ui.evaluateScript {code}` runs a line in the
//! console's own project, which lives as long as the app. Everything scripts print and every error
//! goes to the log the PaperCraft Console panel shows; the last line also shows in the status bar.
//! The scripts folder (`UiState::papercraft_folder`) is what the PaperCraft panel lists. A
//! script's view is the first artboard, so `view.bounds` is its size.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use vectorcraft_doc::{Document, NodeId};
use vectorcraft_papercraft::{Engine, PlatformEngine, Runtime, Scope};

use crate::{FilePick, VectorcraftApp};

/// Script files.
const EXTS: &[&str] = &["js"];

/// How many log lines are kept.
const LOG_LINES: usize = 1000;

/// How far into the scripts folder the panel looks, and how many entries it lists at most.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 5000;

/// The scripts that keep running, the console, and what they printed.
#[derive(Default)]
pub struct Scripts {
    running: Vec<Running>,
    /// The console's runtime, made on its first line.
    console: Option<Running>,
    log: Vec<LogLine>,
    /// The scripts folder as last listed, with the folder it lists.
    tree: Option<(PathBuf, Vec<Entry>)>,
}

/// A line of script output.
pub struct LogLine {
    pub text: String,
    pub error: bool,
}

/// A folder or script in the scripts folder.
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
}

pub enum EntryKind {
    Folder(Vec<Entry>),
    /// A script; `tool` when it defines mouse or key handlers, which makes it a tool.
    Script {
        tool: bool,
    },
}

struct Running {
    /// The file name, for messages.
    name: String,
    /// The file it runs, if any (not for the console).
    path: Option<PathBuf>,
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
        !self.running.is_empty() || self.console.as_ref().is_some_and(|c| c.runtime.is_running())
    }

    /// The files of the running scripts.
    pub fn running_paths(&self) -> BTreeSet<PathBuf> {
        self.running.iter().filter_map(|s| s.path.clone()).collect()
    }

    pub fn log(&self) -> &[LogLine] {
        &self.log
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    fn push_log(&mut self, text: String, error: bool) {
        self.log.push(LogLine { text, error });
        if self.log.len() > LOG_LINES {
            self.log.drain(..self.log.len() - LOG_LINES);
        }
    }

    /// The scripts in `folder`, listed again when `reload` or when the folder changed.
    pub fn tree(&mut self, folder: &Path, reload: bool) -> &[Entry] {
        if reload || self.tree.as_ref().is_none_or(|(listed, _)| listed != folder) {
            let mut count = 0;
            self.tree = Some((folder.to_owned(), list(folder, 0, &mut count)));
        }
        self.tree.as_ref().map_or(&[], |(_, entries)| entries)
    }
}

/// The folders and scripts in `folder`, folders first, each sorted by name.
fn list(folder: &Path, depth: usize, count: &mut usize) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || *count >= MAX_ENTRIES {
                return None;
            }
            *count += 1;
            if path.is_dir() {
                let children = if depth < MAX_DEPTH { list(&path, depth + 1, count) } else { Vec::new() };
                Some(Entry { name, path, kind: EntryKind::Folder(children) })
            } else if path.extension().is_some_and(|e| EXTS.iter().any(|x| e.eq_ignore_ascii_case(x))) {
                let tool = std::fs::read_to_string(&path).is_ok_and(|source| is_tool(&source));
                Some(Entry { name, path, kind: EntryKind::Script { tool } })
            } else {
                None
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        let folder = |e: &Entry| !matches!(e.kind, EntryKind::Folder(_));
        (folder(a), a.name.to_lowercase()).cmp(&(folder(b), b.name.to_lowercase()))
    });
    entries
}

/// Whether a script is a tool: it defines mouse or key handlers, or uses `tool`, the test
/// PaperScript makes before giving a script its tool.
pub fn is_tool(source: &str) -> bool {
    const HANDLERS: [&str; 6] = ["onMouseDown", "onMouseUp", "onMouseDrag", "onMouseMove", "onKeyDown", "onKeyUp"];
    source.contains("tool.") || HANDLERS.iter().any(|h| source.contains(h))
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
    let name = Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
    let mut script = start(app, name, Some(PathBuf::from(&path)))?;
    let result = execute(app, &mut script, &source);
    let running = result.is_ok() && script.runtime.is_running();
    if running {
        app.scripts.running.push(script);
    }
    let changed = result?;
    Ok(json!({ "changed": changed, "running": running }))
}

/// `ui.evaluateScript {code}`: run `code` in the console's project, made on the first line.
pub(crate) fn evaluate(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let code = p.get("code").and_then(Value::as_str).ok_or("missing code")?.to_owned();
    let mut console = match app.scripts.console.take() {
        Some(console) => console,
        None => start(app, "Console".into(), None)?,
    };
    app.scripts.push_log(format!("> {code}"), false);
    let result = catch_up(app, &mut console).and_then(|_| execute(app, &mut console, &code));
    app.scripts.console = Some(console);
    Ok(json!({ "changed": result? }))
}

/// `ui.stopScripts`: stop every running script, and the console's `onFrame` and intervals.
pub(crate) fn stop(app: &mut VectorcraftApp) -> Result<Value, String> {
    let stopped = app.scripts.running.len();
    app.scripts.running.clear();
    app.scripts.console = None;
    Ok(json!({ "stopped": stopped }))
}

/// A runtime bound to the active document, for the script `name`.
fn start(app: &VectorcraftApp, name: String, path: Option<PathBuf>) -> Result<Running, String> {
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    let size = doc.artboards.first().map_or((800.0, 600.0), |a| (a.rect.width(), a.rect.height()));
    let mut runtime = Runtime::<PlatformEngine>::new().map_err(|e| e.to_string())?;
    let scope = runtime.create_scope(size.0, size.1).map_err(|e| e.to_string())?;
    runtime.bind(&scope, doc, selection).map_err(|e| e.to_string())?;
    Ok(Running { name, path, runtime, scope, started: app.last_time, last: None })
}

/// Runs `source` in the script's project and writes back what it changed, even when it threw, as
/// a browser keeps what a script did before an error. Returns whether the document changed.
fn execute(app: &mut VectorcraftApp, script: &mut Running, source: &str) -> Result<bool, String> {
    let selected = script.runtime.selected_ids(&script.scope).map_err(|e| e.to_string())?;
    let result = script.runtime.execute(&script.scope, source).map_err(|e| e.to_string());
    let changed = write_back(app, script, &selected);
    show_output(app, script);
    let result = result.and(changed);
    if let Err(e) = &result {
        report(app, script, e);
    }
    result
}

/// Drives the running scripts, once per frame: their timers and `onFrame` handlers run, and what
/// they changed goes back to the document.
pub(crate) fn frame(app: &mut VectorcraftApp, ctx: &egui::Context, now: f64) {
    if !app.scripts.is_running() {
        return;
    }
    let mut scripts = std::mem::take(&mut app.scripts.running);
    scripts.retain_mut(|script| tick(app, script, now));
    // Scripts started by a script's frame join the list.
    scripts.append(&mut app.scripts.running);
    app.scripts.running = scripts;
    if let Some(mut console) = app.scripts.console.take() {
        if console.runtime.is_running() {
            tick(app, &mut console, now);
        }
        app.scripts.console = Some(console);
    }
    if app.scripts.is_running() {
        ctx.request_repaint();
    }
}

/// One frame of a running script. Returns whether it keeps running.
fn tick(app: &mut VectorcraftApp, script: &mut Running, now: f64) -> bool {
    let result = catch_up(app, script).and_then(|selected| {
        script.runtime.run_timers((now - script.started) * 1000.0).map_err(|e| e.to_string())?;
        write_back(app, script, &selected)
    });
    show_output(app, script);
    match result {
        Ok(_) => script.runtime.is_running(),
        Err(e) => {
            report(app, script, &e);
            false
        }
    }
}

/// An error from a script: in the log and the status bar.
fn report(app: &mut VectorcraftApp, script: &Running, error: &str) {
    let message = format!("{}: {error}", script.name);
    app.scripts.push_log(message.clone(), true);
    app.status(message);
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

/// Puts what the script printed in the log, and its last line in the status bar.
fn show_output(app: &mut VectorcraftApp, script: &mut Running) {
    let output = script.runtime.take_output();
    if let Some(last) = output.last() {
        app.status(last.clone());
    }
    for line in output {
        app.scripts.push_log(line, false);
    }
}

/// `ui.newScript`: a new, empty script in the scripts folder (or `folder`), named Untitled.js or
/// the first free Untitled N.js. Returns its path.
pub(crate) fn new_script(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let folder = match p.get("folder").and_then(Value::as_str) {
        Some(folder) => PathBuf::from(folder),
        None => app.ui.papercraft_folder.clone().map(PathBuf::from).ok_or("choose a scripts folder first")?,
    };
    let path = (1..1000)
        .map(|n| folder.join(if n == 1 { "Untitled.js".to_owned() } else { format!("Untitled {n}.js") }))
        .find(|path| !path.exists())
        .ok_or("too many untitled scripts")?;
    std::fs::write(&path, "").map_err(|e| format!("{}: {e}", path.display()))?;
    app.scripts.tree = None;
    Ok(json!({ "path": path.to_string_lossy() }))
}
