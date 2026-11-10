//! The cut card: a plane through a region's mesh, the cells it crosses drawn whole (the scene's
//! `cuts`). The plane is across an axis at a position within the region's extent; the region's
//! boundary is hidden while the card is open, so the cut inside it shows.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use serde_json::{Value, json};
use umcf_engine::{DisplayEntry, Layer};
use umcf_render::Source;
use umcf_render::raster::Mode;

use crate::icons::{self, Icon, Ink};
use crate::{App, theme};

/// What the card holds.
#[derive(Debug, Clone, PartialEq)]
pub struct CutForm {
    pub region: String,
    /// 0, 1, 2: across x, y or z.
    pub axis: usize,
    /// Where along the axis.
    pub at: f64,
    /// The plane faces the other way: the layer of cells on its other side, where it lies on
    /// faces between them.
    pub flip: bool,
    /// The extent along the axis the slider covers (the region's, once known).
    pub range: (f64, f64),
    /// The region whose boundary the card hid, to show again.
    pub(crate) hid: Option<String>,
    /// The browser set how that region is drawn since: its choice stands, the card's hiding is
    /// gone (and not put back while the card stays on the region).
    pub(crate) released: bool,
    /// The position was set by hand: kept when the region's extent comes to be known.
    pub(crate) placed: bool,
}

impl CutForm {
    pub fn new(region: &str) -> CutForm {
        CutForm {
            region: region.to_string(),
            axis: 0,
            at: 0.0,
            flip: false,
            range: (0.0, 0.0),
            hid: None,
            released: false,
            placed: false,
        }
    }

    /// The scene's `cuts` entry.
    pub fn cut(&self) -> Value {
        let mut normal = [0.0; 3];
        let mut point = [0.0; 3];
        if let (Some(n), Some(p)) = (normal.get_mut(self.axis), point.get_mut(self.axis)) {
            *n = if self.flip { -1.0 } else { 1.0 };
            *p = self.at;
        }
        json!({ "region": self.region, "point": point, "normal": normal })
    }
}

impl App {
    /// The regions a cut can go through: those with a mesh.
    fn meshed(&self) -> Vec<String> {
        self.session
            .case()
            .map_or(Vec::new(), |c| c.regions.iter().filter(|r| r.patches.is_some()).map(|r| r.name.clone()).collect())
    }

    /// Whether a cut can be made: a region of the case has a mesh.
    pub(crate) fn can_cut(&self) -> bool {
        !self.meshed().is_empty()
    }

    /// Keeps the card's hiding entry last, after the display's entries are sorted (patches
    /// after regions), so that it still wins over the region's patches.
    pub(crate) fn keep_hiding_last(&mut self) {
        let Some(r) = self.cut.as_ref().filter(|f| !f.released).and_then(|f| f.hid.clone()) else { return };
        let layer = Layer::Region(r);
        if let Some(k) = self.display.iter().rposition(|e| e.layer == layer && e.mode == Some(Mode::Hidden)) {
            let e = self.display.remove(k);
            self.display.push(e);
        }
    }

    /// The browser sets the mode of `layer` (or resets it): where that is the region the card
    /// hides, the card's hiding goes first, so that what the browser set is what shows, with or
    /// without an entry of the region's own from before; closing the card then leaves it.
    pub(crate) fn release_hiding(&mut self, layer: &Layer, change: &crate::display::Change) {
        use crate::display::Change;
        if !matches!(change, Change::Mode(_) | Change::Reset) {
            return;
        }
        let Some(r) = self.cut.as_ref().filter(|f| !f.released).and_then(|f| f.hid.clone()) else { return };
        if *layer != Layer::Region(r.clone()) {
            return;
        }
        self.unhide(&r);
        if let Some(f) = &mut self.cut {
            f.released = true;
        }
    }

    /// Opens the cut card (on the first region with a mesh, `air` if it has one), or closes it.
    pub(crate) fn toggle_cut(&mut self) {
        if self.cut.is_some() {
            self.close_cut();
            return;
        }
        let regions = self.meshed();
        let Some(region) = regions.iter().find(|r| *r == "air").or(regions.first()).cloned() else { return };
        let mut form = CutForm::new(&region);
        self.fit_cut(&mut form, true);
        self.cut = Some(form);
        self.hide_boundary();
    }

    /// Closes the card: the cut goes, the region's boundary shows as before.
    pub(crate) fn close_cut(&mut self) {
        if let Some(r) = self.cut.take().filter(|f| !f.released).and_then(|f| f.hid) {
            self.unhide(&r);
        }
        // The view drawn again without the cut, whether or not a hiding was left to take out.
        self.look += 1;
    }

    /// The entry the card adds to hide region `region`'s boundary: last, so that it wins over
    /// the region's and its patches' own, which stay as they are.
    fn hiding(region: &str) -> DisplayEntry {
        DisplayEntry { layer: Layer::Region(region.to_string()), mode: Some(Mode::Hidden), opacity: None }
    }

    /// Takes the card's hiding out again: the region's last entry that hides it (the card's,
    /// though the browser may have given it an opacity since: that stays, the mode goes).
    fn unhide(&mut self, region: &str) {
        let layer = Layer::Region(region.to_string());
        let Some(k) = self.display.iter().rposition(|e| e.layer == layer && e.mode == Some(Mode::Hidden)) else {
            return;
        };
        match self.display.get_mut(k) {
            Some(e) if e.opacity.is_some() => e.mode = None,
            _ => {
                self.display.remove(k);
            }
        }
        self.look += 1;
    }

