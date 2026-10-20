//! The interface every JavaScript engine implements: V8 on desktop, the browser's engine on the
//! web and Boa where V8 isn't available. It is deliberately small, so that everything scripts see
//! (Paper.js, `console`, timers, the document binding) is written once, above it.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{Error, Host};

/// A value passed between Rust and an engine. Values that Rust doesn't look into (objects,
/// functions, symbols, big integers) stay in the engine; `H` is the engine's handle to one.
#[derive(Clone, Debug)]
pub enum Value<H> {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Js(H),
}

/// Stops the script an engine runs, from another thread: the script throws an error that it can't
/// catch.
pub type Interrupt = Box<dyn Fn() + Send + Sync>;

/// A Rust function that scripts can call, with `this` and the arguments. Errors are thrown in the
/// script as `Error`s.
pub type HostFunction<H> = fn(&mut dyn Call<H>, &Value<H>, &[Value<H>]) -> Result<Value<H>, Error>;

/// Sets up a new engine's realm, before the host's functions are added.
pub type Prelude<E> = fn(&mut E) -> Result<(), Error>;

/// What Rust can do with the engine's values: inside a [`HostFunction`] while a script calls it,
/// and outside scripts through [`Engine::with_call`]. Exceptions a script throws come back as
/// errors.
pub trait Call<H> {
    /// The state shared by the host functions of this runtime.
    fn host(&self) -> Result<Rc<RefCell<Host<H>>>, Error>;

    /// JavaScript's `String(value)`.
    fn to_string(&mut self, value: &Value<H>) -> Result<String, Error>;

    /// JavaScript's `Number(value)`.
    fn to_number(&mut self, value: &Value<H>) -> Result<f64, Error>;

    fn new_object(&mut self) -> Result<H, Error>;

    fn new_array(&mut self, elements: &[Value<H>]) -> Result<H, Error>;

    fn new_float64_array(&mut self, elements: &[f64]) -> Result<H, Error>;

    /// The bytes of a typed array, a `DataView` or an `ArrayBuffer`, copied in one go.
    fn bytes(&mut self, object: &H) -> Result<Vec<u8>, Error>;

    fn new_uint8_array(&mut self, elements: &[u8]) -> Result<H, Error>;

    fn new_uint16_array(&mut self, elements: &[u16]) -> Result<H, Error>;

    /// `object[key]`.
    fn get(&mut self, object: &H, key: &str) -> Result<Value<H>, Error>;

    /// `object[key] = value`.
    fn set(&mut self, object: &H, key: &str, value: &Value<H>) -> Result<(), Error>;

    /// `function.apply(this, args)`.
    fn call(&mut self, function: &H, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error>;

    /// `new constructor(...args)`.
    fn construct(&mut self, constructor: &H, args: &[Value<H>]) -> Result<Value<H>, Error>;
}

/// A JavaScript engine with one realm (global object) for scripts.
pub trait Engine: Sized {
    /// A handle to a value that stays in the engine.
    type Handle: Clone;

    /// Creates the engine, its realm set up by `prelude`. Host functions reach `host` through
    /// [`Call::host`]. A process passes the same `prelude` every time, and it makes no host
    /// functions, so an engine may run it once and start later realms from a snapshot of it.
    fn new(host: Rc<RefCell<Host<Self::Handle>>>, prelude: Prelude<Self>) -> Result<Self, Error>;

    /// Runs `source` as a classic script in the global scope and returns its completion value.
    fn eval(&mut self, source: &str) -> Result<Value<Self::Handle>, Error>;

    fn global(&mut self) -> Result<Self::Handle, Error>;

    /// A function that calls `function`, named `name` in scripts.
    fn function(&mut self, name: &str, function: HostFunction<Self::Handle>) -> Result<Self::Handle, Error>;

    /// A constructor that calls `function` with `this` the object `new` made, of the class it was
    /// called as (`new.target`, so classes can extend it), as in JavaScript: `new` returns that
    /// object, unless `function` returns another one. Called without `new`, it throws a
    /// `TypeError`.
    fn constructor(&mut self, name: &str, function: HostFunction<Self::Handle>) -> Result<Self::Handle, Error>;

    /// Runs `f` with the engine's values, as a [`HostFunction`] has them.
    fn with_call<R>(&mut self, f: impl FnOnce(&mut dyn Call<Self::Handle>) -> Result<R, Error>) -> Result<R, Error>;

    /// A way to stop scripts from another thread, for engines that have one (V8).
    fn interrupt(&mut self) -> Option<Interrupt> {
        None
    }
}
