//! Scriptographer: PaperScript and the Paper.js API in VectorCraft.
//!
//! A [`Runtime`] loads Paper.js into a JavaScript [`Engine`]: V8 on desktop, the browser's engine
//! on the web, Boa where V8 isn't available. The globals Paper.js needs from its host (`self`,
//! `navigator`, `console`, `setInterval`) are written here once, over the engine interface. Each
//! [`Scope`] is a `PaperScope` with its own project and view, in which scripts run as PaperScript.

#![forbid(unsafe_code)]

mod binding;
mod bitmap;
mod canvas;
mod clipboard;
mod clone;
mod console;
mod dialog;
#[cfg(not(target_arch = "wasm32"))]
mod dom;
mod drawing;
mod engine;
mod engines;
mod events;
mod exception;
#[cfg(not(target_arch = "wasm32"))]
mod fetch;
#[cfg(not(target_arch = "wasm32"))]
mod file_system;
mod geometry;
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
mod local_file;
mod modules;
mod palette;
mod path2d;
mod storage;
#[cfg(not(target_arch = "wasm32"))]
mod urls;
#[cfg(not(target_arch = "wasm32"))]
mod web;
#[cfg(target_arch = "wasm32")]
mod web_file;
#[cfg(target_arch = "wasm32")]
pub use web_file::Picked;
#[cfg(not(target_arch = "wasm32"))]
mod web_worker;
#[cfg(not(target_arch = "wasm32"))]
mod websocket;
pub mod worker;
mod writeback;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use vectorcraft_doc::Document;

pub use binding::Patch;
pub use dialog::Ask;
pub use engine::{Call, Engine, HostFunction, Interrupt, Prelude, Value};
#[cfg(feature = "boa")]
pub use engines::boa::Boa;
#[cfg(target_arch = "wasm32")]
pub use engines::browser::Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub use engines::v8::V8;
pub use palette::PaletteEvent;

/// The handlers a script defines for the app to call, as Scriptographer had them: when the app
/// starts (after the `__init__` scripts ran) and quits, when the user stops the running scripts,
/// when the app comes to the front or goes behind another, and, for a tool script, when the user
/// double-clicks its tool's button, to edit the tool's options.
pub const HANDLERS: [&str; 6] = ["onStartup", "onShutdown", "onStop", "onAppActivate", "onAppDeactivate", "onEditOptions"];

/// The engine scripts run in on this platform: the browser's on the web, V8 on desktop, Boa where
/// V8 is left out.
#[cfg(target_arch = "wasm32")]
pub type PlatformEngine = Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub type PlatformEngine = V8;
#[cfg(all(feature = "boa", not(feature = "v8"), not(target_arch = "wasm32")))]
pub type PlatformEngine = Boa;

/// Paper.js with the PaperScript compiler and items backed by a host document: the vendored
/// `paper-hosted.js` build of Paper.js 0.14.0-develop (MIT; `dist/paper-hosted.js` of the npm package),
/// or the build `SCRIPTOGRAPHER_PAPER_JS` names (see `build.rs`).
const PAPER_JS: &str = include_str!(env!("SCRIPTOGRAPHER_PAPER_JS_PATH"));

/// Acorn 8.19.0 (MIT; `dist/acorn.js` of the npm package), which PaperScript prefers to the Acorn
/// 0.5 Paper.js bundles when it finds it as `self.acorn`: scripts can use today's JavaScript.
const ACORN: &str = include_str!("../vendor/acorn.js");

/// The version of the Paper.js build scripts run with, as its header names it (`0.13.0-develop`).
pub fn paper_version() -> &'static str {
    PAPER_JS.split_once("Paper.js v").and_then(|(_, rest)| rest.split_whitespace().next()).unwrap_or("")
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A script threw, or Paper.js failed to load; thrown to scripts as an `Error`.
    #[error("{0}")]
    Script(String),
    /// A host function's `TypeError`: an argument of the wrong kind.
    #[error("TypeError: {0}")]
    Type(String),
    /// A host function's `RangeError`: a number out of range.
    #[error("RangeError: {0}")]
    Range(String),
    /// A host function's `DOMException` of this name (`IndexSizeError`, `InvalidStateError`,
    /// `SyntaxError`, `AbortError` …), as browsers throw them.
    #[error("{0}: {1}")]
    Dom(&'static str, String),
}

