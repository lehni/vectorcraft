//! The PaperCraft panel: the scripts folder as a tree, as Scriptographer's palette showed it.
//! Tool scripts (they define mouse or key handlers) have a pencil, the one the PaperCraft Tool
//! stands for a coloured one, running scripts their name in the accent colour. Double-click opens a
//! script in its editor. The bottom bar has Scriptographer's buttons in its groups: execute the
//! selected script and stop the running ones, edit, create a script or a folder, show or hide the
//! console. The panel menu lists the folder again or chooses another one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use egui::{Align2, FontId, Sense, Ui, vec2};
use serde_json::json;

use super::{pstate, set_pstate};
use crate::scripts::{Entry, EntryKind};
use crate::theme::Tokens;
use crate::{VectorcraftApp, icons, widgets};

pub const ID: &str = "papercraft";

const SELECTED: &str = "papercraft-selected";
const OPEN: &str = "papercraft-open";
const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;
/// The space between the bottom bar's groups of buttons.
const GAP: f32 = 8.0;

pub fn show(app: &mut VectorcraftApp, ui: &mut Ui) {
    let Some(folder) = app.ui.papercraft_folder.clone().map(PathBuf::from) else {
        ui.add_space(6.0);
        widgets::dim_label(ui, tl!("Choose the folder your scripts are in."));
        ui.add_space(4.0);
        if ui.button(tl!("Choose Scripts Folder…")).clicked() {
            choose_folder(app);
        }
        return;
    };
    let mut selected: Option<PathBuf> = pstate(ui.ctx(), SELECTED);
    let mut open: BTreeSet<PathBuf> = pstate(ui.ctx(), OPEN);
    let (mut run, mut edit) = (None, None);
    let running = app.scripts.running_paths();
    let active = app.scripts.tool().map(PathBuf::from);
    // Laid out from the bottom: the bar stays at the bottom, the tree takes the height above it.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        // Scriptographer's buttons, in its groups: execute and stop, edit, new script and folder,
        // console.
        ui.horizontal(|ui| {
            let script = selected.clone().filter(|p| p.is_file());
            let into =
                selected.as_ref().map(|p| if p.is_dir() { p.clone() } else { p.parent().map(PathBuf::from).unwrap_or_else(|| folder.clone()) });
            let into = into.map_or(json!({}), |f| json!({ "folder": f.to_string_lossy() }));
            if widgets::icon_button_enabled(ui, "play", tl!("Execute Script"), false, script.is_some(), 24.0).clicked() {
                run = script.clone();
            }
            if widgets::icon_button(ui, "square", tl!("Stop Running Scripts"), false, 24.0).clicked() {
                command(app, "ui.stopScripts", json!({}));
            }
            ui.add_space(GAP);
            if widgets::icon_button_enabled(ui, "file-pen", tl!("Edit Script…"), false, script.is_some(), 24.0).clicked()
                && let Some(path) = &script
            {
                command(app, "ui.editScript", json!({ "path": path.to_string_lossy() }));
            }
            ui.add_space(GAP);
            if widgets::icon_button(ui, "file-plus", tl!("Create a New Script…"), false, 24.0).clicked()
                && let Some(r) = command(app, "ui.newScript", into.clone())
            {
                selected = r["path"].as_str().map(PathBuf::from);
            }
            if widgets::icon_button(ui, "folder-plus", tl!("Create New Folder…"), false, 24.0).clicked()
                && let Some(r) = command(app, "ui.newScriptFolder", into)
            {
                selected = r["path"].as_str().map(PathBuf::from);
            }
            ui.add_space(GAP);
            if widgets::icon_button(ui, "terminal", tl!("Show / Hide Console"), false, 24.0).clicked() {
                command(app, "window.panel", json!({ "panel": crate::panels::papercraft_console::ID, "toggle": true }));
            }
        });
        widgets::divider(ui);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::ScrollArea::vertical().id_salt("papercraft-tree").auto_shrink([false, false]).show(ui, |ui| {
                let entries = app.scripts.tree(&folder, false);
                let mut rows = Vec::new();
                flatten(entries, 0, &open, &mut rows);
                for (depth, entry) in rows {
                    let state = Row {
                        selected: selected.as_deref() == Some(&entry.path),
                        open: open.contains(&entry.path),
                        running: running.contains(&entry.path),
                        active: active.as_deref() == Some(&entry.path),
                    };
                    let action = row(ui, depth, entry, state);
                    match action {
                        Some(Action::Select) => selected = Some(entry.path.clone()),
                        Some(Action::Toggle) => {
                            if !open.remove(&entry.path) {
                                open.insert(entry.path.clone());
                            }
                        }
                        Some(Action::Edit) => edit = Some(entry.path.clone()),
                        None => {}
                    }
                }
            });
        });
    });
    set_pstate(ui.ctx(), SELECTED, selected);
    set_pstate(ui.ctx(), OPEN, open);
    if let Some(path) = run {
        command(app, "ui.runScript", json!({ "path": path.to_string_lossy() }));
    }
    if let Some(path) = edit {
        command(app, "ui.editScript", json!({ "path": path.to_string_lossy() }));
    }
}

