//! `LocalFile` on the web: a real `File` of the browser's (so its own `fetch()`, `FormData` and
//! readers take it), as it was when the user picked it, that can change the file it came from:
//! through its handle where the browser has them (Chromium's pickers), or by downloading it, for
//! one to save where browsers have none. A file picked without a handle (Firefox, Safari) can only
//! be read. `write()`, `append()` and `exists()` are Promises, as on desktop; `path` and `parent`
//! are `null`, as the web has no paths, so `new LocalFile(path)` throws. `LocalFile.from(handle)`
//! is a Promise of the handle's file.

use wasm_bindgen::JsValue;

use crate::engine::HostFunction;
use crate::{Call, Engine, Error, Runtime, Value, define_accessor, method, object};

/// What the app picked for a script's file dialog.
pub enum Picked {
    /// A file without a handle, read-only: a `File`.
    File(JsValue),
    /// A `FileSystemFileHandle`.
    Handle(JsValue),
    /// A file to save, by downloading it under this name.
    Download(String),
}

/// What `LocalFile`s on the web share: the class (the `newTarget` that makes `File`s ones), the
/// realm, its built-ins they use, and the steps of their Promises.
#[derive(Clone)]
pub(crate) struct Files<H> {
    local_file: H,
    global: H,
    file: H,
    reflect: H,
    promise: H,
    steps: Steps<H>,
}

/// The steps of the Promises, each given what it needs through `bind()`.
#[derive(Clone)]
struct Steps<H> {
    wrap: H,
    write: H,
    append: H,
    append_at: H,
    close: H,
    found: H,
    missing: H,
}

