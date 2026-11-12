//! The *Box mesh* card (`mesh.box`): its form filled from the region's box, and while it is
//! open the box it makes previewed in the 3D view (its sides, its cells) with its size and
//! cells said on the card, before anything is written.

use serde_json::Value;
use umcf_geom::BBox;
use umcf_model::blocks::Side;
use umcf_model::mesh_box::{BoxCells, MeshBox};
use umcf_render::guides::{BlockPreview, PREVIEW_LINES};

use crate::App;

/// The bounds of a set of surfaces, kept while they and the case's geometry don't change
/// ([`App::geometry_revision`]).
pub(crate) type Bounds = (Vec<String>, u64, Result<BBox, String>);

impl App {
    /// Each frame: while the box card shows its form, its fields follow the region chosen (the
    /// region's box, when it has one), and the view previews the box; else nothing previewed.
    pub(crate) fn box_card_frame(&mut self) {
        let open = self.dialog.as_ref().is_some_and(|d| d.id == "mesh.box" && d.form.as_ref().is_some_and(|f| !f.json));
        if !open {
            self.viewport.blocks = None;
            self.box_summary = None;
            // A read that failed or was cancelled is tried again when the card opens again.
            if self.box_bounds.as_ref().is_some_and(|(_, _, b)| b.is_err()) {
                self.box_bounds = None;
            }
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
        f.initial_lists = f.lists.clone();
    }

    /// The box the form makes, from the parameters OK sends (the fields and what *Edit as
    /// JSON* kept), with what the command takes for what they leave out (the region's box):
    /// the preview of its grid and a line saying its size and cells; or what keeps it from
    /// being made.
    fn box_plan(&mut self) -> Result<(BlockPreview, String), String> {
        let params = {
            let Some(f) = self.dialog.as_ref().and_then(|d| d.form.as_ref()) else { return Err(String::new()) };
            let spec = crate::forms::spec("mesh.box").ok_or_else(String::new)?;
            f.params(spec)?
        };
        let case = self.session.case().ok_or_else(String::new)?;
        let Some((description, _)) = case.description() else {
            return Err(
                "The case has no workflow description: Write workflow description (Case › More) first.".to_string()
            );
        };
        let region = params.get("region").and_then(Value::as_str).unwrap_or_default().to_string();
        if region.is_empty() {
            return Err("Choose the region.".to_string());
        }
        let Some(r) = description.region(&region) else {
            return Err(format!("Region {region} isn't in system/umcfWorkflow."));
        };
        let had = r.mesh_box.clone();
        let (ground, _, _) = umcf_model::blocks::ground_and_top(case, &region);
        let bad = |what: &str| format!("{what}: as OK would send it, the command refuses it");
        // The surfaces: those given, else the box's, else every surface (as the command).
        let surfaces: Vec<String> = match params.get("surfaces") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Some(_) => return Err(bad("Surfaces")),
            None => had.as_ref().map(|b| b.surfaces.clone()).unwrap_or_else(|| self.form_surfaces()),
        };
        if surfaces.is_empty() {
            return Err("No STL surface in constant/triSurface to put a box around.".to_string());
        }
        // One margin: every side but the ground, which is 0 (as the command).
        let margin: [f64; 6] = match params.get("margin") {
            Some(Value::Number(n)) => {
                let m = n.as_f64().ok_or_else(|| bad("Margins"))?;
                let mut out = [m; 6];
                if let Some(slot) = Side::ALL.iter().position(|s| *s == ground).and_then(|i| out.get_mut(i)) {
                    *slot = 0.0;
                }
                out
            }
            Some(Value::Array(a)) => {
                let v: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                match v.as_slice() {
                    // Each checked by the box itself (MeshBox::problems).
                    [a, b, c, d, e, g] => [*a, *b, *c, *d, *e, *g],
                    _ => return Err(bad("Margins")),
                }
            }
            Some(_) => return Err(bad("Margins")),
            None => match &had {
                Some(b) => b.margin,
                None => return Err("Give the six margins to see the box.".to_string()),
            },
        };
        let cells = match (params.get("cellSize"), params.get("cells")) {
            (Some(_), Some(_)) => return Err("Give a cell size or cells, not both.".to_string()),
            (Some(h), None) => BoxCells::Size(h.as_f64().ok_or_else(|| bad("Cell size"))?),
            (None, Some(c)) => {
                let n: Option<Vec<u32>> = c
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .and_then(|a| a.iter().map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok())).collect());
                match n.as_deref() {
                    Some([x, y, z]) => BoxCells::Counts([*x, *y, *z]),
                    _ => return Err(bad("Cells")),
                }
            }
            (None, None) => match &had {
                Some(b) => b.cells.clone(),
                None => return Err("Give a cell size or the cells along each axis.".to_string()),
            },
        };
        let b = MeshBox { surfaces: surfaces.clone(), margin, cells, made: None };
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
            format!("{line} (the view draws a line every {thinned} cells along the axis with the most)")
        } else {
            line
        };
        Ok((preview, line))
    }

    /// The bounds of `surfaces`, read again only when they or the case's geometry changed
    /// (a geometry command, an undo or redo, another case): not as the view changes. Read as a
    /// job (the window draws meanwhile; *Cancel* stops it): until it is done, the card says so.
    fn box_bounds_of(&mut self, surfaces: &[String]) -> Result<BBox, String> {
        let revision = self.geometry_revision;
        if let Some((s, r, b)) = &self.box_bounds
            && s.as_slice() == surfaces
            && *r == revision
        {
            return b.clone();
        }
        let asked = |w: &crate::jobs::Work| matches!(w, crate::jobs::Work::Bounds { surfaces: s, revision: r } if s.as_slice() == surfaces && *r == revision);
        let running = self.job.as_ref().is_some_and(|j| asked(&j.work)) || self.queue.iter().any(asked);
        if !running {
            self.start(crate::jobs::Work::Bounds { surfaces: surfaces.to_vec(), revision });
        }
        Err("Reading the surfaces…".to_string())
    }
}
