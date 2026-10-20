//! The File System Access API on desktop, over paths: `showOpenFilePicker()`,
//! `showSaveFilePicker()` and `showDirectoryPicker()`, which ask the app ([`crate::dialog::ask`])
//! and give Promises of handles, or reject with an `AbortError` when the user cancelled.
//!
//! `FileSystemFileHandle` and `FileSystemDirectoryHandle` (both `FileSystemHandle`s: `kind`,
//! `name`, `isSameEntry()`, and `queryPermission()` and `requestPermission()`, granted) stand for
//! a file or directory; a file's `getFile()` is a `File` of what it holds now, and its
//! `createWritable()` a `FileSystemWritableFileStream` (`write()`, `seek()`, `truncate()`,
//! `close()`, `abort()`), which writes the file on `close()`, as browsers do. A directory's
//! `getFileHandle()`, `getDirectoryHandle()` (`{ create }`), `removeEntry()` (`{ recursive }`),
//! `resolve()`, and `entries()`, `keys()` and `values()`, also its async iterator.
//!
//! What's left out: the stream is not a `WritableStream` (no `getWriter()` or `pipeTo()`), and
//! `showOpenFilePicker({ multiple: true })` shows every file, as the app's dialog for several
//! files has no file types. A browser's realm has its own (Chromium's), or none.

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use crate::engine::HostFunction;
use crate::local_file::{display, io_error, settled};
use crate::{Call, Engine, Error, Runtime, Value, define_accessor, method, object};

/// What the handles share: their prototypes, `Object`, which makes them, and `File`.
#[derive(Clone)]
pub(crate) struct Handles<H> {
    file: H,
    directory: H,
    writable: H,
    iterator: H,
    object: H,
    file_class: H,
}

/// Sets the handle classes and the pickers on `global`, after `File`.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let illegal: HostFunction<E::Handle> = |_, _, _| Err(Error::Type("Illegal constructor".into()));
    let mut classes = Vec::new();
    for (name, methods, getters) in classes_table() {
        let constructor = runtime.engine.constructor(name, illegal)?;
        let methods = methods.into_iter().map(|(n, f)| Ok((n, runtime.engine.function(n, f)?))).collect::<Result<Vec<_>, Error>>()?;
        let getters = getters.into_iter().map(|(n, f)| Ok((n, runtime.engine.function(n, f)?))).collect::<Result<Vec<_>, Error>>()?;
        classes.push((name, constructor, methods, getters));
    }
    let pickers: Vec<(&str, E::Handle)> = vec![
        ("showOpenFilePicker", runtime.engine.function("showOpenFilePicker", show_open_file_picker)?),
        ("showSaveFilePicker", runtime.engine.function("showSaveFilePicker", show_save_file_picker)?),
        ("showDirectoryPicker", runtime.engine.function("showDirectoryPicker", show_directory_picker)?),
    ];
    let this = runtime.engine.function("[Symbol.asyncIterator]", |_, this, _| Ok(this.clone()))?;
    let next = runtime.engine.function("next", next)?;
    let handles = runtime.engine.with_call(|call| {
        let object_class = object(call, global, "Object")?;
        let symbol = object(call, global, "Symbol")?;
        let async_iterator = call.get(&symbol, "asyncIterator")?;
        let reflect = object(call, global, "Reflect")?;
        let mut prototypes: Vec<E::Handle> = Vec::new();
        for (name, constructor, methods, getters) in classes {
            // The file and directory handles extend `FileSystemHandle`, the first.
            let parent = match (name, prototypes.first()) {
                ("FileSystemFileHandle" | "FileSystemDirectoryHandle", Some(handle)) => Value::Js(handle.clone()),
                _ => call.get(&object_class, "prototype")?,
            };
            let Value::Js(prototype) = method(call, &object_class, "create", &[parent])? else {
                return Err(Error::Script("Object.create made nothing".into()));
            };
            for (key, function) in methods {
                call.set(&prototype, key, &Value::Js(function))?;
            }
            for (key, getter) in getters {
                define_accessor(call, global, &prototype, key, getter, None)?;
            }
            // `for await (const [name, handle] of directory)`, as `entries()`.
            if name == "FileSystemDirectoryHandle" {
                let entries = call.get(&prototype, "entries")?;
                method(call, &reflect, "set", &[Value::Js(prototype.clone()), async_iterator.clone(), entries])?;
            }
            call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
            call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
            call.set(global, name, &Value::Js(constructor))?;
            crate::tag(call, global, &prototype, name)?;
            prototypes.push(prototype);
        }
        let iterator = call.new_object()?;
        call.set(&iterator, "next", &Value::Js(next))?;
        method(call, &reflect, "set", &[Value::Js(iterator.clone()), async_iterator, Value::Js(this)])?;
        for (name, picker) in pickers {
            call.set(global, name, &Value::Js(picker))?;
        }
        let [_, file, directory, writable]: [E::Handle; 4] =
            prototypes.try_into().map_err(|_| Error::Script("the file system classes didn't set up".into()))?;
        Ok(Handles { file, directory, writable, iterator, object: object_class, file_class: object(call, global, "File")? })
    })?;
    runtime.host.borrow_mut().handles = Some(handles);
    Ok(())
}

