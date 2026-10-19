//! PaperCraft: PaperScript and the Paper.js API in VectorCraft.
//!
//! A [`Runtime`] loads Paper.js into a JavaScript [`Engine`]: V8 on desktop, the browser's engine
//! on the web, Boa where V8 isn't available. The globals Paper.js needs from its host (`self`,
//! `navigator`, `console`, `setInterval`) are written here once, over the engine interface. Each
//! [`Scope`] is a `PaperScope` with its own project and view, in which scripts run as PaperScript.

#![forbid(unsafe_code)]

mod binding;
mod canvas;
mod dialog;
mod engine;
mod engines;
mod palette;
pub mod source;
pub mod worker;
mod writeback;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use vectorcraft_doc::Document;

pub use binding::Patch;
pub use dialog::Ask;
pub use engine::{Call, Engine, HostFunction, Interrupt, Value};
#[cfg(feature = "boa")]
pub use engines::boa::Boa;
#[cfg(target_arch = "wasm32")]
pub use engines::browser::Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub use engines::v8::V8;
pub use palette::PaletteEvent;

/// The engine scripts run in on this platform: the browser's on the web, V8 on desktop, Boa where
/// V8 is left out.
#[cfg(target_arch = "wasm32")]
pub type PlatformEngine = Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub type PlatformEngine = V8;
#[cfg(all(feature = "boa", not(feature = "v8"), not(target_arch = "wasm32")))]
pub type PlatformEngine = Boa;

/// Paper.js with the PaperScript compiler and items backed by a host document: the vendored
/// `paper-sourced.js` build of Paper.js `develop` at 429dbb64 (MIT; no release has it yet), or the
/// build `PAPERCRAFT_PAPER_JS` names (see `build.rs`).
const PAPER_JS: &str = include_str!(env!("PAPERCRAFT_PAPER_JS_PATH"));

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A script threw, or Paper.js failed to load.
    #[error("{0}")]
    Script(String),
}

