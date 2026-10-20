//! `EventTarget`, `Event` and `CustomEvent` for scripts outside a browser, as the DOM standard has
//! them, to extend (`class Emitter extends EventTarget`): listeners with `capture`, `once`,
//! `passive` and `signal`, objects with `handleEvent()`, `dispatchEvent()`, `preventDefault()` and
//! the propagation methods. There's no tree to propagate through: an event reaches its target only
//! (DOM nodes have linkedom's own events). A target's listeners and an event's state are kept in
//! the realm's `WeakMap`s, out of the objects' own properties, so any object of the classes, or
//! that adopts their prototypes (`AbortSignal`), has them.

use crate::{Call, Engine, Error, HostFunction, Value, define_accessor, method, object};

/// The event phases, as `Event.NONE` … name them.
const NONE: f64 = 0.0;
const AT_TARGET: f64 = 2.0;
const PHASES: [(&str, f64); 4] = [("NONE", NONE), ("CAPTURING_PHASE", 1.0), ("AT_TARGET", AT_TARGET), ("BUBBLING_PHASE", 3.0)];

/// What an event keeps, its getters' names: `srcElement` is its target.
const FIELDS: [&str; 11] =
    ["type", "bubbles", "cancelable", "composed", "defaultPrevented", "eventPhase", "target", "currentTarget", "timeStamp", "isTrusted", "detail"];

/// What the classes share: the realm's `WeakMap`s of targets' listeners and events' states, the
/// `Event` prototype, `Object`, and `performance`, the clock of `timeStamp`.
#[derive(Clone)]
pub(crate) struct Classes<H> {
    object: H,
    /// `Object.prototype.toString`, which tells functions, and the function that removes a
    /// listener when its `signal` aborts.
    to_string: H,
    remover: H,
    listeners: H,
    states: H,
    event: H,
    performance: Option<H>,
}

