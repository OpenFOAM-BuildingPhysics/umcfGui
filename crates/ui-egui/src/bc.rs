//! The boundary-condition card: `bc.set` on the selected patches, as a form. The card only
//! builds the command's parameters; the engine checks and runs it.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, vec2};
use serde_json::{Map, Value, json};
use umcf_engine::{Sel, Session};
use umcf_model::Bc;

use crate::icons::{self, Icon, Ink};
use crate::{App, theme};

/// The most suggestions listed under the type.
const SUGGESTIONS: usize = 10;

/// The most patches whose condition the card lists.
const NOW: usize = 30;

/// What the card holds while it is open.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BcForm {
    pub field: String,
    pub kind: String,
    /// OpenFOAM text (`uniform 300`); empty for none.
    pub value: String,
    /// Other entries, one `key value` per line.
    pub more: String,
    pub error: Option<String>,
    /// The field the type and value were filled in for, from the first patch's condition.
    pub(crate) filled_for: Option<String>,
}

impl BcForm {
    /// The parameters of `bc.set` (the patches are the selection's).
    pub fn params(&self) -> Result<Value, String> {
        if self.field.is_empty() {
            return Err("Choose a field.".to_string());
        }
        let kind = self.kind.trim();
        if kind.is_empty() {
            return Err("Give a type.".to_string());
        }
        let mut p = Map::new();
        p.insert("field".into(), json!(self.field));
        p.insert("type".into(), json!(kind));
        if !self.value.trim().is_empty() {
            p.insert("value".into(), json!(self.value.trim()));
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
        Ok(Value::Object(p))
    }
}

/// The selected patches, `(region, patch)`.
pub fn selected(session: &Session) -> Vec<(String, String)> {
    session
        .selection()
        .iter()
        .filter_map(|s| match s {
            Sel::Patch { region, patch } => Some((region.clone(), patch.clone())),
            Sel::Solid { .. } => None,
        })
        .collect()
}

/// The fields a condition is set for most often, first in the list.
const FIRST: [&str; 9] = ["U", "T", "p_rgh", "p", "k", "epsilon", "nut", "alphat", "w"];

/// The fields every region of `patches` has at the start time, with their class: the usual
/// ones first, then the others in the first region's order.
pub fn fields(session: &Session, patches: &[(String, String)]) -> Vec<(String, String)> {
    let Some(case) = session.case() else { return Vec::new() };
    let mut regions: Vec<&str> = patches.iter().map(|(r, _)| r.as_str()).collect();
    regions.dedup();
    let Some(first) = regions.first().and_then(|r| case.region(r)) else { return Vec::new() };
    let mut out: Vec<(String, String)> = first
        .fields
        .iter()
        .filter(|f| regions.iter().all(|r| case.region(r).is_some_and(|r| r.field(&f.name).is_some())))
        .map(|f| (f.name.clone(), f.class.clone().unwrap_or_default()))
        .collect();
    out.sort_by_key(|(f, _)| FIRST.iter().position(|x| x == f).unwrap_or(FIRST.len()));
    out
}

/// The entry that sets a patch's condition in a field: the patch's own, or one of its groups'
/// (a pattern is matched by the case check, not here).
pub fn current<'a>(session: &'a Session, region: &str, patch: &str, field: &str) -> Option<&'a Bc> {
    let r = session.case()?.region(region)?;
    let groups = r.patches.as_ref()?.iter().find(|p| p.name == patch).map(|p| p.groups.as_slice()).unwrap_or(&[]);
    let bcs = r.field(field)?.bcs.as_ref()?;
    bcs.iter()
        .find(|b| !b.pattern && b.patch == patch)
        .or_else(|| bcs.iter().find(|b| !b.pattern && groups.contains(&b.patch)))
}

/// The short text of a condition: its type and value.
pub fn describe(b: &Bc) -> String {
    let mut text = b.kind.clone().unwrap_or_else(|| "(no type)".to_string());
    if let Some(v) = &b.value {
        text.push_str(&format!("  {v}"));
    }
    text
}