    /// Hides the boundary of the cut region (and shows again one hidden before for another).
    fn hide_boundary(&mut self) {
        let Some(form) = &self.cut else { return };
        let (region, before, released) = (form.region.clone(), form.hid.clone(), form.released);
        if before.as_deref() == Some(region.as_str()) {
            return;
        }
        if let Some(r) = before.filter(|_| !released) {
            self.unhide(&r);
        }
        self.display.push(Self::hiding(&region));
        if let Some(f) = &mut self.cut {
            f.hid = Some(region);
            f.released = false;
        }
        self.look += 1;
    }

    /// The slider's range: the region's extent along the axis, from its boundary in the scene
    /// built last, else from its whole mesh once read for the cut (a region turned off in the
    /// browser); the position into it (to its middle with `middle`).
    pub(crate) fn fit_cut(&self, form: &mut CutForm, middle: bool) {
        let boundary = self.session.last_scene().and_then(|scene| {
            scene.objects.iter().find_map(|o| match &o.source {
                Source::Mesh { region } if *region == form.region => o.bbox(scene.origin),
                _ => None,
            })
        });
        let extent = boundary.or_else(|| self.session.whole_bounds(&form.region));
        if let Some(b) = extent {
            let (lo, hi) = match form.axis {
                0 => (b.min.x, b.max.x),
                1 => (b.min.y, b.max.y),
                _ => (b.min.z, b.max.z),
            };
            form.range = (lo, hi);
            if (middle && !form.placed) || !(lo..=hi).contains(&form.at) {
                form.at = 0.5 * (lo + hi);
            }
        } else {
            // Not known yet (a region turned off, its whole mesh not read): none, so that the
            // card fits it once the mesh is read, not keeping another region's or axis's.
            form.range = (0.0, 0.0);
        }
    }

    /// The card; returns whether Close was pressed.
    pub(crate) fn cut_card(&mut self, ui: &mut Ui, width: f32) -> bool {
        let t = self.tokens.clone();
        let regions = self.meshed();
        let mut close = false;
        let Some(mut form) = self.cut.clone() else { return false };
        let before = form.clone();
        let muted = Ink { line: t.text_muted, fill: t.accent };
        t.card_frame().show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Cut, Ink { line: t.text, fill: t.accent });
                ui.label(RichText::new("Cut").font(theme::strong(14.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close = crate::icon_button(ui, Icon::Close, 22.0, true, false, "Close", muted, &t).clicked();
                });
            });
            ui.separator();
            egui::Grid::new("cut-form").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                let label = ui.label("Region");
                egui::ComboBox::from_id_salt("cut-region")
                    .selected_text(if form.region.is_empty() { "mesh" } else { form.region.as_str() })
                    .show_ui(ui, |ui| {
                        for r in &regions {
                            ui.selectable_value(&mut form.region, r.clone(), if r.is_empty() { "mesh" } else { r });
                        }
                    })
                    .response
                    .labelled_by(label.id);
                ui.end_row();
                ui.label("Across");
                ui.horizontal(|ui| {
                    for (k, name) in ["x", "y", "z"].iter().enumerate() {
                        ui.selectable_value(&mut form.axis, k, *name);
                    }
                    ui.checkbox(&mut form.flip, "Flip")
                        .on_hover_text("On faces between cells, show the layer on the other side of the plane");
                });
                ui.end_row();
                let label = ui.label("At");
                let (lo, hi) = form.range;
                let moved = if hi > lo {
                    // Short of the ends, where the plane has no cell behind it (or, flipped, in
                    // front): nothing to show.
                    let step = (hi - lo) / 1000.0;
                    let r = (lo + step)..=(hi - step);
                    ui.add(egui::Slider::new(&mut form.at, r).step_by(step).max_decimals(3)).labelled_by(label.id)
                } else {
                    ui.add(egui::DragValue::new(&mut form.at).speed(0.1)).labelled_by(label.id)
                };
                if moved.changed() {
                    form.placed = true;
                }
                ui.end_row();
            });
            ui.label(
                RichText::new(
                    "The cells the plane crosses, whole. The region's boundary is hidden while the cut is shown.",
                )
                .size(12.0)
                .color(t.text_muted),
            );
        });
        // Another region or axis: the slider over its extent, the plane in its middle; the
        // extent once known (the region drawn), the position kept if set by hand.
        let other = form.region != before.region || form.axis != before.axis;
        if other {
            form.placed = false;
        }
        if other || form.range.1 <= form.range.0 {
            self.fit_cut(&mut form, true);
        }
        if form != before {
            self.set_cut(form);
            ui.ctx().request_repaint();
        }
        close
    }

    /// Takes the card's new state: the region's boundary hidden, and the view drawn again (its
    /// image is kept by the case's revision and the look, which a moved plane leaves as they
    /// were).
    pub(crate) fn set_cut(&mut self, form: CutForm) {
        self.cut = Some(form);
        self.hide_boundary();
        self.look += 1;
    }
}
