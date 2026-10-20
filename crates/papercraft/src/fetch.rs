//! The browser's `fetch()` for scripts outside a browser, with `Headers`, `Request`, `Response`,
//! `Blob`, `FormData`, `AbortController` and `AbortSignal`. A request runs on a thread of its own
//! (`ureq`, HTTPS through rustls); the script's next turn ([`poll`]) settles its Promise. Bodies
//! arrive whole: no streams. There is no CORS, cookie store or HTTP cache outside a browser.

use std::sync::mpsc::{Receiver, TryRecvError};

use crate::{Call, Engine, Error, HostFunction, Runtime, Value, method, object};

/// The largest response body read, in bytes.
const MOST: u64 = 512 * 1024 * 1024;

/// What the classes share: the realm's built-ins they use, and their prototypes.
pub(crate) struct Classes<H> {
    object: H,
    promise: H,
    json: H,
    function: H,
    headers: H,
    blob: H,
    form_data: H,
    request: H,
    response: H,
    abort_controller: H,
    abort_signal: H,
    abort: H,
}

/// A request on its thread, and what settles its Promise.
pub(crate) struct Pending<H> {
    answer: Receiver<Result<Answer, String>>,
    resolve: H,
    reject: H,
    signal: Option<H>,
}

/// A `FormData` entry: its name, its value (a string or a `Blob`), and a `Blob`'s file name.
type FormEntry<H> = (String, Value<H>, Option<String>);

/// A response as its thread received it.
struct Answer {
    status: u16,
    status_text: String,
    url: String,
    redirected: bool,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// The classes, as `(global name, constructor, methods)`.
type Class<H> = (&'static str, HostFunction<H>, Vec<(&'static str, HostFunction<H>)>);

