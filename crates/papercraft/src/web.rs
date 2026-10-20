//! The browser's everyday APIs for scripts outside a browser, where the engine has none:
//! `performance.now()`, `queueMicrotask(callback)`, `TextEncoder` and `TextDecoder` (UTF-8),
//! `crypto.getRandomValues(array)` and `crypto.randomUUID()`. Each one is set only where the
//! global object lacks it, so a browser keeps its own.

use std::time::Instant;

use crate::{Call, Engine, Error, Runtime, Value, method, object};

/// What the APIs share: classes of the script's realm, the prototypes of `TextEncoder` and
/// `TextDecoder`, and when the runtime started.
pub(crate) struct Web<H> {
    promise: H,
    bytes: H,
    object: H,
    encoder: H,
    decoder: H,
    started: Instant,
}

/// Sets the APIs the global object lacks.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let now = runtime.engine.function("now", now)?;
    let queue_microtask = runtime.engine.function("queueMicrotask", queue_microtask)?;
    let text_encoder = runtime.engine.constructor("TextEncoder", text_encoder)?;
    let encode = runtime.engine.function("encode", encode)?;
    let text_decoder = runtime.engine.constructor("TextDecoder", text_decoder)?;
    let decode = runtime.engine.function("decode", decode)?;
    let get_random_values = runtime.engine.function("getRandomValues", get_random_values)?;
    let random_uuid = runtime.engine.function("randomUUID", random_uuid)?;
    let web = runtime.engine.with_call(|call| {
        let missing = |call: &mut dyn Call<E::Handle>, name: &str| -> Result<bool, Error> { Ok(matches!(call.get(global, name)?, Value::Undefined)) };
        if missing(call, "performance")? {
            let performance = call.new_object()?;
            call.set(&performance, "now", &Value::Js(now))?;
            call.set(global, "performance", &Value::Js(performance))?;
        }
        if missing(call, "queueMicrotask")? {
            call.set(global, "queueMicrotask", &Value::Js(queue_microtask))?;
        }
        let encoder = call.new_object()?;
        call.set(&encoder, "encoding", &Value::String("utf-8".into()))?;
        call.set(&encoder, "encode", &Value::Js(encode))?;
        if missing(call, "TextEncoder")? {
            call.set(&text_encoder, "prototype", &Value::Js(encoder.clone()))?;
            call.set(global, "TextEncoder", &Value::Js(text_encoder))?;
        }
        let decoder = call.new_object()?;
        call.set(&decoder, "encoding", &Value::String("utf-8".into()))?;
        call.set(&decoder, "decode", &Value::Js(decode))?;
        if missing(call, "TextDecoder")? {
            call.set(&text_decoder, "prototype", &Value::Js(decoder.clone()))?;
            call.set(global, "TextDecoder", &Value::Js(text_decoder))?;
        }
        if missing(call, "crypto")? {
            let crypto = call.new_object()?;
            call.set(&crypto, "getRandomValues", &Value::Js(get_random_values))?;
            call.set(&crypto, "randomUUID", &Value::Js(random_uuid))?;
            call.set(global, "crypto", &Value::Js(crypto))?;
        }
        Ok(Web {
            promise: object(call, global, "Promise")?,
            bytes: object(call, global, "Uint8Array")?,
            object: object(call, global, "Object")?,
            encoder,
            decoder,
            started: Instant::now(),
        })
    })?;
    runtime.host.borrow_mut().web = Some(web);
    Ok(())
}

/// `Promise`, `Uint8Array` and the start, from the host.
fn shared<H: Clone>(call: &mut dyn Call<H>) -> Result<(H, H, Instant), Error> {
    with_web(call, |web| (web.promise.clone(), web.bytes.clone(), web.started))
}

fn with_web<H: Clone, R>(call: &mut dyn Call<H>, f: impl FnOnce(&Web<H>) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let host = host.borrow();
    host.web.as_ref().map(f).ok_or_else(|| Error::Script("the browser APIs aren't set up".into()))
}

/// `performance.now()`: the milliseconds since the runtime started.
fn now<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (_, _, started) = shared(call)?;
    Ok(Value::Number(started.elapsed().as_secs_f64() * 1000.0))
}

/// `queueMicrotask(callback)`: `callback` runs once the script's current job is done.
fn queue_microtask<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(callback)) = args.first() else {
        return Err(Error::Script("queueMicrotask: the callback is not a function".into()));
    };
    let (promise, _, _) = shared(call)?;
    let resolved = method(call, &promise, "resolve", &[])?;
    if let Value::Js(resolved) = resolved {
        method(call, &resolved, "then", &[Value::Js(callback.clone())])?;
    }
    Ok(Value::Undefined)
}

/// `new TextEncoder()`.
fn text_encoder<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (object, encoder) = with_web(call, |web| (web.object.clone(), web.encoder.clone()))?;
    method(call, &object, "create", &[Value::Js(encoder)])
}

/// `new TextDecoder([label])`: UTF-8 only, as `TextEncoder` writes.
fn text_decoder<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(label) = args.first().filter(|l| !matches!(l, Value::Undefined)) {
        let label = call.to_string(label)?.to_ascii_lowercase();
        if !matches!(label.trim(), "utf-8" | "utf8" | "unicode-1-1-utf-8") {
            return Err(Error::Script(format!("TextDecoder: only UTF-8 is supported, not {label}")));
        }
    }
    let (object, decoder) = with_web(call, |web| (web.object.clone(), web.decoder.clone()))?;
    method(call, &object, "create", &[Value::Js(decoder)])
}

/// `encoder.encode(text)`: `text` as UTF-8, a `Uint8Array`.
fn encode<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = match args.first() {
        Some(Value::Undefined) | None => String::new(),
        Some(text) => call.to_string(text)?,
    };
    Ok(Value::Js(call.new_uint8_array(text.as_bytes())?))
}

/// `decoder.decode(bytes)`: a `Uint8Array`, another typed array's or an `ArrayBuffer`'s bytes as
/// UTF-8 text, invalid sequences replaced, as browsers do.
fn decode<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(input)) = args.first() else {
        return Ok(Value::String(String::new()));
    };
    let bytes = call.bytes(input)?;
    Ok(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
}


/// `crypto.getRandomValues(array)`: fills the integer typed array with random values and returns
/// it. At most 65536 bytes, as in browsers.
fn get_random_values<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(array)) = args.first() else {
        return Err(Error::Script("getRandomValues: not a typed array".into()));
    };
    let (_, bytes_class, _) = shared(call)?;
    let byte_length = match call.get(array, "byteLength")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => return Err(Error::Script("getRandomValues: not a typed array".into())),
    };
    if byte_length > 65536 {
        return Err(Error::Script("getRandomValues: more than 65536 bytes".into()));
    }
    let mut random = vec![0; byte_length];
    getrandom::fill(&mut random).map_err(|e| Error::Script(format!("getRandomValues: {e}")))?;
    // Written byte by byte through a view of the array's memory.
    let buffer = call.get(array, "buffer")?;
    let offset = call.get(array, "byteOffset")?;
    let view = call.construct(&bytes_class, &[buffer, offset, Value::Number(byte_length as f64)])?;
    if let Value::Js(view) = view {
        for (i, byte) in random.into_iter().enumerate() {
            call.set(&view, &i.to_string(), &Value::Number(f64::from(byte)))?;
        }
    }
    Ok(Value::Js(array.clone()))
}

/// `crypto.randomUUID()`: a random version 4 UUID.
fn random_uuid<H>(_call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Script(format!("randomUUID: {e}")))?;
    // Version 4, variant 10xx.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(Value::String(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])))
}
