//! What the runtime reads and makes of the realm's `Blob`s: the browser's, or outside a browser
//! those of `js/src/blob.js`, which gives the runtime their bytes (`getBytes()`) and those of a
//! request's body (`getBody()`).

use crate::{Call, Error, Value};

/// A `Blob` of `bytes`, of MIME type `kind`.
pub(crate) fn new_blob<H: Clone>(call: &mut dyn Call<H>, bytes: &[u8], kind: &str) -> Result<Value<H>, Error> {
    let blob = crate::global_class(call, "Blob")?;
    let bytes = call.new_uint8_array(bytes)?;
    let parts = Value::Js(call.new_array(&[Value::Js(bytes)])?);
    let options = call.new_object()?;
    call.set(&options, "type", &Value::String(kind.to_owned()))?;
    call.construct(&blob, &[parts, Value::Js(options)])
}

/// Whether `value` is a `Blob`, a `File` or a `LocalFile`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_blob<H: Clone>(call: &mut dyn Call<H>, value: &H) -> Result<bool, Error> {
    let blob = crate::global_class(call, "Blob")?;
    let prototype = crate::handle(call.get(&blob, "prototype")?, "Blob.prototype")?;
    Ok(matches!(crate::method(call, &prototype, "isPrototypeOf", &[Value::Js(value.clone())])?, Value::Bool(true)))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn blob_bytes<H: Clone>(call: &mut dyn Call<H>, blob: &Value<H>) -> Result<Vec<u8>, Error> {
    let bytes = crate::call_bundle(call, "getBytes", std::slice::from_ref(blob))?;
    call.bytes(&crate::handle(bytes, "the blob's bytes")?)
}

/// A body's bytes and the content type it implies: a string, bytes, a `Blob` or `FormData`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn body_bytes<H: Clone>(call: &mut dyn Call<H>, body: &Value<H>) -> Result<(Vec<u8>, Option<String>), Error> {
    let body = crate::handle(crate::call_bundle(call, "getBody", std::slice::from_ref(body))?, "the body")?;
    let bytes = crate::handle(call.get(&body, "bytes")?, "the body's bytes")?;
    let kind = match call.get(&body, "type")? {
        Value::String(kind) => Some(kind),
        _ => None,
    };
    Ok((call.bytes(&bytes)?, kind))
}
