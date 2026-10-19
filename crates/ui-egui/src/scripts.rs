//! Window › PaperCraft › Run Script…: runs a PaperScript file (`.js`) on the active document with
//! PaperCraft. The script sees the document as a Paper.js project, and what it changes comes back
//! as one undo step. `ui.runScript {path?}`; without a path it asks for the file.
//!
//! The script's view is the first artboard, so `view.bounds` is its size. Output from
//! `console.log` shows in the status bar.

use serde_json::{Value, json};
use vectorcraft_papercraft::{PlatformEngine, Runtime};

use crate::{FilePick, VectorcraftApp};

/// Script files.
const EXTS: &[&str] = &["js"];

/// `ui.runScript`: run the script at `path`, or the file the user picks.
pub(crate) fn run(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    if app.services.open_async.is_some() {
        return Err("Running scripts needs the desktop app".into());
    }
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => path.to_owned(),
        None => {
            let pick = FilePick { filters: vec![("Scripts", EXTS)], ..Default::default() };
            crate::picks::open(app, &pick).ok_or("cancelled")?
        }
    };
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let state = app.session.doc().map_err(|e| e.to_string())?;
    let (doc, selection) = (state.doc.clone(), state.selection.clone());
    let size = doc.artboards.first().map_or((800.0, 600.0), |a| (a.rect.width(), a.rect.height()));

    let mut runtime = Runtime::<PlatformEngine>::new().map_err(|e| e.to_string())?;
    let scope = runtime.create_scope(size.0, size.1).map_err(|e| e.to_string())?;
    runtime.bind(&scope, doc, selection).map_err(|e| e.to_string())?;
    let result = runtime.execute(&scope, &source);
    let output = runtime.take_output();
    // What the script changed before an error still goes back, as in a browser.
    let patch = runtime.take_patch(&scope).map_err(|e| e.to_string())?;
    let changed = match patch {
        Some(patch) => {
            app.run("document.patch", patch.params)?;
            true
        }
        None => false,
    };
    if let Err(e) = result {
        let path = std::path::Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
        return Err(format!("{path}: {e}"));
    }
    if let Some(line) = output.last() {
        app.status(line.clone());
    }
    Ok(json!({ "output": output, "changed": changed }))
}
