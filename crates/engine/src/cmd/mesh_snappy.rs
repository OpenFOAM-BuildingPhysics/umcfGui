//! `mesh.snappy`: a region meshed from the case's STL surfaces by snappyHexMesh, kept in the
//! region's `snappy` entry of the workflow description (the surfaces with their mode, levels,
//! layers and feature edges, refinement boxes, the inside point), and its `snappyHexMeshDict`
//! written from it, after the checks snappyHexMesh would otherwise fail on (or mesh wrongly).

use std::collections::HashSet;

use serde_json::{Map, Value, json};
use umcf_geom::{check, stl};
use umcf_model::blocks;
use umcf_model::description::{PATH, Role};
use umcf_model::description_edit::PatchEdit;
use umcf_model::snappy::{self as sn, Mode, RefineBox, Snappy, Surface};
use umcf_model::workflow::{MeshBuild, RootStep};

use super::geometry::{SURFACE_DIR, limits};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SNAPPY: CommandSpec = CommandSpec {
    id: "mesh.snappy",
    label: "Snappy mesh",
    menu: "Mesh/From the surfaces (snappyHexMesh)",
    shortcut: None,
    params: "region: a fluid or vegetation region of system/umcfWorkflow. inside: [x, y, z], a point in the cells the region keeps (locationInMesh). surfaces: {\"buildings\": {\"mode\": \"cut\", \"level\": [3, 5], \"layers\": 3, \"features\": 4, \"role\": \"coupled\"}, \"trees\": {\"file\": \"trees.stl\", \"mode\": \"refine\", \"distance\": [[2, 4], [4, 2]]}}: by name, each merged into the surface of that name (a key given null is taken away, a surface given null is taken out). file: the STL file in constant/triSurface (default <name>.stl). mode: cut (the mesh ends at it, a patch of its name, or <name>_<solid> for each solid of a surface of several; refined to level [least, most]; layers: cells added along it) or refine (only the cells within each distance of it refined to its level, the nearest first). features: the level its feature edges are refined to, from constant/triSurface/<stem>.eMesh, which surfaceFeatures writes (an uncompressed surface): system/surfaceFeaturesDict is written listing the surfaces of every region's snappy entry that ask for features, and those whose .eMesh a snappyHexMeshDict of the case's own reads (one edited by hand or held by an entry only in part) (generated, with a checksum; one edited by hand or not written by umcfGui is left, noted for the surfaces it doesn't list, unless replace), and the workflow given features yes;. role: the role given to the patch of a cutting surface (each of its <name>_<solid> patches) in the description (inlet, outlet, side, top, ground, coupled, canopy, interior, bottom, sky), which types it: ground, coupled, interior and bottom a wall. boxes: {\"street\": {\"min\": [0, 0, 0], \"max\": [100, 50, 10], \"level\": 2}}: the cells inside refined to level, merged as the surfaces. expansion: the layers' expansion ratio (default 1.2); firstLayer: the first layer's thickness (default: thicknesses relative to the cells); null takes either away. Levels are 10 at most, layers and distances 100, surfaces and boxes 1000. replace: true writes over a snappyHexMeshDict, and a surfaceFeaturesDict that doesn't list every surface asking for features, edited by hand or not written by umcfGui (not the surfaceFeaturesDict while a region's own snappyHexMeshDict reads features that can't be told: an .eMesh of no surface it names, features from a $macro or an #include other than OpenFOAM's snappyHexMeshDict.cfg header (also inside a features entry), castellatedMeshControls or geometry given twice, more than 1000 features or geometry entries, a dictionary that doesn't load) (case.save keeps them in the backup). remove: true takes the snappy entry out of the description (the dictionary stays, the case's own, and so do the surfaces in system/surfaceFeaturesDict whose .eMesh it reads; while what it reads can't be told, system/surfaceFeaturesDict isn't written, noted). The dictionary is the one the region's snappyHexMesh step names; a region meshed by blocks alone (mesh blocks;), or without a mesh, is given the steps ( blockMesh \"system/<region>/blockMeshDict\" snappyHexMesh \"system/snappyHexMeshDict.<region>\" ), and one built by steps without snappyHexMesh gets snappyHexMesh \"system/snappyHexMeshDict.<region>\" after them. Each surface is read (as OpenFOAM reads it; the call can be cancelled between them) and checked before anything is written: refused for a surface that isn't there or doesn't read, two cutting surfaces' patches or a cutting surface's and a block patch of the same name, an inside point outside the region's blocks, in one of their missing blocks, or inside a closed cutting surface (the vegetation region is the air without the trees: its point is outside them too); a file name with a space or a quote (which OpenFOAM doesn't take) or ; { } ( ) $ # (which would change the dictionary); noted for a cutting surface that isn't one closed surface (open edges, or edges of more than two triangles, as where its solids share faces: whether the inside point is inside it isn't checked, and the mesh may leak through it), a cutting surface reaching past the blocks or outside them, system/surfaceFeaturesDict written or features yes given, and an OpenFOAM-12 case with constant/geometry (where its snappyHexMesh reads the surfaces instead). Each cutting surface's solids are recorded in its entry (solids), its layers and type given to each <name>_<solid> patch; without a role, when its file gained or lost solids, a patch it makes that the description doesn't list takes the role the patches it made before all had (not when they had a partner: the new patches are then listed in the outcome). A block patch declared without faces (noFaces) isn't a clash: snappyHexMesh fills it. An inside point on a face between missing blocks only is refused. An inside point on a face between the blocks is noted. A surface switched to refine loses its level and layers (a refining surface has neither). The outcome names the dictionary, whether it was written, the surfaces' patches, bounds and whether each is closed, and the notes. case.save writes it all; edit.undo takes it back.",
    enabled: needs_case,
    run: snappy,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// What OpenFOAM-8's snappyHexMesh needs in `system/fvSchemes` and `system/fvSolution`: the
/// schemes' dictionaries, empty (it solves nothing).
const V8_STUBS: [(&str, &str); 2] = [
    (
        "fvSchemes",
        "\nddtSchemes {}\ngradSchemes {}\ndivSchemes {}\nlaplacianSchemes {}\ninterpolationSchemes {}\nsnGradSchemes {}\n",
    ),
    ("fvSolution", ""),
];

fn bad(m: impl std::fmt::Display) -> Error {
    Error::Params(format!("{}: {m}", SNAPPY.id))
}

fn failed(m: impl std::fmt::Display) -> Error {
    Error::Failed(format!("{}: {m}", SNAPPY.id))
}

fn point_of(v: &Value, what: &str) -> Result<[f64; 3], Error> {
    let shape = || bad(format!("{what} is a point, [x, y, z]"));
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(shape)?;
    let mut out = [0.0; 3];
    for (slot, x) in out.iter_mut().zip(a) {
        *slot = x.as_f64().ok_or_else(shape)?;
    }
    Ok(out)
}

fn whole(v: &Value, what: &str) -> Result<u32, Error> {
    v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(format!("{what} is a whole number")))
}

