//! The patch card: `patch.set` on the selected patches, as a form. It is filled from the first
//! selected patch and sends only what was changed; the engine checks and runs it.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, vec2};
use serde_json::{Map, Value, json};
use umcf_engine::Session;
use umcf_model::Patch;

use crate::bc::selected;
use crate::icons::{self, Icon, Ink};
use crate::{App, theme};

/// The patch types offered (OpenFOAM's; any other word can be typed).
const TYPES: [&str; 9] =
    ["patch", "wall", "mappedWall", "mapped", "mappedInternal", "symmetry", "symmetryPlane", "empty", "cyclic"];

/// What the card holds while it is open.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatchForm {
    /// The new name (one patch only).
    pub name: String,
    pub kind: String,
    /// The groups, separated by spaces.
    pub groups: String,
    /// Other entries, one `key value` per line.
    pub more: String,
    /// A new name followed into the results too, when saved (`results`).
    pub results: bool,
    pub error: Option<String>,
    /// The patches the form was filled for, and what it was filled with.
    pub(crate) filled_for: Option<Vec<(String, String)>>,
    pub(crate) filled: (String, String, String),
}

impl PatchForm {
    /// The parameters of `patch.set` (the patches are the selection's): only what was changed.
    pub fn params(&self, single: bool) -> Result<Value, String> {
        let (name, kind, groups) = &self.filled;
        let mut p = Map::new();
        if single && self.name.trim() != name {
            if self.name.trim().is_empty() {
                return Err("A patch needs a name.".to_string());
            }
            p.insert("name".into(), json!(self.name.trim()));
            if self.results {
                p.insert("results".into(), json!(true));
            }
        }
        if self.kind.trim() != kind {
            if self.kind.trim().is_empty() {
                return Err("A patch needs a type.".to_string());
            }
            p.insert("type".into(), json!(self.kind.trim()));
        }
        if self.groups.split_whitespace().collect::<Vec<_>>() != groups.split_whitespace().collect::<Vec<_>>() {
            p.insert("groups".into(), json!(self.groups.split_whitespace().collect::<Vec<_>>()));
        }
        let mut entries = Map::new();
        for line in self.more.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (k, v) = line
                .split_once(char::is_whitespace)
                .map(|(k, v)| (k, v.trim().trim_end_matches(';').trim()))
                .filter(|(_, v)| !v.is_empty())
                .ok_or_else(|| format!("`{line}`: write an entry as `key value`."))?;
            entries.insert(k.to_string(), json!(v));
        }
        if !entries.is_empty() {
            p.insert("entries".into(), Value::Object(entries));
        }
        if p.is_empty() {
            return Err("Nothing changed.".to_string());
        }
        Ok(Value::Object(p))
    }
}

/// The selected patch as the case has it.
fn patch<'a>(session: &'a Session, region: &str, name: &str) -> Option<&'a Patch> {
    session.case()?.region(region)?.patches.as_ref()?.iter().find(|p| p.name == name)
}

