//! Scriptographer's `Dialog`: `Dialog.alert([title,] message)`, `Dialog.confirm([title,]
//! message)`, `Dialog.prompt(title, components, values)`, `Dialog.chooseColor([color])`, and the
//! file dialogs `Dialog.openFile({ types, path })`, `Dialog.saveFile(…)` and
//! `Dialog.chooseDirectory({ path })`, Promises of a `LocalFile` or `null`. Each one asks the host
//! ([`Runtime::set_ask`]) and waits for the answer, which the user gives in a dialog the app shows:
//! scripts block on them as they did in Scriptographer.
//!
//! On the web, where scripts can't wait, `alert` and `confirm` are the browser's own, and `prompt`,
//! `chooseColor` and the file dialogs give Promises: their questions wait in a queue the app takes
//! one at a time ([`Runtime::take_question`]), and its answer settles them ([`Runtime::answer`],
//! [`Runtime::picked`]).
//!
//! The host gets the question as JSON: `{type: 'alert' | 'confirm', title, message}`, or
//! `{type: 'prompt', title, components: [{name, type, label, value, …}]}` with each component's
//! definition as the script wrote it, its type filled in where the script left it out, and its
//! value from `values` or the definition, or `{type: 'openFile' | 'saveFile' | 'chooseDirectory',
//! path, filters}` with the file or folder to start from. It answers `true` (OK) or `false`,
//! for a prompt the values by name, for a file dialog the path, or `null` (Cancel). The file
//! dialogs' `filters` come as `[label, [extensions]]`.
//! `chooseColor` is a prompt with one colour.

use serde_json::{Map, Value as Json, json};

use crate::{Call, Engine, Error, Runtime, Value, handle, method, object};

/// The title of a dialog the script doesn't name.
const TITLE: &str = "Scriptographer";

/// What answers a script's dialogs: it gets the question, and returns the answer once the user gave
/// it.
pub type Ask = Box<dyn FnMut(Json) -> Result<Json, Error>>;

/// What the dialog functions use of the script's realm.
#[derive(Clone)]
pub(crate) struct Globals<H> {
    json: H,
    object: H,
    color: H,
    /// The global object, whose pickers the file dialogs show (on the web, its `alert` and
    /// `confirm`).
    global: H,
    #[cfg(target_arch = "wasm32")]
    promise: H,
    /// What the file dialogs make of the pickers' answers: a `LocalFile`, or `null`.
    #[cfg(not(target_arch = "wasm32"))]
    picked: H,
    #[cfg(not(target_arch = "wasm32"))]
    cancelled: H,
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
    let files = [
        ("openFile", runtime.engine.function("openFile", |call, _, args| file(call, "openFile", args.first()))?),
        ("saveFile", runtime.engine.function("saveFile", |call, _, args| file(call, "saveFile", args.first()))?),
        ("chooseDirectory", runtime.engine.function("chooseDirectory", |call, _, args| file(call, "chooseDirectory", args.first()))?),
    ];
    let choose_color = runtime.engine.function("chooseColor", choose_color)?;
    #[cfg(not(target_arch = "wasm32"))]
    let (picked, cancelled) = (runtime.engine.function("picked", picked)?, runtime.engine.function("cancelled", cancelled)?);
    let paper = runtime.paper.clone();
    let globals = runtime.engine.with_call(|call| {
        let dialog = call.new_object()?;
        call.set(&dialog, "alert", &Value::Js(alert))?;
        call.set(&dialog, "confirm", &Value::Js(confirm))?;
        call.set(&dialog, "prompt", &Value::Js(prompt))?;
        for (name, function) in files {
            call.set(&dialog, name, &Value::Js(function))?;
        }
        call.set(&dialog, "chooseColor", &Value::Js(choose_color))?;
        call.set(global, "Dialog", &Value::Js(dialog))?;
        Ok(Globals {
            json: object(call, global, "JSON")?,
            object: object(call, global, "Object")?,
            color: object(call, &paper, "Color")?,
            global: global.clone(),
            #[cfg(target_arch = "wasm32")]
            promise: object(call, global, "Promise")?,
            #[cfg(not(target_arch = "wasm32"))]
            picked,
            #[cfg(not(target_arch = "wasm32"))]
            cancelled,
        })
    })?;
    runtime.host.borrow_mut().dialog_globals = Some(globals);
    Ok(())
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

/// The title and message of `alert` and `confirm`: `(message)` or `(title, message)`.
fn texts<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<(String, String), Error> {
    match args {
        [title, message, ..] => Ok((call.to_string(title)?, call.to_string(message)?)),
        [message] => Ok((TITLE.into(), call.to_string(message)?)),
        [] => Ok((TITLE.into(), String::new())),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn alert<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (title, message) = texts(call, args)?;
    ask(call, json!({ "type": "alert", "title": title, "message": message }))?;
    Ok(Value::Undefined)
}

#[cfg(not(target_arch = "wasm32"))]
fn confirm<H>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let (title, message) = texts(call, args)?;
    let answer = ask(call, json!({ "type": "confirm", "title": title, "message": message }))?;
    Ok(Value::Bool(answer == Json::Bool(true)))
}

/// The browser's `alert()`, which shows no title of the script's: a title the script gave heads
/// the message.
#[cfg(target_arch = "wasm32")]
fn alert<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = browser_text(call, args)?;
    let window = globals(call)?.global;
    method(call, &window, "alert", &[Value::String(text)])?;
    Ok(Value::Undefined)
}