/// The state behind the host functions of one runtime.
pub struct Host<H> {
    /// Lines printed with `console.log`, kept until [`Runtime::take_output`].
    output: Vec<String>,
    next_interval_id: u32,
    /// The time of the last [`Runtime::run_timers`], in milliseconds.
    now: f64,
    intervals: Vec<Interval<H>>,
    /// The document the project reads from ([`Runtime::bind`]).
    bound: Option<binding::Bound>,
    /// The methods of every `OffscreenCanvas`, by name.
    canvas_methods: Vec<(&'static str, H)>,
    /// Answers the script's dialogs ([`Runtime::set_ask`]).
    ask: Option<dialog::Ask>,
    /// What `Dialog` uses of the script's realm.
    dialog_globals: Option<dialog::Globals<H>>,
    /// The palettes the script made, open.
    palettes: Vec<palette::Palette<H>>,
    next_palette_id: u64,
    /// `script.preferences` as last taken, as JSON.
    preferences: String,
}

struct Interval<H> {
    id: u32,
    callback: H,
    delay: f64,
    due: f64,
}

impl<H> Default for Host<H> {
    fn default() -> Self {
        Self {
            output: Vec::new(),
            next_interval_id: 0,
            now: 0.0,
            intervals: Vec::new(),
            bound: None,
            canvas_methods: Vec::new(),
            ask: None,
            dialog_globals: None,
            palettes: Vec::new(),
            next_palette_id: 0,
            preferences: String::new(),
        }
    }
}

/// Paper.js loaded into an engine.
pub struct Runtime<E: Engine> {
    engine: E,
    host: Rc<RefCell<Host<E::Handle>>>,
    paper: E::Handle,
}

/// A `PaperScope` with its own project and view.
pub struct Scope<H>(H);

impl<E: Engine> Runtime<E> {
    pub fn new() -> Result<Self, Error> {
        let host = Rc::new(RefCell::new(Host::default()));
        let mut engine = E::new(host.clone())?;
        install_globals(&mut engine, &host)?;
        engine.eval(PAPER_JS)?;
        let global = engine.global()?;
        let paper = engine.with_call(|call| object(call, &global, "paper"))?;
        let mut runtime = Self { engine, host, paper };
        dialog::install(&mut runtime, &global)?;
        palette::install(&mut runtime.engine, &global)?;
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

    /// Compiles and runs `source` as PaperScript in `scope`.
    pub fn execute(&mut self, scope: &Scope<E::Handle>, source: &str) -> Result<(), Error> {
        let paper = &self.paper;
        self.engine.with_call(|call| {
            let paperscript = object(call, paper, "PaperScript")?;
            method(call, &paperscript, "execute", &[Value::String(source.to_owned()), Value::Js(scope.0.clone())])?;
            Ok(())
        })
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

    /// Runs the `setInterval` callbacks that are due at `now`, in milliseconds since the runtime
    /// was created. The host calls this once per frame, so no interval runs more often than that.
    pub fn run_timers(&mut self, now: f64) -> Result<(), Error> {
        let mut due = Vec::new();
        {
            let mut host = self.host.borrow_mut();
            host.now = now;
            for interval in host.intervals.iter_mut().filter(|interval| interval.due <= now) {
                // Keep the cadence, but don't catch up on intervals missed while the host was busy.
                interval.due += interval.delay;
                if interval.due <= now {
                    interval.due = now + interval.delay;
                }
                due.push(interval.callback.clone());
            }
        }
        // Callbacks may set or clear intervals, so they run after the borrow ends.
        self.engine.with_call(|call| {
            for callback in due {
                call.call(&callback, &Value::Undefined, &[])?;
            }
            Ok(())
        })
    }

    /// Whether the scripts have something left to run: an interval, which `onFrame` uses too.
    pub fn is_running(&self) -> bool {
        !self.host.borrow().intervals.is_empty()
    }

    /// Sets `project.currentStyle` of `scope`: `style` as [`source::current_style`] makes it.
    pub fn set_current_style(&mut self, scope: &Scope<E::Handle>, style: &serde_json::Value) -> Result<(), Error> {
        self.engine.with_call(|call| {
            let globals = dialog::globals(call)?;
            let style = dialog::from_json(call, &globals, style)?;
            let project = object(call, &scope.0, "project")?;
            call.set(&project, "currentStyle", &style)
        })
    }

    /// Sets the global `script`, as Scriptographer's: `script.file` and `script.directory`, the
    /// script's file and its folder (`null` for code typed in the console), `script.keepAlive`,
    /// which the script sets to keep running when another script runs, and
    /// `script.preferences`, `preferences` as the script last left them
    /// ([`Runtime::take_preferences`]).
    pub fn set_script(&mut self, file: Option<&str>, preferences: &serde_json::Value) -> Result<(), Error> {
        let global = self.engine.global()?;
        self.host.borrow_mut().preferences = preferences.to_string();
        self.engine.with_call(|call| {
            let script = call.new_object()?;
            let path = |p: Option<&str>| p.map_or(Value::Null, |p| Value::String(p.to_owned()));
            let directory = file.and_then(|f| std::path::Path::new(f).parent()).map(|d| d.to_string_lossy().into_owned());
            call.set(&script, "file", &path(file))?;
            call.set(&script, "directory", &path(directory.as_deref()))?;
            call.set(&script, "keepAlive", &Value::Bool(false))?;
            let preferences = match preferences {
                serde_json::Value::Object(_) => preferences.clone(),
                _ => serde_json::json!({}),
            };
            let globals = dialog::globals(call)?;
            let preferences = dialog::from_json(call, &globals, &preferences)?;
            call.set(&script, "preferences", &preferences)?;
            call.set(&global, "script", &Value::Js(script))
        })
    }

    /// `script.preferences` as JSON, when it changed since the last call (or [`Runtime::set_script`]).
    pub fn take_preferences(&mut self) -> Result<Option<serde_json::Value>, Error> {
        let global = self.engine.global()?;
        let preferences = self.engine.with_call(|call| {
            let Value::Js(script) = call.get(&global, "script")? else {
                return Ok(serde_json::Value::Null);
            };
            let preferences = call.get(&script, "preferences")?;
            let globals = dialog::globals(call)?;
            dialog::to_json(call, &globals, &preferences)
        })?;
        let text = preferences.to_string();
        let mut host = self.host.borrow_mut();
        if host.preferences == text {
            return Ok(None);
        }
        host.preferences = text;
        Ok(Some(preferences))
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
    /// say, and so do the handlers of the items and the view.
    pub fn mouse_event(&mut self, scope: &Scope<E::Handle>, event: MouseEvent, point: (f64, f64), modifiers: Modifiers) -> Result<(), Error> {
        let paper = &self.paper;
        self.engine.with_call(|call| {
            let options = event_options(call, modifiers)?;
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
fn event_options<H>(call: &mut dyn Call<H>, modifiers: Modifiers) -> Result<H, Error> {
    let held = call.new_object()?;
    for (name, down) in [("shift", modifiers.shift), ("control", modifiers.control), ("alt", modifiers.alt), ("meta", modifiers.meta)] {
        call.set(&held, name, &Value::Bool(down))?;
    }
    let options = call.new_object()?;
    call.set(&options, "modifiers", &Value::Js(held))?;
    Ok(options)
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

/// The globals Paper.js expects in a web worker: `self` (the global object), `navigator`,
/// `console`, `setInterval` / `clearInterval` for its frame loop, and `OffscreenCanvas` for CSS
/// colours.
fn install_globals<E: Engine>(engine: &mut E, host: &Rc<RefCell<Host<E::Handle>>>) -> Result<(), Error> {
    let global = engine.global()?;
    let log = engine.function("log", console_log)?;
    let set_interval = engine.function("setInterval", set_interval)?;
    let clear_interval = engine.function("clearInterval", clear_interval)?;
    engine.with_call(|call| {
        call.set(&global, "self", &Value::Js(global.clone()))?;
        // Paper.js reads the platform from the user agent, for its key modifiers.
        let navigator = call.new_object()?;
        let user_agent = format!("VectorCraft ({})", std::env::consts::OS);
        call.set(&navigator, "userAgent", &Value::String(user_agent))?;
        call.set(&global, "navigator", &Value::Js(navigator))?;
        let console = call.new_object()?;
        call.set(&console, "log", &Value::Js(log))?;
        call.set(&global, "console", &Value::Js(console))?;
        call.set(&global, "setInterval", &Value::Js(set_interval))?;
        call.set(&global, "clearInterval", &Value::Js(clear_interval))
    })?;
    canvas::install(engine, &global, host)
}

fn console_log<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(call.to_string(arg)?);
    }
    call.host()?.borrow_mut().output.push(parts.join(" "));
    Ok(Value::Undefined)
}

fn set_interval<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(callback)) = args.first() else {
        return Err(Error::Script("setInterval: the callback is not a function".into()));
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
    host.intervals.push(Interval { id, callback: callback.clone(), delay, due });
    Ok(Value::Number(f64::from(id)))
}

fn clear_interval<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Number(id)) = args.first() {
        call.host()?.borrow_mut().intervals.retain(|interval| f64::from(interval.id) != *id);
    }
    Ok(Value::Undefined)
}
