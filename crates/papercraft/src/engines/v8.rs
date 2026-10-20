//! V8, the engine of Chrome and Node.js, through Deno's `v8` crate.
//!
//! Each runtime is a V8 context in its thread's one isolate. V8 enters an isolate when it makes
//! one, and the isolates of a thread must be dropped in the reverse order of their creation, which
//! runtimes that start and stop in any order (running scripts) can't keep: so they share one,
//! made on first use and kept for the thread's lifetime. A context keeps its runtime's host state
//! in a slot.
//!
//! The first engine of a process runs the prelude in an isolate of its own and keeps a snapshot of
//! its heap, which every isolate then starts from: a context begins with the prelude done, without
//! running it again.

use std::cell::RefCell;
use std::pin::pin;
use std::rc::Rc;
use std::sync::{Once, OnceLock};

use crate::{Call, Engine, Error, Host, HostFunction, Interrupt, Prelude, Value};

type Handle = v8::Global<v8::Value>;
type V8Host = Rc<RefCell<Host<Handle>>>;

thread_local! {
    /// The thread's isolate, shared by its runtimes.
    static ISOLATE: RefCell<Option<v8::OwnedIsolate>> = const { RefCell::new(None) };
}

/// The snapshot of the prelude, made with the first engine; none when V8 couldn't make one, and
/// each context runs the prelude itself then.
static SNAPSHOT: OnceLock<Option<Vec<u8>>> = OnceLock::new();

/// What a context keeps for its runtime: the host state, and the host functions, each function's
/// data its index here.
struct ContextData {
    host: V8Host,
    functions: RefCell<Vec<HostFunctionEntry>>,
}

#[derive(Clone, Copy)]
struct HostFunctionEntry {
    function: HostFunction<Handle>,
    /// Called with `this` undefined, as in the other engines.
    constructor: bool,
}

pub struct V8 {
    context: v8::Global<v8::Context>,
}

/// Starts V8, once per process.
fn initialize() {
    static INITIALIZE: Once = Once::new();
    INITIALIZE.call_once(|| {
        v8::V8::initialize_platform(v8::new_default_platform(0, false).make_shared());
        v8::V8::initialize();
    });
}

/// Runs `f` with the thread's isolate, made on first use, from the snapshot when there is one.
fn with_isolate<R>(f: impl FnOnce(&mut v8::OwnedIsolate) -> Result<R, Error>) -> Result<R, Error> {
    initialize();
    ISOLATE.with(|cell| {
        // A runtime's call into V8 never makes another one, so the isolate is never in use here.
        let mut cell = cell.try_borrow_mut().map_err(|_| Error::Script("the V8 isolate is in use".into()))?;
        f(cell.get_or_insert_with(|| {
            let params = match SNAPSHOT.get() {
                Some(Some(snapshot)) => v8::CreateParams::default().snapshot_blob(snapshot.as_slice().into()),
                _ => v8::CreateParams::default(),
            };
            v8::Isolate::new(params)
        }))
    })
}

/// A snapshot of an isolate whose default context ran `prelude`, made on a thread of its own, whose
/// isolate is the snapshot's.
fn snapshot(prelude: Prelude<V8>) -> Option<Vec<u8>> {
    let thread = std::thread::spawn(move || {
        initialize();
        ISOLATE.with(|cell| cell.replace(Some(v8::Isolate::snapshot_creator(None, None))));
        let mut engine = V8::with_context(Rc::new(RefCell::new(Host::default()))).ok()?;
        prelude(&mut engine).ok()?;
        with_isolate(|isolate| {
            let scope = pin!(v8::HandleScope::new(isolate));
            let scope = &mut scope.init();
            let context = v8::Local::new(scope, &engine.context);
            // Slots hold Rust values, which a snapshot can't.
            context.clear_all_slots();
            scope.set_default_context(context);
            Ok(())
        })
        .ok()?;
        // The blob is made without handles into the heap.
        drop(engine);
        let isolate = ISOLATE.with(|cell| cell.borrow_mut().take())?;
        isolate.create_blob(v8::FunctionCodeHandling::Keep).map(|blob| blob.to_vec())
    });
    thread.join().ok().flatten()
}

