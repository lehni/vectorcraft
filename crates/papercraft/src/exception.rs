//! The exceptions host functions throw: an [`Error`] becomes a `TypeError`, `RangeError`,
//! `DOMException` or `Error` of the script's realm, as browsers throw them, so scripts can tell
//! them apart with `instanceof` and `name`. Realms without `DOMException` (V8 and Boa outside a
//! browser) get one, built on `Error.prototype`, with `name`, `message`, `code` and the legacy
//! code constants.

use crate::{Call, Engine, Error, Value, object};

/// The legacy codes of `DOMException`'s names, and their constants (`INDEX_SIZE_ERR` …).
const CODES: [(&str, &str, u8); 25] = [
    ("IndexSizeError", "INDEX_SIZE_ERR", 1),
    ("DOMStringSizeError", "DOMSTRING_SIZE_ERR", 2),
    ("HierarchyRequestError", "HIERARCHY_REQUEST_ERR", 3),
    ("WrongDocumentError", "WRONG_DOCUMENT_ERR", 4),
    ("InvalidCharacterError", "INVALID_CHARACTER_ERR", 5),
    ("NoDataAllowedError", "NO_DATA_ALLOWED_ERR", 6),
    ("NoModificationAllowedError", "NO_MODIFICATION_ALLOWED_ERR", 7),
    ("NotFoundError", "NOT_FOUND_ERR", 8),
    ("NotSupportedError", "NOT_SUPPORTED_ERR", 9),
    ("InUseAttributeError", "INUSE_ATTRIBUTE_ERR", 10),
    ("InvalidStateError", "INVALID_STATE_ERR", 11),
    ("SyntaxError", "SYNTAX_ERR", 12),
    ("InvalidModificationError", "INVALID_MODIFICATION_ERR", 13),
    ("NamespaceError", "NAMESPACE_ERR", 14),
    ("InvalidAccessError", "INVALID_ACCESS_ERR", 15),
    ("ValidationError", "VALIDATION_ERR", 16),
    ("TypeMismatchError", "TYPE_MISMATCH_ERR", 17),
    ("SecurityError", "SECURITY_ERR", 18),
    ("NetworkError", "NETWORK_ERR", 19),
    ("AbortError", "ABORT_ERR", 20),
    ("URLMismatchError", "URL_MISMATCH_ERR", 21),
    ("QuotaExceededError", "QUOTA_EXCEEDED_ERR", 22),
    ("TimeoutError", "TIMEOUT_ERR", 23),
    ("InvalidNodeTypeError", "INVALID_NODE_TYPE_ERR", 24),
    ("DataCloneError", "DATA_CLONE_ERR", 25),
];

/// The realm's error classes, and its `Object`.
pub(crate) struct Classes<H> {
    object: H,
    error: H,
    type_error: H,
    range_error: H,
    dom_exception: H,
    /// `DOMException.prototype`, when the runtime made the class.
    prototype: Option<H>,
}

/// Keeps the realm's error classes, and sets `DOMException` on `global` unless it has one.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let constructor = engine.constructor("DOMException", construct)?;
    engine.with_call(|call| {
        let error = object(call, global, "Error")?;
        let object_class = object(call, global, "Object")?;
        let (dom_exception, prototype) = match call.get(global, "DOMException")? {
            Value::Js(native) => (native, None),
            _ => {
                let error_prototype = call.get(&error, "prototype")?;
                let Value::Js(prototype) = crate::method(call, &object_class, "create", &[error_prototype])? else {
                    return Err(Error::Script("Object.create made nothing".into()));
                };
                call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
                call.set(&prototype, "name", &Value::String("Error".into()))?;
                for (_, constant, code) in CODES {
                    call.set(&prototype, constant, &Value::Number(f64::from(code)))?;
                    call.set(&constructor, constant, &Value::Number(f64::from(code)))?;
                }
                call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
                call.set(global, "DOMException", &Value::Js(constructor.clone()))?;
                crate::tag(call, global, &prototype, "DOMException")?;
                (constructor, Some(prototype))
            }
        };
        let classes = Classes {
            object: object_class,
            type_error: object(call, global, "TypeError")?,
            range_error: object(call, global, "RangeError")?,
            error,
            dom_exception,
            prototype,
        };
        call.host()?.borrow_mut().exceptions = Some(classes);
        Ok(())
    })
}

/// `new DOMException(message, name)`.
fn construct<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let message = match args.first() {
        Some(Value::Undefined) | None => String::new(),
        Some(message) => call.to_string(message)?,
    };
    let name = match args.get(1) {
        Some(Value::Undefined) | None => "Error".to_owned(),
        Some(name) => call.to_string(name)?,
    };
    let classes = call.host()?.borrow().exceptions.as_ref().and_then(|c| Some((c.object.clone(), c.prototype.clone()?)));
    let Some((object_class, prototype)) = classes else {
        return Err(Error::Script("DOMException is not set up".into()));
    };
    let Value::Js(exception) = crate::method(call, &object_class, "create", &[Value::Js(prototype)])? else {
        return Err(Error::Script("Object.create made nothing".into()));
    };
    let code = CODES.iter().find(|(n, _, _)| *n == name).map_or(0, |(_, _, code)| *code);
    for (key, value) in [("name", Value::String(name)), ("message", Value::String(message)), ("code", Value::Number(f64::from(code)))] {
        call.set(&exception, key, &value)?;
    }
    Ok(Value::Js(exception))
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