/// The browser's `confirm()`.
#[cfg(target_arch = "wasm32")]
fn confirm<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let text = browser_text(call, args)?;
    let window = globals(call)?.global;
    let answer = method(call, &window, "confirm", &[Value::String(text)])?;
    Ok(Value::Bool(crate::truthy(&answer)))
}

#[cfg(target_arch = "wasm32")]
fn browser_text<H>(call: &mut dyn Call<H>, args: &[Value<H>]) -> Result<String, Error> {
    let (title, message) = texts(call, args)?;
    Ok(if title == TITLE { message } else { format!("{title}\n\n{message}") })
}

/// `Dialog.openFile(options)`, `saveFile(options)` and `chooseDirectory(options)`: a Promise of
/// the `LocalFile` the user picked, or `null` when they cancelled, from the realm's
/// `showOpenFilePicker()`, `showSaveFilePicker()` or `showDirectoryPicker()`. `options` are its
/// file `types` (not for `chooseDirectory`) and the `path` it starts from (a path or a
/// `LocalFile`; for `saveFile`, the file it suggests).
#[cfg(not(target_arch = "wasm32"))]
fn file<H: Clone>(call: &mut dyn Call<H>, kind: &str, options: Option<&Value<H>>) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let (types, path) = match options {
        Some(Value::Js(options)) => (call.get(options, "types")?, call.get(options, "path")?),
        _ => (Value::Undefined, Value::Undefined),
    };
    let picker_options = call.new_object()?;
    if let Value::Js(types) = types
        && kind != "chooseDirectory"
    {
        // The types with extensions; one without shows every file, as the pickers' own does.
        let mut accepted = Vec::new();
        let mut all = false;
        for kind in file_types(call, &types)? {
            let (label, extensions) = (kind[0].as_str().unwrap_or_default(), kind[1].as_array().cloned().unwrap_or_default());
            if extensions.is_empty() {
                all = true;
                continue;
            }
            let dotted: Vec<Json> = extensions.iter().filter_map(|e| e.as_str()).map(|e| Json::String(format!(".{e}"))).collect();
            accepted.push(json!({ "description": label, "accept": { "application/octet-stream": dotted } }));
        }
        let accepted = from_json(call, &globals, &Json::Array(accepted))?;
        call.set(&picker_options, "types", &accepted)?;
        call.set(&picker_options, "excludeAcceptAllOption", &Value::Bool(!all))?;
    }
    if !matches!(path, Value::Undefined | Value::Null) {
        let path = std::path::PathBuf::from(call.to_string(&path)?);
        // A directory to start in, or a file: its directory, and for saving its name.
        let (directory, name) =
            if path.is_dir() { (Some(path.as_path()), None) } else { (path.parent().filter(|p| !p.as_os_str().is_empty()), path.file_name()) };
        if let Some(directory) = directory {
            let start = crate::file_system::new_handle(call, directory, true)?;
            call.set(&picker_options, "startIn", &start)?;
        }
        if let (Some(name), "saveFile") = (name, kind) {
            call.set(&picker_options, "suggestedName", &Value::String(name.to_string_lossy().into_owned()))?;
        }
    }
    let picker = match kind {
        "openFile" => "showOpenFilePicker",
        "saveFile" => "showSaveFilePicker",
        _ => "showDirectoryPicker",
    };
    let Value::Js(promise) = method(call, &globals.global, picker, &[Value::Js(picker_options)])? else {
        return Err(Error::Script(format!("{picker} gave no Promise")));
    };
    method(call, &promise, "then", &[Value::Js(globals.picked), Value::Js(globals.cancelled)])
}