/// The panel's (≡) menu: list the scripts folder again, or choose another one.
pub fn menu(app: &mut VectorcraftApp, ui: &mut Ui) {
    let folder = app.ui.papercraft_folder.clone().map(PathBuf::from);
    if widgets::menu_item(ui, "Reload", folder.is_some(), false)
        && let Some(folder) = folder
    {
        app.scripts.tree(&folder, true);
    }
    if widgets::menu_item(ui, "Choose Scripts Folder…", true, false) {
        choose_folder(app);
    }
}

/// Runs a command, its error in the status bar. Returns its result.
fn command(app: &mut VectorcraftApp, id: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    app.run(id, params).map_err(|e| app.status(e)).ok()
}

enum Action {
    Select,
    Toggle,
    /// Double-click on a script opens it in its editor, as in Scriptographer.
    Edit,
}

/// The visible rows: every entry, and the children of open folders, with their depth.
fn flatten<'a>(entries: &'a [Entry], depth: usize, open: &BTreeSet<PathBuf>, rows: &mut Vec<(usize, &'a Entry)>) {
    for entry in entries {
        rows.push((depth, entry));
        if let EntryKind::Folder(children) = &entry.kind
            && open.contains(&entry.path)
        {
            flatten(children, depth + 1, open, rows);
        }
    }
}

/// How a row of the tree shows.
struct Row {
    selected: bool,
    /// A folder whose contents show.
    open: bool,
    /// A script that runs, or a tool script that is loaded.
    running: bool,
    /// The tool script the PaperCraft Tool button stands for.
    active: bool,
}

fn row(ui: &mut Ui, depth: usize, entry: &Entry, Row { selected, open, running, active }: Row) -> Option<Action> {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, t.row_selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let x = rect.left() + 4.0 + depth as f32 * INDENT;
    let icon_rect = |x: f32| egui::Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0));
    let mut action = None;
    let icon = match &entry.kind {
        EntryKind::Folder(_) => {
            let disclosure = icon_rect(x);
            let toggle = ui.interact(disclosure, ui.id().with(("papercraft-disclosure", &entry.path)), Sense::click());
            icons::paint(ui, if open { "chevron-down" } else { "chevron-right" }, disclosure, t.text_dim);
            if toggle.clicked() {
                action = Some(Action::Toggle);
            }
            "folder-open"
        }
        EntryKind::Script { tool } => {
            if *tool {
                "dc-papercraft-tool"
            } else {
                "file-code"
            }
        }
    };
    // The tool script the PaperCraft Tool stands for has the coloured pencil, as in Scriptographer.
    if active {
        icons::paint(ui, "dc-papercraft-tool-active", icon_rect(x + 16.0), egui::Color32::WHITE);
    } else {
        icons::paint(ui, icon, icon_rect(x + 16.0), t.icon);
    }
    let color = if running { t.accent } else { t.text_strong };
    ui.painter().text(egui::pos2(x + 36.0, rect.center().y), Align2::LEFT_CENTER, &entry.name, FontId::proportional(12.5), color);
    if action.is_none() {
        if resp.double_clicked() {
            action = Some(match entry.kind {
                EntryKind::Folder(_) => Action::Toggle,
                EntryKind::Script { .. } => Action::Edit,
            });
        } else if resp.clicked() {
            action = Some(Action::Select);
        }
    }
    action
}

/// Asks for the scripts folder and lists it.
fn choose_folder(app: &mut VectorcraftApp) {
    crate::picks::button(app, |app| {
        if let Some(folder) = crate::picks::folder(app) {
            app.ui.papercraft_folder = Some(folder);
            app.prefs_changed = true;
        }
    });
}
