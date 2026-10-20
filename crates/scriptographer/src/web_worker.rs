//! The workers of `Worker` outside a browser (`js/src/worker.js`): `new Worker(path)` runs the
//! script at `path` (from the script's folder, or an `https:` URL) as a classic script on a thread
//! of its own, in a runtime of its own, with the same APIs (and Paper.js, but no document).
//! Messages go both ways as structured serialization keeps them ([`crate::clone`]), on the
//! receiver's turn ([`poll`] in the parent). An uncaught error in the worker goes to the `Worker`,
//! and the worker goes on. The worker's console writes to its parent's. A worker ends with
//! `close()`, `worker.terminate()`, or its parent; while it runs, its parent does.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::clone::{Data, deserialize, serialize};
use crate::{Call, Engine, Error, HostFunction, Interrupt, Runtime, Setup, Value};

/// How long a worker waits for a message before it runs its timers.
const FRAME: Duration = Duration::from_millis(16);

/// What a worker tells its parent.
enum FromWorker {
    Message(Data),
    Error(String),
    Output(Vec<String>),
    Closed,
}

/// A worker its parent started: its number, the function its `Worker` receives with, its thread's
/// channels, and what stops its script.
pub(crate) struct Child<H> {
    number: u32,
    receive: H,
    to: Sender<Data>,
    from: Receiver<FromWorker>,
    interrupt: Arc<Mutex<Option<Interrupt>>>,
}

/// What a worker's runtime keeps of its parent: the channel to it, the function its global object
/// receives with, and whether the worker's script closed it.
pub(crate) struct Parent<H> {
    to: Sender<FromWorker>,
    receive: H,
    closing: bool,
}

/// The natives `js/src/worker.js` talks through: `start(url, name, receive)`, which gives the
/// worker's number, `post(number, data)` and `terminate(number)`, and in the worker
/// `self.postMessage(data)`, `self.close()` and `self.importScripts(...paths)`.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 3] = [("start", start::<E>), ("post", post_to_worker), ("terminate", terminate)];
    let mut made = natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let worker: [(&str, HostFunction<E::Handle>); 3] = [("postMessage", post_to_parent), ("close", close), ("importScripts", import_scripts)];
    let worker = worker.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let self_natives = engine.with_call(|call| {
        let object = call.new_object()?;
        for (name, function) in worker {
            call.set(&object, name, &Value::Js(function))?;
        }
        Ok(object)
    })?;
    made.push(("self", self_natives));
    Ok(made)
}

/// Runs the script at `url` on a thread.
fn start<E: Engine>(call: &mut dyn Call<E::Handle>, _this: &Value<E::Handle>, args: &[Value<E::Handle>]) -> Result<Value<E::Handle>, Error> {
    let path = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let name = call.to_string(args.get(1).unwrap_or(&Value::Undefined))?;
    let Some(Value::Js(receive)) = args.get(2) else {
        return Err(Error::Type("Worker: nothing to receive with".into()));
    };
    let (file, directory, language, cache) = {
        let host = call.host()?;
        let host = host.borrow();
        (host.file.as_ref().map(|f| f.to_string_lossy().into_owned()), host.directory.clone(), host.language.clone(), host.cache.clone())
    };
    // A script that isn't there fires an error at the worker, as a page's does.
    let key = crate::modules::locate(&path, file.as_deref(), directory).map_err(|e| e.to_string());
    let (to, inbox) = channel();
    let (outbox, from) = channel();
    let interrupt = Arc::new(Mutex::new(None));
    let stop = interrupt.clone();
    std::thread::Builder::new()
        .name("scriptographer worker".into())
        .spawn(move || run::<E>(key, &name, Setup { language, cache, ..Setup::default() }, &inbox, &outbox, &stop))
        .map_err(|e| Error::Script(format!("Worker: no thread for it: {e}")))?;
    let host = call.host()?;
    let mut host = host.borrow_mut();
    host.next_child = host.next_child.wrapping_add(1);
    let number = host.next_child;
    host.children.push(Child { number, receive: receive.clone(), to, from, interrupt });
    Ok(Value::Number(f64::from(number)))
}

