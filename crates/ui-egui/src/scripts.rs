//! PaperCraft: PaperScript files (`.js`) run on the active document. A script sees the document
//! as a Paper.js project, and what it changes comes back as one undo step. A script with `onFrame`
//! or an interval keeps running, driven by the app's frames, until it stops or Stop Running
//! Scripts stops it; the whole run is one undo step while nothing else edits the document in
//! between. Each script runs on a thread of its own (`vectorcraft_papercraft::worker`): the app
//! sends it its work with the document as it is, waits a moment for what it changed, and goes on
//! when it takes longer. Edits made in the meantime win over the script's. A script runs on the document it started on: it waits while another one is active, and stops
//! when its document closes. One tool script at a time is the PaperCraft Tool, which works in
//! every document.
//!
//! `ui.runScript {path?}` runs a script (without a path it asks for the file),
//! `ui.stopScripts` stops the running ones and `ui.evaluateScript {code}` runs a line in the
//! console's own project, which lives as long as the app. Everything scripts print and every error
//! goes to the log the PaperCraft Console panel shows; the last line also shows in the status bar.
//! The scripts folder (`UiState::papercraft_folder`) is what the PaperCraft panel lists. A
//! script's view is the first artboard, so `view.bounds` is its size.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use vectorcraft_doc::{Document, NodeId};
use vectorcraft_papercraft::worker::{Reply, Request, Work, Worker};
use vectorcraft_papercraft::{Modifiers, MouseEvent};
use vectorcraft_tools::Cursor;

use crate::{FilePick, VectorcraftApp};

/// Script files.
const EXTS: &[&str] = &["js"];

/// How long the app waits for a script it runs before it goes on, and the script with it.
const RUN_WAIT: Duration = Duration::from_secs(2);

/// How long a frame waits for each of a script's replies: an animation's changes show in the
/// frame that asked for them, unless the script takes longer, when they show in a later one.
const FRAME_WAIT: Duration = Duration::from_millis(100);

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
    /// The tool script the PaperCraft Tool button stands for: the last one run, until it fails.
    tool: Option<ToolScript>,
    /// Whether the PaperCraft Tool is the active tool: the canvas then sends its pointer events to
    /// the tool script.
    tool_active: bool,
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

/// The tool script, global as the PaperCraft Tool is: as PaperScript fixes a script's `project`
/// when it runs, it runs once in each document, the first time the tool is used there.
#[derive(Clone)]
struct ToolScript {
    path: PathBuf,
    source: String,
    /// Whether its tool handles keys: the keys typed on the canvas are then its own.
    keys: bool,
}

/// A script that runs, on its thread.
struct Running {
    /// Names it for the dialog it waits on.
    id: u64,
    /// The file name, for messages.
    name: String,
    /// The file it runs, if any (not for the console).
    path: Option<PathBuf>,
    /// What it runs (empty for the console).
    source: String,
    worker: Worker,
    /// The document it runs on (`DocState::uid`). It waits while another document is active, and
    /// stops when its document closes.
    doc: u64,
    /// The app's clock when it started, in seconds.
    started: f64,
    /// The document after the script's last change: while it is still the document, the next
    /// change adds to the same undo step.
    last: Option<Arc<Document>>,
    /// Whether the script made a tool: it then stays loaded, to be the active tool in its document.
    tool: bool,
    /// Whether it has timers left (`onFrame`, intervals), as its last work said.
    timers: bool,
    /// The work it was sent and hasn't finished.
    busy: Option<Kind>,
    /// The work waiting to be sent: pointer events while it works.
    queued: VecDeque<Work>,
    /// The dialog it waits on (`Dialog.alert`, `confirm`, `prompt`).
    asking: Option<Asking>,
}

/// A script's dialog: its question, and whether it shows (it waits while another dialog is open).
struct Asking {
    question: Value,
    shown: bool,
}

/// What a script's work is, while it does it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Execute,
    Frame,
    Mouse,
    Key,
}

impl Scripts {
    /// Whether a script has timers left or is working (not waiting on a dialog).
    pub fn is_running(&self) -> bool {
        self.running.iter().chain(&self.console).any(|s| s.timers || (s.busy.is_some() && s.asking.is_none()))
    }

