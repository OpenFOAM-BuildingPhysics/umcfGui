//! How the 3D view draws each layer: a region's mesh, a patch, a patch group, a surface or one
//! of its regions as surfaces, wireframe or points, or hidden, each with its own opacity. Set
//! from the browser's context menus, kept as `display` entries (as `scene.snapshot` takes them)
//! and sent with the view's picks, so that what is hidden is not picked.

use eframe::egui::{self, Ui};
use serde_json::Value;
use umcf_engine::{DisplayEntry, Layer};
use umcf_render::Source;
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

/// Whether an entry of `outer` applies to `inner` (`inner_groups`: its groups, if it is a patch):
/// a region holds its patches and patch groups, a group its patches, a surface its regions. The
/// one rule for what is drawn ([`App::layer_display`]) and what a change takes over
/// ([`App::set_display`]); the engine's `display_style` applies the same to a scene's classes.
fn holds(outer: &Layer, inner: &Layer, inner_groups: &[String]) -> bool {
    match (outer, inner) {
        (Layer::Region(r), Layer::Patch { region, .. } | Layer::Group { region, .. }) => r == region,
        (Layer::Group { region: r, group }, Layer::Patch { region, .. }) => r == region && inner_groups.contains(group),
        (Layer::Surface(f), Layer::Solid { file, .. }) => f == file,
        _ => false,
    }
}

/// How deep `layer` is: a region or surface holds groups and patches or surface regions, a
/// group holds patches.
fn depth(layer: &Layer) -> u8 {
    match layer {
        Layer::Region(_) | Layer::Surface(_) | Layer::Cut(_) => 0,
        Layer::Group { .. } => 1,
        Layer::Patch { .. } | Layer::Solid { .. } => 2,
    }
}

impl App {
    /// The entry of `layer` itself, if it has one.
    pub fn layer_entry(&self, layer: &Layer) -> Option<&DisplayEntry> {
        self.display.iter().find(|e| e.layer == *layer)
    }

    /// A short note of how `layer`'s own entries draw it, for the browser, as the view draws it:
    /// the last mode and the last opacity set (a region's own entry and the cut card's hiding,
    /// the hiding left out while the card shows nothing); empty without one.
    pub(crate) fn layer_note(&self, layer: &Layer) -> String {
        let idle = self.idle_hiding();
        let mut own = DisplayEntry { layer: layer.clone(), mode: None, opacity: None };
        let mut any = false;
        for (k, e) in self.display.iter().enumerate().filter(|(_, e)| e.layer == *layer) {
            any = true;
            if let Some(m) = e.mode.filter(|_| idle != Some(k)) {
                own.mode = Some(m);
            }
            if let Some(a) = e.opacity {
                own.opacity = Some(a);
            }
        }
        if any { note(&own) } else { String::new() }
    }

    /// How `layer` is drawn: after every entry the view draws with ([`App::view_display`]: the
    /// cut card's hiding only while it shows something), as for one of its classes (`groups`: a
    /// patch's groups).
    pub(crate) fn layer_display(&self, layer: &Layer, groups: &[String]) -> Display {
        let mut d = Display::default();
        let idle = self.idle_hiding();
        for (k, e) in
            self.display.iter().enumerate().filter(|(_, e)| e.layer == *layer || holds(&e.layer, layer, groups))
        {
            if let Some(m) = e.mode.filter(|_| idle != Some(k)) {
                d.mode = m;
            }
            if let Some(a) = e.opacity {
                d.opacity = a;
            }
        }
        d
    }

