//! Scriptographer's `File`: `new File(path)` or `new File(directory, name)` stands for a file or
//! folder, with what it is (`name`, `path`, `parent`, `extension`, `size`, `lastModified`,
//! `exists()`, `isDirectory()`, `isFile()`, `isHidden()`), what is in it (`readAll()`,
//! `readLines()`, `list([filter, recursive])`), and what changes it (`open([{ append }])`,
//! `write(text)`, `writeln(text)` and `close()`, `makeDirectory()`, `renameTo(file)`,
//! `copyTo(file)`, `remove([recursive])`). A relative path is relative to the script's folder.
//!
//! Without a file system (the web), what reads or changes files throws.

use std::path::{Path, PathBuf};

use crate::engine::HostFunction;
use crate::{Call, Engine, Error, Runtime, Value, define_getter, method, object};

/// What `File` objects share: their prototype, and `Object`, which makes them from it.
pub(crate) struct Files<H> {
    prototype: H,
    object: H,
}

/// Sets `File` on `global`.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let constructor = runtime.engine.constructor("File", construct)?;
    let methods = methods().into_iter().map(|(name, f)| Ok((name, runtime.engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let getters = getters().into_iter().map(|(name, f)| Ok((name, runtime.engine.function(name, f)?))).collect::<Result<Vec<_>, Error>>()?;
    let files = runtime.engine.with_call(|call| {
        let prototype = call.new_object()?;
        for (name, function) in methods {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        for (name, getter) in getters {
            define_getter(call, global, &prototype, name, getter)?;
        }
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(global, "File", &Value::Js(constructor))?;
        Ok(Files { prototype, object: object(call, global, "Object")? })
    })?;
    runtime.host.borrow_mut().files = Some(files);
    Ok(())
}

/// The methods of `File` objects.
fn methods<H: Clone>() -> Vec<(&'static str, HostFunction<H>)> {
    vec![
        ("toString", |call, this, _| Ok(Value::String(display(&path(call, this)?)))),
        ("exists", |call, this, _| Ok(Value::Bool(path(call, this)?.exists()))),
        ("isDirectory", |call, this, _| Ok(Value::Bool(path(call, this)?.is_dir()))),
        ("isFile", |call, this, _| Ok(Value::Bool(path(call, this)?.is_file()))),
        ("isHidden", |call, this, _| Ok(Value::Bool(path(call, this)?.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))))),
        ("readAll", |call, this, _| Ok(Value::String(read(&path(call, this)?)?))),
        ("readLines", read_lines),
        ("list", list),
        ("open", open),
        ("write", |call, this, args| write(call, this, args, false)),
        ("writeln", |call, this, args| write(call, this, args, true)),
        ("close", close),
        ("makeDirectory", |call, this, _| Ok(Value::Bool(std::fs::create_dir_all(path(call, this)?).is_ok()))),
        ("renameTo", |call, this, args| Ok(Value::Bool(std::fs::rename(path(call, this)?, target(call, args)?).is_ok()))),
        ("copyTo", |call, this, args| Ok(Value::Bool(std::fs::copy(path(call, this)?, target(call, args)?).is_ok()))),
        ("remove", remove),
    ]
}

/// The properties of `File` objects.
fn getters<H: Clone>() -> Vec<(&'static str, HostFunction<H>)> {
    vec![
        ("path", |call, this, _| Ok(Value::String(display(&path(call, this)?)))),
        ("name", |call, this, _| Ok(Value::String(path(call, this)?.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))),
        ("extension", |call, this, _| Ok(Value::String(path(call, this)?.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()))),
        ("parent", |call, this, _| match path(call, this)?.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(parent) => new_file(call, parent),
            None => Ok(Value::Null),
        }),
        ("size", |call, this, _| Ok(Value::Number(std::fs::metadata(path(call, this)?).map_or(0.0, |m| m.len() as f64)))),
        ("lastModified", |call, this, _| {
            let modified = std::fs::metadata(path(call, this)?).and_then(|m| m.modified()).ok();
            let since = modified.and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
            Ok(Value::Number(since.map_or(0.0, |d| d.as_secs_f64() * 1000.0)))
        }),
    ]
}

/// A `File` object for `path`, made absolute against the script's folder.
pub(crate) fn new_file<H: Clone>(call: &mut dyn Call<H>, path: &Path) -> Result<Value<H>, Error> {
    let (prototype, object_class, directory) = {
        let host = call.host()?;
        let host = host.borrow();
        let files = host.files.as_ref().ok_or_else(|| Error::Script("File is not set up".into()))?;
        (files.prototype.clone(), files.object.clone(), host.directory.clone())
    };
    let path = match directory {
        Some(directory) if path.is_relative() => directory.join(path),
        _ => path.to_owned(),
    };
    let file = method(call, &object_class, "create", &[Value::Js(prototype)])?;
    if let Value::Js(handle) = &file {
        call.set(handle, "_path", &Value::String(display(&path)))?;
    }
    Ok(file)
}