type Members<H> = Vec<(&'static str, HostFunction<H>)>;

/// The classes, `FileSystemHandle` first: their names, methods and getters.
fn classes_table<H: Clone>() -> Vec<(&'static str, Members<H>, Members<H>)> {
    vec![
        (
            "FileSystemHandle",
            vec![
                ("isSameEntry", |call, this, args| {
                    let same = (|| {
                        let other = match args.first() {
                            Some(Value::Js(other)) => path_of(call, other)?,
                            _ => None,
                        };
                        Ok(Value::Bool(other.is_some() && other == path_of_value(call, this)?))
                    })();
                    settled(call, same)
                }),
                ("queryPermission", |call, _, _| settled(call, Ok(Value::String("granted".into())))),
                ("requestPermission", |call, _, _| settled(call, Ok(Value::String("granted".into())))),
            ],
            vec![
                ("kind", |call, this, _| match this {
                    Value::Js(handle) => call.get(handle, "_kind"),
                    _ => Err(Error::Type("not a FileSystemHandle".into())),
                }),
                ("name", |call, this, _| Ok(Value::String(name(&handle_path(call, this)?)))),
            ],
        ),
        ("FileSystemFileHandle", vec![("getFile", get_file), ("createWritable", create_writable)], vec![]),
        (
            "FileSystemDirectoryHandle",
            vec![
                ("getFileHandle", |call, this, args| child(call, this, args, false)),
                ("getDirectoryHandle", |call, this, args| child(call, this, args, true)),
                ("removeEntry", remove_entry),
                ("resolve", resolve),
                ("entries", |call, this, _| entries(call, this, Listing::Entries)),
                ("keys", |call, this, _| entries(call, this, Listing::Keys)),
                ("values", |call, this, _| entries(call, this, Listing::Values)),
            ],
            vec![],
        ),
        (
            "FileSystemWritableFileStream",
            vec![
                ("write", write),
                ("seek", |call, this, args| {
                    let moved = seek(call, this, args.first());
                    settled(call, moved)
                }),
                ("truncate", |call, this, args| {
                    let cut = truncate(call, this, args.first());
                    settled(call, cut)
                }),
                ("close", close),
                ("abort", |call, this, _| {
                    let aborted = (|| {
                        open_stream(call, this)?;
                        set_hidden(call, this, "_closed", Value::Bool(true))?;
                        Ok(Value::Undefined)
                    })();
                    settled(call, aborted)
                }),
            ],
            vec![],
        ),
    ]
}

fn shared<H: Clone>(call: &mut dyn Call<H>) -> Result<Handles<H>, Error> {
    call.host()?.borrow().handles.clone().ok_or_else(|| Error::Script("the file system is not set up".into()))
}

/// A handle for the file, or the directory, at `path`.
pub(crate) fn new_handle<H: Clone>(call: &mut dyn Call<H>, path: &Path, is_directory: bool) -> Result<Value<H>, Error> {
    let shared = shared(call)?;
    let prototype = if is_directory { shared.directory } else { shared.file };
    let handle = method(call, &shared.object, "create", &[Value::Js(prototype)])?;
    set_hidden(call, &handle, "_path", Value::String(display(path)))?;
    set_hidden(call, &handle, "_kind", Value::String(if is_directory { "directory" } else { "file" }.into()))?;
    Ok(handle)
}

/// The path of `handle`, when it is one of these handles.
pub(crate) fn path_of<H>(call: &mut dyn Call<H>, handle: &H) -> Result<Option<PathBuf>, Error> {
    Ok(match (call.get(handle, "_path")?, call.get(handle, "_kind")?) {
        (Value::String(path), Value::String(_)) => Some(PathBuf::from(path)),
        _ => None,
    })
}

fn path_of_value<H>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<Option<PathBuf>, Error> {
    match value {
        Value::Js(handle) => path_of(call, handle),
        _ => Ok(None),
    }
}

/// The path of the handle that is `this`.
fn handle_path<H>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<PathBuf, Error> {
    path_of_value(call, this)?.ok_or_else(|| Error::Type("not a FileSystemHandle".into()))
}

fn set_hidden<H>(call: &mut dyn Call<H>, target: &Value<H>, key: &str, value: Value<H>) -> Result<(), Error> {
    match target {
        Value::Js(object) => call.set(object, key, &value),
        _ => Err(Error::Type("not an object".into())),
    }
}

fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The largest file a stream writes, in bytes.
const MOST: usize = 512 * 1024 * 1024;

/// A size or position as a count of bytes, up to [`MOST`].
fn count(n: f64) -> Result<usize, Error> {
    if !n.is_finite() || n < 0.0 {
        return Err(Error::Type(format!("{n} is not a size")));
    }
    if n > MOST as f64 {
        return Err(Error::Dom("QuotaExceededError", format!("{n} bytes is more than a file may have here")));
    }
    Ok(n as usize)
}

/// `getFile()`: a `File` of the bytes the file holds now.
fn get_file<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let file = (|| {
        let path = handle_path(call, this)?;
        let bytes = std::fs::read(&path).map_err(|e| io_error(&path, &e, false))?;
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
        let file_class = shared(call)?.file_class;
        let part = call.new_uint8_array(&bytes)?;
        let parts = call.new_array(&[Value::Js(part)])?;
        let options = call.new_object()?;
        call.set(&options, "type", &Value::String(crate::local_file::type_of(&path).into()))?;
        call.set(&options, "lastModified", &Value::Number(modified.map_or(0.0, |d| d.as_millis() as f64)))?;
        call.construct(&file_class, &[Value::Js(parts), Value::String(name(&path)), Value::Js(options)])
    })();
    settled(call, file)
}