    /// The tool script the PaperCraft Tool button stands for.
    pub fn tool(&self) -> Option<&Path> {
        self.tool.as_ref().map(|t| t.path.as_path())
    }

    /// The tool script, while the PaperCraft Tool is the active tool.
    pub fn active_tool(&self) -> Option<&Path> {
        self.tool().filter(|_| self.tool_active)
    }

    /// Makes the PaperCraft Tool the active tool, when a tool script is assigned to it.
    pub fn select_tool(&mut self) {
        self.tool_active = self.tool.is_some();
    }

    /// Unloads the tool scripts the PaperCraft Tool no longer stands for, from every document.
    fn tidy(&mut self) {
        if self.tool.is_none() {
            self.tool_active = false;
        }
        let tool = self.tool.as_ref().map(|t| t.path.clone());
        self.running.retain(|s| !s.tool || s.path == tool);
    }

    /// Makes the app's own tools active again: the canvas no longer sends pointer events to the
    /// tool script, which stays assigned to the PaperCraft Tool.
    pub fn deselect_tool(&mut self) {
        self.tool_active = false;
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

/// `ui.runScript`: run the script at `path`, or the file the user picks. A script that is still
/// working after a moment goes on on its own: `started` instead of its result.
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
    let path = PathBuf::from(&path);
    // Run again, a script replaces the one that runs from the same file.
    app.scripts.running.retain(|s| s.path.as_deref() != Some(&path));
    let before = app.session.doc().map_err(|e| e.to_string())?.doc.clone();
    let mut script = start(app, file_name(&path), Some(path), source)?;
    let result = execute(app, &mut script, RUN_WAIT);
    let changed = !Arc::ptr_eq(&before, &app.session.doc().map_err(|e| e.to_string())?.doc);
    let (working, running, tool) = (script.busy.is_some(), script.timers, script.tool);
    if result? {
        app.scripts.running.push(script);
    }
    app.scripts.tidy();
    if working {
        return Ok(json!({ "started": true }));
    }
    Ok(json!({ "changed": changed, "running": running, "tool": tool }))
}

/// `ui.evaluateScript {code}`: run `code` in the console's project, made on the first line.
pub(crate) fn evaluate(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let code = p.get("code").and_then(Value::as_str).ok_or("missing code")?.to_owned();
    // The console's project is on the active document: a new one for another document.
    let active = app.session.doc().map_err(|e| e.to_string())?.uid;
    let mut console = match app.scripts.console.take().filter(|c| c.doc == active) {
        Some(console) => console,
        None => start(app, "Console".into(), None, String::new())?,
    };
    app.scripts.push_log(format!("> {code}"), false);
    let before = app.session.doc().map_err(|e| e.to_string())?.doc.clone();
    console.queued.push_back(Work::Execute(code));
    let result = execute(app, &mut console, RUN_WAIT);
    let changed = !Arc::ptr_eq(&before, &app.session.doc().map_err(|e| e.to_string())?.doc);
    app.scripts.console = Some(console);
    result?;
    Ok(json!({ "changed": changed }))
}

/// `ui.stopScripts`: stop every running script, and the console's `onFrame` and intervals.
pub(crate) fn stop(app: &mut VectorcraftApp) -> Result<Value, String> {
    let stopped = app.scripts.running.len();
    app.scripts.running.clear();
    app.scripts.console = None;
    app.scripts.tool = None;
    app.scripts.tool_active = false;
    if app.ui.dialog.as_ref().is_some_and(|d| d.kind == crate::dialogs::script::KIND) {
        app.ui.dialog = None;
    }
    Ok(json!({ "stopped": stopped }))
}

/// A script's thread, on the active document, to run `source` (queued, as the first work).
fn start(app: &VectorcraftApp, name: String, path: Option<PathBuf>, source: String) -> Result<Running, String> {
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    let size = doc.artboards.first().map_or((800.0, 600.0), |a| (a.rect.width(), a.rect.height()));
    let worker = Worker::spawn(size, doc, selection).map_err(|e| e.to_string())?;
    let queued = if source.is_empty() { VecDeque::new() } else { VecDeque::from([Work::Execute(source.clone())]) };
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(Running {
        id,
        name,
        path,
        source,
        worker,
        doc: state.uid,
        started: app.last_time,
        last: None,
        tool: false,
        timers: false,
        busy: None,
        queued,
        asking: None,
    })
}

/// Runs the script's queued work, waiting at most `wait` for each. Returns whether it keeps
/// running, or reports its error.
fn execute(app: &mut VectorcraftApp, script: &mut Running, wait: Duration) -> Result<bool, String> {
    pump(app, script, None, wait).inspect_err(|e| report(app, &script.name, e))
}

/// Drives the running scripts, once per frame: their queued work, timers and `onFrame` handlers
/// run, and what they changed goes back to the document.
pub(crate) fn frame(app: &mut VectorcraftApp, ctx: &egui::Context, now: f64) {
    if app.scripts.running.is_empty() && app.scripts.console.is_none() {
        return;
    }
    dialogs(app);
    let mut scripts = std::mem::take(&mut app.scripts.running);
    let active = app.session.active().map(|d| d.uid);
    scripts.retain_mut(|script| {
        if !is_open(app, script.doc) {
            // The tool script stays the PaperCraft Tool's, for the other documents.
            if !script.tool {
                app.scripts.push_log(format!("{} stopped: its document closed", script.name), false);
            }
            return false;
        }
        if active != Some(script.doc) {
            return true;
        }
        let keeps = pump(app, script, Some((now - script.started) * 1000.0), FRAME_WAIT).unwrap_or_else(|e| {
            report(app, &script.name, &e);
            false
        });
        if !keeps {
            app.scripts.push_log(format!("{} stopped", script.name), false);
        }
        keeps
    });
    // Scripts started by a script's frame join the list.
    scripts.append(&mut app.scripts.running);
    app.scripts.running = scripts;
    app.scripts.tidy();
    if let Some(mut console) = app.scripts.console.take() {
        let ms = (now - console.started) * 1000.0;
        if active == Some(console.doc)
            && let Err(e) = pump(app, &mut console, Some(ms), FRAME_WAIT)
        {
            report(app, &console.name, &e);
        }
        app.scripts.console = Some(console).filter(|c| is_open(app, c.doc));
    }
    if app.scripts.is_running() {
        ctx.request_repaint();
    }
}

/// Sends the script its queued work, then its frame at `frame` (milliseconds since it started)
/// when it has timers, one at a time, each with the document as it is, and applies what each one
/// changed. Waits at most `wait` for each: a script still working then goes on in a later frame.
/// Returns whether the script keeps running, or the error of the work that failed.
fn pump(app: &mut VectorcraftApp, script: &mut Running, mut frame: Option<f64>, wait: Duration) -> Result<bool, String> {
    loop {
        // Waiting on a dialog: nothing else until it's answered.
        if script.asking.is_some() {
            return Ok(true);
        }
        if script.busy.is_none() {
            let work = match script.queued.pop_front() {
                Some(work) => work,
                None => match frame.take() {
                    Some(now) if script.timers => Work::Frame(now),
                    _ => return Ok(script.timers || script.tool),
                },
            };
            send(app, script, work)?;
        }
        let Some(kind) = script.busy else {
            return Ok(true);
        };
        let Some(Done { error, tool, keys }) = settle(app, script, wait) else {
            return Ok(true);
        };
        if let Some(error) = error {
            return Err(error);
        }
        if kind == Kind::Execute
            && tool
            && let Some(path) = script.path.clone()
        {
            // As Scriptographer's tool button: the tool script runs, and its tool is the active
            // tool.
            script.tool = true;
            if app.scripts.tool().is_none_or(|t| t != path) {
                app.scripts.tool = Some(ToolScript { path, source: script.source.clone(), keys });
                app.scripts.tool_active = true;
            }
        }
    }
}

/// Sends `work` to the script with the active document.
fn send(app: &mut VectorcraftApp, script: &mut Running, work: Work) -> Result<(), String> {
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    script.busy = Some(match work {
        Work::Execute(_) => Kind::Execute,
        Work::Frame(_) => Kind::Frame,
        Work::Key(..) => Kind::Key,
        Work::Mouse(event, ..) => {
            if event == MouseEvent::Down {
                // A drag starts its own undo step.
                script.last = None;
            }
            Kind::Mouse
        }
    });
    script.worker.send(Request::Run { doc, selection, work });
    Ok(())
}

/// How a script's work ended: its error, whether it made a tool, and whether that handles keys.
struct Done {
    error: Option<String>,
    tool: bool,
    keys: bool,
}

/// Applies what the script's work changed as its replies come, until the work is done, the script
/// asks a dialog, or a reply takes longer than `wait`.
fn settle(app: &mut VectorcraftApp, script: &mut Running, wait: Duration) -> Option<Done> {
    let mut failed = None;
    while let Some(reply) = script.worker.recv(wait) {
        match reply {
            Reply::Changes { patch, base, before, selected } => {
                if let Err(e) = write_back(app, script, patch, base, &before, &selected) {
                    failed.get_or_insert(e);
                }
            }
            Reply::Ask(question) => {
                script.asking = Some(Asking { question, shown: false });
                return None;
            }
            Reply::Done { output, error, running, tool, keys } => {
                show_output(app, output);
                script.busy = None;
                script.timers = running;
                return Some(Done { error: error.or(failed), tool, keys });
            }
        }
    }
    None
}

/// Shows the dialogs scripts wait on, one at a time while no other dialog is open, and answers a
/// dialog that closed without OK as Cancel would.
fn dialogs(app: &mut VectorcraftApp) {
    let open = app.ui.dialog.as_ref().filter(|d| d.kind == crate::dialogs::script::KIND).and_then(|d| d.fields.get("script")?.as_u64());
    let mut show = None;
    for script in app.scripts.running.iter_mut().chain(&mut app.scripts.console) {
        let Some(asking) = &mut script.asking else {
            continue;
        };
        if asking.shown && open != Some(script.id) {
            script.worker.send(Request::Answer(crate::dialogs::script::cancelled(&asking.question)));
            script.asking = None;
        } else if !asking.shown && show.is_none() && app.ui.dialog.is_none() {
            asking.shown = true;
            show = Some((script.id, asking.question.clone()));
        }
    }
    if let Some((id, question)) = show {
        crate::dialogs::script::open(app, id, &question);
    }
}

/// The user's answer to the dialog the script `id` waits on.
pub(crate) fn answer(app: &mut VectorcraftApp, id: u64, answer: Value) {
    if let Some(script) = app.scripts.running.iter_mut().chain(&mut app.scripts.console).find(|s| s.id == id && s.asking.is_some()) {
        script.worker.send(Request::Answer(answer));
        script.asking = None;
    }
}

/// A script's file name, for messages.
fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// Whether the document `uid` is still open.
fn is_open(app: &VectorcraftApp, uid: u64) -> bool {
    app.session.documents().iter().any(|d| d.uid == uid)
}

/// Sends a pointer event on the canvas to the active tool script. Returns whether there is one,
/// which then has the event instead of the app's tool. Each drag is one undo step.
pub(crate) fn pointer(app: &mut VectorcraftApp, ev: &vectorcraft_tools::PointerEvent) -> bool {
    use vectorcraft_tools::PointerKind;
    let Some(active) = app.session.active().map(|d| d.uid).filter(|_| app.scripts.active_tool().is_some()) else {
        return false;
    };
    let event = match ev.kind {
        PointerKind::Down => MouseEvent::Down,
        PointerKind::Drag | PointerKind::Move => MouseEvent::Move,
        PointerKind::Up => MouseEvent::Up,
        PointerKind::DoubleClick => return true,
    };
    // `cmd` is Command on macOS, Ctrl elsewhere, as Paper.js's `modifiers.command` is.
    let mac = cfg!(target_os = "macos");
    let modifiers = Modifiers { shift: ev.mods.shift, control: ev.mods.ctrl || (ev.mods.cmd && !mac), alt: ev.mods.alt, meta: ev.mods.cmd && mac };
    to_tool(app, active, [Work::Mouse(event, (ev.pos.x, ev.pos.y), modifiers)]);
    true
}

/// Sends the keys typed on the canvas to the active tool script, when it handles keys: plain keys,
/// with Shift or not. Keys with Cmd, Ctrl or Alt stay the app's shortcuts, and so do Space (the
/// Hand tool) and Escape. The keys it gets are taken out of the frame's input.
pub(crate) fn keys(app: &mut VectorcraftApp, ctx: &egui::Context) {
    let handles = app.scripts.tool.as_ref().is_some_and(|t| t.keys) && app.scripts.active_tool().is_some();
    let Some(active) = app.session.active().map(|d| d.uid).filter(|_| handles) else {
        return;
    };
    let work = ctx.input_mut(|i| {
        let mut work = Vec::new();
        i.events.retain(|e| match e {
            egui::Event::Key { key, pressed, modifiers, .. }
                if !(modifiers.command || modifiers.ctrl || modifiers.alt) && !matches!(key, egui::Key::Space | egui::Key::Escape) =>
            {
                let (name, character) = paper_key(*key, modifiers.shift);
                work.push(Work::Key(*pressed, name, character, Modifiers { shift: modifiers.shift, ..Default::default() }));
                false
            }
            _ => true,
        });
        work
    });
    if !work.is_empty() {
        to_tool(app, active, work);
    }
}

/// A key's name as Paper.js has it, as browsers name keys (`a`, `5`, `-`, `enter`, `left`,
/// `page-up`…), and the character it types.
fn paper_key(key: egui::Key, shift: bool) -> (String, Option<String>) {
    let symbol = match key {
        egui::Key::Minus => "-",
        key => key.symbol_or_name(),
    };
    if symbol.len() == 1 {
        let name = symbol.to_lowercase();
        let character = if shift { symbol.to_uppercase() } else { name.clone() };
        return (name, Some(character));
    }
    let mut name = String::new();
    for (i, c) in key.name().chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            name.push('-');
        }
        name.extend(c.to_lowercase());
    }
    let character = match key {
        egui::Key::Enter => Some("\r".to_owned()),
        egui::Key::Tab => Some("\t".to_owned()),
        _ => None,
    };
    (name, character)
}