fn classes<H: Clone>() -> Vec<Class<H>> {
    vec![
        (
            "Headers",
            |call, _, args| new_headers(call, args.first()),
            vec![
                ("get", |call, this, args| {
                    let name = name(call, args.first())?;
                    let values: Vec<String> = headers(call, this)?.into_iter().filter(|(n, _)| *n == name).map(|(_, v)| v).collect();
                    Ok(if values.is_empty() { Value::Null } else { Value::String(values.join(", ")) })
                }),
                ("has", |call, this, args| {
                    let name = name(call, args.first())?;
                    Ok(Value::Bool(headers(call, this)?.iter().any(|(n, _)| *n == name)))
                }),
                ("set", |call, this, args| {
                    let (name, value) = (name(call, args.first())?, text(call, args.get(1))?);
                    let mut list = headers(call, this)?;
                    let at = list.iter().position(|(n, _)| *n == name);
                    list.retain(|(n, _)| *n != name);
                    list.insert(at.unwrap_or(list.len()).min(list.len()), (name, value));
                    store_headers(call, this, &list)
                }),
                ("append", |call, this, args| {
                    let (name, value) = (name(call, args.first())?, text(call, args.get(1))?);
                    let mut list = headers(call, this)?;
                    list.push((name, value));
                    store_headers(call, this, &list)
                }),
                ("delete", |call, this, args| {
                    let name = name(call, args.first())?;
                    let mut list = headers(call, this)?;
                    list.retain(|(n, _)| *n != name);
                    store_headers(call, this, &list)
                }),
                ("forEach", |call, this, args| {
                    let Some(Value::Js(callback)) = args.first() else {
                        return Err(Error::Script("Headers.forEach: the callback is not a function".into()));
                    };
                    let receiver = args.get(1).cloned().unwrap_or(Value::Undefined);
                    for (name, value) in combined(headers(call, this)?) {
                        call.call(callback, &receiver, &[Value::String(value), Value::String(name), this.clone()])?;
                    }
                    Ok(Value::Undefined)
                }),
                ("entries", |call, this, _| {
                    let list = combined(headers(call, this)?);
                    let pairs = list
                        .into_iter()
                        .map(|(n, v)| call.new_array(&[Value::String(n), Value::String(v)]).map(Value::Js))
                        .collect::<Result<Vec<_>, _>>()?;
                    iterate(call, &pairs)
                }),
                ("keys", |call, this, _| {
                    let names: Vec<_> = combined(headers(call, this)?).into_iter().map(|(n, _)| Value::String(n)).collect();
                    iterate(call, &names)
                }),
                ("values", |call, this, _| {
                    let values: Vec<_> = combined(headers(call, this)?).into_iter().map(|(_, v)| Value::String(v)).collect();
                    iterate(call, &values)
                }),
            ],
        ),
        (
            "Blob",
            |call, _, args| {
                let bytes = parts(call, args.first())?;
                let kind = option_text(call, args.get(1), "type")?.unwrap_or_default().to_ascii_lowercase();
                new_blob(call, &bytes, &kind)
            },
            vec![
                ("text", |call, this, _| {
                    let bytes = blob_bytes(call, this)?;
                    resolved(call, Value::String(String::from_utf8_lossy(&bytes).into_owned()))
                }),
                ("arrayBuffer", |call, this, _| {
                    let bytes = blob_bytes(call, this)?;
                    let array = call.new_uint8_array(&bytes)?;
                    let buffer = call.get(&array, "buffer")?;
                    resolved(call, buffer)
                }),
                ("bytes", |call, this, _| {
                    let bytes = blob_bytes(call, this)?;
                    let array = call.new_uint8_array(&bytes)?;
                    resolved(call, Value::Js(array))
                }),
                ("slice", |call, this, args| {
                    let bytes = blob_bytes(call, this)?;
                    let at = |value: Option<&Value<H>>, default: usize| match value {
                        Some(Value::Number(n)) if n.is_finite() => {
                            let n = n.trunc();
                            if n < 0.0 { bytes.len().saturating_sub((-n) as usize) } else { (n as usize).min(bytes.len()) }
                        }
                        _ => default,
                    };
                    let (start, end) = (at(args.first(), 0), at(args.get(1), bytes.len()));
                    let kind = match args.get(2) {
                        Some(Value::String(kind)) => kind.to_ascii_lowercase(),
                        _ => String::new(),
                    };
                    new_blob(call, bytes.get(start..end.max(start)).unwrap_or_default(), &kind)
                }),
            ],
        ),
        (
            "FormData",
            |call, _, _| {
                let form = create(call, |c| &c.form_data)?;
                let entries = call.new_array(&[])?;
                call.set(&form, "_entries", &Value::Js(entries))?;
                Ok(Value::Js(form))
            },
            vec![
                ("append", |call, this, args| {
                    let entry = form_entry(call, args)?;
                    let mut list = form_entries(call, this)?;
                    list.push(entry);
                    store_form(call, this, list)
                }),
                ("set", |call, this, args| {
                    let entry = form_entry(call, args)?;
                    let mut list = form_entries(call, this)?;
                    let at = list.iter().position(|(n, ..)| *n == entry.0);
                    list.retain(|(n, ..)| *n != entry.0);
                    let at = at.unwrap_or(list.len()).min(list.len());
                    list.insert(at, entry);
                    store_form(call, this, list)
                }),
                ("get", |call, this, args| {
                    let name = text(call, args.first())?;
                    Ok(form_entries(call, this)?.into_iter().find(|(n, ..)| *n == name).map_or(Value::Null, |(_, v, _)| v))
                }),
                ("getAll", |call, this, args| {
                    let name = text(call, args.first())?;
                    let values: Vec<_> = form_entries(call, this)?.into_iter().filter(|(n, ..)| *n == name).map(|(_, v, _)| v).collect();
                    Ok(Value::Js(call.new_array(&values)?))
                }),
                ("has", |call, this, args| {
                    let name = text(call, args.first())?;
                    Ok(Value::Bool(form_entries(call, this)?.iter().any(|(n, ..)| *n == name)))
                }),
                ("delete", |call, this, args| {
                    let name = text(call, args.first())?;
                    let mut list = form_entries(call, this)?;
                    list.retain(|(n, ..)| *n != name);
                    store_form(call, this, list)
                }),
                ("forEach", |call, this, args| {
                    let Some(Value::Js(callback)) = args.first() else {
                        return Err(Error::Script("FormData.forEach: the callback is not a function".into()));
                    };
                    let receiver = args.get(1).cloned().unwrap_or(Value::Undefined);
                    for (name, value, _) in form_entries(call, this)? {
                        call.call(callback, &receiver, &[value, Value::String(name), this.clone()])?;
                    }
                    Ok(Value::Undefined)
                }),
                ("entries", |call, this, _| {
                    let list = form_entries(call, this)?;
                    let pairs =
                        list.into_iter().map(|(n, v, _)| call.new_array(&[Value::String(n), v]).map(Value::Js)).collect::<Result<Vec<_>, _>>()?;
                    iterate(call, &pairs)
                }),
                ("keys", |call, this, _| {
                    let names: Vec<_> = form_entries(call, this)?.into_iter().map(|(n, ..)| Value::String(n)).collect();
                    iterate(call, &names)
                }),
                ("values", |call, this, _| {
                    let values: Vec<_> = form_entries(call, this)?.into_iter().map(|(_, v, _)| v).collect();
                    iterate(call, &values)
                }),
            ],
        ),
        (
            "Request",
            |call, _, args| new_request(call, args.first(), args.get(1)),
            vec![("clone", |call, this, _| new_request(call, Some(this), None))],
        ),
        (
            "Response",
            |call, _, args| {
                let (bytes, kind) = match args.first() {
                    None | Some(Value::Undefined | Value::Null) => (Vec::new(), None),
                    Some(body) => body_bytes(call, body)?,
                };
                let init = args.get(1);
                let status = match init {
                    Some(Value::Js(init)) => match call.get(init, "status")? {
                        Value::Number(n) if (200.0..=599.0).contains(&n) => n as u16,
                        Value::Undefined => 200,
                        _ => return Err(Error::Script("Response: the status is not between 200 and 599".into())),
                    },
                    _ => 200,
                };
                let status_text = option_text(call, init, "statusText")?.unwrap_or_default();
                let headers = option_value(call, init, "headers")?;
                let headers = new_headers(call, headers.as_ref())?;
                if let (Some(kind), Value::Js(headers)) = (kind, &headers)
                    && !headers_has(call, headers, "content-type")?
                {
                    let mut list = headers_list(call, headers)?;
                    list.push(("content-type".into(), kind));
                    store_headers(call, &Value::Js(headers.clone()), &list)?;
                }
                new_response(call, status, &status_text, "", false, headers, &bytes)
            },
            vec![
                ("text", |call, this, _| {
                    let bytes = take_body(call, this)?;
                    resolved(call, Value::String(String::from_utf8_lossy(&bytes).into_owned()))
                }),
                ("json", |call, this, _| {
                    let bytes = take_body(call, this)?;
                    let json = with(call, |c| c.json.clone())?;
                    match method(call, &json, "parse", &[Value::String(String::from_utf8_lossy(&bytes).into_owned())]) {
                        Ok(value) => resolved(call, value),
                        Err(e) => rejected(call, e),
                    }
                }),
                ("arrayBuffer", |call, this, _| {
                    let bytes = take_body(call, this)?;
                    let array = call.new_uint8_array(&bytes)?;
                    let buffer = call.get(&array, "buffer")?;
                    resolved(call, buffer)
                }),
                ("bytes", |call, this, _| {
                    let bytes = take_body(call, this)?;
                    let array = call.new_uint8_array(&bytes)?;
                    resolved(call, Value::Js(array))
                }),
                ("blob", |call, this, _| {
                    let bytes = take_body(call, this)?;
                    let kind = match this {
                        Value::Js(response) => match call.get(response, "headers")? {
                            Value::Js(headers) => headers_list(call, &headers)?.into_iter().find(|(n, _)| n == "content-type").map(|(_, v)| v),
                            _ => None,
                        },
                        _ => None,
                    };
                    let blob = new_blob(call, &bytes, &kind.unwrap_or_default())?;
                    resolved(call, blob)
                }),
                ("clone", |call, this, _| {
                    let Value::Js(response) = this else {
                        return Err(Error::Script("not a Response".into()));
                    };
                    if matches!(call.get(response, "bodyUsed")?, Value::Bool(true)) {
                        return Err(Error::Script("Response.clone: the body was read".into()));
                    }
                    let copy = create(call, |c| &c.response)?;
                    for key in ["status", "statusText", "ok", "url", "redirected", "type", "_body"] {
                        let value = call.get(response, key)?;
                        call.set(&copy, key, &value)?;
                    }
                    let headers = call.get(response, "headers")?;
                    let headers = new_headers(call, Some(&headers))?;
                    call.set(&copy, "headers", &headers)?;
                    call.set(&copy, "bodyUsed", &Value::Bool(false))?;
                    Ok(Value::Js(copy))
                }),
            ],
        ),
        (
            "AbortController",
            |call, _, _| {
                let controller = create(call, |c| &c.abort_controller)?;
                let signal = new_signal(call)?;
                call.set(&controller, "signal", &Value::Js(signal))?;
                Ok(Value::Js(controller))
            },
            vec![("abort", |call, this, args| {
                let Value::Js(controller) = this else {
                    return Err(Error::Script("not an AbortController".into()));
                };
                let signal = call.get(controller, "signal")?;
                abort_signal(call, &signal, args.first().cloned())
            })],
        ),
        (
            "AbortSignal",
            |_, _, _| Err(Error::Script("AbortSignal: use an AbortController, AbortSignal.abort() or AbortSignal.timeout()".into())),
            // Its listeners are `EventTarget`'s, whose prototype its own extends.
            vec![("throwIfAborted", |call, this, _| {
                if let Value::Js(signal) = this
                    && matches!(call.get(signal, "aborted")?, Value::Bool(true))
                {
                    let reason = call.get(signal, "reason")?;
                    let reason = call.to_string(&reason)?;
                    return Err(Error::Dom("AbortError", reason));
                }
                Ok(Value::Undefined)
            })],
        ),
    ]
}

