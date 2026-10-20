//! `Worker` for scripts outside a browser: `new Worker(path)` runs the script at `path` (from the
//! script's folder, or an `https:` URL) as a classic script on a thread of its own, in a runtime of
//! its own, with the same APIs (and Paper.js, but no document). Messages go both ways as structured
//! serialization keeps them ([`crate::clone`]): `worker.postMessage(data)` fires `message` at the
//! worker's global object (`self.onmessage`), the worker's `postMessage(data)` at the `Worker`, on
//! its parent's turn ([`poll`]). An uncaught error in the worker fires `error` (an `ErrorEvent`) at
//! the `Worker`, and the worker goes on. The worker's console writes to its parent's. A worker
//! ends with `close()`, `worker.terminate()`, or its parent; while it runs, its parent does.
//!
//! In the worker, `self` is its global object, an `EventTarget`, with `name`, `postMessage()`,
//! `close()` and `importScripts()`. Module workers (`type: 'module'`) aren't supported.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::clone::{Data, deserialize, serialize};
use crate::{Call, Engine, Error, HostFunction, Interrupt, Runtime, Setup, Value, method, object};

/// How long a worker waits for a message before it runs its timers.
const FRAME: Duration = Duration::from_millis(16);

/// What a worker tells its parent.
enum FromWorker {
    Message(Data),
    Error(String),
    Output(Vec<String>),
    Closed,
}

/// A worker its parent started: its number (its object's `_worker`), its object, its thread's
/// channels, and what stops its script.
pub(crate) struct Child<H> {
    number: u32,
    object: H,
    to: Sender<Data>,
    from: Receiver<FromWorker>,
    interrupt: Arc<Mutex<Option<Interrupt>>>,
}

/// What a worker's runtime keeps of its parent: the channel to it, the worker's global object, and
/// whether the worker's script closed it.
pub(crate) struct Parent<H> {
    to: Sender<FromWorker>,
    global: H,
    closing: bool,
}

/// Sets `Worker` on `global`, unless it has it.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    if engine.with_call(|call| Ok(!matches!(call.get(global, "Worker")?, Value::Undefined)))? {
        return Ok(());
    }
    let constructor = engine.constructor("Worker", construct::<E>)?;
    let methods: [(&str, HostFunction<E::Handle>); 2] = [("postMessage", post_to_worker), ("terminate", terminate)];
    let mut made = Vec::new();
    for (name, f) in methods {
        made.push((name, engine.function(name, f)?));
    }
    engine.with_call(|call| {
        let event_target = object(call, global, "EventTarget")?;
        let event_target = call.get(&event_target, "prototype")?;
        let object_class = object(call, global, "Object")?;
        let Value::Js(prototype) = method(call, &object_class, "create", &[event_target])? else {
            return Err(Error::Script("Object.create made nothing".into()));
        };
        for (name, function) in made {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(global, "Worker", &Value::Js(constructor))?;
        crate::tag(call, global, &prototype, "Worker")
    })
}

/// `new Worker(path, options)`, on `this`, the object `new` made: its script runs on a thread.
fn construct<E: Engine>(call: &mut dyn Call<E::Handle>, this: &Value<E::Handle>, args: &[Value<E::Handle>]) -> Result<Value<E::Handle>, Error> {
    let Value::Js(worker) = this else {
        return Err(Error::Type("Worker needs 'new'".into()));
    };
    let path = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let mut name = String::new();
    if let Some(Value::Js(options)) = args.get(1) {
        if matches!(call.get(options, "type")?, Value::String(kind) if kind == "module") {
            return Err(Error::Dom("NotSupportedError", "module workers aren't supported".into()));
        }
        if let name_given @ Value::String(_) = call.get(options, "name")? {
            name = call.to_string(&name_given)?;
        }
    }
    let (file, directory, language) = {
        let host = call.host()?;
        let host = host.borrow();
        (host.file.as_ref().map(|f| f.to_string_lossy().into_owned()), host.directory.clone(), host.language.clone())
    };
    // A script that isn't there fires an error at the worker, as a page's does.
    let key = crate::modules::locate(&path, file.as_deref(), directory).map_err(|e| e.to_string());
    let (to, inbox) = channel();
    let (outbox, from) = channel();
    let interrupt = Arc::new(Mutex::new(None));
    let stop = interrupt.clone();
    std::thread::Builder::new()
        .name("papercraft worker".into())
        .spawn(move || run::<E>(key, &name, language, &inbox, &outbox, &stop))
        .map_err(|e| Error::Script(format!("Worker: no thread for it: {e}")))?;
    let number = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        host.next_child = host.next_child.wrapping_add(1);
        host.next_child
    };
    call.set(worker, "_worker", &Value::Number(f64::from(number)))?;
    for handler in ["onmessage", "onmessageerror", "onerror"] {
        call.set(worker, handler, &Value::Null)?;
    }
    call.host()?.borrow_mut().children.push(Child { number, object: worker.clone(), to, from, interrupt });
    Ok(Value::Undefined)
}

