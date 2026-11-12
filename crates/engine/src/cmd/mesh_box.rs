//! `mesh.box`: a region's blocks made from a box around the case's surfaces (their bounds and
//! a margin on each side, with a cell size or the cells along each axis), kept in the region's
//! `box` entry of the workflow description, and its `blockMeshDict` written from them as
//! `mesh.block` writes it.

use serde_json::{Map, Value, json};
use umcf_geom::BBox;
use umcf_geom::stl;
use umcf_model::blocks::{Grading, Side};
use umcf_model::description::{PATH, Role};
use umcf_model::description_edit::PatchEdit;
use umcf_model::mesh_box::{BoxCells, MAX_SURFACES, MeshBox, SIDES};

use super::geometry::{SURFACE_DIR, limits, surface_files, surface_path};
use super::mesh_block;
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const BOX: CommandSpec = CommandSpec {
    id: "mesh.box",
    label: "Box mesh",
    menu: "Mesh/Box around the surfaces",
    shortcut: None,
    params: "region: a region of system/umcfWorkflow whose mesh is `blocks` or built in constant/polyMesh by steps starting with blockMesh (as for mesh.block; a region without a mesh is given `mesh blocks;`). surfaces: the surfaces in constant/triSurface the box is around, [\"buildings.stl\", …] (default: the region's box's, else every STL surface there). margin: how far the box reaches past the surfaces' bounds, a number (every side but the ground, which is 0) or six [west, east, south, north, ground, top] (west/east along x, south/north along y, ground/top along z). The ground is the side the gravity points to (constant/<region>/g, else constant/g, else z down): ground with g along -z, south with g along -y. cellSize: the cells' size, cubes (the box grows by less than a cell so each axis is a whole number of cells: at the far end of each axis, and away from the ground along the gravity; a length within rounding of whole cells keeps its end, the cells a size within rounding); or cells: [nx, ny, nz], the box kept as the bounds and margins make it. Each defaults to the region's box's; a region without a box needs margin and cellSize or cells. The box is one block with one span along each axis, its grading 1, at most 1000000000 cells. patches, types, noFaces: as mesh.block takes them (a region without blocks gets ground and top across the gravity, inlet and outlet at the ends of the first other axis, side on both ends of the last: along z {\"west\": \"inlet\", \"east\": \"outlet\", \"south\": \"side\", \"north\": \"side\", \"ground\": \"ground\", \"top\": \"top\"}, which patches changes; in a fluid region those of these names the description doesn't list are added with their roles of the same names, on whichever side patches puts them). Blocks that weren't made from the region's box (given by mesh.block, read by workflow.import, or kept when remove took the box out), or edited since the box made them (its entry keeps the checksum of the blocks it made, made; without one: blocks of another shape than one span along each axis, graded 1, nothing missing), are kept unless reshape: true, which replaces their stations, missing blocks and single faces and drops the patch around the missing blocks (the description keeps the rest; edit.undo takes it all back). replace, dropPatches, dropEntries, unreadPatches, dropSettings: as mesh.block takes them, for the blockMeshDict there; the outcome is mesh.block's, with the box, the surfaces' bounds, boxChanged and the ground's side (ground; without a g read, z is taken as up, and the message says so). With only region, the blocks are made again from the box there (after the surfaces changed). remove: true takes the box out of the description (the blocks stay, as they are). The surfaces are read one after the other; the call can be cancelled between them. The box goes into the region's box entry of system/umcfWorkflow (only the keys that change), with made: the checksum of the blocks it made, and a later mesh.block that changes the grid's shape takes it out. case.save writes it all; edit.undo takes it back. The case check (workflow.blocks) names a box without blocks, one with a problem of its own (a surface name that isn't an STL file of constant/triSurface, a negative margin), and surfaces the case doesn't have.",
    enabled: needs_case,
    run: make_box,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// What `mesh.box` passes on to `mesh.block` as it is.
const PASSED: [&str; 8] =
    ["patches", "types", "noFaces", "replace", "dropPatches", "dropEntries", "unreadPatches", "dropSettings"];

/// The sides' patches of a region that has no blocks yet, the ground and top those across the
/// gravity: along z, `west` inlet, `east` outlet, `south` and `north` side, `ground` ground and
/// `top` top; along another axis the inlet and outlet on the first of the two others, the
/// sides on the second.
fn default_patches(ground: Side, top: Side) -> Vec<(&'static str, &'static str)> {
    let up = ground.axis().map_or(2, |(a, _)| a);
    let across: Vec<usize> = (0..3).filter(|a| *a != up).collect();
    let mut out = vec![(ground.name(), "ground"), (top.name(), "top")];
    let side = |a: usize, end: bool| Side::ALL.into_iter().find(|s| s.axis() == Some((a, end))).map(Side::name);
    if let Some(a) = across.first().copied() {
        out.extend(side(a, false).map(|s| (s, "inlet")));
        out.extend(side(a, true).map(|s| (s, "outlet")));
    }
    if let Some(a) = across.get(1).copied() {
        out.extend(side(a, false).map(|s| (s, "side")));
        out.extend(side(a, true).map(|s| (s, "side")));
    }
    // West to top, as the description lists them.
    out.sort_by_key(|(s, _)| Side::parse(s));
    out
}

fn bad(m: impl std::fmt::Display) -> Error {
    Error::Params(format!("{}: {m}", BOX.id))
}

/// `v` as six margins; one number is every side's but the ground's, which is 0.
fn margin_of(v: &Value, ground: Side) -> Result<[f64; 6], Error> {
    let shape = || bad("margin is a distance, or six [west, east, south, north, ground, top]");
    match v {
        Value::Number(n) => {
            let m = n.as_f64().ok_or_else(shape)?;
            let mut out = [m; 6];
            if let Some(slot) = Side::ALL.iter().position(|s| *s == ground).and_then(|i| out.get_mut(i)) {
                *slot = 0.0;
            }
            Ok(out)
        }
        Value::Array(a) if a.len() == 6 => {
            let mut out = [0.0; 6];
            for (slot, x) in out.iter_mut().zip(a) {
                *slot = x.as_f64().ok_or_else(shape)?;
            }
            Ok(out)
        }
        _ => Err(shape()),
    }
}

fn cells_of(v: &Value) -> Result<[u32; 3], Error> {
    let shape = || bad("cells is [nx, ny, nz], whole numbers");
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(shape)?;
    let mut out = [0u32; 3];
    for (slot, x) in out.iter_mut().zip(a) {
        *slot = x.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(shape)?;
    }
    Ok(out)
}

/// A surface's file name in constant/triSurface, from `buildings.stl` or
/// `constant/triSurface/buildings.stl`.
fn surface_name(name: &str) -> Result<String, Error> {
    let rel = surface_path(BOX.id, name, false)?;
    Ok(rel.strip_prefix(&format!("{SURFACE_DIR}/")).unwrap_or(&rel).to_string())
}

/// The bounds of the surfaces `names`, each read as OpenFOAM would; an error naming one that
/// isn't there, doesn't read, or has no finite point.
fn bounds(s: &Session, names: &[String]) -> Result<BBox, Error> {
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let mut all: Option<BBox> = None;
    for n in names {
        if s.cancel.is_cancelled() {
            return Err(Error::Cancelled { id: BOX.id.to_string() });
        }
        let rel = format!("{SURFACE_DIR}/{n}");
        let bytes = case.file(fs, &rel)?.ok_or_else(|| {
            Error::Failed(format!("{}: there is no surface {rel} (geometry.import brings one)", BOX.id))
        })?;
        let surface = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{}: {rel}: {e}", BOX.id)))?;
        let b = surface
            .bbox()
            .ok_or_else(|| Error::Failed(format!("{}: {rel} has no point with finite coordinates", BOX.id)))?;
        all = Some(match all {
            None => b,
            Some(a) => BBox::of([a.min, a.max, b.min, b.max]).unwrap_or(a),
        });
    }
    all.ok_or_else(|| bad("the box is around no surface"))
}

/// A box as JSON: its surfaces, its margins by side, and its cell size or cells.
pub(crate) fn box_json(b: &MeshBox) -> Value {
    json!({
        "surfaces": b.surfaces,
        "margin": SIDES.iter().zip(b.margin).map(|(k, m)| (k.to_string(), json!(m))).collect::<Map<String, Value>>(),
        "cellSize": match &b.cells { BoxCells::Size(h) => json!(h), BoxCells::Counts(_) => Value::Null },
        "cells": match &b.cells { BoxCells::Counts(n) => json!(n), BoxCells::Size(_) => Value::Null },
    })
}

impl Session {
    /// The bounds of the case's surfaces `names` (file names in constant/triSurface), each read
    /// as OpenFOAM would and as `mesh.box` reads them: for a preview of a box around them. An
    /// error names one that isn't there, doesn't read, or has no finite point.
    pub fn surface_bounds(&self, names: &[String]) -> Result<BBox, Error> {
        let names = names.iter().map(|n| surface_name(n)).collect::<Result<Vec<_>, _>>()?;
        bounds(self, &names)
    }
}

/// `mesh.box`: the region's blocks made from a box around the surfaces.
fn make_box(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let allowed: Vec<&str> =
        ["region", "surfaces", "margin", "cellSize", "cells", "reshape", "remove"].into_iter().chain(PASSED).collect();
    let p = Params::new(BOX.id, p, &allowed)?;
    let region = p.required_str("region")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let reshape = p.bool("reshape")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let Some((d, _)) = o.case.description() else {
        return Err(Error::Failed(format!(
            "the case has no {PATH} (or it doesn't load): workflow.import writes one from the case first"
        )));
    };
    let Some(r) = d.region(region) else {
        return Err(Error::Failed(format!("region {region} is not in {PATH}: workflow.region adds it")));
    };
    let r = r.clone();
    if remove {
        let others: Vec<&str> = allowed.iter().copied().filter(|k| *k != "region" && *k != "remove").collect();
        if others.iter().any(|k| p.value(k).is_some()) {
            return Err(bad("remove: true takes the box out: give nothing else but the region"));
        }
        if !o.case.undescribe_box(region)? {
            return Err(Error::Failed(format!("{}: region {region} has no box in {PATH}", BOX.id)));
        }
        let message = format!("{PATH}: the box of region {region} taken out (its blocks stay as they are)");
        return Ok(Outcome::new(message, json!({ "region": region, "file": PATH, "box": Value::Null })));
    }
    // A box entry that doesn't read: what isn't read would be taken for absent.
    o.case.box_read(region).map_err(|e| Error::Failed(format!("{}: {e}", BOX.id)))?;
    let had = r.mesh_box.clone();
    let (ground, top, by_g) = umcf_model::blocks::ground_and_top(&o.case, region);
    // Blocks not made from the box (none there, or blocks of another shape than a box's: one
    // span along each axis, graded 1, nothing missing, no single faces, as after a hand edit):
    // replaced only when asked.
    let boxlike = r.blocks.as_ref().is_some_and(|g| {
        g.axes.iter().all(|a| {
            a.stations.len() == 2 && a.spans.len() == 1 && a.spans.iter().all(|s| s.grading == Grading::Ratio(1.0))
        }) && g.missing.is_empty()
            && g.faces.is_empty()
    });
    // The checksum of the blocks the box last made: the blocks as it left them (a hand edit
    // that keeps a box's shape, other stations or cells, is told too; one of the box entry
    // itself, or of the surfaces, isn't an edit of the blocks).
    let made_then = match (&had, &r.blocks) {
        (Some(h), Some(g)) => match h.made {
            Some(c) => umcf_model::blocks::shape_checksum(g) == c,
            None => boxlike,
        },
        _ => false,
    };
    let replaces = r.blocks.is_some() && !made_then;
    if replaces && !reshape {
        let why = if had.is_none() {
            "blocks that weren't made from its box (it has none now: given by mesh.block, read by workflow.import, or its box taken out with remove)"
        } else {
            "blocks edited since its box made them"
        };
        return Err(Error::Failed(format!(
            "{}: region {region} has {why}: pass \"reshape\": true to replace their stations, missing blocks and single faces with the box",
            BOX.id
        )));
    }
    let surfaces: Vec<String> = match p.value("surfaces") {
        Some(v) => {
            let a = v.as_array().ok_or_else(|| bad("surfaces is a list of names, [\"buildings.stl\", …]"))?;
            if a.len() > MAX_SURFACES {
                return Err(bad(format!("surfaces names {} surfaces, {MAX_SURFACES} at most", a.len())));
            }
            a.iter()
                .map(|x| x.as_str().ok_or_else(|| bad("surfaces is a list of names, [\"buildings.stl\", …]")))
                .map(|n| n.and_then(surface_name))
                .collect::<Result<_, _>>()?
        }
        None => match &had {
            Some(b) => b.surfaces.clone(),
            None => {
                let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
                let (names, listed) = surface_files(case, fs, false);
                if let Err(e) = listed
                    && names.is_empty()
                {
                    return Err(Error::Failed(format!("{}: {SURFACE_DIR} can't be listed: {e}", BOX.id)));
                }
                let names: Vec<String> =
                    names.iter().map(|n| n.strip_prefix(&format!("{SURFACE_DIR}/")).unwrap_or(n).to_string()).collect();
                if names.is_empty() {
                    return Err(Error::Failed(format!(
                        "{}: there is no STL surface in {SURFACE_DIR} to put a box around (geometry.import brings one)",
                        BOX.id
                    )));
                }
                if names.len() > MAX_SURFACES {
                    return Err(bad(format!(
                        "{SURFACE_DIR} has {} surfaces, more than a box takes ({MAX_SURFACES}): name them in surfaces",
                        names.len()
                    )));
                }
                names
            }
        },
    };
    let margin = match (p.value("margin"), &had) {
        (Some(v), _) => margin_of(v, ground)?,
        (None, Some(b)) => b.margin,
        (None, None) => return Err(bad(format!("region {region} has no box yet: give margin"))),
    };
    let cells = match (p.number("cellSize")?, p.value("cells"), &had) {
        (Some(_), Some(_), _) => return Err(bad("give cellSize or cells, not both")),
        (Some(h), None, _) => BoxCells::Size(h),
        (None, Some(v), _) => BoxCells::Counts(cells_of(v)?),
        (None, None, Some(b)) => b.cells.clone(),
        (None, None, None) => return Err(bad(format!("region {region} has no box yet: give cellSize or cells"))),
    };
    let mut b = MeshBox { surfaces, margin, cells, made: None };
    if let Some((k, m)) = b.problems().first() {
        return Err(bad(format!("{k} {m}")));
    }
    let bbox = bounds(s, &b.surfaces)?;
    let at = |v: umcf_geom::Vec3| [v.x, v.y, v.z];
    let axes = b.axes(at(bbox.min), at(bbox.max), ground).map_err(bad)?;
    let made = umcf_model::blocks::BlockGrid { axes: axes.clone(), ..Default::default() };
    b.made = Some(umcf_model::blocks::shape_checksum(&made));
    // The blocks, as mesh.block takes them: the box's stations, no missing blocks or single
    // faces, the sides' patches the region has (or the defaults), and what the call passes on.
    let mut call = Map::new();
    let mut dropped_missing = false;
    call.insert("region".to_string(), json!(region));
    let grid = umcf_model::blocks::BlockGrid { axes: axes.clone(), ..Default::default() };
    let shape = mesh_block::grid_json(&grid);
    for k in ["x", "y", "z"] {
        call.insert(k.to_string(), shape[k].clone());
    }
    if let Some(g) = &r.blocks {
        if !g.missing.is_empty() {
            call.insert("missing".to_string(), json!([]));
        }
        // The patch around the missing blocks has no faces without them.
        if g.sides.iter().any(|(side, _)| *side == Side::Missing) {
            dropped_missing = true;
        }
        if !g.faces.is_empty() {
            let gone: Map<String, Value> = g.faces.iter().map(|(n, _)| (n.clone(), Value::Null)).collect();
            call.insert("faces".to_string(), Value::Object(gone));
        }
    }
    let mut patches: Map<String, Value> = Map::new();
    if dropped_missing {
        patches.insert("missing".to_string(), Value::Null);
    }
    let defaults = r.blocks.as_ref().is_none_or(|g| g.sides.is_empty());
    let default_list = default_patches(ground, top);
    if defaults {
        for (side, name) in &default_list {
            patches.insert(side.to_string(), json!(name));
        }
    }
    for k in PASSED {
        if let Some(v) = p.value(k) {
            if k == "patches" {
                let m =
                    v.as_object().ok_or_else(|| bad("patches is {\"west\": \"inlet\", …}, as mesh.block takes it"))?;
                patches.extend(m.iter().map(|(a, b)| (a.clone(), b.clone())));
            } else {
                call.insert(k.to_string(), v.clone());
            }
        }
    }
    // The types of patches the box no longer has (those of the single faces, of the faces
    // around the missing blocks): taken away with them, unless this call gives them.
    if let Some(g) = &r.blocks {
        let mut kept: std::collections::HashSet<String> =
            g.sides.iter().filter(|(side, _)| !patches.contains_key(side.name())).map(|(_, n)| n.clone()).collect();
        kept.extend(patches.values().filter_map(Value::as_str).map(str::to_string));
        match p.value("noFaces").and_then(Value::as_array) {
            Some(a) => kept.extend(a.iter().filter_map(Value::as_str).map(str::to_string)),
            None => kept.extend(g.no_faces.iter().cloned()),
        }
        let gone: Vec<&String> = g.types.iter().map(|(n, _)| n).filter(|n| !kept.contains(n.as_str())).collect();
        // A types parameter that isn't a map is left for mesh.block to refuse.
        let mergeable = call.get("types").is_none_or(Value::is_object);
        if !gone.is_empty() && mergeable {
            let mut types = match call.remove("types") {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            };
            for n in gone {
                types.entry(n.clone()).or_insert(Value::Null);
            }
            call.insert("types".to_string(), Value::Object(types));
        }
    }
    if !patches.is_empty() {
        call.insert("patches".to_string(), Value::Object(patches.clone()));
    }
    // The default patches of a fluid region, which it doesn't describe yet: their roles are
    // their names (so the ground is a wall). Any error after takes them back with the rest.
    let mut roled: Vec<&str> = Vec::new();
    if defaults && r.kind == umcf_model::RegionKind::Fluid {
        // By name, on whichever side the call put it (an inlet east, say).
        for name in ["inlet", "outlet", "side", "ground", "top"] {
            let used = patches.values().any(|v| v.as_str() == Some(name));
            if used && !r.patches.iter().any(|q| q.name == name) {
                let edit = PatchEdit { role: Some(Role::parse(name)), ..PatchEdit::default() };
                s.opened_mut()?.case.describe_patch(region, name, &edit)?;
                roled.push(name);
            }
        }
    }
    let out = mesh_block::set(s, &Value::Object(call), BOX.id)?;
    let boxed = had.as_ref() != Some(&b);
    if boxed {
        s.opened_mut()?.case.describe_box(region, &b)?;
    }
    let v = |x: umcf_geom::Vec3| json!([x.x, x.y, x.z]);
    let mut data = out.data;
    if let Some(m) = data.as_object_mut() {
        m.insert("box".to_string(), box_json(&b));
        m.insert("bounds".to_string(), json!({ "min": v(bbox.min), "max": v(bbox.max) }));
        m.insert("boxChanged".to_string(), json!(boxed));
        m.insert("ground".to_string(), json!(ground.name()));
    }
    let c = axes.iter().map(|a| a.spans.first().map_or(0, |s| s.cells)).collect::<Vec<_>>();
    let size: Vec<String> = axes
        .iter()
        .map(|a| {
            umcf_model::blocks::number(
                a.stations.last().copied().unwrap_or(0.0) - a.stations.first().copied().unwrap_or(0.0),
            )
        })
        .collect();
    let mut message = format!(
        "region {region}: a box of {} m around {} ({} cells){}; {}",
        size.join(" × "),
        umcf_model::blocks::list_some(&b.surfaces.iter().map(String::as_str).collect::<Vec<_>>()),
        c.iter().map(u32::to_string).collect::<Vec<_>>().join(" × "),
        if boxed { format!(", in {PATH}") } else { String::new() },
        out.message
    );
    if !by_g {
        message.push_str("; no g read (constant/<region>/g or constant/g): z taken as up, the ground at z min");
    }
    if !roled.is_empty() {
        message.push_str(&format!("; patches of region {region} given their roles in {PATH}: {}", roled.join(", ")));
    }
    if replaces {
        message.push_str("; the blocks it replaces are in the undo history (edit.undo)");
    }
    Ok(Outcome::new(message, data))
}