impl App {
    /// The card; returns whether Set or Close was pressed. `room` as for the condition card.
    pub(crate) fn patch_card(&mut self, ui: &mut Ui, width: f32, room: f32) -> (bool, bool) {
        let t = self.tokens.clone();
        // Its command runs or waits: shown as it was sent, not to be changed till it's done.
        let busy = self.card_busy(crate::jobs::Card::Patch);
        let patches = selected(&self.session);
        let (mut ok, mut close) = (false, false);
        let Some(form) = &mut self.patch_form else { return (false, false) };
        if form.filled_for.as_ref() != Some(&patches) {
            let first = patches.first().and_then(|(r, p)| patch(&self.session, r, p));
            let filled = first.map(|p| (p.name.clone(), p.kind.clone(), p.groups.join(" "))).unwrap_or_default();
            (form.name, form.kind, form.groups) = filled.clone();
            form.filled = filled;
            form.more.clear();
            form.results = false;
            form.error = None;
            form.filled_for = Some(patches.clone());
        }
        let muted = Ink { line: t.text_muted, fill: t.accent };
        let card_top = ui.cursor().top();
        t.card_frame().show(ui, |ui| {
            if busy {
                ui.disable();
            }
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Patch, Ink { line: t.text, fill: t.accent });
                ui.label(RichText::new("Patch").font(theme::strong(14.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close = crate::icon_button(ui, Icon::Close, 22.0, true, false, "Close", muted, &t).clicked();
                });
            });
            ui.separator();
            if patches.is_empty() {
                ui.label(
                    RichText::new("Select patches in the view or the browser (Ctrl+click for more).")
                        .color(t.text_muted),
                );
                return;
            }
            let names: Vec<&str> = patches.iter().map(|(_, p)| p.as_str()).collect();
            let shown = names.iter().take(6).copied().collect::<Vec<_>>().join(", ");
            let more = if names.len() > 6 { format!(" and {} more", names.len() - 6) } else { String::new() };
            let n = if names.len() == 1 { "1 patch".to_string() } else { format!("{} patches", names.len()) };
            ui.label(RichText::new(format!("{n}: {shown}{more}")).color(t.text_muted));
            let used = ui.cursor().top() - card_top;
            let below = if form.error.is_some() { 100.0 } else { 80.0 };
            let body = (room - used - below).max(100.0);
            egui::ScrollArea::vertical().id_salt("patch-body").max_height(body).show(ui, |ui| {
                egui::Grid::new("patch-form").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                    if patches.len() == 1 {
                        let label = ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(width - 90.0))
                            .labelled_by(label.id);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut form.results, "In the results too, when saved").on_hover_text(
                            "The other time directories, the processor directories and boundaryData follow the new name on Save (not backed up: a rename back undoes it)",
                        );
                        ui.end_row();
                    }
                    let label = ui.label("Type");
                    ui.add(egui::TextEdit::singleline(&mut form.kind).desired_width(width - 90.0).hint_text("wall"))
                        .labelled_by(label.id);
                    ui.end_row();
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    let current = form.kind.trim().to_string();
                    for k in TYPES.iter().filter(|k| **k != current) {
                        let chip = egui::Button::new(RichText::new(*k).size(11.5).monospace()).small();
                        if ui.add(chip).on_hover_text("Use this type").clicked() {
                            form.kind = k.to_string();
                        }
                    }
                });
                egui::Grid::new("patch-form-groups").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                    let label = ui.label("Groups");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.groups)
                            .desired_width(width - 90.0)
                            .hint_text("wall buildings"),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
                let label =
                    ui.label(RichText::new("Other entries, one per line: key value").size(12.0).color(t.text_muted));
                ui.add(
                    egui::TextEdit::multiline(&mut form.more)
                        .code_editor()
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("neighbourRegion buildings"),
                )
                .labelled_by(label.id);
                ui.add_space(4.0);
                ui.label(RichText::new("Now").size(12.0).color(t.text_muted));
                egui::Grid::new("patch-now-grid").num_columns(2).striped(true).show(ui, |ui| {
                    for (r, name) in patches.iter().take(30) {
                        ui.label(RichText::new(name).monospace());
                        match patch(&self.session, r, name) {
                            Some(p) => {
                                let mut text = format!("{} · {} faces", p.kind, p.n_faces);
                                for (k, v) in p.params.iter().take(3) {
                                    text.push_str(&format!(" · {k} {v}"));
                                }
                                ui.label(text)
                            }
                            None => ui.label(RichText::new("not in the mesh").color(t.text_muted)),
                        };
                        ui.end_row();
                    }
                });
                if patches.len() > 1 {
                    ui.label(RichText::new("A name is set one patch at a time.").size(12.0).color(t.text_muted));
                }
            });
            if let Some(e) = &form.error {
                ui.label(RichText::new(e).color(t.error));
            }
            ui.add_space(4.0);
            let row = vec2(ui.available_width(), 28.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                let set = egui::Button::new(
                    RichText::new(if busy { "Setting…" } else { "Set" }).color(Color32::WHITE).strong(),
                )
                .fill(t.accent)
                .min_size(vec2(64.0, 26.0));
                ok = ui.add(set).clicked();
                close |= ui.add(egui::Button::new("Cancel").min_size(vec2(64.0, 26.0))).clicked();
            });
        });
        (ok, close)
    }

    /// Runs `patch.set` with the card's parameters, as a job; the card closes when it works, and
    /// stays open with the error when it doesn't.
    pub(crate) fn set_patch(&mut self) {
        let shown = selected(&self.session);
        let single = shown.len() == 1;
        let Some(form) = self.patch_form.clone() else { return };
        match form.params(single) {
            // The patches the card shows (see `set_bc`).
            Ok(params) => {
                let params = crate::bc::with_patches(params, &shown);
                self.start_card("patch.set", params, crate::jobs::Card::Patch)
            }
            Err(e) => self.card_error(crate::jobs::Card::Patch, e),
        }
    }
}