/// Sets `fetch` and its classes on `global`, unless it has them.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let fetch = runtime.engine.function("fetch", fetch)?;
    let executor = runtime.engine.function("executor", executor)?;
    let abort = runtime.engine.function("abort", |call, this, args| abort_signal(call, this, args.first().cloned()))?;
    let signal_abort = runtime.engine.function("abort", |call, _, args| {
        let signal = new_signal(call)?;
        abort_signal(call, &Value::Js(signal.clone()), args.first().cloned())?;
        Ok(Value::Js(signal))
    })?;
    let signal_timeout = runtime.engine.function("timeout", signal_timeout)?;
    let mut made = Vec::new();
    for (name, constructor, methods) in classes() {
        let constructor = runtime.engine.constructor(name, constructor)?;
        let methods = methods.into_iter().map(|(n, f)| Ok((n, runtime.engine.function(n, f)?))).collect::<Result<Vec<_>, Error>>()?;
        made.push((name, constructor, methods));
    }
    let classes = runtime.engine.with_call(|call| {
        if !matches!(call.get(global, "fetch")?, Value::Undefined) {
            return Ok(None);
        }
        let reflect = object(call, global, "Reflect")?;
        let symbol = object(call, global, "Symbol")?;
        let iterator = call.get(&symbol, "iterator")?;
        let mut prototypes = Vec::new();
        for (name, constructor, methods) in made {
            let prototype = call.new_object()?;
            for (key, function) in &methods {
                call.set(&prototype, key, &Value::Js(function.clone()))?;
            }
            // `for (const [name, value] of headers)`, as `entries()`.
            if let Some((_, entries)) = methods.iter().find(|(n, _)| *n == "entries") {
                method(call, &reflect, "set", &[Value::Js(prototype.clone()), iterator.clone(), Value::Js(entries.clone())])?;
            }
            call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
            if name == "AbortSignal" {
                call.set(&constructor, "abort", &Value::Js(signal_abort.clone()))?;
                call.set(&constructor, "timeout", &Value::Js(signal_timeout.clone()))?;
                let event_target = object(call, global, "EventTarget")?;
                let event_target = call.get(&event_target, "prototype")?;
                let object_class = object(call, global, "Object")?;
                method(call, &object_class, "setPrototypeOf", &[Value::Js(prototype.clone()), event_target])?;
            }
            call.set(global, name, &Value::Js(constructor))?;
            prototypes.push(prototype);
        }
        call.set(global, "fetch", &Value::Js(fetch))?;
        let [headers, blob, form_data, request, response, abort_controller, abort_signal]: [E::Handle; 7] =
            prototypes.try_into().map_err(|_| Error::Script("fetch: the classes didn't set up".into()))?;
        Ok(Some(Classes {
            object: object(call, global, "Object")?,
            promise: object(call, global, "Promise")?,
            json: object(call, global, "JSON")?,
            function: executor,
            headers,
            blob,
            form_data,
            request,
            response,
            abort_controller,
            abort_signal,
            abort,
        }))
    })?;
    runtime.host.borrow_mut().fetch = classes;
    Ok(())
}

