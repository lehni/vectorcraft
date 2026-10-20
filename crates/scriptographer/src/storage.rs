//! `localStorage` and `sessionStorage` for scripts outside a browser. A script's `localStorage` is
//! its own, as a web page's is its origin's, and lasts: it's kept in a JSON file in the folder the
//! app gives ([`Runtime::set_script`](crate::Runtime::set_script)), named for the script's path,
//! with its `script.preferences`, written when either changed, at most once a second while the
//! script runs and when it ends. A script without a file or a folder, and `sessionStorage`, keep
//! theirs while the script runs. Each holds up to 5 MB, as browsers allow. Scripts reach them
//! through `js/src/storage.js`.

use std::path::{Path, PathBuf};

use crate::{Call, Engine, Error, HostFunction, Value};

/// The most a storage holds: its keys' and values' length in UTF-16, as browsers count.
const QUOTA: usize = 5 * 1024 * 1024;

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

    /// Sets item `key` to `value`, or fails with a `QuotaExceededError` when that's more than it
    /// holds.
    fn set(&mut self, key: String, value: String) -> Result<(), Error> {
        let old = self.items.iter().position(|(k, _)| *k == key);
        if let Some(old) = old
            && self.items.get(old).is_some_and(|(_, v)| *v == value)
        {
            return Ok(());
        }
        let freed = old.and_then(|i| self.items.get(i)).map_or(0, |(k, v)| k.encode_utf16().count() + v.encode_utf16().count());
        if self.size() - freed + key.encode_utf16().count() + value.encode_utf16().count() > QUOTA {
            return Err(Error::Dom("QuotaExceededError", "the storage is full".into()));
        }
        match old.and_then(|i| self.items.get_mut(i)) {
            Some(item) => item.1 = value,
            None => self.items.push((key, value)),
        }
        self.changed = true;
        Ok(())
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

/// The natives `js/src/storage.js` keeps the items with, each with whether it's `sessionStorage`
/// first: `getItem(session, key)`, `setItem(session, key, value)`, `removeItem(session, key)`,
/// `clear(session)` and `keys(session)`.
pub(crate) fn natives<E: Engine>(engine: &mut E) -> Result<Vec<(&'static str, E::Handle)>, Error> {
    let natives: [(&str, HostFunction<E::Handle>); 5] = [
        ("getItem", |call, _, args| {
            let key = text(call, args, 1)?;
            with(call, args, |s| s.items.iter().find(|(k, _)| *k == key).map_or(Value::Null, |(_, v)| Value::String(v.clone())))
        }),
        ("setItem", |call, _, args| {
            let (key, value) = (text(call, args, 1)?, text(call, args, 2)?);
            with(call, args, |s| s.set(key, value))??;
            Ok(Value::Undefined)
        }),
        ("removeItem", |call, _, args| {
            let key = text(call, args, 1)?;
            with(call, args, |s| {
                let before = s.items.len();
                s.items.retain(|(k, _)| *k != key);
                s.changed |= s.items.len() != before;
            })?;
            Ok(Value::Undefined)
        }),
        ("clear", |call, _, args| {
            with(call, args, |s| {
                s.changed |= !s.items.is_empty();
                s.items.clear();
            })?;
            Ok(Value::Undefined)
        }),
        ("keys", |call, _, args| {
            let keys: Vec<_> = with(call, args, |s| s.items.iter().map(|(k, _)| Value::String(k.clone())).collect())?;
            Ok(Value::Js(call.new_array(&keys)?))
        }),
    ];
    natives.into_iter().map(|(name, f)| Ok((name, engine.function(name, f)?))).collect()
}

/// `f` with the storage the first of `args` names: `sessionStorage` when it's `true`.
fn with<H, R>(call: &mut dyn Call<H>, args: &[Value<H>], f: impl FnOnce(&mut Storage) -> R) -> Result<R, Error> {
    let host = call.host()?;
    let mut host = host.borrow_mut();
    Ok(f(if matches!(args.first(), Some(Value::Bool(true))) { &mut host.session_storage } else { &mut host.local_storage }))
}

fn text<H>(call: &mut dyn Call<H>, args: &[Value<H>], n: usize) -> Result<String, Error> {
    call.to_string(args.get(n).unwrap_or(&Value::Undefined))
}
