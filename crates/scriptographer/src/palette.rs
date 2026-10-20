//! Scriptographer's `Palette` (`js/src/palette.js`), a panel that stays open while the script
//! runs. The host shows the palettes [`Runtime::take_palettes`] reports, and sends what the user
//! does with [`Runtime::palette_event`].

use serde_json::Value as Json;

use crate::{Engine, Error, Runtime, Value, dialog};

/// What the user did in a palette.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    /// Changed a component's value.
    Change(String, Json),
    /// Clicked a button, or picked a menu entry.
    Click(String),
    /// Closed the palette.
    Close,
}

impl<E: Engine> Runtime<E> {
    /// The palettes that are new or changed since the last call, each as `{id, title, components}`
    /// with the components as `Dialog.prompt` describes them, their values the current ones.
    pub fn take_palettes(&mut self) -> Result<Vec<Json>, Error> {
        let palettes = self.call_bundle("takePalettes", Vec::new())?;
        Ok(match palettes {
            Json::Array(palettes) => palettes,
            _ => Vec::new(),
        })
    }

    /// Whether the script has palettes open.
    pub fn has_palettes(&mut self) -> bool {
        self.call_bundle("hasPalettes", Vec::new()).is_ok_and(|open| open == Json::Bool(true))
    }

    /// What the user did in palette `id`.
    pub fn palette_event(&mut self, id: u64, event: PaletteEvent) -> Result<(), Error> {
        let (kind, name, value) = match event {
            PaletteEvent::Change(name, value) => ("change", name, value),
            PaletteEvent::Click(name) => ("click", name, Json::Null),
            PaletteEvent::Close => ("close", String::new(), Json::Null),
        };
        self.call_bundle("handlePaletteEvent", vec![Json::from(id), Json::from(kind), Json::from(name), value]).map(|_| ())
    }

    /// Calls the function `name` the bundle gave the runtime with `args`, as JSON both ways.
    fn call_bundle(&mut self, name: &str, args: Vec<Json>) -> Result<Json, Error> {
        self.engine.with_call(|call| {
            let globals = dialog::globals(call)?;
            let args = args.iter().map(|arg| dialog::from_json(call, &globals, arg)).collect::<Result<Vec<Value<E::Handle>>, _>>()?;
            let result = crate::call_bundle(call, name, &args)?;
            dialog::to_json(call, &globals, &result)
        })
    }
}
