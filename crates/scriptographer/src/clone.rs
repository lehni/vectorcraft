//! Structured serialization, as browsers copy values between windows and workers: [`serialize`]
//! keeps a value as [`Data`], which [`deserialize`] makes again in any realm, on any thread.
//! Primitives are kept as they are; arrays and objects deeply, as plain ones (their own enumerable
//! properties), each object once, so shared and cyclic ones stay so; `Date`, `RegExp`, `Map`,
//! `Set`, `ArrayBuffer`, typed arrays and `DataView` (sharing the copies of their buffers as they
//! shared them), the `Error` classes, boxed primitives, `Blob`, `ImageData` and the geometry classes
//! as themselves. Functions, symbols and other objects throw a `DataCloneError`.
//!
//! `structuredClone(value)` is both in one realm. Transferring isn't supported.

use crate::{Call, Engine, Error, Value, method, object};

/// How deep objects may nest, before serializing would run out of stack.
const DEPTH: usize = 1024;

/// The typed array classes, as `Object.prototype.toString()` names them.
const TYPED_ARRAYS: [&str; 12] = [
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float16Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
];

/// The `Error` classes kept as themselves.
const ERRORS: [&str; 7] = ["Error", "EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError"];

/// A serialized value. Objects are numbered in the order they're first met; one met again is
/// [`Data::Seen`].
#[derive(Clone, Debug)]
pub(crate) enum Data {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    /// A BigInt, as its decimal digits.
    BigInt(String),
    /// The object of this number, met before.
    Seen(usize),
    /// An object's or array's own enumerable properties (an array's `length` too).
    Object(Vec<(String, Data)>),
    Array(f64, Vec<(String, Data)>),
    Date(f64),
    RegExp(String, String),
    Map(Vec<(Data, Data)>),
    Set(Vec<Data>),
    Buffer(Vec<u8>),
    /// A typed array or `DataView` (its class), of a buffer, at an offset, of a length.
    View(String, Box<Data>, f64, f64),
    /// An error of a class of [`ERRORS`], with its message and stack.
    Error(String, String, Option<String>),
    /// A boxed `Boolean`, `Number` or `String`.
    Boxed(String, Box<Data>),
    /// A `Blob`: its bytes and type. (A browser's realm clones its own.)
    #[cfg(not(target_arch = "wasm32"))]
    Blob(Vec<u8>, String),
    /// An `ImageData`: its pixels, width and height.
    Pixels(Box<Data>, f64, f64),
    /// A geometry object: its class's name, and its numbers ([`geometry_values`]).
    Geometry(String, Vec<f64>),
}

/// The realm's global object and `Object.prototype.toString`, which names an object's class.
pub(crate) struct Classes<H> {
    global: H,
    to_string: H,
}

/// Sets `structuredClone` on `global`, unless it has it, and keeps what serializing uses.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let function = engine.function("structuredClone", structured_clone)?;
    engine.with_call(|call| {
        let object_class = object(call, global, "Object")?;
        let prototype = object(call, &object_class, "prototype")?;
        let to_string = object(call, &prototype, "toString")?;
        call.host()?.borrow_mut().clone_classes = Some(Classes { global: global.clone(), to_string });
        if matches!(call.get(global, "structuredClone")?, Value::Undefined) {
            call.set(global, "structuredClone", &Value::Js(function))?;
        }
        Ok(())
    })
}

fn data_clone_error(what: &str) -> Error {
    Error::Dom("DataCloneError", format!("{what} can't be cloned"))
}

fn structured_clone<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let Some(Value::Js(options)) = args.get(1)
        && let Value::Js(transfer) = call.get(options, "transfer")?
        && !matches!(call.get(&transfer, "length")?, Value::Number(n) if n == 0.0)
    {
        return Err(data_clone_error("a transfer list"));
    }
    let data = serialize(call, args.first().unwrap_or(&Value::Undefined))?;
    deserialize(call, &data)
}

fn classes<H: Clone>(call: &mut dyn Call<H>) -> Result<(H, H), Error> {
    let host = call.host()?;
    let host = host.borrow();
    let classes = host.clone_classes.as_ref().ok_or_else(|| Error::Script("structured cloning is not set up".into()))?;
    Ok((classes.global.clone(), classes.to_string.clone()))
}

fn new_map<H: Clone>(call: &mut dyn Call<H>, global: &H) -> Result<H, Error> {
    let map = object(call, global, "Map")?;
    match call.construct(&map, &[])? {
        Value::Js(map) => Ok(map),
        _ => Err(Error::Script("new Map() made nothing".into())),
    }
}

fn length<H: Clone>(call: &mut dyn Call<H>, object: &H) -> Result<usize, Error> {
    Ok(match call.get(object, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    })
}

