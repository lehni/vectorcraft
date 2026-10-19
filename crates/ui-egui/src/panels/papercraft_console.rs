//! The PaperCraft Console panel, as Scriptographer's console was: a line on top that runs
//! PaperScript in the console's own project on the active document (Enter runs it, each line one
//! undo step), and under it everything scripts printed, errors in the error colour.

use egui::{RichText, Ui};
use serde_json::json;

use super::{pstate, set_pstate};
use crate::theme::Tokens;
use crate::{VectorcraftApp, widgets};

pub const ID: &str = "papercraftConsole";

const INPUT: &str = "papercraft-console-input";

pub fn show(app: &mut VectorcraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut input: String = pstate(ui.ctx(), INPUT);
    let field = ui.add(egui::TextEdit::singleline(&mut input).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !input.trim().is_empty() {
        let code = std::mem::take(&mut input);
        if let Err(e) = app.run("ui.evaluateScript", json!({ "code": code })) {
            app.status(e);
        }
        field.request_focus();
    }
    set_pstate(ui.ctx(), INPUT, input);
    // Laid out from the bottom: the bar stays at the bottom, the log takes the height above it.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.horizontal(|ui| {
            if widgets::icon_button(ui, "trash-2", tl!("Clear Console"), false, 24.0).clicked() {
                app.scripts.clear_log();
            }
        });
        widgets::divider(ui);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            widgets::divider(ui);
            egui::ScrollArea::vertical().id_salt("papercraft-console-log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for line in app.scripts.log() {
                    let color = if line.error { t.error } else { t.text };
                    ui.label(RichText::new(&line.text).monospace().color(color));
                }
            });
        });
    });
}
