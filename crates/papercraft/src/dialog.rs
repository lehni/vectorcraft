//! Scriptographer's `Dialog`: `Dialog.alert([title,] message)`, `Dialog.confirm([title,]
//! message)`, `Dialog.prompt(title, components, values)`, the file dialogs
//! `Dialog.fileOpen([message, filters, selectedFile])`, `Dialog.fileSave(…)` and
//! `Dialog.chooseDirectory([message, selectedDirectory])`, and `Dialog.chooseColor([color])`. Each
//! one asks the host ([`Runtime::set_ask`]) and waits for the answer, which the user gives in a
//! dialog the app shows: scripts block on them as they did in Scriptographer.
//!
//! The host gets the question as JSON: `{type: 'alert' | 'confirm', title, message}`, or
//! `{type: 'prompt', title, components: [{name, type, label, value, …}]}` with each component's
//! definition as the script wrote it, its type filled in where the script left it out, and its
//! value from `values` or the definition, or `{type: 'fileOpen' | 'fileSave' | 'chooseDirectory',
//! path, filters}` with the file or folder to start from. It answers `true` (OK) or `false`, for a prompt
//! the values by name, for a file dialog the path, or `null` (Cancel). The file dialogs return
//! a `File`; their `filters` come as `[label, [extensions]]`.
//! `chooseColor` is a prompt with one colour.

use serde_json::{Map, Value as Json, json};

use crate::{Call, Engine, Error, Runtime, Value, handle, method, object};

/// The title of a dialog the script doesn't name.
const TITLE: &str = "PaperCraft";

/// What answers a script's dialogs: it gets the question, and returns the answer once the user gave
/// it.
pub type Ask = Box<dyn FnMut(Json) -> Result<Json, Error>>;

/// What the dialog functions use of the script's realm.
pub(crate) struct Globals<H> {
    json: H,
    object: H,
    color: H,
}

impl<E: Engine> Runtime<E> {
    /// Makes `ask` answer the script's dialogs. Without one, they throw.
    pub fn set_ask(&mut self, ask: Ask) {
        self.host.borrow_mut().ask = Some(ask);
    }
}

/// Sets `Dialog` on `global`, with `paper` loaded.
pub(crate) fn install<E: Engine>(runtime: &mut Runtime<E>, global: &E::Handle) -> Result<(), Error> {
    let alert = runtime.engine.function("alert", alert)?;
    let confirm = runtime.engine.function("confirm", confirm)?;
    let prompt = runtime.engine.function("prompt", prompt)?;
    let file_open = runtime.engine.function("fileOpen", |call, _, args| file(call, "fileOpen", args.get(1), args.get(2)))?;
    let file_save = runtime.engine.function("fileSave", |call, _, args| file(call, "fileSave", args.get(1), args.get(2)))?;
    let choose_directory = runtime.engine.function("chooseDirectory", |call, _, args| file(call, "chooseDirectory", None, args.get(1)))?;
    let choose_color = runtime.engine.function("chooseColor", choose_color)?;
    let paper = runtime.paper.clone();
    let globals = runtime.engine.with_call(|call| {
        let dialog = call.new_object()?;
        call.set(&dialog, "alert", &Value::Js(alert))?;
        call.set(&dialog, "confirm", &Value::Js(confirm))?;
        call.set(&dialog, "prompt", &Value::Js(prompt))?;
        call.set(&dialog, "fileOpen", &Value::Js(file_open))?;
        call.set(&dialog, "fileSave", &Value::Js(file_save))?;
        call.set(&dialog, "chooseDirectory", &Value::Js(choose_directory))?;
        call.set(&dialog, "chooseColor", &Value::Js(choose_color))?;
        call.set(global, "Dialog", &Value::Js(dialog))?;
        Ok(Globals { json: object(call, global, "JSON")?, object: object(call, global, "Object")?, color: object(call, &paper, "Color")? })
    })?;
    runtime.host.borrow_mut().dialog_globals = Some(globals);
    Ok(())
}

/// Asks the host `question`, and waits for the answer.
fn ask<H>(call: &mut dyn Call<H>, question: Json) -> Result<Json, Error> {
    let host = call.host()?;
    let taken = host.borrow_mut().ask.take();
    let mut ask = taken.ok_or_else(|| Error::Script("Dialogs need the desktop app".into()))?;
    let answer = ask(question);
    host.borrow_mut().ask = Some(ask);
    answer
}

/// The title and message of `alert` and `confirm`: `(message)` or `(title, message)`.
fn texts<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<(String, String), Error> {
    match args {
        [title, message, ..] => Ok((call.to_string(title)?, call.to_string(message)?)),
        [message] => Ok((TITLE.into(), call.to_string(message)?)),
        [] => Ok((TITLE.into(), String::new())),
    }
}