/// Sets `EventTarget`, `Event` and `CustomEvent` on `global`, unless it has them.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    if engine.with_call(|call| Ok(!matches!(call.get(global, "EventTarget")?, Value::Undefined)))? {
        return Ok(());
    }
    let target = engine.constructor("EventTarget", |_, _, _| Ok(Value::Undefined))?;
    let event = engine.constructor("Event", |call, this, args| construct_event(call, this, args, false))?;
    let custom = engine.constructor("CustomEvent", |call, this, args| construct_event(call, this, args, true))?;
    let target_methods: [(&str, HostFunction<E::Handle>); 3] =
        [("addEventListener", add_listener), ("removeEventListener", remove_listener), ("dispatchEvent", dispatch_event)];
    let event_methods: [(&str, HostFunction<E::Handle>); 5] = [
        ("preventDefault", prevent_default),
        ("stopPropagation", |call, this, _| set_state(call, this, "_stop", Value::Bool(true))),
        ("stopImmediatePropagation", |call, this, _| {
            set_state(call, this, "_stop", Value::Bool(true))?;
            set_state(call, this, "_stopImmediately", Value::Bool(true))
        }),
        ("composedPath", composed_path),
        ("initEvent", init_event),
    ];
    let make = |engine: &mut E, list: &[(&'static str, HostFunction<E::Handle>)]| -> Result<Vec<(&'static str, E::Handle)>, Error> {
        list.iter().map(|(name, f)| Ok((*name, engine.function(name, *f)?))).collect()
    };
    let target_methods = make(engine, &target_methods)?;
    let event_methods = make(engine, &event_methods)?;
    let init_custom = engine.function("initCustomEvent", init_custom_event)?;
    let remover = engine.function("remove", remove_aborted)?;
    macro_rules! getters {
        ($($i:literal)*) => { [$(get::<E::Handle, $i> as HostFunction<E::Handle>),*] };
    }
    let getters: [HostFunction<E::Handle>; FIELDS.len()] = getters!(0 1 2 3 4 5 6 7 8 9 10);
    let mut fields = Vec::new();
    for (name, get) in FIELDS.iter().zip(getters) {
        fields.push((*name, engine.function(name, get)?));
    }
    let src_element = engine.function("srcElement", get::<E::Handle, 6>)?;
    let get_return = engine.function("returnValue", |call, this, _| Ok(Value::Bool(!crate::truthy(&state(call, this, "defaultPrevented")?))))?;
    let set_return = engine.function("returnValue", |call, this, args| {
        if !crate::truthy(args.first().unwrap_or(&Value::Bool(true))) {
            prevent_default(call, this, &[])?;
        }
        Ok(Value::Undefined)
    })?;
    let get_cancel = engine.function("cancelBubble", |call, this, _| Ok(Value::Bool(crate::truthy(&state(call, this, "_stop")?))))?;
    let set_cancel = engine.function("cancelBubble", |call, this, args| {
        if crate::truthy(args.first().unwrap_or(&Value::Undefined)) {
            set_state(call, this, "_stop", Value::Bool(true))?;
        }
        Ok(Value::Undefined)
    })?;
    engine.with_call(|call| {
        let object_class = object(call, global, "Object")?;
        let weak_map = object(call, global, "WeakMap")?;
        let target_prototype = call.new_object()?;
        for (name, function) in target_methods {
            call.set(&target_prototype, name, &Value::Js(function))?;
        }
        let event_prototype = call.new_object()?;
        for (name, function) in event_methods {
            call.set(&event_prototype, name, &Value::Js(function))?;
        }
        let mut detail = None;
        for (name, get) in fields {
            if name == "detail" {
                detail = Some(get);
            } else {
                define_accessor(call, global, &event_prototype, name, get, None)?;
            }
        }
        define_accessor(call, global, &event_prototype, "srcElement", src_element, None)?;
        define_accessor(call, global, &event_prototype, "returnValue", get_return, Some(set_return))?;
        define_accessor(call, global, &event_prototype, "cancelBubble", get_cancel, Some(set_cancel))?;
        for (name, phase) in PHASES {
            call.set(&event_prototype, name, &Value::Number(phase))?;
            call.set(&event, name, &Value::Number(phase))?;
        }
        let Value::Js(custom_prototype) = method(call, &object_class, "create", &[Value::Js(event_prototype.clone())])? else {
            return Err(Error::Script("Object.create made nothing".into()));
        };
        call.set(&custom_prototype, "initCustomEvent", &Value::Js(init_custom))?;
        if let Some(detail) = detail {
            define_accessor(call, global, &custom_prototype, "detail", detail, None)?;
        }
        method(call, &object_class, "setPrototypeOf", &[Value::Js(custom.clone()), Value::Js(event.clone())])?;
        for (name, class, prototype) in
            [("EventTarget", &target, &target_prototype), ("Event", &event, &event_prototype), ("CustomEvent", &custom, &custom_prototype)]
        {
            call.set(prototype, "constructor", &Value::Js(class.clone()))?;
            call.set(class, "prototype", &Value::Js(prototype.clone()))?;
            call.set(global, name, &Value::Js(class.clone()))?;
            crate::tag(call, global, prototype, name)?;
        }
        let new_map = |call: &mut dyn Call<E::Handle>| match call.construct(&weak_map, &[])? {
            Value::Js(map) => Ok(map),
            _ => Err(Error::Script("new WeakMap() made nothing".into())),
        };
        let object_prototype = object(call, &object_class, "prototype")?;
        let classes = Classes {
            to_string: object(call, &object_prototype, "toString")?,
            remover,
            object: object_class,
            listeners: new_map(call)?,
            states: new_map(call)?,
            event: event_prototype,
            performance: match call.get(global, "performance")? {
                Value::Js(performance) => Some(performance),
                _ => None,
            },
        };
        call.host()?.borrow_mut().events = Some(classes);
        Ok(())
    })
}

fn shared<H: Clone>(call: &mut dyn Call<H>) -> Result<Classes<H>, Error> {
    let host = call.host()?;
    let host = host.borrow();
    host.events.clone().ok_or_else(|| Error::Script("events are not set up".into()))
}

/// The state of the event `this` is, made when the event is.
fn event_state<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>) -> Result<Option<H>, Error> {
    let states = shared(call)?.states;
    Ok(match method(call, &states, "get", std::slice::from_ref(this))? {
        Value::Js(state) => Some(state),
        _ => None,
    })
}

/// The event `this` is's `key`.
fn state<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<Value<H>, Error> {
    match event_state(call, this)? {
        Some(state) => call.get(&state, key),
        None => Err(Error::Type("not an Event".into())),
    }
}

