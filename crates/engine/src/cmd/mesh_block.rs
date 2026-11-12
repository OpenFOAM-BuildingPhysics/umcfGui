//! `mesh.block`: a region's grid of blocks in the workflow description, and the
//! `blockMeshDict` written from it.

use serde_json::{Map, Value, json};
use umcf_model::blocks::{self, Axis, BlockFace, BlockGrid, Grading, Side, Span};
use umcf_model::description::PATH;

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const BLOCK: CommandSpec = CommandSpec {
    id: "mesh.block",
    label: "Block mesh",
    menu: "Mesh/Blocks",
    shortcut: None,
    params: "region: a region of system/umcfWorkflow whose mesh is `blocks` (blockMesh -region <region> from system/<region>/blockMeshDict) or built in constant/polyMesh by steps starting with blockMesh (the dictionary that step names, system/blockMeshDict by default); a region without a mesh (as case.new makes them) is given `mesh blocks;`. The grid, each part optional (those given replace the region's; a region without blocks needs x, y and z): x, y, z: the stations along the axis with (cells grading) between each two, [0, [7, 0.0074], 200, [10, [[0.5, 0.5, 13.9], [0.5, 0.5, 0.072]]], 590] (a grading is the ratio of the last cell to the first, or blockMesh's sections [fraction, cells-fraction, expansion]); missing: the blocks left out (the buildings), [[1, 1, 0], …], counted from 0 along x, y, z; patches: each side's patch, {\"west\": \"inlet\", \"east\": \"outlet\", \"south\": \"side\", \"north\": \"side\", \"ground\": \"ground\", \"top\": \"top\", \"missing\": \"buildings\"} (west/east: x min/max, south/north: y min/max, ground/top: z min/max, missing: the faces around the missing blocks; defaultFaces leaves faces to blockMesh's own patch, type empty); faces: single block faces in another patch than their side's, {\"air_to_street\": [[2, 1, 0, \"ground\"], …]} (block i, j, k and its side west … top); noFaces: patches declared without faces, [\"street\"] ([] takes them away); types: patch types given explicitly, {\"side1\": \"symmetryPlane\"}. The maps patches, faces and types merge into the region's: a key given replaces its entry (a new one goes after the others, in the order of its name), null takes it away, the others stay. Each patch's blockMesh type is its types entry, else the one its role in the region's patches gives (wall for ground, coupled, interior and bottom in a fluid or vegetation region, patch otherwise and in a solid), else patch (workflow.import fills types for every type the role wouldn't give). The outcome lists every patch whose type the written dictionary changes from the one it replaces (typesChanged), and the patches it drops or adds (patchesRemoved, patchesAdded). The dictionary there is read whole for its patches: its boundary, the older patches ( type name ( faces ) … ) syntax, its defaultPatch, and blockMesh's own defaultFaces when it leaves faces to it. A wall or a constraint type there (symmetryPlane, symmetry, empty, wedge, cyclic…, processor…) is changed only when this call's types gives the patch its new type; such a patch the blocks don't have is dropped only when dropPatches ([\"name\", …]) names it (a patch renamed in the description reads as one dropped and one added, and the refusal says so); a patch's entries besides type and faces (neighbourPatch, inGroups, …), which the blocks don't hold, are lost only when dropEntries ([\"name\", …]) names the patch (entriesLost lists them); a dictionary whose patches can't all be read (an #include, a macro, a list of another shape, a name twice, faces left to its default patch that can't be told) is written over only with unreadPatches: true; and what else it holds that the blocks don't (cell zones of blocks, curved edges, projected faces and geometry, mergePatchPairs, a scale other than 1, any other key at its top, a key given twice, expressions in its vertices or blocks) is lost only with dropSettings: true (settingsLost lists it). The default patch, when faces are left to it, is typed like the others: types {\"defaultFaces\": \"patch\"}; the import fills it from the dictionary's defaultPatch, and reads one named otherwise as a patch of that name. Otherwise the call is refused, nothing changed. A cyclic…, nonConformal…, mapped… or processor… type in types is refused: it needs entries the blocks don't hold (give it after blockMesh, with changeDictionary or createPatch). At most 20000 blocks, 10000 stations along an axis (9999 spans), 1000 sections in a grading, 10000 names in each of faces, noFaces, types, dropPatches and dropEntries, 40000 missing blocks, 120000 single faces in all, and a dictionary that reads back (48 MB and 3000000 tokens, as the dictionary parser counts them, and no more than the case reads as a dictionary: 4 MB unless opened with more). With only region, the dictionary is written again from the blocks there. Refused while the region's blocks entry in the description doesn't read whole (an #include, a $macro or a value it can't read, also in its patches, faces or types; remove: true still takes it out), and when another region's mesh reads the same dictionary. Only the keys of blocks that change are written, each on one line (comments inside it are not kept). remove: true takes the blocks out of the description, with the box they were made from (the dictionary stays, the case's own). A call that changes the grid (x, y, z, missing or faces) of a region made from a box (mesh.box) takes the box out of the description, and says so. A grid with problems (stations not increasing, no cells, a missing block outside the grid or listed twice, a face without a patch, more than 20000 blocks) is refused, nothing changed. The blocks go into the region's blocks entry of system/umcfWorkflow (only the keys that change; comments stay), and the blockMeshDict is written from them, generated (its header says so, with a checksum). One that umcfGui didn't write, or that was edited since, is refused unless replace: true (case.save keeps the one there in the backup). case.save writes both; edit.undo takes both back. The case check (workflow.blocks) says when the dictionary isn't what the blocks make.",
    enabled: needs_case,
    run: block,
    journal: true,
    undoable: true,
    reads_only: false,
};