/// `value`, serialized.
pub(crate) fn serialize<H: Clone>(call: &mut dyn Call<H>, value: &Value<H>) -> Result<Data, Error> {
    let (global, to_string) = classes(call)?;
    let seen = new_map(call, &global)?;
    let mut serializer = Serializer { global, to_string, seen, count: 0 };
    serializer.serialize(call, value, 0)
}

/// A serialization in progress: what it uses of the realm, and the objects met so far, by number.
struct Serializer<H> {
    global: H,
    to_string: H,
    seen: H,
    count: usize,
}

impl<H: Clone> Serializer<H> {
    fn serialize(&mut self, call: &mut dyn Call<H>, value: &Value<H>, depth: usize) -> Result<Data, Error> {
        let original = match value {
            Value::Undefined => return Ok(Data::Undefined),
            Value::Null => return Ok(Data::Null),
            Value::Bool(b) => return Ok(Data::Bool(*b)),
            Value::Number(n) => return Ok(Data::Number(*n)),
            Value::String(s) => return Ok(Data::String(s.clone())),
            Value::Js(original) => original,
        };
        if depth > DEPTH {
            return Err(Error::Range("structuredClone: the value nests too deeply".into()));
        }
        if let Value::Number(number) = method(call, &self.seen, "get", std::slice::from_ref(value))? {
            return Ok(Data::Seen(number as usize));
        }
        let class = call.call(&self.to_string, value, &[])?;
        let class = call.to_string(&class)?;
        let name = class.trim_start_matches("[object ").trim_end_matches(']').to_owned();
        // A BigInt is a primitive, boxed or not; symbols and functions can't be kept.
        if name == "BigInt" {
            return Ok(Data::BigInt(call.to_string(value)?));
        }
        if name == "Symbol" || name.ends_with("Function") {
            return Err(data_clone_error(if name == "Symbol" { "a symbol" } else { "a function" }));
        }
        // Numbered before what it holds, so cycles come back to it.
        method(call, &self.seen, "set", &[value.clone(), Value::Number(self.count as f64)])?;
        self.count += 1;
        Ok(match name.as_str() {
            "Array" => {
                let length = call.get(original, "length")?;
                Data::Array(call.to_number(&length)?, self.members(call, original, depth)?)
            }
            "Object" | "Arguments" => Data::Object(self.members(call, original, depth)?),
            "Map" | "Set" => {
                let array = object(call, &self.global, "Array")?;
                let Value::Js(entries) = method(call, &array, "from", std::slice::from_ref(value))? else {
                    return Ok(Data::Set(Vec::new()));
                };
                let mut pairs = Vec::new();
                let mut items = Vec::new();
                for i in 0..length(call, &entries)? {
                    let entry = call.get(&entries, &i.to_string())?;
                    if name == "Map" {
                        let Value::Js(pair) = &entry else { continue };
                        let (key, item) = (call.get(pair, "0")?, call.get(pair, "1")?);
                        pairs.push((self.serialize(call, &key, depth + 1)?, self.serialize(call, &item, depth + 1)?));
                    } else {
                        items.push(self.serialize(call, &entry, depth + 1)?);
                    }
                }
                if name == "Map" { Data::Map(pairs) } else { Data::Set(items) }
            }
            "Date" => {
                let time = method(call, original, "getTime", &[])?;
                Data::Date(call.to_number(&time)?)
            }
            "RegExp" => {
                let (source, flags) = (call.get(original, "source")?, call.get(original, "flags")?);
                Data::RegExp(call.to_string(&source)?, call.to_string(&flags)?)
            }
            "Boolean" | "Number" | "String" => {
                let primitive = method(call, original, "valueOf", &[])?;
                Data::Boxed(name.clone(), Box::new(self.serialize(call, &primitive, depth + 1)?))
            }
            "ArrayBuffer" => Data::Buffer(call.bytes(original)?),
            name if name == "DataView" || TYPED_ARRAYS.contains(&name) => {
                let buffer = call.get(original, "buffer")?;
                let buffer = self.serialize(call, &buffer, depth + 1)?;
                let offset = call.get(original, "byteOffset")?;
                let length = call.get(original, if name == "DataView" { "byteLength" } else { "length" })?;
                Data::View(name.to_owned(), Box::new(buffer), call.to_number(&offset)?, call.to_number(&length)?)
            }
            "Error" => {
                let error_name = call.get(original, "name")?;
                let error_name = call.to_string(&error_name)?;
                let message = call.get(original, "message")?;
                let stack = match call.get(original, "stack")? {
                    Value::String(stack) => Some(stack),
                    _ => None,
                };
                let class = if ERRORS.contains(&error_name.as_str()) { error_name } else { "Error".into() };
                Data::Error(class, call.to_string(&message)?, stack)
            }
            "ImageData" => {
                let data = call.get(original, "data")?;
                let data = self.serialize(call, &data, depth + 1)?;
                let (width, height) = (call.get(original, "width")?, call.get(original, "height")?);
                Data::Pixels(Box::new(data), call.to_number(&width)?, call.to_number(&height)?)
            }
            #[cfg(not(target_arch = "wasm32"))]
            "Blob" => {
                let bytes = crate::blob::blob_bytes(call, value)?;
                let kind = call.get(original, "type")?;
                Data::Blob(bytes, call.to_string(&kind)?)
            }
            _ => match geometry_values(call, original, &name)? {
                Some(values) => Data::Geometry(name.clone(), values),
                None => return Err(data_clone_error(&class)),
            },
        })
    }

