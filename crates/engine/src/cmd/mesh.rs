//! `mesh.check`: mesh quality, as `checkMesh` checks it.

use serde_json::{Value, json};
use umcf_core::CaseFs;
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_render::{Found as Shown, FoundClass};
use umcf_validate::mesh::{
    AllGeometry, CHECKMESH_SETS, CheckMeshLog, Found, MAX_LISTED, MAX_LOG_RESULTS, Quality, SetOf, SnappyLog,
    checkmesh_log_diagnostics, diagnostics, layer_diagnostics, parse_checkmesh_log, parse_snappy_log, quality_steps,
    quality_with, soften_for_vegetation, stale_set, unreadable,
};
use umcf_validate::{Diagnostic, Level};

use std::sync::Arc;

use super::case::{count, diagnostic};
use super::scene::{Whole, room_left};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const CHECK: CommandSpec = CommandSpec {
    id: "mesh.check",
    label: "Check mesh",
    menu: "Mesh/Check",
    shortcut: None,
    params: "region: one region (default: every region with a mesh). Counts its progress (the mesh files' bytes read, then the faces checked) and stops when cancelled, within 65,536 faces (all_geometry's own checks only at their end). Reads the whole mesh (a mesh that can't be read is a `mesh.unreadable` error, and the other regions are still checked), then reports what checkMesh checks by default: closed cells and boundary, aspect ratio, face areas, cell volumes, non-orthogonality, face pyramids, skewness; with checkMesh's thresholds, and the faces and cells involved (the first 1000 of each). What it found in each region (those faces and cells, and those of checkMesh's sets of faces and cells that aren't stale) is kept for the 3D view, `scene.snapshot`'s cuts {\"region\", \"found\": true}, with the whole mesh it read while it fits in the budget for hidden objects (so that showing them reads nothing again), until the next check of the region, the case opened again or its meshes read again (reload). all_geometry: true adds the checks of checkMesh -allGeometry (face tets, short edges, near points, concave faces and cells, warped faces, cell determinants, face weights, volume ratios); slower.",
    enabled: needs_case,
    run: check,
    journal: false,
    undoable: false,
    reads_only: true,
};

/// The largest `checkMesh` log read.
const MAX_LOG_BYTES: u64 = 16 << 20;

/// A note about the case's `checkMesh` (`code` `mesh.checkmesh`) or `snappyHexMesh`
/// (`mesh.snappyhexmesh`) logs: one not read, or not used.
fn log_note(code: &'static str, file: &str, message: String) -> Diagnostic {
    Diagnostic { level: Level::Info, code, path: file.to_string(), region: None, patch: None, field: None, message }
}

/// The directories logs are looked for in: the case directory and its `log/` (where the
/// tutorials' `RunFunctions` and many case scripts put them).
const LOG_DIRS: [&str; 2] = ["", "log"];

/// The logs named `<prefix>` or `<prefix>.<anything>` in [`LOG_DIRS`], each handed to `parse`
/// with its case-relative path and text as it is read (one in memory at a time); notes on those
/// that could not be read. A log in `log/` with the name of one in the case directory is left
/// out, with a note: the case scripts move older logs there.
fn read_logs(
    fs: &dyn CaseFs,
    prefix: &str,
    code: &'static str,
    notes: &mut Vec<Diagnostic>,
    mut parse: impl FnMut(&str, &str),
) {
    let mut seen: Vec<String> = Vec::new();
    for dir in LOG_DIRS {
        let names = match fs.list(dir) {
            Ok(names) => names,
            Err(e) => {
                let at = if dir.is_empty() { "." } else { dir };
                notes.push(log_note(code, at, format!("could not be listed, so no {prefix} log in it was read: {e}")));
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
                notes.push(log_note(code, &path, format!("not used: the case directory has a `{n}` too, which is used (the case scripts move older logs into log/)")));
                continue;
            }
            seen.push(n.clone());
            match fs.read(&path, MAX_LOG_BYTES) {
                Ok(Some(bytes)) => parse(&path, &String::from_utf8_lossy(&bytes)),
                Ok(None) => {}
                Err(e) => notes.push(log_note(code, &path, format!("not read: {e}"))),
            }
        }
    }
}