/// Sets `LocalFile` on `global`, extending the realm's `File`.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let constructor = runtime.engine.constructor("LocalFile", |_, _, _| {
        Err(Error::Type("The web has no paths: Dialog.openFile() and LocalFile.from(handle) give LocalFiles".into()))
    })?;
    let from = runtime.engine.function("from", |call, _, args| from_handle(call, args.first().unwrap_or(&Value::Undefined)))?;
    let methods: [(&str, HostFunction<E::Handle>); 4] =
        [("write", write), ("append", append), ("exists", exists), ("toString", |call, this, _| name(call, this))];
    let getters: [(&str, HostFunction<E::Handle>); 5] = [
        ("path", |_, _, _| Ok(Value::Null)),
        ("parent", |_, _, _| Ok(Value::Null)),
        ("extension", |call, this, _| {
            let name = name(call, this)?;
            let name = call.to_string(&name)?;
            Ok(Value::String(name.rsplit_once('.').map(|(_, e)| e.to_owned()).unwrap_or_default()))
        }),
        ("isDirectory", |_, _, _| Ok(Value::Bool(false))),
        ("writable", |call, this, _| Ok(Value::Bool(!matches!(backing(call, this)?, Backing::Picked)))),
    ];
    let made = |runtime: &mut Runtime<E>, list: &[(&'static str, HostFunction<E::Handle>)]| {
        list.iter().map(|(name, f)| Ok((*name, runtime.engine.function(name, *f)?))).collect::<Result<Vec<_>, Error>>()
    };
    let (methods, getters) = (made(runtime, &methods)?, made(runtime, &getters)?);
    let steps = Steps {
        wrap: runtime.engine.function("wrap", |call, _, args| match args {
            [handle, Value::Js(file), ..] => wrap(call, file, Some(handle.clone())),
            _ => Err(Error::Type("no file".into())),
        })?,
        write: runtime.engine.function("write", |call, _, args| match args {
            [data, Value::Js(stream), ..] => {
                let written = method(call, stream, "write", std::slice::from_ref(data))?;
                then(call, written, |s| &s.close, vec![Value::Js(stream.clone())])
            }
            _ => Err(Error::Type("no stream".into())),
        })?,
        append: runtime.engine.function("append", |call, _, args| match args {
            [Value::Js(handle), data, Value::Js(file), ..] => {
                let size = call.get(file, "size")?;
                let options = call.new_object()?;
                call.set(&options, "keepExistingData", &Value::Bool(true))?;
                let stream = method(call, handle, "createWritable", &[Value::Js(options)])?;
                then(call, stream, |s| &s.append_at, vec![data.clone(), size])
            }
            _ => Err(Error::Type("no file".into())),
        })?,
        append_at: runtime.engine.function("appendAt", |call, _, args| match args {
            [data, position, Value::Js(stream), ..] => {
                let command = call.new_object()?;
                call.set(&command, "type", &Value::String("write".into()))?;
                call.set(&command, "position", position)?;
                call.set(&command, "data", data)?;
                let written = method(call, stream, "write", &[Value::Js(command)])?;
                then(call, written, |s| &s.close, vec![Value::Js(stream.clone())])
            }
            _ => Err(Error::Type("no stream".into())),
        })?,
        close: runtime.engine.function("close", |call, _, args| match args.first() {
            Some(Value::Js(stream)) => method(call, stream, "close", &[]),
            _ => Err(Error::Type("no stream".into())),
        })?,
        found: runtime.engine.function("found", |_, _, _| Ok(Value::Bool(true)))?,
        // `getFile()` failing with a `NotFoundError` means it's gone; other failures stay.
        missing: runtime.engine.function("missing", |call, _, args| {
            let reason = args.first().cloned().unwrap_or(Value::Undefined);
            if let Value::Js(error) = &reason
                && matches!(call.get(error, "name")?, Value::String(name) if name == "NotFoundError")
            {
                return Ok(Value::Bool(false));
            }
            let promise = files(call)?.promise;
            method(call, &promise, "reject", &[reason])
        })?,
    };
    let files = runtime.engine.with_call(|call| {
        let file = object(call, global, "File")?;
        let file_prototype = call.get(&file, "prototype")?;
        let object_class = object(call, global, "Object")?;
        let Value::Js(prototype) = method(call, &object_class, "create", &[file_prototype])? else {
            return Err(Error::Script("Object.create made nothing".into()));
        };
        method(call, &object_class, "setPrototypeOf", &[Value::Js(constructor.clone()), Value::Js(file.clone())])?;
        for (name, function) in methods {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        for (name, getter) in getters {
            define_accessor(call, global, &prototype, name, getter, None)?;
        }
        call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(&constructor, "from", &Value::Js(from))?;
        call.set(global, "LocalFile", &Value::Js(constructor.clone()))?;
        crate::tag(call, global, &prototype, "LocalFile")?;
        Ok(Files {
            local_file: constructor,
            global: global.clone(),
            file,
            reflect: object(call, global, "Reflect")?,
            promise: object(call, global, "Promise")?,
            steps,
        })
    })?;
    runtime.host.borrow_mut().web_files = Some(files);
    Ok(())
}

fn files<H: Clone>(call: &mut dyn Call<H>) -> Result<Files<H>, Error> {
    call.host()?.borrow().web_files.clone().ok_or_else(|| Error::Script("LocalFile is not set up".into()))
}

/// `promise.then(step)`, with `bound` the step's first arguments.
fn then<H: Clone>(call: &mut dyn Call<H>, promise: Value<H>, step: impl Fn(&Steps<H>) -> &H, bound: Vec<Value<H>>) -> Result<Value<H>, Error> {
    let Value::Js(promise) = promise else {
        return Err(Error::Type("not a Promise".into()));
    };
    let files = files(call)?;
    let mut args = vec![Value::Undefined];
    args.extend(bound);
    let step = method(call, step(&files.steps), "bind", &args)?;
    method(call, &promise, "then", &[step])
}

/// A Promise rejected with `error`'s exception.
fn rejected<H: Clone>(call: &mut dyn Call<H>, error: &Error) -> Result<Value<H>, Error> {
    let promise = files(call)?.promise;
    let reason = crate::exception::value(call, error)?;
    method(call, &promise, "reject", &[reason])
}

/// The `LocalFile` of what the app picked: a Promise for a handle's.
pub(crate) fn local_file<E: Engine>(call: &mut dyn Call<E::Handle>, picked: Picked) -> Result<Value<E::Handle>, Error> {
    let page = |value| E::page_value(value).ok_or_else(|| Error::Script("files picked in the page need the browser's engine".into()));
    match picked {
        Picked::File(file) => wrap(call, &page(file)?, None),
        Picked::Handle(handle) => from_handle(call, &Value::Js(page(handle)?)),
        Picked::Download(name) => {
            let files = files(call)?;
            let options = call.new_object()?;
            let parts = call.new_array(&[])?;
            let args = call.new_array(&[Value::Js(parts), Value::String(name), Value::Js(options)])?;
            let file = method(call, &files.reflect, "construct", &[Value::Js(files.file), Value::Js(args), Value::Js(files.local_file)])?;
            if let Value::Js(file) = &file {
                call.set(file, "_download", &Value::Bool(true))?;
            }
            Ok(file)
        }
    }
}

/// `LocalFile.from(handle)`: a Promise of the `LocalFile` of the handle's file.
fn from_handle<H: Clone>(call: &mut dyn Call<H>, handle: &Value<H>) -> Result<Value<H>, Error> {
    let Value::Js(file_handle) = handle else {
        return Err(Error::Type("LocalFile.from() needs a FileSystemFileHandle".into()));
    };
    let file = method(call, file_handle, "getFile", &[])?;
    then(call, file, |s| &s.wrap, vec![handle.clone()])
}

/// A `LocalFile` of `file`, a `File`, as it is: one changing the file through `handle`, if any.
fn wrap<H: Clone>(call: &mut dyn Call<H>, file: &H, handle: Option<Value<H>>) -> Result<Value<H>, Error> {
    let files = files(call)?;
    let options = call.new_object()?;
    for key in ["type", "lastModified"] {
        let value = call.get(file, key)?;
        call.set(&options, key, &value)?;
    }
    let name = call.get(file, "name")?;
    let parts = call.new_array(&[Value::Js(file.clone())])?;
    let args = call.new_array(&[Value::Js(parts), name, Value::Js(options)])?;
    let local = method(call, &files.reflect, "construct", &[Value::Js(files.file), Value::Js(args), Value::Js(files.local_file)])?;
    if let (Value::Js(local), Some(handle)) = (&local, handle) {
        call.set(local, "_handle", &handle)?;
    }
    Ok(local)
}

/// What a `LocalFile` changes its file through.
enum Backing<H> {
    Handle(H),
    Download,
    Picked,
}

fn backing<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Backing<H>, Error> {
    let Value::Js(file) = this else {
        return Err(Error::Type("not a LocalFile".into()));
    };
    Ok(match (call.get(file, "_handle")?, call.get(file, "_download")?) {
        (Value::Js(handle), _) => Backing::Handle(handle),
        (_, Value::Bool(true)) => Backing::Download,
        _ => Backing::Picked,
    })
}

fn name<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Value<H>, Error> {
    match this {
        Value::Js(file) => call.get(file, "name"),
        _ => Err(Error::Type("not a LocalFile".into())),
    }
}

/// The error of changing a file picked without a handle.
fn read_only() -> Error {
    Error::Dom("NotAllowedError", "This file can only be read: Dialog.saveFile() gives one to write".into())
}

/// `write(data)`: the file's contents become `data`.
fn write<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let data = args.first().cloned().unwrap_or(Value::String(String::new()));
    match backing(call, this)? {
        Backing::Handle(handle) => {
            let stream = method(call, &handle, "createWritable", &[])?;
            then(call, stream, |s| &s.write, vec![data])
        }
        Backing::Download => {
            let name = name(call, this)?;
            match download(call, name, data) {
                Ok(()) => {
                    let promise = files(call)?.promise;
                    method(call, &promise, "resolve", &[])
                }
                Err(e) => rejected(call, &e),
            }
        }
        Backing::Picked => rejected(call, &read_only()),
    }
}

