//! `mesh.check`: mesh quality, as `checkMesh` checks it.

use serde_json::{Value, json};
use umcf_core::CaseFs;
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_validate::mesh::{
    AllGeometry, CHECKMESH_SETS, CheckMeshLog, Found, MAX_LISTED, MAX_LOG_RESULTS, Quality, SetOf,
    checkmesh_log_diagnostics, diagnostics, parse_checkmesh_log, quality, quality_all, soften_for_vegetation,
    stale_set, unreadable,
};
use umcf_validate::{Diagnostic, Level};

use super::case::{count, diagnostic};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const CHECK: CommandSpec = CommandSpec {
    id: "mesh.check",
    label: "Check mesh",
    menu: "Mesh/Check",
    shortcut: None,
    params: "region: one region (default: every region with a mesh). Reads the whole mesh (a mesh that can't be read is a `mesh.unreadable` error, and the other regions are still checked), then reports what checkMesh checks by default: closed cells and boundary, aspect ratio, face areas, cell volumes, non-orthogonality, face pyramids, skewness; with checkMesh's thresholds, and the faces and cells involved (the first 1000 of each). all_geometry: true adds the checks of checkMesh -allGeometry (face tets, short edges, near points, concave faces and cells, warped faces, cell determinants, face weights, volume ratios); slower.",
    enabled: needs_case,
    run: check,
    journal: false,
    undoable: false,
};

/// The largest `checkMesh` log read.
const MAX_LOG_BYTES: u64 = 16 << 20;

/// A note about the case's `checkMesh` logs as a whole (one not read, or not for any region).
fn log_note(file: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: Level::Info,
        code: "mesh.checkmesh",
        path: file.to_string(),
        region: None,
        patch: None,
        field: None,
        message,
    }
}

