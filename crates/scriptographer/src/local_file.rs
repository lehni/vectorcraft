//! `LocalFile`, a file or directory on the user's computer: a `File` (and so a `Blob`, for
//! `fetch()`, `FormData` and `createImageBitmap()`), which shows the file as it is now, and can
//! change it. `new LocalFile(...parts)` joins its parts (paths or `LocalFile`s, the last its name;
//! a relative path is from the script's folder): `new LocalFile(script.directory, 'data',
//! 'notes.txt')`; `LocalFile.from(handle)` is a Promise of the file or directory of a
//! `FileSystemHandle`. `Dialog.openFile()` and the others give them too, and `script.file` and
//! `script.directory` are ones.
//!
//! What it is: `name`, `size`, `type` (from its extension), `lastModified`, `path`, `parent`,
//! `extension`, `isDirectory`, `writable`. What it holds, as a `Blob` does (whose readers read it
//! as it is now): `text()`, `arrayBuffer()`, `bytes()`, `slice()`. What changes it, each a
//! Promise: `write(data)`, `append(data)` (text, a `Blob`, a buffer or a view), `makeDirectory()`,
//! `rename(nameOrPath)` (which it follows), `copyTo(fileOrPath)`, `remove({ recursive })`; and
//! `exists()`, `list()` of a directory, `file(name)` in one. What fails rejects with the
//! `DOMException` browsers give: `NotFoundError`, `NotAllowedError`, …
//!
//! On desktop it's a path. (On the web it will be a handle, a picked file or a download.)

use std::path::{Path, PathBuf};

use crate::engine::HostFunction;
use crate::{Call, Engine, Error, Runtime, Value, define_accessor, method, object};

/// What `LocalFile` objects share: their prototype, `Object`, which makes them, and `Promise`.
pub(crate) struct Files<H> {
    prototype: H,
    object: H,
    promise: H,
}