/// `f` with the classes.
fn with<H: Clone, R>(call: &mut dyn Call<H>, f: impl FnOnce(&Classes<H>) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let host = host.borrow();
    host.fetch.as_ref().map(f).ok_or_else(|| Error::Script("fetch is not set up".into()))
}

/// A new object with the prototype `prototype` picks.
fn create<H: Clone>(call: &mut dyn Call<H>, prototype: impl FnOnce(&Classes<H>) -> &H) -> Result<H, Error> {
    let (object_class, prototype) = with(call, |c| (c.object.clone(), prototype(c).clone()))?;
    match method(call, &object_class, "create", &[Value::Js(prototype)])? {
        Value::Js(made) => Ok(made),
        _ => Err(Error::Script("Object.create made nothing".into())),
    }
}

/// `String(value)`, `""` for nothing.
fn text<H>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<String, Error> {
    match value {
        None | Some(Value::Undefined) => Ok(String::new()),
        Some(value) => call.to_string(value),
    }
}

/// A header name, as headers compare them: lower case.
fn name<H>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<String, Error> {
    Ok(text(call, value)?.trim().to_ascii_lowercase())
}

/// `options[key]`, when `options` is an object.
fn option_value<H>(call: &mut dyn Call<H>, options: Option<&Value<H>>, key: &str) -> Result<Option<Value<H>>, Error> {
    match options {
        Some(Value::Js(options)) => match call.get(options, key)? {
            Value::Undefined => Ok(None),
            value => Ok(Some(value)),
        },
        _ => Ok(None),
    }
}

/// `String(options[key])`, when it's there.
fn option_text<H>(call: &mut dyn Call<H>, options: Option<&Value<H>>, key: &str) -> Result<Option<String>, Error> {
    match option_value(call, options, key)? {
        Some(value) => Ok(Some(call.to_string(&value)?)),
        None => Ok(None),
    }
}

/// An iterator over `values`, as `entries()`, `keys()` and `values()` return.
fn iterate<H: Clone>(call: &mut dyn Call<H>, values: &[Value<H>]) -> Result<Value<H>, Error> {
    let array = call.new_array(values)?;
    method(call, &array, "values", &[])
}

/// `Promise.resolve(value)`.
fn resolved<H: Clone>(call: &mut dyn Call<H>, value: Value<H>) -> Result<Value<H>, Error> {
    let promise = with(call, |c| c.promise.clone())?;
    method(call, &promise, "resolve", &[value])
}

/// A Promise rejected for `error`: with a `TypeError` unless it has a type of its own, as `fetch()`
/// and the body readers reject.
fn rejected<H: Clone>(call: &mut dyn Call<H>, error: Error) -> Result<Value<H>, Error> {
    let error = match error {
        Error::Script(message) => Error::Type(message),
        typed => typed,
    };
    let reason = crate::exception::value(call, &error)?;
    let promise = with(call, |c| c.promise.clone())?;
    method(call, &promise, "reject", &[reason])
}

/// The header list of `headers`, a `Headers` object.
fn headers<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<(String, String)>, Error> {
    match this {
        Value::Js(headers) => headers_list(call, headers),
        _ => Err(Error::Script("not a Headers object".into())),
    }
}