/// The file dialogs on the web: a question for the app, `{type, name, filters}` with the name to
/// suggest for saving, which [`Runtime::picked`] answers with the `LocalFile`, or `null`.
/// `chooseDirectory` rejects with a `NotSupportedError` for now.
#[cfg(target_arch = "wasm32")]
fn file<H: Clone>(call: &mut dyn Call<H>, kind: &str, options: Option<&Value<H>>) -> Result<Value<H>, Error> {
    // A `LocalFile` on the web is a file, not yet a directory.
    if kind == "chooseDirectory" {
        let promise = globals(call)?.promise;
        let reason = crate::exception::value(call, &Error::Dom("NotSupportedError", "Dialog.chooseDirectory() isn't on the web yet".into()))?;
        return method(call, &promise, "reject", &[reason]);
    }
    let (types, path) = match options {
        Some(Value::Js(options)) => (call.get(options, "types")?, call.get(options, "path")?),
        _ => (Value::Undefined, Value::Undefined),
    };
    let filters = match types {
        Value::Js(types) if kind != "chooseDirectory" => file_types(call, &types)?,
        _ => Vec::new(),
    };
    // The web has no paths: only a file's name, to suggest.
    let name = match path {
        Value::Undefined | Value::Null => Json::Null,
        path => {
            let path = call.to_string(&path)?;
            Json::String(path.rsplit(['/', '\\']).next().unwrap_or_default().to_owned())
        }
    };
    ask_later(call, json!({ "type": kind, "name": name, "filters": filters }), Finish::File)
}

/// A question the app answers later (the web): it, whether the app has it, the script's Promise
/// for the answer, and what makes the answer the script's value.
#[cfg(target_arch = "wasm32")]
pub(crate) struct Asked<H> {
    question: Json,
    given: bool,
    resolve: H,
    finish: Finish<H>,
}

/// What makes an answer the script's value: a prompt's values (put into `values` when given),
/// `chooseColor`'s colour, a file dialog's `LocalFile`.
#[cfg(target_arch = "wasm32")]
pub(crate) enum Finish<H> {
    Prompt { list: Vec<Json>, values: Option<H> },
    Color { list: Vec<Json> },
    File,
}

