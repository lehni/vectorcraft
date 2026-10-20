//! What the runtime reads and makes of the `Blob`s of `js/src/blob.js`, which gives the runtime
//! their bytes (`getBytes()`), for structured cloning.

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

pub(crate) fn blob_bytes<H: Clone>(call: &mut dyn Call<H>, blob: &Value<H>) -> Result<Vec<u8>, Error> {
    let bytes = crate::call_bundle(call, "getBytes", std::slice::from_ref(blob))?;
    call.bytes(&crate::handle(bytes, "the blob's bytes")?)
}