/// The surfaces' changes merged into `list`; the roles given to cutting surfaces' patches.
fn merge_surfaces(list: &mut Vec<Surface>, v: &Value) -> Result<Vec<(String, Option<Role>)>, Error> {
    let m = v.as_object().ok_or_else(|| bad("surfaces is {\"buildings\": {…}, …}"))?;
    let mut roles = Vec::new();
    for (name, change) in m {
        let at = format!("surfaces.{name}");
        if change.is_null() {
            list.retain(|s| s.name != *name);
            continue;
        }
        let c = change.as_object().ok_or_else(|| bad(format!("{at} is {{\"mode\": …}} or null")))?;
        let mut s = list.iter().find(|s| s.name == *name).cloned().unwrap_or(Surface {
            name: name.clone(),
            file: Surface::default_file(name),
            mode: Mode::Cut,
            level: None,
            distance: Vec::new(),
            layers: 0,
            features: None,
            solids: Vec::new(),
        });
        let fresh = !list.iter().any(|x| x.name == *name);
        if fresh && !c.contains_key("mode") {
            return Err(bad(format!("{at} is new: give its mode, cut or refine")));
        }
        for (k, x) in c {
            let here = format!("{at}.{k}");
            match (k.as_str(), x) {
                ("file", Value::Null) => s.file = Surface::default_file(name),
                ("file", Value::String(f)) => {
                    s.file = f.strip_prefix(&format!("{SURFACE_DIR}/")).unwrap_or(f).to_string();
                }
                ("mode", Value::String(w)) => {
                    s.mode = Mode::parse(w).ok_or_else(|| bad(format!("{here} is cut or refine")))?;
                }
                ("level", Value::Null) => s.level = None,
                ("level", Value::Array(a)) if a.len() == 2 => {
                    s.level = Some((whole(&a[0], &here)?, whole(&a[1], &here)?));
                }
                ("distance", Value::Null) => s.distance.clear(),
                ("distance", Value::Array(a)) => {
                    s.distance = a
                        .iter()
                        .map(|pair| match pair.as_array().map(Vec::as_slice) {
                            Some([d, l]) => Ok((
                                d.as_f64().ok_or_else(|| bad(format!("{here} is [[distance, level], …]")))?,
                                whole(l, &here)?,
                            )),
                            _ => Err(bad(format!("{here} is [[distance, level], …]"))),
                        })
                        .collect::<Result<_, _>>()?;
                }
                ("layers", Value::Null) => s.layers = 0,
                ("layers", x) => s.layers = whole(x, &here)?,
                ("features", Value::Null) => s.features = None,
                ("features", x) => s.features = Some(whole(x, &here)?),
                ("role", Value::Null) => roles.push((name.clone(), None)),
                ("role", Value::String(w)) => {
                    let role = Role::parse(w).ok_or_else(|| {
                        bad(format!(
                            "{here} is inlet, outlet, side, top, ground, coupled, canopy, interior, bottom or sky"
                        ))
                    })?;
                    roles.push((name.clone(), Some(role)));
                }
                ("file" | "mode" | "level" | "distance" | "role", _) => {
                    return Err(bad(format!("{here} isn't of its kind")));
                }
                _ => {
                    return Err(bad(format!(
                        "{here} isn't a key of a surface: file, mode, level, distance, layers, features or role"
                    )));
                }
            }
        }
        // A refining surface has no level of its own, nor layers.
        if s.mode == Mode::Refine && !c.contains_key("level") {
            s.level = None;
        }
        if s.mode == Mode::Refine && !c.contains_key("layers") {
            s.layers = 0;
        }
        match list.iter_mut().find(|x| x.name == *name) {
            Some(slot) => *slot = s,
            None => list.push(s),
        }
    }
    Ok(roles)
}