/// `createWritable({ keepExistingData })`: a stream that writes the file on `close()`, starting
/// empty, or with what the file holds.
fn create_writable<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let stream = (|| {
        let path = handle_path(call, this)?;
        let keep = match args.first() {
            Some(Value::Js(options)) => crate::truthy(&call.get(options, "keepExistingData")?),
            _ => false,
        };
        let bytes = if keep { std::fs::read(&path).map_err(|e| io_error(&path, &e, false))? } else { Vec::new() };
        let shared = shared(call)?;
        let stream = method(call, &shared.object, "create", &[Value::Js(shared.writable)])?;
        set_hidden(call, &stream, "_path", Value::String(display(&path)))?;
        let data = call.new_uint8_array(&bytes)?;
        set_hidden(call, &stream, "_data", Value::Js(data))?;
        set_hidden(call, &stream, "_position", Value::Number(0.0))?;
        Ok(stream)
    })();
    settled(call, stream)
}

/// The bytes and position of the open stream that is `this`.
fn open_stream<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<(H, Vec<u8>, f64), Error> {
    let Value::Js(stream) = this else {
        return Err(Error::Type("not a FileSystemWritableFileStream".into()));
    };
    if crate::truthy(&call.get(stream, "_closed")?) {
        return Err(Error::Type("the stream is closed".into()));
    }
    let Value::Js(data) = call.get(stream, "_data")? else {
        return Err(Error::Type("not a FileSystemWritableFileStream".into()));
    };
    let position = call.get(stream, "_position")?;
    let position = call.to_number(&position)?;
    Ok((stream.clone(), call.bytes(&data)?, position))
}

