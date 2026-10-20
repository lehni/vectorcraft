//! The requests of `fetch()` outside a browser (`js/src/fetch.js`), which the runtime sends: each on
//! a thread of its own (`ureq`, HTTPS through rustls); the script's next turn ([`poll`]) settles
//! its Promise.

use std::sync::mpsc::{Receiver, TryRecvError};

use crate::{Call, Error, Value, handle, method};

/// The largest response body read, in bytes.
const MOST: u64 = 512 * 1024 * 1024;

/// A request on its thread, and what settles its Promise.
pub(crate) struct Pending<H> {
    answer: Receiver<Result<Answer, String>>,
    resolve: H,
    reject: H,
}

/// A response as its thread received it.
pub(crate) struct Answer {
    pub status: u16,
    status_text: String,
    url: String,
    redirected: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// `request(method, url, headers, body, follow)`, which `fetch()` sends: a Promise of what the
/// server answers (`{ status, statusText, url, redirected, headers, body }`), rejected with a
/// `TypeError` when the request fails.
pub(crate) fn request<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = |call: &mut dyn Call<H>, index: usize| call.to_string(args.get(index).unwrap_or(&Value::Undefined));
    let (method_name, url) = (text(call, 0)?, text(call, 1)?);
    let headers = match args.get(2) {
        Some(Value::Js(list)) => pairs(call, list)?,
        _ => Vec::new(),
    };
    let body = match args.get(3) {
        Some(Value::Js(bytes)) => Some(call.bytes(bytes)?),
        _ => None,
    };
    let follow = matches!(args.get(4), Some(Value::Bool(true)));
    let promise = crate::global_class(call, "Promise")?;
    let resolvers = handle(method(call, &promise, "withResolvers", &[])?, "Promise.withResolvers()")?;
    let resolve = handle(call.get(&resolvers, "resolve")?, "resolve")?;
    let reject = handle(call.get(&resolvers, "reject")?, "reject")?;
    if url.starts_with("https://") || url.starts_with("http://") {
        let (send, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Nobody to tell when the script is gone.
            let _ = send.send(request_on_thread(&method_name, &url, &headers, body, follow));
        });
        call.host()?.borrow_mut().fetches.push(Pending { answer, resolve, reject });
    } else {
        fail(call, &reject, &format!("only http: and https: URLs, not '{url}'"))?;
    }
    call.get(&resolvers, "promise")
}

/// The `[name, value]` pairs of `list`.
fn pairs<H: Clone>(call: &mut dyn Call<H>, list: &H) -> Result<Vec<(String, String)>, Error> {
    let length = call.get(list, "length")?;
    let length = call.to_number(&length)?;
    let mut pairs = Vec::new();
    for i in 0..(if length.is_finite() { length.max(0.0) as usize } else { 0 }) {
        let pair = handle(call.get(list, &i.to_string())?, "header")?;
        let (name, value) = (call.get(&pair, "0")?, call.get(&pair, "1")?);
        pairs.push((call.to_string(&name)?, call.to_string(&value)?));
    }
    Ok(pairs)
}

/// `GET url` with `headers`, on this thread.
pub(crate) fn get(url: &str, headers: &[(String, String)]) -> Result<Answer, String> {
    request_on_thread("GET", url, headers, None, true)
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

/// Settles the Promises of the requests that answered, on the script's turn.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let pending = std::mem::take(&mut call.host()?.borrow_mut().fetches);
    let mut waiting = Vec::new();
    for fetch in pending {
        match fetch.answer.try_recv() {
            Ok(Ok(answer)) => {
                let response = call.new_object()?;
                let headers = answer
                    .headers
                    .into_iter()
                    .map(|(n, v)| call.new_array(&[Value::String(n), Value::String(v)]).map(Value::Js))
                    .collect::<Result<Vec<_>, _>>()?;
                let fields = [
                    ("status", Value::Number(f64::from(answer.status))),
                    ("statusText", Value::String(answer.status_text)),
                    ("url", Value::String(answer.url)),
                    ("redirected", Value::Bool(answer.redirected)),
                    ("headers", Value::Js(call.new_array(&headers)?)),
                    ("body", Value::Js(call.new_uint8_array(&answer.body)?)),
                ];
                for (key, value) in fields {
                    call.set(&response, key, &value)?;
                }
                call.call(&fetch.resolve, &Value::Undefined, &[Value::Js(response)])?;
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
