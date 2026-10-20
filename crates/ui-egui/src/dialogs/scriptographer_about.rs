//! About Scriptographer, from the Scriptographer panel's menu, as the original Scriptographer had
//! its About: the Paper.js logo, the Paper.js version scripts run with, who made it, and links.

use serde_json::Value;

use super::DialogSpec;
use crate::state::Dialog;
use crate::{VectorcraftApp, theme};

pub const KIND: &str = "scriptographerAbout";

/// The Paper.js logo.
const LOGO: &[u8] = include_bytes!("../../../../assets/scriptographer/paper-js.png");

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("About Scriptographer").into(),
    body: |app, ui, _| {
        body(app, ui);
        false
    },
    ok: None,
    max_width: Some(420.0),
    ..DialogSpec::FORM
};

pub(crate) fn open(app: &mut VectorcraftApp) -> Result<Value, String> {
    app.ui.dialog = Some(Dialog::new(KIND, Value::Null));
    Ok(Value::Null)
}

#[cfg(test)]
#[test]
fn the_logo_decodes() {
    assert_eq!(crate::brand::png_texture(&egui::Context::default(), "scriptographer-logo", LOGO).size(), [146, 146]);
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        // The logo is drawn on white.
        egui::Frame::NONE.fill(egui::Color32::WHITE).corner_radius(8).inner_margin(6).show(ui, |ui| {
            let logo = crate::brand::png_texture(ui.ctx(), "scriptographer-logo", LOGO);
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(96.0, 96.0)));
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Scriptographer").font(theme::semibold(22.0)));
            ui.label(crate::i18n::fmt(tl!("Scripting with Paper.js {version}"), &[("version", vectorcraft_scriptographer::paper_version())]));
            ui.add_space(8.0);
            ui.label(tl!("Paper.js by Jürg Lehni and Jonathan Puckey"));
            ui.add_space(8.0);
            for url in ["https://paperjs.org", "https://scriptographer.org"] {
                if ui.link(url.trim_start_matches("https://")).clicked() {
                    app.open_url(url);
                }
            }
        });
    });
}