    /// Changes how `layer` is drawn. What it sets governs what it holds: a region's mode or
    /// opacity is taken from the entries of its patches and patch groups (a group's from its
    /// patches', a surface's from its regions'), so that all of them are drawn as it is; an
    /// entry left with nothing to set goes. The entries apply in order: the layer's goes last among
    /// those of its kind, after those that hold it, so that what was set last wins: a patch set
    /// after its region keeps what it set until the region, or a group of the patch, sets the
    /// same again. Two groups of one patch hold neither the other: the one changed last wins,
    /// for its mode and opacity.
    pub fn set_display(&mut self, layer: Layer, change: Change) {
        // For the region the cut card hides while it shows a cut: the cut's.
        if let Some(cut) = self.cut_takes(&layer) {
            self.set_cut_display(cut, &change);
            return;
        }
        self.release_hiding(&layer, &change);
        if let Change::Mode(_) | Change::Opacity(_) = change {
            // Only a group needs its patches' groups.
            let groups = if matches!(layer, Layer::Group { .. }) { self.patch_groups() } else { Default::default() };
            let groups_of = |e: &Layer| match e {
                Layer::Patch { region, patch } => groups.get(&(region.clone(), patch.clone())).map(Vec::as_slice),
                _ => None,
            };
            for e in
                self.display.iter_mut().filter(|e| holds(&layer, &e.layer, groups_of(&e.layer).unwrap_or_default()))
            {
                match change {
                    Change::Mode(_) => e.mode = None,
                    Change::Opacity(_) => e.opacity = None,
                    Change::Reset => {}
                }
            }
            self.display.retain(|e| e.layer == layer || e.mode.is_some() || e.opacity.is_some());
        }
        let at = self.display.iter().position(|e| e.layer == layer);
        match (change, at) {
            (Change::Reset, Some(k)) => {
                self.display.remove(k);
            }
            (Change::Reset, None) => return,
            (c, Some(k)) => {
                let mut e = self.display.remove(k);
                match c {
                    Change::Mode(m) => e.mode = Some(m),
                    Change::Opacity(a) => e.opacity = Some(a),
                    Change::Reset => {}
                }
                self.display.push(e);
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
        // What holds before what it holds, the newest last among equals: what an entry still
        // has was set after what holds it last set the same (that set took it off), so it wins.
        self.display.sort_by_key(|e| depth(&e.layer));
        self.keep_hiding_last();
        self.look += 1;
    }

    /// Each patch's groups (`inGroups`), by region and patch.
    fn patch_groups(&self) -> std::collections::HashMap<(String, String), Vec<String>> {
        let mut out = std::collections::HashMap::new();
        for r in self.session.case().map(|c| c.regions.as_slice()).unwrap_or_default() {
            for p in r.patches.iter().flatten() {
                out.insert((r.name.clone(), p.name.clone()), p.groups.clone());
            }
        }
        out
    }

    /// The entries as commands take them: those of what the view shows (an entry of a surface
    /// region gone from its file, say, would have the command refused).
    pub(crate) fn display_json(&mut self) -> Value {
        let params = self.shown();
        let entries = self.view_display();
        let Ok(scene) = self.session.scene(&params) else { return Value::Array(Vec::new()) };
        let shown = |layer: &Layer| {
            scene.objects.iter().any(|o| match (layer, &o.source) {
                (Layer::Region(r), Source::Mesh { region }) => r == region,
                (Layer::Patch { region, patch }, Source::Mesh { region: r }) => {
                    region == r && o.classes.contains(patch)
                }
                (Layer::Group { region, group }, Source::Mesh { region: r }) => {
                    region == r && o.groups.iter().any(|g| g.contains(group))
                }
                (Layer::Surface(f), Source::Surface { file }) => f == file,
                (Layer::Solid { file, solid }, Source::Surface { file: f }) => file == f && o.classes.contains(solid),
                _ => false,
            })
        };
        entries.iter().filter(|e| shown(&e.layer)).map(DisplayEntry::to_json).collect()
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
            Layer::Region(r) | Layer::Cut(r) => patches(r).is_some(),
            Layer::Patch { region, patch } => patches(region).is_some_and(|p| p.iter().any(|x| x.name == *patch)),
            Layer::Group { region, group } => {
                patches(region).is_some_and(|p| p.iter().any(|x| x.groups.contains(group)))
            }
            Layer::Surface(f) | Layer::Solid { file: f, .. } => surfaces.contains(f),
        });
        if self.display.len() != before {
            self.look += 1;
        }
    }
}