fn headers_list<H: Clone>(call: &mut dyn Call<H>, headers: &H) -> Result<Vec<(String, String)>, Error> {
    match call.get(headers, "_list")? {
        Value::String(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
        _ => Ok(Vec::new()),
    }
}

fn headers_has<H: Clone>(call: &mut dyn Call<H>, headers: &H, name: &str) -> Result<bool, Error> {
    Ok(headers_list(call, headers)?.iter().any(|(n, _)| n == name))
}

fn store_headers<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, list: &[(String, String)]) -> Result<Value<H>, Error> {
    if let Value::Js(headers) = this {
        let json = serde_json::to_string(list).map_err(|e| Error::Script(e.to_string()))?;
        call.set(headers, "_list", &Value::String(json))?;
    }
    Ok(Value::Undefined)
}

/// The headers as scripts see them: sorted by name, the values of a name joined.
fn combined(list: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut names: Vec<String> = list.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let values: Vec<&str> = list.iter().filter(|(n, _)| *n == name).map(|(_, v)| v.as_str()).collect();
            let value = values.join(", ");
            (name, value)
        })
        .collect()
}

/// `new Headers(init)`: from another `Headers`, `[[name, value], …]` or `{ name: value }`.
fn new_headers<H: Clone>(call: &mut dyn Call<H>, init: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let made = create(call, |c| &c.headers)?;
    let mut list = Vec::new();
    if let Some(Value::Js(init)) = init {
        if matches!(call.get(init, "_list")?, Value::String(_)) {
            list = headers_list(call, init)?;
        } else {
            let globals = crate::dialog::globals(call)?;
            match crate::dialog::to_json(call, &globals, &Value::Js(init.clone()))? {
                serde_json::Value::Array(pairs) => {
                    for pair in pairs {
                        if let (Some(n), Some(v)) = (pair.get(0), pair.get(1)) {
                            list.push((json_text(n).trim().to_ascii_lowercase(), json_text(v)));
                        }
                    }
                }
                serde_json::Value::Object(map) => {
                    for (n, v) in map {
                        list.push((n.trim().to_ascii_lowercase(), json_text(&v)));
                    }
                }
                _ => {}
            }
        }
    }
    store_headers(call, &Value::Js(made.clone()), &list)?;
    Ok(Value::Js(made))
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A `Blob` of `bytes`, of MIME type `kind`.
pub(crate) fn new_blob<H: Clone>(call: &mut dyn Call<H>, bytes: &[u8], kind: &str) -> Result<Value<H>, Error> {
    let blob = create(call, |c| &c.blob)?;
    let array = call.new_uint8_array(bytes)?;
    call.set(&blob, "_bytes", &Value::Js(array))?;
    call.set(&blob, "size", &Value::Number(bytes.len() as f64))?;
    call.set(&blob, "type", &Value::String(kind.to_owned()))?;
    Ok(Value::Js(blob))
}

pub(crate) fn is_blob<H: Clone>(call: &mut dyn Call<H>, value: &H) -> Result<bool, Error> {
    Ok(matches!(call.get(value, "_bytes")?, Value::Js(_)))
}

pub(crate) fn blob_bytes<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<u8>, Error> {
    match this {
        Value::Js(blob) => match call.get(blob, "_bytes")? {
            Value::Js(bytes) => call.bytes(&bytes),
            _ => Err(Error::Script("not a Blob".into())),
        },
        _ => Err(Error::Script("not a Blob".into())),
    }
}

/// The bytes of a `Blob`'s parts: strings (UTF-8), typed arrays, `ArrayBuffer`s and `Blob`s.
fn parts<H: Clone>(call: &mut dyn Call<H>, parts: Option<&Value<H>>) -> Result<Vec<u8>, Error> {
    let Some(Value::Js(list)) = parts else {
        return Ok(Vec::new());
    };
    let count = match call.get(list, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    };
    let mut bytes = Vec::new();
    for i in 0..count {
        let part = call.get(list, &i.to_string())?;
        bytes.extend(part_bytes(call, &part)?);
    }
    Ok(bytes)
}

fn part_bytes<H: Clone>(call: &mut dyn Call<H>, part: &Value<H>) -> Result<Vec<u8>, Error> {
    match part {
        Value::Js(object) if is_blob(call, object)? => blob_bytes(call, part),
        Value::Js(object) => call.bytes(object).or_else(|_| Ok(call.to_string(part)?.into_bytes())),
        other => Ok(call.to_string(other)?.into_bytes()),
    }
}

/// A body's bytes and the content type it implies: a string, bytes, a `Blob` or `FormData`.
fn body_bytes<H: Clone>(call: &mut dyn Call<H>, body: &Value<H>) -> Result<(Vec<u8>, Option<String>), Error> {
    match body {
        Value::Js(object) if is_blob(call, object)? => {
            let kind = match call.get(object, "type")? {
                Value::String(kind) if !kind.is_empty() => Some(kind),
                _ => None,
            };
            Ok((blob_bytes(call, body)?, kind))
        }
        Value::Js(object) if matches!(call.get(object, "_entries")?, Value::Js(_)) => multipart(call, body),
        Value::Js(object) => match call.bytes(object) {
            Ok(bytes) => Ok((bytes, None)),
            Err(_) => Ok((call.to_string(body)?.into_bytes(), Some("text/plain;charset=UTF-8".into()))),
        },
        other => Ok((call.to_string(other)?.into_bytes(), Some("text/plain;charset=UTF-8".into()))),
    }
}

/// A `FormData` entry from `append(name, value[, filename])`: a `Blob` keeps its file name.
fn form_entry<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<FormEntry<H>, Error> {
    let name = text(call, args.first())?;
    match args.get(1) {
        Some(Value::Js(value)) if is_blob(call, value)? => {
            let filename = match args.get(2) {
                Some(Value::Undefined) | None => "blob".to_owned(),
                Some(filename) => call.to_string(filename)?,
            };
            Ok((name, Value::Js(value.clone()), Some(filename)))
        }
        value => Ok((name.clone(), Value::String(text(call, value)?), None)),
    }
}

fn form_entries<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<FormEntry<H>>, Error> {
    let Value::Js(form) = this else {
        return Err(Error::Script("not a FormData object".into()));
    };
    let Value::Js(list) = call.get(form, "_entries")? else {
        return Err(Error::Script("not a FormData object".into()));
    };
    let count = match call.get(&list, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    };
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        if let Value::Js(entry) = call.get(&list, &i.to_string())? {
            let name = call.get(&entry, "0")?;
            let name = call.to_string(&name)?;
            let value = call.get(&entry, "1")?;
            let filename = match call.get(&entry, "2")? {
                Value::String(filename) => Some(filename),
                _ => None,
            };
            entries.push((name, value, filename));
        }
    }
    Ok(entries)
}

