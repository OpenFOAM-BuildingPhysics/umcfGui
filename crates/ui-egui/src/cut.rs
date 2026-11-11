//! The cut card: the inside of a region's mesh (the scene's `cuts`): a plane through it, the
//! cells it crosses drawn whole; the faces and cells *Check mesh* found there; the layers of
//! cells at one of its patches; or the cells near an STL surface, by size. The plane is across an axis at a position within the region's
//! extent; the region's boundary is hidden while the card is open, so what is inside it shows.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use serde_json::{Value, json};
use umcf_engine::{DisplayEntry, Layer};
use umcf_render::Source;
use umcf_render::raster::Mode;

use crate::icons::{self, Icon, Ink};
use crate::{App, theme};

/// What the card shows of the region's cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// The cells a plane crosses.
    Plane,
    /// What *Check mesh* found.
    Found,
    /// The layers of cells at a patch.
    Layers,
    /// The cells near an STL surface.
    Near,
}

/// What the card holds.
#[derive(Debug, Clone, PartialEq)]
pub struct CutForm {
    pub region: String,
    pub show: Show,
    /// The patch whose layers are shown (with [`Show::Layers`]).
    pub patch: String,
    /// How many layers.
    pub layers: u32,
    /// The surface the cells near which are shown (case-relative, with [`Show::Near`]).
    pub surface: String,
    /// How near, in metres.
    pub distance: f64,
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
    /// The region's extent (world), once known: the plane drawn over it in the view.
    pub(crate) extent: Option<umcf_geom::BBox>,
}

impl CutForm {
    pub fn new(region: &str) -> CutForm {
        CutForm {
            region: region.to_string(),
            show: Show::Plane,
            patch: String::new(),
            layers: umcf_engine::DEFAULT_LAYERS,
            surface: String::new(),
            distance: 1.0,
            axis: 0,
            at: 0.0,
            flip: false,
            range: (0.0, 0.0),
            hid: None,
            released: false,
            placed: false,
            extent: None,
        }
    }

    /// The scene's `cuts` entry.
    pub fn cut(&self) -> Value {
        match self.show {
            Show::Found => return json!({ "region": self.region, "found": true }),
            Show::Layers => return json!({ "region": self.region, "patch": self.patch, "layers": self.layers }),
            Show::Near => {
                return json!({ "region": self.region, "surface": self.surface, "distance": self.distance });
            }
            Show::Plane => {}
        }
        let mut normal = [0.0; 3];
        let mut point = [0.0; 3];
        if let (Some(n), Some(p)) = (normal.get_mut(self.axis), point.get_mut(self.axis)) {
            *n = if self.flip { -1.0 } else { 1.0 };
            *p = self.at;
        }
        json!({ "region": self.region, "point": point, "normal": normal })
    }