/// Puts `question` in the queue for the app; a Promise of the answer, as `finish` makes it.
#[cfg(target_arch = "wasm32")]
fn ask_later<H: Clone>(call: &mut dyn Call<H>, question: Json, finish: Finish<H>) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let resolvers = handle(method(call, &globals.promise, "withResolvers", &[])?, "Promise.withResolvers()")?;
    let resolve = handle(call.get(&resolvers, "resolve")?, "resolve")?;
    call.host()?.borrow_mut().asked.push_back(Asked { question, given: false, resolve, finish });
    call.get(&resolvers, "promise")
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
        let Some(Asked { resolve, finish, .. }) = self.host.borrow_mut().asked.pop_front() else {
            return Ok(());
        };
        self.engine.with_call(|call| {
            let value = match finish {
                Finish::Prompt { list, values } => prompt_values(call, &list, values, answer)?,
                Finish::Color { list } => match prompt_values(call, &list, None, answer)? {
                    Value::Js(values) => call.get(&values, "color")?,
                    other => other,
                },
                Finish::File => Value::Null,
            };
            call.call(&resolve, &Value::Undefined, &[value]).map(|_| ())
        })
    }

    /// What the user picked in the oldest question's file dialog: its `LocalFile`, or `null` for
    /// nothing.
    pub fn picked(&mut self, picked: Option<crate::web_file::Picked>) -> Result<(), Error> {
        let Some(Asked { resolve, .. }) = self.host.borrow_mut().asked.pop_front() else {
            return Ok(());
        };
        self.engine.with_call(|call| {
            let value = match picked {
                Some(picked) => crate::web_file::local_file::<E>(call, picked)?,
                None => Value::Null,
            };
            call.call(&resolve, &Value::Undefined, &[value]).map(|_| ())
        })
    }
}

/// The `LocalFile` of the handle a picker gave, or of the first of the handles.
#[cfg(not(target_arch = "wasm32"))]
fn picked<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let Some(Value::Js(picked)) = args.first() else {
        return Ok(Value::Null);
    };
    let handle = match crate::file_system::path_of(call, picked)? {
        Some(_) => Value::Js(picked.clone()),
        None => call.get(picked, "0")?,
    };
    crate::local_file::from_handle(call, &handle)
}

/// `null` for a picker the user cancelled; any other failure stays one.
#[cfg(not(target_arch = "wasm32"))]
fn cancelled<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let reason = args.first().cloned().unwrap_or(Value::Undefined);
    if let Value::Js(error) = &reason
        && matches!(call.get(error, "name")?, Value::String(name) if name == "AbortError")
    {
        return Ok(Value::Null);
    }
    crate::local_file::rejected(call, reason)
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
    #[cfg(target_arch = "wasm32")]
    {
        let (question, list) = prompt_question(call, TITLE.into(), &components, None)?;
        ask_later(call, question, Finish::Color { list })
    }
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(target_arch = "wasm32")]
    {
        let (question, list) = prompt_question(call, title, components, values.as_ref())?;
        ask_later(call, question, Finish::Prompt { list, values })
    }
    #[cfg(not(target_arch = "wasm32"))]
    ask_prompt(call, title, components, values)
}

/// Asks the prompt titled `title` with `components`, its values from `values` when given.
#[cfg(not(target_arch = "wasm32"))]
fn ask_prompt<H: Clone>(call: &mut dyn Call<H>, title: String, components: &H, values: Option<H>) -> Result<Value<H>, Error> {
    let (question, list) = prompt_question(call, title, components, values.as_ref())?;
    let answer = ask(call, question)?;
    prompt_values(call, &list, values, &answer)
}

/// The question of the prompt titled `title` with `components`, and the components as the host
/// gets them.
fn prompt_question<H: Clone>(call: &mut dyn Call<H>, title: String, components: &H, values: Option<&H>) -> Result<(Json, Vec<Json>), Error> {
    let globals = globals(call)?;
    let list = describe(call, &globals, components, values)?;
    Ok((json!({ "type": "prompt", "title": title, "components": list }), list))
}

/// The values of the user's `answer` to a prompt of the components `list`, put into `values` when
/// given, or `null` when they cancelled.
fn prompt_values<H: Clone>(call: &mut dyn Call<H>, list: &[Json], values: Option<H>, answer: &Json) -> Result<Value<H>, Error> {
    let globals = globals(call)?;
    let Json::Object(answer) = answer else {
        return Ok(Value::Null);
    };
    let result = match values {
        Some(values) => values,
        None => call.new_object()?,
    };
    for component in list {
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
    host.dialog_globals.clone().ok_or_else(|| Error::Script("Dialog is not set up".into()))
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
