//! `localStorage` and `sessionStorage` for scripts outside a browser. A script's `localStorage` is
//! its own, as a web page's is its origin's, and lasts: it's kept in a JSON file in the folder the
//! app gives ([`Runtime::set_script`](crate::Runtime::set_script)), named for the script's path,
//! with its `script.preferences`, written when either changed, at most once a second while the
//! script runs and when it ends. A script without a file or a folder, and `sessionStorage`, keep
//! theirs while the script runs. Each holds up to 5 MB, as browsers allow. Items read and write as
//! properties too (`localStorage.size = '10'`), through a `Proxy`.

use std::path::{Path, PathBuf};

use crate::{Call, Engine, Error, HostFunction, Value, define_accessor, method, object};

/// The most a storage holds: its keys' and values' length in UTF-16, as browsers count.
const QUOTA: usize = 5 * 1024 * 1024;

/// The key of the hidden property that tells a storage which one it is (`true` for
/// `sessionStorage`).
const SESSION: &str = "__scriptographerSessionStorage";

/// The least time between writes of a script's `localStorage`, in milliseconds.
const WRITE_INTERVAL: f64 = 1000.0;

/// What a script's file keeps.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(rename = "localStorage", default)]
    items: Vec<(String, String)>,
    #[serde(default)]
    preferences: serde_json::Value,
}

/// A storage's items, in the order they were added, and where they're kept; `localStorage`'s
/// with the script's `script.preferences`.
#[derive(Default)]
pub(crate) struct Storage {
    items: Vec<(String, String)>,
    pub(crate) preferences: serde_json::Value,
    /// The file it's kept in, if it's kept.
    file: Option<PathBuf>,
    /// Whether it changed since it was last written.
    changed: bool,
    /// When it was last written, in the runtime's milliseconds.
    written: f64,
}

impl Storage {
    /// The storage kept in `file`, with what's there: none when it can't be read.
    pub(crate) fn kept_in(file: PathBuf) -> Storage {
        let Kept { items, preferences } = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Storage { items, preferences, file: Some(file), changed: false, written: f64::NEG_INFINITY }
    }

    /// Keeps `preferences`, when they changed.
    pub(crate) fn keep_preferences(&mut self, preferences: serde_json::Value) {
        if self.preferences != preferences {
            self.preferences = preferences;
            self.changed = true;
        }
    }

    /// The file for the script at `script`'s storage in `folder`: its path's hash.
    pub(crate) fn file_for(folder: &Path, script: &Path) -> PathBuf {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(script.to_string_lossy().as_bytes());
        let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        folder.join(format!("{name}.json"))
    }

    fn size(&self) -> usize {
        self.items.iter().map(|(k, v)| k.encode_utf16().count() + v.encode_utf16().count()).sum()
    }

    /// Writes the items and preferences to their file, when they changed and (unless `now` is `None`, at the end)
    /// the last write is long enough ago. The script goes on with its items when that fails.
    pub(crate) fn write(&mut self, now: Option<f64>) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if !self.changed || now.is_some_and(|now| now - self.written < WRITE_INTERVAL) {
            return Ok(());
        }
        let kept = Kept { items: self.items.clone(), preferences: self.preferences.clone() };
        let result = serde_json::to_vec(&kept).map_err(|e| e.to_string()).and_then(|json| {
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
            }
            // Written whole, then moved over the old one, so a crash leaves one or the other.
            let partial = file.with_extension("json.partial");
            std::fs::write(&partial, json).map_err(|e| e.to_string())?;
            std::fs::rename(&partial, file).map_err(|e| e.to_string())
        });
        let result = result.map_err(|e| format!("localStorage: couldn't write {}: {e}", file.display()));
        self.changed = false;
        self.written = now.unwrap_or(self.written);
        result
    }
}

/// What the storages share: the `Proxy` handler's realm built-ins.
pub(crate) struct Classes<H> {
    reflect: H,
}