    /// The own enumerable properties of `original`, serialized.
    fn members(&mut self, call: &mut dyn Call<H>, original: &H, depth: usize) -> Result<Vec<(String, Data)>, Error> {
        let object_class = object(call, &self.global, "Object")?;
        let Value::Js(keys) = method(call, &object_class, "keys", &[Value::Js(original.clone())])? else {
            return Ok(Vec::new());
        };
        let mut members = Vec::new();
        for i in 0..length(call, &keys)? {
            let key = call.get(&keys, &i.to_string())?;
            let key = call.to_string(&key)?;
            let member = call.get(original, &key)?;
            members.push((key, self.serialize(call, &member, depth + 1)?));
        }
        Ok(members)
    }
}

/// `data`, made again in the realm `call` is in.
pub(crate) fn deserialize<H: Clone>(call: &mut dyn Call<H>, data: &Data) -> Result<Value<H>, Error> {
    let (global, _) = classes(call)?;
    let mut made = Vec::new();
    make(call, &global, data, &mut made)
}

/// `data` made, with `made` the objects made so far, by number.
fn make<H: Clone>(call: &mut dyn Call<H>, global: &H, data: &Data, made: &mut Vec<Value<H>>) -> Result<Value<H>, Error> {
    let construct = |call: &mut dyn Call<H>, class: &str, args: &[Value<H>]| -> Result<Value<H>, Error> {
        let class = object(call, global, class)?;
        call.construct(&class, args)
    };
    // An object is numbered when it's made, before what it holds is.
    let number = made.len();
    let value = match data {
        Data::Undefined => return Ok(Value::Undefined),
        Data::Null => return Ok(Value::Null),
        Data::Bool(b) => return Ok(Value::Bool(*b)),
        Data::Number(n) => return Ok(Value::Number(*n)),
        Data::String(s) => return Ok(Value::String(s.clone())),
        Data::BigInt(digits) => {
            let big_int = object(call, global, "BigInt")?;
            return call.call(&big_int, &Value::Undefined, &[Value::String(digits.clone())]);
        }
        Data::Seen(number) => return made.get(*number).cloned().ok_or_else(|| Error::Script("the serialized value is broken".into())),
        Data::Object(members) | Data::Array(_, members) => {
            let made_object = match data {
                Data::Array(length, _) => construct(call, "Array", &[Value::Number(*length)])?,
                _ => Value::Js(call.new_object()?),
            };
            made.push(made_object.clone());
            if let Value::Js(target) = &made_object {
                for (key, member) in members {
                    let member = make(call, global, member, made)?;
                    call.set(target, key, &member)?;
                }
            }
            return Ok(made_object);
        }
        Data::Map(pairs) => {
            let map = construct(call, "Map", &[])?;
            made.push(map.clone());
            if let Value::Js(target) = &map {
                for (key, item) in pairs {
                    let (key, item) = (make(call, global, key, made)?, make(call, global, item, made)?);
                    method(call, target, "set", &[key, item])?;
                }
            }
            return Ok(map);
        }
        Data::Set(items) => {
            let set = construct(call, "Set", &[])?;
            made.push(set.clone());
            if let Value::Js(target) = &set {
                for item in items {
                    let item = make(call, global, item, made)?;
                    method(call, target, "add", &[item])?;
                }
            }
            return Ok(set);
        }
        Data::Date(time) => construct(call, "Date", &[Value::Number(*time)])?,
        Data::RegExp(source, flags) => construct(call, "RegExp", &[Value::String(source.clone()), Value::String(flags.clone())])?,
        Data::Boxed(class, primitive) => {
            made.push(Value::Undefined);
            let primitive = make(call, global, primitive, made)?;
            let boxed = construct(call, class, &[primitive])?;
            if let Some(slot) = made.get_mut(number) {
                *slot = boxed.clone();
            }
            return Ok(boxed);
        }
        Data::Buffer(bytes) => {
            let array = call.new_uint8_array(bytes)?;
            call.get(&array, "buffer")?
        }
        Data::View(class, buffer, offset, length) => {
            made.push(Value::Undefined);
            let buffer = make(call, global, buffer, made)?;
            let view = construct(call, class, &[buffer, Value::Number(*offset), Value::Number(*length)])?;
            if let Some(slot) = made.get_mut(number) {
                *slot = view.clone();
            }
            return Ok(view);
        }
        Data::Error(class, message, stack) => {
            let error = construct(call, class, &[Value::String(message.clone())])?;
            if let (Value::Js(target), Some(stack)) = (&error, stack) {
                call.set(target, "stack", &Value::String(stack.clone()))?;
            }
            error
        }
        Data::Pixels(data, width, height) => {
            made.push(Value::Undefined);
            let data = make(call, global, data, made)?;
            let image = construct(call, "ImageData", &[data, Value::Number(*width), Value::Number(*height)])?;
            if let Some(slot) = made.get_mut(number) {
                *slot = image.clone();
            }
            return Ok(image);
        }
        #[cfg(not(target_arch = "wasm32"))]
        Data::Blob(bytes, kind) => crate::blob::new_blob(call, bytes, kind)?,
        Data::Geometry(class, values) => geometry_object(call, class, values)?.ok_or_else(|| data_clone_error(class))?,
    };
    made.push(value.clone());
    Ok(value)
}

