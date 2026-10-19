//! PaperCraft: PaperScript and the Paper.js API in VectorCraft.
//!
//! A [`Runtime`] loads Paper.js into a JavaScript [`Engine`]: V8 on desktop, the browser's engine
//! on the web, Boa where V8 isn't available. The globals Paper.js needs from its host (`self`,
//! `navigator`, `console`, `setInterval`) are written here once, over the engine interface. Each
//! [`Scope`] is a `PaperScope` with its own project and view, in which scripts run as PaperScript.

#![forbid(unsafe_code)]

mod binding;
mod canvas;
mod engine;
mod engines;
pub mod source;
mod writeback;

use std::cell::RefCell;
use std::rc::Rc;

pub use binding::Patch;
pub use engine::{Call, Engine, HostFunction, Value};
#[cfg(feature = "boa")]
pub use engines::boa::Boa;
#[cfg(target_arch = "wasm32")]
pub use engines::browser::Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub use engines::v8::V8;

/// The engine scripts run in on this platform: the browser's on the web, V8 on desktop, Boa where
/// V8 is left out.
#[cfg(target_arch = "wasm32")]
pub type PlatformEngine = Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub type PlatformEngine = V8;
#[cfg(all(feature = "boa", not(feature = "v8"), not(target_arch = "wasm32")))]
pub type PlatformEngine = Boa;

/// Paper.js with the PaperScript compiler and items backed by a host document: the vendored
/// `paper-sourced.js` build of Paper.js `develop` at 57bd9b51 (MIT; no release has it yet), or the
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
}

struct Interval<H> {
    id: u32,
    callback: H,
    delay: f64,
    due: f64,
}

impl<H> Default for Host<H> {
    fn default() -> Self {
        Self { output: Vec::new(), next_interval_id: 0, now: 0.0, intervals: Vec::new(), bound: None, canvas_methods: Vec::new() }
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
        Ok(Self { engine, host, paper })
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

    /// Returns the lines printed with `console.log` since the last call.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.host.borrow_mut().output)
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
