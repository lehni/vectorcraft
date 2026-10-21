//! The ports of `MessageChannel` outside a browser (`js/src/message-port.js`). A channel is two
//! ports, each a pair of channel ends crossed with the other's, so it works the same whether the
//! two are in one runtime or in two: a port sent in a transfer list moves its ends into the
//! [`Message`], and the runtime that receives it numbers it as a port of its own. Messages wait in
//! a port until its script starts it, and arrive on the runtime's turn ([`poll`]).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::clone::{Data, deserialize_with, serialize_with};
use crate::{Call, Engine, Error, HostFunction, Value};

/// A message between ports or workers: the value as structured serialization keeps it, and the
/// ends of the ports it transfers, which [`Data::Port`] numbers.
#[derive(Debug)]
pub(crate) struct Message {
    pub data: Data,
    pub ports: Vec<Ends>,
}

/// A port's ends of its channel: what it sends on, what it receives from, and a link its other
/// port shares, which shows whether that one is still there.
#[derive(Debug)]
pub(crate) struct Ends {
    to: Sender<Message>,
    from: Receiver<Message>,
    link: Arc<()>,
}

/// A port of the runtime: its number, its ends, and once its script started it, the function its
/// `MessagePort` receives with.
pub(crate) struct Port<H> {
    number: u32,
    ends: Ends,
    receive: Option<H>,
}

impl<H> Port<H> {
    /// Whether it keeps its script running: started, with its other port still there to send.
    pub(crate) fn is_open(&self) -> bool {
        self.receive.is_some() && Arc::strong_count(&self.ends.link) > 1
    }
}

/// The natives `js/src/message-port.js` talks through: `channel()`, which gives the numbers of
/// two new ports, `post(number, message, ports, numbers)`, `start(number, receive)` and
/// `close(number)`.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 4] = [("channel", new_channel), ("post", post), ("start", start), ("close", close)];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

/// `new MessageChannel()`: two ports, each sending to the other.
fn new_channel<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let ((to_first, from_first), (to_second, from_second)) = (channel(), channel());
    let link = Arc::new(());
    let first = add(call, Ends { to: to_second, from: from_first, link: link.clone() })?;
    let second = add(call, Ends { to: to_first, from: from_second, link })?;
    let numbers = call.new_float64_array(&[f64::from(first), f64::from(second)])?;
    Ok(Value::Js(numbers))
}

/// Numbers `ends` as a port of the runtime.
fn add<H>(call: &mut dyn Call<H>, ends: Ends) -> Result<u32, Error> {
    let host = call.host()?;
    let mut host = host.borrow_mut();
    host.next_port = host.next_port.wrapping_add(1);
    let number = host.next_port;
    host.ports.push(Port { number, ends, receive: None });
    Ok(number)
}

/// The port `args[0]` numbers.
fn number<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<u32, Error> {
    Ok(call.to_number(args.first().unwrap_or(&Value::Undefined))? as u32)
}

/// `port.postMessage(message, transfer)`: `args` are the port's number, the message, and the ports
/// it transfers, as objects and by number.
fn post<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, args)?;
    let message = message(call, args.get(1..).unwrap_or_default())?;
    if let Some(port) = call.host()?.borrow().ports.iter().find(|p| p.number == number) {
        // A port whose other port is gone takes nothing, as in browsers.
        let _ = port.ends.to.send(message);
    }
    Ok(Value::Undefined)
}

/// The [`Message`] of `args`: a value, then the ports it transfers as objects and by number, whose
/// ends it takes from the runtime.
pub(crate) fn message<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<Message, Error> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    let transferred = args.get(1).cloned().unwrap_or(Value::Undefined);
    let data = serialize_with(call, &value, &transferred)?;
    let mut numbers = Vec::new();
    if let Some(Value::Js(list)) = args.get(2) {
        let length = call.get(list, "length")?;
        for i in 0..call.to_number(&length)? as usize {
            let number = call.get(list, &i.to_string())?;
            numbers.push(call.to_number(&number)? as u32);
        }
    }
    let host = call.host()?;
    let mut host = host.borrow_mut();
    let mut ports = Vec::with_capacity(numbers.len());
    for number in numbers {
        let at = host.ports.iter().position(|p| p.number == number);
        let port = at.map(|at| host.ports.swap_remove(at));
        ports.push(port.ok_or_else(|| Error::Dom("DataCloneError", "a port that was closed or sent can't be transferred".into()))?.ends);
    }
    Ok(Message { data, ports })
}

/// `message`, made in the realm `call` is in, as `{ type, data, ports }` for a `MessageEvent`: its
/// ports numbered as the runtime's, `messageerror` without `data` when the value can't be made
/// here.
pub(crate) fn receive<H: Clone>(call: &mut dyn Call<H>, message: Message) -> Result<Value<H>, Error> {
    let mut ports = Vec::with_capacity(message.ports.len());
    for ends in message.ports {
        let number = add(call, ends)?;
        ports.push(crate::call_bundle(call, "makePort", &[Value::Number(f64::from(number))])?);
    }
    let object = call.new_object()?;
    match deserialize_with(call, &message.data, &ports) {
        Ok(value) => {
            call.set(&object, "type", &Value::String("message".into()))?;
            call.set(&object, "data", &value)?;
        }
        Err(_) => call.set(&object, "type", &Value::String("messageerror".into()))?,
    }
    let array = crate::global_class(call, "Array")?;
    let list = crate::method(call, &array, "of", &ports)?;
    call.set(&object, "ports", &list)?;
    Ok(Value::Js(object))
}

/// `port.start()`: what arrived and arrives goes to `receive`.
fn start<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, args)?;
    let Some(Value::Js(receive)) = args.get(1) else {
        return Err(Error::Type("MessagePort: nothing to receive with".into()));
    };
    if let Some(port) = call.host()?.borrow_mut().ports.iter_mut().find(|p| p.number == number) {
        port.receive = Some(receive.clone());
    }
    Ok(Value::Undefined)
}

/// `port.close()`: it sends and receives nothing from then on.
fn close<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let number = number(call, args)?;
    call.host()?.borrow_mut().ports.retain(|p| p.number != number);
    Ok(Value::Undefined)
}

/// Hands what arrived at the started ports to their `MessagePort`s, on the runtime's turn.
pub(crate) fn poll<H: Clone>(call: &mut dyn Call<H>) -> Result<(), Error> {
    let arrived: Vec<(H, Vec<Message>)> = call
        .host()?
        .borrow()
        .ports
        .iter()
        .filter_map(|port| Some((port.receive.clone()?, std::iter::from_fn(|| port.ends.from.try_recv().ok()).collect::<Vec<_>>())))
        .filter(|(_, messages)| !messages.is_empty())
        .collect();
    for (receive, messages) in arrived {
        for message in messages {
            let received = self::receive(call, message)?;
            call.call(&receive, &Value::Undefined, &[received])?;
        }
    }
    Ok(())
}
