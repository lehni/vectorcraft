//! What `LocalFile` and the File System Access API on desktop (`js/src/local-file.js` and
//! `js/src/file-system.js`) need of the runtime: the file system, by path. A relative path is from
//! the script's folder. What fails throws the `DOMException` browsers give: `NotFoundError`,
//! `NotAllowedError`, …

use std::path::{Path, PathBuf};

use crate::{Call, Engine, Error, HostFunction, Value};

/// The natives: `path(...parts)`, the parts joined as `path.join()` joins them (an absolute part
/// starts the path again), `info(path)`, `read(path)`, `write(path, bytes, append)`, `list(path)`,
/// `makeDirectory(path, recursive)`, `remove(path, recursive)`, `rename(path, nameOrPath)`, which
/// gives the new path, `copy(from, to)`, `relative(directory, path)`, the names from the directory
/// down to the path or `null`, and `userDirectory(name)`, one of the user's directories.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 11] = [
        ("path", |call, _, args| {
            let mut path = PathBuf::new();
            for part in args {
                path.push(call.to_string(part)?);
            }
            Ok(Value::String(display(&absolute(call, &path)?)))
        }),
        ("info", info),
        ("read", |call, _, args| {
            let path = path(call, args, 0)?;
            let bytes = std::fs::read(&path).map_err(|e| io_error(&path, &e, false))?;
            Ok(Value::Js(call.new_uint8_array(&bytes)?))
        }),
        ("write", write),
        ("list", |call, _, args| {
            let directory = path(call, args, 0)?;
            let entries = std::fs::read_dir(&directory).map_err(|e| io_error(&directory, &e, false))?;
            let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            let paths: Vec<_> = paths.iter().map(|path| Value::String(display(path))).collect();
            Ok(Value::Js(call.new_array(&paths)?))
        }),
        ("makeDirectory", |call, _, args| {
            let path = path(call, args, 0)?;
            let made =
                if crate::truthy(args.get(1).unwrap_or(&Value::Undefined)) { std::fs::create_dir_all(&path) } else { std::fs::create_dir(&path) };
            made.map_err(|e| io_error(&path, &e, true))?;
            Ok(Value::Undefined)
        }),
        ("remove", |call, _, args| {
            let path = path(call, args, 0)?;
            let removed = match std::fs::metadata(&path) {
                Ok(metadata) if !metadata.is_dir() => std::fs::remove_file(&path),
                Ok(_) if crate::truthy(args.get(1).unwrap_or(&Value::Undefined)) => std::fs::remove_dir_all(&path),
                Ok(_) => std::fs::remove_dir(&path),
                Err(e) => Err(e),
            };
            removed.map_err(|e| io_error(&path, &e, true))?;
            Ok(Value::Undefined)
        }),
        // A name renames it where it is, a path moves it.
        ("rename", |call, _, args| {
            let from = path(call, args, 0)?;
            let given = call.to_string(args.get(1).unwrap_or(&Value::Undefined))?;
            let to = if Path::new(&given).components().count() == 1 { from.with_file_name(&given) } else { absolute(call, Path::new(&given))? };
            std::fs::rename(&from, &to).map_err(|e| io_error(&from, &e, true))?;
            Ok(Value::String(display(&to)))
        }),
        ("copy", |call, _, args| {
            let (from, to) = (path(call, args, 0)?, path(call, args, 1)?);
            std::fs::copy(&from, &to).map_err(|e| io_error(&from, &e, true))?;
            Ok(Value::Undefined)
        }),
        ("relative", |call, _, args| {
            let (directory, inner) = (path(call, args, 0)?, path(call, args, 1)?);
            match inner.strip_prefix(&directory) {
                Ok(relative) => {
                    let names: Vec<_> = relative.components().map(|c| Value::String(c.as_os_str().to_string_lossy().into_owned())).collect();
                    Ok(Value::Js(call.new_array(&names)?))
                }
                Err(_) => Ok(Value::Null),
            }
        }),
        ("userDirectory", |call, _, args| {
            let name = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
            let folder = match name.as_str() {
                "desktop" => "Desktop",
                "documents" => "Documents",
                "downloads" => "Downloads",
                "music" => "Music",
                "pictures" => "Pictures",
                "videos" if cfg!(target_os = "macos") => "Movies",
                "videos" => "Videos",
                _ => return Err(Error::Type(format!("Unknown directory: '{name}'."))),
            };
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
            Ok(home.map_or(Value::Null, |home| Value::String(display(&PathBuf::from(home).join(folder)))))
        }),
    ];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

/// What the path is: `{ name, parent, extension, exists, isDirectory, size, lastModified }`.
fn info<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let path = path(call, args, 0)?;
    let metadata = std::fs::metadata(&path).ok();
    let modified = metadata.as_ref().and_then(|m| m.modified().ok()).and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok());
    let text = |part: Option<&std::ffi::OsStr>| Value::String(part.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let info = call.new_object()?;
    for (key, value) in [
        ("name", text(path.file_name())),
        ("parent", path.parent().filter(|p| !p.as_os_str().is_empty()).map_or(Value::Null, |p| Value::String(display(p)))),
        ("extension", text(path.extension())),
        ("exists", Value::Bool(metadata.is_some())),
        ("isDirectory", Value::Bool(metadata.as_ref().is_some_and(std::fs::Metadata::is_dir))),
        ("size", Value::Number(metadata.as_ref().map_or(0.0, |m| m.len() as f64))),
        ("lastModified", Value::Number(modified.map_or(0.0, |d| d.as_millis() as f64))),
    ] {
        call.set(&info, key, &value)?;
    }
    Ok(Value::Js(info))
}

/// The file's contents become, or end with when appending, the bytes.
fn write<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let path = path(call, args, 0)?;
    let bytes = match args.get(1) {
        Some(Value::Js(bytes)) => call.bytes(bytes)?,
        _ => Vec::new(),
    };
    let written = if crate::truthy(args.get(2).unwrap_or(&Value::Undefined)) {
        use std::io::Write as _;
        std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut file| file.write_all(&bytes))
    } else {
        std::fs::write(&path, &bytes)
    };
    written.map_err(|e| io_error(&path, &e, true))?;
    Ok(Value::Undefined)
}

/// The path `args[n]` names, from the script's folder.
fn path<H>(call: &mut dyn Call<H>, args: &[Value<H>], n: usize) -> Result<PathBuf, Error> {
    let path = call.to_string(args.get(n).unwrap_or(&Value::Undefined))?;
    absolute(call, Path::new(&path))
}

/// `path`, from the script's folder when it's relative.
fn absolute<H>(call: &mut dyn Call<H>, path: &Path) -> Result<PathBuf, Error> {
    let directory = call.host()?.borrow().directory.clone();
    Ok(match directory {
        Some(directory) if path.is_relative() => directory.join(path),
        _ => path.to_owned(),
    })
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The `DOMException` browsers give for `error` on `path`: `writing` tells a change from a read.
fn io_error(path: &Path, error: &std::io::Error, writing: bool) -> Error {
    let name = match error.kind() {
        std::io::ErrorKind::NotFound => "NotFoundError",
        std::io::ErrorKind::PermissionDenied => "NotAllowedError",
        _ if writing => "InvalidModificationError",
        _ => "NotReadableError",
    };
    Error::Dom(name, format!("{}: {error}", display(path)))
}