/// A worker's thread: runs the script at `key`, then the messages and timers, until it closes or
/// its parent is gone.
fn run<E: Engine>(
    key: Result<String, String>,
    name: &str,
    language: Option<String>,
    inbox: &Receiver<Data>,
    outbox: &Sender<FromWorker>,
    interrupt: &Arc<Mutex<Option<Interrupt>>>,
) {
    // The parent may be gone: then nobody listens.
    let report = |message: String| {
        let _ = outbox.send(FromWorker::Error(message));
    };
    let started = (|| -> Result<(Runtime<E>, String), String> {
        let key = key?;
        let source = crate::modules::source(&key)?;
        let mut runtime = Runtime::<E>::new().map_err(|e| e.to_string())?;
        let file = url::Url::parse(&key).is_err().then(|| key.clone());
        runtime.set_script(&Setup { file, language, ..Setup::default() }).map_err(|e| e.to_string())?;
        if let (Some(stop), Ok(mut slot)) = (runtime.interrupt(), interrupt.lock()) {
            *slot = Some(stop);
        }
        become_worker(&mut runtime, name, outbox.clone()).map_err(|e| e.to_string())?;
        Ok((runtime, source))
    })();
    let (mut runtime, source) = match started {
        Ok(started) => started,
        Err(e) => {
            report(e);
            let _ = outbox.send(FromWorker::Closed);
            return;
        }
    };
    if let Err(e) = runtime.eval(&source) {
        report(e.to_string());
    }
    let clock = Instant::now();
    loop {
        let output = runtime.take_output();
        if !output.is_empty() && outbox.send(FromWorker::Output(output)).is_err() {
            break;
        }
        if runtime.host.borrow().parent.as_ref().is_none_or(|p| p.closing) {
            break;
        }
        match inbox.recv_timeout(FRAME) {
            Ok(data) => {
                if let Err(e) = deliver(&mut runtime, &data) {
                    report(e.to_string());
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Err(e) = runtime.run_timers(clock.elapsed().as_secs_f64() * 1000.0) {
            report(e.to_string());
        }
    }
    let _ = outbox.send(FromWorker::Closed);
}

/// Gives the worker's global object what a worker's has: `self` is an `EventTarget`, with `name`,
/// `postMessage()`, `close()` and `importScripts()`.
fn become_worker<E: Engine>(runtime: &mut Runtime<E>, name: &str, to: Sender<FromWorker>) -> Result<(), Error> {
    let methods: [(&str, HostFunction<E::Handle>); 3] = [("postMessage", post_to_parent), ("close", close), ("importScripts", import_scripts)];
    let mut made = Vec::new();
    for (key, f) in methods {
        made.push((key, runtime.engine.function(key, f)?));
    }
    let global = runtime.engine.global()?;
    runtime.engine.with_call(|call| {
        for (key, function) in made {
            call.set(&global, key, &Value::Js(function))?;
        }
        call.set(&global, "name", &Value::String(name.to_owned()))?;
        for handler in ["onmessage", "onmessageerror"] {
            call.set(&global, handler, &Value::Null)?;
        }
        // Its listeners are kept as any EventTarget's.
        let event_target = object(call, &global, "EventTarget")?;
        let prototype = object(call, &event_target, "prototype")?;
        for key in ["addEventListener", "removeEventListener", "dispatchEvent"] {
            let function = call.get(&prototype, key)?;
            call.set(&global, key, &function)?;
        }
        call.host()?.borrow_mut().parent = Some(Parent { to, global: global.clone(), closing: false });
        Ok(())
    })
}

/// A message from the parent: a `message` event at the worker's global object, or `messageerror`
/// when it can't be made in this realm.
fn deliver<E: Engine>(runtime: &mut Runtime<E>, data: &Data) -> Result<(), Error> {
    let global = runtime.engine.global()?;
    runtime.engine.with_call(|call| {
        let target = Value::Js(global.clone());
        let event = match deserialize(call, data) {
            Ok(value) => crate::events::new_event(call, "MessageEvent", "message", &[("data", value)])?,
            Err(_) => crate::events::new_event(call, "MessageEvent", "messageerror", &[])?,
        };
        crate::events::fire(call, &target, &event).map(|_| ())
    })
}

/// The worker's `postMessage(data)`.
fn post_to_parent<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let data = serialize(call, args.first().unwrap_or(&Value::Undefined))?;
    if let Some(parent) = call.host()?.borrow().parent.as_ref() {
        // A parent that's gone takes nothing.
        let _ = parent.to.send(FromWorker::Message(data));
    }
    Ok(Value::Undefined)
}

/// The worker's `close()`: it ends once its script returns.
fn close<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(parent) = call.host()?.borrow_mut().parent.as_mut() {
        parent.closing = true;
    }
    Ok(Value::Undefined)
}

/// The worker's `importScripts(...paths)`: runs each script, in order, as a classic script.
fn import_scripts<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (file, directory, global) = {
        let host = call.host()?;
        let host = host.borrow();
        let global = host.parent.as_ref().map(|p| p.global.clone()).ok_or_else(|| Error::Script("importScripts: not in a worker".into()))?;
        (host.file.as_ref().map(|f| f.to_string_lossy().into_owned()), host.directory.clone(), global)
    };
    let eval = object(call, &global, "eval")?;
    for path in args {
        let path = call.to_string(path)?;
        let key = crate::modules::locate(&path, file.as_deref(), directory.clone()).map_err(|e| Error::Dom("NetworkError", e.to_string()))?;
        let source = crate::modules::source(&key).map_err(|e| Error::Dom("NetworkError", format!("importScripts '{path}': {e}")))?;
        // Called as itself, `eval` runs in the global scope, as a classic script does.
        call.call(&eval, &Value::Undefined, &[Value::String(source)])?;
    }
    Ok(Value::Undefined)
}