fn merge_boxes(list: &mut Vec<RefineBox>, v: &Value) -> Result<(), Error> {
    let m = v.as_object().ok_or_else(|| bad("boxes is {\"street\": {…}, …}"))?;
    for (name, change) in m {
        let at = format!("boxes.{name}");
        if change.is_null() {
            list.retain(|b| b.name != *name);
            continue;
        }
        let c = change.as_object().ok_or_else(|| bad(format!("{at} is {{\"min\": …}} or null")))?;
        let had = list.iter().find(|b| b.name == *name).cloned();
        let get = |k: &str| c.get(k).filter(|x| !x.is_null());
        let min = match (get("min"), &had) {
            (Some(x), _) => point_of(x, &format!("{at}.min"))?,
            (None, Some(h)) => h.min,
            (None, None) => return Err(bad(format!("{at} is new: give min, max and level"))),
        };
        let max = match (get("max"), &had) {
            (Some(x), _) => point_of(x, &format!("{at}.max"))?,
            (None, Some(h)) => h.max,
            (None, None) => return Err(bad(format!("{at} is new: give min, max and level"))),
        };
        let level = match (get("level"), &had) {
            (Some(x), _) => whole(x, &format!("{at}.level"))?,
            (None, Some(h)) => h.level,
            (None, None) => return Err(bad(format!("{at} is new: give min, max and level"))),
        };
        if let Some(k) = c.keys().find(|k| !matches!(k.as_str(), "min" | "max" | "level")) {
            return Err(bad(format!("{at}.{k} isn't a key of a box: min, max or level")));
        }
        let b = RefineBox { name: name.clone(), min, max, level };
        match list.iter_mut().find(|x| x.name == *name) {
            Some(slot) => *slot = b,
            None => list.push(b),
        }
    }
    Ok(())
}

/// Whether `p` is a path of the open case, on disk or put.
fn o_exists(s: &Session, p: &str) -> bool {
    s.case()
        .is_some_and(|c| c.exists(p) == Some(true) || c.put_paths().any(|x| x == p || x.starts_with(&format!("{p}/"))))
}

/// The missing block of `g` that `p` is in, if any: every block whose stations hold it (two
/// or more for a point on a station between them) is missing. The desktop's snappy card says
/// it before *OK* by the same rule.
pub fn missing_at(g: &blocks::BlockGrid, p: [f64; 3]) -> Option<[usize; 3]> {
    // Along each axis the blocks that hold the coordinate: one inside its span, the two either
    // side of a station between spans, the one at an end.
    let mut along: [Vec<usize>; 3] = Default::default();
    for (a, slot) in along.iter_mut().enumerate() {
        let st = &g.axes.get(a)?.stations;
        let x = *p.get(a)?;
        let spans = st.len().checked_sub(1)?;
        *slot = (0..spans)
            .filter(|i| matches!((st.get(*i), st.get(i + 1)), (Some(lo), Some(hi)) if *lo <= x && x <= *hi))
            .collect();
        if slot.is_empty() {
            return None;
        }
    }
    // In a missing block only when every block that holds it is missing.
    let missing: std::collections::HashSet<[usize; 3]> = g.missing.iter().copied().collect();
    let mut first = None;
    for i in &along[0] {
        for j in &along[1] {
            for k in &along[2] {
                let b = [*i, *j, *k];
                if !missing.contains(&b) {
                    return None;
                }
                first.get_or_insert(b);
            }
        }
    }
    first
}

/// Whether `p` lies on a station of `g`'s grid: on a face between blocks (and cells).
fn on_station(g: &blocks::BlockGrid, p: [f64; 3]) -> bool {
    g.axes.iter().zip(p).any(|(a, x)| a.stations.contains(&x))
}

/// What one surface is, read.
struct Read {
    name: String,
    file: String,
    mode: Mode,
    bounds: Option<umcf_geom::BBox>,
    closed: bool,
    patches: Vec<String>,
    /// Whether the inside point is inside it: `None` when not closed, or it can't be told.
    holds_inside: Option<bool>,
}