/// The `checkMesh` logs (`log.checkMesh`, `log.checkMesh.<anything>`, in [`LOG_DIRS`]), and
/// notes on those that could not be read.
fn checkmesh_logs(fs: &dyn CaseFs, notes: &mut Vec<Diagnostic>) -> Vec<CheckMeshLog> {
    let mut out = Vec::new();
    let mut unparsed = Vec::new();
    read_logs(fs, "log.checkMesh", "mesh.checkmesh", notes, |n, text| {
        let logs = parse_checkmesh_log(n, text);
        if logs.is_empty() {
            unparsed.push(n.to_string());
        }
        out.extend(logs);
    });
    for n in unparsed {
        notes.push(log_note(
            "mesh.checkmesh",
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
    read_logs(fs, "log.snappyHexMesh", "mesh.snappyhexmesh", notes, |n, text| {
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
/// one naming more than the mesh has is stale (`mesh.stale-set`). Besides, the first labels of
/// each set of faces or cells that is not stale, for the 3D view.
fn checkmesh_sets(
    fs: &dyn CaseFs,
    region: &str,
    mesh: &PolyMesh,
    out: &mut Vec<Diagnostic>,
) -> (Vec<Value>, Vec<FoundClass>) {
    let dir = mesh_dir(region);
    let present = match umcf_foam_mesh::list_sets(fs, &dir) {
        Ok(p) => p,
        Err(e) => {
            out.push(set_warning(
                region,
                format!("{dir}/sets"),
                format!("the sets could not be listed, so checkMesh's sets were not read: {e}"),
            ));
            return (Vec::new(), Vec::new());
        }
    };
    let mut sets = Vec::new();
    let mut shown = Vec::new();
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
        if !stale && !first.is_empty() && !matches!(of, SetOf::Points) {
            shown.push(FoundClass {
                name: format!("checkMesh: {holds}"),
                cells: matches!(of, SetOf::Cells),
                labels: first.clone(),
                count: set.labels.len(),
            });
        }
        sets.push(json!({
            "name": name,
            "holds": holds,
            "class": set.class,
            "count": set.labels.len(),
            "first": first,
            "stale": stale,
        }));
    }
    (sets, shown)
}

/// What the native check found, by the faces and cells where it is (the first
/// [`MAX_LISTED`] of each), and what checkMesh's sets name, for the 3D view: what found
/// nothing is left out, and so are points.
fn shown(q: &Quality, sets: Vec<FoundClass>) -> Shown {
    let mut list: Vec<(&str, bool, &umcf_validate::mesh::Found)> = vec![
        ("open cells", true, &q.open_cells),
        ("cells of a high aspect ratio", true, &q.high_aspect_cells),
        ("faces of zero area", false, &q.zero_area_faces),
        ("cells of zero or negative volume", true, &q.negative_volume_cells),
        ("faces more than 70° non-orthogonal", false, &q.severe_non_orthogonal_faces),
        ("faces 90° or more non-orthogonal", false, &q.non_orthogonality_errors),
        ("skew faces", false, &q.skew_faces),
        ("faces with a wrong pyramid", false, &q.wrong_pyramids),
        ("faces too large to measure", false, &q.non_finite_faces),
        ("cells too large to measure", true, &q.non_finite_cells),
    ];
    if let Some(a) = &q.all {
        list.extend([
            ("faces of poor tetrahedra", false, &a.low_quality_tets),
            ("concave faces", false, &a.concave_faces),
            ("warped faces", false, &a.warped_faces),
            ("underdetermined cells", true, &a.underdetermined_cells),
            ("concave cells", true, &a.concave_cells),
            ("faces of a low weight", false, &a.low_weight_faces),
            ("faces of a low volume ratio", false, &a.low_volume_ratio_faces),
        ]);
    }
    let mut classes: Vec<FoundClass> = list
        .into_iter()
        .filter(|(_, _, f)| !f.first.is_empty())
        .map(|(name, cells, f)| FoundClass { name: name.to_string(), cells, labels: f.first.clone(), count: f.places })
        .collect();
    classes.extend(sets);
    Shown { classes }
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

/// About how many bytes reading region `region`'s whole mesh reads: its files as on disk
/// (plain or compressed).
fn mesh_bytes(fs: &dyn CaseFs, region: &str) -> u64 {
    let dir = mesh_dir(region);
    ["boundary", "points", "faces", "owner", "neighbour"]
        .iter()
        .map(|n| {
            let f = format!("{dir}/{n}");
            fs.size(&f).or_else(|| fs.size(&format!("{f}.gz"))).unwrap_or(0)
        })
        .fold(0, u64::saturating_add)
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
            notes.push(log_note("mesh.checkmesh", &l.file, format!("not used: it {what}")));
        }
        for (lr, l) in snappy.iter().filter(|(lr, l)| {
            (l.layer_phase || l.fatal.is_some()) && lr.as_ref().is_some_and(|r| !all.contains(&r.as_str()))
        }) {
            let what = lr.as_deref().unwrap_or_default();
            notes.push(log_note(
                "mesh.snappyhexmesh",
                &l.file,
                format!("not used: it meshed region `{what}`, which this case doesn't have"),
            ));
        }
    }
    // The progress bar: the mesh files' bytes read of those expected, and the faces each quality
    // pass has gone through (from the counts `owner` notes; a mesh without them adds its own once
    // read).
    let progress = s.cancel.progress();
    let noted: Vec<Option<u64>> = names
        .iter()
        .map(|r| umcf_foam_mesh::counts(fs, &mesh_dir(r), &Limits::default()).ok().and_then(|c| c.faces))
        .collect();
    progress.start(noted.iter().flatten().map(|f| quality_steps(*f, all_geometry)).fold(0, u64::saturating_add));
    progress.expect(names.iter().map(|r| mesh_bytes(fs, r)).fold(0, u64::saturating_add));
    let mut regions = Vec::new();
    let mut lines = Vec::new();
    let (mut errors, mut warnings) = (0, 0);
    // What was found and the meshes read, for the 3D view: a mesh is kept while it fits in what
    // the budget for hidden objects leaves.
    let mut found: Vec<(String, Arc<Shown>)> = Vec::new();
    let mut hand: Vec<Whole> = Vec::new();
    let mut room = room_left(s, &names);
    // The regions whose mesh was read (and so may differ from what the scene kept of it).
    let mut read: Vec<String> = Vec::new();
    for (r, noted) in names.iter().zip(&noted) {
        let name = if r.is_empty() { "mesh" } else { r.as_str() };
        let candidates: Vec<&CheckMeshLog> = logs.iter().filter(|l| &l.region == r).collect();
        let mut d: Vec<Diagnostic> = Vec::new();
        // A mesh that doesn't read is that region's error; the others are still checked.
        let before = progress.get().bytes;
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
                        "mesh.checkmesh",
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
                // The steps of the faces it has, not of those its `owner` notes (a stale note).
                let faces = mesh.n_faces() as u64;
                if *noted != Some(faces) {
                    progress.add_steps(quality_steps(faces, all_geometry));
                    if let Some(f) = noted {
                        progress.advance(quality_steps(*f, all_geometry));
                    }
                }
                let q = quality_with(mesh, all_geometry, &s.cancel)
                    .ok_or_else(|| Error::Cancelled { id: CHECK.id.to_string() })?;
                d.extend(diagnostics(&q, r));
                let (sets, from_sets) = checkmesh_sets(fs, r, mesh, &mut d);
                found.push((r.clone(), Arc::new(shown(&q, from_sets))));
                (Some(q), sets)
            }
            // A read stopped by a cancel: the command stops, not the region.
            Err(_) if s.cancel.is_cancelled() => return Err(Error::Cancelled { id: CHECK.id.to_string() }),
            Err(e) => {
                // Its quality steps, counted up front, are done: there is nothing to check; so are
                // the bytes of its files that weren't read.
                if let Some(f) = noted {
                    progress.advance(quality_steps(*f, all_geometry));
                }
                let read = progress.get().bytes.saturating_sub(before);
                progress.read(mesh_bytes(fs, r).saturating_sub(read));
                d.insert(0, unreadable(r, &e.to_string()));
                (None, Vec::new())
            }
        };
        let mut from_logs: Vec<Diagnostic> =
            mine.iter().flat_map(|l| checkmesh_log_diagnostics(l, q.as_ref())).collect();
        // snappyHexMesh's layers, for the mesh it wrote (one with layers only: a log without a
        // layer phase says nothing about them).
        let mut layer_logs: Vec<&SnappyLog> = Vec::new();
        // A run that stopped with a fatal error says so, layer phase or not.
        for (k, (lr, l)) in snappy.iter().enumerate().filter(|(_, (_, l))| l.layer_phase || l.fatal.is_some()) {
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
                        "mesh.snappyhexmesh",
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
        if let (Ok(m), true) = (mesh, q.is_some()) {
            read.push(r.clone());
            if m.bytes() <= room {
                room -= m.bytes();
                hand.push(Whole::of(r, Arc::new(m)));
            }
        }
    }
    // Logs of the default mesh in a case without one that no region's mesh matches.
    if region.is_none() && !has_default {
        for ((lr, l), used) in snappy.iter().zip(&snappy_used) {
            if !(lr.is_none() && (l.layer_phase || l.fatal.is_some()) && !used) {
                continue;
            }
            // A run that failed and fits no region (it wrote no mesh, or not one that was
            // moved into a region): still say it failed, for the case (counted as a warning).
            if let Some(f) = &l.fatal {
                let place = match [l.cells, l.faces, l.points] {
                    [None, None, None] => "before it wrote a mesh".to_string(),
                    _ => format!("with a mesh of {} that no region has", mesh_size(l.cells, l.faces, l.points)),
                };
                let mut d = log_note(
                    "mesh.snappyhexmesh",
                    &l.file,
                    format!(
                        "snappyHexMesh stopped with a fatal error {place} (no -region, so for which region isn't known): {f}"
                    ),
                );
                d.level = Level::Warning;
                notes.push(d);
                warnings += 1;
            } else {
                notes.push(log_note(
                    "mesh.snappyhexmesh",
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
    s.found.retain(|(r, _)| !names.contains(r));
    s.found.extend(found);
    s.handed.retain(|w| !names.iter().any(|r| w.is(r)));
    // A mesh read and not handed over: the scene doesn't use what it kept of the region before.
    for r in read.iter().filter(|r| !hand.iter().any(|w| w.is(r))) {
        if !s.outdated.contains(r) {
            s.outdated.push(r.clone());
        }
    }
    s.outdated.retain(|r| !hand.iter().any(|w| w.is(r)));
    s.handed.extend(hand);
    let data = json!({
        "errors": errors,
        "warnings": warnings,
        "regions": regions,
        "diagnostics": notes.iter().map(diagnostic).collect::<Vec<_>>(),
    });
    Ok(Outcome::new(lines.join("; "), data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_found_is_kept_by_problem_faces_or_cells() {
        let mut q = Quality::default();
        q.skew_faces.first = vec![4, 7];
        q.skew_faces.places = 2000;
        q.open_cells.first = vec![1];
        let mut all = AllGeometry::default();
        all.concave_cells.first = vec![2];
        q.all = Some(all);
        let set = FoundClass { name: "checkMesh: concave faces".into(), cells: false, labels: vec![3], count: 1 };
        let f = shown(&q, vec![set]);
        assert_eq!(f.classes[1].count, 2000);
        let got: Vec<(&str, bool, &[u32])> =
            f.classes.iter().map(|c| (c.name.as_str(), c.cells, c.labels.as_slice())).collect();
        assert_eq!(
            got,
            [
                ("open cells", true, &[1][..]),
                ("skew faces", false, &[4, 7][..]),
                ("concave cells", true, &[2][..]),
                ("checkMesh: concave faces", false, &[3][..]),
            ]
        );
        assert!(shown(&Quality::default(), Vec::new()).classes.is_empty());
    }
}