/// Sets `localStorage`, `sessionStorage` and `Storage` on `global`, unless it has them.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    if engine.with_call(|call| Ok(!matches!(call.get(global, "localStorage")?, Value::Undefined)))? {
        return Ok(());
    }
    let methods: [(&str, HostFunction<E::Handle>); 5] =
        [("getItem", get_item), ("setItem", set_item), ("removeItem", remove_item), ("clear", clear), ("key", key)];
    let mut made = Vec::new();
    for (name, f) in methods {
        made.push((name, engine.function(name, f)?));
    }
    let length = engine.function("length", |call, this, _| with(call, this, |s| Value::Number(s.items.len() as f64)))?;
    let constructor = engine.constructor("Storage", |_, _, _| Err(Error::Type("Illegal constructor".into())))?;
    let traps: [(&str, HostFunction<E::Handle>); 6] = [
        ("get", trap_get),
        ("set", trap_set),
        ("has", trap_has),
        ("deleteProperty", trap_delete),
        ("ownKeys", trap_keys),
        ("getOwnPropertyDescriptor", trap_descriptor),
    ];
    let mut handler_traps = Vec::new();
    for (name, f) in traps {
        handler_traps.push((name, engine.function(name, f)?));
    }
    engine.with_call(|call| {
        let prototype = call.new_object()?;
        for (name, function) in made {
            call.set(&prototype, name, &Value::Js(function))?;
        }
        define_accessor(call, global, &prototype, "length", length, None)?;
        call.set(&prototype, "constructor", &Value::Js(constructor.clone()))?;
        call.set(&constructor, "prototype", &Value::Js(prototype.clone()))?;
        call.set(global, "Storage", &Value::Js(constructor))?;
        crate::tag(call, global, &prototype, "Storage")?;
        let handler = call.new_object()?;
        for (name, function) in handler_traps {
            call.set(&handler, name, &Value::Js(function))?;
        }
        let object_class = object(call, global, "Object")?;
        let proxy = object(call, global, "Proxy")?;
        for (name, session) in [("localStorage", false), ("sessionStorage", true)] {
            let Value::Js(target) = method(call, &object_class, "create", &[Value::Js(prototype.clone())])? else {
                return Err(Error::Script("Object.create made nothing".into()));
            };
            // Hidden: not enumerable, and configurable, so the proxy may leave it out of its keys.
            let descriptor = call.new_object()?;
            call.set(&descriptor, "value", &Value::Bool(session))?;
            call.set(&descriptor, "configurable", &Value::Bool(true))?;
            method(call, &object_class, "defineProperty", &[Value::Js(target.clone()), Value::String(SESSION.into()), Value::Js(descriptor)])?;
            let storage = call.construct(&proxy, &[Value::Js(target), Value::Js(handler.clone())])?;
            call.set(global, name, &storage)?;
        }
        let reflect = object(call, global, "Reflect")?;
        call.host()?.borrow_mut().storages = Some(Classes { reflect });
        Ok(())
    })
}

/// `f` with the storage `this` is (its proxy or its target).
fn with<H: Clone, R>(call: &mut dyn Call<H>, this: &Value<H>, f: impl FnOnce(&mut Storage) -> R) -> Result<R, Error> {
    let Value::Js(object) = this else {
        return Err(Error::Type("not a Storage".into()));
    };
    let session = match call.get(object, SESSION)? {
        Value::Bool(session) => session,
        _ => return Err(Error::Type("not a Storage".into())),
    };
    let host = call.host()?;
    let mut host = host.borrow_mut();
    Ok(f(if session { &mut host.session_storage } else { &mut host.local_storage }))
}

/// The `n`th argument as a string, as WebIDL converts it.
fn text<H: Clone>(call: &mut dyn Call<H>, args: &[Value<H>], n: usize, name: &str) -> Result<String, Error> {
    match args.get(n) {
        Some(value) => call.to_string(value),
        None => Err(Error::Type(format!("{name}: an argument is missing"))),
    }
}

fn get_item<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let key = text(call, args, 0, "getItem")?;
    with(call, this, |s| s.items.iter().find(|(k, _)| *k == key).map_or(Value::Null, |(_, v)| Value::String(v.clone())))
}

/// Sets item `key` to `value`, or throws a `QuotaExceededError` when that's more than it holds.
fn store<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: String, value: String) -> Result<(), Error> {
    with(call, this, |s| {
        let old = s.items.iter().position(|(k, _)| *k == key);
        if let Some(old) = old
            && s.items.get(old).is_some_and(|(_, v)| *v == value)
        {
            return Ok(());
        }
        let freed = old.and_then(|i| s.items.get(i)).map_or(0, |(k, v)| k.encode_utf16().count() + v.encode_utf16().count());
        if s.size() - freed + key.encode_utf16().count() + value.encode_utf16().count() > QUOTA {
            return Err(Error::Dom("QuotaExceededError", "the storage is full".into()));
        }
        match old.and_then(|i| s.items.get_mut(i)) {
            Some(item) => item.1 = value,
            None => s.items.push((key, value)),
        }
        s.changed = true;
        Ok(())
    })?
}

fn set_item<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (key, value) = (text(call, args, 0, "setItem")?, text(call, args, 1, "setItem")?);
    store(call, this, key, value)?;
    Ok(Value::Undefined)
}

/// Removes item `key`; whether there was one.
fn remove<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, key: &str) -> Result<bool, Error> {
    with(call, this, |s| {
        let before = s.items.len();
        s.items.retain(|(k, _)| k != key);
        let removed = s.items.len() != before;
        s.changed |= removed;
        removed
    })
}

fn remove_item<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let key = text(call, args, 0, "removeItem")?;
    remove(call, this, &key)?;
    Ok(Value::Undefined)
}

