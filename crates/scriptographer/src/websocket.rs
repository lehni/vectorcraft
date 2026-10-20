//! The sockets of `WebSocket` outside a browser (`js/src/websocket.js`), over tungstenite and
//! rustls: a socket's thread connects, then reads (a little at a time) and writes what the script
//! sends; what it reads goes back to the script on its turn ([`poll`], from `Runtime::run_timers`).
//! An open socket keeps the script running.

use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::{Call, Engine, Error, HostFunction, Value};

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

/// A script's socket: its number, the function it receives with, and its thread's channels.
pub(crate) struct Socket<H> {
    number: u32,
    receive: H,
    commands: Sender<Command>,
    incoming: Receiver<Incoming>,
}

/// The natives `js/src/websocket.js` talks through: `open(url, protocols, receive)`, which gives
/// the socket's number, `send(number, data)` (text, or bytes) and `close(number, code, reason)`.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 3] = [("open", open), ("send", send), ("close", close)];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

/// Connects to `url` on a thread.
fn open<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let url = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let mut protocols = Vec::new();
    if let Some(Value::Js(list)) = args.get(1) {
        let length = call.get(list, "length")?;
        let length = call.to_number(&length)?;
        for i in 0..(if length.is_finite() { length.clamp(0.0, 64.0) as usize } else { 0 }) {
            let protocol = call.get(list, &i.to_string())?;
            protocols.push(call.to_string(&protocol)?);
        }
    }
    let Some(Value::Js(receive)) = args.get(2) else {
        return Err(Error::Type("WebSocket: nothing to receive with".into()));
    };
    let (commands, received) = channel();
    let (sent, incoming) = channel();
    std::thread::Builder::new()
        .name("scriptographer websocket".into())
        .spawn(move || run(&url, &protocols, &received, &sent))
        .map_err(|e| Error::Script(format!("WebSocket: no thread for it: {e}")))?;
    let host = call.host()?;
    let mut host = host.borrow_mut();
    host.next_socket = host.next_socket.wrapping_add(1);
    let number = host.next_socket;
    host.sockets.push(Socket { number, receive: receive.clone(), commands, incoming });
    Ok(Value::Number(f64::from(number)))
}

/// Tells the socket `args[0]`'s thread `command`, while it runs.
fn command<H>(call: &mut dyn Call<H>, args: &[Value<H>], command: Command) -> Result<Value<H>, Error> {
    let number = call.to_number(args.first().unwrap_or(&Value::Undefined))?;
    if let Some(socket) = call.host()?.borrow().sockets.iter().find(|s| f64::from(s.number) == number) {
        // A thread that ended has told the script why, or will.
        let _ = socket.commands.send(command);
    }
    Ok(Value::Undefined)
}

fn send<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let data = match args.get(1) {
        Some(Value::String(text)) => Command::Text(text.clone()),
        Some(Value::Js(bytes)) => Command::Binary(call.bytes(bytes)?),
        _ => return Err(Error::Type("WebSocket: nothing to send".into())),
    };
    command(call, args, data)
}

fn close<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let code = call.to_number(args.get(1).unwrap_or(&Value::Undefined))?;
    let reason = call.to_string(args.get(2).unwrap_or(&Value::Undefined))?;
    command(call, args, Command::Close(if code.is_finite() { code as u16 } else { 1000 }, reason))
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

/// Hands what the sockets' threads read to their scripts' sockets (`{ type, … }`), on the script's
/// turn. The sockets stay where `send()` finds them while their events' listeners run.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let arrived: Vec<(u32, H, Vec<Incoming>)> = call
        .host()?
        .borrow()
        .sockets
        .iter()
        .map(|socket| (socket.number, socket.receive.clone(), std::iter::from_fn(|| socket.incoming.try_recv().ok()).collect()))
        .filter(|(_, _, incoming): &(u32, H, Vec<Incoming>)| !incoming.is_empty())
        .collect();
    for (number, receive, incoming) in arrived {
        for incoming in incoming {
            let closed = matches!(incoming, Incoming::Closed { .. });
            let fields = match incoming {
                Incoming::Open { protocol } => vec![("type", Value::String("open".into())), ("protocol", Value::String(protocol))],
                Incoming::Text(text) => vec![("type", Value::String("message".into())), ("data", Value::String(text))],
                Incoming::Binary(bytes) => vec![("type", Value::String("message".into())), ("data", Value::Js(call.new_uint8_array(&bytes)?))],
                Incoming::Error(message) => vec![("type", Value::String("error".into())), ("message", Value::String(message))],
                Incoming::Closed { code, reason, clean } => {
                    call.host()?.borrow_mut().sockets.retain(|s| s.number != number);
                    vec![
                        ("type", Value::String("close".into())),
                        ("code", Value::Number(f64::from(code))),
                        ("reason", Value::String(reason)),
                        ("wasClean", Value::Bool(clean)),
                    ]
                }
            };
            let object = call.new_object()?;
            for (key, value) in fields {
                call.set(&object, key, &value)?;
            }
            call.call(&receive, &Value::Undefined, &[Value::Js(object)])?;
            if closed {
                break;
            }
        }
    }
    Ok(())
}