fn set_state<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str, value: Value<H>) -> Result<Value<H>, Error> {
    match event_state(call, this)? {
        Some(state) => call.set(&state, key, &value)?,
        None => return Err(Error::Type("not an Event".into())),
    }
    Ok(Value::Undefined)
}

/// The getter of `FIELDS[I]`.
fn get<H: Clone, const I: usize>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    match FIELDS.get(I) {
        Some(field) => state(call, this, field),
        None => Ok(Value::Undefined),
    }
}

/// `new Event(type, init)` or `new CustomEvent(type, init)` (`custom`), on `this`, the object `new`
/// made.
fn construct_event<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>], custom: bool) -> Result<Value<H>, Error> {
    let Some(kind) = args.first() else {
        return Err(Error::Type("Event: the type is missing".into()));
    };
    let kind = call.to_string(kind)?;
    let init = match args.get(1) {
        Some(Value::Js(init)) => Some(init.clone()),
        _ => None,
    };
    let flag = |call: &mut dyn Call<H>, key: &str| -> Result<Value<H>, Error> {
        Ok(Value::Bool(match &init {
            Some(init) => crate::truthy(&call.get(init, key)?),
            None => false,
        }))
    };
    let Classes { states, performance, .. } = shared(call)?;
    let state = call.new_object()?;
    for (key, value) in [
        ("type", Value::String(kind)),
        ("bubbles", flag(call, "bubbles")?),
        ("cancelable", flag(call, "cancelable")?),
        ("composed", flag(call, "composed")?),
        ("defaultPrevented", Value::Bool(false)),
        ("eventPhase", Value::Number(NONE)),
        ("target", Value::Null),
        ("currentTarget", Value::Null),
        ("isTrusted", Value::Bool(false)),
    ] {
        call.set(&state, key, &value)?;
    }
    let now = match performance {
        Some(performance) => method(call, &performance, "now", &[])?,
        None => Value::Number(call.host()?.borrow().now),
    };
    call.set(&state, "timeStamp", &now)?;
    let detail = match (&init, custom) {
        (Some(init), true) => match call.get(init, "detail")? {
            Value::Undefined => Value::Null,
            detail => detail,
        },
        _ => Value::Null,
    };
    call.set(&state, "detail", &detail)?;
    method(call, &states, "set", &[this.clone(), Value::Js(state)])?;
    Ok(Value::Undefined)
}

/// `preventDefault()`: cancels a cancelable event, unless a passive listener calls it.
fn prevent_default<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    if crate::truthy(&state(call, this, "cancelable")?) && !crate::truthy(&state(call, this, "_passive")?) {
        set_state(call, this, "defaultPrevented", Value::Bool(true))?;
    }
    Ok(Value::Undefined)
}

/// `composedPath()`: the target while the event is dispatched, as there's no tree around it.
fn composed_path<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    let current = state(call, this, "currentTarget")?;
    let path: Vec<Value<H>> = if matches!(current, Value::Null | Value::Undefined) { Vec::new() } else { vec![current] };
    call.new_array(&path).map(Value::Js)
}

/// `initEvent(type, bubbles, cancelable)`, the legacy way, before the event is dispatched.
fn init_event<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if crate::truthy(&state(call, this, "_dispatching")?) {
        return Ok(Value::Undefined);
    }
    let kind = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    set_state(call, this, "type", Value::String(kind))?;
    for (key, i) in [("bubbles", 1), ("cancelable", 2)] {
        set_state(call, this, key, Value::Bool(crate::truthy(args.get(i).unwrap_or(&Value::Undefined))))?;
    }
    for key in ["_stop", "_stopImmediately"] {
        set_state(call, this, key, Value::Bool(false))?;
    }
    set_state(call, this, "defaultPrevented", Value::Bool(false))
}

fn init_custom_event<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    init_event(call, this, args)?;
    if !crate::truthy(&state(call, this, "_dispatching")?) {
        set_state(call, this, "detail", args.get(3).cloned().unwrap_or(Value::Null))?;
    }
    Ok(Value::Undefined)
}

// Listeners.

/// The listener list of `target`, made when `create`.
fn listeners<H: Clone>(call: &mut dyn Call<H>, target: &Value<H>, create: bool) -> Result<Option<H>, Error> {
    let map = shared(call)?.listeners;
    match method(call, &map, "get", std::slice::from_ref(target))? {
        Value::Js(list) => Ok(Some(list)),
        _ if create => {
            let list = call.new_array(&[])?;
            method(call, &map, "set", &[target.clone(), Value::Js(list.clone())])?;
            Ok(Some(list))
        }
        _ => Ok(None),
    }
}

