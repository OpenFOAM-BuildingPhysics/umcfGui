//! How the 3D view draws each layer: a region's mesh, a patch, a patch group, a surface or one
//! of its regions as surfaces, wireframe or points, or hidden, each with its own opacity. Set
//! from the browser's context menus, kept as `display` entries (as `scene.snapshot` takes them)
//! and sent with the view's picks, so that what is hidden is not picked.

use eframe::egui::{self, Ui};
use serde_json::Value;
use umcf_engine::{DisplayEntry, Layer};
use umcf_render::raster::{Display, Mode};

use crate::App;

/// A change asked for in a layer's menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    Mode(Mode),
    Opacity(f32),
    /// Back to how the layers it is part of are drawn.
    Reset,
}

/// The menu of a layer drawn as `current` (`set`: an entry of its own sets it); the change the
/// user made in it, if any.
pub(crate) fn menu(ui: &mut Ui, title: &str, current: Display, set: bool) -> Option<Change> {
    let mut change = None;
    ui.label(egui::RichText::new(title).strong());
    ui.separator();
    for (name, mode) in Mode::ALL {
        let label = match mode {
            Mode::Surface => "Surface",
            Mode::Wireframe => "Wireframe",
            Mode::Points => "Points",
            Mode::Hidden => "Hidden",
        };
        if ui.radio(current.mode == mode, label).on_hover_text(format!("Show it as {name}")).clicked() {
            change = Some(Change::Mode(mode));
        }
    }
    ui.separator();
    let mut opacity = current.opacity;
    let slider = egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity").fixed_decimals(2);
    if ui.add(slider).changed() {
        change = Some(Change::Opacity(opacity));
    }
    if ui.add_enabled(set, egui::Button::new("Reset")).on_hover_text("Draw it as what it is part of").clicked() {
        change = Some(Change::Reset);
        ui.close();
    }
    change
}

/// A short note of how an entry draws its layer, for the browser.
pub(crate) fn note(d: &DisplayEntry) -> String {
    let mut parts = Vec::new();
    if let Some(m) = d.mode.filter(|m| *m != Mode::Surface) {
        parts.push(m.name().to_string());
    }
    if let Some(a) = d.opacity.filter(|a| *a < 1.0) {
        parts.push(format!("{:.0}%", a * 100.0));
    }
    parts.join(" ")
}

impl App {
    /// The entry of `layer` itself, if it has one.
    pub fn layer_entry(&self, layer: &Layer) -> Option<&DisplayEntry> {
        self.display.iter().find(|e| e.layer == *layer)
    }

    /// How `layer` is drawn: after every entry, as for one of its classes (`groups`: a patch's
    /// groups).
    pub(crate) fn layer_display(&self, layer: &Layer, groups: &[String]) -> Display {
        let mut d = Display::default();
        let applies = |e: &Layer| match (e, layer) {
            (a, b) if a == b => true,
            (Layer::Region(r), Layer::Patch { region, .. } | Layer::Group { region, .. }) => r == region,
            (Layer::Group { region: r, group }, Layer::Patch { region, .. }) => r == region && groups.contains(group),
            (Layer::Surface(f), Layer::Solid { file, .. }) => f == file,
            _ => false,
        };
        for e in self.display.iter().filter(|e| applies(&e.layer)) {
            if let Some(m) = e.mode {
                d.mode = m;
            }
            if let Some(a) = e.opacity {
                d.opacity = a;
            }
        }
        d
    }

    /// Changes how `layer` is drawn. A layer's entry keeps its place, so a patch set after its
    /// region still wins over it; a new one goes last.
    pub fn set_display(&mut self, layer: Layer, change: Change) {
        let at = self.display.iter().position(|e| e.layer == layer);
        match (change, at) {
            (Change::Reset, Some(k)) => {
                self.display.remove(k);
            }
            (Change::Reset, None) => return,
            (c, Some(k)) => {
                if let Some(e) = self.display.get_mut(k) {
                    match c {
                        Change::Mode(m) => e.mode = Some(m),
                        Change::Opacity(a) => e.opacity = Some(a),
                        Change::Reset => {}
                    }
                }
            }
            (c, None) => {
                let (mode, opacity) = match c {
                    Change::Mode(m) => (Some(m), None),
                    Change::Opacity(a) => (None, Some(a)),
                    Change::Reset => (None, None),
                };
                self.display.push(DisplayEntry { layer, mode, opacity });
            }
        }
        self.revision += 1;
    }

    /// The entries as commands take them.
    pub(crate) fn display_json(&self) -> Value {
        self.display.iter().map(DisplayEntry::to_json).collect()
    }

    /// Drops the entries of what the case no longer has (a patch renamed, a surface an undo
    /// took away): the view's picks would be refused with them.
    pub(crate) fn prune_display(&mut self) {
        let Some(case) = self.session.case() else {
            self.display.clear();
            return;
        };
        let patches = |region: &str| case.regions.iter().find(|r| r.name == region).and_then(|r| r.patches.as_ref());
        let surfaces = &self.surfaces;
        let before = self.display.len();
        self.display.retain(|e| match &e.layer {
            Layer::Region(r) => patches(r).is_some(),
            Layer::Patch { region, patch } => patches(region).is_some_and(|p| p.iter().any(|x| x.name == *patch)),
            Layer::Group { region, group } => {
                patches(region).is_some_and(|p| p.iter().any(|x| x.groups.contains(group)))
            }
            Layer::Surface(f) | Layer::Solid { file: f, .. } => surfaces.contains(f),
        });
        if self.display.len() != before {
            self.revision += 1;
        }
    }
}
