//! A DOM for scripts outside a browser (`js/src/dom.js`): `window` and `document` are linkedom's,
//! which the runtime loads the first time a script reads either one, so scripts that never touch
//! them pay nothing. Libraries that build or query elements (d3's selections, SVG builders) work;
//! there is no layout, network or rendering behind it.

use crate::{Call, Error, Value};

/// linkedom 0.18.13 and its dependencies as one script, which sets the global `linkedom`
/// (ISC, MIT and BSD-2-Clause; see `vendor/LICENSE-linkedom.txt`, and the file's first line for
/// how it was bundled).
const LINKEDOM: &str = include_str!("../vendor/linkedom.js");

/// `dom()`: a new window of linkedom's, with an empty document.
pub(crate) fn load<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let function = crate::global_class(call, "Function")?;
    let source = format!("{LINKEDOM}\nreturn linkedom.parseHTML('<!doctype html><html><head></head><body></body></html>');");
    let load = crate::handle(call.construct(&function, &[Value::String(source)])?, "linkedom")?;
    match call.call(&load, &Value::Undefined, &[])? {
        Value::Js(view) => Ok(Value::Js(view)),
        _ => Err(Error::Script("linkedom made no window".into())),
    }
}
