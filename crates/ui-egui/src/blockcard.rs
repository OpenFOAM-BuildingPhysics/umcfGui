//! The *Block mesh* card (`mesh.block`): its form filled from the region's grid of blocks, and
//! while it is open the grid it makes previewed in the 3D view (its stations, its cells as
//! graded, the missing blocks) with its blocks and cells said on the card, before anything is
//! written.

use serde_json::Value;
use umcf_model::blocks::BlockGrid;
use umcf_render::guides::{BlockPreview, PREVIEW_LINES};

use crate::App;
use crate::forms::{Kind, grid_text};

/// The block card's plan for some parameters and a revision of the view: the preview and its
/// line, or what keeps the grid from being made.
pub(crate) type Plan = (Value, u64, Result<(Option<BlockPreview>, String), String>);

impl App {
    /// Each frame while the block card shows its form: its fields follow the region chosen
    /// (its grid, when it has one), and the view previews the grid the form makes.
    pub(crate) fn block_card_frame(&mut self) {
        self.blocks_prefill();
        let plan = self.blocks_plan();
        let (preview, summary) = match plan {
            Ok((p, line)) => (p, Ok(line)),
            Err(e) => (None, Err(e)),
        };
        if self.viewport.blocks != preview {
            self.viewport.blocks = preview;
        }
        self.preview_summary = Some(summary);
    }

    /// The region the block card starts with: the first with blocks, else the first of the
    /// description.
    pub(crate) fn blocks_region(&self) -> String {
        let Some((d, _)) = self.session.case().and_then(|c| c.description()) else { return String::new() };
        d.regions
            .iter()
            .find(|r| r.blocks.is_some())
            .or_else(|| d.regions.first())
            .map(|r| r.name.clone())
            .unwrap_or_default()
    }

    /// Fills the grid's fields from the region's blocks when the region chosen changed (empty
    /// without blocks): what they were filled with is what the form leaves out unchanged.
    fn blocks_prefill(&mut self) {
        let Some(spec) = crate::forms::spec("mesh.block") else { return };
        let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()) else { return };
        let region = f.values.get("region").cloned().unwrap_or_default();
        if f.filled_for.as_deref() == Some(region.as_str()) {
            return;
        }
        f.filled_for = Some(region.clone());
        let grid = self
            .session
            .case()
            .and_then(|c| c.description())
            .and_then(|(d, _)| d.region(&region).and_then(|r| r.blocks.clone()));
        let json = grid.as_ref().map(umcf_engine::blocks_json);
        for field in spec.fields {
            if !matches!(field.kind, Kind::Axis | Kind::Missing(_) | Kind::Sides(_)) {
                continue;
            }
            let text = json
                .as_ref()
                .and_then(|j| j.get(field.key))
                .and_then(|v| grid_text(field.kind, v, ""))
                .unwrap_or_default();
            f.values.insert(field.key, text);
            f.extra.remove(field.key);
        }
        f.initial = f.values.clone();
        f.initial_lists = f.lists.clone();
        // Parameters from *Edit as JSON* for this region: laid over its grid.
        if let Some(v) = f.refill.take()
            && f.fill(spec, &v).is_err()
        {
            f.values = f.initial.clone();
        }
    }

    /// After a command, on a block card typed in: each grid field set to the region's grid as
    /// it is now where it wasn't changed, and what it was filled with (what OK leaves out
    /// unchanged) set to it, edited or not.
    pub(crate) fn blocks_rebase(&mut self) {
        let Some(spec) = crate::forms::spec("mesh.block") else { return };
        // Not while the card shows its JSON: back on the form, what the JSON leaves out is as
        // filled.
        let Some(f) = self.dialog.as_mut().and_then(|d| d.form.as_mut()).filter(|f| !f.json) else { return };
        let Some(region) = f.filled_for.clone() else { return };
        let json = self
            .session
            .case()
            .and_then(|c| c.description())
            .and_then(|(d, _)| d.region(&region).and_then(|r| r.blocks.clone()))
            .map(|g| umcf_engine::blocks_json(&g));
        for field in spec.fields {
            if !matches!(field.kind, Kind::Axis | Kind::Missing(_) | Kind::Sides(_)) {
                continue;
            }
            let now = json
                .as_ref()
                .and_then(|j| j.get(field.key))
                .and_then(|v| grid_text(field.kind, v, ""))
                .unwrap_or_default();
            // A value from the JSON the field can't hold (sent as it is, for the command to
            // refuse by name) stays as given; what is typed over it later is told from the grid
            // as it is now.
            if f.extra.contains_key(field.key) {
                f.initial.insert(field.key, now);
                continue;
            }
            let was = f.initial.get(field.key).cloned().unwrap_or_default();
            let value = f.values.get(field.key).cloned().unwrap_or_default();
            if value == was {
                f.values.insert(field.key, now.clone());
            } else if let Kind::Sides(names) = field.kind {
                // Sides are sent one by one: those not typed in follow the grid too.
                let merged = crate::forms::merge_sides(&value, &was, &now, names.len());
                f.values.insert(field.key, merged);
            }
            f.initial.insert(field.key, now);
        }
    }

    /// The grid the form makes, from the parameters OK sends over the region's blocks (as the
    /// command puts them together, [`umcf_engine::planned_blocks`]): its preview and a line
    /// saying its blocks and cells; none and a line for *remove*; or what keeps it from being
    /// made.
    fn blocks_plan(&mut self) -> Result<(Option<BlockPreview>, String), String> {
        let params = {
            let Some(f) = self.dialog.as_ref().and_then(|d| d.form.as_ref()) else { return Err(String::new()) };
            let spec = crate::forms::spec("mesh.block").ok_or_else(String::new)?;
            f.params(spec)?
        };
        // Made again only when the parameters changed or a command ran, not every frame.
        let revision = self.revision;
        if let Some((p, r, plan)) = &self.block_plan
            && *p == params
            && *r == revision
        {
            return plan.clone();
        }
        let plan = self.blocks_plan_of(&params);
        self.block_plan = Some((params, revision, plan.clone()));
        plan
    }

    /// [`App::blocks_plan`] of these parameters.
    fn blocks_plan_of(&self, params: &Value) -> Result<(Option<BlockPreview>, String), String> {
        let case = self.session.case().ok_or_else(String::new)?;
        let Some((description, read_problems)) = case.description() else {
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
        // A blocks entry that doesn't read: the command refuses (unless it takes them out).
        let here = format!("regions/{region}/blocks");
        let unread = read_problems.iter().find(|x| x.at == here || x.at.starts_with(&format!("{here}/")));
        let remove = params.get("remove").and_then(Value::as_bool) == Some(true);
        if let (Some(pr), false) = (unread, remove) {
            return Err(format!(
                "The blocks of region {region} don't read ({}: {}): mend them by hand, or take them out.",
                pr.at, pr.message
            ));
        }
        let Some(grid) = umcf_engine::planned_blocks(r.blocks.as_ref(), params)? else {
            return Ok((
                None,
                format!("OK takes the blocks of region {region} out of the description (its blockMeshDict stays)."),
            ));
        };
        let mut line = summary(&grid);
        // A grid of a box changed (as the command tells: its stations, cells, missing blocks
        // or single faces; any grid where the box made none): the command takes the box out.
        let reshaped =
            r.blocks.as_ref().is_none_or(|h| h.axes != grid.axes || h.missing != grid.missing || h.faces != grid.faces);
        if r.mesh_box.is_some() && reshaped {
            line.push_str("; OK takes the region's box out of the description (the grid is no longer the box's)");
        }
        Ok((Some(preview(&grid)), line))
    }
}

