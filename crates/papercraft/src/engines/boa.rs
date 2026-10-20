//! Boa, a JavaScript engine in pure Rust: slower than V8, but it builds everywhere.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::{JsArray, JsFloat64Array, JsUint8Array, JsUint16Array};
use boa_engine::{Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, Source};

use crate::{Call, Engine, Error, Host, HostFunction, Value};

type BoaHost = Rc<RefCell<Host<JsValue>>>;

pub struct Boa {
    context: Context,
}

impl Engine for Boa {
    type Handle = JsValue;

    fn new(host: BoaHost) -> Result<Self, Error> {
        let mut context = Context::default();
        context.insert_data(host);
        Ok(Self { context })
    }

    fn eval(&mut self, source: &str) -> Result<Value<JsValue>, Error> {
        let result = self.context.eval(Source::from_bytes(source)).map_err(script_error)?;
        self.run_jobs()?;
        Ok(from_js(result))
    }

    fn global(&mut self) -> Result<JsValue, Error> {
        Ok(self.context.global_object().into())
    }

    fn function(&mut self, name: &str, function: HostFunction<JsValue>) -> Result<JsValue, Error> {
        Ok(self.host_function(name, function, false))
    }

    fn constructor(&mut self, name: &str, function: HostFunction<JsValue>) -> Result<JsValue, Error> {
        // Boa calls native constructors with `this` undefined.
        Ok(self.host_function(name, function, true))
    }

    fn with_call<R>(&mut self, f: impl FnOnce(&mut dyn Call<JsValue>) -> Result<R, Error>) -> Result<R, Error> {
        let result = f(&mut BoaCall { context: &mut self.context });
        self.run_jobs()?;
        result
    }
}

impl Boa {
    /// Runs the Promise jobs the script queued, as V8 does by itself when the host's call returns:
    /// Boa leaves it to the host.
    fn run_jobs(&mut self) -> Result<(), Error> {
        self.context.run_jobs().map_err(script_error)
    }

    fn host_function(&mut self, name: &str, function: HostFunction<JsValue>, constructor: bool) -> JsValue {
        let native = NativeFunction::from_copy_closure(move |this, args, context| call_host(function, this, args, context));
        let function = FunctionObjectBuilder::new(self.context.realm(), native).name(JsString::from(name)).constructor(constructor).build();
        JsObject::from(function).into()
    }
}

/// The engine's values, inside a host function or through [`Engine::with_call`].
struct BoaCall<'a> {
    context: &'a mut Context,
}

impl Call<JsValue> for BoaCall<'_> {
    fn host(&self) -> Result<BoaHost, Error> {
        self.context.get_data::<BoaHost>().cloned().ok_or_else(|| Error::Script("no host".into()))
    }

    fn to_string(&mut self, value: &Value<JsValue>) -> Result<String, Error> {
        let string = to_js(value).to_string(self.context).map_err(script_error)?;
        Ok(string.to_std_string_escaped())
    }

    fn new_object(&mut self) -> Result<JsValue, Error> {
        Ok(JsObject::with_object_proto(self.context.intrinsics()).into())
    }

    fn new_array(&mut self, elements: &[Value<JsValue>]) -> Result<JsValue, Error> {
        Ok(JsArray::from_iter(elements.iter().map(to_js), self.context).into())
    }

    fn new_float64_array(&mut self, elements: &[f64]) -> Result<JsValue, Error> {
        Ok(JsFloat64Array::from_iter(elements.iter().copied(), self.context).map_err(script_error)?.into())
    }

    fn new_uint8_array(&mut self, elements: &[u8]) -> Result<JsValue, Error> {
        Ok(JsUint8Array::from_iter(elements.iter().copied(), self.context).map_err(script_error)?.into())
    }

    fn new_uint16_array(&mut self, elements: &[u16]) -> Result<JsValue, Error> {
        Ok(JsUint16Array::from_iter(elements.iter().copied(), self.context).map_err(script_error)?.into())
    }

    fn get(&mut self, object: &JsValue, key: &str) -> Result<Value<JsValue>, Error> {
        let object = object.to_object(self.context).map_err(script_error)?;
        Ok(from_js(object.get(JsString::from(key), self.context).map_err(script_error)?))
    }

    fn set(&mut self, object: &JsValue, key: &str, value: &Value<JsValue>) -> Result<(), Error> {
        let object = object.to_object(self.context).map_err(script_error)?;
        object.set(JsString::from(key), to_js(value), true, self.context).map_err(script_error)?;
        Ok(())
    }

    fn call(&mut self, function: &JsValue, this: &Value<JsValue>, args: &[Value<JsValue>]) -> Result<Value<JsValue>, Error> {
        let function = function.as_callable().ok_or_else(|| Error::Script("not a function".into()))?;
        let args: Vec<_> = args.iter().map(to_js).collect();
        Ok(from_js(function.call(&to_js(this), &args, self.context).map_err(script_error)?))
    }

    fn construct(&mut self, constructor: &JsValue, args: &[Value<JsValue>]) -> Result<Value<JsValue>, Error> {
        let constructor = constructor.as_constructor().ok_or_else(|| Error::Script("not a constructor".into()))?;
        let args: Vec<_> = args.iter().map(to_js).collect();
        let object = constructor.construct(&args, None, self.context).map_err(script_error)?;
        Ok(Value::Js(object.into()))
    }
}

fn call_host(function: HostFunction<JsValue>, this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let this = from_js(this.clone());
    let args: Vec<_> = args.iter().cloned().map(from_js).collect();
    match function(&mut BoaCall { context }, &this, &args) {
        Ok(value) => Ok(to_js(&value)),
        Err(error) => Err(JsNativeError::error().with_message(error.to_string()).into()),
    }
}

fn script_error(error: JsError) -> Error {
    Error::Script(error.to_string())
}

fn from_js(value: JsValue) -> Value<JsValue> {
    if value.is_undefined() {
        Value::Undefined
    } else if value.is_null() {
        Value::Null
    } else if let Some(bool) = value.as_boolean() {
        Value::Bool(bool)
    } else if let Some(number) = value.as_number() {
        Value::Number(number)
    } else if let Some(string) = value.as_string() {
        Value::String(string.to_std_string_escaped())
    } else {
        Value::Js(value)
    }
}

fn to_js(value: &Value<JsValue>) -> JsValue {
    match value {
        Value::Undefined => JsValue::undefined(),
        Value::Null => JsValue::null(),
        Value::Bool(bool) => JsValue::from(*bool),
        Value::Number(number) => JsValue::from(*number),
        Value::String(string) => JsValue::from(JsString::from(string.as_str())),
        Value::Js(value) => value.clone(),
    }
}