/// `new File(path)` or `new File(directory, name)`, `directory` a path or a `File`.
fn construct<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut path = match args.first() {
        Some(first) => PathBuf::from(call.to_string(first)?),
        None => return Err(Error::Script("File: no path".into())),
    };
    if let Some(name) = args.get(1).filter(|n| !matches!(n, Value::Undefined | Value::Null)) {
        path.push(call.to_string(name)?);
    }
    new_file(call, &path)
}

/// The path of the `File` that is `this`.
fn path<H>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<PathBuf, Error> {
    let Value::Js(file) = this else {
        return Err(Error::Script("not a File".into()));
    };
    match call.get(file, "_path")? {
        Value::String(path) => Ok(PathBuf::from(path)),
        _ => Err(Error::Script("not a File".into())),
    }
}

/// The file the first of `args` names: a `File` or a path.
fn target<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<PathBuf, Error> {
    let first = args.first().ok_or_else(|| Error::Script("no file given".into()))?;
    let named = call.to_string(first)?;
    let file = new_file(call, Path::new(&named))?;
    path(call, &file)
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The text in the file at `path`.
fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|e| Error::Script(format!("{}: {e}", display(path))))
}

/// `readLines()`: the file's lines, without their line ends.
fn read_lines<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = read(&path(call, this)?)?;
    let lines: Vec<_> = text.lines().map(|l| Value::String(l.to_owned())).collect();
    Ok(Value::Js(call.new_array(&lines)?))
}

/// `list([filter, recursive])`: the files in the folder, those whose name the regular expression
/// `filter` matches or for which the function `filter` returns true; with `recursive`, those in
/// its folders too. Empty for a file.
fn list<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let filter = match args.first() {
        Some(Value::Js(filter)) => Some(filter.clone()),
        _ => None,
    };
    let recursive = matches!(args.get(1), Some(Value::Bool(true)));
    let mut files = Vec::new();
    let folder = path(call, this)?;
    collect(call, &folder, filter.as_ref(), recursive, &mut files)?;
    Ok(Value::Js(call.new_array(&files)?))
}

fn collect<H: Clone>(call: &mut dyn Call<H>, folder: &Path, filter: Option<&H>, recursive: bool, files: &mut Vec<Value<H>>) -> Result<(), Error> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(());
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let file = new_file(call, &path)?;
        let name = Value::String(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let accepted = match filter {
            None => true,
            // A regular expression tests the name; a function gets the file.
            Some(filter) if matches!(call.get(filter, "test")?, Value::Js(_)) => matches!(method(call, filter, "test", &[name])?, Value::Bool(true)),
            Some(filter) => matches!(call.call(filter, &Value::Undefined, std::slice::from_ref(&file))?, Value::Bool(true)),
        };
        if accepted {
            files.push(file);
        }
        if recursive && path.is_dir() {
            collect(call, &path, filter, true, files)?;
        }
    }
    Ok(())
}

/// `open([{ append }])`: opens the file for `write()`, empty unless `append`.
fn open<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let append = match args.first() {
        Some(Value::Js(options)) => matches!(call.get(options, "append")?, Value::Bool(true)),
        _ => false,
    };
    let path = path(call, this)?;
    let opened = if append { std::fs::OpenOptions::new().create(true).append(true).open(&path) } else { std::fs::File::create(&path) };
    opened.map_err(|e| Error::Script(format!("{}: {e}", display(&path))))?;
    if let Value::Js(file) = this {
        call.set(file, "_opened", &Value::Bool(true))?;
    }
    Ok(Value::Bool(true))
}

/// `write(text)`, or `writeln(text)` with a line end: adds to the file opened with `open()`.
fn write<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>], line: bool) -> Result<Value<H>, Error> {
    use std::io::Write as _;
    let Value::Js(file) = this else {
        return Err(Error::Script("not a File".into()));
    };
    if !matches!(call.get(file, "_opened")?, Value::Bool(true)) {
        return Err(Error::Script("File: not opened".into()));
    }
    let mut text = match args.first() {
        Some(what) => call.to_string(what)?,
        None => String::new(),
    };
    if line {
        text.push('\n');
    }
    let path = path(call, this)?;
    let written = std::fs::OpenOptions::new().append(true).open(&path).and_then(|mut f| f.write_all(text.as_bytes()));
    written.map_err(|e| Error::Script(format!("{}: {e}", display(&path))))?;
    Ok(Value::Bool(true))
}

/// `close()`: what `write()` adds needs `open()` again.
fn close<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Value::Js(file) = this {
        call.set(file, "_opened", &Value::Bool(false))?;
    }
    Ok(Value::Bool(true))
}

/// `remove([recursive])`: removes the file, or the folder when it's empty or `recursive`.
fn remove<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let path = path(call, this)?;
    let removed = if path.is_dir() {
        if matches!(args.first(), Some(Value::Bool(true))) { std::fs::remove_dir_all(&path) } else { std::fs::remove_dir(&path) }
    } else {
        std::fs::remove_file(&path)
    };
    Ok(Value::Bool(removed.is_ok()))
}