fn alert<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (title, message) = texts(call, args)?;
    ask(call, json!({ "type": "alert", "title": title, "message": message }))?;
    Ok(Value::Undefined)
}

fn confirm<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (title, message) = texts(call, args)?;
    let answer = ask(call, json!({ "type": "confirm", "title": title, "message": message }))?;
    Ok(Value::Bool(answer == Json::Bool(true)))
}

/// A file dialog of `kind`, starting from `path` when the script gives one (a path or a `File`):
/// the `File` picked, or `null` when the user cancelled.
fn file<H: Clone>(call: &mut dyn Call<H>, kind: &str, filters: Option<&Value<H>>, path: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let path = match path {
        None | Some(Value::Undefined | Value::Null) => Json::Null,
        Some(path) => Json::String(call.to_string(path)?),
    };
    let filters = match filters {
        Some(Value::Js(filters)) => file_types(call, filters)?,
        _ => Vec::new(),
    };
    match ask(call, json!({ "type": kind, "path": path, "filters": filters }))? {
        Json::String(path) => crate::file::new_file(call, std::path::Path::new(&path)),
        _ => Ok(Value::Null),
    }
}

/// A file dialog's file types as `[label, [extensions]]`, the first the default, from `filters`:
/// labels and their extensions in the script's order (`{ Text: ['txt', 'text'], SVG: 'svg',
/// 'All Files': '*' }`), or Scriptographer's pairs of a label and its patterns (`['Text (*.txt)',
/// '*.txt;*.text', 'All Files', '*.*']`). A type without extensions (`'*'`) shows every file.
fn file_types<H: Clone>(call: &mut dyn Call<H>, filters: &H) -> Result<Vec<Json>, Error> {
    let globals = globals(call)?;
    let pairs: Vec<(String, Json)> = match to_json(call, &globals, &Value::Js(filters.clone()))? {
        Json::Array(list) => {
            let strings: Vec<String> = list.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect();
            // A pattern without its label labels itself.
            strings
                .chunks(2)
                .filter_map(|pair| match pair {
                    [label, patterns] => Some((label.clone(), Json::String(patterns.clone()))),
                    [patterns] => Some((patterns.clone(), Json::String(patterns.clone()))),
                    _ => None,
                })
                .collect()
        }
        _ => {
            let mut pairs = Vec::new();
            for label in keys(call, &globals, filters)? {
                let extensions = call.get(filters, &label)?;
                pairs.push((label, to_json(call, &globals, &extensions)?));
            }
            pairs
        }
    };
    Ok(pairs
        .into_iter()
        .map(|(label, extensions)| {
            let patterns: Vec<String> = match extensions {
                Json::Array(list) => list.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect(),
                Json::String(patterns) => patterns.split(';').map(str::to_owned).collect(),
                _ => Vec::new(),
            };
            let extensions: Vec<&str> =
                patterns.iter().map(|p| p.trim().trim_start_matches('*').trim_start_matches('.')).filter(|e| !e.is_empty() && *e != "*").collect();
            json!([label, extensions])
        })
        .collect())
}

/// `Dialog.chooseColor([color])`: a prompt with one colour, the colour picked or `null`.
fn choose_color<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let component = call.new_object()?;
    call.set(&component, "type", &Value::String("color".into()))?;
    if let Some(color) = args.first().filter(|c| !matches!(c, Value::Undefined | Value::Null)) {
        call.set(&component, "value", color)?;
    }
    let components = call.new_object()?;
    call.set(&components, "color", &Value::Js(component))?;
    match ask_prompt(call, TITLE.into(), &components, None)? {
        Value::Js(values) => call.get(&values, "color"),
        other => Ok(other),
    }
}

/// `Dialog.prompt(title, components, values)`: the values by name, put into `values` when given,
/// or `null` when the user cancelled.
fn prompt<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let title = match args.first() {
        Some(title) => call.to_string(title)?,
        None => TITLE.into(),
    };
    let Some(Value::Js(components)) = args.get(1) else {
        return Err(Error::Script("Dialog.prompt: components must be an object".into()));
    };
    let values = match args.get(2) {
        Some(Value::Js(values)) => Some(values.clone()),
        _ => None,
    };
    ask_prompt(call, title, components, values)
}