const KEYS: [&str; 8] = ["x", "y", "z", "missing", "patches", "faces", "noFaces", "types"];

fn bad(m: impl std::fmt::Display) -> Error {
    Error::Params(format!("{}: {m}", BLOCK.id))
}

fn grading_of(v: &Value, at: &str) -> Result<Grading, Error> {
    match v {
        Value::Number(n) => n.as_f64().map(Grading::Ratio).ok_or_else(|| bad(format!("{at}: the grading is a number"))),
        Value::Array(sections) if sections.len() > blocks::MAX_SECTIONS => {
            Err(bad(format!("{at}: a grading has {} sections at most", blocks::MAX_SECTIONS)))
        }
        Value::Array(sections) => {
            let mut out = Vec::with_capacity(sections.len());
            for s in sections {
                let three: Option<Vec<f64>> =
                    s.as_array().filter(|a| a.len() == 3).map(|a| a.iter().filter_map(Value::as_f64).collect());
                match three.as_deref() {
                    Some([a, b, c]) => out.push([*a, *b, *c]),
                    _ => return Err(bad(format!("{at}: a section is [fraction, cells-fraction, expansion]"))),
                }
            }
            Ok(Grading::Sections(out))
        }
        _ => Err(bad(format!("{at}: the grading is a number or sections [[fraction, cells-fraction, expansion], …]"))),
    }
}

fn axis_of(v: &Value, at: &str) -> Result<Axis, Error> {
    let shape = || bad(format!("{at} is [station, [cells, grading], station, …]"));
    let items = v.as_array().ok_or_else(shape)?;
    if items.len() > 2 * blocks::MAX_STATIONS - 1 {
        return Err(bad(format!("{at} has {} stations at most", blocks::MAX_STATIONS)));
    }
    let mut a = Axis::default();
    for (i, item) in items.iter().enumerate() {
        if i % 2 == 0 {
            a.stations.push(item.as_f64().ok_or_else(shape)?);
        } else {
            let pair = item.as_array().filter(|p| p.len() == 2).ok_or_else(shape)?;
            let (Some(c), Some(g)) = (pair.first(), pair.get(1)) else { return Err(shape()) };
            let cells = c
                .as_u64()
                .and_then(|c| u32::try_from(c).ok())
                .ok_or_else(|| bad(format!("{at}: the cells are a whole number")))?;
            a.spans.push(Span { cells, grading: grading_of(g, at)? });
        }
    }
    if items.len() % 2 == 0 {
        return Err(shape());
    }
    Ok(a)
}

fn index_of(v: Option<&Value>) -> Option<usize> {
    v?.as_u64().and_then(|x| usize::try_from(x).ok())
}