fn clear<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, _args: &[Value<H>]) -> Result<Value<H>, Error> {
    with(call, this, |s| {
        s.changed |= !s.items.is_empty();
        s.items.clear();
    })?;
    Ok(Value::Undefined)
}

/// `key(n)`: the `n`th item's key, `null` past the last.
fn key<H: Clone>(call: &mut dyn Call<H>, this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let n = call.to_number(args.first().unwrap_or(&Value::Undefined))?;
    // WebIDL's `unsigned long`: whole, wrapped to 32 bits.
    let n = if n.is_finite() { (n.trunc().rem_euclid(4_294_967_296.0)) as usize } else { 0 };
    with(call, this, |s| s.items.get(n).map_or(Value::Null, |(k, _)| Value::String(k.clone())))
}

// The proxy's traps: reading a property the storage has gives that one, any other string an item,
// as browsers have it.

/// `Reflect[name](...args)`.
fn reflect<H: Clone>(call: &mut dyn Call<H>, name: &str, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let reflect = call.host()?.borrow().storages.as_ref().map(|c| c.reflect.clone()).ok_or_else(|| Error::Script("Storage is not set up".into()))?;
    method(call, &reflect, name, args)
}

/// The item key a property key is, unless the storage itself has the property.
fn item_key<H: Clone>(call: &mut dyn Call<H>, target: &Value<H>, key: &Value<H>) -> Result<Option<String>, Error> {
    match key {
        Value::String(key) if !crate::truthy(&reflect(call, "has", &[target.clone(), Value::String(key.clone())])?) => Ok(Some(key.clone())),
        _ => Ok(None),
    }
}

fn trap_get<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (target, key) = (args.first().cloned().unwrap_or(Value::Undefined), args.get(1).cloned().unwrap_or(Value::Undefined));
    match item_key(call, &target, &key)? {
        Some(key) => {
            let value = get_item(call, &target, &[Value::String(key)])?;
            Ok(if matches!(value, Value::Null) { Value::Undefined } else { value })
        }
        None => reflect(call, "get", args),
    }
}

/// Any string property set is an item, even one the storage has (`localStorage.key = '1'`), as
/// WebIDL's named property setter has it.
fn trap_set<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let target = args.first().cloned().unwrap_or(Value::Undefined);
    match args.get(1) {
        Some(Value::String(key)) => {
            let key = key.clone();
            let value = call.to_string(args.get(2).unwrap_or(&Value::Undefined))?;
            store(call, &target, key, value)?;
            Ok(Value::Bool(true))
        }
        // Symbols.
        _ => reflect(call, "set", args),
    }
}

fn trap_has<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (target, key) = (args.first().cloned().unwrap_or(Value::Undefined), args.get(1).cloned().unwrap_or(Value::Undefined));
    if let Value::String(name) = &key
        && with(call, &target, |s| s.items.iter().any(|(k, _)| k == name))?
    {
        return Ok(Value::Bool(true));
    }
    reflect(call, "has", args)
}

fn trap_delete<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (target, key) = (args.first().cloned().unwrap_or(Value::Undefined), args.get(1).cloned().unwrap_or(Value::Undefined));
    match item_key(call, &target, &key)? {
        Some(key) => {
            remove(call, &target, &key)?;
            Ok(Value::Bool(true))
        }
        None => reflect(call, "deleteProperty", args),
    }
}

/// The items' keys, then the storage's own enumerable keys, as `Object.keys()` lists them.
fn trap_keys<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let target = args.first().cloned().unwrap_or(Value::Undefined);
    let mut keys: Vec<Value<H>> = with(call, &target, |s| s.items.iter().map(|(k, _)| Value::String(k.clone())).collect())?;
    if let Value::Js(own) = reflect(call, "ownKeys", &[target])? {
        let count = match call.get(&own, "length")? {
            Value::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
            _ => 0,
        };
        for i in 0..count {
            let key = call.get(&own, &i.to_string())?;
            if !matches!(&key, Value::String(k) if k == SESSION || keys.iter().any(|known| matches!(known, Value::String(known) if known == k))) {
                keys.push(key);
            }
        }
    }
    call.new_array(&keys).map(Value::Js)
}

/// An item as an own property: enumerable, writable and configurable, as browsers list them.
fn trap_descriptor<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (target, key) = (args.first().cloned().unwrap_or(Value::Undefined), args.get(1).cloned().unwrap_or(Value::Undefined));
    if let Some(name) = item_key(call, &target, &key)?
        && let Value::String(value) = get_item(call, &target, &[Value::String(name)])?
    {
        let descriptor = call.new_object()?;
        call.set(&descriptor, "value", &Value::String(value))?;
        for flag in ["writable", "enumerable", "configurable"] {
            call.set(&descriptor, flag, &Value::Bool(true))?;
        }
        return Ok(Value::Js(descriptor));
    }
    reflect(call, "getOwnPropertyDescriptor", args)
}
