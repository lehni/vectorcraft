//! The exceptions host functions throw: an [`Error`] becomes a `TypeError`, `RangeError`,
//! `DOMException` or `Error` of the script's realm, as browsers throw them, so scripts can tell
//! them apart with `instanceof` and `name`.

use crate::{Call, Engine, Error, Value, object};

/// The realm's error classes.
pub(crate) struct Classes<H> {
    error: H,
    type_error: H,
    range_error: H,
    dom_exception: H,
}

/// Keeps the realm's error classes, `DOMException` the browser's or that of
/// `js/src/dom-exception.js`.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    engine.with_call(|call| {
        let classes = Classes {
            error: object(call, global, "Error")?,
            type_error: object(call, global, "TypeError")?,
            range_error: object(call, global, "RangeError")?,
            dom_exception: object(call, global, "DOMException")?,
        };
        call.host()?.borrow_mut().exceptions = Some(classes);
        Ok(())
    })
}

/// What a script gets for `error`: an exception of the realm's matching class.
pub(crate) fn value<H: Clone>(call: &mut dyn Call<H>, error: &Error) -> Result<Value<H>, Error> {
    let classes = {
        let host = call.host()?;
        let host = host.borrow();
        let classes = host.exceptions.as_ref().ok_or_else(|| Error::Script("the error classes are not set up".into()))?;
        (classes.error.clone(), classes.type_error.clone(), classes.range_error.clone(), classes.dom_exception.clone())
    };
    let (error_class, type_error, range_error, dom_exception) = classes;
    match error {
        Error::Script(message) => call.construct(&error_class, &[Value::String(message.clone())]),
        Error::Type(message) => call.construct(&type_error, &[Value::String(message.clone())]),
        Error::Range(message) => call.construct(&range_error, &[Value::String(message.clone())]),
        Error::Dom(name, message) => call.construct(&dom_exception, &[Value::String(message.clone()), Value::String((*name).to_owned())]),
    }
}
