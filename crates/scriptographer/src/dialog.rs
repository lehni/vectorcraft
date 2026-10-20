//! What Scriptographer's `Dialog` (`js/src/dialog.js`) asks the app with: the host answers
//! ([`Runtime::set_ask`]) once the user did, in a dialog the app shows, and scripts on desktop wait
//! for it, as they did in Scriptographer. On the web, where scripts can't wait, the questions wait
//! in a queue the app takes one at a time ([`Runtime::take_question`]), and its answer settles
//! their Promises ([`Runtime::answer`], [`Runtime::picked`]).
//!
//! The host gets the question as JSON: `{type: 'alert' | 'confirm', title, message}`,
//! `{type: 'prompt', title, components: [{name, type, label, value, …}]}`, or on desktop `{type:
//! 'openFile' | 'saveFile' | 'chooseDirectory', path, filters}` for the pickers (`file_system.rs`).
//! It answers `true` (OK) or `false`, for a prompt the values by name, for a file dialog the
//! paths, or `null` (Cancel).

use serde_json::{Value as Json, json};

use crate::{Call, Engine, Error, Runtime, Value, method, object};

/// What answers a script's dialogs: it gets the question, and returns the answer once the user gave
/// it.
pub type Ask = Box<dyn FnMut(Json) -> Result<Json, Error>>;

/// What the dialogs use of the script's realm.
#[derive(Clone)]
pub(crate) struct Globals<H> {
    json: H,
    #[cfg(target_arch = "wasm32")]
    promise: H,
}

impl<E: Engine> Runtime<E> {
    /// Makes `ask` answer the script's dialogs. Without one, they throw.
    pub fn set_ask(&mut self, ask: Ask) {
        self.host.borrow_mut().ask = Some(ask);
    }
}

/// Keeps what the dialogs use of the realm.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    engine.with_call(|call| {
        let globals = Globals {
            json: object(call, global, "JSON")?,
            #[cfg(target_arch = "wasm32")]
            promise: object(call, global, "Promise")?,
        };
        call.host()?.borrow_mut().dialog_globals = Some(globals);
        Ok(())
    })
}

/// Asks the host `question`, and waits for the answer.
pub(crate) fn ask<H>(call: &mut dyn Call<H>, question: Json) -> Result<Json, Error> {
    let host = call.host()?;
    let taken = host.borrow_mut().ask.take();
    let mut ask = taken.ok_or_else(|| Error::Script("Dialogs need the desktop app".into()))?;
    let answer = ask(question);
    host.borrow_mut().ask = Some(ask);
    answer
}

/// `ask(question)`, which `js/src/dialog.js` asks with: the answer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn ask_native<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let question = to_json(call, &globals, args.first().unwrap_or(&Value::Undefined))?;
    let answer = ask(call, question)?;
    from_json(call, &globals, &answer)
}

/// `ask(question)` on the web, for the dialogs and the pickers: the question in the queue for the
/// app; a Promise of the answer, or of what was picked (a list of handles or files, or a name to
/// download as), or `null`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn ask_native<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let question = to_json(call, &globals, args.first().unwrap_or(&Value::Undefined))?;
    let resolvers = crate::handle(method(call, &globals.promise, "withResolvers", &[])?, "Promise.withResolvers()")?;
    let resolve = crate::handle(call.get(&resolvers, "resolve")?, "resolve")?;
    call.host()?.borrow_mut().asked.push_back(Asked { question, given: false, resolve });
    call.get(&resolvers, "promise")
}

/// A question the app answers later (the web): it, whether the app has it, and the script's
/// Promise for the answer.
#[cfg(target_arch = "wasm32")]
pub(crate) struct Asked<H> {
    question: Json,
    given: bool,
    resolve: H,
}

/// What the app's pickers gave a script, on the web.
#[cfg(target_arch = "wasm32")]
pub enum Picked {
    /// The browser's handles, or where it has none, the files picked.
    Items(Vec<wasm_bindgen::JsValue>),
    /// A file to save by downloading it, under this name.
    Download(String),
}

#[cfg(target_arch = "wasm32")]
impl<E: Engine> Runtime<E> {
    /// The question of the script's next dialog, once the app answered the one before: they show
    /// one at a time, in the order the script asked them.
    pub fn take_question(&mut self) -> Option<Json> {
        let mut host = self.host.borrow_mut();
        let next = host.asked.front_mut().filter(|asked| !asked.given)?;
        next.given = true;
        Some(next.question.clone())
    }

    /// The user's answer to the oldest question: a prompt's values by name, or `null` when they
    /// cancelled any dialog.
    pub fn answer(&mut self, answer: &Json) -> Result<(), Error> {
        let Some(Asked { resolve, .. }) = self.host.borrow_mut().asked.pop_front() else {
            return Ok(());
        };
        self.engine.with_call(|call| {
            let globals = globals(call)?;
            let value = from_json(call, &globals, answer)?;
            call.call(&resolve, &Value::Undefined, &[value]).map(|_| ())
        })
    }

    /// What the user picked in the oldest question's picker: a list of what the browser gave, or
    /// of the name to download as, or `null` for nothing.
    pub fn picked(&mut self, picked: Option<Picked>) -> Result<(), Error> {
        let Some(Asked { resolve, .. }) = self.host.borrow_mut().asked.pop_front() else {
            return Ok(());
        };
        let page = |value| E::page_value(value).ok_or_else(|| Error::Script("files picked in the page need the browser's engine".into()));
        let items = match picked {
            Some(Picked::Items(items)) => Some(items.into_iter().map(|item| page(item).map(Value::Js)).collect::<Result<Vec<_>, _>>()?),
            Some(Picked::Download(name)) => Some(vec![Value::String(name)]),
            None => None,
        };
        self.engine.with_call(|call| {
            let value = match &items {
                Some(items) => Value::Js(call.new_array(items)?),
                None => Value::Null,
            };
            call.call(&resolve, &Value::Undefined, &[value]).map(|_| ())
        })
    }
}

pub(crate) fn globals<H: Clone>(call: &mut dyn Call<H>) -> Result<Globals<H>, Error> {
    let host = call.host()?;
    let host = host.borrow();
    host.dialog_globals.clone().ok_or_else(|| Error::Script("Dialog is not set up".into()))
}

/// `value` as JSON, through `JSON.stringify()`: functions and what JSON can't hold drop out.
pub(crate) fn to_json<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, value: &Value<H>) -> Result<Json, Error> {
    Ok(match value {
        Value::Undefined | Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        // Whole numbers as integers, as `JSON.stringify()` writes them.
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => json!(*n as i64),
        Value::Number(n) => serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::String(s) => Json::String(s.clone()),
        Value::Js(_) => match method(call, &globals.json, "stringify", std::slice::from_ref(value))? {
            Value::String(text) => serde_json::from_str(&text).unwrap_or(Json::Null),
            _ => Json::Null,
        },
    })
}

/// `json` as a script value, through `JSON.parse()` for arrays and objects.
pub(crate) fn from_json<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, json: &Json) -> Result<Value<H>, Error> {
    Ok(match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(s.clone()),
        Json::Array(_) | Json::Object(_) => method(call, &globals.json, "parse", &[Value::String(json.to_string())])?,
    })
}
