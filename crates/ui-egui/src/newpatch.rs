//! The new patch card: `patch.create` from the faces picked in the view, or from the faces of
//! the selected patches in a box or near a surface, as a form. The engine checks and runs it.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, vec2};
use serde_json::{Map, Value, json};

use crate::bc::selected;
use crate::icons::{self, Icon, Ink};
use crate::{App, theme};

/// Where the faces are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Box,
    Surface,
    /// The faces picked in the view.
    Picked,
}

/// The picked faces: (region, face).
fn picked(session: &umcf_engine::Session) -> Vec<(String, u32)> {
    session
        .selection()
        .iter()
        .filter_map(|s| match s {
            umcf_engine::Sel::Face { region, face } => Some((region.clone(), *face)),
            _ => None,
        })
        .collect()
}

/// What the card holds while it is open.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPatchForm {
    pub name: String,
    pub kind: String,
    /// The groups, separated by spaces.
    pub groups: String,
    pub source: Source,
    /// The box's corners, x y z each.
    pub min: [String; 3],
    pub max: [String; 3],
    /// The surface (a file of constant/triSurface), one of its solids, and the distance.
    pub file: String,
    pub solid: String,
    pub distance: String,
    pub error: Option<String>,
    /// A source was chosen by hand: picking faces doesn't switch to `Picked` any more.
    pub(crate) chose: bool,
}

impl Default for NewPatchForm {
    fn default() -> Self {
        NewPatchForm {
            name: String::new(),
            kind: "patch".to_string(),
            groups: String::new(),
            source: Source::Box,
            min: Default::default(),
            max: Default::default(),
            file: String::new(),
            solid: String::new(),
            distance: String::new(),
            error: None,
            chose: false,
        }
    }
}

fn number(what: &str, s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("{what}: a number, not `{}`.", s.trim()))
}

impl NewPatchForm {
    /// The parameters of `patch.create`; `region` is the selection's region, when it has one.
    pub fn params(&self, region: Option<&str>) -> Result<Value, String> {
        let mut p = Map::new();
        if self.name.trim().is_empty() {
            return Err("A new patch needs a name.".to_string());
        }
        p.insert("name".into(), json!(self.name.trim()));
        if self.kind.trim().is_empty() {
            return Err("A patch needs a type.".to_string());
        }
        p.insert("type".into(), json!(self.kind.trim()));
        let groups: Vec<&str> = self.groups.split_whitespace().collect();
        if !groups.is_empty() {
            p.insert("groups".into(), json!(groups));
        }
        if let Some(r) = region {
            p.insert("region".into(), json!(r));
        }
        match self.source {
            Source::Box => {
                let corner = |c: &[String; 3], what: &str| -> Result<Vec<f64>, String> {
                    c.iter().zip(["x", "y", "z"]).map(|(v, k)| number(&format!("{what} {k}"), v)).collect()
                };
                p.insert("box".into(), json!({ "min": corner(&self.min, "From")?, "max": corner(&self.max, "To")? }));
            }
            Source::Surface => {
                if self.file.trim().is_empty() {
                    return Err("Give the surface (a file of constant/triSurface).".to_string());
                }
                let mut s = Map::new();
                s.insert("file".into(), json!(self.file.trim()));
                if !self.solid.trim().is_empty() {
                    s.insert("solid".into(), json!(self.solid.trim()));
                }
                s.insert("distance".into(), json!(number("Distance", &self.distance)?));
                p.insert("surface".into(), Value::Object(s));
            }
            // patch.create takes the selected faces.
            Source::Picked => {}
        }
        Ok(Value::Object(p))
    }
}