impl App {
    /// The card; returns whether OK or Close was pressed.
    /// `room` is the height the card may take; its middle scrolls when it would need more, so
    /// the card stays where it is (egui moves a floating card that doesn't fit the window).
    pub(crate) fn bc_card(&mut self, ui: &mut Ui, width: f32, room: f32) -> (bool, bool) {
        let t = self.tokens.clone();
        let patches = selected(&self.session);
        let fields = fields(&self.session, &patches);
        let (mut ok, mut close) = (false, false);
        let Some(form) = &mut self.bc else { return (false, false) };
        if form.field.is_empty() || !fields.iter().any(|(f, _)| *f == form.field) {
            form.field = fields.first().map(|(f, _)| f.clone()).unwrap_or_default();
        }
        // Start from what the first patch has.
        if form.filled_for.as_deref() != Some(form.field.as_str()) {
            let b = patches.first().and_then(|(r, p)| current(&self.session, r, p, &form.field));
            form.kind = b.and_then(|b| b.kind.clone()).unwrap_or_default();
            form.value = b.and_then(|b| b.value.clone()).filter(|v| !v.starts_with("nonuniform")).unwrap_or_default();
            form.more.clear();
            form.filled_for = Some(form.field.clone());
        }
        let class = fields.iter().find(|(f, _)| *f == form.field).map(|(_, c)| c.clone()).unwrap_or_default();
        let muted = Ink { line: t.text_muted, fill: t.accent };
        let card_top = ui.cursor().top();
        t.card_frame().show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Condition, Ink { line: t.text, fill: t.accent });
                ui.label(RichText::new("Boundary condition").font(theme::strong(14.0)));
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
            if fields.is_empty() {
                ui.label(
                    RichText::new("These patches' regions have no field in common at the start time.").color(t.warning),
                );
                return;
            }
            // What is above takes `used`; the error and the buttons below about 80 points.
            let used = ui.cursor().top() - card_top;
            let below = if form.error.is_some() { 100.0 } else { 80.0 };
            let body = (room - used - below).max(100.0);
            egui::ScrollArea::vertical().id_salt("bc-body").max_height(body).show(ui, |ui| {
                egui::Grid::new("bc-form").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                    let label = ui.label("Field");
                    egui::ComboBox::from_id_salt("bc-field")
                        .selected_text(RichText::new(&form.field).monospace())
                        .width(width - 90.0)
                        .show_ui(ui, |ui| {
                            for (f, c) in &fields {
                                ui.selectable_value(&mut form.field, f.clone(), RichText::new(f).monospace())
                                    .on_hover_text(c);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label("Type");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.kind).desired_width(width - 90.0).hint_text("fixedValue"),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                });
                // The types to offer: OpenFOAM's common ones and the solver's, matching what is typed.
                let typed = form.kind.trim().to_lowercase();
                let offered: Vec<&str> = self
                    .session
                    .bc_types(&class)
                    .into_iter()
                    .filter(|k| typed.is_empty() || (k.to_lowercase().contains(&typed) && k.to_lowercase() != typed))
                    .take(SUGGESTIONS)
                    .collect();
                if !offered.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for k in offered {
                            let chip = egui::Button::new(RichText::new(k).size(11.5).monospace()).small();
                            if ui.add(chip).on_hover_text("Use this type").clicked() {
                                form.kind = k.to_string();
                            }
                        }
                    });
                }
                // Whether the entries already there stay.
                let kind = form.kind.trim();
                let kept = patches
                    .iter()
                    .filter(|(r, p)| {
                        current(&self.session, r, p, &form.field)
                            .is_some_and(|b| b.patch == *p && b.kind.as_deref() == Some(kind))
                    })
                    .count();
                let note = match kept {
                    _ if kind.is_empty() => None,
                    0 => Some("Each patch gets a new entry with this type.".to_string()),
                    k if k == patches.len() => Some(
                        "Same type: each patch keeps its entry; the value and entries here are set in it.".to_string(),
                    ),
                    k => {
                        let n = patches.len();
                        let (has, rest) = if k == 1 {
                            ("has this type and keeps its", "")
                        } else {
                            ("have this type and keep their", "s")
                        };
                        let other = if n - k == 1 {
                            "the other gets".to_string()
                        } else {
                            format!("the other{rest} {} get", n - k)
                        };
                        Some(format!("{k} of the {n} patches {has} entry; {other} a new one."))
                    }
                };
                if let Some(n) = note {
                    ui.label(RichText::new(n).size(12.0).color(t.text_muted));
                }
                egui::Grid::new("bc-form-values").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                    let label = ui.label("Value");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.value)
                            .desired_width(width - 90.0)
                            .hint_text("uniform 300")
                            .code_editor(),
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
                        .hint_text("inletValue uniform 0"),
                )
                .labelled_by(label.id);
                ui.add_space(4.0);
                ui.label(RichText::new("Now").size(12.0).color(t.text_muted));
                egui::Grid::new("bc-now-grid").num_columns(2).striped(true).show(ui, |ui| {
                    for (r, p) in patches.iter().take(NOW) {
                        ui.label(RichText::new(p).monospace());
                        match current(&self.session, r, p, &form.field) {
                            Some(b) => ui.label(describe(b)),
                            None => ui.label(RichText::new("by pattern or none").color(t.text_muted)),
                        };
                        ui.end_row();
                    }
                });
                if patches.len() > NOW {
                    ui.label(RichText::new(format!("… and {} more", patches.len() - NOW)).color(t.text_muted));
                }
            });
            if let Some(e) = &form.error {
                ui.label(RichText::new(e).color(t.error));
            }
            ui.add_space(4.0);
            let row = vec2(ui.available_width(), 28.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                let set = egui::Button::new(RichText::new("Set").color(Color32::WHITE).strong())
                    .fill(t.accent)
                    .min_size(vec2(64.0, 26.0));
                ok = ui.add(set).clicked();
                close |= ui.add(egui::Button::new("Cancel").min_size(vec2(64.0, 26.0))).clicked();
            });
        });
        (ok, close)
    }

    /// Runs `bc.set` with the card's parameters; the card stays open with the error if it fails.
    pub(crate) fn set_bc(&mut self) {
        let Some(form) = self.bc.clone() else { return };
        let result = form.params().and_then(|p| self.run("bc.set", p).map(|_| ()));
        match (result, &mut self.bc) {
            (Ok(()), _) => self.bc = None,
            (Err(e), Some(f)) => f.error = Some(e),
            (Err(_), None) => {}
        }
    }
}
