//! The browser's own engine, through `js_sys`, in the web build.
//!
//! Scripts run in a realm of their own: the window of a hidden iframe, so their globals (`self`,
//! `console`, `setInterval`…) never touch the app's page. There, Paper.js has a DOM and runs as in
//! any browser. Host functions are `Proxy`s whose traps are Rust closures, which receive the
//! arguments as one array.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Proxy, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::{Call, Engine, Error, Host, HostFunction, Prelude, Value};

type BrowserHost = Rc<RefCell<Host<JsValue>>>;

pub struct Browser {
    host: BrowserHost,
    /// The realm's global object: the iframe's window.
    global: Object,
    /// Kept in the page for as long as the realm is used.
    _frame: web_sys::HtmlIFrameElement,
}

impl Engine for Browser {
    type Handle = JsValue;

    fn new(host: BrowserHost, prelude: Prelude<Self>) -> Result<Self, Error> {
        let no_dom = || Error::Script("scripts need a browser window".into());
        let document = web_sys::window().and_then(|window| window.document()).ok_or_else(no_dom)?;
        let frame: web_sys::HtmlIFrameElement = document.create_element("iframe").map_err(js_error)?.dyn_into().map_err(|_| no_dom())?;
        frame.set_hidden(true);
        document.body().ok_or_else(no_dom)?.append_child(&frame).map_err(js_error)?;
        let global: Object = frame.content_window().ok_or_else(no_dom)?.into();
        let mut engine = Self { host, global, _frame: frame };
        prelude(&mut engine)?;
        Ok(engine)
    }

    fn eval(&mut self, source: &str) -> Result<Value<JsValue>, Error> {
        // The realm's own `eval`, called indirectly, runs in its global scope.
        let eval: Function = Reflect::get(&self.global, &"eval".into()).map_err(js_error)?.dyn_into().map_err(js_error)?;
        Ok(from_js(eval.call1(&self.global, &source.into()).map_err(js_error)?))
    }

    fn global(&mut self) -> Result<JsValue, Error> {
        Ok(self.global.clone().into())
    }

    fn function(&mut self, _name: &str, function: HostFunction<JsValue>) -> Result<JsValue, Error> {
        let host = self.host.clone();
        let apply =
            Closure::<dyn FnMut(JsValue, JsValue, Array) -> Result<JsValue, JsValue>>::new(move |_target: JsValue, this: JsValue, args: Array| {
                call_host(function, &host, from_js(this), &args)
            })
            .into_js_value();
        // The closure is an arrow function, which `new` can't call, and is also the proxy's target.
        proxy(&apply, "apply", &apply)
    }

    fn constructor(&mut self, _name: &str, function: HostFunction<JsValue>) -> Result<JsValue, Error> {
        let host = self.host.clone();
        let construct = Closure::<dyn FnMut(JsValue, Array) -> Result<JsValue, JsValue>>::new(move |_target: JsValue, args: Array| {
            call_host(function, &host, Value::Undefined, &args)
        })
        .into_js_value();
        // A proxy can only be called with `new` if its target can: `Object` is the plainest one.
        proxy(&Object::new().constructor().into(), "construct", &construct)
    }

    fn with_call<R>(&mut self, f: impl FnOnce(&mut dyn Call<JsValue>) -> Result<R, Error>) -> Result<R, Error> {
        f(&mut BrowserCall { host: self.host.clone() })
    }
}

/// The engine's values, inside a host function or through [`Engine::with_call`].
struct BrowserCall {
    host: BrowserHost,
}

impl Call<JsValue> for BrowserCall {
    fn host(&self) -> Result<BrowserHost, Error> {
        Ok(self.host.clone())
    }

    fn to_string(&mut self, value: &Value<JsValue>) -> Result<String, Error> {
        Ok(match value {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(bool) => bool.to_string(),
            Value::String(string) => string.clone(),
            // `join()` converts its elements with `String()`, numbers included.
            Value::Number(_) | Value::Js(_) => Array::of1(&to_js(value)).join("").into(),
        })
    }

    fn new_object(&mut self) -> Result<JsValue, Error> {
        Ok(Object::new().into())
    }

    fn new_array(&mut self, elements: &[Value<JsValue>]) -> Result<JsValue, Error> {
        Ok(elements.iter().map(to_js).collect::<Array>().into())
    }

    fn new_float64_array(&mut self, elements: &[f64]) -> Result<JsValue, Error> {
        Ok(js_sys::Float64Array::from(elements).into())
    }

