//! `structuredClone(value)`, as browsers copy values between windows and workers: primitives as
//! they are; arrays and objects deeply, as plain ones (their own enumerable properties), each
//! object once, so shared and cyclic ones stay so; `Date`, `RegExp`, `Map`, `Set`, `ArrayBuffer`,
//! typed arrays and `DataView` (sharing the copies of their buffers as they shared them), the
//! `Error` classes, boxed primitives, `Blob`, `ImageData` and the geometry classes as themselves.
//! Functions, symbols and other objects throw a `DataCloneError`. Transferring isn't supported.

use crate::{Call, Engine, Error, Value, method, object};

/// How deep objects may nest, before the copy would run out of stack.
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

/// The `Error` classes a copy keeps.
const ERRORS: [&str; 7] = ["Error", "EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError"];

/// The realm's global object and `Object.prototype.toString`, which names an object's class.
pub(crate) struct Classes<H> {
    global: H,
    to_string: H,
}

/// Sets `structuredClone` on `global`, unless it has it.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let function = engine.function("structuredClone", structured_clone)?;
    engine.with_call(|call| {
        if !matches!(call.get(global, "structuredClone")?, Value::Undefined) {
            return Ok(());
        }
        call.set(global, "structuredClone", &Value::Js(function))?;
        let object_class = object(call, global, "Object")?;
        let prototype = object(call, &object_class, "prototype")?;
        let to_string = object(call, &prototype, "toString")?;
        call.host()?.borrow_mut().clone_classes = Some(Classes { global: global.clone(), to_string });
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
    let (global, to_string) = {
        let host = call.host()?;
        let host = host.borrow();
        let classes = host.clone_classes.as_ref().ok_or_else(|| Error::Script("structuredClone is not set up".into()))?;
        (classes.global.clone(), classes.to_string.clone())
    };
    let map = object(call, &global, "Map")?;
    let Value::Js(seen) = call.construct(&map, &[])? else {
        return Err(Error::Script("new Map() made nothing".into()));
    };
    let mut copier = Copier { global, to_string, seen };
    copier.copy(call, args.first().unwrap_or(&Value::Undefined), 0)
}

/// A copy in progress: what it uses of the realm, and the objects copied so far, by original.
struct Copier<H> {
    global: H,
    to_string: H,
    seen: H,
}

impl<H: Clone> Copier<H> {
    fn class(&self, call: &mut dyn Call<H>, name: &str) -> Result<H, Error> {
        object(call, &self.global, name)
    }

    fn length(call: &mut dyn Call<H>, object: &H) -> Result<usize, Error> {
        Ok(match call.get(object, "length")? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
            _ => 0,
        })
    }

    /// The copy of `value`, `depth` objects deep.
    fn copy(&mut self, call: &mut dyn Call<H>, value: &Value<H>, depth: usize) -> Result<Value<H>, Error> {
        let Value::Js(original) = value else {
            return Ok(value.clone());
        };
        if depth > DEPTH {
            return Err(Error::Range("structuredClone: the value nests too deeply".into()));
        }
        if crate::truthy(&method(call, &self.seen, "has", std::slice::from_ref(value))?) {
            return method(call, &self.seen, "get", std::slice::from_ref(value));
        }
        let class = call.call(&self.to_string, value, &[])?;
        let class = call.to_string(&class)?;
        let name = class.trim_start_matches("[object ").trim_end_matches(']').to_owned();
        // What holds others is remembered before they're copied, so cycles come back to it.
        let copy = match name.as_str() {
            "Array" => {
                let array = self.class(call, "Array")?;
                let copy = call.construct(&array, &[])?;
                self.remember(call, value, &copy)?;
                self.copy_members(call, original, &copy, depth)?;
                return Ok(copy);
            }
            "Object" | "Arguments" => {
                let copy = Value::Js(call.new_object()?);
                self.remember(call, value, &copy)?;
                self.copy_members(call, original, &copy, depth)?;
                return Ok(copy);
            }
            "Map" | "Set" => {
                let class = self.class(call, &name)?;
                let copy = call.construct(&class, &[])?;
                self.remember(call, value, &copy)?;
                let array = self.class(call, "Array")?;
                let Value::Js(entries) = method(call, &array, "from", std::slice::from_ref(value))? else {
                    return Ok(copy);
                };
                let Value::Js(target) = &copy else {
                    return Ok(copy);
                };
                for i in 0..Self::length(call, &entries)? {
                    let entry = call.get(&entries, &i.to_string())?;
                    if name == "Map" {
                        let Value::Js(pair) = &entry else { continue };
                        let (key, item) = (call.get(pair, "0")?, call.get(pair, "1")?);
                        let (key, item) = (self.copy(call, &key, depth + 1)?, self.copy(call, &item, depth + 1)?);
                        method(call, target, "set", &[key, item])?;
                    } else {
                        let item = self.copy(call, &entry, depth + 1)?;
                        method(call, target, "add", &[item])?;
                    }
                }
                return Ok(copy);
            }
            "Date" => {
                let time = method(call, original, "getTime", &[])?;
                let class = self.class(call, "Date")?;
                call.construct(&class, &[time])?
            }
            "RegExp" => {
                let (source, flags) = (call.get(original, "source")?, call.get(original, "flags")?);
                let class = self.class(call, "RegExp")?;
                call.construct(&class, &[source, flags])?
            }
            "Boolean" | "Number" | "String" => {
                let primitive = method(call, original, "valueOf", &[])?;
                let class = self.class(call, &name)?;
                call.construct(&class, &[primitive])?
            }
            // A BigInt is a primitive (or boxed one).
            "BigInt" => return Ok(value.clone()),
            "ArrayBuffer" => method(call, original, "slice", &[Value::Number(0.0)])?,
            "DataView" => {
                let buffer = call.get(original, "buffer")?;
                let buffer = self.copy(call, &buffer, depth + 1)?;
                let (offset, length) = (call.get(original, "byteOffset")?, call.get(original, "byteLength")?);
                let class = self.class(call, "DataView")?;
                call.construct(&class, &[buffer, offset, length])?
            }
            name if TYPED_ARRAYS.contains(&name) => {
                let buffer = call.get(original, "buffer")?;
                let buffer = self.copy(call, &buffer, depth + 1)?;
                let (offset, length) = (call.get(original, "byteOffset")?, call.get(original, "length")?);
                let class = self.class(call, name)?;
                call.construct(&class, &[buffer, offset, length])?
            }
            "Error" => {
                let error_name = call.get(original, "name")?;
                let error_name = call.to_string(&error_name)?;
                let class = self.class(call, if ERRORS.contains(&error_name.as_str()) { &error_name } else { "Error" })?;
                let message = call.get(original, "message")?;
                let copy = call.construct(&class, &[message])?;
                if let Value::Js(target) = &copy {
                    let stack = call.get(original, "stack")?;
                    call.set(target, "stack", &stack)?;
                }
                copy
            }
            "ImageData" => {
                let data = call.get(original, "data")?;
                let data = self.copy(call, &data, depth + 1)?;
                let (width, height) = (call.get(original, "width")?, call.get(original, "height")?);
                let class = self.class(call, "ImageData")?;
                call.construct(&class, &[data, width, height])?
            }
            #[cfg(not(target_arch = "wasm32"))]
            "Blob" => {
                let bytes = crate::fetch::blob_bytes(call, value)?;
                let kind = call.get(original, "type")?;
                let kind = call.to_string(&kind)?;
                crate::fetch::new_blob(call, &bytes, &kind)?
            }
            _ => match crate::geometry::copy(call, value, &class)? {
                Some(copy) => copy,
                None => return Err(data_clone_error(if name.ends_with("Function") { "a function" } else { &class })),
            },
        };
        self.remember(call, value, &copy)?;
        Ok(copy)
    }

    fn remember(&self, call: &mut dyn Call<H>, original: &Value<H>, copy: &Value<H>) -> Result<(), Error> {
        method(call, &self.seen, "set", &[original.clone(), copy.clone()])?;
        Ok(())
    }

    /// Copies the own enumerable properties of `original` onto `copy`.
    fn copy_members(&mut self, call: &mut dyn Call<H>, original: &H, copy: &Value<H>, depth: usize) -> Result<(), Error> {
        let Value::Js(target) = copy else {
            return Ok(());
        };
        let object_class = self.class(call, "Object")?;
        let Value::Js(keys) = method(call, &object_class, "keys", &[Value::Js(original.clone())])? else {
            return Ok(());
        };
        for i in 0..Self::length(call, &keys)? {
            let key = call.get(&keys, &i.to_string())?;
            let key = call.to_string(&key)?;
            let member = call.get(original, &key)?;
            let member = self.copy(call, &member, depth + 1)?;
            call.set(target, &key, &member)?;
        }
        Ok(())
    }
}