impl App {
    /// The card; returns whether Prepare or Close was pressed. `room` as for the condition card.
    pub(crate) fn new_patch_card(&mut self, ui: &mut Ui, width: f32, room: f32) -> (bool, bool) {
        let t = self.tokens.clone();
        let patches = selected(&self.session);
        let faces = picked(&self.session);
        let (mut ok, mut close) = (false, false);
        let Some(form) = &mut self.new_patch else { return (false, false) };
        // Faces picked: those; none: a box. Unless a source was chosen by hand.
        if !form.chose {
            form.source = if faces.is_empty() { Source::Box } else { Source::Picked };
        }
        let muted = Ink { line: t.text_muted, fill: t.accent };
        let card_top = ui.cursor().top();
        t.card_frame().show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Patch, Ink { line: t.text, fill: t.accent });
                ui.label(RichText::new("New patch").font(theme::strong(14.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close = crate::icon_button(ui, Icon::Close, 22.0, true, false, "Close", muted, &t).clicked();
                });
            });
            ui.separator();
            let from = if form.source == Source::Picked {
                let n = faces.len();
                if n == 0 {
                    "Pick faces in the view (the Pick faces button under it; Ctrl+click for more).".to_string()
                } else {
                    format!("The {} picked in the view.", if n == 1 { "face".to_string() } else { format!("{n} faces") })
                }
            } else if patches.is_empty() {
                "Select the patches to take faces from, in the view or the browser (Ctrl+click for more).".to_string()
            } else {
                let names: Vec<&str> = patches.iter().map(|(_, p)| p.as_str()).take(6).collect();
                let more = if patches.len() > 6 { format!(" and {} more", patches.len() - 6) } else { String::new() };
                format!("Faces of {}{more}.", names.join(", "))
            };
            ui.label(RichText::new(from).color(t.text_muted));
            let used = ui.cursor().top() - card_top;
            let below = if form.error.is_some() { 120.0 } else { 100.0 };
            let body = (room - used - below).max(100.0);
            let field = |ui: &mut Ui, label: &str, text: &mut String, hint: &str, w: f32| {
                let l = ui.label(label);
                ui.add(egui::TextEdit::singleline(text).desired_width(w).hint_text(hint)).labelled_by(l.id);
                ui.end_row();
            };
            egui::ScrollArea::vertical().id_salt("new-patch-body").max_height(body).show(ui, |ui| {
                egui::Grid::new("new-patch-form").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                    field(ui, "Name", &mut form.name, "roof", width - 90.0);
                    field(ui, "Type", &mut form.kind, "wall", width - 90.0);
                    field(ui, "Groups", &mut form.groups, "buildings", width - 90.0);
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Faces");
                    let before = form.source;
                    ui.radio_value(&mut form.source, Source::Picked, "Picked");
                    ui.radio_value(&mut form.source, Source::Box, "In a box");
                    ui.radio_value(&mut form.source, Source::Surface, "Near a surface");
                    form.chose |= form.source != before;
                });
                match form.source {
                    Source::Picked => {}
                    Source::Box => {
                        ui.label(RichText::new("Faces whose centre is inside, x y z").size(12.0).color(t.text_muted));
                        egui::Grid::new("new-patch-box").num_columns(4).spacing(vec2(6.0, 6.0)).show(ui, |ui| {
                            for (label, c) in [("From", &mut form.min), ("To", &mut form.max)] {
                                ui.label(label);
                                for (v, k) in c.iter_mut().zip(["x", "y", "z"]) {
                                    ui.add(
                                        egui::TextEdit::singleline(v).desired_width((width - 90.0) / 3.0).hint_text(k),
                                    )
                                    .on_hover_text(format!("{label} {k}"));
                                }
                                ui.end_row();
                            }
                        });
                    }
                    Source::Surface => {
                        ui.label(
                            RichText::new("Faces whose centre is within the distance of it")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                        egui::Grid::new("new-patch-surface").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                            field(ui, "Surface", &mut form.file, "buildings.stl", width - 90.0);
                            field(ui, "Solid", &mut form.solid, "all of it", width - 90.0);
                            field(ui, "Distance", &mut form.distance, "0.5", width - 90.0);
                        });
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("The mesh gets the patch when createPatch runs, once, with the dictionary of the patches prepared on this mesh (the result says how).")
                        .size(12.0)
                        .color(t.text_muted),
                );
            });
            if let Some(e) = &form.error {
                ui.label(RichText::new(e).color(t.error));
            }
            ui.add_space(4.0);
            let row = vec2(ui.available_width(), 28.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                let set = egui::Button::new(RichText::new("Prepare").color(Color32::WHITE).strong())
                    .fill(t.accent)
                    .min_size(vec2(64.0, 26.0));
                ok = ui.add(set).clicked();
                close |= ui.add(egui::Button::new("Cancel").min_size(vec2(64.0, 26.0))).clicked();
            });
        });
        (ok, close)
    }

    /// Runs `patch.create` with the card's parameters, as a job; the card closes when it works,
    /// and stays open with the error when it doesn't.
    pub(crate) fn create_patch(&mut self) {
        // While a job runs the session here is a stand-in: refused, and the card says why.
        if self.busy() {
            return self.start_card("patch.create", serde_json::json!({}), crate::jobs::Card::NewPatch);
        }
        let Some(form) = self.new_patch.clone() else { return };
        let mut regions: Vec<String> = if form.source == Source::Picked {
            picked(&self.session).into_iter().map(|(r, _)| r).collect()
        } else {
            selected(&self.session).into_iter().map(|(r, _)| r).collect()
        };
        regions.sort();
        regions.dedup();
        let what = if form.source == Source::Picked { "faces" } else { "patches" };
        let params = match regions.as_slice() {
            [r] => form.params(Some(r)),
            [] if form.source == Source::Picked => Err("Pick the faces in the view first.".to_string()),
            [] => Err("Select the patches to take faces from.".to_string()),
            _ => Err(format!("Select {what} of one region.")),
        };
        match params {
            Ok(params) => self.start_card("patch.create", params, crate::jobs::Card::NewPatch),
            Err(e) => self.card_error(crate::jobs::Card::NewPatch, e),
        }
    }
}