fn names_of(v: &Value, at: &str) -> Result<Vec<String>, Error> {
    let a = v.as_array().ok_or_else(|| bad(format!("{at} is a list of names")))?;
    // Counted before anything is copied.
    if a.len() > blocks::MAX_NAMES {
        return Err(bad(format!("{at} has {} names, {} at most", a.len(), blocks::MAX_NAMES)));
    }
    a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| bad(format!("{at} is a list of names")))).collect()
}

/// `grid` with the parts `p` gives put over it.
fn apply(grid: &mut BlockGrid, p: &Params<'_>) -> Result<(), Error> {
    for (i, k) in ["x", "y", "z"].into_iter().enumerate() {
        if let (Some(v), Some(slot)) = (p.value(k), grid.axes.get_mut(i)) {
            *slot = axis_of(v, k)?;
        }
    }
    if let Some(v) = p.value("missing") {
        let a = v.as_array().ok_or_else(|| bad("missing is a list of blocks [[i, j, k], …]"))?;
        if a.len() > 2 * blocks::MAX_BLOCKS {
            return Err(bad(format!("missing lists {} blocks, {} at most", a.len(), 2 * blocks::MAX_BLOCKS)));
        }
        grid.missing = a
            .iter()
            .map(|b| {
                let b = b.as_array().filter(|b| b.len() == 3);
                match b.map(|b| [index_of(b.first()), index_of(b.get(1)), index_of(b.get(2))]) {
                    Some([Some(i), Some(j), Some(k)]) => Ok([i, j, k]),
                    _ => Err(bad("missing is a list of blocks [[i, j, k], …], whole numbers from 0")),
                }
            })
            .collect::<Result<_, _>>()?;
    }
    // The maps merge: a key given replaces its entry (or is added, after the others), `null`
    // takes it away, the rest stay.
    // In one pass, whatever the number of keys.
    fn merge<T>(list: &mut Vec<(String, T)>, changes: Vec<(String, Option<T>)>) {
        let mut slots: Vec<Option<(String, T)>> = std::mem::take(list).into_iter().map(Some).collect();
        let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (i, s) in slots.iter().enumerate() {
            if let Some((k, _)) = s {
                at.entry(k.clone()).or_insert(i);
            }
        }
        for (key, value) in changes {
            match (at.get(&key).copied(), value) {
                (Some(i), Some(v)) => {
                    if let Some(slot) = slots.get_mut(i) {
                        *slot = Some((key, v));
                    }
                }
                (None, Some(v)) => {
                    at.insert(key.clone(), slots.len());
                    slots.push(Some((key, v)));
                }
                (Some(i), None) => {
                    if let Some(slot) = slots.get_mut(i) {
                        *slot = None;
                    }
                    at.remove(&key);
                }
                (None, None) => {}
            }
        }
        *list = slots.into_iter().flatten().collect();
    }
    // A map of more keys than a grid may name is refused before it is read.
    let map = |key: &str, shape: &str| -> Result<Option<&serde_json::Map<String, Value>>, Error> {
        match p.value(key) {
            None => Ok(None),
            Some(v) => {
                let m = v.as_object().ok_or_else(|| bad(shape))?;
                if m.len() > blocks::MAX_NAMES {
                    return Err(bad(format!("{key} has {} entries, {} at most", m.len(), blocks::MAX_NAMES)));
                }
                Ok(Some(m))
            }
        }
    };
    if let Some(v) = p.value("patches") {
        let m = v.as_object().ok_or_else(|| bad("patches is {\"west\": \"inlet\", …} (null takes a side's away)"))?;
        for (k, n) in m {
            let side = Side::parse(k).ok_or_else(|| {
                bad(format!("patches/{k} is not a side: west, east, south, north, ground, top or missing"))
            })?;
            let n = match n {
                Value::Null => None,
                Value::String(n) => Some(n.clone()),
                _ => return Err(bad(format!("patches/{k}: the patch's name, or null"))),
            };
            let at = grid.sides.iter().position(|(s, _)| *s == side);
            match (at, n) {
                (Some(i), Some(n)) => {
                    if let Some(slot) = grid.sides.get_mut(i) {
                        slot.1 = n;
                    }
                }
                (None, Some(n)) => grid.sides.push((side, n)),
                (Some(i), None) => {
                    grid.sides.remove(i);
                }
                (None, None) => {}
            }
        }
        // West to missing, as the description lists them.
        grid.sides.sort_by_key(|(s, _)| *s);
    }
    if let Some(m) = map("faces", "faces is {\"patch\": [[i, j, k, \"side\"], …], …} (null takes a patch's away)")?
    {
        let mut changes = Vec::with_capacity(m.len());
        let mut single = 0usize;
        for (name, list) in m {
            if list.is_null() {
                changes.push((name.clone(), None));
                continue;
            }
            let shape = || bad(format!("faces/{name} is a list of faces [[i, j, k, \"west\"], …], or null"));
            let list = list.as_array().ok_or_else(shape)?;
            single = single.saturating_add(list.len());
            if single > 6 * blocks::MAX_BLOCKS {
                return Err(bad(format!("faces lists more than {} faces", 6 * blocks::MAX_BLOCKS)));
            }
            let mut faces = Vec::with_capacity(list.len());
            for f in list {
                let f = f.as_array().filter(|f| f.len() == 4).ok_or_else(shape)?;
                let side = f.get(3).and_then(Value::as_str).and_then(Side::parse).filter(|s| *s != Side::Missing);
                match ([index_of(f.first()), index_of(f.get(1)), index_of(f.get(2))], side) {
                    ([Some(i), Some(j), Some(k)], Some(side)) => faces.push(BlockFace { block: [i, j, k], side }),
                    _ => return Err(shape()),
                }
            }
            changes.push((name.clone(), Some(faces)));
        }
        merge(&mut grid.faces, changes);
    }
    if let Some(v) = p.value("noFaces") {
        grid.no_faces = names_of(v, "noFaces")?;
        if grid.no_faces.len() > blocks::MAX_NAMES {
            return Err(bad(format!("noFaces names {} patches, {} at most", grid.no_faces.len(), blocks::MAX_NAMES)));
        }
    }
    if let Some(m) = map("types", "types is {\"patch\": \"type\", …} (null takes a type away)")? {
        let mut changes = Vec::with_capacity(m.len());
        for (n, t) in m {
            match t {
                Value::Null => changes.push((n.clone(), None)),
                Value::String(t) => changes.push((n.clone(), Some(t.clone()))),
                _ => return Err(bad(format!("types/{n}: the patch's blockMesh type, or null"))),
            }
        }
        merge(&mut grid.types, changes);
    }
    Ok(())
}