/// `append(data)`: the file's contents end with `data`. A download is written whole.
fn append<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let data = args.first().cloned().unwrap_or(Value::String(String::new()));
    match backing(call, this)? {
        Backing::Handle(handle) => {
            let file = method(call, &handle, "getFile", &[])?;
            then(call, file, |s| &s.append, vec![Value::Js(handle), data])
        }
        Backing::Download => rejected(call, &Error::Dom("NotAllowedError", "A download is written whole: write() it".into())),
        Backing::Picked => rejected(call, &read_only()),
    }
}

/// `exists()`: whether the file is still there; a picked one or a download always is.
fn exists<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let files = files(call)?;
    match backing(call, this)? {
        Backing::Handle(handle) => {
            let file = method(call, &handle, "getFile", &[])?;
            let Value::Js(file) = file else {
                return Err(Error::Type("getFile() gave no Promise".into()));
            };
            method(call, &file, "then", &[Value::Js(files.steps.found), Value::Js(files.steps.missing)])
        }
        _ => method(call, &files.promise, "resolve", &[Value::Bool(true)]),
    }
}

/// Downloads `data` (text, a `Blob`, a buffer or a view) as a file named `name`, from the realm's
/// page.
fn download<H: Clone>(call: &mut dyn Call<H>, name: Value<H>, data: Value<H>) -> Result<(), Error> {
    let global = files(call)?.global;
    let blob_class = object(call, &global, "Blob")?;
    let parts = call.new_array(&[data])?;
    let blob = call.construct(&blob_class, &[Value::Js(parts)])?;
    let url_class = object(call, &global, "URL")?;
    let url = method(call, &url_class, "createObjectURL", &[blob])?;
    let document = object(call, &global, "document")?;
    let Value::Js(link) = method(call, &document, "createElement", &[Value::String("a".into())])? else {
        return Err(Error::Script("no link".into()));
    };
    call.set(&link, "href", &url)?;
    call.set(&link, "download", &name)?;
    method(call, &link, "click", &[])?;
    // Once the download has its own copy.
    let revoke = object(call, &url_class, "revokeObjectURL")?;
    let revoke = method(call, &revoke, "bind", &[Value::Js(url_class), url])?;
    method(call, &global, "setTimeout", &[revoke, Value::Number(60_000.0)]).map(|_| ())
}