/// A worker's thread: runs the script at `key`, then the messages and timers, until it closes or
/// its parent is gone.
fn run<E: Engine>(
    key: Result<String, String>,
    name: &str,
    setup: Setup,
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
        let source = crate::modules::source(&key, setup.cache.as_deref())?;
        let mut runtime = Runtime::<E>::new().map_err(|e| e.to_string())?;
        let file = url::Url::parse(&key).is_err().then(|| key.clone());
        runtime.set_script(&Setup { file, ..setup }).map_err(|e| e.to_string())?;
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

/// Gives the worker's global object what a worker's has (`setupWorker()` in `js/src/worker.js`).
fn become_worker<E: Engine>(runtime: &mut Runtime<E>, name: &str, to: Sender<FromWorker>) -> Result<(), Error> {
    runtime.engine.with_call(|call| {
        let receive = crate::handle(crate::call_bundle(call, "setupWorker", &[Value::String(name.to_owned())])?, "setupWorker()")?;
        call.host()?.borrow_mut().parent = Some(Parent { to, receive, closing: false });
        Ok(())
    })
}

/// A message from the parent, to the worker's global object: `message`, or `messageerror` when it
/// can't be made in this realm.
fn deliver<E: Engine>(runtime: &mut Runtime<E>, data: &Data) -> Result<(), Error> {
    runtime.engine.with_call(|call| {
        let receive = call.host()?.borrow().parent.as_ref().map(|p| p.receive.clone()).ok_or_else(|| Error::Script("not a worker".into()))?;
        let sent = message(call, data)?;
        call.call(&receive, &Value::Undefined, &[sent]).map(|_| ())
    })
}

/// `{ type: 'message', data }` for `data`, or `{ type: 'messageerror' }`.
fn message<H: Clone>(call: &mut dyn Call<H>, data: &Data) -> Result<Value<H>, Error> {
    let object = call.new_object()?;
    match deserialize(call, data) {
        Ok(value) => {
            call.set(&object, "type", &Value::String("message".into()))?;
            call.set(&object, "data", &value)?;
        }
        Err(_) => call.set(&object, "type", &Value::String("messageerror".into()))?,
    }
    Ok(Value::Js(object))
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
    let (file, directory, cache) = {
        let host = call.host()?;
        let host = host.borrow();
        (host.file.as_ref().map(|f| f.to_string_lossy().into_owned()), host.directory.clone(), host.cache.clone())
    };
    let eval = crate::global_class(call, "eval")?;
    for path in args {
        let path = call.to_string(path)?;
        let key = crate::modules::locate(&path, file.as_deref(), directory.clone()).map_err(|e| Error::Dom("NetworkError", e.to_string()))?;
        let source =
            crate::modules::source(&key, cache.as_deref()).map_err(|e| Error::Dom("NetworkError", format!("importScripts '{path}': {e}")))?;
        // Called as itself, `eval` runs in the global scope, as a classic script does.
        call.call(&eval, &Value::Undefined, &[Value::String(source)])?;
    }
    Ok(Value::Undefined)
}

/// The worker `args[0]` numbers.
fn number<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<f64, Error> {
    call.to_number(args.first().unwrap_or(&Value::Undefined))
}

/// The parent's `worker.postMessage(data)`.
fn post_to_worker<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, args)?;
    let data = serialize(call, args.get(1).unwrap_or(&Value::Undefined))?;
    if let Some(child) = call.host()?.borrow().children.iter().find(|c| f64::from(c.number) == number) {
        // A worker that ended takes nothing, as in browsers.
        let _ = child.to.send(data);
    }
    Ok(Value::Undefined)
}

/// `worker.terminate()`: stops its script and ends it.
fn terminate<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, args)?;
    let child = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        let at = host.children.iter().position(|c| f64::from(c.number) == number);
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

/// Hands what the workers sent to their `Worker`s, on the parent's turn, and passes on their
/// output.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let arrived: Vec<(u32, H, Vec<FromWorker>)> = call
        .host()?
        .borrow()
        .children
        .iter()
        .map(|child| (child.number, child.receive.clone(), std::iter::from_fn(|| child.from.try_recv().ok()).collect()))
        .filter(|(_, _, sent): &(u32, H, Vec<FromWorker>)| !sent.is_empty())
        .collect();
    for (number, receive, sent) in arrived {
        for sent in sent {
            let sent = match sent {
                FromWorker::Output(lines) => {
                    call.host()?.borrow_mut().output.extend(lines);
                    continue;
                }
                FromWorker::Closed => {
                    call.host()?.borrow_mut().children.retain(|c| c.number != number);
                    break;
                }
                FromWorker::Message(data) => message(call, &data)?,
                FromWorker::Error(message) => {
                    let error = call.new_object()?;
                    call.set(&error, "type", &Value::String("error".into()))?;
                    call.set(&error, "message", &Value::String(message))?;
                    Value::Js(error)
                }
            };
            call.call(&receive, &Value::Undefined, &[sent])?;
        }
    }
    Ok(())
}