/// MIME types by extension, as browsers give a file's `type`.
const TYPES: [(&str, &str); 18] = [
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("html", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("json", "application/json"),
    ("xml", "application/xml"),
    ("pdf", "application/pdf"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
];

/// Sets `LocalFile` on `global`, extending `File`.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let constructor = runtime.engine.constructor("LocalFile", construct)?;
    // A Promise, as on the web, where a handle gives its file only so.
    let from = runtime.engine.function("from", |call, _, args| {
        let file = from_handle(call, args.first().unwrap_or(&Value::Undefined));
        settled(call, file)
    })?;
    let methods = methods().into_iter().map(|(name, f)| Ok((name, runtime.engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let getters = getters().into_iter().map(|(name, f)| Ok((name, runtime.engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let files = runtime.engine.with_call(|call| {
        let object_class = object(call, global, "Object")?;
        let file = object(call, global, "File")?;
        let file_prototype = call.get(&file, "prototype")?;
        let Value::Js(prototype) = method(call, &object_class, "create", &[file_prototype])? else {
            return Err(Error::Script("Object.create made nothing".into()));
        };
        method(call, &object_class, "setPrototypeOf", &[Value::Js(constructor.clone()), Value::Js(file)])?;
        for (name, function) in methods {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        for (name, getter) in getters {
            define_accessor(call, global, &prototype, name, getter, None)?;
        }
        call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(&constructor, "from", &Value::Js(from))?;
        call.set(global, "LocalFile", &Value::Js(constructor))?;
        crate::tag(call, global, &prototype, "LocalFile")?;
        Ok(Files { prototype, object: object_class, promise: object(call, global, "Promise")? })
    })?;
    runtime.host.borrow_mut().files = Some(files);
    Ok(())
}

/// The methods of `LocalFile` objects.
fn methods<H: Clone>() -> Vec<(&'static str, HostFunction<H>)> {
    vec![
        ("toString", |call, this, _| Ok(Value::String(display(&path(call, this)?)))),
        ("write", |call, this, args| change(call, this, args, false)),
        ("append", |call, this, args| change(call, this, args, true)),
        ("exists", |call, this, _| {
            let exists = path(call, this).map(|path| Value::Bool(path.exists()));
            settled(call, exists)
        }),
        ("list", list),
        // A Promise, as a directory on the web answers it.
        ("file", |call, this, args| {
            let file = (|| {
                let name = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
                let path = path(call, this)?.join(name);
                new_local_file(call, &path)
            })();
            settled(call, file)
        }),
        ("makeDirectory", |call, this, _| {
            let made = path(call, this).and_then(|path| std::fs::create_dir_all(&path).map_err(|e| io_error(&path, &e, true)).map(|_| this.clone()));
            settled(call, made)
        }),
        ("rename", rename),
        ("copyTo", copy_to),
        ("remove", remove),
    ]
}

/// The getters of `LocalFile` objects: what the file is, now.
fn getters<H: Clone>() -> Vec<(&'static str, HostFunction<H>)> {
    vec![
        ("path", |call, this, _| Ok(Value::String(display(&path(call, this)?)))),
        ("name", |call, this, _| Ok(Value::String(path(call, this)?.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))),
        ("extension", |call, this, _| Ok(Value::String(extension(&path(call, this)?)))),
        ("type", |call, this, _| Ok(Value::String(type_of(&path(call, this)?).to_owned()))),
        ("parent", |call, this, _| match path(call, this)?.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(parent) => new_local_file(call, parent),
            None => Ok(Value::Null),
        }),
        ("isDirectory", |call, this, _| Ok(Value::Bool(path(call, this)?.is_dir()))),
        // On desktop, every file can be written where it is.
        ("writable", |_, _, _| Ok(Value::Bool(true))),
        ("size", |call, this, _| Ok(Value::Number(std::fs::metadata(path(call, this)?).map_or(0.0, |m| m.len() as f64)))),
        ("lastModified", |call, this, _| {
            let modified = std::fs::metadata(path(call, this)?).and_then(|m| m.modified()).ok();
            let since = modified.and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
            Ok(Value::Number(since.map_or(0.0, |d| d.as_millis() as f64)))
        }),
    ]
}

/// A `LocalFile` for `path`, made absolute against the script's folder.
pub(crate) fn new_local_file<H: Clone>(call: &mut dyn Call<H>, path: &Path) -> Result<Value<H>, Error> {
    let (prototype, object_class) = {
        let host = call.host()?;
        let host = host.borrow();
        let files = host.files.as_ref().ok_or_else(|| Error::Script("LocalFile is not set up".into()))?;
        (files.prototype.clone(), files.object.clone())
    };
    let file = method(call, &object_class, "create", &[Value::Js(prototype)])?;
    if let Value::Js(handle) = &file {
        let path = absolute(call, path)?;
        call.set(handle, "_path", &Value::String(display(&path)))?;
    }
    Ok(file)
}

/// `path`, from the script's folder when it's relative.
fn absolute<H>(call: &mut dyn Call<H>, path: &Path) -> Result<PathBuf, Error> {
    let directory = call.host()?.borrow().directory.clone();
    Ok(match directory {
        Some(directory) if path.is_relative() => directory.join(path),
        _ => path.to_owned(),
    })
}

/// `new LocalFile(...parts)`, on `this`, the object `new` made: its path, from parts that are paths
/// or `LocalFile`s, the last its name.
fn construct<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(file) = this else {
        return Err(Error::Type("LocalFile needs 'new'".into()));
    };
    if args.is_empty() {
        return Err(Error::Type("LocalFile: no path".into()));
    }
    // Joined as `path.join()` joins: an absolute part starts the path again.
    let mut path = PathBuf::new();
    for part in args {
        path.push(call.to_string(part)?);
    }
    let path = absolute(call, &path)?;
    call.set(file, "_path", &Value::String(display(&path)))?;
    Ok(Value::Undefined)
}

/// The path of the `LocalFile` that is `this`.
pub(crate) fn path<H>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<PathBuf, Error> {
    let Value::Js(file) = this else {
        return Err(Error::Type("not a LocalFile".into()));
    };
    match call.get(file, "_path")? {
        Value::String(path) => Ok(PathBuf::from(path)),
        _ => Err(Error::Type("not a LocalFile".into())),
    }
}

/// Whether `value` is a `LocalFile`.
pub(crate) fn is_local_file<H>(call: &mut dyn Call<H>, value: &H) -> Result<bool, Error> {
    Ok(matches!(call.get(value, "_path")?, Value::String(_)))
}

/// The path the first of `args` names: a `LocalFile` or a path, from the script's folder.
fn target<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<PathBuf, Error> {
    let first = args.first().ok_or_else(|| Error::Type("no file given".into()))?;
    let named = call.to_string(first)?;
    absolute(call, Path::new(&named))
}

pub(crate) fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The MIME type of the file at `path`, by its extension, or none.
pub(crate) fn type_of(path: &Path) -> &'static str {
    let extension = extension(path).to_ascii_lowercase();
    TYPES.iter().find(|(e, _)| *e == extension).map_or("", |(_, t)| *t)
}

/// `LocalFile.from(handle)`: the `LocalFile` of a file or directory handle.
pub(crate) fn from_handle<H: Clone>(call: &mut dyn Call<H>, handle: &Value<H>) -> Result<Value<H>, Error> {
    let path = match handle {
        Value::Js(handle) => crate::file_system::path_of(call, handle)?,
        _ => None,
    };
    let path = path.ok_or_else(|| Error::Type("LocalFile.from() needs a FileSystemHandle".into()))?;
    new_local_file(call, &path)
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The `DOMException` browsers give for `error` on `path`: `writing` tells a change from a read.
pub(crate) fn io_error(path: &Path, error: &std::io::Error, writing: bool) -> Error {
    let name = match error.kind() {
        std::io::ErrorKind::NotFound => "NotFoundError",
        std::io::ErrorKind::PermissionDenied => "NotAllowedError",
        _ if writing => "InvalidModificationError",
        _ => "NotReadableError",
    };
    Error::Dom(name, format!("{}: {error}", display(path)))
}

/// A Promise settled with `result`: resolved with its value, else rejected with its exception.
pub(crate) fn settled<H: Clone>(call: &mut dyn Call<H>, result: Result<Value<H>, Error>) -> Result<Value<H>, Error> {
    let promise = call.host()?.borrow().files.as_ref().map(|f| f.promise.clone()).ok_or_else(|| Error::Script("LocalFile is not set up".into()))?;
    match result {
        Ok(value) => method(call, &promise, "resolve", &[value]),
        Err(error) => {
            let reason = crate::exception::value(call, &error)?;
            method(call, &promise, "reject", &[reason])
        }
    }
}

/// The bytes in the file `this` is, as they are now.
pub(crate) fn read<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<u8>, Error> {
    let path = path(call, this)?;
    std::fs::read(&path).map_err(|e| io_error(&path, &e, false))
}

/// `write(data)` or `append(data)`: the file's contents become, or end with, `data`.
fn change<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>], append: bool) -> Result<Value<H>, Error> {
    let written = (|| {
        let path = path(call, this)?;
        let (bytes, _) = crate::fetch::body_bytes(call, args.first().unwrap_or(&Value::String(String::new())))?;
        let result = if append {
            use std::io::Write as _;
            std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut file| file.write_all(&bytes))
        } else {
            std::fs::write(&path, &bytes)
        };
        result.map_err(|e| io_error(&path, &e, true)).map(|_| Value::Undefined)
    })();
    settled(call, written)
}

/// `list()`: the files and directories in the directory, by name.
fn list<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let listed = (|| {
        let folder = path(call, this)?;
        let entries = std::fs::read_dir(&folder).map_err(|e| io_error(&folder, &e, false))?;
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            files.push(new_local_file(call, &path)?);
        }
        call.new_array(&files).map(Value::Js)
    })();
    settled(call, listed)
}

/// `rename(nameOrPath)`: a name renames it where it is, a path moves it; the `LocalFile` follows.
fn rename<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let renamed = (|| {
        let from = path(call, this)?;
        let given = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
        let to = if Path::new(&given).components().count() == 1 { from.with_file_name(&given) } else { target(call, args)? };
        std::fs::rename(&from, &to).map_err(|e| io_error(&from, &e, true))?;
        if let Value::Js(file) = this {
            call.set(file, "_path", &Value::String(display(&to)))?;
        }
        Ok(this.clone())
    })();
    settled(call, renamed)
}

/// `copyTo(fileOrPath)`: a Promise of the copy.
fn copy_to<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let copied = (|| {
        let (from, to) = (path(call, this)?, target(call, args)?);
        std::fs::copy(&from, &to).map_err(|e| io_error(&from, &e, true))?;
        new_local_file(call, &to)
    })();
    settled(call, copied)
}

/// `remove({ recursive })`: removes the file, or the directory: with what's in it only when
/// `recursive`.
fn remove<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let removed = (|| {
        let recursive = match args.first() {
            Some(Value::Js(options)) => crate::truthy(&call.get(options, "recursive")?),
            _ => false,
        };
        let path = path(call, this)?;
        let result = if !path.is_dir() {
            std::fs::remove_file(&path)
        } else if recursive {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_dir(&path)
        };
        result.map_err(|e| io_error(&path, &e, true)).map(|_| Value::Undefined)
    })();
    settled(call, removed)
}