/// Opens a handle scope in `$isolate`, enters `$context` and catches exceptions in `$scope`.
macro_rules! enter {
    ($isolate:expr, $context:expr, $scope:ident) => {
        let scope = pin!(v8::HandleScope::new($isolate));
        let scope = &mut scope.init();
        let context = v8::Local::new(scope, $context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let $scope = pin!(v8::TryCatch::new(scope));
        let $scope = &mut $scope.init();
    };
}

impl Engine for V8 {
    type Handle = Handle;

    fn new(host: V8Host, prelude: Prelude<Self>) -> Result<Self, Error> {
        let snapshot = SNAPSHOT.get_or_init(|| snapshot(prelude));
        let mut engine = Self::with_context(host)?;
        if snapshot.is_none() {
            prelude(&mut engine)?;
        }
        Ok(engine)
    }

    fn eval(&mut self, source: &str) -> Result<Value<Handle>, Error> {
        with_isolate(|isolate| {
            enter!(isolate, &self.context, scope);
            let source = string(scope, source)?;
            let result = v8::Script::compile(scope, source, None).and_then(|script| script.run(scope));
            let result = result.ok_or_else(|| exception(scope))?;
            Ok(from_v8(scope, result))
        })
    }

    fn global(&mut self) -> Result<Handle, Error> {
        with_isolate(|isolate| {
            enter!(isolate, &self.context, scope);
            let global = scope.get_current_context().global(scope);
            Ok(handle(scope, global.into()))
        })
    }

    fn function(&mut self, name: &str, function: HostFunction<Handle>) -> Result<Handle, Error> {
        self.host_function(name, function, false)
    }

    fn constructor(&mut self, name: &str, function: HostFunction<Handle>) -> Result<Handle, Error> {
        self.host_function(name, function, true)
    }

    fn with_call<R>(&mut self, f: impl FnOnce(&mut dyn Call<Handle>) -> Result<R, Error>) -> Result<R, Error> {
        with_isolate(|isolate| {
            enter!(isolate, &self.context, scope);
            f(&mut V8Call { scope })
        })
    }

    fn interrupt(&mut self) -> Option<Interrupt> {
        let handle = with_isolate(|isolate| Ok(isolate.thread_safe_handle())).ok()?;
        Some(Box::new(move || {
            // False once the isolate is gone: then nothing runs that could be stopped.
            let _ = handle.terminate_execution();
        }))
    }
}

impl V8 {
    /// An engine with a new context of the thread's isolate.
    fn with_context(host: V8Host) -> Result<Self, Error> {
        with_isolate(|isolate| {
            let scope = pin!(v8::HandleScope::new(isolate));
            let scope = &mut scope.init();
            let context = v8::Context::new(scope, Default::default());
            context.set_slot(Rc::new(ContextData { host, functions: RefCell::new(Vec::new()) }));
            Ok(Self { context: v8::Global::new(scope, context) })
        })
    }

    fn host_function(&mut self, name: &str, function: HostFunction<Handle>, constructor: bool) -> Result<Handle, Error> {
        with_isolate(|isolate| {
            enter!(isolate, &self.context, scope);
            let data = context_data(scope)?;
            let index = {
                let mut functions = data.functions.borrow_mut();
                functions.push(HostFunctionEntry { function, constructor });
                i32::try_from(functions.len() - 1).map_err(|_| Error::Script("too many host functions".into()))?
            };
            let data = v8::Integer::new(scope, index);
            let behavior = if constructor { v8::ConstructorBehavior::Allow } else { v8::ConstructorBehavior::Throw };
            let function =
                v8::Function::builder(call_host).data(data.into()).constructor_behavior(behavior).build(scope).ok_or_else(|| exception(scope))?;
            function.set_name(string(scope, name)?);
            Ok(handle(scope, function.into()))
        })
    }
}

/// The runtime state of the context `scope` is in.
fn context_data(scope: &v8::PinScope<'_, '_>) -> Result<Rc<ContextData>, Error> {
    scope.get_current_context().get_slot::<ContextData>().ok_or_else(|| missing("runtime"))
}

/// The engine's values, inside a host function or through [`Engine::with_call`].
struct V8Call<'a, 's, 'i> {
    scope: &'a mut v8::PinScope<'s, 'i>,
}