/// Sends `work` to the tool script in document `active`, running it there first for the tool's
/// first use in that document.
fn to_tool(app: &mut VectorcraftApp, active: u64, work: impl IntoIterator<Item = Work>) {
    let script = match app.scripts.running.iter().position(|s| s.tool && s.doc == active) {
        Some(index) => Ok(app.scripts.running.remove(index)),
        None => match app.scripts.tool.clone() {
            Some(tool) => load_tool(app, tool),
            None => return,
        },
    };
    let mut script = match script {
        Ok(script) => script,
        Err(e) => {
            app.status(e);
            app.scripts.tool = None;
            return;
        }
    };
    script.queued.extend(work);
    match pump(app, &mut script, None, FRAME_WAIT) {
        Ok(_) => app.scripts.running.push(script),
        Err(e) => {
            report(app, &script.name, &e);
            app.scripts.tool = None;
        }
    }
    app.scripts.tidy();
}

/// The tool script, to run in the active document for the tool's first use there.
fn load_tool(app: &mut VectorcraftApp, ToolScript { path, source, .. }: ToolScript) -> Result<Running, String> {
    let mut script = start(app, file_name(&path), Some(path), source)?;
    script.tool = true;
    Ok(script)
}

/// The canvas cursor while the PaperCraft Tool is the active tool, as Scriptographer's tool had it:
/// a pencil, or the crosshair with Use Precise Cursors.
pub(crate) fn cursor(app: &VectorcraftApp) -> Option<Cursor> {
    app.scripts.active_tool()?;
    Some(if app.session.prefs.use_precise_cursors { Cursor::Script.precise() } else { Cursor::Script })
}