/// The child `this` is, as its number.
fn number<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Option<u32>, Error> {
    let Value::Js(worker) = this else {
        return Err(Error::Type("not a Worker".into()));
    };
    Ok(match call.get(worker, "_worker")? {
        Value::Number(n) => Some(n as u32),
        _ => None,
    })
}

/// The parent's `worker.postMessage(data)`.
fn post_to_worker<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, this)?;
    let data = serialize(call, args.first().unwrap_or(&Value::Undefined))?;
    if let Some(child) = call.host()?.borrow().children.iter().find(|c| Some(c.number) == number) {
        // A worker that ended takes nothing, as in browsers.
        let _ = child.to.send(data);
    }
    Ok(Value::Undefined)
}

/// `worker.terminate()`: stops its script and ends it.
fn terminate<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, this)?;
    let child = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        let at = host.children.iter().position(|c| Some(c.number) == number);
        at.map(|at| host.children.swap_remove(at))
    };
    if let Some(child) = child
        && let Ok(slot) = child.interrupt.lock()
        && let Some(stop) = slot.as_ref()
    {
        stop();
    }
    Ok(Value::Undefined)
}

/// Fires the events of what the workers sent, on the parent's turn, and passes on their output.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let arrived: Vec<(u32, H, Vec<FromWorker>)> = call
        .host()?
        .borrow()
        .children
        .iter()
        .map(|child| (child.number, child.object.clone(), std::iter::from_fn(|| child.from.try_recv().ok()).collect()))
        .filter(|(_, _, sent): &(u32, H, Vec<FromWorker>)| !sent.is_empty())
        .collect();
    for (number, worker, sent) in arrived {
        let target = Value::Js(worker);
        for message in sent {
            let event = match message {
                FromWorker::Output(lines) => {
                    call.host()?.borrow_mut().output.extend(lines);
                    continue;
                }
                FromWorker::Closed => {
                    call.host()?.borrow_mut().children.retain(|c| c.number != number);
                    break;
                }
                FromWorker::Message(data) => match deserialize(call, &data) {
                    Ok(value) => crate::events::new_event(call, "MessageEvent", "message", &[("data", value)])?,
                    Err(_) => crate::events::new_event(call, "MessageEvent", "messageerror", &[])?,
                },
                FromWorker::Error(message) => {
                    let event = crate::events::new_event(
                        call,
                        "ErrorEvent",
                        "error",
                        &[("message", Value::String(message.clone())), ("cancelable", Value::Bool(true))],
                    )?;
                    // Unless a handler prevents it, the error shows in the console too, as in browsers.
                    if crate::events::fire(call, &target, &event)? {
                        call.host()?.borrow_mut().output.push(format!("Uncaught (in worker) {message}"));
                    }
                    continue;
                }
            };
            crate::events::fire(call, &target, &event)?;
        }
    }
    Ok(())
}