fn length<H: Clone>(call: &mut dyn Call<H>, list: &H) -> Result<usize, Error> {
    Ok(match call.get(list, "length")? {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    })
}

/// The listener options: `capture`, from a boolean too, and for adding, `once`, `passive` and
/// `signal`.
fn options<H: Clone>(call: &mut dyn Call<H>, value: Option<&Value<H>>) -> Result<(bool, bool, bool, Option<H>), Error> {
    Ok(match value {
        Some(Value::Js(options)) => {
            let capture = crate::truthy(&call.get(options, "capture")?);
            let once = crate::truthy(&call.get(options, "once")?);
            let passive = crate::truthy(&call.get(options, "passive")?);
            let signal = match call.get(options, "signal")? {
                Value::Js(signal) => Some(signal),
                _ => None,
            };
            (capture, once, passive, signal)
        }
        Some(value) => (crate::truthy(value), false, false, None),
        None => (false, false, false, None),
    })
}

/// The index of the listener for `kind`, `callback` and `capture` in `list`.
fn find<H: Clone>(call: &mut dyn Call<H>, list: &H, kind: &str, callback: &Value<H>, capture: bool) -> Result<Option<usize>, Error> {
    for i in 0..length(call, list)? {
        let Value::Js(record) = call.get(list, &i.to_string())? else { continue };
        let listener = call.get(&record, "callback")?;
        if matches!(call.get(&record, "type")?, Value::String(t) if t == kind)
            && crate::truthy(&call.get(&record, "capture")?) == capture
            && same(call, &listener, callback)?
        {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// Whether `a` and `b` are the same value, as `Object.is()` tells.
fn same<H: Clone>(call: &mut dyn Call<H>, a: &Value<H>, b: &Value<H>) -> Result<bool, Error> {
    let object_class = shared(call)?.object;
    Ok(crate::truthy(&method(call, &object_class, "is", &[a.clone(), b.clone()])?))
}

fn add_listener<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let kind = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let callback = match args.get(1) {
        Some(Value::Js(callback)) => Value::Js(callback.clone()),
        _ => return Ok(Value::Undefined),
    };
    let (capture, once, passive, signal) = options(call, args.get(2))?;
    if let Some(signal) = &signal
        && crate::truthy(&call.get(signal, "aborted")?)
    {
        return Ok(Value::Undefined);
    }
    let Some(list) = listeners(call, this, true)? else {
        return Ok(Value::Undefined);
    };
    if find(call, &list, &kind, &callback, capture)?.is_some() {
        return Ok(Value::Undefined);
    }
    let record = call.new_object()?;
    for (key, value) in [
        ("type", Value::String(kind)),
        ("callback", callback),
        ("capture", Value::Bool(capture)),
        ("once", Value::Bool(once)),
        ("passive", Value::Bool(passive)),
        ("removed", Value::Bool(false)),
    ] {
        call.set(&record, key, &value)?;
    }
    method(call, &list, "push", &[Value::Js(record.clone())])?;
    // Removed when the signal aborts.
    if let Some(signal) = signal {
        let remover = shared(call)?.remover;
        let bound = method(call, &remover, "bind", &[Value::Undefined, this.clone(), Value::Js(record)])?;
        add_listener(call, &Value::Js(signal), &[Value::String("abort".into()), bound])?;
    }
    Ok(Value::Undefined)
}

/// Takes `record` off the list of `target`, so a dispatch under way skips it too.
fn remove<H: Clone>(call: &mut dyn Call<H>, target: &Value<H>, record: &H) -> Result<(), Error> {
    call.set(record, "removed", &Value::Bool(true))?;
    if let Some(list) = listeners(call, target, false)?
        && let Value::Number(at) = method(call, &list, "indexOf", &[Value::Js(record.clone())])?
        && at >= 0.0
    {
        method(call, &list, "splice", &[Value::Number(at), Value::Number(1.0)])?;
    }
    Ok(())
}

/// The remover bound to a target and its listener, which its signal calls.
fn remove_aborted<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    if let (Some(target), Some(Value::Js(record))) = (args.first(), args.get(1)) {
        remove(call, target, record)?;
    }
    Ok(Value::Undefined)
}

fn remove_listener<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let kind = call.to_string(args.first().unwrap_or(&Value::Undefined))?;
    let callback = args.get(1).cloned().unwrap_or(Value::Undefined);
    let (capture, ..) = options(call, args.get(2))?;
    if let Some(list) = listeners(call, this, false)?
        && let Some(at) = find(call, &list, &kind, &callback, capture)?
        && let Value::Js(record) = call.get(&list, &at.to_string())?
    {
        remove(call, this, &record)?;
    }
    Ok(Value::Undefined)
}

