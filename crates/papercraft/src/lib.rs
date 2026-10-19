//! PaperCraft: PaperScript and the Paper.js API in VectorCraft.
//!
//! A [`Runtime`] loads Paper.js into a JavaScript [`Engine`]: V8 on desktop, the browser's engine
//! on the web, Boa where V8 isn't available. The globals Paper.js needs from its host (`self`,
//! `navigator`, `console`, `setInterval`) are written here once, over the engine interface. Each
//! [`Scope`] is a `PaperScope` with its own project and view, in which scripts run as PaperScript.

#![forbid(unsafe_code)]

mod canvas;
mod engine;
mod engines;

use std::cell::RefCell;
use std::rc::Rc;

pub use engine::{Call, Engine, HostFunction, Value};
#[cfg(feature = "boa")]
pub use engines::boa::Boa;
#[cfg(target_arch = "wasm32")]
pub use engines::browser::Browser;
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
pub use engines::v8::V8;

/// Paper.js with the PaperScript compiler: the vendored 0.13.0 release (`dist/paper-full.js` of the
/// npm package, MIT), or the build `PAPERCRAFT_PAPER_JS` names (see `build.rs`).
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
        Self { output: Vec::new(), next_interval_id: 0, now: 0.0, intervals: Vec::new(), canvas_methods: Vec::new() }
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
        let paper = handle(engine.get(&global, "paper")?, "paper")?;
        Ok(Self { engine, host, paper })
    }

    /// Creates a scope whose view has the given size.
    pub fn create_scope(&mut self, width: f64, height: f64) -> Result<Scope<E::Handle>, Error> {
        let engine = &mut self.engine;
        let constructor = handle(engine.get(&self.paper, "PaperScope")?, "PaperScope")?;
        let scope = handle(engine.construct(&constructor, &[])?, "PaperScope")?;
        let size = handle(engine.get(&self.paper, "Size")?, "Size")?;
        let size = engine.construct(&size, &[Value::Number(width), Value::Number(height)])?;
        let setup = handle(engine.get(&scope, "setup")?, "setup")?;
        engine.call(&setup, &Value::Js(scope.clone()), &[size])?;
        Ok(Scope(scope))
    }

    /// Compiles and runs `source` as PaperScript in `scope`.
    pub fn execute(&mut self, scope: &Scope<E::Handle>, source: &str) -> Result<(), Error> {
        // `PaperScope#execute()` would also update the browser's keyboard focus, which workers
        // don't have.
        let engine = &mut self.engine;
        let paperscript = handle(engine.get(&self.paper, "PaperScript")?, "PaperScript")?;
        let execute = handle(engine.get(&paperscript, "execute")?, "execute")?;
        let args = [Value::String(source.to_owned()), Value::Js(scope.0.clone())];
        engine.call(&execute, &Value::Js(paperscript), &args)?;
        Ok(())
    }

    /// The scope's project as Paper.js JSON (`project.exportJSON()`).
    pub fn export_json(&mut self, scope: &Scope<E::Handle>) -> Result<String, Error> {
        let engine = &mut self.engine;
        let project = handle(engine.get(&scope.0, "project")?, "project")?;
        let export = handle(engine.get(&project, "exportJSON")?, "exportJSON")?;
        match engine.call(&export, &Value::Js(project), &[])? {
            Value::String(json) => Ok(json),
            _ => Err(Error::Script("`exportJSON()` didn't return a string".into())),
        }
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
        for callback in due {
            self.engine.call(&callback, &Value::Undefined, &[])?;
        }
        Ok(())
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

/// The globals Paper.js expects in a web worker: `self` (the global object), `navigator`,
/// `console`, `setInterval` / `clearInterval` for its frame loop, and `OffscreenCanvas` for CSS
/// colours.
fn install_globals<E: Engine>(engine: &mut E, host: &Rc<RefCell<Host<E::Handle>>>) -> Result<(), Error> {
    let global = engine.global()?;
    engine.set(&global, "self", &Value::Js(global.clone()))?;
    // Paper.js reads the platform from the user agent, for its key modifiers.
    let navigator = engine.new_object()?;
    let user_agent = format!("VectorCraft ({})", std::env::consts::OS);
    engine.set(&navigator, "userAgent", &Value::String(user_agent))?;
    engine.set(&global, "navigator", &Value::Js(navigator))?;
    let console = engine.new_object()?;
    let log = engine.function("log", console_log)?;
    engine.set(&console, "log", &Value::Js(log))?;
    engine.set(&global, "console", &Value::Js(console))?;
    let set_interval = engine.function("setInterval", set_interval)?;
    engine.set(&global, "setInterval", &Value::Js(set_interval))?;
    let clear_interval = engine.function("clearInterval", clear_interval)?;
    engine.set(&global, "clearInterval", &Value::Js(clear_interval))?;
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
