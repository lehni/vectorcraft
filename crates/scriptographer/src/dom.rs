//! A DOM for scripts outside a browser: `window` and `document` are linkedom's, loaded the first
//! time a script reads either one, so scripts that never touch them pay nothing. Libraries that
//! build or query elements (d3's selections, SVG builders) work; there is no layout, network or
//! rendering behind it. In a browser, scripts have the page's own.

use crate::{Call, Engine, Error, Runtime, Value, define_accessor, object};

/// linkedom 0.18.13 and its dependencies as one script, which sets the global `linkedom`
/// (ISC, MIT and BSD-2-Clause; see `vendor/LICENSE-linkedom.txt`, and the file's first line for
/// how it was bundled).
const LINKEDOM: &str = include_str!("../vendor/linkedom.js");

/// Sets the getters `window` and `document` on `global`, unless it has a DOM already.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let window = runtime.engine.function("window", |call, _, _| view(call))?;
    let document = runtime.engine.function("document", |call, _, _| match view(call)? {
        Value::Js(view) => call.get(&view, "document"),
        other => Ok(other),
    })?;
    runtime.engine.with_call(|call| {
        if !matches!(call.get(global, "document")?, Value::Undefined) {
            return Ok(());
        }
        let function = object(call, global, "Function")?;
        call.host()?.borrow_mut().dom = Dom::Unloaded(function);
        define_accessor(call, global, global, "window", window, None)?;
        define_accessor(call, global, global, "document", document, None)
    })
}

/// The DOM's state in a runtime.
pub(crate) enum Dom<H> {
    /// The runtime has a DOM of its own, or none is set up.
    None,
    /// Not loaded yet: `Function`, which loads it.
    Unloaded(H),
    /// linkedom's window.
    Loaded(H),
}

/// linkedom's window, loaded on the first call.
fn view<H: Clone>(call: &mut dyn Call<H>) -> Result<Value<H>, Error> {
    let host = call.host()?;
    let state = std::mem::replace(&mut host.borrow_mut().dom, Dom::None);
    let view = match state {
        Dom::Loaded(view) => view,
        Dom::Unloaded(function) => {
            let source = format!("{LINKEDOM}\nreturn linkedom.parseHTML('<!doctype html><html><head></head><body></body></html>');");
            let load = call.construct(&function, &[Value::String(source)])?;
            match load {
                Value::Js(load) => match call.call(&load, &Value::Undefined, &[])? {
                    Value::Js(view) => view,
                    _ => return Err(Error::Script("linkedom made no window".into())),
                },
                _ => return Err(Error::Script("linkedom didn't load".into())),
            }
        }
        Dom::None => return Ok(Value::Undefined),
    };
    host.borrow_mut().dom = Dom::Loaded(view.clone());
    Ok(Value::Js(view))
}
