//! Scripts on threads of their own: a [`Worker`] runs one script in its own runtime, so the app
//! goes on while the script works, waits for a dialog or loops, and can stop it. The app sends
//! [`Request`]s with the document as it is, and the worker answers each one with the [`Reply`]s
//! that say what the script changed, then [`Reply::Done`].
//!
//! The worker reads the document from the snapshot it got; its changes go back as
//! `document.patch` params, which the app applies and answers with [`Request::Applied`] before
//! anything else. On the web there are no threads: the worker runs each request as it is sent.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as Json;
use vectorcraft_doc::{Document, NodeId, Selection};

use crate::{Engine, Error, Modifiers, MouseEvent, Patch, PlatformEngine, Runtime, Scope};

/// What the app asks a worker to do.
pub enum Request {
    /// Catch up with the document, then do `work`.
    Run { doc: Arc<Document>, selection: Selection, work: Work },
    /// The answer to [`Reply::Changes`]: `document.patch`'s result, and the document after it.
    /// `behind` when the document had changed in other ways since the patch's base.
    Applied { result: Json, doc: Arc<Document>, selection: Selection, behind: bool },
    /// The answer to [`Reply::Ask`].
    Answer(Json),
}

/// A script's work.
pub enum Work {
    /// Runs PaperScript.
    Execute(String),
    /// Runs the timers due at `now`, in milliseconds since the script started: its `onFrame` and
    /// intervals.
    Frame(f64),
    /// A pointer event for the script's tool, at a point in document coordinates.
    Mouse(MouseEvent, (f64, f64), Modifiers),
    /// A key going down (`true`) or up for the script's tool: its name as Paper.js has it, and the
    /// character it types.
    Key(bool, String, Option<String>, Modifiers),
}

/// What a worker tells the app.
pub enum Reply {
    /// The script shows a dialog (`Dialog.alert`, `confirm`, `prompt`) and waits for the user's
    /// answer ([`Request::Answer`]); the question as [`crate::Runtime::set_ask`] has it.
    Ask(Json),
    /// What the script changed: a patch of `base`, the document it read (the app answers with
    /// [`Request::Applied`]), and the objects selected in its project before and after.
    Changes { patch: Option<Json>, base: Arc<Document>, before: BTreeSet<NodeId>, selected: BTreeSet<NodeId> },
    /// The work is done: what the script printed, its error if it threw, whether it has timers
    /// left, whether it made a tool, and whether that tool handles keys.
    Done { output: Vec<String>, error: Option<String>, running: bool, tool: bool, keys: bool },
}

/// One script's runtime, and the patch waiting for its ids.
struct Script<E: Engine> {
    runtime: Runtime<E>,
    scope: Scope<E::Handle>,
    pending: Option<Patch<E::Handle>>,
}

impl<E: Engine> Script<E> {
    /// A runtime whose view has the given size, bound to the document.
    fn new(size: (f64, f64), doc: Arc<Document>, selection: Selection) -> Result<Self, Error> {
        let mut runtime = Runtime::<E>::new()?;
        let scope = runtime.create_scope(size.0, size.1)?;
        runtime.bind(&scope, doc, selection)?;
        Ok(Self { runtime, scope, pending: None })
    }

    fn handle(&mut self, request: Request) -> Vec<Reply> {
        match request {
            Request::Run { doc, selection, work } => self.run(doc, selection, work),
            // An answer no dialog waits for.
            Request::Answer(_) => Vec::new(),
            Request::Applied { result, doc, selection, behind } => match self.pending.take() {
                Some(patch) => match self.runtime.apply_ids(&self.scope, patch, &result, doc, selection, behind) {
                    Ok(()) => Vec::new(),
                    Err(e) => vec![self.done(Some(e.to_string()))],
                },
                None => Vec::new(),
            },
        }
    }

    /// Does `work` on the document, and replies what it changed, even when it threw, as a browser
    /// keeps what a script did before an error.
    fn run(&mut self, doc: Arc<Document>, selection: Selection, work: Work) -> Vec<Reply> {
        let before = match self.runtime.update(&self.scope, doc, selection).and_then(|()| self.runtime.selected_ids(&self.scope)) {
            Ok(before) => before,
            Err(e) => return vec![self.done(Some(e.to_string()))],
        };
        let result = match work {
            Work::Execute(source) => self.runtime.execute(&self.scope, &source),
            Work::Frame(now) => self.runtime.run_timers(now),
            Work::Mouse(event, point, modifiers) => self.runtime.mouse_event(&self.scope, event, point, modifiers),
            Work::Key(down, key, character, modifiers) => self.runtime.key_event(&self.scope, down, &key, character.as_deref(), modifiers),
        };
        let mut replies = Vec::new();
        let changes = self.changes(before).map(|changes| replies.extend(changes));
        replies.push(self.done(result.and(changes).err().map(|e| e.to_string())));
        replies
    }

