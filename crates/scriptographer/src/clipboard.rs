//! `navigator.clipboard` for scripts outside a browser: `readText()` and `writeText(text)`, whose
//! Promises settle with the system clipboard's text, which the app reads and writes for the script
//! (as it answers its dialogs: [`crate::dialog::ask`]). Without the app (headless), they reject
//! with a `NotAllowedError`. A browser's realm has its own.

use serde_json::json;

use crate::{Call, Engine, Error, Value, method, object};

/// Sets `navigator.clipboard`, unless the realm's navigator has one.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let read = engine.function("readText", read_text)?;
    let write = engine.function("writeText", write_text)?;
    engine.with_call(|call| {
        let Value::Js(navigator) = call.get(global, "navigator")? else {
            return Ok(());
        };
        if !matches!(call.get(&navigator, "clipboard")?, Value::Undefined) {
            return Ok(());
        }
        let clipboard = call.new_object()?;
        call.set(&clipboard, "readText", &Value::Js(read))?;
        call.set(&clipboard, "writeText", &Value::Js(write))?;
        call.set(&navigator, "clipboard", &Value::Js(clipboard))?;
        let promise = object(call, global, "Promise")?;
        call.host()?.borrow_mut().clipboard_promise = Some(promise);
        Ok(())
    })
}

/// A Promise settled with `result`: resolved with its value, else rejected with a
/// `NotAllowedError`, as browsers deny the clipboard.
fn settled<H: Clone>(call: &mut dyn Call<H>, result: Result<Value<H>, Error>) -> Result<Value<H>, Error> {
    let promise = call.host()?.borrow().clipboard_promise.clone().ok_or_else(|| Error::Script("the clipboard is not set up".into()))?;
    match result {
        Ok(value) => method(call, &promise, "resolve", &[value]),
        Err(e) => {
            let reason = crate::exception::value(call, &Error::Dom("NotAllowedError", format!("the clipboard can't be used: {e}")))?;
            method(call, &promise, "reject", &[reason])
        }
    }
}

/// `readText()`: a Promise of the clipboard's text, empty when it has none.
fn read_text<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let answer = crate::dialog::ask(call, json!({ "type": "clipboardRead" }));
    let text = answer.map(|answer| Value::String(answer["text"].as_str().unwrap_or_default().to_owned()));
    settled(call, text)
}

/// `writeText(text)`: a Promise that resolves once the text is on the clipboard.
fn write_text<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(text) = args.first() else {
        return Err(Error::Type("writeText: the text is missing".into()));
    };
    let text = call.to_string(text)?;
    let written = crate::dialog::ask(call, json!({ "type": "clipboardWrite", "text": text })).map(|_| Value::Undefined);
    settled(call, written)
}
