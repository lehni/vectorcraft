//! `import` in scripts: PaperScript compiles `import … from 'specifier'` into calls to the loader
//! [`load`], which resolves a path against the importing file's folder (the script's for its own
//! imports), or a URL (an `https:` specifier, or a relative one in a module from the web, which
//! loads on desktop and shows in the console), compiles the module as PaperScript in the script's
//! scope, with operator overloading and `export`, and gives back its exports. Each module runs
//! once per runtime; a cycle throws.

use std::path::{Path, PathBuf};

use crate::{Call, Error, Value, method, object};

/// A module the runtime loaded, or is loading.
pub(crate) struct Module<H> {
    /// Its canonical path, or its URL.
    key: String,
    /// Its exports, once it ran: whatever `module.exports` became.
    exports: Option<Value<H>>,
}

/// What the loader needs of the script it loads for: `PaperScript`, the script's scope, and the
/// loader itself, for the modules' own imports.
pub(crate) struct Loader<H> {
    pub paperscript: H,
    pub scope: H,
    pub load: H,
}

/// The `options` of `PaperScript.execute()` for code at `url`: the [`HANDLERS`](crate::HANDLERS)
/// it may define, and the loader.
pub(crate) fn options<H: Clone>(call: &mut dyn Call<H>, url: &str) -> Result<H, Error> {
    let options = call.new_object()?;
    let names: Vec<Value<H>> = crate::HANDLERS.iter().map(|h| Value::String((*h).into())).collect();
    let names = call.new_array(&names)?;
    call.set(&options, "handlers", &Value::Js(names))?;
    call.set(&options, "url", &Value::String(url.to_owned()))?;
    let load = call.host()?.borrow().loader.as_ref().map(|l| l.load.clone());
    if let Some(load) = load {
        call.set(&options, "load", &Value::Js(load))?;
    }
    Ok(options)
}

/// `load(specifier, url)`: the exports of the module `specifier` names, imported by the code at
/// `url`.
pub(crate) fn load<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let specifier = match args.first() {
        Some(specifier) => call.to_string(specifier)?,
        None => return Err(Error::Script("import: no module named".into())),
    };
    let importer = match args.get(1) {
        Some(Value::String(url)) if !url.is_empty() => Some(url.clone()),
        _ => None,
    };
    let directory = call.host()?.borrow().directory.clone();
    // One module, however its importers spell its path or URL.
    let key = locate(&specifier, importer.as_deref(), directory)?;
    let known = call.host()?.borrow().modules.iter().find(|m| m.key == key).map(|m| m.exports.clone());
    match known {
        Some(Some(exports)) => return Ok(exports),
        Some(None) => return Err(Error::Script(format!("import '{specifier}': it imports itself, through its imports"))),
        None => {}
    }
    if is_web(&key) {
        call.host()?.borrow_mut().output.push(format!("import {key}"));
    }
    let source = source(&key).map_err(|e| Error::Script(format!("import '{specifier}': {e}")))?;
    let (paperscript, scope) = {
        let host = call.host()?;
        let host = host.borrow();
        let loader = host.loader.as_ref().ok_or_else(|| Error::Script("import: no script is running".into()))?;
        (loader.paperscript.clone(), loader.scope.clone())
    };
    call.host()?.borrow_mut().modules.push(Module { key: key.clone(), exports: None });
    let options = options(call, &key)?;
    let exports = method(call, &paperscript, "execute", &[Value::String(source), Value::Js(scope), Value::Js(options)]);
    let exports = match exports {
        Ok(exports) => exports,
        Err(e) => {
            // A module that failed loads again the next time it's imported.
            call.host()?.borrow_mut().modules.retain(|m| m.key != key);
            return Err(e);
        }
    };
    if let Some(module) = call.host()?.borrow_mut().modules.iter_mut().find(|m| m.key == key) {
        module.exports = Some(exports.clone());
    }
    Ok(exports)
}

/// Sets up the loader for scripts in `scope`.
pub(crate) fn set_up<H: Clone>(call: &mut dyn Call<H>, paper: &H, scope: &H, load: H) -> Result<(), Error> {
    let paperscript = object(call, paper, "PaperScript")?;
    call.host()?.borrow_mut().loader = Some(Loader { paperscript, scope: scope.clone(), load });
    Ok(())
}

/// The module `specifier` names, imported by the code at `importer` (a path or a URL; the script's
/// folder when there is none), as its canonical path or its URL.
pub(crate) fn locate(specifier: &str, importer: Option<&str>, directory: Option<PathBuf>) -> Result<String, Error> {
    let fail = |e: &dyn std::fmt::Display| Error::Script(format!("import '{specifier}': {e}"));
    if let Ok(url) = url::Url::parse(specifier)
        && url.scheme() != "file"
    {
        return Ok(url.to_string());
    }
    if let Some(base) = importer.and_then(|i| url::Url::parse(i).ok()).filter(|u| matches!(u.scheme(), "https" | "http")) {
        return base.join(specifier).map(|u| u.to_string()).map_err(|e| fail(&e));
    }
    let folder = importer.and_then(|i| Path::new(i).parent().map(Path::to_path_buf)).or(directory);
    let path = match folder {
        Some(folder) if Path::new(specifier).is_relative() => folder.join(specifier),
        _ => PathBuf::from(specifier),
    };
    let path = std::fs::canonicalize(&path).map_err(|e| fail(&e))?;
    Ok(path.to_string_lossy().into_owned())
}

fn is_web(key: &str) -> bool {
    url::Url::parse(key).is_ok_and(|url| matches!(url.scheme(), "https" | "http"))
}

/// The text of the script at `key` ([`locate`]): downloaded when it's a URL, else read.
pub(crate) fn source(key: &str) -> Result<String, String> {
    if is_web(key) { download(key) } else { std::fs::read_to_string(key).map_err(|e| e.to_string()) }
}

/// The text at `url`, downloaded on the script's thread.
#[cfg(not(target_arch = "wasm32"))]
fn download(url: &str) -> Result<String, String> {
    let answer = crate::fetch::get(url)?;
    if !(200..300).contains(&answer.0) {
        return Err(format!("{url} answered {}", answer.0));
    }
    Ok(String::from_utf8_lossy(&answer.1).into_owned())
}

#[cfg(target_arch = "wasm32")]
fn download(_url: &str) -> Result<String, String> {
    Err("modules from the web aren't supported in the web build yet".into())
}
