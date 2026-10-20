//! `WebSocket` for scripts outside a browser, over tungstenite and rustls: a socket's thread
//! connects, then reads (a little at a time) and writes what the script sends; what it reads comes
//! back on the script's turn ([`poll`], from `Runtime::run_timers`) as the events browsers fire:
//! `open`, `message` (text as a string, binary as a `Blob` or an `ArrayBuffer` as `binaryType`
//! says), `error` and `close`, each to its `on…` handler, then to the listeners. An open socket keeps
//! the script running. A browser's realm has its own.

use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::{Call, Engine, Error, HostFunction, Value, method, object};

/// The ready states, as `WebSocket.CONNECTING` … name them.
const CONNECTING: f64 = 0.0;
const OPEN: f64 = 1.0;
const CLOSING: f64 = 2.0;
const CLOSED: f64 = 3.0;
const STATES: [(&str, f64); 4] = [("CONNECTING", CONNECTING), ("OPEN", OPEN), ("CLOSING", CLOSING), ("CLOSED", CLOSED)];

/// How long a socket's thread waits for what the server sends before it looks at what to send.
const READ_WAIT: Duration = Duration::from_millis(20);

/// What the script asks a socket's thread to do.
enum Command {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

/// What a socket's thread tells the script.
enum Incoming {
    Open { protocol: String },
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Closed { code: u16, reason: String, clean: bool },
}

/// A script's socket: its number (its object's `_socket`), its object, and its thread's channels.
pub(crate) struct Socket<H> {
    number: u32,
    object: H,
    commands: Sender<Command>,
    incoming: Receiver<Incoming>,
}

/// Sets `WebSocket` on `global`, unless it has it.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    if engine.with_call(|call| Ok(!matches!(call.get(global, "WebSocket")?, Value::Undefined)))? {
        return Ok(());
    }
    let constructor = engine.constructor("WebSocket", construct)?;
    let methods: [(&str, HostFunction<E::Handle>); 2] = [("send", send), ("close", close)];
    let mut made = Vec::new();
    for (name, f) in methods {
        made.push((name, engine.function(name, f)?));
    }
    engine.with_call(|call| {
        let event_target = object(call, global, "EventTarget")?;
        let event_target = call.get(&event_target, "prototype")?;
        let object_class = object(call, global, "Object")?;
        let Value::Js(prototype) = method(call, &object_class, "create", &[event_target])? else {
            return Err(Error::Script("Object.create made nothing".into()));
        };
        for (name, function) in made {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        for (name, state) in STATES {
            call.set(&prototype, name, &Value::Number(state))?;
            call.set(&constructor, name, &Value::Number(state))?;
        }
        call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(global, "WebSocket", &Value::Js(constructor))?;
        crate::tag(call, global, &prototype, "WebSocket")
    })
}

/// `new WebSocket(url, protocols)`, on `this`, the object `new` made: it connects on a thread.
fn construct<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(socket) = this else {
        return Err(Error::Type("WebSocket needs 'new'".into()));
    };
    let given = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let mut url = url::Url::parse(&given).map_err(|e| Error::Dom("SyntaxError", format!("'{given}' isn't a URL: {e}")))?;
    // As browsers have it: http and https mean ws and wss, and a fragment isn't allowed.
    let scheme = match url.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => return Err(Error::Dom("SyntaxError", format!("a WebSocket's URL can't be {other}:"))),
    };
    if url.set_scheme(scheme).is_err() || url.fragment().is_some() {
        return Err(Error::Dom("SyntaxError", format!("'{given}' isn't a WebSocket URL")));
    }
    let protocols: Vec<String> = match args.get(1) {
        None | Some(Value::Undefined) => Vec::new(),
        Some(Value::Js(list)) => {
            let count = match call.get(list, "length")? {
                Value::Number(n) if n.is_finite() && n >= 0.0 => (n as usize).min(64),
                _ => 0,
            };
            let mut protocols = Vec::with_capacity(count);
            for i in 0..count {
                let protocol = call.get(list, &i.to_string())?;
                protocols.push(call.to_string(&protocol)?);
            }
            protocols
        }
        Some(protocol) => vec![call.to_string(protocol)?],
    };
    for (key, value) in [
        ("url", Value::String(url.to_string())),
        ("readyState", Value::Number(CONNECTING)),
        ("protocol", Value::String(String::new())),
        ("extensions", Value::String(String::new())),
        ("bufferedAmount", Value::Number(0.0)),
        ("binaryType", Value::String("blob".into())),
        ("onopen", Value::Null),
        ("onmessage", Value::Null),
        ("onerror", Value::Null),
        ("onclose", Value::Null),
    ] {
        call.set(socket, key, &value)?;
    }
    let (commands, received) = channel();
    let (sent, incoming) = channel();
    let address = url.to_string();
    std::thread::Builder::new()
        .name("scriptographer websocket".into())
        .spawn(move || run(&address, &protocols, &received, &sent))
        .map_err(|e| Error::Script(format!("WebSocket: no thread for it: {e}")))?;
    let number = {
        let host = call.host()?;
        let mut host = host.borrow_mut();
        host.next_socket = host.next_socket.wrapping_add(1);
        host.next_socket
    };
    call.set(socket, "_socket", &Value::Number(f64::from(number)))?;
    call.host()?.borrow_mut().sockets.push(Socket { number, object: socket.clone(), commands, incoming });
    Ok(Value::Undefined)
}

