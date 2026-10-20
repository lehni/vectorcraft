//! `import` in scripts: PaperScript compiles `import … from 'specifier'` into calls to the loader
//! [`load`], which resolves a path against the importing file's folder (the script's for its own
//! imports), compiles the module as PaperScript in the script's scope, with operator overloading
//! and `export`, and gives back its exports. Each module runs once per runtime; a cycle throws.

use std::path::{Path, PathBuf};

use crate::{Call, Error, Value, method, object};

/// A module the runtime loaded, or is loading.
pub(crate) struct Module<H> {
    path: PathBuf,
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
    if specifier.starts_with("https:") || specifier.starts_with("http:") {
        return Err(Error::Script(format!("import '{specifier}': modules from the web aren't supported yet")));
    }
    let importer = match args.get(1) {
        Some(Value::String(url)) if !url.is_empty() => Some(PathBuf::from(url)),
        _ => None,
    };
    let folder = match importer.as_deref().and_then(Path::parent) {
        Some(folder) => Some(folder.to_path_buf()),
        None => call.host()?.borrow().directory.clone(),
    };
    let path = match folder {
        Some(folder) if Path::new(&specifier).is_relative() => folder.join(&specifier),
        _ => PathBuf::from(&specifier),
    };
    // One module, however its importers spell its path.
    let path = std::fs::canonicalize(&path).map_err(|e| Error::Script(format!("import '{specifier}': {e}")))?;
    let known = call.host()?.borrow().modules.iter().find(|m| m.path == path).map(|m| m.exports.clone());
    match known {
        Some(Some(exports)) => return Ok(exports),
        Some(None) => return Err(Error::Script(format!("import '{specifier}': it imports itself, through its imports"))),
        None => {}
    }
    let source = std::fs::read_to_string(&path).map_err(|e| Error::Script(format!("import '{specifier}': {e}")))?;
    let (paperscript, scope) = {
        let host = call.host()?;
        let host = host.borrow();
        let loader = host.loader.as_ref().ok_or_else(|| Error::Script("import: no script is running".into()))?;
        (loader.paperscript.clone(), loader.scope.clone())
    };
    call.host()?.borrow_mut().modules.push(Module { path: path.clone(), exports: None });
    let options = options(call, &path.to_string_lossy())?;
    let exports = method(call, &paperscript, "execute", &[Value::String(source), Value::Js(scope), Value::Js(options)]);
    let exports = match exports {
        Ok(exports) => exports,
        Err(e) => {
            // A module that failed loads again the next time it's imported.
            call.host()?.borrow_mut().modules.retain(|m| m.path != path);
            return Err(e);
        }
    };
    if let Some(module) = call.host()?.borrow_mut().modules.iter_mut().find(|m| m.path == path) {
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