fn store_form<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, list: Vec<FormEntry<H>>) -> Result<Value<H>, Error> {
    let Value::Js(form) = this else {
        return Err(Error::Script("not a FormData object".into()));
    };
    let mut entries = Vec::with_capacity(list.len());
    for (name, value, filename) in list {
        let filename = filename.map_or(Value::Undefined, Value::String);
        entries.push(Value::Js(call.new_array(&[Value::String(name), value, filename])?));
    }
    let entries = call.new_array(&entries)?;
    call.set(form, "_entries", &Value::Js(entries))?;
    Ok(Value::Undefined)
}

/// `FormData` as `multipart/form-data`, with a random boundary.
fn multipart<H: Clone>(call: &mut dyn Call<H>, form: &Value<H>) -> Result<(Vec<u8>, Option<String>), Error> {
    let mut random = [0u8; 12];
    getrandom::fill(&mut random).map_err(|e| Error::Script(format!("FormData: {e}")))?;
    let boundary = format!("----PaperCraft{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let quote = |s: &str| s.replace('\r', "%0D").replace('\n', "%0A").replace('"', "%22");
    let mut bytes = Vec::new();
    for (name, value, filename) in form_entries(call, form)? {
        bytes.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"", quote(&name)).into_bytes());
        match filename {
            Some(filename) => {
                let (content, kind) = body_bytes(call, &value)?;
                let kind = kind.unwrap_or_else(|| "application/octet-stream".into());
                bytes.extend(format!("; filename=\"{}\"\r\nContent-Type: {kind}\r\n\r\n", quote(&filename)).into_bytes());
                bytes.extend(content);
            }
            None => {
                bytes.extend(b"\r\n\r\n");
                bytes.extend(call.to_string(&value)?.into_bytes());
            }
        }
        bytes.extend(b"\r\n");
    }
    bytes.extend(format!("--{boundary}--\r\n").into_bytes());
    Ok((bytes, Some(format!("multipart/form-data; boundary={boundary}"))))
}

/// `new Request(input, init)`: `input` a URL or a `Request`, `init` with `method`, `headers`,
/// `body` and `signal`.
fn new_request<H: Clone>(call: &mut dyn Call<H>, input: Option<&Value<H>>, init: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let request = create(call, |c| &c.request)?;
    let from = match input {
        Some(Value::Js(from)) if matches!(call.get(from, "method")?, Value::String(_)) => Some(from.clone()),
        _ => None,
    };
    let field = |call: &mut dyn Call<H>, key: &str| -> Result<Option<Value<H>>, Error> {
        if let Some(value) = option_value(call, init, key)? {
            return Ok(Some(value));
        }
        match &from {
            Some(from) => match call.get(from, key)? {
                Value::Undefined | Value::Null => Ok(None),
                value => Ok(Some(value)),
            },
            None => Ok(None),
        }
    };
    let url = match &from {
        Some(from) => call.get(from, "url")?,
        None => Value::String(text(call, input)?),
    };
    let method = match field(call, "method")? {
        Some(method) => call.to_string(&method)?.to_ascii_uppercase(),
        None => "GET".to_owned(),
    };
    let headers = field(call, "headers")?;
    let headers = new_headers(call, headers.as_ref())?;
    let body = match option_value(call, init, "body")? {
        Some(Value::Null) | None => match &from {
            Some(from) => call.get(from, "_body")?,
            None => Value::Null,
        },
        Some(body) => {
            let (bytes, kind) = body_bytes(call, &body)?;
            if let (Some(kind), Value::Js(headers)) = (kind, &headers)
                && !headers_has(call, headers, "content-type")?
            {
                let mut list = headers_list(call, headers)?;
                list.push(("content-type".into(), kind));
                store_headers(call, &Value::Js(headers.clone()), &list)?;
            }
            Value::Js(call.new_uint8_array(&bytes)?)
        }
    };
    if !matches!(body, Value::Null) && matches!(method.as_str(), "GET" | "HEAD") {
        return Err(Error::Script(format!("Request: a {method} request can't have a body")));
    }
    let signal = field(call, "signal")?.unwrap_or(Value::Null);
    let redirect = field(call, "redirect")?;
    let redirect = match redirect {
        Some(redirect) => call.to_string(&redirect)?,
        None => "follow".to_owned(),
    };
    for (key, value) in [
        ("url", url),
        ("method", Value::String(method)),
        ("headers", headers),
        ("_body", body),
        ("signal", signal),
        ("redirect", Value::String(redirect)),
    ] {
        call.set(&request, key, &value)?;
    }
    Ok(Value::Js(request))
}