impl Call<Handle> for V8Call<'_, '_, '_> {
    fn host(&self) -> Result<V8Host, Error> {
        Ok(context_data(self.scope)?.host.clone())
    }

    fn to_string(&mut self, value: &Value<Handle>) -> Result<String, Error> {
        let value = to_v8(self.scope, value)?;
        let string = value.to_string(self.scope).ok_or_else(|| Error::Script("can't convert to a string".into()))?;
        Ok(string.to_rust_string_lossy(self.scope))
    }

    fn new_object(&mut self) -> Result<Handle, Error> {
        let object = v8::Object::new(self.scope);
        Ok(handle(self.scope, object.into()))
    }

    fn new_array(&mut self, elements: &[Value<Handle>]) -> Result<Handle, Error> {
        let elements = elements.iter().map(|element| to_v8(self.scope, element)).collect::<Result<Vec<_>, _>>()?;
        let array = v8::Array::new_with_elements(self.scope, &elements);
        Ok(handle(self.scope, array.into()))
    }

    fn new_float64_array(&mut self, elements: &[f64]) -> Result<Handle, Error> {
        let bytes = elements.iter().flat_map(|e| e.to_ne_bytes()).collect();
        let buffer = buffer(self.scope, bytes);
        let array = v8::Float64Array::new(self.scope, buffer, 0, elements.len()).ok_or_else(|| thrown("Float64Array"))?;
        Ok(handle(self.scope, array.into()))
    }

    fn new_uint8_array(&mut self, elements: &[u8]) -> Result<Handle, Error> {
        let buffer = buffer(self.scope, elements.to_vec());
        let array = v8::Uint8Array::new(self.scope, buffer, 0, elements.len()).ok_or_else(|| thrown("Uint8Array"))?;
        Ok(handle(self.scope, array.into()))
    }

    fn bytes(&mut self, object: &Handle) -> Result<Vec<u8>, Error> {
        let value = v8::Local::new(self.scope, object);
        // An `ArrayBuffer` through a view of all of it.
        let view = match v8::Local::<v8::ArrayBuffer>::try_from(value) {
            Ok(buffer) => v8::Uint8Array::new(self.scope, buffer, 0, buffer.byte_length()).map(Into::into),
            Err(_) => v8::Local::<v8::ArrayBufferView>::try_from(value).ok(),
        };
        let view: v8::Local<v8::ArrayBufferView> = view.ok_or_else(|| thrown("not a typed array, DataView or ArrayBuffer"))?;
        let mut bytes = vec![0; view.byte_length()];
        view.copy_contents(&mut bytes);
        Ok(bytes)
    }

    fn new_uint16_array(&mut self, elements: &[u16]) -> Result<Handle, Error> {
        let bytes = elements.iter().flat_map(|e| e.to_ne_bytes()).collect();
        let buffer = buffer(self.scope, bytes);
        let array = v8::Uint16Array::new(self.scope, buffer, 0, elements.len()).ok_or_else(|| thrown("Uint16Array"))?;
        Ok(handle(self.scope, array.into()))
    }

    fn get(&mut self, object: &Handle, key: &str) -> Result<Value<Handle>, Error> {
        let scope = pin!(v8::TryCatch::new(self.scope));
        let scope = &mut scope.init();
        let object = v8::Local::new(scope, object).to_object(scope).ok_or_else(|| exception(scope))?;
        let key = string(scope, key)?;
        let value = object.get(scope, key.into()).ok_or_else(|| exception(scope))?;
        Ok(from_v8(scope, value))
    }

    fn set(&mut self, object: &Handle, key: &str, value: &Value<Handle>) -> Result<(), Error> {
        let scope = pin!(v8::TryCatch::new(self.scope));
        let scope = &mut scope.init();
        let object = v8::Local::new(scope, object).to_object(scope).ok_or_else(|| exception(scope))?;
        let key = string(scope, key)?;
        let value = to_v8(scope, value)?;
        object.set(scope, key.into(), value).ok_or_else(|| exception(scope))?;
        Ok(())
    }

    fn call(&mut self, function: &Handle, this: &Value<Handle>, args: &[Value<Handle>]) -> Result<Value<Handle>, Error> {
        let scope = pin!(v8::TryCatch::new(self.scope));
        let scope = &mut scope.init();
        let function = v8::Local::<v8::Function>::try_from(v8::Local::new(scope, function)).map_err(|_| Error::Script("not a function".into()))?;
        let this = to_v8(scope, this)?;
        let args = args.iter().map(|arg| to_v8(scope, arg)).collect::<Result<Vec<_>, _>>()?;
        let result = function.call(scope, this, &args).ok_or_else(|| exception(scope))?;
        Ok(from_v8(scope, result))
    }

    fn construct(&mut self, constructor: &Handle, args: &[Value<Handle>]) -> Result<Value<Handle>, Error> {
        let scope = pin!(v8::TryCatch::new(self.scope));
        let scope = &mut scope.init();
        let constructor =
            v8::Local::<v8::Function>::try_from(v8::Local::new(scope, constructor)).map_err(|_| Error::Script("not a constructor".into()))?;
        let args = args.iter().map(|arg| to_v8(scope, arg)).collect::<Result<Vec<_>, _>>()?;
        let object = constructor.new_instance(scope, &args).ok_or_else(|| exception(scope))?;
        Ok(Value::Js(handle(scope, object.into())))
    }
}