/// The grid's blocks and cells, said in a line.
fn summary(g: &BlockGrid) -> String {
    let [a, b, c] = g.counts();
    let cells: Vec<String> = g.axes.iter().map(|a| a.cells().map_or("?".to_string(), |n| n.to_string())).collect();
    let missing = if g.missing.is_empty() { String::new() } else { format!(", {} missing", g.missing.len()) };
    let total = g.cells().map_or("?".to_string(), |n| n.to_string());
    format!("{a} × {b} × {c} blocks{missing} · {} cells along x, y, z: {total} cells", cells.join(" × "))
}

/// The preview of a grid: along each axis its stations, its cells' lines (at most about
/// [`PREVIEW_LINES`]), and its missing blocks by their corners.
fn preview(g: &BlockGrid) -> BlockPreview {
    let mut p = BlockPreview::default();
    for (k, a) in g.axes.iter().enumerate() {
        let n = a.cells().unwrap_or(u64::MAX);
        let step = usize::try_from(n.div_ceil(PREVIEW_LINES as u64)).unwrap_or(usize::MAX).max(1);
        if let (Some(c), Some(s)) = (p.cells.get_mut(k), p.stations.get_mut(k)) {
            *c = a.cell_lines(step);
            s.clone_from(&a.stations);
        }
    }
    let corner = |b: &[usize; 3], end: usize| -> Option<[f64; 3]> {
        let mut out = [0.0; 3];
        for (k, a) in g.axes.iter().enumerate() {
            *out.get_mut(k)? = *a.stations.get(b.get(k)?.checked_add(end)?)?;
        }
        Some(out)
    };
    p.missing = g
        .missing
        .iter()
        .take(umcf_render::guides::PREVIEW_BOXES)
        .filter_map(|b| Some([corner(b, 0)?, corner(b, 1)?]))
        .collect();
    p
}