/// The PaperCraft Tool's button in the toolbox, under the app's tools, as Scriptographer's tool
/// button: dimmed until a tool script is run, then named after it and selected while it is the
/// active tool. Clicking it makes the tool script the active tool again.
pub(crate) fn tool_button(app: &mut VectorcraftApp, ui: &mut egui::Ui, center_x: f32) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let assigned = app.scripts.tool().map(file_name);
    let active = app.scripts.active_tool().is_some();
    ui.horizontal(|ui| {
        ui.add_space((center_x - 18.0 - ui.cursor().min.x).max(0.0));
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 30.0), egui::Sense::click());
        let well = egui::Rect::from_center_size(rect.center(), egui::vec2(30.0, 27.5));
        if active {
            ui.painter().rect_filled(well, egui::CornerRadius::same(6), t.tool_active);
        } else if resp.hovered() && assigned.is_some() {
            ui.painter().rect_filled(well, egui::CornerRadius::same(6), t.hover);
        }
        // Scriptographer's tool button: an outline pencil until a tool script is assigned to it,
        // then a coloured one (painted untinted).
        let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
        match &assigned {
            Some(_) => crate::icons::paint(ui, "dc-papercraft-tool-active", icon, egui::Color32::WHITE),
            None => crate::icons::paint(ui, "dc-papercraft-tool", icon, t.text_disabled),
        }
        let tip = assigned.clone().unwrap_or_else(|| tl!("Run a tool script to assign it to this tool button").to_owned());
        if resp.on_hover_text(tip).clicked() && assigned.is_some() {
            app.scripts.select_tool();
        }
    });
}