/// Changes the bytes and position of the open stream that is `this`.
fn stream_change<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, change: impl FnOnce(&mut Vec<u8>, &mut f64)) -> Result<Value<H>, Error> {
    let (stream, mut data, mut position) = open_stream(call, this)?;
    change(&mut data, &mut position);
    let array = call.new_uint8_array(&data)?;
    call.set(&stream, "_data", &Value::Js(array))?;
    call.set(&stream, "_position", &Value::Number(position))?;
    Ok(Value::Undefined)
}

/// `seek(position)`: where the stream writes next.
fn seek<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, position: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let position = match position {
        Some(Value::Undefined | Value::Null) | None => return Err(Error::Dom("SyntaxError", "seek needs a position".into())),
        Some(position) => count(call.to_number(position)?)? as f64,
    };
    stream_change(call, this, |_, at| *at = position)
}

/// `truncate(size)`: the stream's bytes cut, or filled with zeros, to `size`.
fn truncate<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, size: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let size = count(call.to_number(size.unwrap_or(&Value::Undefined))?)?;
    stream_change(call, this, |bytes, at| {
        bytes.resize(size, 0);
        *at = at.min(size as f64);
    })
}

/// `write(data)`: text, a `Blob`, a buffer or a view at the position, or `{ type: 'write' |
/// 'seek' | 'truncate', data, position, size }`.
fn write<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let written = (|| {
        let data = args.first().cloned().unwrap_or(Value::Undefined);
        let command = match &data {
            Value::Js(object) if !crate::blob::is_blob(call, object)? && call.bytes(object).is_err() => match call.get(object, "type")? {
                Value::String(kind) => Some((object.clone(), kind)),
                _ => None,
            },
            _ => None,
        };
        let (data, position, kind) = match command {
            Some((params, kind)) => {
                let position = match call.get(&params, "position")? {
                    Value::Undefined | Value::Null => None,
                    position => Some(call.to_number(&position)?),
                };
                match kind.as_str() {
                    "write" => (call.get(&params, "data")?, position, "write"),
                    "seek" => (Value::Undefined, position, "seek"),
                    "truncate" => (call.get(&params, "size")?, None, "truncate"),
                    _ => return Err(Error::Type(format!("write: no command '{kind}'"))),
                }
            }
            None => (data, None, "write"),
        };
        match kind {
            "seek" => seek(call, this, position.map(Value::Number).as_ref()),
            "truncate" => truncate(call, this, Some(&data)),
            _ => {
                if matches!(data, Value::Undefined | Value::Null) {
                    return Err(Error::Dom("SyntaxError", "write needs data".into()));
                }
                let (bytes, _) = crate::blob::body_bytes(call, &data)?;
                let (_, _, at) = open_stream(call, this)?;
                let start = count(position.unwrap_or(at))?;
                count((start + bytes.len()) as f64)?;
                stream_change(call, this, |stored, at| {
                    if stored.len() < start + bytes.len() {
                        stored.resize(start + bytes.len(), 0);
                    }
                    if let Some(slot) = stored.get_mut(start..start + bytes.len()) {
                        slot.copy_from_slice(&bytes);
                    }
                    *at = (start + bytes.len()) as f64;
                })
            }
        }
    })();
    settled(call, written)
}