/// The first 20 of a list, and how many more.
fn some(items: &[String]) -> String {
    let v: Vec<&str> = items.iter().map(String::as_str).collect();
    blocks::list_some(&v)
}

/// A number as JSON, written as the description writes it: whole numbers as integers, the rest
/// with the digits that read back as the same value.
fn number(x: f64) -> Value {
    serde_json::from_str(&blocks::number(x)).unwrap_or(Value::Null)
}

/// A grid as JSON, as `mesh.block` takes it.
pub(crate) fn grid_json(g: &BlockGrid) -> Value {
    let axis = |a: &Axis| -> Value {
        let mut out = Vec::new();
        for (i, x) in a.stations.iter().enumerate() {
            out.push(number(*x));
            if let Some(s) = a.spans.get(i).filter(|_| i + 1 < a.stations.len()) {
                let grading = match &s.grading {
                    Grading::Ratio(r) => number(*r),
                    Grading::Sections(v) => {
                        Value::Array(v.iter().map(|t| Value::Array(t.iter().map(|x| number(*x)).collect())).collect())
                    }
                };
                out.push(json!([s.cells, grading]));
            }
        }
        Value::Array(out)
    };
    let patches: Map<String, Value> = g.sides.iter().map(|(s, n)| (s.name().to_string(), json!(n))).collect();
    let faces: Map<String, Value> = g
        .faces
        .iter()
        .map(|(n, fs)| {
            let list: Vec<Value> =
                fs.iter().map(|f| json!([f.block[0], f.block[1], f.block[2], f.side.name()])).collect();
            (n.clone(), Value::Array(list))
        })
        .collect();
    json!({
        "x": axis(&g.axes[0]),
        "y": axis(&g.axes[1]),
        "z": axis(&g.axes[2]),
        "missing": g.missing.iter().map(|b| json!(b)).collect::<Vec<_>>(),
        "patches": patches,
        "faces": faces,
        "noFaces": g.no_faces,
        "types": g.types.iter().map(|(n, t)| (n.clone(), json!(t))).collect::<Map<String, Value>>(),
    })
}

