//! `import` in scripts: PaperScript compiles `import … from 'specifier'` into calls to the loader
//! [`load`], which resolves a path against the importing file's folder (the script's for its own
//! imports), or a URL (an `https:` specifier, or a relative one in a module from the web, which
//! loads on desktop and shows in the console), compiles the module in the script's scope, with
//! `export`, and gives back its exports. A `.pjs` module is PaperScript, with operator overloading,
//! any other plain JavaScript, unless the import says which with `with { type: 'paperscript' }` or
//! `with { type: 'javascript' }`. Each module runs once per runtime, per kind; a cycle throws.

use std::path::{Path, PathBuf};

use crate::{Call, Error, Value, method, object};

/// A module the runtime loaded, or is loading.
pub(crate) struct Module<H> {
    /// Its canonical path, or its URL.
    key: String,
    /// Whether it runs as PaperScript.
    paperscript: bool,
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
/// it may define, the loader, and without `paperscript`, no operator overloading.
pub(crate) fn options<H: Clone>(call: &mut dyn Call<H>, url: &str, paperscript: bool) -> Result<H, Error> {
    let options = call.new_object()?;
    if !paperscript {
        let features = call.new_object()?;
        call.set(&features, "operatorOverloading", &Value::Bool(false))?;
        call.set(&options, "paperFeatures", &Value::Js(features))?;
    }
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

/// `load(specifier, url, attributes)`: the exports of the module `specifier` names, imported by
/// the code at `url`, as the import's `attributes` say, `{ type }`.
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
    let as_paperscript = match args.get(2) {
        Some(Value::Js(attributes)) => match call.get(attributes, "type")? {
            Value::Undefined => is_paperscript(&key),
            kind => match call.to_string(&kind)?.as_str() {
                "paperscript" => true,
                "javascript" => false,
                kind => return Err(Error::Type(format!("import '{specifier}': '{kind}' isn't a type of module"))),
            },
        },
        _ => is_paperscript(&key),
    };
    let known = call.host()?.borrow().modules.iter().find(|m| m.key == key && m.paperscript == as_paperscript).map(|m| m.exports.clone());
    match known {
        Some(Some(exports)) => return Ok(exports),
        Some(None) => return Err(Error::Script(format!("import '{specifier}': it imports itself, through its imports"))),
        None => {}
    }
    if is_web(&key) {
        call.host()?.borrow_mut().output.push(format!("import {key}"));
    }
    let cache = call.host()?.borrow().cache.clone();
    let source = source(&key, cache.as_deref()).map_err(|e| Error::Script(format!("import '{specifier}': {e}")))?;
    let (paperscript, scope) = {
        let host = call.host()?;
        let host = host.borrow();
        let loader = host.loader.as_ref().ok_or_else(|| Error::Script("import: no script is running".into()))?;
        (loader.paperscript.clone(), loader.scope.clone())
    };
    call.host()?.borrow_mut().modules.push(Module { key: key.clone(), paperscript: as_paperscript, exports: None });
    let options = options(call, &key, as_paperscript)?;
    let exports = method(call, &paperscript, "execute", &[Value::String(source), Value::Js(scope), Value::Js(options)]);
    let exports = match exports {
        Ok(exports) => exports,
        Err(e) => {
            // A module that failed loads again the next time it's imported.
            call.host()?.borrow_mut().modules.retain(|m| m.key != key || m.paperscript != as_paperscript);
            return Err(e);
        }
    };
    if let Some(module) = call.host()?.borrow_mut().modules.iter_mut().find(|m| m.key == key && m.paperscript == as_paperscript) {
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

/// Whether the module at `key` is PaperScript by its name: a `.pjs` file.
fn is_paperscript(key: &str) -> bool {
    let path = key.split(['?', '#']).next().unwrap_or(key);
    path.rsplit_once('.').is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pjs"))
}

fn is_web(key: &str) -> bool {
    url::Url::parse(key).is_ok_and(|url| matches!(url.scheme(), "https" | "http"))
}

/// The text of the script at `key` ([`locate`]): downloaded when it's a URL, with its copy in
/// `cache`, else read.
pub(crate) fn source(key: &str, cache: Option<&Path>) -> Result<String, String> {
    if is_web(key) { download(key, cache) } else { std::fs::read_to_string(key).map_err(|e| e.to_string()) }
}

/// What the cache keeps of a download besides its text: what tells the server which one it is.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Cached {
    url: String,
    etag: Option<String>,
    modified: Option<String>,
}

/// The text at `url`, downloaded on the script's thread. With a `cache`, the copy kept there is
/// what the server says is still the one (`304 Not Modified`, asked by its `ETag` or
/// `Last-Modified`), and what stands in when the server can't be reached or fails (5xx); each
/// download is kept there, a file named for its URL, with what tells the server which it is.
#[cfg(not(target_arch = "wasm32"))]
fn download(url: &str, cache: Option<&Path>) -> Result<String, String> {
    use sha2::Digest as _;
    let name: String = sha2::Sha256::digest(url.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    let files = cache.map(|cache| (cache.join(format!("{name}.js")), cache.join(format!("{name}.json"))));
    let kept = files.as_ref().and_then(|(text, about)| {
        let about: Cached = serde_json::from_slice(&std::fs::read(about).ok()?).ok().filter(|a: &Cached| a.url == url)?;
        Some((std::fs::read_to_string(text).ok()?, about))
    });
    let mut asking = Vec::new();
    if let Some((_, about)) = &kept {
        asking.extend(about.etag.clone().map(|etag| ("If-None-Match".to_owned(), etag)));
        asking.extend(about.modified.clone().map(|modified| ("If-Modified-Since".to_owned(), modified)));
    }
    let answer = match crate::fetch::get(url, &asking) {
        Ok(answer) => answer,
        Err(e) => return kept.map(|(text, _)| text).ok_or(e),
    };
    match (answer.status, kept) {
        (304, Some((text, _))) => Ok(text),
        (500..=599, Some((text, _))) => Ok(text),
        (200..=299, _) => {
            let text = String::from_utf8_lossy(&answer.body).into_owned();
            if let Some((text_file, about_file)) = files {
                let header = |name: &str| answer.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
                let about = Cached { url: url.to_owned(), etag: header("etag"), modified: header("last-modified") };
                // A copy that can't be kept only costs the next download.
                let _ = keep(&text_file, &about_file, &text, &about);
            }
            Ok(text)
        }
        (status, _) => Err(format!("{url} answered {status}")),
    }
}

/// Keeps a download's `text` and what's `about` it.
#[cfg(not(target_arch = "wasm32"))]
fn keep(text_file: &Path, about_file: &Path, text: &str, about: &Cached) -> Result<(), String> {
    if let Some(folder) = text_file.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    std::fs::write(text_file, text).map_err(|e| e.to_string())?;
    std::fs::write(about_file, serde_json::to_vec(about).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn download(_url: &str, _cache: Option<&Path>) -> Result<String, String> {
    Err("modules from the web aren't supported in the web build yet".into())
}
