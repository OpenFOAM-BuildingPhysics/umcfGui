//! `mesh.check`: mesh quality, as `checkMesh` checks it.

use serde_json::{Value, json};
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_validate::Level;
use umcf_validate::mesh::{Found, Quality, diagnostics, quality, unreadable};

use super::case::{count, diagnostic};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const CHECK: CommandSpec = CommandSpec {
    id: "mesh.check",
    label: "Check mesh",
    menu: "Mesh/Check",
    shortcut: None,
    params: "region: one region (default: every region with a mesh). Reads the whole mesh (a mesh that can't be read is a `mesh.unreadable` error, and the other regions are still checked), then reports what checkMesh checks by default: closed cells and boundary, aspect ratio, face areas, cell volumes, non-orthogonality, face pyramids, skewness; with checkMesh's thresholds, and the faces and cells involved (the first 1000 of each).",
    enabled: needs_case,
    run: check,
    journal: false,
    undoable: false,
};

fn found(f: &Found) -> Value {
    json!({ "count": f.count, "places": f.places, "first": f.first })
}

fn quality_json(q: &Quality) -> Value {
    json!({
        "cells": q.n_cells,
        "faces": q.n_faces,
        "directions": q.directions,
        "boundary_openness": q.boundary_openness,
        "max_openness": q.max_openness,
        "open_cells": found(&q.open_cells),
        "max_aspect_ratio": q.max_aspect_ratio,
        "high_aspect_cells": found(&q.high_aspect_cells),
        "min_face_area": q.min_face_area,
        "max_face_area": q.max_face_area,
        "zero_area_faces": found(&q.zero_area_faces),
        "min_volume": q.min_volume,
        "max_volume": q.max_volume,
        "total_volume": q.total_volume,
        "negative_volume_cells": found(&q.negative_volume_cells),
        "max_non_orthogonality": q.max_non_orthogonality,
        "average_non_orthogonality": q.average_non_orthogonality,
        "severe_non_orthogonal_faces": found(&q.severe_non_orthogonal_faces),
        "non_orthogonality_errors": found(&q.non_orthogonality_errors),
        "max_skewness": q.max_skewness,
        "skew_faces": found(&q.skew_faces),
        "wrong_pyramids": found(&q.wrong_pyramids),
        "non_finite_points": found(&q.non_finite_points),
        "non_finite_faces": found(&q.non_finite_faces),
        "non_finite_cells": found(&q.non_finite_cells),
    })
}

fn check(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(CHECK.id, p, &["region"])?;
    let region = p.str("region")?;
    let case = s.case().ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let fs = s.fs().ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let names: Vec<String> = match region {
        Some(r) => {
            if !case.regions.iter().any(|x| x.name == r) {
                let all: Vec<&str> = case.regions.iter().map(|x| x.name.as_str()).collect();
                return Err(Error::Params(format!("mesh.check: no region `{r}` (the case has: {})", all.join(", "))));
            }
            vec![r.to_string()]
        }
        None => case.regions.iter().filter(|r| r.patches.is_some()).map(|r| r.name.clone()).collect(),
    };
    let mut regions = Vec::new();
    let mut lines = Vec::new();
    let (mut errors, mut warnings) = (0, 0);
    for r in &names {
        let name = if r.is_empty() { "mesh" } else { r.as_str() };
        // A mesh that doesn't read is that region's error; the others are still checked.
        let mesh = match PolyMesh::read(fs, &mesh_dir(r), &Limits::default()) {
            Ok(m) => m,
            Err(e) => {
                errors += 1;
                lines.push(format!("{name}: the mesh could not be read"));
                regions.push(json!({
                    "region": r,
                    "quality": Value::Null,
                    "diagnostics": [diagnostic(&unreadable(r, &e.to_string()))],
                }));
                continue;
            }
        };
        let q = quality(&mesh);
        let d = diagnostics(&q, r);
        let e = d.iter().filter(|x| x.level == Level::Error).count();
        let w = d.iter().filter(|x| x.level == Level::Warning).count();
        errors += e;
        warnings += w;
        lines.push(format!(
            "{name}: {} cells, non-orthogonality max {:.1}°, skewness max {:.2}, {}, {}",
            q.n_cells,
            q.max_non_orthogonality,
            q.max_skewness,
            count(e, "error"),
            count(w, "warning")
        ));
        regions.push(json!({
            "region": r,
            "quality": quality_json(&q),
            "diagnostics": d.iter().map(diagnostic).collect::<Vec<_>>(),
        }));
    }
    if names.is_empty() {
        lines.push("no region has a mesh".to_string());
    }
    Ok(Outcome::new(lines.join("; "), json!({ "errors": errors, "warnings": warnings, "regions": regions })))
}