/// The state behind the host functions of one runtime.
pub struct Host<H> {
    /// Lines printed with `console.log`, kept until [`Runtime::take_output`].
    output: Vec<String>,
    next_interval_id: u32,
    /// The time of the last [`Runtime::run_timers`], in milliseconds.
    now: f64,
    /// The timers of `setInterval` and `setTimeout`.
    intervals: Vec<Interval<H>>,
    /// The callbacks `requestAnimationFrame()` asked to run on the next frame, by id.
    frames: Vec<(u32, H)>,
    /// The document the project reads from ([`Runtime::bind`]).
    bound: Option<binding::Bound>,
    /// The document's active artboard, `project.activeArtboard` ([`Runtime::set_artboard`]).
    artboard: usize,
    /// Paper.js's `Rectangle`, for `project.artboards`.
    rectangle: Option<H>,
    /// The methods of every `OffscreenCanvas`, by name.
    canvases: Option<canvas::Canvases<H>>,
    /// Answers the script's dialogs ([`Runtime::set_ask`]).
    ask: Option<dialog::Ask>,
    /// The questions of the script's dialogs the app answers later (the web).
    #[cfg(target_arch = "wasm32")]
    asked: std::collections::VecDeque<dialog::Asked<H>>,
    /// What `LocalFile`s on the web share.
    #[cfg(target_arch = "wasm32")]
    web_files: Option<web_file::Files<H>>,
    /// What `Dialog` uses of the script's realm.
    dialog_globals: Option<dialog::Globals<H>>,
    /// The palettes the script made, open.
    palettes: Vec<palette::Palette<H>>,
    /// The `reset()` of every palette.
    palette_reset: Option<H>,
    /// What `LocalFile` objects share.
    #[cfg(not(target_arch = "wasm32"))]
    files: Option<local_file::Files<H>>,
    /// What the file system's handles share.
    #[cfg(not(target_arch = "wasm32"))]
    handles: Option<file_system::Handles<H>>,
    /// The script's file, which its imports are relative to.
    file: Option<std::path::PathBuf>,
    /// The script's folder, which relative paths of `LocalFile`s are in.
    directory: Option<std::path::PathBuf>,
    /// What `import` needs, once a script runs.
    loader: Option<modules::Loader<H>>,
    /// The modules the scripts imported.
    modules: Vec<modules::Module<H>>,
    /// `window` and `document` outside a browser.
    #[cfg(not(target_arch = "wasm32"))]
    dom: dom::Dom<H>,
    /// The browser's everyday APIs outside a browser.
    #[cfg(not(target_arch = "wasm32"))]
    web: Option<web::Web<H>>,
    /// `fetch` and its classes outside a browser, its requests out, and the `resolve` and
    /// `reject` of the Promise being made.
    #[cfg(not(target_arch = "wasm32"))]
    fetch: Option<fetch::Classes<H>>,
    #[cfg(not(target_arch = "wasm32"))]
    fetches: Vec<fetch::Pending<H>>,
    /// The open `WebSocket`s, and the number the next one gets.
    #[cfg(not(target_arch = "wasm32"))]
    sockets: Vec<websocket::Socket<H>>,
    #[cfg(not(target_arch = "wasm32"))]
    next_socket: u32,
    /// The workers the script started, and the number the next one gets; in a worker, its parent.
    #[cfg(not(target_arch = "wasm32"))]
    children: Vec<web_worker::Child<H>>,
    #[cfg(not(target_arch = "wasm32"))]
    next_child: u32,
    #[cfg(not(target_arch = "wasm32"))]
    parent: Option<web_worker::Parent<H>>,
    #[cfg(not(target_arch = "wasm32"))]
    resolvers: Option<(H, H)>,
    /// `URL` and `URLSearchParams` outside a browser.
    #[cfg(not(target_arch = "wasm32"))]
    urls: Option<urls::Classes<H>>,
    /// The realm's error classes, for the exceptions host functions throw.
    exceptions: Option<exception::Classes<H>>,
    /// What `EventTarget` and `Event` share.
    events: Option<events::Classes<H>>,
    /// What `DOMMatrix` and the other geometry classes share.
    geometry: Option<geometry::Classes<H>>,
    /// The realm's `Promise`, for the clipboard's answers.
    clipboard_promise: Option<H>,
    /// What `structuredClone()` uses of the realm.
    clone_classes: Option<clone::Classes<H>>,
    /// The console's groups, counters and timers.
    console: Option<console::Console<H>>,
    /// The script's `localStorage` and `sessionStorage`, and what their proxies use.
    local_storage: storage::Storage,
    session_storage: storage::Storage,
    storages: Option<storage::Classes<H>>,
    next_palette_id: u64,
    /// The language the app gave the script ([`Setup`]), which its workers have too.
    language: Option<String>,
    /// The folder of the copies of what scripts import from the web ([`Setup`]), which its workers
    /// use too.
    cache: Option<std::path::PathBuf>,
    /// The [`HANDLERS`] the scripts defined, by name.
    handlers: Vec<(&'static str, H)>,
    /// Why a script with top-level `await` failed, once it did, for the call it happened in.
    failure: Option<Error>,
}

struct Interval<H> {
    id: u32,
    callback: H,
    delay: f64,
    due: f64,
    /// An interval's, which runs again; else a timeout's, which runs once.
    repeats: bool,
}

impl<H> Default for Host<H> {
    fn default() -> Self {
        Self {
            output: Vec::new(),
            next_interval_id: 0,
            now: 0.0,
            intervals: Vec::new(),
            frames: Vec::new(),
            bound: None,
            artboard: 0,
            rectangle: None,
            canvases: None,
            ask: None,
            #[cfg(target_arch = "wasm32")]
            asked: Default::default(),
            #[cfg(target_arch = "wasm32")]
            web_files: None,
            dialog_globals: None,
            palettes: Vec::new(),
            palette_reset: None,
            #[cfg(not(target_arch = "wasm32"))]
            files: None,
            #[cfg(not(target_arch = "wasm32"))]
            handles: None,
            file: None,
            directory: None,
            loader: None,
            modules: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            dom: dom::Dom::None,
            #[cfg(not(target_arch = "wasm32"))]
            web: None,
            #[cfg(not(target_arch = "wasm32"))]
            fetch: None,
            #[cfg(not(target_arch = "wasm32"))]
            fetches: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            sockets: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            next_socket: 0,
            #[cfg(not(target_arch = "wasm32"))]
            children: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            next_child: 0,
            #[cfg(not(target_arch = "wasm32"))]
            parent: None,
            #[cfg(not(target_arch = "wasm32"))]
            resolvers: None,
            #[cfg(not(target_arch = "wasm32"))]
            urls: None,
            exceptions: None,
            events: None,
            geometry: None,
            clipboard_promise: None,
            clone_classes: None,
            console: None,
            local_storage: storage::Storage::default(),
            session_storage: storage::Storage::default(),
            storages: None,
            next_palette_id: 0,
            language: None,
            cache: None,
            handlers: Vec::new(),
            failure: None,
        }
    }
}

/// Paper.js loaded into an engine.
pub struct Runtime<E: Engine> {
    engine: E,
    host: Rc<RefCell<Host<E::Handle>>>,
    paper: E::Handle,
    /// The loader of `import`s ([`modules::load`]).
    load: E::Handle,
}

/// What the app gives a script when it starts: its file (none for code typed in the console), the
/// folder the app keeps scripts' `localStorage` and `script.preferences` in, the one it keeps
/// what they import from the web in, and the language the app's UI is in, as `navigator.language`.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub file: Option<String>,
    pub storage: Option<std::path::PathBuf>,
    pub cache: Option<std::path::PathBuf>,
    pub language: Option<String>,
}