/// `mesh.block`: a region's blocks, and its `blockMeshDict` written from them.
fn block(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    set(s, p, false)
}

/// The blocks `p` gives, and the `blockMeshDict` written from them; `from_box`: made by
/// `mesh.box` from the region's box, which a change of the grid otherwise takes out.
pub(super) fn set(s: &mut Session, p: &Value, from_box: bool) -> Result<Outcome, Error> {
    let allowed: Vec<&str> =
        ["region", "remove", "replace", "dropPatches", "dropEntries", "unreadPatches", "dropSettings"]
            .into_iter()
            .chain(KEYS)
            .collect();
    let p = Params::new(BLOCK.id, p, &allowed)?;
    let region = p.required_str("region")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let replace = p.bool("replace")?.unwrap_or(false);
    let given: Vec<&str> = KEYS.into_iter().filter(|k| p.value(k).is_some()).collect();
    let o = s.opened_mut()?;
    let Some((d, read_problems)) = o.case.description() else {
        return Err(Error::Failed(format!(
            "the case has no {PATH} (or it doesn't load): workflow.import writes one from the case first"
        )));
    };
    let Some(r) = d.region(region) else {
        return Err(Error::Failed(format!("region {region} is not in {PATH}: workflow.region adds it")));
    };
    // A blocks entry that doesn't read: what the call doesn't give would be taken for absent.
    let here = format!("regions/{region}/blocks");
    if !remove && let Some(pr) = read_problems.iter().find(|x| x.at == here || x.at.starts_with(&format!("{here}/"))) {
        return Err(Error::Failed(format!(
            "{}: the blocks of region {region} in {PATH} don't read ({}: {}): mend them by hand first (remove: true takes them out)",
            BLOCK.id, pr.at, pr.message
        )));
    }
    if remove {
        if !given.is_empty() || replace {
            return Err(bad("remove: true takes the blocks out: give nothing else but the region"));
        }
        o.case.undescribe_blocks(region)?;
        let boxed = o.case.undescribe_box(region)?;
        let message = format!(
            "{PATH}: the blocks of region {region} taken out{} (its blockMeshDict stays as it is, the case's own)",
            if boxed { ", with the box they were made from" } else { "" }
        );
        return Ok(Outcome::new(message, json!({ "region": region, "file": PATH, "blocks": Value::Null })));
    }
    let had = r.blocks.clone();
    let mut grid = had.clone().unwrap_or_default();
    apply(&mut grid, &p)?;
    if had.is_none() {
        let lacking: Vec<&str> = ["x", "y", "z"].into_iter().filter(|k| !given.contains(k)).collect();
        if !lacking.is_empty() {
            return Err(bad(format!(
                "region {region} has no blocks yet: give x, y and z ({} missing)",
                lacking.join(", ")
            )));
        }
    }
    let problems = grid.problems();
    if !problems.is_empty() {
        let list: Vec<String> =
            problems.iter().map(|(at, m)| if at.is_empty() { m.clone() } else { format!("{at}: {m}") }).collect();
        return Err(Error::Failed(format!(
            "{}: the blocks of region {region} have problems, nothing is changed: {}",
            BLOCK.id,
            list.join("; ")
        )));
    }
    let mut with = r.clone();
    with.blocks = Some(grid.clone());
    // A region without a mesh (as case.new makes them) is meshed from its own dictionary; one
    // whose mesh entry doesn't read is left to be mended.
    let meshless = r.mesh.is_none();
    if meshless {
        let at = format!("regions/{region}/mesh");
        if let Some(pr) = read_problems.iter().find(|x| x.at == at) {
            return Err(Error::Failed(format!(
                "{}: the mesh of region {region} in {PATH} doesn't read ({}): mend it by hand first",
                BLOCK.id, pr.message
            )));
        }
        with.mesh = Some(umcf_model::workflow::MeshBuild::Block);
    }
    let (path, text) = blocks::generate(&with).map_err(|why| {
        Error::Failed(format!("{}: {why}: give the region mesh blocks; or ( blockMesh … ) in {PATH} first", BLOCK.id))
    })?;
    // A dictionary another region's mesh reads too: written from one region's blocks, it would
    // change the other's mesh unsaid.
    let sharing: Vec<&str> = d
        .regions
        .iter()
        .filter(|x| x.name != region)
        .filter(|x| x.mesh.as_ref().and_then(|m| blocks::dict_path(&x.name, m)).as_deref() == Some(path.as_str()))
        .map(|x| x.name.as_str())
        .collect();
    if !sharing.is_empty() {
        return Err(Error::Failed(format!(
            "{}: {path} is read by the mesh of region {} too: give region {region} a dictionary of its own (its blockMesh step in {PATH}) first",
            BLOCK.id,
            sharing.join(", ")
        )));
    }
    // No larger than the case reads a dictionary (read back when the case opens again).
    let limit = o.case.options().dict_bytes;
    if text.len() as u64 > limit {
        return Err(Error::Failed(format!(
            "{}: {path} would be {} bytes, more than the case reads as a dictionary ({limit}): fewer blocks or simpler gradings",
            BLOCK.id,
            text.len()
        )));
    }
    // The dictionary there: written over only when umcfGui wrote it and nobody changed it since.
    let old = o.case.dict(&path);
    let (write, hand) = match old {
        Some(doc) => {
            let old = doc.to_string();
            let hand = !blocks::untouched(&old);
            if old == text {
                (false, false)
            } else if hand && !replace {
                return Err(Error::Failed(format!(
                    "{path} was edited by hand or not written by umcfGui: pass \"replace\": true to write it from the blocks (case.save keeps the one there in the backup), or workflow.import to read its blocks into {PATH}"
                )));
            } else {
                (true, hand)
            }
        }
        None => (true, false),
    };
    // Nothing of the dictionary there is lost unsaid: a type changes from wall or a constraint
    // type only when this call's types gives the patch its new type; a patch the blocks don't
    // have is dropped, if of such a type, only when dropPatches names it; a patch's other
    // entries (neighbourPatch, inGroups, …) only when dropEntries names it; and a dictionary
    // whose patches can't all be read is written over only when unreadPatches says so.
    let given: std::collections::HashSet<&str> = p
        .value("types")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter(|(_, t)| t.is_string()).map(|(k, _)| k.as_str()).collect())
        .unwrap_or_default();
    let list = |key: &str| -> Result<Vec<String>, Error> {
        match p.value(key) {
            None => Ok(Vec::new()),
            Some(v) => names_of(v, key),
        }
    };
    let drop: std::collections::HashSet<String> = list("dropPatches")?.into_iter().collect();
    let drop_entries: std::collections::HashSet<String> = list("dropEntries")?.into_iter().collect();
    let unread_ok = p.bool("unreadPatches")?.unwrap_or(false);
    let mut unread = None;
    let mut read_notes: Vec<String> = Vec::new();
    let old_patches = match old.filter(|_| write) {
        None => Vec::new(),
        Some(doc) => match blocks::dict_patches(doc) {
            Ok(v) => {
                read_notes = v.notes;
                v.patches
            }
            Err(why) if unread_ok => {
                unread = Some(why);
                Vec::new()
            }
            Err(why) => {
                return Err(Error::Failed(format!(
                    "{}: the patches of {path} can't all be read ({why}), so what writing over it loses can't be said: pass \"unreadPatches\": true to write over it all the same (case.save keeps it in the backup)",
                    BLOCK.id
                )));
            }
        },
    };
    let new_patches = umcf_foam_dict::Document::parse(&text)
        .map_err(|e| Error::Failed(format!("{}: the dictionary written doesn't read back: {e}", BLOCK.id)))
        .and_then(|d| {
            blocks::dict_patches(&d)
                .map(|v| v.patches)
                .map_err(|e| Error::Failed(format!("{}: the dictionary written: {e}", BLOCK.id)))
        })?;
    // What else the dictionary there holds that the blocks don't: lost only when said.
    let settings: Vec<String> = old.filter(|_| write).map(blocks::dict_settings).unwrap_or_default();
    if !settings.is_empty() && !p.bool("dropSettings")?.unwrap_or(false) {
        return Err(Error::Failed(format!(
            "{}: {path} holds what the blocks don't, which writing over it would lose: {}. Pass \"dropSettings\": true to lose them (case.save keeps the dictionary in the backup)",
            BLOCK.id,
            settings.join("; ")
        )));
    }
    let new_set: std::collections::HashSet<&str> = new_patches.iter().map(|q| q.name.as_str()).collect();
    let old_set: std::collections::HashSet<&str> = old_patches.iter().map(|q| q.name.as_str()).collect();
    let removed: Vec<&blocks::DictPatch> = old_patches.iter().filter(|q| !new_set.contains(q.name.as_str())).collect();
    let added: Vec<&str> = if old.is_some() && write && unread.is_none() {
        new_patches.iter().map(|q| q.name.as_str()).filter(|n| !old_set.contains(n)).collect()
    } else {
        Vec::new()
    };
    // A patch renamed in the description reads as one dropped and one added.
    let renamed = if added.is_empty() {
        String::new()
    } else {
        format!(
            " (the blocks name {} instead, which {path} doesn't have: if one is a patch renamed in the description, name its old name in dropPatches)",
            blocks::list_some(&added)
        )
    };
    let changes = blocks::type_changes(&old_patches, &new_patches);
    let forced: Vec<&(String, String, String)> =
        changes.iter().filter(|(n, was, _)| blocks::is_protected_type(was) && !given.contains(n.as_str())).collect();
    if let Some((n, was, _)) = forced.first() {
        let list: Vec<String> = forced.iter().map(|(n, was, now)| format!("{n} ({was} → {now})")).collect();
        return Err(Error::Failed(format!(
            "{}: the blocks would change wall or constraint types of {path}, which the fields' conditions need: {}. Name the patch in types to keep or change its type: {{\"types\": {{\"{n}\": \"{was}\"}}}}",
            BLOCK.id,
            some(&list)
        )));
    }
    let kept: Vec<String> = removed
        .iter()
        .filter(|q| blocks::is_protected_type(&q.kind) && !drop.contains(&q.name))
        .map(|q| format!("{} ({})", q.name, q.kind))
        .collect();
    if !kept.is_empty() {
        return Err(Error::Failed(format!(
            "{}: {path} has wall or constraint patches the blocks don't have: {}. Give them to the blocks (patches, faces or noFaces), or name them in dropPatches to drop them{renamed}",
            BLOCK.id,
            some(&kept)
        )));
    }
    let lost: Vec<&blocks::DictPatch> = old_patches.iter().filter(|q| !q.extra.is_empty()).collect();
    let unasked: Vec<String> = lost
        .iter()
        .filter(|q| !drop_entries.contains(&q.name))
        .map(|q| format!("{} ({})", q.name, q.extra.join(", ")))
        .collect();
    if !unasked.is_empty() {
        return Err(Error::Failed(format!(
            "{}: patches of {path} have entries the blocks don't hold, which writing over it would lose: {}. Name the patches in dropEntries to lose them (and give the entries again in a step after blockMesh: changeDictionary, createPatch)",
            BLOCK.id,
            some(&unasked)
        )));
    }
    let described = had.as_ref() != Some(&grid);
    if meshless {
        o.case.describe_block_mesh(region)?;
    }
    if described {
        o.case.describe_blocks(region, &grid)?;
    }
    // Blocks set by hand aren't the box's any more: its entry would make others.
    let reshaped =
        had.as_ref().is_none_or(|h| h.axes != grid.axes || h.missing != grid.missing || h.faces != grid.faces);
    // Whether the box entry reads or not: either way it no longer makes these blocks.
    let unboxed = !from_box && reshaped && o.case.undescribe_box(region)?;
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
    // Patches the region doesn't describe: no role to type them by.
    let described_names: std::collections::HashSet<&str> = with.patches.iter().map(|q| q.name.as_str()).collect();
    let undescribed: Vec<&str> = grid.patch_names().into_iter().filter(|n| !described_names.contains(n)).collect();
    let mut notes: Vec<String> = Vec::new();
    if meshless {
        notes.push(format!(
            "region {region} had no mesh: given mesh blocks; in {PATH} (blockMesh -region {region} from {path})"
        ));
    }
    if unboxed {
        notes.push(format!(
            "the box of region {region} taken out of {PATH}: the blocks are given as they are now (mesh.box makes them from a box again)"
        ));
    }
    for n in &read_notes {
        notes.push(format!("the dictionary it replaces: {n}"));
    }
    if write && !settings.is_empty() {
        notes.push(format!("what the dictionary it replaces held besides its patches, lost: {}", settings.join("; ")));
    }
    if let Some(why) = &unread {
        notes.push(format!(
            "the patches of the dictionary it replaces couldn't all be read ({why}): written over as unreadPatches said"
        ));
    }
    if write && !lost.is_empty() {
        let list: Vec<String> = lost.iter().map(|q| format!("{} ({})", q.name, q.extra.join(", "))).collect();
        notes.push(format!("entries of the dictionary it replaces that it loses: {}", list.join("; ")));
    }
    if write && !removed.is_empty() {
        let list: Vec<String> = removed.iter().map(|q| format!("{} ({})", q.name, q.kind)).collect();
        notes.push(format!("patches of the dictionary it replaces that it drops: {}", some(&list)));
    }
    if write && !added.is_empty() {
        notes.push(format!("patches it adds to the dictionary it replaces: {}", blocks::list_some(&added)));
    }
    if write && !changes.is_empty() {
        let list: Vec<String> = changes.iter().map(|(n, was, now)| format!("{n} {was} → {now}")).collect();
        notes.push(format!("patch types changed from the dictionary it replaces: {}", some(&list)));
    }
    if !undescribed.is_empty() {
        notes.push(format!(
            "not patches of region {region} in {PATH}, so without a role: {} (each is the type types gives it, else patch; workflow.patch gives one a role)",
            blocks::list_some(&undescribed)
        ));
    }
    let c = grid.counts();
    let cells = grid.cells();
    let mut message = match (described, write) {
        (_, true) => format!(
            "{path}: written from the blocks of region {region} ({}×{}×{} blocks, {} missing, {} cells)",
            c[0],
            c[1],
            c[2],
            grid.missing.len(),
            cells.map_or_else(|| "?".to_string(), |n| n.to_string())
        ),
        (true, false) => format!("{PATH}: the blocks of region {region} changed; {path} is as they make it already"),
        (false, false) => format!("{path}: as the blocks of region {region} make it already, nothing to write"),
    };
    if described && write {
        message.push_str(&format!("; the blocks in {PATH}"));
    }
    if hand {
        message.push_str(" (over one edited by hand or not written by umcfGui: case.save keeps it in the backup; edit.undo takes it back)");
    }
    for n in &notes {
        message.push_str(&format!("; {n}"));
    }
    Ok(Outcome::new(
        message,
        json!({
            "region": region,
            "file": path,
            "written": write,
            "replaced": hand,
            "described": described,
            "cells": cells,
            "patchesRemoved": if write { removed.iter().map(|q| q.name.clone()).collect::<Vec<_>>() } else { Vec::new() },
            "entriesLost": if write { lost.iter().map(|q| (q.name.clone(), json!(q.extra))).collect::<Map<String, Value>>() } else { Map::new() },
            "unreadPatches": unread,
            "settingsLost": if write { settings.clone() } else { Vec::new() },
            "patchesAdded": if write { added.iter().map(|n| n.to_string()).collect::<Vec<_>>() } else { Vec::new() },
            "typesChanged": changes.iter().map(|(n, was, now)| json!({ "patch": n, "from": was, "to": now })).collect::<Vec<_>>(),
            "blocks": grid_json(&grid),
            "notes": notes,
        }),
    ))
}