    /// What the script changed, if anything.
    fn changes(&mut self, before: BTreeSet<NodeId>) -> Result<Option<Reply>, Error> {
        let patch = self.runtime.take_patch(&self.scope)?;
        let selected = self.runtime.selected_ids(&self.scope)?;
        if patch.is_none() && selected == before {
            return Ok(None);
        }
        let base = self.runtime.document().ok_or_else(|| Error::Script("no document is bound".into()))?;
        let params = patch.as_ref().map(|p| p.params.clone());
        self.pending = patch;
        Ok(Some(Reply::Changes { patch: params, base, before, selected }))
    }

    fn done(&mut self, error: Option<String>) -> Reply {
        let tool = self.runtime.has_tool(&self.scope).unwrap_or(false);
        let keys = tool && self.runtime.has_key_handlers(&self.scope).unwrap_or(false);
        Reply::Done { output: self.runtime.take_output(), error, running: self.runtime.is_running(), tool, keys }
    }
}

/// A script running on a thread of its own.
#[cfg(not(target_arch = "wasm32"))]
pub struct Worker {
    requests: std::sync::mpsc::Sender<Request>,
    replies: std::sync::mpsc::Receiver<Reply>,
    /// Stops the script mid-run, once its runtime is made, where the engine can.
    interrupt: Arc<std::sync::Mutex<Option<crate::Interrupt>>>,
    /// Set when the worker is dropped: the requests still queued are left.
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    /// Starts a script's thread, with a runtime whose view has the given size, on the document.
    pub fn spawn(size: (f64, f64), doc: Arc<Document>, selection: Selection) -> Result<Self, Error> {
        let (requests, inbox) = std::sync::mpsc::channel::<Request>();
        let (outbox, replies) = std::sync::mpsc::channel();
        let interrupt = Arc::new(std::sync::Mutex::new(None));
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (shared, is_stopped) = (interrupt.clone(), stopped.clone());
        let is_stopped = move || is_stopped.load(std::sync::atomic::Ordering::SeqCst);
        std::thread::Builder::new()
            .name("papercraft script".into())
            .spawn(move || {
                let inbox = std::rc::Rc::new(inbox);
                let mut script = match Script::<PlatformEngine>::new(size, doc, selection) {
                    Ok(script) => script,
                    Err(e) => {
                        // The app may have stopped the script already; then nobody listens.
                        let _ = outbox.send(Reply::Done { output: Vec::new(), error: Some(e.to_string()), running: false, tool: false, keys: false });
                        return;
                    }
                };
                if let Ok(mut slot) = shared.lock() {
                    *slot = script.runtime.interrupt();
                }
                // A dialog asks the app, and waits for its answer: the app sends nothing else
                // meanwhile. Stopped, the script throws.
                let (asking, answers) = (outbox.clone(), inbox.clone());
                script.runtime.set_ask(Box::new(move |question| {
                    asking.send(Reply::Ask(question)).map_err(|_| Error::Script("stopped".into()))?;
                    loop {
                        match answers.recv() {
                            Ok(Request::Answer(answer)) => return Ok(answer),
                            Ok(_) => continue,
                            Err(_) => return Err(Error::Script("stopped".into())),
                        }
                    }
                }));
                // Until the app drops the worker.
                while let Ok(request) = inbox.recv()
                    && !is_stopped()
                {
                    for reply in script.handle(request) {
                        if outbox.send(reply).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|e| Error::Script(format!("can't start the script's thread: {e}")))?;
        Ok(Self { requests, replies, interrupt, stopped })
    }

    pub fn send(&self, request: Request) {
        // Fails only when the thread ended, which its last reply said.
        let _ = self.requests.send(request);
    }

    /// The next reply, waiting for it at most `wait`.
    pub fn recv(&self, wait: Duration) -> Option<Reply> {
        self.replies.recv_timeout(wait).ok()
    }

    /// Stops what the script runs now, where the engine can (V8): it throws, and its `Done` says
    /// so. Boa scripts run on until they return.
    pub fn interrupt(&self) {
        if let Ok(slot) = self.interrupt.lock()
            && let Some(interrupt) = slot.as_ref()
        {
            interrupt();
        }
    }
}

/// Stops the script: its thread ends once the script returns.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        self.interrupt();
    }
}

/// A script, run on the app's thread: each request runs as it is sent.
#[cfg(target_arch = "wasm32")]
pub struct Worker {
    script: std::cell::RefCell<Script<PlatformEngine>>,
    replies: std::cell::RefCell<std::collections::VecDeque<Reply>>,
}

#[cfg(target_arch = "wasm32")]
impl Worker {
    pub fn spawn(size: (f64, f64), doc: Arc<Document>, selection: Selection) -> Result<Self, Error> {
        let script = Script::<PlatformEngine>::new(size, doc, selection)?;
        Ok(Self { script: std::cell::RefCell::new(script), replies: Default::default() })
    }

    pub fn send(&self, request: Request) {
        // Requests come from the app, one at a time: the borrows are free.
        if let (Ok(mut script), Ok(mut replies)) = (self.script.try_borrow_mut(), self.replies.try_borrow_mut()) {
            replies.extend(script.handle(request));
        }
    }

    pub fn recv(&self, _wait: Duration) -> Option<Reply> {
        self.replies.try_borrow_mut().ok()?.pop_front()
    }
}
