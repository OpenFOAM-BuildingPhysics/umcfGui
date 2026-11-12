//! The *Box mesh* card (`mesh.box`): its form filled from the region's box, and while it is
//! open the box it makes previewed in the 3D view (its sides, its cells) with its size and
//! cells said on the card, before anything is written.

use umcf_geom::BBox;
use umcf_model::mesh_box::{BoxCells, MeshBox};
use umcf_render::guides::{BlockPreview, PREVIEW_LINES};

use crate::App;

/// The bounds of a set of surfaces, kept while they and the case's geometry don't change
/// ([`App::geometry_revision`]).
pub(crate) type Bounds = (Vec<String>, u64, Result<BBox, String>);

/// A number as the form reads one (JSON's syntax: what OK sends).
fn number(text: &str) -> Option<f64> {
    serde_json::from_str::<f64>(text.trim()).ok().filter(|x| x.is_finite())
}

/// The parts of a [`crate::forms::Kind::Numbers`] field's text.
fn numbers(text: &str) -> Vec<Option<f64>> {
    text.split('|').map(number).collect()
}

impl App {
    /// Each frame: while the box card shows its form, its fields follow the region chosen (the
    /// region's box, when it has one), and the view previews the box; else nothing previewed.
    pub(crate) fn box_card_frame(&mut self) {
        let open = self.dialog.as_ref().is_some_and(|d| d.id == "mesh.box" && d.form.as_ref().is_some_and(|f| !f.json));
        if !open {
            self.viewport.blocks = None;
            self.box_summary = None;
            return;
        }
        self.box_prefill();
        let (preview, summary) = match self.box_plan() {
            Ok((axes, line)) => (Some(axes), Ok(line)),
            Err(e) => (None, Err(e)),
        };
        if self.viewport.blocks != preview {
            self.viewport.blocks = preview;
        }
        self.box_summary = Some(summary);
    }