/// `close()`: writes the file, all at once.
fn close<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let closed = (|| {
        let (stream, data, _) = open_stream(call, this)?;
        let Value::String(path) = call.get(&stream, "_path")? else {
            return Err(Error::Type("not a FileSystemWritableFileStream".into()));
        };
        call.set(&stream, "_closed", &Value::Bool(true))?;
        let path = PathBuf::from(path);
        std::fs::write(&path, data).map_err(|e| io_error(&path, &e, true))?;
        Ok(Value::Undefined)
    })();
    settled(call, closed)
}

/// The path of the entry `name` in the directory `this`: a name, not a path.
fn entry<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<PathBuf, Error> {
    let name = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::Type(format!("'{name}' is not a valid name")));
    }
    Ok(handle_path(call, this)?.join(name))
}

/// `getFileHandle(name, { create })` and `getDirectoryHandle(…)`.
fn child<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>], directory: bool) -> Result<Value<H>, Error> {
    let handle = (|| {
        let path = entry(call, this, args)?;
        let create = match args.get(1) {
            Some(Value::Js(options)) => crate::truthy(&call.get(options, "create")?),
            _ => false,
        };
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() != directory => {
                return Err(Error::Dom("TypeMismatchError", format!("{} is a {}", display(&path), if directory { "file" } else { "directory" })));
            }
            Ok(_) => {}
            Err(_) if !create => return Err(Error::Dom("NotFoundError", format!("{} does not exist", display(&path)))),
            Err(_) => {
                let made = if directory { std::fs::create_dir(&path) } else { std::fs::write(&path, []) };
                made.map_err(|e| io_error(&path, &e, true))?;
            }
        }
        new_handle(call, &path, directory)
    })();
    settled(call, handle)
}

/// `removeEntry(name, { recursive })`.
fn remove_entry<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let removed = (|| {
        let path = entry(call, this, args)?;
        let recursive = match args.get(1) {
            Some(Value::Js(options)) => crate::truthy(&call.get(options, "recursive")?),
            _ => false,
        };
        let result = match std::fs::metadata(&path) {
            Ok(metadata) if !metadata.is_dir() => std::fs::remove_file(&path),
            Ok(_) if recursive => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_dir(&path),
            Err(e) => Err(e),
        };
        result.map_err(|e| io_error(&path, &e, true)).map(|_| Value::Undefined)
    })();
    settled(call, removed)
}

/// `resolve(handle)`: the names from the directory down to `handle`, or `null` outside it.
fn resolve<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let resolved = (|| {
        let directory = handle_path(call, this)?;
        let inner = match args.first() {
            Some(Value::Js(handle)) => path_of(call, handle)?,
            _ => None,
        };
        match inner.as_deref().and_then(|inner| inner.strip_prefix(&directory).ok()) {
            Some(relative) => {
                let names: Vec<Value<H>> = relative.components().map(|c| Value::String(c.as_os_str().to_string_lossy().into_owned())).collect();
                call.new_array(&names).map(Value::Js)
            }
            None => Ok(Value::Null),
        }
    })();
    settled(call, resolved)
}

#[derive(Clone, Copy)]
enum Listing {
    Entries,
    Keys,
    Values,
}

/// `entries()`, `keys()` and `values()`: an async iterator of what's in the directory, by name.
fn entries<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, listing: Listing) -> Result<Value<H>, Error> {
    let directory = handle_path(call, this)?;
    let read = std::fs::read_dir(&directory).map_err(|e| io_error(&directory, &e, false))?;
    let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let key = Value::String(name(&path));
        let item = match listing {
            Listing::Keys => key,
            Listing::Values => new_handle(call, &path, path.is_dir())?,
            Listing::Entries => {
                let handle = new_handle(call, &path, path.is_dir())?;
                Value::Js(call.new_array(&[key, handle])?)
            }
        };
        items.push(item);
    }
    let shared = shared(call)?;
    let iterator = method(call, &shared.object, "create", &[Value::Js(shared.iterator)])?;
    let items = call.new_array(&items)?;
    set_hidden(call, &iterator, "_items", Value::Js(items))?;
    set_hidden(call, &iterator, "_at", Value::Number(0.0))?;
    Ok(iterator)
}

