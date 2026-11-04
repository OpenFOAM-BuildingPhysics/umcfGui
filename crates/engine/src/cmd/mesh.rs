//! `mesh.check`: mesh quality, as `checkMesh` checks it.

use serde_json::{Value, json};
use umcf_core::CaseFs;
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_validate::mesh::{
    AllGeometry, CHECKMESH_SETS, CheckMeshLog, Found, MAX_LISTED, MAX_LOG_RESULTS, Quality, SetOf, SnappyLog,
    checkmesh_log_diagnostics, diagnostics, layer_diagnostics, parse_checkmesh_log, parse_snappy_log, quality,
    quality_all, soften_for_vegetation, stale_set, unreadable,
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

/// The directories logs are looked for in: the case directory and its `log/` (where the
/// tutorials' `RunFunctions` and many case scripts put them).
const LOG_DIRS: [&str; 2] = ["", "log"];

/// The logs named `<prefix>` or `<prefix>.<anything>` in [`LOG_DIRS`], each handed to `parse`
/// with its case-relative path and text as it is read (one in memory at a time); notes on those
/// that could not be read. A log in `log/` with the name of one in the case directory is left
/// out, with a note: the case scripts move older logs there.
fn read_logs(fs: &dyn CaseFs, prefix: &str, notes: &mut Vec<Diagnostic>, mut parse: impl FnMut(&str, &str)) {
    let mut seen: Vec<String> = Vec::new();
    for dir in LOG_DIRS {
        let names = match fs.list(dir) {
            Ok(names) => names,
            Err(e) => {
                let at = if dir.is_empty() { "." } else { dir };
                notes.push(log_note(at, format!("could not be listed, so no {prefix} log in it was read: {e}")));
                continue;
            }
        };
        let matches = |n: &&String| n.as_str() == prefix || n.strip_prefix(prefix).is_some_and(|r| r.starts_with('.'));
        for n in names.iter().filter(matches) {
            let path = if dir.is_empty() { n.clone() } else { format!("{dir}/{n}") };
            if !fs.is_file(&path) {
                continue;
            }
            if seen.contains(n) {
                notes.push(log_note(&path, format!("not used: the case directory has a `{n}` too, which is used (the case scripts move older logs into log/)")));
                continue;
            }
            seen.push(n.clone());
            match fs.read(&path, MAX_LOG_BYTES) {
                Ok(Some(bytes)) => parse(&path, &String::from_utf8_lossy(&bytes)),
                Ok(None) => {}
                Err(e) => notes.push(log_note(&path, format!("not read: {e}"))),
            }
        }
    }
}

/// The `checkMesh` logs (`log.checkMesh`, `log.checkMesh.<anything>`, in [`LOG_DIRS`]), and
/// notes on those that could not be read.
fn checkmesh_logs(fs: &dyn CaseFs, notes: &mut Vec<Diagnostic>) -> Vec<CheckMeshLog> {
    let mut out = Vec::new();
    let mut unparsed = Vec::new();
    read_logs(fs, "log.checkMesh", notes, |n, text| {
        let logs = parse_checkmesh_log(n, text);
        if logs.is_empty() {
            unparsed.push(n.to_string());
        }
        out.extend(logs);
    });
    for n in unparsed {
        notes.push(log_note(
            &n,
            "not used: no `Exec : checkMesh` line (an empty, cut or compressed log, or another program's)".to_string(),
        ));
    }
    out
}

/// The `snappyHexMesh` logs (`log.snappyHexMesh[.<anything>]`, in [`LOG_DIRS`]) with their
/// region: `-region`'s, else the name's suffix when it is a region of the case
/// (`log.snappyHexMesh.air`, as cases that swap `snappyHexMeshDict`s name them), else `None`:
/// the default mesh, or one moved into a region afterwards (matched by size).
fn snappy_logs(fs: &dyn CaseFs, regions: &[&str], notes: &mut Vec<Diagnostic>) -> Vec<(Option<String>, SnappyLog)> {
    let mut out = Vec::new();
    read_logs(fs, "log.snappyHexMesh", notes, |n, text| {
        let Some(log) = parse_snappy_log(n, text) else { return };
        let suffix =
            n.rsplit('/').next().and_then(|b| b.strip_prefix("log.snappyHexMesh.")).filter(|s| regions.contains(s));
        let region = log.region.clone().or_else(|| suffix.map(str::to_string));
        out.push((region, log));
    });
    out
}

/// `11274 cells, 37893 faces, 15906 points` (`?` for what a log doesn't say).
fn mesh_size(c: Option<u64>, f: Option<u64>, p: Option<u64>) -> String {
    let s = |x: Option<u64>| x.map_or("?".to_string(), |x| x.to_string());
    format!("{} cells, {} faces, {} points", s(c), s(f), s(p))
}

fn snappy_json(l: &SnappyLog) -> Value {
    let layers: Vec<Value> = l
        .patches
        .iter()
        .map(|p| {
            json!({
                "patch": p.name,
                "faces": p.faces,
                "wanted": p.wanted,
                "layers": p.layers,
                "coverage": p.coverage(),
                "thickness": p.thickness,
                "wanted_thickness": p.wanted_thickness,
                "percent": p.percent,
            })
        })
        .collect();
    json!({
        "file": l.file,
        "version": l.version,
        "command": l.command,
        "layer_phase": l.layer_phase,
        "finished": l.finished,
        "fatal": l.fatal,
        "cells": l.cells,
        "layers": layers,
    })
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
    let region_names: Vec<&str> = case.regions.iter().map(|x| x.name.as_str()).collect();
    let snappy = snappy_logs(fs, &region_names, &mut notes);
    let has_default = region_names.contains(&"");
    let mut snappy_used = vec![false; snappy.len()];
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
        for (lr, l) in
            snappy.iter().filter(|(lr, l)| l.layer_phase && lr.as_ref().is_some_and(|r| !all.contains(&r.as_str())))
        {
            let what = lr.as_deref().unwrap_or_default();
            notes.push(log_note(&l.file, format!("not used: it meshed region `{what}`, which this case doesn't have")));
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
                    let mut note = log_note(
                        &l.file,
                        format!(
                            "not used{at}: it checked a mesh of {}, constant/ has {}",
                            mesh_size(l.cells, l.faces, l.points),
                            mesh_size(Some(m.n_cells() as u64), Some(m.n_faces() as u64), Some(m.n_points() as u64))
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
        // snappyHexMesh's layers, for the mesh it wrote (one with layers only: a log without a
        // layer phase says nothing about them).
        let mut layer_logs: Vec<&SnappyLog> = Vec::new();
        for (k, (lr, l)) in snappy.iter().enumerate().filter(|(_, (_, l))| l.layer_phase) {
            let size = [l.cells, l.faces, l.points];
            let same = |m: &PolyMesh| {
                size.iter().zip([m.n_cells(), m.n_faces(), m.n_points()]).all(|(s, n)| *s == Some(n as u64))
            };
            match lr {
                Some(lr) if lr == r => {}
                Some(_) => continue,
                // The default mesh: this region's when the case has one.
                None if has_default => {
                    if !r.is_empty() {
                        continue;
                    }
                }
                // Meshed as the default mesh and moved into a region (`mv constant/polyMesh
                // constant/air`): the region whose mesh it is, by size.
                None => match &mesh {
                    Ok(m) if same(m) => {}
                    _ => continue,
                },
            }
            if let Some(u) = snappy_used.get_mut(k) {
                *u = true;
            }
            match &mesh {
                // A count it gives that differs (one it doesn't give can't tell).
                Ok(m)
                    if size
                        .iter()
                        .zip([m.n_cells(), m.n_faces(), m.n_points()])
                        .any(|(s, n)| s.is_some_and(|s| s != n as u64)) =>
                {
                    let mut note = log_note(
                        &l.file,
                        format!(
                            "not used: it wrote a mesh of {}, constant/ has {} (meshed again, or changed since)",
                            mesh_size(l.cells, l.faces, l.points),
                            mesh_size(Some(m.n_cells() as u64), Some(m.n_faces() as u64), Some(m.n_points() as u64))
                        ),
                    );
                    if !r.is_empty() {
                        note.region = Some(r.clone());
                    }
                    d.push(note);
                }
                _ => {
                    from_logs.extend(layer_diagnostics(l, r));
                    layer_logs.push(l);
                }
            }
        }
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
            "snappyhexmesh": {
                "logs": layer_logs.iter().map(|l| snappy_json(l)).collect::<Vec<_>>(),
            },
            "diagnostics": from_logs.iter().chain(&d).map(diagnostic).collect::<Vec<_>>(),
        }));
    }
    // Logs of the default mesh in a case without one that no region's mesh matches.
    if region.is_none() && !has_default {
        for ((lr, l), used) in snappy.iter().zip(&snappy_used) {
            if lr.is_none() && l.layer_phase && !used {
                notes.push(log_note(
                    &l.file,
                    format!(
                        "not used: it meshed the default mesh (no -region), and no region's mesh has its size ({})",
                        mesh_size(l.cells, l.faces, l.points)
                    ),
                ));
            }
        }
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
