//! A PaperCraft script's dialog: `Dialog.alert`, `confirm` and `prompt`, while the script waits
//! for the answer ([`crate::scripts`]). Its title, message, labels and options are the script's,
//! shown as they are.
//!
//! Fields: `script` (the script's id), `question` (as the script asked it: `type`, `title`,
//! `message`, `components`) and `values` (a prompt's values by name, as the user edits them).

use serde_json::{Map, Value, json};

use super::DialogSpec;
use crate::VectorcraftApp;
use crate::state::Dialog;
use crate::widgets;

pub const KIND: &str = "scriptDialog";

/// Width of a prompt's labels.
const LABEL_W: f32 = 110.0;
/// Width of a prompt's fields.
const FIELD_W: f32 = 180.0;

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |d| d.fields["question"]["title"].as_str().unwrap_or_default().to_owned(),
    body: |_, ui, d| {
        body(ui, d);
        false
    },
    confirm: |app, d| {
        let answer = match d.fields["question"]["type"].as_str() {
            Some("prompt") => d.fields.get("values").cloned().unwrap_or_else(|| json!({})),
            _ => json!(true),
        };
        let script = d.fields.get("script").and_then(Value::as_u64).unwrap_or_default();
        crate::scripts::answer(app, script, answer);
        app.ui.dialog = None;
        Ok(Value::Null)
    },
    max_width: Some(460.0),
    ..DialogSpec::FORM
};

/// The dialog for `question`, which the script with id `script` waits on.
pub(crate) fn open(app: &mut VectorcraftApp, script: u64, question: &Value) {
    let values: Map<String, Value> =
        question["components"].as_array().into_iter().flatten().filter_map(|c| Some((c["name"].as_str()?.to_owned(), c["value"].clone()))).collect();
    app.ui.dialog = Some(Dialog::new(KIND, json!({ "script": script, "question": question, "values": values })));
}

/// What the script gets when the dialog closes without OK: `false` from `confirm`, `null` from
/// `prompt` (and `alert`, which returns nothing).
pub(crate) fn cancelled(question: &Value) -> Value {
    match question["type"].as_str() {
        Some("confirm") => json!(false),
        _ => Value::Null,
    }
}

fn body(ui: &mut egui::Ui, d: &mut Dialog) {
    let question = d.fields["question"].clone();
    if let Some(message) = question["message"].as_str() {
        ui.label(message);
    }
    let Some(components) = question["components"].as_array() else {
        return;
    };
    let mut values = d.fields.get("values").and_then(Value::as_object).cloned().unwrap_or_default();
    for c in components {
        let Some(name) = c["name"].as_str() else {
            continue;
        };
        let label = c["label"].as_str().unwrap_or(name);
        let value = values.get(name).cloned().unwrap_or(Value::Null);
        let enabled = c["enabled"].as_bool().unwrap_or(true);
        if c["visible"].as_bool() == Some(false) {
            continue;
        }
        let changed = ui
            .add_enabled_ui(enabled, |ui| match c["type"].as_str().unwrap_or("string") {
                "ruler" | "menu-separator" => {
                    widgets::divider(ui);
                    None
                }
                "text" => {
                    ui.label(value.as_str().map_or_else(|| label.to_owned(), str::to_owned));
                    None
                }
                kind => row(ui, label, |ui| field(ui, name, kind, c, &value)),
            })
            .inner;
        if let Some(value) = changed {
            values.insert(name.to_owned(), value);
        }
    }
    d.fields.insert("values".into(), Value::Object(values));
}

/// A row: the component's label (the script's, as it is), then its field.
fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> Option<Value>) -> Option<Value> {
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W, 24.0], egui::Label::new(label).truncate());
        add(ui)
    })
    .inner
}

/// The field of a component of type `kind` showing `value`. Returns the new value when it changed.
fn field(ui: &mut egui::Ui, name: &str, kind: &str, c: &Value, value: &Value) -> Option<Value> {
    let id = ("script-dialog", name);
    let (min, max) = (c["min"].as_f64().or(c["range"][0].as_f64()), c["max"].as_f64().or(c["range"][1].as_f64()));
    match kind {
        "number" => {
            let decimals = c["fractionDigits"].as_u64().unwrap_or(3) as usize;
            let range = min.unwrap_or(f64::MIN)..=max.unwrap_or(f64::MAX);
            widgets::range_field(ui, id, value.as_f64().unwrap_or(0.0), range, "", decimals, FIELD_W).map(|v| json!(v))
        }
        "slider" | "range" => {
            let mut v = value.as_f64().unwrap_or(0.0);
            let (lo, hi) = (min.unwrap_or(0.0), max.unwrap_or(1.0).max(min.unwrap_or(0.0)));
            widgets::slider(ui, egui::Slider::new(&mut v, lo..=hi)).changed().then(|| json!(v))
        }
        "boolean" | "checkbox" => {
            let on = value.as_bool().unwrap_or(false);
            widgets::check_box(ui, name, on, true).then(|| json!(!on))
        }
        "list" => {
            let options: Vec<Value> = c["options"].as_array().cloned().unwrap_or_default();
            let names: Vec<String> = options.iter().map(text).collect();
            let shown: Vec<&str> = names.iter().map(String::as_str).collect();
            widgets::dropdown_names(ui, id, &text(value), &shown, FIELD_W).and_then(|i| options.get(i).cloned())
        }
        "color" => {
            let hex = value.as_str().unwrap_or("#000000");
            let mut rgb = parse_hex(hex);
            ui.color_edit_button_srgb(&mut rgb).changed().then(|| json!(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])))
        }
        "string" => {
            let rows = if c["multiline"].as_bool() == Some(true) { c["rows"].as_u64().unwrap_or(4) as usize } else { 1 };
            widgets::text_field(ui, id, Some(&text(value)), FIELD_W, rows).map(|s| json!(s))
        }
        // Buttons, fonts and menu entries: palettes have them, dialogs don't.
        _ => None,
    }
}

/// A value as text: strings as they are, everything else as JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// `#rrggbb` as bytes; black when it isn't one.
fn parse_hex(hex: &str) -> [u8; 3] {
    let channel = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0);
    if hex.len() == 7 && hex.starts_with('#') { [channel(1), channel(3), channel(5)] } else { [0, 0, 0] }
}