/// The iterator's `next()`: a Promise of its next item.
fn next<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(iterator) = this else {
        return Err(Error::Type("not an iterator".into()));
    };
    let (Value::Js(items), at) = (call.get(iterator, "_items")?, call.get(iterator, "_at")?) else {
        return Err(Error::Type("not an iterator".into()));
    };
    let at = call.to_number(&at)?;
    let length = call.get(&items, "length")?;
    let length = call.to_number(&length)?;
    let result = call.new_object()?;
    if at < length {
        let value = call.get(&items, &at.to_string())?;
        call.set(iterator, "_at", &Value::Number(at + 1.0))?;
        call.set(&result, "value", &value)?;
        call.set(&result, "done", &Value::Bool(false))?;
    } else {
        call.set(&result, "value", &Value::Undefined)?;
        call.set(&result, "done", &Value::Bool(true))?;
    }
    settled(call, Ok(Value::Js(result)))
}

/// The pickers' `types` (`[{ description, accept: { type: extensions } }]`) as the app's file
/// types, `[label, [extensions]]`, with one for every file unless `excludeAcceptAllOption`.
fn file_types<H: Clone>(call: &mut dyn Call<H>, options: Option<&H>) -> Result<Vec<Json>, Error> {
    let Some(options) = options else {
        return Ok(Vec::new());
    };
    let mut filters = Vec::new();
    if let Value::Js(types) = call.get(options, "types")? {
        let length = call.get(&types, "length")?;
        let length = call.to_number(&length)?;
        for i in 0..count(length)?.min(256) {
            let Value::Js(kind) = call.get(&types, &i.to_string())? else { continue };
            let Value::Js(accept) = call.get(&kind, "accept")? else {
                return Err(Error::Type("a file type needs its accept".into()));
            };
            let globals = crate::dialog::globals(call)?;
            let accept = crate::dialog::to_json(call, &globals, &Value::Js(accept))?;
            let mut extensions = Vec::new();
            for value in accept.as_object().into_iter().flat_map(|a| a.values()) {
                let listed = match value {
                    Json::Array(list) => list.iter().filter_map(|e| e.as_str()).collect(),
                    Json::String(one) => vec![one.as_str()],
                    _ => Vec::new(),
                };
                for extension in listed {
                    let Some(extension) = extension.strip_prefix('.').filter(|e| !e.is_empty()) else {
                        return Err(Error::Type(format!("'{extension}' is not an extension, which starts with '.'")));
                    };
                    extensions.push(Json::String(extension.to_owned()));
                }
            }
            let label = match call.get(&kind, "description")? {
                Value::String(label) if !label.is_empty() => label,
                _ => extensions.iter().filter_map(|e| e.as_str()).map(|e| format!(".{e}")).collect::<Vec<_>>().join(", "),
            };
            filters.push(json!([label, extensions]));
        }
    }
    if !filters.is_empty() && !crate::truthy(&call.get(options, "excludeAcceptAllOption")?) {
        filters.push(json!(["All Files", []]));
    }
    Ok(filters)
}

/// The directory the picker starts in: `startIn`, a handle or a `LocalFile` (a file's: its
/// directory) or one of the user's directories (`'desktop'`, `'documents'`, `'downloads'`,
/// `'music'`, `'pictures'`, `'videos'`).
fn start_in<H: Clone>(call: &mut dyn Call<H>, options: Option<&H>) -> Result<Option<PathBuf>, Error> {
    let Some(options) = options else {
        return Ok(None);
    };
    Ok(match call.get(options, "startIn")? {
        Value::Undefined | Value::Null => None,
        // A handle, or a `LocalFile`, as `Dialog`'s file dialogs give:
        Value::Js(handle) => {
            let path = match path_of(call, &handle)? {
                Some(path) => path,
                None => crate::local_file::path(call, &Value::Js(handle))?,
            };
            if path.is_dir() { Some(path) } else { path.parent().map(Path::to_path_buf) }
        }
        place => {
            let place = call.to_string(&place)?;
            let folder = match place.as_str() {
                "desktop" => "Desktop",
                "documents" => "Documents",
                "downloads" => "Downloads",
                "music" => "Music",
                "pictures" => "Pictures",
                "videos" if cfg!(target_os = "macos") => "Movies",
                "videos" => "Videos",
                _ => return Err(Error::Type(format!("startIn: no directory '{place}'"))),
            };
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|home| PathBuf::from(home).join(folder))
        }
    })
}