/// A socket's thread: connects to `url`, then passes on what's read and what's to send, until
/// either side closes. A failure is an error, then a close that wasn't clean.
fn run(url: &str, protocols: &[String], commands: &Receiver<Command>, incoming: &Sender<Incoming>) {
    let unclean = |incoming: &Sender<Incoming>, message: String| {
        // The script may have dropped the socket: then nobody listens.
        let _ = incoming.send(Incoming::Error(message));
        let _ = incoming.send(Incoming::Closed { code: 1006, reason: String::new(), clean: false });
    };
    let mut request = match url.into_client_request() {
        Ok(request) => request,
        Err(e) => return unclean(incoming, e.to_string()),
    };
    if !protocols.is_empty()
        && let Ok(value) = protocols.join(", ").parse()
    {
        request.headers_mut().insert("Sec-WebSocket-Protocol", value);
    }
    let (mut socket, response) = match tungstenite::connect(request) {
        Ok(connected) => connected,
        Err(e) => return unclean(incoming, e.to_string()),
    };
    let protocol = response.headers().get("Sec-WebSocket-Protocol").and_then(|p| p.to_str().ok()).unwrap_or_default().to_owned();
    if incoming.send(Incoming::Open { protocol }).is_err() {
        return;
    }
    if let Err(e) = wait_briefly(&mut socket) {
        return unclean(incoming, e);
    }
    let mut closing = None;
    loop {
        loop {
            let sent = match commands.try_recv() {
                Ok(Command::Text(text)) => socket.send(Message::text(text)),
                Ok(Command::Binary(bytes)) => socket.send(Message::binary(bytes)),
                Ok(Command::Close(code, reason)) => socket.close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() })),
                Err(TryRecvError::Empty) => break,
                // The script dropped the socket.
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            };
            if let Err(e) = sent {
                return unclean(incoming, e.to_string());
            }
        }
        let message = match socket.read() {
            Ok(Message::Text(text)) => Incoming::Text(text.to_string()),
            Ok(Message::Binary(bytes)) => Incoming::Binary(bytes.to_vec()),
            Ok(Message::Close(frame)) => {
                closing = Some(frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string())));
                continue;
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(tungstenite::Error::ConnectionClosed) => {
                let (code, reason) = closing.unwrap_or((1005, String::new()));
                let _ = incoming.send(Incoming::Closed { code, reason, clean: true });
                return;
            }
            Err(e) => return unclean(incoming, e.to_string()),
        };
        if incoming.send(message).is_err() {
            return;
        }
    }
}

/// Makes the socket's reads give up after a little while, so its thread can send meanwhile.
fn wait_briefly(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<(), String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => &mut stream.sock,
        _ => return Err("an unknown kind of connection".into()),
    };
    stream.set_read_timeout(Some(READ_WAIT)).map_err(|e| e.to_string())
}

fn ready_state<H: Clone>(call: &mut dyn Call<H>, socket: &H) -> Result<f64, Error> {
    Ok(match call.get(socket, "readyState")? {
        Value::Number(state) => state,
        _ => CLOSED,
    })
}

/// The channel to `socket`'s thread, while it runs.
fn commands<H: Clone>(call: &mut dyn Call<H>, socket: &H) -> Result<Option<Sender<Command>>, Error> {
    let Value::Number(number) = call.get(socket, "_socket")? else {
        return Ok(None);
    };
    Ok(call.host()?.borrow().sockets.iter().find(|s| f64::from(s.number) == number).map(|s| s.commands.clone()))
}