    fn new_uint8_array(&mut self, elements: &[u8]) -> Result<JsValue, Error> {
        Ok(js_sys::Uint8Array::from(elements).into())
    }

    fn bytes(&mut self, object: &JsValue) -> Result<Vec<u8>, Error> {
        // An `ArrayBuffer` whole, a view the part of its buffer it shows.
        let bytes = if let Some(buffer) = object.dyn_ref::<js_sys::ArrayBuffer>() {
            js_sys::Uint8Array::new(buffer)
        } else if js_sys::ArrayBuffer::is_view(object) {
            let buffer = Reflect::get(object, &"buffer".into()).map_err(js_error)?;
            let number = |key: &str| Reflect::get(object, &key.into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
            js_sys::Uint8Array::new_with_byte_offset_and_length(&buffer, number("byteOffset"), number("byteLength"))
        } else {
            return Err(Error::Script("not a typed array, DataView or ArrayBuffer".into()));
        };
        Ok(bytes.to_vec())
    }

    fn new_uint16_array(&mut self, elements: &[u16]) -> Result<JsValue, Error> {
        Ok(js_sys::Uint16Array::from(elements).into())
    }

    fn get(&mut self, object: &JsValue, key: &str) -> Result<Value<JsValue>, Error> {
        Ok(from_js(Reflect::get(object, &key.into()).map_err(js_error)?))
    }

    fn set(&mut self, object: &JsValue, key: &str, value: &Value<JsValue>) -> Result<(), Error> {
        // As an assignment outside strict mode: read-only properties stay as they are.
        Reflect::set(object, &key.into(), &to_js(value)).map_err(js_error)?;
        Ok(())
    }

    fn call(&mut self, function: &JsValue, this: &Value<JsValue>, args: &[Value<JsValue>]) -> Result<Value<JsValue>, Error> {
        let function: &Function = function.dyn_ref().ok_or_else(|| Error::Script("not a function".into()))?;
        let args: Array = args.iter().map(to_js).collect();
        Ok(from_js(Reflect::apply(function, &to_js(this), &args).map_err(js_error)?))
    }

    fn construct(&mut self, constructor: &JsValue, args: &[Value<JsValue>]) -> Result<Value<JsValue>, Error> {
        let constructor: &Function = constructor.dyn_ref().ok_or_else(|| Error::Script("not a constructor".into()))?;
        let args: Array = args.iter().map(to_js).collect();
        Ok(from_js(Reflect::construct(constructor, &args).map_err(js_error)?))
    }
}

/// A proxy of `target` whose `trap` is `handler`. It has its target's name: proxies can't be named.
fn proxy(target: &JsValue, trap: &str, handler: &JsValue) -> Result<JsValue, Error> {
    let traps = Object::new();
    Reflect::set(&traps, &trap.into(), handler).map_err(js_error)?;
    let target: &Object = target.dyn_ref().ok_or_else(|| Error::Script("not an object".into()))?;
    Ok(Proxy::new(target, &traps).into())
}

fn call_host(function: HostFunction<JsValue>, host: &BrowserHost, this: Value<JsValue>, args: &Array) -> Result<JsValue, JsValue> {
    let args: Vec<_> = args.iter().map(from_js).collect();
    match function(&mut BrowserCall { host: host.clone() }, &this, &args) {
        Ok(value) => Ok(to_js(&value)),
        Err(error) => Err(js_sys::Error::new(&error.to_string()).into()),
    }
}

/// An error from a thrown value: its stack if it has one, else `String(value)`.
fn js_error(value: JsValue) -> Error {
    let stack = Reflect::get(&value, &"stack".into()).ok().and_then(|stack| stack.as_string());
    Error::Script(stack.unwrap_or_else(|| Array::of1(&value).join("").into()))
}

fn from_js(value: JsValue) -> Value<JsValue> {
    if value.is_undefined() {
        Value::Undefined
    } else if value.is_null() {
        Value::Null
    } else if let Some(bool) = value.as_bool() {
        Value::Bool(bool)
    } else if let Some(number) = value.as_f64() {
        Value::Number(number)
    } else if let Some(string) = value.as_string() {
        Value::String(string)
    } else {
        Value::Js(value)
    }
}

fn to_js(value: &Value<JsValue>) -> JsValue {
    match value {
        Value::Undefined => JsValue::UNDEFINED,
        Value::Null => JsValue::NULL,
        Value::Bool(bool) => JsValue::from_bool(*bool),
        Value::Number(number) => JsValue::from_f64(*number),
        Value::String(string) => JsValue::from_str(string),
        Value::Js(value) => value.clone(),
    }
}