    /// The plane as the view draws it to be dragged, once the region's extent is known (none
    /// while what a check found or a patch's layers are shown).
    pub fn plane(&self) -> Option<umcf_render::guides::CutPlane> {
        if self.show != Show::Plane {
            return None;
        }
        let extent = self.extent?;
        let mut axis = [0.0; 3];
        let mut point = [extent.centre().x, extent.centre().y, extent.centre().z];
        if let (Some(n), Some(p)) = (axis.get_mut(self.axis), point.get_mut(self.axis)) {
            *n = if self.flip { -1.0 } else { 1.0 };
            *p = self.at;
        }
        Some(umcf_render::guides::CutPlane {
            point: umcf_geom::Vec3::new(point[0], point[1], point[2]),
            normal: umcf_geom::Vec3::new(axis[0], axis[1], axis[2]),
            extent,
            active: false,
        })
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

    /// The card's `cuts` entry for the scene, when there is something to show: what a check
    /// found only once there is a check of the region, layers once a patch of it is chosen.
    pub(crate) fn cut_shown(&self) -> Option<Value> {
        let form = self.cut.as_ref()?;
        match form.show {
            Show::Found if self.session.found(&form.region).is_none() => return None,
            Show::Layers if !self.patches(&form.region).contains(&form.patch) => return None,
            Show::Near if !self.surfaces.contains(&form.surface) => return None,
            _ => {}
        }
        Some(form.cut())
    }

    /// The patches of region `region`'s mesh, the walls first (where layers are grown).
    pub(crate) fn patches(&self, region: &str) -> Vec<String> {
        let Some(patches) = self
            .session
            .case()
            .and_then(|c| c.regions.iter().find(|r| r.name == region))
            .and_then(|r| r.patches.as_ref())
        else {
            return Vec::new();
        };
        let wall = |p: &&umcf_model::Patch| p.kind.to_lowercase().contains("wall");
        patches.iter().filter(wall).chain(patches.iter().filter(|p| !wall(p))).map(|p| p.name.clone()).collect()
    }

    /// The card's surface kept while the case has it, else its first.
    fn fit_surface(&self, form: &mut CutForm) {
        if !self.surfaces.contains(&form.surface) {
            form.surface = self.surfaces.first().cloned().unwrap_or_default();
        }
    }

    /// Switches the open card to the cells near a surface of the case (its first).
    pub(crate) fn show_near(&mut self) {
        let Some(mut form) = self.cut.clone() else { return };
        form.show = Show::Near;
        self.fit_surface(&mut form);
        self.set_cut(form);
    }

    /// The card's patch kept where the region has it, else its first (a wall where it has one).
    fn fit_patch(&self, form: &mut CutForm) {
        let patches = self.patches(&form.region);
        if !patches.contains(&form.patch) {
            form.patch = patches.into_iter().next().unwrap_or_default();
        }
    }

    /// The display entries the view draws with: without the card's hiding of its region while
    /// the card has nothing to show (no surface, a patch or check missing), so that the region's
    /// boundary shows then rather than an empty view.
    pub(crate) fn view_display(&self) -> Vec<DisplayEntry> {
        let mut out = self.display.clone();
        if let Some(k) = self.idle_hiding() {
            match out.get_mut(k) {
                Some(e) if e.opacity.is_some() => e.mode = None,
                _ => {
                    out.remove(k);
                }
            }
        }
        out
    }

    /// Where in `display` the card's hiding of its region is while the card has nothing to show
    /// (and so the view draws without its mode); `None` else.
    pub(crate) fn idle_hiding(&self) -> Option<usize> {
        if self.cut_shown().is_some() {
            return None;
        }
        let r = self.cut.as_ref().filter(|f| !f.released).and_then(|f| f.hid.clone())?;
        let layer = Layer::Region(r);
        self.display.iter().rposition(|e| e.layer == layer && e.mode == Some(Mode::Hidden))
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

    /// Switches the open card to what *Check mesh* found, on the first region it found something
    /// in (else the card's region, which says it wasn't checked or that nothing was found).
    pub(crate) fn show_found(&mut self) {
        let Some(mut form) = self.cut.clone() else { return };
        let regions = self.meshed();
        let with = regions.iter().find(|r| self.session.found(r).is_some_and(|f| !f.classes.is_empty()));
        if let Some(r) = with {
            form.region = r.clone();
        }
        form.show = Show::Found;
        self.fit_cut(&mut form, true);
        self.set_cut(form);
    }

    /// Switches the open card to the layers of cells at a patch of its region (a wall where it
    /// has one).
    pub(crate) fn show_layers(&mut self) {
        let Some(mut form) = self.cut.clone() else { return };
        form.show = Show::Layers;
        self.fit_patch(&mut form);
        self.set_cut(form);
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
            form.extent = Some(b);
            if (middle && !form.placed) || !(lo..=hi).contains(&form.at) {
                form.at = 0.5 * (lo + hi);
            }
        } else {
            // Not known yet (a region turned off, its whole mesh not read): none, so that the
            // card fits it once the mesh is read, not keeping another region's or axis's.
            form.range = (0.0, 0.0);
            form.extent = None;
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
            let found = self.session.found(&form.region);
            let is_check = |w: &crate::jobs::Work| matches!(w, crate::jobs::Work::Command { id, .. } if id == "mesh.check");
            let checking = self.job.as_ref().is_some_and(|j| is_check(&j.work)) || self.queue.iter().any(is_check);
            let (mut check, mut fit) = (false, false);
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
            });
            // What to show: a row of its own, which wraps on a narrow card.
            ui.horizontal_wrapped(|ui| {
                ui.label("Show");
                ui.selectable_value(&mut form.show, Show::Plane, "Plane")
                    .on_hover_text("The cells a plane crosses, whole");
                ui.selectable_value(&mut form.show, Show::Found, "Check findings")
                    .on_hover_text("The faces and cells Check mesh found in the region");
                ui.selectable_value(&mut form.show, Show::Layers, "Wall layers")
                    .on_hover_text("The layers of cells at a patch, each in its colour");
                ui.selectable_value(&mut form.show, Show::Near, "Near a surface")
                    .on_hover_text("The cells near an STL surface, each size in its colour");
            });
            egui::Grid::new("cut-form-2").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                if form.show == Show::Layers {
                    let label = ui.label("Patch");
                    egui::ComboBox::from_id_salt("cut-patch")
                        .selected_text(form.patch.as_str())
                        .show_ui(ui, |ui| {
                            for p in self.patches(&form.region) {
                                ui.selectable_value(&mut form.patch, p.clone(), p);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label("Layers");
                    ui.add(egui::DragValue::new(&mut form.layers).range(1..=umcf_engine::MAX_LAYERS).speed(0.05))
                        .on_hover_text("How many layers to follow from the patch")
                        .labelled_by(label.id);
                    ui.end_row();
                }
                if form.show == Show::Near {
                    let label = ui.label("Surface");
                    let name = |f: &str| f.rsplit('/').next().unwrap_or(f).to_string();
                    egui::ComboBox::from_id_salt("cut-surface")
                        .selected_text(name(&form.surface))
                        .show_ui(ui, |ui| {
                            for f in &self.surfaces {
                                ui.selectable_value(&mut form.surface, f.clone(), name(f));
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                    let label = ui.label("Within");
                    ui.add(egui::DragValue::new(&mut form.distance).range(0.001..=100_000.0).speed(0.05).suffix(" m"))
                        .on_hover_text("The cells with a point this near the surface")
                        .labelled_by(label.id);
                    ui.end_row();
                }
                if form.show != Show::Plane {
                    return;
                }
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
            if matches!(form.show, Show::Layers | Show::Near) {
                // The layers (or sizes) as the scene built them last (each class's name says its count), once
                // they are those of the card's patch and count (the scene is built after it,
                // as a job: till then, or where it failed, another cut's are there).
                let wanted = match form.show {
                    Show::Near => umcf_render::Cells::Near { surface: form.surface.clone(), distance: form.distance },
                    _ => umcf_render::Cells::Layers { patch: form.patch.clone(), layers: form.layers },
                };
                let built = self.session.last_cut(&form.region) == Some(&wanted);
                let shown = self.session.last_scene().filter(|_| built).and_then(|scene| {
                    scene.objects.iter().find(|o| matches!(&o.source, Source::Cut { region } if *region == form.region))
                });
                let classes = shown.map(|o| o.classes.clone());
                if form.show == Show::Layers && self.patches(&form.region).is_empty() {
                    ui.add(egui::Label::new("This region's mesh has no patches to show layers at.").wrap());
                }
                if form.show == Show::Near && self.surfaces.is_empty() {
                    ui.add(egui::Label::new("The case has no STL surface in constant/triSurface.").wrap());
                }
                match classes {
                    Some(classes) if !classes.is_empty() => {
                        // Up to 50 layers: scrolled, so the card stays within the window.
                        egui::ScrollArea::vertical().id_salt("cut-layers").max_height(160.0).show(ui, |ui| {
                            for (k, c) in classes.iter().enumerate() {
                                ui.horizontal_top(|ui| {
                                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                                    let [red, green, blue] =
                                        umcf_render::raster::palette(k).map(|x| (x * 255.0) as u8);
                                    ui.painter().rect_filled(r, 2.0, egui::Color32::from_rgb(red, green, blue));
                                    ui.add(egui::Label::new(c.as_str()).wrap());
                                });
                            }
                        });
                    }
                    Some(_) => {
                        ui.label(if form.show == Show::Near {
                            "No cells that near the surface."
                        } else {
                            "No cells at this patch."
                        });
                    }
                    None => {}
                }
                let any = built
                    && self.session.last_scene().is_some_and(|scene| {
                    scene.objects.iter().any(|o| {
                        matches!(&o.source, Source::Cut { region } if *region == form.region) && !o.triangles.is_empty()
                    })
                });
                fit |= ui
                    .add_enabled(any, egui::Button::new("Fit cells"))
                    .on_hover_text("Frame the view on the cells shown")
                    .clicked();
            }
            if form.show == Show::Found {
                match found {
                    Some(f) if f.classes.is_empty() => {
                        ui.label("Check mesh found nothing here.");
                    }
                    Some(f) => {
                        for (k, c) in f.classes.iter().enumerate() {
                            ui.horizontal_top(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                                let [red, green, blue] = umcf_render::raster::palette(k).map(|x| (x * 255.0) as u8);
                                ui.painter().rect_filled(r, 2.0, egui::Color32::from_rgb(red, green, blue));
                                let what = if c.cells { "cell" } else { "face" };
                                let (shown, n) = (c.labels.len(), c.count.max(c.labels.len()));
                                let plural = if n == 1 { "" } else { "s" };
                                let text = if shown < n {
                                    format!("{}: {n} {what}{plural}, the first {shown} shown", c.name)
                                } else {
                                    format!("{}: {n} {what}{plural}", c.name)
                                };
                                ui.add(egui::Label::new(text).wrap());
                            });
                        }
                    }
                    None => {
                        ui.add(
                            egui::Label::new(
                                "No check of this region to show: Check mesh hasn't run on it since it was read, or couldn't read its mesh.",
                            )
                            .wrap(),
                        );
                    }
                }
                ui.horizontal(|ui| {
                    let label = if checking { "Checking…" } else { "Check mesh" };
                    let tip = "Check this region's mesh (its whole mesh is read; what it finds is shown here)";
                    check = ui.add_enabled(!checking, egui::Button::new(label)).on_hover_text(tip).clicked();
                    let any = found.is_some_and(|f| !f.classes.is_empty());
                    fit = ui
                        .add_enabled(any, egui::Button::new("Fit findings"))
                        .on_hover_text("Frame the view on what was found")
                        .clicked();
                });
            }
            let note = match form.show {
                Show::Found => {
                    "The faces and cells Check mesh found, each problem in its colour, a cell by its faces (the first 1000 of each). The region's boundary is hidden while they are shown."
                }
                Show::Layers => {
                    "From each face of the patch, the cells across their opposite faces, each layer in its colour; where that stops short (a cell split or collapsed), the patch face is marked. Past the mesh's layers, the cells beyond count as layers too. The region's boundary is hidden while they are shown."
                }
                Show::Near => {
                    "The cells with a point within the distance of the surface, each size in its colour (halvings of the largest, as snappyHexMesh refines). The region's boundary is hidden while they are shown."
                }
                Show::Plane => "The cells the plane crosses, whole. The region's boundary is hidden while the cut is shown.",
            };
            ui.label(RichText::new(note).size(12.0).color(t.text_muted));
            if check {
                self.start(crate::jobs::Work::Command {
                    id: "mesh.check".to_string(),
                    params: json!({ "region": form.region }),
                    then: crate::jobs::Then::Nothing,
                });
            }
            if fit {
                self.fit_findings();
            }
        });
        // Another region or axis: the slider over its extent, the plane in its middle; the
        // extent once known (the region drawn), the position kept if set by hand.
        if form.show == Show::Layers {
            self.fit_patch(&mut form);
        }
        if form.show == Show::Near {
            self.fit_surface(&mut form);
        }
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

    /// The plane dragged in the view to pass through `point`: the position along the card's
    /// axis, within the slider's range.
    pub(crate) fn move_cut(&mut self, point: umcf_geom::Vec3) {
        let Some(mut form) = self.cut.clone() else { return };
        let mut at = [point.x, point.y, point.z].get(form.axis).copied().unwrap_or(form.at);
        let (lo, hi) = form.range;
        if hi > lo && lo.is_finite() && hi.is_finite() {
            let step = (hi - lo) / 1000.0;
            at = at.max(lo + step).min(hi - step);
        }
        if at.is_finite() && at != form.at {
            form.at = at;
            form.placed = true;
            self.set_cut(form);
        }
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