fn dispatch_event<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let event = args.first().cloned().unwrap_or(Value::Undefined);
    if event_state(call, &event)?.is_none() {
        return Err(Error::Type("dispatchEvent: not an Event".into()));
    }
    if crate::truthy(&state(call, &event, "_dispatching")?) {
        return Err(Error::Dom("InvalidStateError", "the event is being dispatched".into()));
    }
    dispatch(call, this, &event).map(Value::Bool)
}

/// A new `Event` of `kind`, as the host fires them (`abort`).
pub(crate) fn new_event<H: Clone>(call: &mut dyn Call<H>, kind: &str) -> Result<Value<H>, Error> {
    let event = shared(call)?.event;
    let constructor = object(call, &event, "constructor")?;
    call.construct(&constructor, &[Value::String(kind.to_owned())])
}

/// Dispatches `event` to `target`'s listeners for it, in order: whether nothing prevented its
/// default. A listener that throws is reported, and the others run, as in browsers.
pub(crate) fn dispatch<H: Clone>(call: &mut dyn Call<H>, target: &Value<H>, event: &Value<H>) -> Result<bool, Error> {
    for (key, value) in
        [("_dispatching", Value::Bool(true)), ("target", target.clone()), ("currentTarget", target.clone()), ("eventPhase", Value::Number(AT_TARGET))]
    {
        set_state(call, event, key, value)?;
    }
    let kind = state(call, event, "type")?;
    let kind = call.to_string(&kind)?;
    // Those added meanwhile wait for the next event; those removed are skipped.
    let snapshot = match listeners(call, target, false)? {
        Some(list) => match method(call, &list, "slice", &[])? {
            Value::Js(copy) => Some(copy),
            _ => None,
        },
        None => None,
    };
    let to_string = shared(call)?.to_string;
    if let Some(snapshot) = snapshot {
        for i in 0..length(call, &snapshot)? {
            let Value::Js(record) = call.get(&snapshot, &i.to_string())? else { continue };
            if crate::truthy(&call.get(&record, "removed")?) || !matches!(call.get(&record, "type")?, Value::String(t) if t == kind) {
                continue;
            }
            if crate::truthy(&call.get(&record, "once")?) {
                remove(call, target, &record)?;
            }
            let callback = call.get(&record, "callback")?;
            let passive = call.get(&record, "passive")?;
            set_state(call, event, "_passive", passive)?;
            let class = call.call(&to_string, &callback, &[])?;
            let called = if call.to_string(&class)?.ends_with("Function]") {
                match &callback {
                    Value::Js(function) => call.call(function, target, std::slice::from_ref(event)),
                    _ => Ok(Value::Undefined),
                }
            } else {
                match &callback {
                    Value::Js(object) => match call.get(object, "handleEvent")? {
                        Value::Js(handle) => call.call(&handle, &callback, std::slice::from_ref(event)),
                        _ => Err(Error::Type("the listener has no handleEvent()".into())),
                    },
                    _ => Ok(Value::Undefined),
                }
            };
            if let Err(e) = called {
                call.host()?.borrow_mut().output.push(format!("Uncaught {e}"));
            }
            set_state(call, event, "_passive", Value::Bool(false))?;
            if crate::truthy(&state(call, event, "_stopImmediately")?) {
                break;
            }
        }
    }
    for (key, value) in [
        ("_dispatching", Value::Bool(false)),
        ("_stop", Value::Bool(false)),
        ("_stopImmediately", Value::Bool(false)),
        ("currentTarget", Value::Null),
        ("eventPhase", Value::Number(NONE)),
    ] {
        set_state(call, event, key, value)?;
    }
    Ok(!crate::truthy(&state(call, event, "defaultPrevented")?))
}
