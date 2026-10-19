//! Scriptographer's `Palette`: `new Palette(title, components, values)` is a panel that stays open
//! while the script runs, with the same components as `Dialog.prompt`. What the user changes goes
//! into `values` and calls the component's `onChange(value)`, then the palette's
//! `onChange(component)`; a button calls its `onClick()`. What the script changes in `values` or
//! in the components shows in the panel.
//!
//! The host shows the palettes [`Runtime::take_palettes`] reports, and sends what the user does
//! with [`Runtime::palette_event`].

use serde_json::{Value as Json, json};

use crate::dialog::{self, describe, from_component};
use crate::{Call, Engine, Error, Runtime, Value, object};

/// A palette the script made.
pub(crate) struct Palette<H> {
    id: u64,
    title: String,
    /// The `Palette` object the script has.
    object: H,
    components: H,
    values: H,
    /// Its state as last reported, as JSON.
    reported: String,
}

/// What the user did in a palette.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    /// Changed a component's value.
    Change(String, Json),
    /// Clicked a button.
    Click(String),
    /// Closed the palette.
    Close,
}

/// Sets `Palette` on `global`.
pub(crate) fn install<E: Engine>(engine: &mut E, global: &E::Handle) -> Result<(), Error> {
    let constructor = engine.constructor("Palette", palette)?;
    engine.with_call(|call| call.set(global, "Palette", &Value::Js(constructor)))
}

/// `new Palette(title, components, values)`.
fn palette<H: Clone>(call: &mut dyn Call<H>, _this: &Value<H>, args: &[Value<H>]) -> Result<Value<H>, Error> {
    let title = match args.first() {
        Some(title) => call.to_string(title)?,
        None => String::new(),
    };
    let Some(Value::Js(components)) = args.get(1) else {
        return Err(Error::Script("Palette: components must be an object".into()));
    };
    let values = match args.get(2) {
        Some(Value::Js(values)) => values.clone(),
        _ => call.new_object()?,
    };
    // `values` holds every component's value from the start.
    let globals = dialog::globals(call)?;
    for component in describe(call, &globals, components, Some(&values))? {
        let name = component["name"].as_str().unwrap_or_default();
        if matches!(call.get(&values, name)?, Value::Undefined) {
            let value = from_component(call, &globals, &component, &component["value"])?;
            call.set(&values, name, &value)?;
        }
    }
    let palette = call.new_object()?;
    call.set(&palette, "title", &Value::String(title.clone()))?;
    call.set(&palette, "components", &Value::Js(components.clone()))?;
    call.set(&palette, "values", &Value::Js(values.clone()))?;
    let host = call.host()?;
    let mut host = host.borrow_mut();
    host.next_palette_id += 1;
    let id = host.next_palette_id;
    host.palettes.push(Palette { id, title, object: palette.clone(), components: components.clone(), values, reported: String::new() });
    Ok(Value::Js(palette))
}

impl<E: Engine> Runtime<E> {
    /// The palettes that are new or changed since the last call, each as `{id, title, components}`
    /// with the components as `Dialog.prompt` describes them, their values the current ones.
    pub fn take_palettes(&mut self) -> Result<Vec<Json>, Error> {
        let palettes: Vec<(u64, String, E::Handle, E::Handle)> =
            self.host.borrow().palettes.iter().map(|p| (p.id, p.title.clone(), p.components.clone(), p.values.clone())).collect();
        let mut changed = Vec::new();
        for (id, title, components, values) in palettes {
            let list = self.engine.with_call(|call| {
                let globals = dialog::globals(call)?;
                describe(call, &globals, &components, Some(&values))
            })?;
            let state = json!({ "id": id, "title": title, "components": list });
            let text = state.to_string();
            if let Some(palette) = self.host.borrow_mut().palettes.iter_mut().find(|p| p.id == id)
                && palette.reported != text
            {
                palette.reported = text;
                changed.push(state);
            }
        }
        Ok(changed)
    }

    /// Whether the script has palettes open.
    pub fn has_palettes(&self) -> bool {
        !self.host.borrow().palettes.is_empty()
    }

    /// What the user did in palette `id`.
    pub fn palette_event(&mut self, id: u64, event: PaletteEvent) -> Result<(), Error> {
        let found = self
            .host
            .borrow()
            .palettes
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.title.clone(), p.object.clone(), p.components.clone(), p.values.clone()));
        let Some((title, palette, components, values)) = found else {
            return Ok(());
        };
        if event == PaletteEvent::Close {
            self.host.borrow_mut().palettes.retain(|p| p.id != id);
        }
        self.engine.with_call(|call| match event {
            PaletteEvent::Change(name, json) => {
                let globals = dialog::globals(call)?;
                let list = describe(call, &globals, &components, Some(&values))?;
                let Some(component) = list.iter().find(|c| c["name"] == name.as_str()) else {
                    return Ok(());
                };
                let value = from_component(call, &globals, component, &json)?;
                call.set(&values, &name, &value)?;
                // The panel shows this value already: only what the handlers change goes back.
                let state = json!({ "id": id, "title": title, "components": describe(call, &globals, &components, Some(&values))? });
                if let Some(reported) = call.host()?.borrow_mut().palettes.iter_mut().find(|p| p.id == id) {
                    reported.reported = state.to_string();
                }
                let definition = object(call, &components, &name)?;
                handler(call, &definition, "onChange", &[value])?;
                handler(call, &palette, "onChange", &[Value::Js(definition)])
            }
            PaletteEvent::Click(name) => {
                let definition = object(call, &components, &name)?;
                handler(call, &definition, "onClick", &[])
            }
            PaletteEvent::Close => handler(call, &palette, "onClose", &[]),
        })
    }
}

/// Calls `owner[name](...args)` with `owner` as `this`, when the script set that handler.
fn handler<H: Clone>(call: &mut dyn Call<H>, owner: &H, name: &str, args: &[Value<H>]) -> Result<(), Error> {
    if let Value::Js(function) = call.get(owner, name)? {
        call.call(&function, &Value::Js(owner.clone()), args)?;
    }
    Ok(())
}