fn options<H: Clone>(args: &[Value<H>]) -> Option<&H> {
    match args.first() {
        Some(Value::Js(options)) => Some(options),
        _ => None,
    }
}

/// The paths the user picked in the app's dialog of `kind`, or an `AbortError`.
fn pick<H: Clone>(call: &mut dyn Call<H>, kind: &str, path: Option<PathBuf>, filters: Vec<Json>) -> Result<Vec<PathBuf>, Error> {
    let path = path.map_or(Json::Null, |p| Json::String(display(&p)));
    let paths: Vec<PathBuf> = match crate::dialog::ask(call, json!({ "type": kind, "path": path, "filters": filters }))? {
        Json::String(path) => vec![PathBuf::from(path)],
        Json::Array(paths) => paths.iter().filter_map(|p| p.as_str().map(PathBuf::from)).collect(),
        _ => Vec::new(),
    };
    if paths.is_empty() {
        return Err(Error::Dom("AbortError", "The user aborted a request.".into()));
    }
    Ok(paths)
}

/// `showOpenFilePicker({ multiple, types, excludeAcceptAllOption, startIn })`: a Promise of the
/// handles of the files picked.
fn show_open_file_picker<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let picked = (|| {
        let options = options(args);
        let multiple = match options {
            Some(options) => crate::truthy(&call.get(options, "multiple")?),
            None => false,
        };
        let (start, filters) = (start_in(call, options)?, file_types(call, options)?);
        let mut handles = Vec::new();
        for path in pick(call, if multiple { "openFiles" } else { "openFile" }, start, filters)? {
            handles.push(new_handle(call, &path, false)?);
        }
        call.new_array(&handles).map(Value::Js)
    })();
    settled(call, picked)
}

/// `showSaveFilePicker({ suggestedName, types, excludeAcceptAllOption, startIn })`: a Promise of
/// the handle of the file to save, which exists from then on, as browsers make it.
fn show_save_file_picker<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let picked = (|| {
        let options = options(args);
        let suggested = match options.map(|o| call.get(o, "suggestedName")).transpose()? {
            None | Some(Value::Undefined | Value::Null) => None,
            Some(name) => Some(call.to_string(&name)?),
        };
        let start = start_in(call, options)?;
        let path = match (start, suggested) {
            (Some(start), Some(name)) => Some(start.join(name)),
            (start, name) => start.or(name.map(PathBuf::from)),
        };
        let filters = file_types(call, options)?;
        let paths = pick(call, "saveFile", path, filters)?;
        let path = paths.first().ok_or_else(|| Error::Dom("AbortError", "The user aborted a request.".into()))?;
        if !path.exists() {
            std::fs::write(path, []).map_err(|e| io_error(path, &e, true))?;
        }
        new_handle(call, path, false)
    })();
    settled(call, picked)
}

/// `showDirectoryPicker({ startIn })`: a Promise of the handle of the directory picked.
fn show_directory_picker<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let picked = (|| {
        let start = start_in(call, options(args))?;
        let paths = pick(call, "chooseDirectory", start, Vec::new())?;
        let path = paths.first().ok_or_else(|| Error::Dom("AbortError", "The user aborted a request.".into()))?;
        new_handle(call, path, true)
    })();
    settled(call, picked)
}