fn call_host<'s>(scope: &mut v8::PinScope<'s, '_>, args: v8::FunctionCallbackArguments<'s>, mut result: v8::ReturnValue<'s>) {
    let function = args
        .data()
        .int32_value(scope)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| context_data(scope).ok()?.functions.borrow().get(index).copied());
    let value = function.ok_or_else(|| missing("such host function")).and_then(|HostFunctionEntry { function, constructor }| {
        let this = if constructor { Value::Undefined } else { from_v8(scope, args.this().into()) };
        let values: Vec<_> = (0..args.length()).map(|i| from_v8(scope, args.get(i))).collect();
        function(&mut V8Call { scope }, &this, &values).and_then(|value| to_v8(scope, &value))
    });
    match value {
        Ok(value) => result.set(value),
        Err(error) => {
            let thrown = crate::exception::value(&mut V8Call { scope }, &error).and_then(|value| to_v8(scope, &value));
            let thrown = match thrown {
                Ok(thrown) => Some(thrown),
                Err(_) => v8::String::new(scope, &error.to_string()).map(|message| v8::Exception::error(scope, message)),
            };
            if let Some(thrown) = thrown {
                scope.throw_exception(thrown);
            }
        }
    }
}

/// The exception caught by `scope`, as an error with the script's stack if it has one.
fn exception(scope: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>) -> Error {
    let Some(exception) = scope.exception() else {
        // V8 returns no exception when the script was terminated.
        return Error::Script("the script was terminated".into());
    };
    let stack = scope.stack_trace().filter(|stack| stack.is_string());
    Error::Script(stack.unwrap_or(exception).to_rust_string_lossy(scope))
}

fn handle(scope: &v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> Handle {
    v8::Global::new(scope, value)
}

/// An error for a V8 call that failed without an exception to report.
fn thrown(what: &str) -> Error {
    Error::Script(format!("V8 couldn't create a {what}"))
}

/// An `ArrayBuffer` that takes over `bytes`.
fn buffer<'s>(scope: &v8::PinScope<'s, '_>, bytes: Vec<u8>) -> v8::Local<'s, v8::ArrayBuffer> {
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
    v8::ArrayBuffer::with_backing_store(scope, &store)
}

fn missing(what: &str) -> Error {
    Error::Script(format!("the V8 context has no {what}"))
}

fn string<'s>(scope: &v8::PinScope<'s, '_>, string: &str) -> Result<v8::Local<'s, v8::String>, Error> {
    v8::String::new(scope, string).ok_or_else(|| Error::Script("the string is too long for V8".into()))
}

fn from_v8(scope: &v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> Value<Handle> {
    if value.is_undefined() {
        Value::Undefined
    } else if value.is_null() {
        Value::Null
    } else if value.is_boolean() {
        Value::Bool(value.boolean_value(scope))
    } else if value.is_number() {
        Value::Number(value.number_value(scope).unwrap_or(f64::NAN))
    } else if value.is_string() {
        Value::String(value.to_rust_string_lossy(scope))
    } else {
        Value::Js(handle(scope, value))
    }
}

fn to_v8<'s>(scope: &v8::PinScope<'s, '_>, value: &Value<Handle>) -> Result<v8::Local<'s, v8::Value>, Error> {
    Ok(match value {
        Value::Undefined => v8::undefined(scope).into(),
        Value::Null => v8::null(scope).into(),
        Value::Bool(bool) => v8::Boolean::new(scope, *bool).into(),
        Value::Number(number) => v8::Number::new(scope, *number).into(),
        Value::String(value) => string(scope, value)?.into(),
        Value::Js(handle) => v8::Local::new(scope, handle),
    })
}