/// The `checkMesh` logs in the case directory (`log.checkMesh`, `log.checkMesh.<anything>`),
/// and notes on those that could not be read.
fn checkmesh_logs(fs: &dyn CaseFs, notes: &mut Vec<Diagnostic>) -> Vec<CheckMeshLog> {
    let names = match fs.list("") {
        Ok(names) => names,
        Err(e) => {
            notes.push(log_note(
                ".",
                format!("the case directory could not be listed, so no checkMesh log was read: {e}"),
            ));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for n in names.iter().filter(|n| n.starts_with("log.checkMesh") && fs.is_file(n)) {
        match fs.read(n, MAX_LOG_BYTES) {
            Ok(Some(bytes)) => {
                let logs = parse_checkmesh_log(n, &String::from_utf8_lossy(&bytes));
                if logs.is_empty() {
                    notes.push(log_note(
                        n,
                        "not used: no `Exec : checkMesh` line (an empty, cut or compressed log, or another program's)"
                            .to_string(),
                    ));
                }
                out.extend(logs);
            }
            Ok(None) => {}
            Err(e) => notes.push(log_note(n, format!("not read: {e}"))),
        }
    }
    out
}

fn log_json(l: &CheckMeshLog) -> Value {
    json!({
        "file": l.file,
        "version": l.version,
        "command": l.command,
        "time": l.time,
        "ok": l.ok,
        "failed": l.failed,
        "problems": l.problems,
        "notes": l.notes,
        "cells": l.cells,
        "fatal": l.fatal,
        "skipped": l.skipped.map(|s| json!({ "times": s.times, "failed": s.failed, "unfinished": s.unfinished })),
        "truncated": l.truncated,
        "unlisted_problems": l.unlisted_problems,
    })
}

/// Labels a set may hold beyond the size of its mesh. A valid set holds each label once, below
/// the mesh's size; a set from another mesh with labels beyond it is still read (and reported
/// stale), and only one with more labels than size + margin is unreadable (`mesh.unreadable-set`).
const STALE_SET_MARGIN: usize = 1 << 20;

/// The bound on reading a set of a mesh with `size` faces, cells or points: its labels, and the
/// bytes they take as text (up to ten digits and a separator each, plus the header).
fn set_limits(size: usize) -> Limits {
    let max_elements = size.saturating_add(STALE_SET_MARGIN);
    let max_bytes = (max_elements as u64).saturating_mul(11).saturating_add(64 << 10);
    Limits { max_bytes, max_elements, ..Limits::default() }
}

fn set_warning(region: &str, path: String, message: String) -> Diagnostic {
    Diagnostic {
        level: Level::Warning,
        code: "mesh.unreadable-set",
        path,
        region: (!region.is_empty()).then(|| region.to_string()),
        patch: None,
        field: None,
        message,
    }
}

/// The sets `checkMesh -writeSets` left in the region's `polyMesh/sets`, with their labels;
/// one naming more than the mesh has is stale (`mesh.stale-set`).
fn checkmesh_sets(fs: &dyn CaseFs, region: &str, mesh: &PolyMesh, out: &mut Vec<Diagnostic>) -> Vec<Value> {
    let dir = mesh_dir(region);
    let present = match umcf_foam_mesh::list_sets(fs, &dir) {
        Ok(p) => p,
        Err(e) => {
            out.push(set_warning(
                region,
                format!("{dir}/sets"),
                format!("the sets could not be listed, so checkMesh's sets were not read: {e}"),
            ));
            return Vec::new();
        }
    };
    let mut sets = Vec::new();
    for (name, of, holds) in CHECKMESH_SETS.iter().filter(|(n, _, _)| present.iter().any(|p| p == n)) {
        let size = match of {
            SetOf::Faces => mesh.n_faces(),
            SetOf::Cells => mesh.n_cells(),
            SetOf::Points => mesh.n_points(),
        };
        let mut set = match umcf_foam_mesh::read_set(fs, &dir, name, &set_limits(size)) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(e) => {
                out.push(set_warning(
                    region,
                    format!("{dir}/sets/{name}"),
                    format!("the checkMesh set `{name}` could not be read: {e}"),
                ));
                sets.push(json!({ "name": name, "holds": holds, "error": e.to_string() }));
                continue;
            }
        };
        // OpenFOAM writes a set in hash order: the lowest labels first.
        set.labels.sort_unstable();
        let largest = set.labels.iter().copied().max();
        let stale = largest.is_some_and(|m| m as usize >= size);
        if let (true, Some(m)) = (stale, largest) {
            out.push(stale_set(region, name, m, size, *of));
        }
        let first: Vec<u32> = set.labels.iter().take(MAX_LISTED).copied().collect();
        sets.push(json!({
            "name": name,
            "holds": holds,
            "class": set.class,
            "count": set.labels.len(),
            "first": first,
            "stale": stale,
        }));
    }
    sets
}

fn found(f: &Found) -> Value {
    json!({ "count": f.count, "places": f.places, "first": f.first })
}

fn all_geometry_json(g: &AllGeometry) -> Value {
    json!({
        "low_quality_tets": found(&g.low_quality_tets),
        "min_edge_length": g.min_edge_length,
        "max_edge_length": g.max_edge_length,
        "near_distance": g.near_distance,
        "short_edge_points": found(&g.short_edge_points),
        "near_points": found(&g.near_points),
        "concave_faces": found(&g.concave_faces),
        "max_concave_angle": g.max_concave_angle,
        "min_flatness": g.min_flatness,
        "average_flatness": g.average_flatness,
        "warped_faces": found(&g.warped_faces),
        "min_determinant": g.min_determinant,
        "average_determinant": g.average_determinant,
        "underdetermined_cells": found(&g.underdetermined_cells),
        "concave_cells": found(&g.concave_cells),
        "min_weight": g.min_weight,
        "average_weight": g.average_weight,
        "low_weight_faces": found(&g.low_weight_faces),
        "min_volume_ratio": g.min_volume_ratio,
        "average_volume_ratio": g.average_volume_ratio,
        "low_volume_ratio_faces": found(&g.low_volume_ratio_faces),
        "unchecked": g.unchecked,
    })
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
        "all_geometry": q.all.as_ref().map_or(Value::Null, all_geometry_json),
    })
}

