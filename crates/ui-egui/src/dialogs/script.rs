//! A Scriptographer script's dialog: `Dialog.alert`, `confirm` and `prompt`, while the script waits
//! for the answer ([`crate::scripts`]), and the components palettes share with it. Its title,
//! message, labels and options are the script's, shown as they are.
//!
//! Fields: `script` (the script's id), `question` (as the script asked it: `type`, `title`,
//! `message`, `components`) and `values` (a prompt's values by name, as the user edits them).

use serde_json::{Map, Value, json};

use super::DialogSpec;
use crate::VectorcraftApp;
use crate::state::Dialog;
use crate::widgets;

pub const KIND: &str = "scriptDialog";

/// The font of the labels and fields.
const FONT: f32 = 12.5;
/// The space between the labels and the fields.
const GAP: f32 = 8.0;

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
    let layout = Layout::new(ui, components);
    for c in components {
        let Some(name) = c["name"].as_str() else {
            continue;
        };
        if let Some(Edit::Value(value)) = component(ui, &layout, c, values.get(name).unwrap_or(&Value::Null)) {
            values.insert(name.to_owned(), value);
        }
    }
    d.fields.insert("values".into(), Value::Object(values));
}

/// What the user did with a component.
pub(crate) enum Edit {
    Value(Value),
    /// Clicked a button.
    Click,
}

/// How a prompt's or palette's components lay out, as Scriptographer's: labels with a colon,
/// right-aligned in a column as wide as the widest one, then fields as wide as their `width`, or
/// their `length` in H's (16 for text, 8 for numbers).
pub(crate) struct Layout {
    /// The width of the label column.
    label: f32,
    /// The width of an H.
    h: f32,
    /// The widest field.
    field: f32,
}

impl Layout {
    pub(crate) fn new(ui: &egui::Ui, components: &[Value]) -> Self {
        let measure = |text: &str| ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(FONT), egui::Color32::WHITE).size().x;
        let shown = || components.iter().filter(|c| c["visible"].as_bool() != Some(false));
        let label = shown().filter_map(|c| label(c)).map(|l| measure(&format!("{l}:"))).fold(0.0, f32::max);
        let mut layout = Self { label, h: measure("H"), field: 0.0 };
        layout.field = shown().map(|c| layout.field_width(c)).fold(0.0, f32::max);
        layout
    }

    /// The width the components take.
    pub(crate) fn width(&self) -> f32 {
        if self.label > 0.0 { self.label + GAP + self.field } else { self.field }
    }

    /// The width of component `c`'s field.
    fn field_width(&self, c: &Value) -> f32 {
        if let Some(width) = c["width"].as_f64() {
            return width as f32;
        }
        let length = |default: u64| c["length"].as_u64().unwrap_or(default) as f32 * self.h + 8.0;
        match kind(c) {
            "number" => length(8),
            // The track, and a number field beside it.
            "slider" | "range" => length(12) + self.number_width(),
            "string" | "list" => length(16),
            _ => 0.0,
        }
    }

    /// The width of the number field beside a slider.
    fn number_width(&self) -> f32 {
        5.0 * self.h + 8.0
    }
}

/// Component `c`'s label, when it has one: rulers, texts and menu entries have none.
fn label(c: &Value) -> Option<&str> {
    c["label"].as_str().filter(|l| !l.is_empty() && !matches!(kind(c), "ruler" | "text") && !is_menu(c))
}

/// Whether `c` belongs in a palette's menu: a menu entry or separator.
pub(crate) fn is_menu(c: &Value) -> bool {
    matches!(kind(c), "menu-entry" | "menu-separator")
}

fn kind(c: &Value) -> &str {
    c["type"].as_str().unwrap_or("string")
}

/// Component `c` of a prompt or palette showing `value`, with its label: a row, a ruler or a line
/// of text. Hidden components take no room, disabled ones are greyed, and `toolTip` shows over
/// the row. A `fullSize` field takes the rest of the row.
pub(crate) fn component(ui: &mut egui::Ui, layout: &Layout, c: &Value, value: &Value) -> Option<Edit> {
    let name = c["name"].as_str().unwrap_or_default();
    if c["visible"].as_bool() == Some(false) || is_menu(c) {
        return None;
    }
    let enabled = c["enabled"].as_bool().unwrap_or(true);
    let full = c["fullSize"].as_bool() == Some(true);
    let tip = c["toolTip"].as_str().filter(|t| !t.is_empty());
    let width = layout.field_width(c);
    let response = ui.add_enabled_ui(enabled, |ui| match kind(c) {
        "ruler" => {
            widgets::divider(ui);
            None
        }
        "text" => {
            ui.label(value.as_str().or(c["label"].as_str()).unwrap_or_default());
            None
        }
        "button" => row(ui, layout, label(c), |ui| ui.button(text(value)).clicked().then_some(Edit::Click)),
        kind => row(ui, layout, label(c), |ui| {
            let width = if full { ui.available_width() } else { width };
            field(ui, layout, width, name, kind, c, value).map(Edit::Value)
        }),
    });
    if let Some(tip) = tip {
        response.response.on_hover_text(tip);
    }
    response.inner
}