/// A snappy entry as JSON, as the command takes it (with each surface's `solids`, which it
/// records itself).
pub fn snappy_json(s: &Snappy) -> Value {
    let surfaces: Map<String, Value> = s
        .surfaces
        .iter()
        .map(|x| {
            (
                x.name.clone(),
                json!({
                    "file": x.file,
                    "mode": x.mode.name(),
                    "level": x.level.map(|(a, b)| json!([a, b])),
                    "distance": x.distance.iter().map(|(d, l)| json!([d, l])).collect::<Vec<_>>(),
                    "layers": x.layers,
                    "features": x.features,
                    "solids": x.solids,
                }),
            )
        })
        .collect();
    let boxes: Map<String, Value> =
        s.boxes.iter().map(|b| (b.name.clone(), json!({ "min": b.min, "max": b.max, "level": b.level }))).collect();
    json!({
        "inside": s.inside,
        "surfaces": surfaces,
        "boxes": boxes,
        "expansion": s.expansion,
        "firstLayer": s.first_layer,
    })
}

/// Brings `system/surfaceFeaturesDict` and the description's `features` in line with the
/// regions' snappy entries as the description now has them, and with each `snappyHexMeshDict`
/// of the case's own that umcfGui didn't write from an entry ([`snappy_import::feature_needs`]): the
/// dictionary written to
/// list the surfaces that ask for feature edges (over one umcfGui wrote, or with `replace` over
/// any; one edited by hand is otherwise left, noted for the files it doesn't list), and
/// `features yes` given when one asks. Not written while one of those dictionaries reads
/// `.eMesh` files whose surface can't be told (noted). Whether the dictionary was written.
fn sync_features(o: &mut crate::Opened, replace: bool, notes: &mut Vec<String>) -> Result<bool, Error> {
    let Some((d, _)) = o.case.description() else {
        return Err(failed(format!("{PATH} doesn't load after the change: {} isn't written", sn::FEATURES_DICT)));
    };
    let needs = umcf_model::snappy_import::feature_needs(&o.case, &d);
    let files = needs.files;
    let at = sn::FEATURES_DICT;
    let text = sn::features_dict(&files);
    let write = match o.case.dict(at) {
        _ if !needs.unread.is_empty() => {
            notes.push(format!(
                "{at} isn't written: a region's own snappyHexMeshDict reads .eMesh files whose surface can't be told ({}), which it would leave out",
                needs.unread.join("; ")
            ));
            false
        }
        Some(doc) => {
            let old = doc.to_string();
            if old == text {
                false
            } else if blocks::untouched(&old) {
                true
            } else {
                // The case's own: kept while it lists every surface asking for features.
                let listed = sn::features_listed(&doc.root);
                let missing: Vec<&str> =
                    files.iter().filter(|f| !listed.contains(f.as_str())).map(String::as_str).collect();
                if missing.is_empty() {
                    false
                } else if replace {
                    true
                } else {
                    notes.push(format!(
                        "{at} was edited by hand or not written by umcfGui and doesn't list {}: surfaceFeatures makes no .eMesh for them (replace: true writes it from the snappy entries)",
                        blocks::list_some(&missing)
                    ));
                    false
                }
            }
        }
        None if o.case.exists(at) == Some(true) => {
            if !files.is_empty() {
                notes.push(format!("{at} is there but doesn't load as a dictionary: it isn't written over"));
            }
            false
        }
        None => !files.is_empty(),
    };
    if write {
        o.case.put_dict(o.fs.as_ref(), at, &text).map_err(|e| match e {
            umcf_model::Error::IsLink(_) => Error::Failed(format!("{at} is a symbolic link: it isn't written over")),
            e => e.into(),
        })?;
        let listing = if files.is_empty() { "no surface".to_string() } else { files.join(", ") };
        notes.push(format!("{at} written, listing the surfaces whose features are asked for: {listing}"));
    }
    let on = o.case.description().is_some_and(|(d, _)| d.features);
    if !files.is_empty() && !on {
        o.case.describe_features(true)?;
        notes.push(format!(
            "features yes in {PATH}: surfaceFeatures runs first (workflow.scripts writes it into Allprepare)"
        ));
    }
    Ok(write)
}

