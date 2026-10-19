//! A minimal `OffscreenCanvas`, for the one thing Paper.js needs a canvas for without a DOM:
//! resolving CSS colours (`new Color('red')`). Paper.js paints a pixel with the colour as
//! `fillStyle` and reads it back.
//!
//! The canvas holds no pixels. It is its own 2D context, `getImageData()` returns the current
//! `fillStyle` as the pixel, and everything else Paper.js calls on it does nothing. Canvas features
//! beyond that (text measuring, rasters) are missing until VectorCraft's text engine and renderer
//! back them.

use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;

use crate::{Call, Engine, Error, Host, HostFunction, Value};

/// Sets `OffscreenCanvas` on `global`, and the methods every canvas gets on `host`.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle, host: &Rc<RefCell<Host<E::Handle>>>) -> Result<(), Error> {
    let methods: [(&str, HostFunction<E::Handle>); 6] = [
        ("getContext", get_context),
        ("getImageData", get_image_data),
        ("fillRect", nothing),
        ("clearRect", nothing),
        ("save", nothing),
        ("restore", nothing),
    ];
    let mut handles = Vec::with_capacity(methods.len());
    for (name, method) in methods {
        handles.push((name, engine.function(name, method)?));
    }
    host.borrow_mut().canvas_methods = handles;
    let constructor = engine.constructor("OffscreenCanvas", offscreen_canvas)?;
    engine.set(global, "OffscreenCanvas", &Value::Js(constructor))
}

/// `new OffscreenCanvas(width, height)`.
fn offscreen_canvas<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let canvas = call.new_object()?;
    let size = |index: usize| match args.get(index) {
        Some(Value::Number(size)) => *size,
        _ => 0.0,
    };
    call.set(&canvas, "width", &Value::Number(size(0)))?;
    call.set(&canvas, "height", &Value::Number(size(1)))?;
    call.set(&canvas, "canvas", &Value::Js(canvas.clone()))?;
    call.set(&canvas, "fillStyle", &Value::String("#000000".into()))?;
    let methods = call.host()?.borrow().canvas_methods.clone();
    for (name, method) in methods {
        call.set(&canvas, name, &Value::Js(method))?;
    }
    Ok(Value::Js(canvas))
}

/// `canvas.getContext('2d')`: the canvas itself.
fn get_context<H: Clone>(_call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    Ok(this.clone())
}

/// `context.getImageData()`: one pixel of the current `fillStyle`, `{ data: [r, g, b, a] }`. A
/// colour CSS doesn't know is transparent black, as on a canvas cleared to `rgba(0,0,0,0)`.
fn get_image_data<H>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Value::Js(context) = this else {
        return Err(Error::Script("getImageData: not called on a canvas".into()));
    };
    let color = match call.get(context, "fillStyle")? {
        Value::String(style) => svgtypes::Color::from_str(&style).ok(),
        _ => None,
    };
    let color = color.unwrap_or(svgtypes::Color::new_rgba(0, 0, 0, 0));
    let pixel: Vec<_> = [color.red, color.green, color.blue, color.alpha].into_iter().map(|c| Value::Number(f64::from(c))).collect();
    let data = call.new_array(&pixel)?;
    let image = call.new_object()?;
    call.set(&image, "data", &Value::Js(data))?;
    Ok(Value::Js(image))
}

fn nothing<H>(_call: &mut dyn Call<H>, _this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    Ok(Value::Undefined)
}