fn check(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(CHECK.id, p, &["region", "all_geometry"])?;
    let region = p.str("region")?;
    let all_geometry = p.bool("all_geometry")?.unwrap_or(false);
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
    let mut notes = Vec::new();
    let logs = checkmesh_logs(fs, &mut notes);
    // Logs for no region of the case (only when every region is checked).
    if region.is_none() {
        let all: Vec<&str> = case.regions.iter().map(|x| x.name.as_str()).collect();
        let mut noted: Vec<&str> = Vec::new();
        for l in logs.iter().filter(|l| !all.contains(&l.region.as_str())) {
            // One note per file (a long log has a result per time).
            if noted.contains(&l.file.as_str()) {
                continue;
            }
            noted.push(&l.file);
            let what = if l.region.is_empty() {
                format!(
                    "checked the default mesh (no -region), which this case doesn't have; its regions are {}",
                    all.join(", ")
                )
            } else {
                format!("checked region `{}`, which this case doesn't have", l.region)
            };
            notes.push(log_note(&l.file, format!("not used: it {what}")));
        }
    }
    let mut regions = Vec::new();
    let mut lines = Vec::new();
    let (mut errors, mut warnings) = (0, 0);
    for r in &names {
        let name = if r.is_empty() { "mesh" } else { r.as_str() };
        let candidates: Vec<&CheckMeshLog> = logs.iter().filter(|l| &l.region == r).collect();
        let mut d: Vec<Diagnostic> = Vec::new();
        // A mesh that doesn't read is that region's error; the others are still checked.
        let mesh = PolyMesh::read(fs, &mesh_dir(r), &Limits::default());
        // A log is about this mesh when it checked as many cells (a run over the meshes of
        // other times, or an older mesh, checked another one); a log that doesn't say is kept.
        let mut mine: Vec<&CheckMeshLog> = Vec::new();
        for l in candidates {
            // Cells, faces and points: a mesh moved or extruded again can keep its cell count.
            let differs = |m: &PolyMesh| {
                [(l.cells, m.n_cells()), (l.faces, m.n_faces()), (l.points, m.n_points())]
                    .iter()
                    .any(|(said, has)| said.is_some_and(|n| n != *has as u64))
            };
            match &mesh {
                Ok(m) if differs(m) => {
                    let at = match (&l.time, l.skipped) {
                        (Some(t), _) => format!(" (time {t})"),
                        (None, Some(_)) => format!(" (times past the first {MAX_LOG_RESULTS})"),
                        (None, None) => String::new(),
                    };
                    let size = |c: Option<u64>, f: Option<u64>, p: Option<u64>| {
                        let s = |x: Option<u64>| x.map_or("?".to_string(), |x| x.to_string());
                        format!("{} cells, {} faces, {} points", s(c), s(f), s(p))
                    };
                    let mut note = log_note(
                        &l.file,
                        format!(
                            "not used{at}: it checked a mesh of {}, constant/ has {}",
                            size(l.cells, l.faces, l.points),
                            size(Some(m.n_cells() as u64), Some(m.n_faces() as u64), Some(m.n_points() as u64))
                        ),
                    );
                    if !r.is_empty() {
                        note.region = Some(r.clone());
                    }
                    d.push(note);
                }
                _ => mine.push(l),
            }
        }
        let (q, sets) = match &mesh {
            Ok(mesh) => {
                let q = if all_geometry { quality_all(mesh) } else { quality(mesh) };
                d.extend(diagnostics(&q, r));
                let sets = checkmesh_sets(fs, r, mesh, &mut d);
                (Some(q), sets)
            }
            Err(e) => {
                d.insert(0, unreadable(r, &e.to_string()));
                (None, Vec::new())
            }
        };
        let mut from_logs: Vec<Diagnostic> =
            mine.iter().flat_map(|l| checkmesh_log_diagnostics(l, q.as_ref())).collect();
        if case.region(r).is_some_and(|x| x.kind == umcf_model::RegionKind::Vegetation) {
            soften_for_vegetation(&mut d);
            soften_for_vegetation(&mut from_logs);
        }
        // A checkMesh finding the native check also makes is a note (checkmesh_log_diagnostics),
        // so nothing counts twice.
        let e = d.iter().chain(&from_logs).filter(|x| x.level == Level::Error).count();
        let w = d.iter().chain(&from_logs).filter(|x| x.level == Level::Warning).count();
        errors += e;
        warnings += w;
        lines.push(match &q {
            Some(q) => format!(
                "{name}: {} cells, non-orthogonality max {:.1}°, skewness max {:.2}, {}, {}",
                q.n_cells,
                q.max_non_orthogonality,
                q.max_skewness,
                count(e, "error"),
                count(w, "warning")
            ),
            None => format!("{name}: the mesh could not be read"),
        });
        regions.push(json!({
            "region": r,
            "quality": q.as_ref().map_or(Value::Null, quality_json),
            "checkmesh": {
                "logs": mine.iter().map(|l| log_json(l)).collect::<Vec<_>>(),
                "sets": sets,
            },
            "diagnostics": from_logs.iter().chain(&d).map(diagnostic).collect::<Vec<_>>(),
        }));
    }
    if names.is_empty() {
        lines.push("no region has a mesh".to_string());
    }
    let data = json!({
        "errors": errors,
        "warnings": warnings,
        "regions": regions,
        "diagnostics": notes.iter().map(diagnostic).collect::<Vec<_>>(),
    });
    Ok(Outcome::new(lines.join("; "), data))
}
