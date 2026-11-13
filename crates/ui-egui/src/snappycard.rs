//! The *snappyHexMesh* card (`mesh.snappy`): its form filled from the region's snappy entry
//! ([`crate::workflowcard`]), and while it is open the region's blocks, the refinement boxes
//! and the inside point the form holds shown in the 3D view, with what it has said on the card.

use umcf_model::snappy::MAX_ITEMS;
use umcf_render::guides::BlockPreview;

use crate::App;
use crate::forms::{SURFACE_COLS, number_text, point_of, snappy_boxes, table_row_count, table_rows};

impl App {
    /// Each frame while the snappy card shows its form: the view previews the region's blocks
    /// (the background mesh), the boxes and the inside point the form has, as they are typed.
    pub(crate) fn snappy_card_frame(&mut self) {
        let Some(f) = self.dialog.as_ref().and_then(|d| d.form.as_ref()) else { return };
        let value = |k: &str| f.values.get(k).map_or("", String::as_str);
        let region = value("region").trim().to_string();
        let point = point_of(value("inside"));
        // The region's blocks previewed, what its entry has (for a table it couldn't fill: a
        // name with a `|`, say), and where the inside point is in the blocks, by the command's
        // rules: outside them or on their sides (their first and last stations), or in a missing
        // block (every block holding it missing); read, not copied.
        let description = self.session.case().map(|c| c.description().map(|(d, _)| d));
        let (grid, entry, listed, place) = match &description {
            Some(Some(d)) => d.region(&region).map_or((None, (0, 0), false, None), |r| {
                let entry = r.snappy.as_ref().map_or((0, 0), |s| (s.surfaces.len(), s.boxes.len()));
                let place = r.blocks.as_ref().zip(point).map(|(g, q)| {
                    let outside = (0..3).any(|a| {
                        let st = g.axes.get(a).map(|x| x.stations.as_slice()).unwrap_or_default();
                        match (st.first(), st.last(), q.get(a)) {
                            (Some(lo), Some(hi), Some(x)) => !(lo < x && x < hi),
                            _ => true,
                        }
                    });
                    (outside, umcf_engine::snappy_missing_at(g, q))
                });
                (r.blocks.as_ref().map(crate::blockcard::preview), entry, true, place)
            }),
            _ => (None, (0, 0), false, None),
        };
        let loads = matches!(description, Some(Some(_)));
        let has_blocks = grid.is_some();
        let mut p = grid.unwrap_or_default();
        let mut said = Vec::new();
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        // What Edit as JSON gave that a table can't hold (a patch's role, say) is sent as it is.
        let json = |k: &str| f.extra.contains_key(k);
        // Rows counted before any is made: tables past the command's limit (surfaces and boxes
        // together) aren't read each frame.
        let (surface_text, box_text) = (value("surfaces"), value("boxes"));
        let rows = |k: &str, t: &str| if json(k) { 0 } else { table_row_count(t) };
        let (surface_rows, box_rows) = (rows("surfaces", surface_text), rows("boxes", box_text));
        let over = surface_rows.saturating_add(box_rows) > MAX_ITEMS;
        let surfaces = if over { Vec::new() } else { table_rows(surface_text, SURFACE_COLS.len()) };
        let boxes = if over { Vec::new() } else { snappy_boxes(box_text) };
        let mode = SURFACE_COLS.iter().position(|c| *c == "mode");
        let cutting =
            surfaces.iter().filter(|r| mode.and_then(|m| r.get(m)).map(String::as_str) == Some("cut")).count();
        // A table the entry couldn't fill (filled empty though the entry has some): what is
        // typed there is beside the entry's, which stay.
        let unshown = |k: &str, n: usize| n > 0 && f.initial.get(k).is_none_or(|t| t.trim().is_empty());
        let beside = |typed: usize, n: usize, one: &str, many: &str| match typed {
            0 => format!("the entry's {} the table can't show", count(n, one, many)),
            t => format!(
                "{t} typed over the entry's {} the table can't show (one of the same name is changed)",
                count(n, one, many)
            ),
        };
        if json("surfaces") {
            said.push("the surfaces as Edit as JSON gives them".to_string());
        } else if !over {
            said.push(if unshown("surfaces", entry.0) {
                beside(surfaces.len(), entry.0, "surface", "surfaces")
            } else {
                format!("{} ({cutting} cutting)", count(surfaces.len(), "surface", "surfaces"))
            });
        }
        if json("boxes") {
            said.push("the boxes as Edit as JSON gives them".to_string());
        } else if !over {
            said.push(if unshown("boxes", entry.1) {
                beside(boxes.len(), entry.1, "refinement box", "refinement boxes")
            } else {
                count(boxes.len(), "refinement box", "refinement boxes")
            });
        }
        if over {
            // The tables not held as JSON, together.
            let tables: Vec<String> = [
                (!json("surfaces")).then(|| count(surface_rows, "surface", "surfaces")),
                (!json("boxes")).then(|| count(box_rows, "refinement box", "refinement boxes")),
            ]
            .into_iter()
            .flatten()
            .collect();
            said.push(format!("{}: more than the {MAX_ITEMS} the command takes, not shown", tables.join(" and ")));
        }
        let mut unread = Vec::new();
        for (name, b) in boxes {
            match b {
                Ok(b) => p.refine.push(b),
                Err(e) => unread.push(format!("box {name} isn't shown: {e}")),
            }
        }
        // The first that doesn't read, and how many more.
        if let Some(first) = unread.first() {
            said.push(match unread.len().saturating_sub(1) {
                0 => first.clone(),
                more => format!("{first} ({more} more not shown)"),
            });
        }
        p.inside = point;
        // A coordinate in its shortest form (`1e300`, not 301 digits).
        let n = |x: f64| number_text(&serde_json::json!(x));
        let blank = value("inside").chars().all(|c| c == '|' || c.is_whitespace());
        match (p.inside, place) {
            (None, _) if json("inside") => said.push("the inside point as Edit as JSON gives it".to_string()),
            (None, _) if blank => said.push("no inside point given".to_string()),
            (None, _) => said.push("the inside point doesn't read: give x, y and z".to_string()),
            (Some(q), Some((true, _))) => {
                said.push(format!(
                    "the inside point ({}, {}, {}) is outside the region's blocks or on their sides",
                    n(q[0]),
                    n(q[1]),
                    n(q[2])
                ));
            }
            (Some(q), Some((_, Some(b)))) => {
                said.push(format!(
                    "the inside point ({}, {}, {}) is in the missing block ({} {} {}) of the region's blocks",
                    n(q[0]),
                    n(q[1]),
                    n(q[2]),
                    b[0],
                    b[1],
                    b[2]
                ));
            }
            (Some(q), _) => said.push(format!("the inside point ({}, {}, {})", n(q[0]), n(q[1]), n(q[2]))),
        }
        if !loads {
            said.push("system/umcfWorkflow is missing or doesn't load".to_string());
        } else if !listed && !region.is_empty() {
            said.push(format!("region {region} isn't in system/umcfWorkflow"));
        } else if !has_blocks && !region.is_empty() {
            said.push(format!("region {region} has no blocks in system/umcfWorkflow"));
        }
        let preview = (p != BlockPreview::default()).then_some(p);
        if self.viewport.blocks != preview {
            self.viewport.blocks = preview;
        }
        self.preview_summary = Some(Ok(said.join(" · ")));
    }
}