/// An error from a script: in the log and the status bar.
fn report(app: &mut VectorcraftApp, name: &str, error: &str) {
    let message = format!("{name}: {error}");
    app.scripts.push_log(message.clone(), true);
    app.status(message);
}

/// Applies a script's `patch` of `base`, adding to its undo step while nothing else changed the
/// document since its last change, and leaving out what changed since `base` when it is behind;
/// then the objects it selected or deselected since `before`. The worker gets the patch's result.
fn write_back(
    app: &mut VectorcraftApp,
    script: &mut Running,
    patch: Option<Value>,
    base: Arc<Document>,
    before: &BTreeSet<NodeId>,
    selected: &BTreeSet<NodeId>,
) -> Result<(), String> {
    if let Some(mut params) = patch {
        let current = app.session.doc().map_err(|e| e.to_string())?.doc.clone();
        let behind = !Arc::ptr_eq(&base, &current);
        if let Some(params) = params.as_object_mut() {
            if behind {
                params.insert("keep".into(), vectorcraft_engine::cmd::patch::changes_between(&base, &current));
            } else if script.last.as_ref().is_some_and(|last| Arc::ptr_eq(last, &current)) {
                params.insert("extend".into(), json!(true));
            }
        }
        let applied = app.run("document.patch", params);
        let state = app.session.doc().map_err(|e| e.to_string())?;
        let (doc, selection) = (state.doc.clone(), state.selection.clone());
        script.last = Some(doc.clone());
        // A patch that failed changed nothing: the script reloads all that changed since `base`.
        let (result, behind) = match &applied {
            Ok(result) => (result.clone(), behind),
            Err(_) => (json!({}), true),
        };
        script.worker.send(Request::Applied { result, doc, selection, behind });
        applied?;
    }
    if selected != before {
        let state = app.session.doc().map_err(|e| e.to_string())?;
        let mut ids: Vec<NodeId> = state.selection.objects.iter().copied().filter(|id| !before.contains(id) || selected.contains(id)).collect();
        ids.extend(selected.iter().filter(|id| !before.contains(id)));
        app.run("select.set", json!({ "ids": ids.iter().map(|id| id.0).collect::<Vec<_>>() }))?;
    }
    Ok(())
}