/// A `PaperScope` with its own project and view.
pub struct Scope<H>(H);

impl<E: Engine> Drop for Runtime<E> {
    /// Writes what's left of the script's `localStorage`; a failure can't reach the script now.
    fn drop(&mut self) {
        if let Ok(mut host) = self.host.try_borrow_mut() {
            let _ = host.local_storage.write(None);
        }
    }
}

impl<E: Engine> Runtime<E> {
    pub fn new() -> Result<Self, Error> {
        let host = Rc::new(RefCell::new(Host::default()));
        let mut engine = E::new(host.clone(), prelude)?;
        let global = engine.global()?;
        exception::install(&mut engine, &global)?;
        geometry::install(&mut engine, &global)?;
        storage::install(&mut engine, &global)?;
        // Before the classes that are event targets (`AbortSignal`).
        events::install(&mut engine, &global)?;
        install_globals(&mut engine)?;
        let paper = engine.with_call(|call| object(call, &global, "paper"))?;
        let load = engine.function("load", modules::load)?;
        let mut runtime = Self { engine, host, paper, load };
        dialog::install(&mut runtime, &global)?;
        palette::install(&mut runtime.engine, &global)?;
        // After Paper.js, which would take a DOM for a browser's.
        #[cfg(not(target_arch = "wasm32"))]
        dom::install(&mut runtime, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        web::install(&mut runtime, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        fetch::install(&mut runtime, &global)?;
        // After `File`, which it extends.
        #[cfg(not(target_arch = "wasm32"))]
        local_file::install(&mut runtime, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        file_system::install(&mut runtime, &global)?;
        #[cfg(target_arch = "wasm32")]
        web_file::install(&mut runtime, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        urls::install(&mut runtime, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        websocket::install(&mut runtime.engine, &global)?;
        #[cfg(not(target_arch = "wasm32"))]
        web_worker::install(&mut runtime.engine, &global)?;
        clone::install(&mut runtime.engine, &global)?;
        clipboard::install(&mut runtime.engine, &global)?;
        // After `performance`, its clock.
        console::install(&mut runtime.engine, &global)?;
        Ok(runtime)
    }

    /// Creates a scope whose view has the given size.
    pub fn create_scope(&mut self, width: f64, height: f64) -> Result<Scope<E::Handle>, Error> {
        let paper = &self.paper;
        self.engine.with_call(|call| {
            let constructor = object(call, paper, "PaperScope")?;
            let scope = handle(call.construct(&constructor, &[])?, "PaperScope")?;
            let size = object(call, paper, "Size")?;
            let size = call.construct(&size, &[Value::Number(width), Value::Number(height)])?;
            method(call, &scope, "setup", &[size])?;
            Ok(Scope(scope))
        })
    }

    /// Compiles and runs `source` as PaperScript in `scope`, with its `import`s ([`modules`]). The
    /// [`HANDLERS`] it defines are kept for [`Runtime::call_handler`]: with top-level `await`, once
    /// it has finished, and its failure is the error of this call, or of the frame it happens in
    /// ([`Runtime::run_timers`]).
    pub fn execute(&mut self, scope: &Scope<E::Handle>, source: &str) -> Result<(), Error> {
        let kept = self.engine.function("kept", |call, _, args| {
            keep_handlers(call, args.first())?;
            Ok(Value::Undefined)
        })?;
        let failed = self.engine.function("failed", |call, _, args| {
            let reason = args.first().cloned().unwrap_or(Value::Undefined);
            let mut message = call.to_string(&reason)?;
            // The stack where the engine's starts with the error, as V8's does.
            if let Value::Js(error) = &reason
                && let Value::String(stack) = call.get(error, "stack")?
                && stack.starts_with(&message)
            {
                message = stack;
            }
            call.host()?.borrow_mut().failure = Some(Error::Script(message));
            Ok(Value::Undefined)
        })?;
        let (paper, host, load) = (&self.paper, &self.host, &self.load);
        self.engine.with_call(|call| {
            let paperscript = object(call, paper, "PaperScript")?;
            modules::set_up(call, paper, &scope.0, load.clone())?;
            let url = host.borrow().file.as_ref().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let options = modules::options(call, &url)?;
            let exports = method(call, &paperscript, "execute", &[Value::String(source.to_owned()), Value::Js(scope.0.clone()), Value::Js(options)])?;
            // A script with top-level `await` gives a Promise of its exports.
            match &exports {
                Value::Js(promise) if matches!(call.get(promise, "then")?, Value::Js(_)) => {
                    method(call, promise, "then", &[Value::Js(kept), Value::Js(failed)]).map(|_| ())
                }
                _ => keep_handlers(call, Some(&exports)),
            }
        })?;
        self.failure()
    }

    /// Runs `source` as a classic script in the realm's global scope, as a browser's `<script>`
    /// or a worker's `importScripts()` does: not PaperScript, and without imports.
    pub fn eval(&mut self, source: &str) -> Result<(), Error> {
        self.engine.eval(source).map(|_| ())
    }

    /// Calls the script's handler `name` (one of [`HANDLERS`]), when it defines one. Returns
    /// whether it did.
    pub fn call_handler(&mut self, name: &str) -> Result<bool, Error> {
        let handler = self.host.borrow().handlers.iter().find(|(n, _)| *n == name).map(|(_, h)| h.clone());
        let Some(handler) = handler else {
            return Ok(false);
        };
        self.engine.with_call(|call| call.call(&handler, &Value::Undefined, &[]))?;
        Ok(true)
    }

    /// Adds the items in `json`, Paper.js JSON, to the scope's project (`project.importJSON()`).
    pub fn import_json(&mut self, scope: &Scope<E::Handle>, json: &str) -> Result<(), Error> {
        self.engine.with_call(|call| {
            let project = object(call, &scope.0, "project")?;
            method(call, &project, "importJSON", &[Value::String(json.to_owned())])?;
            Ok(())
        })
    }

    /// The scope's project as Paper.js JSON (`project.exportJSON()`).
    pub fn export_json(&mut self, scope: &Scope<E::Handle>) -> Result<String, Error> {
        self.engine.with_call(|call| {
            let project = object(call, &scope.0, "project")?;
            match method(call, &project, "exportJSON", &[])? {
                Value::String(json) => Ok(json),
                _ => Err(Error::Script("`exportJSON()` didn't return a string".into())),
            }
        })
    }

    /// Runs the `setInterval` and `setTimeout` callbacks that are due at `now`, in milliseconds
    /// since the runtime was created, then those `requestAnimationFrame()` asked for, with `now`. The host calls this once per frame, so no interval runs more often than that.
    pub fn run_timers(&mut self, now: f64) -> Result<(), Error> {
        let mut due = Vec::new();
        {
            let mut host = self.host.borrow_mut();
            host.now = now;
            if let Err(e) = host.local_storage.write(Some(now)) {
                host.output.push(e);
            }
            for interval in host.intervals.iter_mut().filter(|interval| interval.due <= now) {
                due.push(interval.callback.clone());
                // Keep the cadence, but don't catch up on intervals missed while the host was busy.
                interval.due += interval.delay;
                if interval.due <= now {
                    interval.due = now + interval.delay;
                }
                // A timeout runs once.
                if !interval.repeats {
                    interval.due = f64::NAN;
                }
            }
            host.intervals.retain(|interval| !interval.due.is_nan());
        }
        // The frame's callbacks: those they request run on the next one.
        let frames = std::mem::take(&mut self.host.borrow_mut().frames);
        // Callbacks may set or clear intervals, so they run after the borrow ends. The requests
        // that answered meanwhile settle first.
        self.engine.with_call(|call| {
            #[cfg(not(target_arch = "wasm32"))]
            fetch::poll(call)?;
            #[cfg(not(target_arch = "wasm32"))]
            websocket::poll(call)?;
            #[cfg(not(target_arch = "wasm32"))]
            web_worker::poll(call)?;
            for callback in due {
                call.call(&callback, &Value::Undefined, &[])?;
            }
            for (_, callback) in frames {
                call.call(&callback, &Value::Undefined, &[Value::Number(now)])?;
            }
            Ok(())
        })?;
        self.failure()
    }

    /// The failure of a script with top-level `await`, once it failed.
    fn failure(&mut self) -> Result<(), Error> {
        self.host.borrow_mut().failure.take().map_or(Ok(()), Err)
    }

    /// Whether the scripts have something left to run: a timer, which `onFrame` uses too, a frame
    /// callback, or a request out.
    pub fn is_running(&self) -> bool {
        let host = self.host.borrow();
        #[cfg(not(target_arch = "wasm32"))]
        if !host.fetches.is_empty() || !host.sockets.is_empty() || !host.children.is_empty() {
            return true;
        }
        !host.intervals.is_empty() || !host.frames.is_empty()
    }

    /// Sets `project.currentStyle` of `scope`: `style` as [`host::current_style`] makes it.
    pub fn set_current_style(&mut self, scope: &Scope<E::Handle>, style: &serde_json::Value) -> Result<(), Error> {
        self.engine.with_call(|call| {
            let globals = dialog::globals(call)?;
            let style = dialog::from_json(call, &globals, style)?;
            let project = object(call, &scope.0, "project")?;
            call.set(&project, "currentStyle", &style)
        })
    }

    /// Sets the global `script`, as Scriptographer's: `script.file` and `script.directory`, the
    /// script's file and its folder as `LocalFile`s (`null` for code typed in the console), `script.keepAlive`,
    /// which the script sets to keep running when another script runs, and
    /// `script.preferences`, the preferences the script last left ([`Runtime::keep_preferences`]).
    /// Its `localStorage` is kept in the app's folder for them, when it has one and the script a
    /// file; `navigator.language` is the app's.
    pub fn set_script(&mut self, setup: &Setup) -> Result<(), Error> {
        let Setup { file, storage, cache, language } = setup;
        let file = file.as_deref();
        let global = self.engine.global()?;
        let directory = file.and_then(|f| std::path::Path::new(f).parent()).map(std::path::Path::to_path_buf);
        {
            let mut host = self.host.borrow_mut();
            if let (Some(folder), Some(file)) = (storage, file) {
                host.local_storage = storage::Storage::kept_in(storage::Storage::file_for(folder, std::path::Path::new(file)));
            }
            host.language.clone_from(language);
            host.cache.clone_from(cache);
            host.file = file.map(std::path::PathBuf::from);
            host.directory = directory.clone();
        }
        self.engine.with_call(|call| {
            let script = call.new_object()?;
            // On the web, scripts have no files.
            #[cfg(not(target_arch = "wasm32"))]
            let (file, directory) = (
                match file {
                    Some(file) => local_file::new_local_file(call, std::path::Path::new(file))?,
                    None => Value::Null,
                },
                match &directory {
                    Some(directory) => local_file::new_local_file(call, directory)?,
                    None => Value::Null,
                },
            );
            #[cfg(target_arch = "wasm32")]
            let (file, directory) = (Value::Null, Value::Null);
            call.set(&script, "file", &file)?;
            call.set(&script, "directory", &directory)?;
            call.set(&script, "keepAlive", &Value::Bool(false))?;
            let preferences = match &self.host.borrow().local_storage.preferences {
                kept @ serde_json::Value::Object(_) => kept.clone(),
                _ => serde_json::json!({}),
            };
            // As Paper.js serializes them, so they come back as Colors, Points…
            let base = object(call, &self.paper, "Base")?;
            let preferences = method(call, &base, "importJSON", &[Value::String(preferences.to_string())])?;
            call.set(&script, "preferences", &preferences)?;
            call.set(&global, "script", &Value::Js(script))?;
            if let (Some(language), Value::Js(navigator)) = (language, call.get(&global, "navigator")?) {
                set_language(call, &navigator, language)?;
            }
            Ok(())
        })
    }

    /// Keeps `script.preferences` with the script's `localStorage`, when they changed.
    pub fn keep_preferences(&mut self) -> Result<(), Error> {
        let global = self.engine.global()?;
        let preferences = self.engine.with_call(|call| {
            let Value::Js(script) = call.get(&global, "script")? else {
                return Ok(None);
            };
            let preferences = call.get(&script, "preferences")?;
            let globals = dialog::globals(call)?;
            dialog::to_json(call, &globals, &preferences).map(Some)
        })?;
        if let Some(preferences) = preferences {
            self.host.borrow_mut().local_storage.keep_preferences(preferences);
        }
        Ok(())
    }

    /// Whether the script set `script.keepAlive`.
    pub fn keep_alive(&mut self) -> Result<bool, Error> {
        let global = self.engine.global()?;
        self.engine.with_call(|call| {
            let Value::Js(script) = call.get(&global, "script")? else {
                return Ok(false);
            };
            Ok(matches!(call.get(&script, "keepAlive")?, Value::Bool(true)))
        })
    }

    /// A way to stop the runtime's scripts from another thread, where the engine has one.
    pub fn interrupt(&mut self) -> Option<Interrupt> {
        self.engine.interrupt()
    }

    /// The document the project reads from, as it was when the project last caught up with it.
    pub fn document(&self) -> Option<Arc<Document>> {
        self.host.borrow().bound.as_ref().map(|b| b.doc.clone())
    }

    /// Returns the lines printed with `console.log` since the last call.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.host.borrow_mut().output)
    }
}

/// A pointer event for a script's tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    Down,
    /// A move, which is a drag while the button is down.
    Move,
    Up,
}

/// The keys held during a pointer or key event, as Paper.js's `Key.modifiers` has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl<E: Engine> Runtime<E> {
    /// Whether the script in `scope` made a tool: it defines mouse or key handlers, or uses `tool`.
    pub fn has_tool(&mut self, scope: &Scope<E::Handle>) -> Result<bool, Error> {
        self.engine.with_call(|call| Ok(matches!(call.get(&scope.0, "tool")?, Value::Js(_))))
    }

    /// Sends a pointer event at `point`, in document coordinates, to the view of `scope`
    /// (`view.dispatchMouseEvent()`): the tool's `onMouseDown`, `onMouseDrag`, `onMouseUp` and
    /// `onMouseMove` run, drags as often as `tool.minDistance`, `maxDistance` and `fixedDistance`
    /// say, and so do the handlers of the items and the view. `time` is the event's `time`, in
    /// seconds.
    pub fn mouse_event(
        &mut self,
        scope: &Scope<E::Handle>,
        event: MouseEvent,
        point: (f64, f64),
        modifiers: Modifiers,
        time: f64,
    ) -> Result<(), Error> {
        let paper = &self.paper;
        self.engine.with_call(|call| {
            let options = event_options(call, modifiers)?;
            call.set(&options, "time", &Value::Number(time))?;
            let kind = match event {
                MouseEvent::Down => "mousedown",
                MouseEvent::Move => "mousemove",
                MouseEvent::Up => "mouseup",
            };
            let constructor = object(call, paper, "Point")?;
            let point = call.construct(&constructor, &[Value::Number(point.0), Value::Number(point.1)])?;
            let view = object(call, &scope.0, "view")?;
            method(call, &view, "dispatchMouseEvent", &[Value::String(kind.into()), point, Value::Js(options)])?;
            Ok(())
        })
    }

    /// The user picked the script's tool (`true`) or another one: its `onActivate` runs (again,
    /// when it was already the active tool), or its `onDeactivate`.
    pub fn activate_tool(&mut self, scope: &Scope<E::Handle>, active: bool) -> Result<(), Error> {
        self.engine.with_call(|call| {
            let Value::Js(tool) = call.get(&scope.0, "tool")? else {
                return Ok(());
            };
            if active {
                method(call, &tool, "activate", &[])?;
            } else {
                method(call, &tool, "emit", &[Value::String("deactivate".into())])?;
            }
            Ok(())
        })
    }

    /// Whether the tool of the script in `scope` handles keys: it has `onKeyDown` or `onKeyUp`.
    pub fn has_key_handlers(&mut self, scope: &Scope<E::Handle>) -> Result<bool, Error> {
        self.engine.with_call(|call| {
            let Value::Js(tool) = call.get(&scope.0, "tool")? else {
                return Ok(false);
            };
            for kind in ["keydown", "keyup"] {
                if matches!(method(call, &tool, "responds", &[Value::String(kind.into())])?, Value::Bool(true)) {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// Sends a key going down (or up) to the view of `scope` (`view.dispatchKeyEvent()`): the
    /// tool's `onKeyDown` and `onKeyUp` run. `key` is the key's name as Paper.js has it (`a`,
    /// `enter`, `left`, `page-up`…), `character` what it types, if anything.
    pub fn key_event(&mut self, scope: &Scope<E::Handle>, down: bool, key: &str, character: Option<&str>, modifiers: Modifiers) -> Result<(), Error> {
        self.engine.with_call(|call| {
            let options = event_options(call, modifiers)?;
            if let Some(character) = character {
                call.set(&options, "character", &Value::String(character.into()))?;
            }
            let view = object(call, &scope.0, "view")?;
            let kind = if down { "keydown" } else { "keyup" };
            method(call, &view, "dispatchKeyEvent", &[Value::String(kind.into()), Value::String(key.into()), Value::Js(options)])?;
            Ok(())
        })
    }
}

/// The options of an event Paper.js dispatches: the keys held.
/// Keeps the [`HANDLERS`] in a script's `exports` for [`Runtime::call_handler`].
fn keep_handlers<H: Clone>(call: &mut dyn Call<H>, exports: Option<&Value<H>>) -> Result<(), Error> {
    let Some(Value::Js(exports)) = exports else {
        return Ok(());
    };
    for name in HANDLERS {
        if let Value::Js(handler) = call.get(exports, name)? {
            let host = call.host()?;
            let mut host = host.borrow_mut();
            host.handlers.retain(|(n, _)| n != &name);
            host.handlers.push((name, handler));
        }
    }
    Ok(())
}

fn event_options<H>(call: &mut dyn Call<H>, modifiers: Modifiers) -> Result<H, Error> {
    let held = call.new_object()?;
    for (name, down) in [("shift", modifiers.shift), ("control", modifiers.control), ("alt", modifiers.alt), ("meta", modifiers.meta)] {
        call.set(&held, name, &Value::Bool(down))?;
    }
    let options = call.new_object()?;
    call.set(&options, "modifiers", &Value::Js(held))?;
    Ok(options)
}

/// Defines the property `name` of `target`, which `getter` gives and `setter`, when there is one,
/// changes; read-only without it.
fn define_accessor<H: Clone>(call: &mut dyn Call<H>, global: &H, target: &H, name: &str, getter: H, setter: Option<H>) -> Result<(), Error> {
    let descriptor = call.new_object()?;
    call.set(&descriptor, "get", &Value::Js(getter))?;
    if let Some(setter) = setter {
        call.set(&descriptor, "set", &Value::Js(setter))?;
    }
    call.set(&descriptor, "enumerable", &Value::Bool(true))?;
    let object_class = object(call, global, "Object")?;
    method(call, &object_class, "defineProperty", &[Value::Js(target.clone()), Value::String(name.into()), Value::Js(descriptor)])?;
    Ok(())
}

/// Names the objects of `prototype` `name` for `Object.prototype.toString()` (`[object
/// DOMMatrix]`), through the realm's `Symbol.toStringTag`, as the browser's classes have it.
pub(crate) fn tag<H: Clone>(call: &mut dyn Call<H>, global: &H, prototype: &H, name: &str) -> Result<(), Error> {
    let symbol = object(call, global, "Symbol")?;
    let to_string_tag = call.get(&symbol, "toStringTag")?;
    let descriptor = call.new_object()?;
    call.set(&descriptor, "value", &Value::String(name.into()))?;
    call.set(&descriptor, "configurable", &Value::Bool(true))?;
    let object_class = object(call, global, "Object")?;
    method(call, &object_class, "defineProperty", &[Value::Js(prototype.clone()), to_string_tag, Value::Js(descriptor)])?;
    Ok(())
}

/// JavaScript's `Boolean(value)`.
pub(crate) fn truthy<H>(value: &Value<H>) -> bool {
    match value {
        Value::Undefined | Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => *n != 0.0 && !n.is_nan(),
        Value::String(text) => !text.is_empty(),
        Value::Js(_) => true,
    }
}

/// The handle in `value`, or an error naming what was expected.
fn handle<H>(value: Value<H>, name: &str) -> Result<H, Error> {
    match value {
        Value::Js(handle) => Ok(handle),
        _ => Err(Error::Script(format!("`{name}` is not an object"))),
    }
}

/// The object `owner[key]`.
fn object<H>(call: &mut dyn Call<H>, owner: &H, key: &str) -> Result<H, Error> {
    handle(call.get(owner, key)?, key)
}

/// Calls the method `owner[key]` with `args`.
fn method<H: Clone>(call: &mut dyn Call<H>, owner: &H, key: &str, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let function = object(call, owner, key)?;
    call.call(&function, &Value::Js(owner.clone()), args)
}

/// What every runtime starts with: the plain globals Paper.js reads as it loads, Acorn and
/// Paper.js. It makes no host functions, so V8 can start runtimes from a snapshot of it.
fn prelude<E: Engine>(engine: &mut E) -> Result<(), Error> {
    let global = engine.global()?;
    engine.with_call(|call| {
        call.set(&global, "self", &Value::Js(global.clone()))?;
        // Paper.js reads the platform from the user agent, for its key modifiers.
        let navigator = call.new_object()?;
        let user_agent = format!("VectorCraft ({})", std::env::consts::OS);
        call.set(&navigator, "userAgent", &Value::String(user_agent))?;
        set_language(call, &navigator, "en-US")?;
        call.set(&global, "navigator", &Value::Js(navigator))
    })?;
    engine.eval(ACORN)?;
    engine.eval(PAPER_JS)?;
    Ok(())
}

/// Sets `navigator.language` to `language`, and `navigator.languages` to a list of it.
fn set_language<H: Clone>(call: &mut dyn Call<H>, navigator: &H, language: &str) -> Result<(), Error> {
    call.set(navigator, "language", &Value::String(language.to_owned()))?;
    let languages = call.new_array(&[Value::String(language.to_owned())])?;
    call.set(navigator, "languages", &Value::Js(languages))
}

/// The host's globals Paper.js expects in a web worker, after the prelude: the timers
/// (`setInterval` for its frame loop) and `OffscreenCanvas` for CSS colours; and the browser's
/// `requestAnimationFrame`, `atob` and `btoa`.
fn install_globals<E: Engine>(engine: &mut E) -> Result<(), Error> {
    let global = engine.global()?;
    let set_interval = engine.function("setInterval", |call, _, args| set_timer(call, args, true))?;
    let set_timeout = engine.function("setTimeout", |call, _, args| set_timer(call, args, false))?;
    let clear_timer = engine.function("clearInterval", clear_timer)?;
    let request_frame = engine.function("requestAnimationFrame", request_frame)?;
    let cancel_frame = engine.function("cancelAnimationFrame", cancel_frame)?;
    let atob = engine.function("atob", atob)?;
    let btoa = engine.function("btoa", btoa)?;
    engine.with_call(|call| {
        call.set(&global, "setInterval", &Value::Js(set_interval))?;
        call.set(&global, "setTimeout", &Value::Js(set_timeout))?;
        // As in browsers, both clear either kind of timer.
        call.set(&global, "clearInterval", &Value::Js(clear_timer.clone()))?;
        call.set(&global, "clearTimeout", &Value::Js(clear_timer))?;
        call.set(&global, "requestAnimationFrame", &Value::Js(request_frame))?;
        call.set(&global, "cancelAnimationFrame", &Value::Js(cancel_frame))?;
        call.set(&global, "atob", &Value::Js(atob))?;
        call.set(&global, "btoa", &Value::Js(btoa))
    })?;
    canvas::install(engine, &global)
}

/// `atob(base64)`: the bytes it encodes, each as the character of that code, as in browsers.
fn atob<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = match args.first() {
        Some(text) => call.to_string(text)?,
        None => String::new(),
    };
    // Browsers accept the padding left out.
    let padded = format!("{text}{}", "=".repeat((4 - text.trim().len() % 4) % 4));
    let bytes = vectorcraft_format::base64_decode(&padded).ok_or_else(|| Error::Script("atob: not base64".into()))?;
    Ok(Value::String(bytes.into_iter().map(char::from).collect()))
}

/// `btoa(text)`: `text`'s characters as bytes, in base64; characters above 255 throw, as in
/// browsers.
fn btoa<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = match args.first() {
        Some(text) => call.to_string(text)?,
        None => String::new(),
    };
    let bytes = text
        .chars()
        .map(|c| u8::try_from(u32::from(c)).map_err(|_| Error::Script("btoa: a character above 255".into())))
        .collect::<Result<Vec<u8>, _>>()?;
    Ok(Value::String(vectorcraft_format::base64_encode(&bytes)))
}

/// `setInterval(callback, delay)` when it `repeats`, else `setTimeout(callback, delay)`.
pub(crate) fn set_timer<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], repeats: bool) -> Result<Value<H>, Error> {
    let Some(Value::Js(callback)) = args.first() else {
        let name = if repeats { "setInterval" } else { "setTimeout" };
        return Err(Error::Script(format!("{name}: the callback is not a function")));
    };
    let delay = match args.get(1) {
        Some(Value::Number(delay)) if delay.is_finite() => delay.max(0.0),
        _ => 0.0,
    };
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let id = host.next_interval_id.wrapping_add(1);
    host.next_interval_id = id;
    let due = host.now + delay;
    host.intervals.push(Interval { id, callback: callback.clone(), delay, due, repeats });
    Ok(Value::Number(f64::from(id)))
}

/// `requestAnimationFrame(callback)`: runs `callback` with the time on the next frame.
fn request_frame<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(callback)) = args.first() else {
        return Err(Error::Type("requestAnimationFrame: the callback is not a function".into()));
    };
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let id = host.next_interval_id.wrapping_add(1);
    host.next_interval_id = id;
    host.frames.push((id, callback.clone()));
    Ok(Value::Number(f64::from(id)))
}

fn cancel_frame<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Number(id)) = args.first() {
        call.host()?.borrow_mut().frames.retain(|(frame, _)| f64::from(*frame) != *id);
    }
    Ok(Value::Undefined)
}

fn clear_timer<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Number(id)) = args.first() {
        call.host()?.borrow_mut().intervals.retain(|interval| f64::from(interval.id) != *id);
    }
    Ok(Value::Undefined)
}