/// Asks the prompt titled `title` with `components`, its values from `values` when given.
fn ask_prompt<H: Clone>(call: &mut dyn Call<H>, title: String, components: &H, values: Option<H>) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let list = describe(call, &globals, components, values.as_ref())?;
    let answer = ask(call, json!({ "type": "prompt", "title": title, "components": list }))?;
    let Json::Object(answer) = answer else {
        return Ok(Value::Null);
    };
    let result = match values {
        Some(values) => values,
        None => call.new_object()?,
    };
    for component in &list {
        let name = component["name"].as_str().unwrap_or_default();
        if let Some(value) = answer.get(name) {
            let value = from_component(call, &globals, component, value)?;
            call.set(&result, name, &value)?;
        }
    }
    Ok(Value::Js(result))
}

/// The components as the host gets them: each definition as JSON with its `name`, its `type`
/// filled in where the script left it out, and its `value` from `values` or the definition, a
/// colour as CSS.
pub(crate) fn describe<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, components: &H, values: Option<&H>) -> Result<Vec<Json>, Error> {
    let mut list = Vec::new();
    for name in keys(call, globals, components)? {
        let definition = object(call, components, &name)?;
        let mut component = match to_json(call, globals, &Value::Js(definition.clone()))? {
            Json::Object(component) => component,
            _ => Map::new(),
        };
        let value = match values {
            Some(values) => match call.get(values, &name)? {
                Value::Undefined => call.get(&definition, "value")?,
                value => value,
            },
            None => call.get(&definition, "value")?,
        };
        let value = match &value {
            // A colour, as CSS: the dialog shows it in a colour field.
            Value::Js(color) if matches!(call.get(color, "toCSS")?, Value::Js(_)) => method(call, color, "toCSS", &[Value::Bool(true)])?,
            _ => value,
        };
        component.insert("value".into(), to_json(call, globals, &value)?);
        if !component.contains_key("type") {
            let kind = kind(&component, call.get(&definition, "onClick")?, call.get(&definition, "onSelect")?);
            component.insert("type".into(), json!(kind));
        }
        // A number with units has steppers unless it says otherwise, as in Scriptographer.
        if component["type"] == "number"
            && component.get("units").is_some_and(|u| u.is_string() && u != "none")
            && !component.contains_key("steppers")
        {
            component.insert("steppers".into(), json!(true));
        }
        component.insert("name".into(), json!(name));
        list.push(Json::Object(component));
    }
    Ok(list)
}

/// The script value of `component`'s value `json` from the host: a colour from CSS.
pub(crate) fn from_component<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, component: &Json, json: &Json) -> Result<Value<H>, Error> {
    match (component["type"].as_str(), json) {
        (Some("color"), Json::String(css)) => call.construct(&globals.color, &[Value::String(css.clone())]),
        _ => from_json(call, globals, json),
    }
}

/// A component's type where its definition leaves it out, as Scriptographer read it from the
/// definition: a list with options, a button with `onClick`, a menu entry with `onSelect`, else
/// by its value.
fn kind<H>(component: &Map<String, Json>, on_click: Value<H>, on_select: Value<H>) -> &'static str {
    if component.contains_key("options") {
        return "list";
    }
    if matches!(on_click, Value::Js(_)) {
        return "button";
    }
    if matches!(on_select, Value::Js(_)) {
        return "menu-entry";
    }
    match component.get("value") {
        Some(Json::Number(_)) => "number",
        Some(Json::Bool(_)) => "boolean",
        _ => "string",
    }
}

pub(crate) fn globals<H: Clone>(call: &mut dyn Call<H>) -> Result<Globals<H>, Error> {
    let host = call.host()?;
    let host = host.borrow();
    let globals = host.dialog_globals.as_ref().ok_or_else(|| Error::Script("Dialog is not set up".into()))?;
    Ok(Globals { json: globals.json.clone(), object: globals.object.clone(), color: globals.color.clone() })
}

/// Whether `a` and `b` are the same value, as `Object.is(a, b)`.
pub(crate) fn same<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, a: &Value<H>, b: &Value<H>) -> Result<bool, Error> {
    Ok(matches!(method(call, &globals.object, "is", &[a.clone(), b.clone()])?, Value::Bool(true)))
}

/// `Object.keys(object)`, in their order.
fn keys<H: Clone>(call: &mut dyn Call<H>, globals: &Globals<H>, object: &H) -> Result<Vec<String>, Error> {
    let keys = handle(method(call, &globals.object, "keys", &[Value::Js(object.clone())])?, "Object.keys()")?;
    let count = match call.get(&keys, "length")? {
        Value::Number(count) => count as usize,
        _ => 0,
    };
    (0..count).map(|i| call.get(&keys, &i.to_string()).and_then(|key| call.to_string(&key))).collect()
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
