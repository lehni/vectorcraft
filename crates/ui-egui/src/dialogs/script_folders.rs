//! The Scriptographer panel's script folders, as the original Scriptographer's repositories were: Script Folders
//! lists them in the panel's order, each with its name, its folder (Choose… picks another), whether
//! the panel shows it, and buttons to move it up or down or remove it; Add Folder… adds one. The
//! Examples, the app's own, can only be hidden and moved. New Folder asks for the name of a folder
//! to make in one.
//!
//! Fields of Script Folders: `folders` (`ScriptFolder`s). Of New Folder: `folder` (where it goes)
//! and `name`.

use serde_json::{Value, json};

use super::{DialogSpec, form};
use crate::VectorcraftApp;
use crate::state::{Dialog, ScriptFolder};
use crate::widgets;

pub const KIND: &str = "scriptFolders";
pub const NEW_FOLDER: &str = "newScriptFolder";

/// Width of a folder's name field.
const NAME_W: f32 = 140.0;
/// Width of a folder's path.
const PATH_W: f32 = 220.0;

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("Script Folders").into(),
    body: |app, ui, d| {
        body(app, ui, d);
        false
    },
    confirm: |app, d| {
        app.ui.scriptographer_folders = folders(d).into_iter().filter(|f| !f.path.is_empty()).collect();
        app.prefs_changed = true;
        app.scripts.forget_tree();
        app.ui.dialog = None;
        Ok(Value::Null)
    },
    min_width: 560.0,
    ..DialogSpec::FORM
};

pub(super) const NEW_FOLDER_SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("New Folder").into(),
    body: |_, ui, d| {
        ui.horizontal(|ui| {
            ui.label(tl!("Name:"));
            form::text(ui, d, "name", 220.0);
        });
        false
    },
    confirm: |app, d| {
        let params = json!({ "folder": d.str("folder"), "name": d.str("name").trim() });
        crate::scripts::new_folder(app, &params)?;
        app.ui.dialog = None;
        Ok(Value::Null)
    },
    ..DialogSpec::FORM
};

/// Opens Script Folders with the folders as they are.
pub(crate) fn open(app: &mut VectorcraftApp) {
    app.ui.dialog = Some(Dialog::new(KIND, json!({ "folders": app.ui.scriptographer_folders })));
}

/// Opens New Folder for a folder in `folder`, suggesting `name`.
pub(crate) fn new_folder(app: &mut VectorcraftApp, folder: &std::path::Path, name: &str) {
    app.ui.dialog = Some(Dialog::new(NEW_FOLDER, json!({ "folder": folder.to_string_lossy(), "name": name })));
}

fn folders(d: &Dialog) -> Vec<ScriptFolder> {
    d.fields.get("folders").cloned().and_then(|f| serde_json::from_value(f).ok()).unwrap_or_default()
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) {
    let mut list = folders(d);
    let count = list.len();
    // What a row's buttons ask for, done after the list is drawn.
    let mut action = None;
    for (i, folder) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let icon = if folder.visible { "eye" } else { "eye-off" };
            if widgets::icon_button(ui, icon, tl!("Show"), false, 22.0).clicked() {
                folder.visible = !folder.visible;
            }
            let id = ("script-folder-name", i);
            if folder.sealed {
                // Where a name field shows its text.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(NAME_W, 22.0), egui::Sense::hover());
                let (font, color) = (egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
                ui.painter().text(rect.left_center() + egui::vec2(7.0, 0.0), egui::Align2::LEFT_CENTER, &folder.name, font, color);
            } else if let Some(name) = widgets::text_field(ui, id, Some(&folder.name), NAME_W, 1) {
                folder.name = name;
            }
            let dim = crate::theme::Tokens::get(ui.ctx()).text_dim;
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(PATH_W, 22.0), egui::Sense::hover());
            let font = egui::FontId::proportional(12.0);
            // A long path shows its end, where its folder's name is; all of it in the tooltip.
            let mut shown = folder.path.clone();
            while shown.chars().count() > 1 && ui.painter().layout_no_wrap(shown.clone(), font.clone(), dim).size().x > PATH_W {
                shown = format!("…{}", shown.chars().skip(2).collect::<String>());
            }
            ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, shown, font, dim);
            resp.on_hover_text(&folder.path);
            // The Examples' can't change; shown, so the rows line up.
            if ui.add_enabled(!folder.sealed, egui::Button::new(tl!("Choose…"))).clicked() {
                action = Some(Row::Choose(i));
            }
            if widgets::icon_button_enabled(ui, "chevron-up", tl!("Move Up"), false, i > 0, 22.0).clicked() {
                action = Some(Row::Swap(i - 1));
            }
            if widgets::icon_button_enabled(ui, "chevron-down", tl!("Move Down"), false, i + 1 < count, 22.0).clicked() {
                action = Some(Row::Swap(i));
            }
            if widgets::icon_button_enabled(ui, "trash-2", tl!("Remove"), false, !folder.sealed, 22.0).clicked() {
                action = Some(Row::Remove(i));
            }
        });
    }
    ui.add_space(6.0);
    if ui.button(tl!("Add Folder…")).clicked() {
        action = Some(Row::Add);
    }
    match action {
        Some(Row::Swap(i)) => list.swap(i, i + 1),
        Some(Row::Remove(i)) => {
            list.remove(i);
        }
        _ => {}
    }
    d.fields.insert("folders".into(), json!(list));
    // Picking a folder goes through the file dialog, which may answer later, on the dialog then.
    let pick = |app: &mut VectorcraftApp, d: &mut Dialog, row: Option<usize>| {
        let r = crate::picks::in_dialog(app, d, move |app, d| {
            if let Some(path) = crate::picks::folder(app) {
                let mut list = folders(d);
                match row.and_then(|i| list.get_mut(i)) {
                    Some(folder) => folder.path = path,
                    None => {
                        let name = std::path::Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                        list.push(ScriptFolder::new(name, path));
                    }
                }
                d.fields.insert("folders".into(), json!(list));
            }
            Ok(())
        });
        // Picking a folder doesn't fail.
        let _ = r;
    };
    match action {
        Some(Row::Choose(i)) => pick(app, d, Some(i)),
        Some(Row::Add) => pick(app, d, None),
        _ => {}
    }
}

/// What a row's buttons do.
enum Row {
    Choose(usize),
    /// Swap the rows at this index and the next.
    Swap(usize),
    Remove(usize),
    Add,
}
