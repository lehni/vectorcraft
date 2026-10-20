//! What the browser's everyday APIs outside a browser (`js/src/encoding.js`, `crypto.js` and
//! `global-scope.js`) need of the runtime: its clock, random bytes, and UTF-8.

use std::time::Instant;

use crate::{Call, Engine, Error, HostFunction, Value};

/// The natives: `now()`, the milliseconds since the runtime started, and `random(length)`, that
/// many random bytes.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let started = Instant::now();
    engine.with_call(|call| {
        call.host()?.borrow_mut().started = Some(started);
        Ok(())
    })?;
    let natives: [(&str, HostFunction<E::Handle>); 2] = [("now", now), ("random", random)];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

/// The natives `js/src/encoding.js` encodes and decodes with: `encode(text)`, its UTF-8 bytes, and
/// `decode(bytes, fatal)`, their text, invalid sequences replaced, or `null` when they're `fatal`.
pub(crate) fn encoding<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 2] = [("encode", encode), ("decode", decode)];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

fn now<H>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let started = call.host()?.borrow().started.unwrap_or_else(Instant::now);
    Ok(Value::Number(started.elapsed().as_secs_f64() * 1000.0))
}

fn random<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let length = call.to_number(args.first().unwrap_or(&Value::Undefined))?;
    // As much as `getRandomValues()` takes.
    let mut bytes = vec![0; if length.is_finite() { length.clamp(0.0, 65536.0) as usize } else { 0 }];
    getrandom::fill(&mut bytes).map_err(|e| Error::Script(format!("random: {e}")))?;
    Ok(Value::Js(call.new_uint8_array(&bytes)?))
}

fn encode<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    Ok(Value::Js(call.new_uint8_array(text.as_bytes())?))
}

fn decode<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(input)) = args.first() else {
        return Ok(Value::String(String::new()));
    };
    let bytes = call.bytes(input)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => Value::String(text),
        Err(_) if crate::truthy(args.get(1).unwrap_or(&Value::Undefined)) => Value::Null,
        Err(e) => Value::String(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    })
}