/// The numbers of a geometry object, whose class `class` names (`DOMPoint` …): a point's `x`, `y`,
/// `z` and `w`, a rect's `x`, `y`, `width` and `height`, a quad's points' sixteen, a matrix's
/// sixteen and whether it's 2D. None for another class.
fn geometry_values<H: Clone>(call: &mut dyn Call<H>, original: &H, class: &str) -> Result<Option<Vec<f64>>, Error> {
    let numbers = |call: &mut dyn Call<H>, object: &H, keys: &[&str]| -> Result<Vec<f64>, Error> {
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            let value = call.get(object, key)?;
            values.push(call.to_number(&value)?);
        }
        Ok(values)
    };
    Ok(Some(match class {
        "DOMPoint" | "DOMPointReadOnly" => numbers(call, original, &POINT)?,
        "DOMRect" | "DOMRectReadOnly" => numbers(call, original, &["x", "y", "width", "height"])?,
        "DOMQuad" => {
            let mut values = Vec::with_capacity(16);
            for key in ["p1", "p2", "p3", "p4"] {
                let point = crate::handle(call.get(original, key)?, key)?;
                values.extend(numbers(call, &point, &POINT)?);
            }
            values
        }
        "DOMMatrix" | "DOMMatrixReadOnly" => {
            let mut values = numbers(call, original, &MATRIX)?;
            values.push(if crate::truthy(&call.get(original, "is2D")?) { 1.0 } else { 0.0 });
            values
        }
        _ => return Ok(None),
    }))
}

/// The geometry object of class `class` with `values`, as [`geometry_values`] gives them; none for
/// another class or numbers that don't fit it.
fn geometry_object<H: Clone>(call: &mut dyn Call<H>, class: &str, values: &[f64]) -> Result<Option<Value<H>>, Error> {
    let numbers = |values: &[f64]| values.iter().map(|n| Value::Number(*n)).collect::<Vec<_>>();
    let made = match (class, values.len()) {
        ("DOMPoint" | "DOMPointReadOnly" | "DOMRect" | "DOMRectReadOnly", 4) => {
            let constructor = crate::global_class(call, class)?;
            call.construct(&constructor, &numbers(values))?
        }
        ("DOMQuad", 16) => {
            let point = crate::global_class(call, "DOMPoint")?;
            let mut points = Vec::with_capacity(4);
            for chunk in values.chunks(4) {
                points.push(call.construct(&point, &numbers(chunk))?);
            }
            let constructor = crate::global_class(call, class)?;
            call.construct(&constructor, &points)?
        }
        ("DOMMatrix" | "DOMMatrixReadOnly", 17) => {
            // A 2D matrix from its six values, which keep it 2D.
            let (m, is_2d) = (values.get(..16).unwrap_or_default(), values.get(16) == Some(&1.0));
            let list = if is_2d { [0, 1, 4, 5, 12, 13].iter().filter_map(|&i| m.get(i).copied()).collect() } else { m.to_vec() };
            let list = call.new_float64_array(&list)?;
            let constructor = crate::global_class(call, class)?;
            call.construct(&constructor, &[Value::Js(list)])?
        }
        _ => return Ok(None),
    };
    Ok(Some(made))
}

/// The values of a point, and of a matrix, by name.
const POINT: [&str; 4] = ["x", "y", "z", "w"];
const MATRIX: [&str; 16] = ["m11", "m12", "m13", "m14", "m21", "m22", "m23", "m24", "m31", "m32", "m33", "m34", "m41", "m42", "m43", "m44"];