/// `socket.send(data)`: text, or the bytes of an `ArrayBuffer`, a view or a `Blob`.
fn send<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(socket) = this else {
        return Err(Error::Type("not a WebSocket".into()));
    };
    let state = ready_state(call, socket)?;
    if state == CONNECTING {
        return Err(Error::Dom("InvalidStateError", "the WebSocket isn't open yet".into()));
    }
    let command = match args.first() {
        Some(Value::Js(data)) if crate::fetch::is_blob(call, data)? => Command::Binary(crate::fetch::blob_bytes(call, &Value::Js(data.clone()))?),
        Some(Value::Js(data)) => match call.bytes(data) {
            Ok(bytes) => Command::Binary(bytes),
            Err(_) => Command::Text(call.to_string(&Value::Js(data.clone()))?),
        },
        Some(data) => Command::Text(call.to_string(data)?),
        None => return Err(Error::Type("send: the data is missing".into())),
    };
    // A closing or closed socket sends nothing, as in browsers.
    if state == OPEN
        && let Some(commands) = commands(call, socket)?
    {
        let _ = commands.send(command);
    }
    Ok(Value::Undefined)
}

/// `socket.close(code, reason)`: a code of 1000 or 3000 to 4999, a reason of 123 bytes at most.
fn close<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(socket) = this else {
        return Err(Error::Type("not a WebSocket".into()));
    };
    let code = match args.first() {
        None | Some(Value::Undefined) => 1000,
        Some(code) => {
            let code = call.to_number(code)?;
            if code != 1000.0 && !(3000.0..=4999.0).contains(&code) {
                return Err(Error::Dom("InvalidAccessError", format!("a WebSocket can't close with code {code}")));
            }
            code as u16
        }
    };
    let reason = match args.get(1) {
        None | Some(Value::Undefined) => String::new(),
        Some(reason) => call.to_string(reason)?,
    };
    if reason.len() > 123 {
        return Err(Error::Dom("SyntaxError", "a WebSocket's close reason is 123 bytes at most".into()));
    }
    if ready_state(call, socket)? >= CLOSING {
        return Ok(Value::Undefined);
    }
    call.set(socket, "readyState", &Value::Number(CLOSING))?;
    if let Some(commands) = commands(call, socket)? {
        let _ = commands.send(Command::Close(code, reason));
    }
    Ok(Value::Undefined)
}

/// Fires the events of what the sockets' threads read, on the script's turn. The sockets stay where
/// `send()` finds them while their events' listeners run.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let arrived: Vec<(u32, H, Vec<Incoming>)> = call
        .host()?
        .borrow()
        .sockets
        .iter()
        .map(|socket| (socket.number, socket.object.clone(), std::iter::from_fn(|| socket.incoming.try_recv().ok()).collect()))
        .filter(|(_, _, incoming): &(u32, H, Vec<Incoming>)| !incoming.is_empty())
        .collect();
    for (number, socket, incoming) in arrived {
        let target = Value::Js(socket.clone());
        for incoming in incoming {
            let closed = matches!(incoming, Incoming::Closed { .. });
            let event = match incoming {
                Incoming::Open { protocol } => {
                    call.set(&socket, "readyState", &Value::Number(OPEN))?;
                    call.set(&socket, "protocol", &Value::String(protocol))?;
                    crate::events::new_event(call, "Event", "open", &[])?
                }
                Incoming::Text(text) => message(call, &socket, Value::String(text))?,
                Incoming::Binary(bytes) => {
                    let data = if matches!(call.get(&socket, "binaryType")?, Value::String(t) if t == "arraybuffer") {
                        let array = call.new_uint8_array(&bytes)?;
                        call.get(&array, "buffer")?
                    } else {
                        crate::fetch::new_blob(call, &bytes, "")?
                    };
                    message(call, &socket, data)?
                }
                // As in browsers, the event says nothing of why; the console does.
                Incoming::Error(message) => {
                    let url = call.get(&socket, "url")?;
                    let url = call.to_string(&url)?;
                    call.host()?.borrow_mut().output.push(format!("WebSocket {url}: {message}"));
                    crate::events::new_event(call, "Event", "error", &[])?
                }
                Incoming::Closed { code, reason, clean } => {
                    call.set(&socket, "readyState", &Value::Number(CLOSED))?;
                    call.host()?.borrow_mut().sockets.retain(|s| s.number != number);
                    let members = [("code", Value::Number(f64::from(code))), ("reason", Value::String(reason)), ("wasClean", Value::Bool(clean))];
                    crate::events::new_event(call, "CloseEvent", "close", &members)?
                }
            };
            crate::events::fire(call, &target, &event)?;
            if closed {
                break;
            }
        }
    }
    Ok(())
}

/// A `message` event of `data`, from the socket's server.
fn message<H: Clone>(call: &mut dyn Call<H>, socket: &H, data: Value<H>) -> Result<Value<H>, Error> {
    let url = call.get(socket, "url")?;
    let url = call.to_string(&url)?;
    let origin = url::Url::parse(&url).map(|u| u.origin().ascii_serialization()).unwrap_or_default();
    crate::events::new_event(call, "MessageEvent", "message", &[("data", data), ("origin", Value::String(origin))])
}