/// A `Response` as scripts read it.
fn new_response<H: Clone>(
    call: &mut dyn Call<H>,
    status: u16,
    status_text: &str,
    url: &str,
    redirected: bool,
    headers: Value<H>,
    body: &[u8],
) -> Result<Value<H>, Error> {
    let response = create(call, |c| &c.response)?;
    let body = call.new_uint8_array(body)?;
    let fields = [
        ("status", Value::Number(f64::from(status))),
        ("statusText", Value::String(status_text.to_owned())),
        ("ok", Value::Bool((200..300).contains(&status))),
        ("url", Value::String(url.to_owned())),
        ("redirected", Value::Bool(redirected)),
        ("type", Value::String("basic".into())),
        ("headers", headers),
        ("bodyUsed", Value::Bool(false)),
        ("_body", Value::Js(body)),
    ];
    for (key, value) in fields {
        call.set(&response, key, &value)?;
    }
    Ok(Value::Js(response))
}

/// A `Response`'s body, read once.
fn take_body<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Vec<u8>, Error> {
    let Value::Js(response) = this else {
        return Err(Error::Script("not a Response".into()));
    };
    if matches!(call.get(response, "bodyUsed")?, Value::Bool(true)) {
        return Err(Error::Type("the body was read already".into()));
    }
    call.set(response, "bodyUsed", &Value::Bool(true))?;
    match call.get(response, "_body")? {
        Value::Js(bytes) => call.bytes(&bytes),
        _ => Ok(Vec::new()),
    }
}

/// An `AbortSignal` that hasn't aborted.
fn new_signal<H: Clone>(call: &mut dyn Call<H>) -> Result<H, Error> {
    let signal = create(call, |c| &c.abort_signal)?;
    for (key, value) in [("aborted", Value::Bool(false)), ("reason", Value::Undefined), ("onabort", Value::Null)] {
        call.set(&signal, key, &value)?;
    }
    Ok(signal)
}

/// Aborts `signal` for `reason` (an `AbortError` when none): an `abort` event for `onabort` and its
/// listeners, once.
fn abort_signal<H: Clone>(call: &mut dyn Call<H>, signal: &Value<H>, reason: Option<Value<H>>) -> Result<Value<H>, Error> {
    let Value::Js(signal) = signal else {
        return Err(Error::Script("not an AbortSignal".into()));
    };
    if matches!(call.get(signal, "aborted")?, Value::Bool(true)) {
        return Ok(Value::Undefined);
    }
    let reason = match reason {
        Some(Value::Undefined) | None => crate::exception::value(call, &Error::Dom("AbortError", "signal is aborted without reason".into()))?,
        Some(reason) => reason,
    };
    call.set(signal, "aborted", &Value::Bool(true))?;
    call.set(signal, "reason", &reason)?;
    let event = crate::events::new_event(call, "Event", "abort", &[])?;
    crate::events::fire(call, &Value::Js(signal.clone()), &event)?;
    Ok(Value::Undefined)
}

/// `AbortSignal.timeout(milliseconds)`: a signal that aborts with a `TimeoutError` then.
fn signal_timeout<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let delay = match args.first() {
        Some(Value::Number(n)) if n.is_finite() && *n >= 0.0 => *n,
        _ => return Err(Error::Script("AbortSignal.timeout: not a number of milliseconds".into())),
    };
    let signal = new_signal(call)?;
    let abort = with(call, |c| c.abort.clone())?;
    let reason = crate::exception::value(call, &Error::Dom("TimeoutError", "signal timed out".into()))?;
    // `abort.bind(signal, reason)`, called when the time is up.
    let bound = method(call, &abort, "bind", &[Value::Js(signal.clone()), reason])?;
    crate::set_timer(call, &[bound, Value::Number(delay)], false)?;
    Ok(Value::Js(signal))
}