    /// Fills the form from the region's box when the region chosen changed: its surfaces,
    /// margins and cells (left empty without a box).
    fn box_prefill(&mut self) {
        let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()) else { return };
        let region = f.values.get("region").cloned().unwrap_or_default();
        if f.filled_for.as_deref() == Some(region.as_str()) {
            return;
        }
        f.filled_for = Some(region.clone());
        let found = self
            .session
            .case()
            .and_then(|c| c.description())
            .and_then(|(d, _)| d.region(&region).and_then(|r| r.mesh_box.clone()));
        let set = f.lists.entry("surfaces").or_default();
        set.clear();
        let (margin, size, cells) = match &found {
            Some(b) => {
                set.extend(b.surfaces.iter().cloned());
                let m: Vec<String> = b.margin.iter().map(|x| umcf_model::blocks::number(*x)).collect();
                let (size, cells) = match &b.cells {
                    BoxCells::Size(h) => (umcf_model::blocks::number(*h), String::new()),
                    BoxCells::Counts(n) => (String::new(), n.map(|c| c.to_string()).join("|")),
                };
                (m.join("|"), size, cells)
            }
            None => (String::new(), String::new(), String::new()),
        };
        f.values.insert("margin", margin);
        f.values.insert("cellSize", size);
        f.values.insert("cells", cells);
        f.initial = f.values.clone();
    }

    /// The box the form makes: the preview of its grid and a line saying its size and cells;
    /// or what keeps it from being made.
    fn box_plan(&mut self) -> Result<(BlockPreview, String), String> {
        let (region, surfaces, margin, size, cells) = {
            let Some(f) = self.dialog.as_ref().and_then(|d| d.form.as_ref()) else { return Err(String::new()) };
            let v = |k: &str| f.values.get(k).cloned().unwrap_or_default();
            let ticked: Vec<String> = f.lists.get("surfaces").map(|s| s.iter().cloned().collect()).unwrap_or_default();
            (v("region"), ticked, v("margin"), v("cellSize"), v("cells"))
        };
        if region.is_empty() {
            return Err("Choose the region.".to_string());
        }
        let case = self.session.case().ok_or_else(String::new)?;
        // None ticked: the region's box's, else every surface (as the command takes them).
        let surfaces = if surfaces.is_empty() {
            let boxed = case
                .description()
                .and_then(|(d, _)| d.region(&region).and_then(|r| r.mesh_box.as_ref().map(|b| b.surfaces.clone())));
            boxed.unwrap_or_else(|| self.form_surfaces())
        } else {
            surfaces
        };
        if surfaces.is_empty() {
            return Err("No STL surface in constant/triSurface to put a box around.".to_string());
        }
        let m = numbers(&margin);
        let margin: [f64; 6] = match m.as_slice() {
            [Some(a), Some(b), Some(c), Some(d), Some(e), Some(g)] => [*a, *b, *c, *d, *e, *g],
            _ => return Err("Give the six margins to see the box.".to_string()),
        };
        // The cells' field holds its three parts (`||` when none is given).
        let counted = cells.split('|').any(|p| !p.trim().is_empty());
        let cells = match (number(&size), if counted { cells.trim() } else { "" }) {
            (Some(h), "") => BoxCells::Size(h),
            (None, c) if !c.is_empty() => {
                let n: Vec<Option<u32>> = c.split('|').map(|p| p.trim().parse::<u32>().ok()).collect();
                match n.as_slice() {
                    [Some(x), Some(y), Some(z)] => BoxCells::Counts([*x, *y, *z]),
                    _ => return Err("Give the cells along x, y and z, or a cell size.".to_string()),
                }
            }
            (Some(_), _) => return Err("Give a cell size or cells, not both.".to_string()),
            (None, _) => return Err("Give a cell size or the cells along each axis.".to_string()),
        };
        let b = MeshBox { surfaces: surfaces.clone(), margin, cells, made: None };
        let (ground, _, _) = umcf_model::blocks::ground_and_top(case, &region);
        let bbox = self.box_bounds_of(&surfaces)?;
        let at = |v: umcf_geom::Vec3| [v.x, v.y, v.z];
        let axes = b.axes(at(bbox.min), at(bbox.max), ground)?;
        let mut preview = BlockPreview::default();
        let mut sizes = Vec::new();
        let mut counts = Vec::new();
        let mut thinned = 1usize;
        for (k, a) in axes.iter().enumerate() {
            let (Some(lo), Some(hi), Some(span)) = (a.stations.first(), a.stations.last(), a.spans.first()) else {
                continue;
            };
            let n = span.cells.max(1);
            // At most about PREVIEW_LINES positions made, whatever the cells.
            let step = (n as usize).div_ceil(PREVIEW_LINES).max(1);
            thinned = thinned.max(step);
            let at: Vec<f64> =
                (0..=n as usize).step_by(step).map(|i| lo + (hi - lo) * i as f64 / f64::from(n)).collect();
            if let (Some(c), Some(s)) = (preview.cells.get_mut(k), preview.stations.get_mut(k)) {
                *c = at;
                *s = vec![*lo, *hi];
            }
            sizes.push(umcf_model::blocks::number(((hi - lo) * 1000.0).round() / 1000.0));
            counts.push(u64::from(n));
        }
        let total: u64 = counts.iter().product();
        let line = format!(
            "{} m · {} = {} cells; the ground: {}",
            sizes.join(" × "),
            counts.iter().map(u64::to_string).collect::<Vec<_>>().join(" × "),
            total,
            ground.name()
        );
        let line = if thinned > 1 {
            format!("{line} (the view draws a line every {thinned} cells along the longest axis)")
        } else {
            line
        };
        Ok((preview, line))
    }

    /// The bounds of `surfaces`, read again only when they or the case's geometry changed
    /// (a geometry command, an undo or redo, another case): not as the view changes.
    fn box_bounds_of(&mut self, surfaces: &[String]) -> Result<BBox, String> {
        let revision = self.geometry_revision;
        if let Some((s, r, b)) = &self.box_bounds
            && s.as_slice() == surfaces
            && *r == revision
        {
            return b.clone();
        }
        let b = self.session.surface_bounds(surfaces).map_err(|e| e.to_string());
        self.box_bounds = Some((surfaces.to_vec(), revision, b.clone()));
        b
    }
}