fn snappy(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let allowed = ["region", "inside", "surfaces", "boxes", "expansion", "firstLayer", "replace", "remove"];
    let p = Params::new(SNAPPY.id, p, &allowed)?;
    let region = p.required_str("region")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let replace = p.bool("replace")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let Some((d, read_problems)) = o.case.description() else {
        return Err(Error::Failed(format!(
            "the case has no {PATH} (or it doesn't load): workflow.import writes one from the case first"
        )));
    };
    let Some(r) = d.region(region) else {
        return Err(Error::Failed(format!("region {region} is not in {PATH}: workflow.region adds it")));
    };
    let r = r.clone();
    let v8 = d.version.or(o.case.layout.version) == Some(umcf_core::OfVersion::V8);
    let others: Vec<(String, Option<MeshBuild>)> =
        d.regions.iter().filter(|x| x.name != region).map(|x| (x.name.clone(), x.mesh.clone())).collect();
    if remove {
        if allowed.iter().any(|k| !matches!(*k, "region" | "remove") && p.value(k).is_some()) {
            return Err(bad("remove: true takes the snappy entry out: give nothing else but the region"));
        }
        if !o.case.undescribe_snappy(region)? {
            return Err(failed(format!("region {region} has no snappy entry in {PATH}")));
        }
        let mut notes = Vec::new();
        sync_features(o, false, &mut notes)?;
        let mut message = format!(
            "{PATH}: the snappy entry of region {region} taken out (its snappyHexMeshDict stays as it is, the case's own)"
        );
        for n in &notes {
            message.push_str(&format!("; {n}"));
        }
        return Ok(Outcome::new(
            message,
            json!({ "region": region, "file": PATH, "snappy": Value::Null, "notes": notes }),
        ));
    }
    if r.kind == umcf_model::RegionKind::Solid {
        return Err(failed(format!(
            "region {region} is a solid: solids are meshed by blocks or extruded, not by snappyHexMesh"
        )));
    }
    o.case.snappy_read(region).map_err(failed)?;
    // The mesh: one that runs snappyHexMesh, given one when it hasn't.
    let mesh_at = format!("regions/{region}/mesh");
    if let Some(pr) = read_problems.iter().find(|x| x.at == mesh_at) {
        return Err(failed(format!(
            "the mesh of region {region} in {PATH} doesn't read ({}): mend it by hand first",
            pr.message
        )));
    }
    let own = format!("system/snappyHexMeshDict.{region}");
    let new_mesh: Option<Vec<RootStep>> = match &r.mesh {
        None | Some(MeshBuild::Block) => Some(vec![
            RootStep::BlockMesh(format!("system/{region}/blockMeshDict")),
            RootStep::SnappyHexMesh(own.clone()),
        ]),
        Some(MeshBuild::Root(steps)) if sn::dict_path(&MeshBuild::Root(steps.clone())).is_none() => {
            let mut steps = steps.clone();
            steps.push(RootStep::SnappyHexMesh(own.clone()));
            Some(steps)
        }
        Some(MeshBuild::Root(_)) => None,
        Some(MeshBuild::Split(_)) => {
            return Err(failed(format!(
                "region {region} is split from another region's mesh: give it another mesh first (mesh.split remove: true)"
            )));
        }
    };
    let mesh = new_mesh.clone().map(MeshBuild::Root).or_else(|| r.mesh.clone());
    let path = mesh.as_ref().and_then(sn::dict_path).ok_or_else(|| failed("no snappyHexMesh step"))?;
    let sharing: Vec<&str> = others
        .iter()
        .filter(|(_, m)| m.as_ref().and_then(sn::dict_path).as_deref() == Some(path.as_str()))
        .map(|(n, _)| n.as_str())
        .collect();
    if !sharing.is_empty() {
        return Err(failed(format!(
            "{path} is read by the mesh of region {} too: give region {region} a dictionary of its own (its snappyHexMesh step in {PATH}) first",
            sharing.join(", ")
        )));
    }
    // The entry, with the call's changes.
    let had = r.snappy.clone();
    let inside = match (p.value("inside"), &had) {
        (Some(v), _) => point_of(v, "inside")?,
        (None, Some(h)) => h.inside,
        (None, None) => return Err(bad(format!("region {region} has no snappy entry yet: give inside"))),
    };
    let mut entry = had.clone().unwrap_or(Snappy {
        inside,
        surfaces: Vec::new(),
        boxes: Vec::new(),
        expansion: None,
        first_layer: None,
    });
    entry.inside = inside;
    let roles = match p.value("surfaces") {
        Some(v) => merge_surfaces(&mut entry.surfaces, v)?,
        None => Vec::new(),
    };
    if let Some(v) = p.value("boxes") {
        merge_boxes(&mut entry.boxes, v)?;
    }
    for (k, slot) in [("expansion", &mut entry.expansion), ("firstLayer", &mut entry.first_layer)] {
        match p.value(k) {
            None => {}
            Some(Value::Null) => *slot = None,
            Some(v) => *slot = Some(v.as_f64().ok_or_else(|| bad(format!("{k} is a number, or null")))?),
        }
    }
    if let Some((name, _)) =
        roles.iter().find(|(n, _)| !entry.surfaces.iter().any(|x| x.name == *n && x.mode == Mode::Cut))
    {
        return Err(bad(format!("surfaces.{name}.role: only a surface that cuts makes a patch to give a role")));
    }
    let problems = entry.problems();
    if !problems.is_empty() {
        let list: Vec<String> =
            problems.iter().map(|(at, m)| if at.is_empty() { m.clone() } else { format!("{at}: {m}") }).collect();
        return Err(failed(format!(
            "the snappy entry of region {region} has problems, nothing is changed: {}",
            list.join("; ")
        )));
    }
    let mut geometry_note = false;
    let mut roles_inherited: Vec<(String, Role)> = Vec::new();
    let mut partnered: Vec<String> = Vec::new();
    // Each surface, read and checked.
    let mut read: Vec<Read> = Vec::new();
    for x in entry.surfaces.iter_mut() {
        if s.cancel.is_cancelled() {
            return Err(Error::Cancelled { id: SNAPPY.id.to_string() });
        }
        let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
        let rel = format!("{SURFACE_DIR}/{}", x.file);
        let bytes = case
            .file(fs, &rel)?
            .ok_or_else(|| failed(format!("there is no surface {rel} (geometry.import brings one)")))?;
        let surface = stl::read(&bytes, &limits()).map_err(|e| failed(format!("{rel}: {e}")))?;
        let report = check::analyse(&surface);
        let closed = report.open_edges == 0 && report.non_manifold_edges == 0 && report.triangles > 0;
        let holds_inside = if x.mode == Mode::Cut && closed { check::contains(&surface, entry.inside) } else { None };
        if report.region_count > report.regions.len() {
            return Err(failed(format!("{rel} has {} solids, more than can be listed", report.region_count)));
        }
        // The solids as the file has them now, each a patch of a cutting surface.
        // (only a cutting surface makes patches of them).
        x.solids = if report.region_count > 1 && x.mode == Mode::Cut { report.regions.clone() } else { Vec::new() };
        read.push(Read {
            name: x.name.clone(),
            file: rel,
            mode: x.mode,
            bounds: report.bbox,
            closed,
            patches: if x.mode == Mode::Cut { x.patches() } else { Vec::new() },
            holds_inside,
        });
    }
    let problems = entry.problems();
    if !problems.is_empty() {
        let list: Vec<String> =
            problems.iter().map(|(at, m)| if at.is_empty() { m.clone() } else { format!("{at}: {m}") }).collect();
        return Err(failed(format!(
            "the surfaces' solids give the snappy entry of region {region} problems, nothing is changed: {}",
            list.join("; ")
        )));
    }
    if !v8 && o_exists(s, "constant/geometry") {
        geometry_note = true;
    }
    // A role given to a surface goes to each of its patches; without one, when the file's solids
    // changed, a patch it makes now that the description doesn't list takes the role the
    // patches it made before shared (not a partner: a coupled patch's is its own to give).
    for x in entry.surfaces.iter().filter(|x| x.mode == Mode::Cut) {
        if roles.iter().any(|(n, _)| *n == x.name) {
            continue;
        }
        let Some(was) = had.as_ref().and_then(|h| h.surfaces.iter().find(|w| w.name == x.name && w.mode == Mode::Cut))
        else {
            continue;
        };
        let (old, new) = (was.patches(), x.patches());
        if old == new {
            continue;
        }
        let listed = |p: &str| r.patches.iter().find(|q| q.name == p);
        let before: Vec<_> = old.iter().filter_map(|p| listed(p)).collect();
        let Some(role) = before.first().and_then(|q| q.role) else { continue };
        if before.len() != old.len() || before.iter().any(|q| q.role != Some(role)) {
            continue;
        }
        let fresh: Vec<&String> = new.iter().filter(|p| listed(p).is_none()).collect();
        if before.iter().any(|q| q.partner.is_some()) {
            if !fresh.is_empty() {
                partnered.push(format!(
                    "{} (its earlier patches are {} with a partner: workflow.patch gives each new one its role and partner)",
                    fresh.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", "),
                    role.name()
                ));
            }
            continue;
        }
        for p in fresh {
            roles_inherited.push((p.clone(), role));
        }
    }
    let roles: Vec<(String, Option<Role>)> = roles
        .iter()
        .flat_map(|(n, role)| {
            entry
                .surfaces
                .iter()
                .filter(|x| x.name == *n)
                .flat_map(|x| x.patches())
                .map(|p| (p, *role))
                .collect::<Vec<_>>()
        })
        .chain(roles_inherited.iter().map(|(p, role)| (p.clone(), Some(*role))))
        .collect();
    let mut notes: Vec<String> = Vec::new();
    for n in &partnered {
        notes.push(format!("patches without a role: {n}"));
    }
    if !roles_inherited.is_empty() {
        notes.push(format!(
            "patches given the role their surface's patches had: {}",
            roles_inherited.iter().map(|(p, role)| format!("{p} {}", role.name())).collect::<Vec<_>>().join(", ")
        ));
    }
    if geometry_note {
        notes.push(format!(
            "the case has constant/geometry, where OpenFOAM-12's snappyHexMesh reads the surfaces instead of {SURFACE_DIR}: those checked here aren't the ones it meshes with"
        ));
    }
    // Patch names: each once, among the cutting surfaces' and the blocks'.
    let mut seen: HashSet<&str> = HashSet::new();
    // Not those the blocks declare without faces: left for a later step (snappyHexMesh) to fill.
    let block_patches: Vec<&str> = r
        .blocks
        .as_ref()
        .map(|g| {
            let faceless = g.faceless();
            g.patch_names().into_iter().filter(|n| !faceless.contains(n)).collect()
        })
        .unwrap_or_default();
    for n in &block_patches {
        seen.insert(n);
    }
    for x in &read {
        for n in &x.patches {
            if !seen.insert(n.as_str()) {
                let whose = if block_patches.contains(&n.as_str()) {
                    "a patch of the region's blocks"
                } else {
                    "another cutting surface's patch"
                };
                return Err(failed(format!(
                    "surface {} makes the patch {n}, which is {whose} too: rename the surface (or the solids in its file)",
                    x.name
                )));
            }
        }
    }
    // The inside point: in the blocks, and not inside a cutting surface.
    let extent = r.blocks.as_ref().map(|g| {
        let at = |a: usize, last: bool| {
            let st = &g.axes[a].stations;
            (if last { st.last() } else { st.first() }).copied().unwrap_or(0.0)
        };
        ([at(0, false), at(1, false), at(2, false)], [at(0, true), at(1, true), at(2, true)])
    });
    let pt = |q: [f64; 3]| format!("({} {} {})", blocks::number(q[0]), blocks::number(q[1]), blocks::number(q[2]));
    match &extent {
        Some((lo, hi)) => {
            if (0..3).any(|a| !(lo[a] < inside[a] && inside[a] < hi[a])) {
                return Err(failed(format!(
                    "the inside point {} is outside the region's blocks, {} to {}: snappyHexMesh would find no cell there",
                    pt(inside),
                    pt(*lo),
                    pt(*hi)
                )));
            }
            if let Some(b) = r.blocks.as_ref().and_then(|g| missing_at(g, inside)) {
                return Err(failed(format!(
                    "the inside point {} is in the missing block ({} {} {}) of the region's blocks: snappyHexMesh would find no cell there",
                    pt(inside),
                    b[0],
                    b[1],
                    b[2]
                )));
            }
        }
        None => notes.push(format!(
            "region {region} has no blocks in {PATH}: whether the inside point and the surfaces are in its background mesh isn't checked (mesh.block or mesh.box describes them)"
        )),
    }
    if r.blocks.as_ref().is_some_and(|g| on_station(g, inside)) {
        notes.push(format!(
            "the inside point {} lies on a face between the blocks: snappyHexMesh may not tell which cell holds it, so give one inside a cell",
            pt(inside)
        ));
    }
    if let Some(x) = read.iter().find(|x| x.holds_inside == Some(true)) {
        return Err(failed(format!(
            "the inside point {} is inside surface {} ({}): snappyHexMesh would keep its inside and cut away the rest; give a point in the cells the region keeps",
            pt(inside),
            x.name,
            x.file
        )));
    }
    for x in &read {
        if x.mode == Mode::Cut && !x.closed {
            notes.push(format!(
                "surface {} ({}) isn't one closed surface (open edges, or edges of more than two triangles, as where its solids share faces): whether the inside point is inside it isn't checked, and the mesh leaks through any holes unless the blocks' sides close them (geometry.check lists them)",
                x.name, x.file
            ));
        } else if x.mode == Mode::Cut && x.holds_inside.is_none() {
            notes.push(format!(
                "whether the inside point is inside surface {} couldn't be told: it may lie on it",
                x.name
            ));
        }
        if let (Some((lo, hi)), Some(b), Mode::Cut) = (&extent, &x.bounds, x.mode) {
            let bmin = [b.min.x, b.min.y, b.min.z];
            let bmax = [b.max.x, b.max.y, b.max.z];
            if (0..3).any(|a| bmax[a] < lo[a] || bmin[a] > hi[a]) {
                notes.push(format!("surface {} is outside the region's blocks: it cuts nothing", x.name));
            } else if (0..3).any(|a| bmin[a] < lo[a] || bmax[a] > hi[a]) {
                notes.push(format!("surface {} reaches past the region's blocks: cut at their sides", x.name));
            }
        }
    }
    // The roles, then the patch types the dictionary is written with.
    let mut with = r.clone();
    for (name, role) in &roles {
        match with.patches.iter_mut().find(|q| q.name == *name) {
            Some(q) => q.role = *role,
            None => {
                with.patches.push(umcf_model::description::PatchDesc { name: name.clone(), role: *role, partner: None })
            }
        }
    }
    let type_of = blocks::type_lookup(&with);
    let location = path.rsplit_once('/').map_or("system", |(dir, _)| dir).to_string();
    let text = entry.dict(region, &location, &type_of);
    let o = s.opened_mut()?;
    let limit = o.case.options().dict_bytes;
    if text.len() as u64 > limit {
        return Err(failed(format!(
            "{path} would be {} bytes, more than the case reads as a dictionary ({limit})",
            text.len()
        )));
    }
    let (write, hand) = match o.case.dict(&path) {
        Some(doc) => {
            let old = doc.to_string();
            let hand = !blocks::untouched(&old);
            if old == text {
                (false, false)
            } else if hand && !replace {
                return Err(failed(format!(
                    "{path} was edited by hand or not written by umcfGui: pass \"replace\": true to write it from the snappy entry (case.save keeps the one there in the backup)"
                )));
            } else {
                (true, hand)
            }
        }
        None => (true, false),
    };
    if let Some(steps) = &new_mesh {
        o.case.describe_root_mesh(region, steps)?;
        notes.push(format!(
            "region {region} is meshed by the steps ( {} ) in {PATH} now",
            steps
                .iter()
                .map(|st| match st {
                    RootStep::BlockMesh(d) => format!("blockMesh {d}"),
                    RootStep::CreatePatch => "createPatch".to_string(),
                    RootStep::SnappyHexMesh(d) => format!("snappyHexMesh {d}"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    for (name, role) in &roles {
        let edit = PatchEdit { role: Some(*role), ..PatchEdit::default() };
        o.case.describe_patch(region, name, &edit)?;
    }
    let described = had.as_ref() != Some(&entry);
    if described {
        o.case.describe_snappy(region, &entry)?;
    }
    if write {
        o.case.put_dict(o.fs.as_ref(), &path, &text).map_err(|e| match e {
            umcf_model::Error::IsLink(_) => Error::Failed(format!("{path} is a symbolic link: it isn't written over")),
            umcf_model::Error::IsDict(_) => Error::Failed(format!(
                "{path} is the same file as another of the case's dictionaries: it isn't written over"
            )),
            umcf_model::Error::NotLoaded { .. } => Error::Failed(format!(
                "{path} is there but doesn't load as a dictionary (or is a directory): mend or remove it first"
            )),
            e => e.into(),
        })?;
    }
    // After the region's dictionary is put: one of the case's own that stays is read as it is.
    let features_written = sync_features(o, replace, &mut notes)?;
    // OpenFOAM-8's snappyHexMesh reads system/fvSchemes and system/fvSolution, which a case of
    // regions has only in theirs: given ones it reads, never over the case's own.
    if v8 {
        for (file, body) in V8_STUBS {
            let at = format!("system/{file}");
            if o.case.dict(&at).is_none() && o.case.exists(&at) != Some(true) {
                let text = umcf_model::blocks::stamped(
                    "What OpenFOAM-8's snappyHexMesh reads in system/ of a case of regions (docs/user-guide/workflow.md#snappy).",
                    &format!(
                        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      {file};\n}}\n{body}"
                    ),
                );
                o.case.put_dict(o.fs.as_ref(), &at, &text)?;
                notes.push(format!("{at} written, which OpenFOAM-8's snappyHexMesh reads"));
            }
        }
    }
    let described_names: HashSet<&str> = with.patches.iter().map(|q| q.name.as_str()).collect();
    let roleless: Vec<&str> = read
        .iter()
        .flat_map(|x| x.patches.iter())
        .map(String::as_str)
        .filter(|n| !described_names.contains(n))
        .collect();
    if !roleless.is_empty() {
        notes.push(format!(
            "not patches of region {region} in {PATH}, so without a role and of type patch: {} (role gives one)",
            blocks::list_some(&roleless)
        ));
    }
    let mut message = match (described, write) {
        (_, true) => format!(
            "{path}: written from the snappy entry of region {region} ({} surfaces, {} boxes)",
            entry.surfaces.len(),
            entry.boxes.len()
        ),
        (true, false) => {
            format!("{PATH}: the snappy entry of region {region} changed; {path} is as it makes it already")
        }
        (false, false) => format!("{path}: as the snappy entry of region {region} makes it already, nothing to write"),
    };
    if described && write {
        message.push_str(&format!("; the snappy entry in {PATH}"));
    }
    if hand {
        message.push_str(" (over one edited by hand or not written by umcfGui: case.save keeps it in the backup; edit.undo takes it back)");
    }
    for n in &notes {
        message.push_str(&format!("; {n}"));
    }
    let v = |x: umcf_geom::Vec3| json!([x.x, x.y, x.z]);
    let surfaces: Vec<Value> = read
        .iter()
        .map(|x| {
            json!({
                "name": x.name,
                "file": x.file,
                "mode": x.mode.name(),
                "patches": x.patches,
                "closed": x.closed,
                "bounds": x.bounds.map(|b| json!({ "min": v(b.min), "max": v(b.max) })),
            })
        })
        .collect();
    Ok(Outcome::new(
        message,
        json!({
            "region": region,
            "file": path,
            "written": write,
            "replaced": hand,
            "described": described,
            "meshChanged": new_mesh.is_some(),
            "featuresWritten": features_written,
            "snappy": snappy_json(&entry),
            "surfaces": surfaces,
            "notes": notes,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use umcf_model::blocks::{Axis, BlockGrid, Grading, Span};

    #[test]
    fn a_point_in_a_missing_block_is_found() {
        let axis = |st: Vec<f64>| Axis {
            spans: (1..st.len()).map(|_| Span { cells: 2, grading: Grading::Ratio(1.0) }).collect(),
            stations: st,
        };
        let g = BlockGrid {
            axes: [axis(vec![0.0, 10.0, 20.0]), axis(vec![0.0, 10.0, 20.0]), axis(vec![0.0, 5.0])],
            missing: vec![[1, 0, 0]],
            ..Default::default()
        };
        assert_eq!(missing_at(&g, [15.0, 5.0, 1.0]), Some([1, 0, 0]));
        assert_eq!(missing_at(&g, [5.0, 5.0, 1.0]), None);
        assert_eq!(missing_at(&g, [5.0, 5.0, 9.0]), None, "outside the grid");
        assert_eq!(missing_at(&g, [10.0, 5.0, 1.0]), None, "between a block and a missing one");
        let mut both = g.clone();
        both.missing.push([1, 1, 0]);
        assert_eq!(missing_at(&both, [15.0, 10.0, 1.0]), Some([1, 0, 0]), "between two missing blocks");
        assert!(on_station(&g, [10.0, 5.0, 1.0]));
        assert!(!on_station(&g, [15.0, 5.0, 1.0]));
    }
}