/// The executor of the Promise `fetch()` returns: keeps its `resolve` and `reject`.
fn executor<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let (Some(Value::Js(resolve)), Some(Value::Js(reject))) = (args.first(), args.get(1)) {
        call.host()?.borrow_mut().resolvers = Some((resolve.clone(), reject.clone()));
    }
    Ok(Value::Undefined)
}

/// `fetch(input, init)`: a Promise of the `Response`, rejected with a `TypeError` when the request
/// fails, or with the signal's reason when it aborts.
fn fetch<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let request = match new_request(call, args.first(), args.get(1)) {
        Ok(Value::Js(request)) => request,
        Ok(_) => return rejected(call, Error::Script("fetch: no request".into())),
        Err(e) => return rejected(call, e),
    };
    let (promise, executor) = with(call, |c| (c.promise.clone(), c.function.clone()))?;
    let made = call.construct(&promise, &[Value::Js(executor)])?;
    let (resolve, reject) = call.host()?.borrow_mut().resolvers.take().ok_or_else(|| Error::Script("fetch: no Promise".into()))?;
    let signal = match call.get(&request, "signal")? {
        Value::Js(signal) => Some(signal),
        _ => None,
    };
    if let Some(signal) = &signal
        && matches!(call.get(signal, "aborted")?, Value::Bool(true))
    {
        let reason = call.get(signal, "reason")?;
        call.call(&reject, &Value::Undefined, &[reason])?;
        return Ok(made);
    }
    let url = call.get(&request, "url")?;
    let url = call.to_string(&url)?;
    let method = call.get(&request, "method")?;
    let method = call.to_string(&method)?;
    let headers = match call.get(&request, "headers")? {
        Value::Js(headers) => headers_list(call, &headers)?,
        _ => Vec::new(),
    };
    let body = match call.get(&request, "_body")? {
        Value::Js(bytes) => Some(call.bytes(&bytes)?),
        _ => None,
    };
    let follow = !matches!(call.get(&request, "redirect")?, Value::String(r) if r != "follow");
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        fail(call, &reject, &format!("only http: and https: URLs, not '{url}'"))?;
        return Ok(made);
    }
    let (send, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Nobody to tell when the script is gone.
        let _ = send.send(request_on_thread(&method, &url, &headers, body, follow));
    });
    call.host()?.borrow_mut().fetches.push(Pending { answer, resolve, reject, signal });
    Ok(made)
}

/// `GET url` on this thread: its status and body.
pub(crate) fn get(url: &str) -> Result<(u16, Vec<u8>), String> {
    request_on_thread("GET", url, &[], None, true).map(|answer| (answer.status, answer.body))
}

/// The request, on its thread.
fn request_on_thread(method: &str, url: &str, headers: &[(String, String)], body: Option<Vec<u8>>, follow: bool) -> Result<Answer, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(if follow { 10 } else { 0 }).build().into();
    let mut builder = ureq::http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(body.unwrap_or_default()).map_err(|e| e.to_string())?;
    let mut response = agent.run(request).map_err(|e| e.to_string())?;
    let status = response.status();
    let final_url = ureq::ResponseExt::get_uri(&response).to_string();
    let headers = response.headers().iter().map(|(n, v)| (n.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let body = response.body_mut().with_config().limit(MOST).read_to_vec().map_err(|e| e.to_string())?;
    Ok(Answer {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or_default().to_owned(),
        redirected: final_url != url,
        url: final_url,
        headers,
        body,
    })
}

/// Settles the Promises of the requests that answered or were aborted, on the script's turn.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let pending = std::mem::take(&mut call.host()?.borrow_mut().fetches);
    let mut waiting = Vec::new();
    for fetch in pending {
        if let Some(signal) = &fetch.signal
            && matches!(call.get(signal, "aborted")?, Value::Bool(true))
        {
            let reason = call.get(signal, "reason")?;
            call.call(&fetch.reject, &Value::Undefined, &[reason])?;
            continue;
        }
        match fetch.answer.try_recv() {
            Ok(Ok(answer)) => {
                let headers = new_headers(call, None)?;
                let list: Vec<(String, String)> = answer.headers;
                store_headers(call, &headers, &list)?;
                let response = new_response(call, answer.status, &answer.status_text, &answer.url, answer.redirected, headers, &answer.body)?;
                call.call(&fetch.resolve, &Value::Undefined, &[response])?;
            }
            Ok(Err(message)) => fail(call, &fetch.reject, &message)?,
            Err(TryRecvError::Disconnected) => fail(call, &fetch.reject, "the request's thread ended")?,
            Err(TryRecvError::Empty) => waiting.push(fetch),
        }
    }
    call.host()?.borrow_mut().fetches.extend(waiting);
    Ok(())
}

/// Rejects a request's Promise with a `TypeError`, as browsers do when a request fails.
fn fail<H: Clone>(call: &mut dyn Call<H>, reject: &H, message: &str) -> Result<(), Error> {
    let reason = crate::exception::value(call, &Error::Type(format!("fetch failed: {message}")))?;
    call.call(reject, &Value::Undefined, &[reason])?;
    Ok(())
}
