//! The events the host fires (`message`, `open`, `close`, `abort`…), as scripts would: through the
//! realm's event classes and `dispatchEvent()`, which are JavaScript (`js/src/events.js`) or, in a
//! browser, the browser's.

use crate::{Call, Error, Value, method};

/// A new event of the class `class` (`Event`, `MessageEvent`…) and type `kind`, with `members` as
/// its init dictionary.
pub(crate) fn new_event<H: Clone>(call: &mut dyn Call<H>, class: &str, kind: &str, members: &[(&str, Value<H>)]) -> Result<Value<H>, Error> {
    let constructor = crate::global_class(call, class)?;
    let init = call.new_object()?;
    for (key, value) in members {
        call.set(&init, key, value)?;
    }
    call.construct(&constructor, &[Value::String(kind.to_owned()), Value::Js(init)])
}

/// Fires `event` at `target`: to its `on…` handler for the event's type, then to its listeners.
/// Whether nothing prevented its default.
pub(crate) fn fire<H: Clone>(call: &mut dyn Call<H>, target: &Value<H>, event: &Value<H>) -> Result<bool, Error> {
    let Value::Js(object) = target else {
        return Ok(true);
    };
    let Value::Js(event_object) = event else {
        return Err(Error::Type("not an Event".into()));
    };
    let kind = call.get(event_object, "type")?;
    let handler = format!("on{}", call.to_string(&kind)?);
    if let Value::Js(handler) = call.get(object, &handler)? {
        // A handler that throws is reported, as a listener's is.
        if let Err(e) = call.call(&handler, target, std::slice::from_ref(event)) {
            call.host()?.borrow_mut().output.push(format!("Uncaught {e}"));
        }
    }
    method(call, object, "dispatchEvent", std::slice::from_ref(event)).map(|done| crate::truthy(&done))
}