/// Puts what a script printed in the log, and its last line in the status bar.
fn show_output(app: &mut VectorcraftApp, output: Vec<String>) {
    if let Some(last) = output.last() {
        app.status(last.clone());
    }
    for line in output {
        app.scripts.push_log(line, false);
    }
}

/// `ui.editScript {path}`: opens the script in the system's editor for it.
pub(crate) fn edit_script(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let path = p.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let open = app.services.open_file.as_mut().ok_or("no app to open files here")?;
    open(path)?;
    Ok(Value::Null)
}

/// `ui.newScriptFolder`: a new folder in the scripts folder (or `folder`), named New Folder or the
/// first free New Folder N. Returns its path.
pub(crate) fn new_folder(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let folder = match p.get("folder").and_then(Value::as_str) {
        Some(folder) => PathBuf::from(folder),
        None => app.ui.papercraft_folder.clone().map(PathBuf::from).ok_or("choose a scripts folder first")?,
    };
    let path = (1..1000)
        .map(|n| folder.join(if n == 1 { "New Folder".to_owned() } else { format!("New Folder {n}") }))
        .find(|path| !path.exists())
        .ok_or("too many new folders")?;
    std::fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    app.scripts.tree = None;
    Ok(json!({ "path": path.to_string_lossy() }))
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
