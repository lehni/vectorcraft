//! What `URL` and `URLSearchParams` outside a browser (`js/src/url.js`) parse with: the `url`
//! crate, which follows the same WHATWG URL standard, its setters too (`url::quirks`).

use url::{Url, quirks};

use crate::{Call, Engine, Error, HostFunction, Value};

/// The natives `js/src/url.js` parses with: `parse(url, base)`, the URL's parts or `null`,
/// `set(href, part, value)`, the parts once `part` is set, `parseQuery(query)`, its
/// `[name, value]` pairs, and `serializeQuery(pairs)`.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 4] =
        [("parse", parse), ("set", set), ("parseQuery", parse_query), ("serializeQuery", serialize_query)];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

fn text<H>(call: &mut dyn Call<H>, args: &[Value<H>], n: usize) -> Result<String, Error> {
    call.to_string(args.get(n).unwrap_or(&Value::Undefined))
}

/// `url` against `base` when it's given, as `new URL(url, base)` parses it.
fn parse<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let url = text(call, args, 0)?;
    let parsed = match args.get(1) {
        None | Some(Value::Undefined) => Url::parse(&url),
        Some(_) => Url::parse(&text(call, args, 1)?).and_then(|base| base.join(&url)),
    };
    match parsed {
        Ok(parsed) => parts(call, &parsed),
        Err(_) => Ok(Value::Null),
    }
}

/// Sets the part `name` of `href` as its setter does: one it can't take changes nothing.
fn set<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let href = text(call, args, 0)?;
    let (name, value) = (text(call, args, 1)?, text(call, args, 2)?);
    let mut url = Url::parse(&href).map_err(|e| Error::Type(format!("Invalid URL: '{href}': {e}")))?;
    // A setter fails only where the standard says to change nothing, but `href`'s.
    match name.as_str() {
        "href" => quirks::set_href(&mut url, &value).map_err(|e| Error::Type(format!("Invalid URL: '{value}': {e}")))?,
        "protocol" => quirks::set_protocol(&mut url, &value).unwrap_or(()),
        "username" => quirks::set_username(&mut url, &value).unwrap_or(()),
        "password" => quirks::set_password(&mut url, &value).unwrap_or(()),
        "host" => quirks::set_host(&mut url, &value).unwrap_or(()),
        "hostname" => quirks::set_hostname(&mut url, &value).unwrap_or(()),
        "port" => quirks::set_port(&mut url, &value).unwrap_or(()),
        "pathname" => quirks::set_pathname(&mut url, &value),
        "search" => quirks::set_search(&mut url, &value),
        "hash" => quirks::set_hash(&mut url, &value),
        _ => return Err(Error::Type(format!("Unknown URL part: '{name}'."))),
    }
    parts(call, &url)
}

/// The parts of `url`, as its getters read them.
fn parts<H: Clone>(call: &mut dyn Call<H>, url: &Url) -> Result<Value<H>, Error> {
    let parts = call.new_object()?;
    for (name, value) in [
        ("href", quirks::href(url).to_owned()),
        ("origin", quirks::origin(url)),
        ("protocol", quirks::protocol(url).to_owned()),
        ("username", quirks::username(url).to_owned()),
        ("password", quirks::password(url).to_owned()),
        ("host", quirks::host(url).to_owned()),
        ("hostname", quirks::hostname(url).to_owned()),
        ("port", quirks::port(url).to_owned()),
        ("pathname", quirks::pathname(url).to_owned()),
        ("search", quirks::search(url).to_owned()),
        ("hash", quirks::hash(url).to_owned()),
    ] {
        call.set(&parts, name, &Value::String(value))?;
    }
    Ok(Value::Js(parts))
}

/// An `application/x-www-form-urlencoded` query's pairs.
fn parse_query<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let query = text(call, args, 0)?;
    let pairs = url::form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| call.new_array(&[Value::String(name.into_owned()), Value::String(value.into_owned())]).map(Value::Js))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Js(call.new_array(&pairs)?))
}

/// The `[name, value]` pairs as an `application/x-www-form-urlencoded` query.
fn serialize_query<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(Value::Js(pairs)) = args.first() {
        let length = call.get(pairs, "length")?;
        let length = call.to_number(&length)?;
        for i in 0..(if length.is_finite() { length.max(0.0) as usize } else { 0 }) {
            let pair = crate::handle(call.get(pairs, &i.to_string())?, "pair")?;
            let (name, value) = (call.get(&pair, "0")?, call.get(&pair, "1")?);
            serializer.append_pair(&call.to_string(&name)?, &call.to_string(&value)?);
        }
    }
    Ok(Value::String(serializer.finish()))
}