/// A row: the component's label (the script's, as it is, with a colon), right-aligned in the label
/// column, then its field.
fn row(ui: &mut egui::Ui, layout: &Layout, label: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> Option<Edit>) -> Option<Edit> {
    let dim = crate::theme::Tokens::get(ui.ctx()).text_dim;
    ui.horizontal(|ui| {
        if layout.label > 0.0 {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(layout.label, 24.0), egui::Sense::hover());
            if let Some(label) = label {
                ui.painter().text(rect.right_center(), egui::Align2::RIGHT_CENTER, format!("{label}:"), egui::FontId::proportional(FONT), dim);
            }
            ui.add_space(GAP - ui.spacing().item_spacing.x);
        }
        add(ui)
    })
    .inner
}

/// The field of a component of type `kind` showing `value`. Returns the new value when it changed.
fn field(ui: &mut egui::Ui, layout: &Layout, width: f32, name: &str, kind: &str, c: &Value, value: &Value) -> Option<Value> {
    let id = ("script-dialog", name);
    let (min, max) = (c["min"].as_f64().or(c["range"][0].as_f64()), c["max"].as_f64().or(c["range"][1].as_f64()));
    match kind {
        "number" => {
            let decimals = c["fractionDigits"].as_u64().unwrap_or(3) as usize;
            let (lo, hi) = (min.unwrap_or(f64::MIN), max.unwrap_or(f64::MAX));
            let v = value.as_f64().unwrap_or(0.0).clamp(lo, hi);
            // Lengths are points shown in their unit, as Scriptographer's `units` had them.
            let (unit, suffix) = units(c["units"].as_str().unwrap_or_default());
            let edited = match (unit, c["steppers"].as_bool() == Some(true)) {
                (Some(unit), true) => widgets::spin_field(ui, id, Some(v), unit, width, c["increment"].as_f64().unwrap_or(1.0), lo, &[]),
                (Some(unit), false) => widgets::num_field(ui, id, Some(v), unit, width),
                (None, true) => widgets::spin_plain(ui, id, v, suffix, decimals, width, c["increment"].as_f64().unwrap_or(1.0), lo, &[]),
                (None, false) => widgets::range_field(ui, id, v, lo..=hi, suffix, decimals, width),
            };
            edited.map(|v| json!(v.clamp(lo, hi)))
        }
        "slider" | "range" => {
            // The track and a number field of fixed width beside it, together the field's width.
            let mut v = value.as_f64().unwrap_or(0.0);
            let (lo, hi) = (min.unwrap_or(0.0), max.unwrap_or(1.0).max(min.unwrap_or(0.0)));
            let decimals = c["fractionDigits"].as_u64().unwrap_or(2) as usize;
            let number = layout.number_width();
            ui.spacing_mut().slider_width = width - number - ui.spacing().item_spacing.x;
            let slid = widgets::slider(ui, egui::Slider::new(&mut v, lo..=hi).show_value(false)).changed();
            let typed = widgets::range_field(ui, (id, "number"), v, lo..=hi, "", decimals, number);
            typed.or(slid.then_some(v)).map(|v| json!(v))
        }
        "boolean" | "checkbox" => {
            let on = value.as_bool().unwrap_or(false);
            widgets::check_box(ui, name, on, true).then(|| json!(!on))
        }
        "list" => {
            let options: Vec<Value> = c["options"].as_array().cloned().unwrap_or_default();
            let names: Vec<String> = options.iter().map(text).collect();
            let shown: Vec<&str> = names.iter().map(String::as_str).collect();
            widgets::dropdown_names(ui, id, &text(value), &shown, width).and_then(|i| options.get(i).cloned())
        }
        "color" => {
            let hex = value.as_str().unwrap_or("#000000");
            let mut rgb = parse_hex(hex);
            ui.color_edit_button_srgb(&mut rgb).changed().then(|| json!(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])))
        }
        "string" => {
            let rows = if c["multiline"].as_bool() == Some(true) { c["rows"].as_u64().unwrap_or(4) as usize } else { 1 };
            widgets::text_field(ui, id, Some(&text(value)), width, rows).map(|s| json!(s))
        }
        // Fonts and menu entries: not yet.
        _ => None,
    }
}

/// Scriptographer's `units` of a number: a length unit its value shows in, or a suffix.
fn units(name: &str) -> (Option<vectorcraft_doc::Unit>, &'static str) {
    use vectorcraft_doc::Unit;
    match name {
        "point" => (Some(Unit::Points), ""),
        "pica" => (Some(Unit::Picas), ""),
        "inch" => (Some(Unit::Inches), ""),
        "millimeter" => (Some(Unit::Millimeters), ""),
        "centimeter" => (Some(Unit::Centimeters), ""),
        "percent" => (None, "%"),
        "degree" => (None, "°"),
        _ => (None, ""),
    }
}

/// A value as text: strings as they are, everything else as JSON.
pub(crate) fn text(value: &Value) -> String {
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
