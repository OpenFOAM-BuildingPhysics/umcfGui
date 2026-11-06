//! Patches of a region's mesh: type, groups, other entries, name (`patch.set`), and new
//! patches from some of the boundary faces, for OpenFOAM's `createPatch` (`patch.create`).

use serde_json::{Value, json};
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_geom::{Vec3, near::near, stl};
use umcf_model::{NewPatch, PatchChange};

use super::bc::entry_text;
use super::case::diagnostic;
use super::geometry::{limits as geometry_limits, surface_path};
use super::scene::{Sel, vector};
use crate::params::{Params, key};
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SET: CommandSpec = CommandSpec {
    id: "patch.set",
    label: "Set patch",
    menu: "Mesh/Patch…",
    shortcut: None,
    params: "type: the patch type (patch, wall, mappedWall, symmetry, symmetryPlane, empty, …). groups: [\"wall\", …], replacing inGroups. entries: other entries of the patch, {\"neighbourRegion\": \"air\", \"neighbourPatch\": \"buildings\"}. name: a new name (one patch only): the region's field files at the start time and in 0.orig, its viewFactorsDict and the mapped patches of other regions that name it follow; the result time directories and each file and line that still has the old name are listed. patches: [{\"region\": \"air\", \"patch\": \"top\"}, …]; default the selected patches. Only those entries change in the mesh's boundary file.",
    enabled: needs_case,
    run: set,
    journal: true,
    undoable: true,
};

/// OpenFOAM's patch types (OpenFOAM-8 and -12), for a note on a type that is none of them.
const KNOWN: &[&str] = &[
    "patch",
    "wall",
    "empty",
    "wedge",
    "symmetry",
    "symmetryPlane",
    "cyclic",
    "cyclicAMI",
    "cyclicSlip",
    "cyclicRepeatAMI",
    "processor",
    "processorCyclic",
    "mapped",
    "mappedPatch",
    "mappedWall",
    "mappedInternal",
    "mappedExtrudedWall",
    "nonConformalCyclic",
    "nonConformalError",
    "nonConformalProcessorCyclic",
    "nonConformalMappedWall",
    "mergedCyclic",
    "internal",
];

/// The most entries one change takes.
const MAX_ENTRIES: usize = 64;

pub(super) const CREATE: CommandSpec = CommandSpec {
    id: "patch.create",
    label: "New patch",
    menu: "Mesh/New patch…",
    shortcut: None,
    params: "Prepares a new patch from boundary faces of a region's mesh, for OpenFOAM's createPatch: the faces go into a face set (constant/<region>/polyMesh/sets/<name>) and the patch into system/<region>/createPatchDict.umcf (constructFrom set), the patches prepared on this mesh; the case's own createPatchDict isn't touched; a face set that is a symbolic link is not written. The mesh changes when `createPatch -region <region> -dict …createPatchDict.umcf -overwrite` runs, once (the line for the case's OpenFOAM version is in the result). name: the new patch (required). region: default the region of the selected patches, or of the picked faces when they are the faces (they must all be of it, and leave the selection once in the patch). type: default patch. groups, entries: as patch.set. The faces, one of: the selected faces (select.pick with faces: true), when none of these is given; faces: [face labels]; box: {\"min\": [x, y, z], \"max\": [x, y, z]}, the faces whose centre is inside; surface: {\"file\": \"buildings.stl\", \"solid\": \"roof\", \"distance\": 0.5}, the faces whose centre is within distance of the surface (or of one solid of an ASCII file). from: [patch names] the faces are taken from (box and surface; default the selected patches of the region, else every patch that is neither coupled nor of a constraint type). When all faces come from one patch, the start-time fields get a copy of its entry for the new patch.",
    enabled: needs_case,
    run: create,
    journal: true,
    undoable: true,
};

/// The most faces given as labels.
const MAX_FACES: usize = 50_000_000;

/// `groups`: patch group names.
fn groups<'a>(id: &str, p: &Params<'a>) -> Result<Option<Vec<&'a str>>, Error> {
    let Ok(v) = p.required("groups") else { return Ok(None) };
    let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `groups` is a list of names")))?;
    if list.len() > MAX_ENTRIES {
        return Err(Error::Params(format!("{id}: more than {MAX_ENTRIES} groups")));
    }
    let mut out = Vec::new();
    for g in list {
        let g = g.as_str().ok_or_else(|| Error::Params(format!("{id}: a group is a name, not {g}")))?;
        out.push(word(id, "group name", g)?);
    }
    Ok(Some(out))
}

/// `entries`: other entries of a patch, as OpenFOAM text.
fn entries(id: &str, p: &Params) -> Result<Vec<(String, String)>, Error> {
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(v) = p.required("entries") {
        let o = v.as_object().ok_or_else(|| Error::Params(format!("{id}: `entries` is an object")))?;
        if o.len() > MAX_ENTRIES {
            return Err(Error::Params(format!("{id}: more than {MAX_ENTRIES} entries")));
        }
        for (k, v) in o {
            let k = key(id, k)?;
            if matches!(k, "type" | "inGroups" | "nFaces" | "startFace") {
                return Err(Error::Params(format!("{id}: `{k}` is not set as an entry")));
            }
            entries.push((k.to_string(), entry_text(id, v)?));
        }
    }
    Ok(entries)
}

/// A patch name or type: one OpenFOAM word.
fn word<'a>(id: &str, what: &str, w: &'a str) -> Result<&'a str, Error> {
    // A number reads as a label, not a word.
    if key(id, w).is_err() || w.contains(['(', ')']) || w.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Params(format!("{id}: `{w}` is not a {what}")));
    }
    Ok(w)
}

fn set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SET.id;
    let p = Params::new(id, p, &["patches", "name", "type", "groups", "entries"])?;
    let name = p.str("name")?.map(|n| word(id, "patch name", n)).transpose()?;
    let kind = p.str("type")?.map(|k| word(id, "patch type", k)).transpose()?;
    let groups = groups(id, &p)?;
    let entries = entries(id, &p)?;
    if name.is_none() && kind.is_none() && groups.is_none() && entries.is_empty() {
        return Err(Error::Params(format!("{id}: give a type, groups, entries or a name")));
    }
    let patches = patches(s, &p)?;
    if name.is_some() && patches.len() != 1 {
        return Err(Error::Params(format!("{id}: a name is for one patch, not {}", patches.len())));
    }
    let pairs: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let change = PatchChange { name, kind, groups: groups.as_deref(), entries: &pairs };
    let o = s.opened_mut()?;
    let mut found: Vec<Value> = Vec::new();
    let mut renamed = Vec::new();
    let warn = |code: &str, path: &str, region: &str, patch: &str, message: &str| json!({ "level": "warning", "code": code, "path": path, "region": region, "patch": patch, "field": null, "message": message });
    for (region, patch) in &patches {
        let report = o.case.set_patch(o.fs.as_ref(), region, patch, &change)?;
        if name.is_some() {
            for f in &report.unread {
                found.push(warn(
                    "patch.unread",
                    f,
                    region,
                    patch,
                    "too large to be read here: an entry for the patch in it keeps the old name",
                ));
            }
            for f in &report.indirect {
                found.push(warn("patch.pattern", f, region, patch, "has a pattern entry or text from elsewhere, which may name the patch by its old name: not changed there"));
            }
        }
        renamed.extend(report.fields.iter().chain(&report.orig).chain(&report.others).chain(&report.partners).cloned());
        if name.is_some() {
            if let Some(last) = report.times.last() {
                let n = report.times.len();
                let shown: Vec<&str> = report.times.iter().take(5).map(String::as_str).collect();
                let more = if n > 5 { format!(" … {last} ({n} time directories)") } else { String::new() };
                found.push(warn(
                    "patch.times",
                    "",
                    region,
                    patch,
                    &format!("the time directories {}{more} aren't read here and may still name the patch `{patch}`: a run started from one of them needs it renamed there", shown.join(", ")),
                ));
            }
            for f in &report.unsearched {
                found.push(warn(
                    "patch.unsearched",
                    f,
                    region,
                    patch,
                    &format!("can't be read: it wasn't searched for `{patch}`"),
                ));
            }
            for (f, why) in &report.not_renamed {
                found.push(warn(
                    "patch.unrenamed",
                    f,
                    region,
                    patch,
                    &format!("not changed, as {why}: rename the patch `{patch}` there by hand if it names it"),
                ));
            }
            // Where the old name is still written: the meshing dictionaries, function objects, …
            let mut by_file: Vec<(String, Vec<usize>)> = Vec::new();
            for (f, line) in &report.mentions {
                match by_file.last_mut() {
                    Some((g, lines)) if g == f => lines.push(*line),
                    _ => by_file.push((f.clone(), vec![*line])),
                }
            }
            for (f, lines) in by_file {
                if lines == [0] {
                    found.push(warn("patch.mention", &f, region, patch, &format!("this directory is named `{patch}`: data found by the patch's name (boundaryData) needs the new name")));
                    continue;
                }
                let at: Vec<String> = lines.iter().map(usize::to_string).collect();
                let (what, verb) = if lines.len() == 1 { ("line", "names") } else { ("lines", "name") };
                found.push(warn(
                    "patch.mention",
                    &f,
                    region,
                    patch,
                    &format!("{what} {} {verb} `{patch}`: rename it there if that is the patch", at.join(", ")),
                ));
            }
            if report.more_mentions {
                found.push(warn(
                    "patch.mention",
                    "",
                    region,
                    patch,
                    &format!("more places name `{patch}` than are listed here ({})", umcf_model::MAX_MENTIONS),
                ));
            }
        }
    }
    if let Some(k) = kind
        && !KNOWN.contains(&k)
    {
        found.push(json!({
            "level": "info",
            "code": "patch.type",
            "path": "",
            "region": null,
            "patch": null,
            "field": null,
            "message": format!("`{k}` is not a patch type umcfGui knows: check its spelling"),
        }));
    }
    if o.case.processors > 0 {
        found.push(json!({
            "level": "warning",
            "code": "patch.decomposed",
            "path": "processor*",
            "region": null,
            "patch": null,
            "field": null,
            "message": "the processor directories still have the patches as they were: decompose the case again before running it",
        }));
    }
    // What the case check says now about these patches.
    let after: Vec<(String, String)> =
        patches.iter().map(|(r, p)| (r.clone(), name.map_or_else(|| p.clone(), str::to_string))).collect();
    found.extend(
        umcf_validate::check(&o.case)
            .iter()
            .filter(|d| {
                d.patch
                    .as_deref()
                    .is_some_and(|x| after.iter().any(|(r, p)| p == x && d.region.as_deref().unwrap_or("") == r))
            })
            .map(diagnostic),
    );
    // A renamed patch stays selected under its new name.
    if let (Some(to), [(region, patch)]) = (name, patches.as_slice()) {
        for sel in &mut s.selection {
            if let Sel::Patch { region: r, patch: p } = sel
                && r == region
                && p == patch
            {
                *p = to.to_string();
            }
        }
    }
    let names: Vec<String> =
        patches.iter().map(|(r, p)| if r.is_empty() { p.clone() } else { format!("{r}/{p}") }).collect();
    let mut what = Vec::new();
    if let Some(k) = kind {
        what.push(format!("type {k}"));
    }
    if let Some(g) = &groups {
        what.push(format!("groups ({})", g.join(" ")));
    }
    if !entries.is_empty() {
        what.push(entries.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", "));
    }
    if let Some(to) = name {
        what.push(format!("renamed {to}"));
    }
    Ok(Outcome::new(
        format!("{}: {}", names.join(", "), what.join("; ")),
        json!({
            "patches": patches.iter().map(|(r, p)| json!({ "region": r, "patch": p })).collect::<Vec<_>>(),
            "renamed_in": renamed,
            "diagnostics": found,
        }),
    ))
}

/// The patches to change, `(region, patch)`: as given, or the selected ones.
fn patches(s: &Session, p: &Params) -> Result<Vec<(String, String)>, Error> {
    let id = SET.id;
    let given: Vec<(String, String)> = match p.required("patches") {
        Ok(v) => {
            let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `patches` is a list")))?;
            let mut out = Vec::new();
            for item in list {
                let o = item.as_object().filter(|o| o.len() == 2);
                let get = |k: &str| o.and_then(|o| o.get(k)).and_then(Value::as_str);
                match (get("region"), get("patch")) {
                    (Some(r), Some(p)) => out.push((r.to_string(), p.to_string())),
                    _ => return Err(Error::Params(format!("{id}: a patch is {{\"region\", \"patch\"}}, not {item}"))),
                }
            }
            out
        }
        Err(_) => s
            .selection()
            .iter()
            .filter_map(|x| match x {
                Sel::Patch { region, patch } => Some((region.clone(), patch.clone())),
                Sel::Solid { .. } | Sel::Face { .. } => None,
            })
            .collect(),
    };
    if given.is_empty() {
        return Err(Error::Params(format!("{id}: no patches: select some, or give `patches`")));
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for x in given {
        if !out.contains(&x) {
            out.push(x);
        }
    }
    Ok(out)
}

fn create(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = CREATE.id;
    let p = Params::new(id, p, &["region", "name", "type", "groups", "entries", "faces", "box", "surface", "from"])?;
    let name = word(id, "patch name", p.required_str("name")?)?;
    let kind = word(id, "patch type", p.str("type")?.unwrap_or("patch"))?;
    let groups = groups(id, &p)?.unwrap_or_default();
    let entries = entries(id, &p)?;
    let selected: Vec<(String, String)> = s
        .selection()
        .iter()
        .filter_map(|x| match x {
            Sel::Patch { region, patch } => Some((region.clone(), patch.clone())),
            Sel::Solid { .. } | Sel::Face { .. } => None,
        })
        .collect();
    let picked: Vec<(String, u32)> = s
        .selection()
        .iter()
        .filter_map(|x| match x {
            Sel::Face { region, face } => Some((region.clone(), *face)),
            _ => None,
        })
        .collect();
    let given = |k: &str| p.required(k).ok();
    let sources = [given("faces"), given("box"), given("surface")];
    // Without one, the selected faces.
    let from_selection = sources.iter().flatten().count() == 0;
    if from_selection && picked.is_empty() {
        return Err(Error::Params(format!(
            "{id}: give the faces as one of `faces`, `box` or `surface`, or select faces (select.pick with faces)"
        )));
    }
    if sources.iter().flatten().count() > 1 {
        return Err(Error::Params(format!("{id}: give the faces as one of `faces`, `box` or `surface`")));
    }
    let region = match p.str("region")? {
        Some(r) => r.to_string(),
        None => {
            let mut regions: Vec<&str> = if from_selection {
                picked.iter().map(|(r, _)| r.as_str()).collect()
            } else {
                selected.iter().map(|(r, _)| r.as_str()).collect()
            };
            regions.sort_unstable();
            regions.dedup();
            match regions.as_slice() {
                [r] => r.to_string(),
                _ => {
                    return Err(Error::Params(format!(
                        "{id}: give `region` (or select patches or faces of one region)"
                    )));
                }
            }
        }
    };
    if from_selection && given("from").is_some() {
        return Err(Error::Params(format!("{id}: `from` is for `box` and `surface`, not the selected faces")));
    }
    if given("faces").is_some() && given("from").is_some() {
        return Err(Error::Params(format!("{id}: `from` is for `box` and `surface`, not `faces`")));
    }
    let o = s.opened_mut()?;
    let patches = o
        .case
        .region(&region)
        .and_then(|r| r.patches.clone())
        .ok_or_else(|| Error::Params(format!("{id}: region `{region}` has no mesh")))?;
    let faces: Vec<u32> = match (given("faces"), given("box"), given("surface")) {
        (None, None, None) => {
            let mine: Vec<u32> = picked.iter().filter(|(r, _)| *r == region).map(|(_, f)| *f).collect();
            let others = picked.len() - mine.len();
            if mine.is_empty() {
                return Err(Error::Params(format!("{id}: none of the picked faces is in region `{region}`")));
            }
            if others > 0 {
                return Err(Error::Params(format!(
                    "{id}: {others} of the picked faces are in other regions than `{region}`: a patch is of one region"
                )));
            }
            if mine.len() > MAX_FACES {
                return Err(Error::Params(format!("{id}: more than {MAX_FACES} faces")));
            }
            mine
        }
        (Some(v), _, _) => labels(id, v)?,
        (None, b, around) => {
            let from = from(id, &p, &region, &patches, &selected)?;
            let (labels, centres) = centres(id, o.fs.as_ref(), &region, &patches, &from)?;
            let inside: Vec<bool> = match (b, around) {
                (Some(b), _) => in_box(id, b, &centres)?,
                (None, Some(v)) => {
                    let (tris, distance) = surface(id, &o.case, o.fs.as_ref(), v)?;
                    near(&tris, &centres, distance)
                }
                (None, None) => Vec::new(),
            };
            labels.into_iter().zip(inside).filter_map(|(f, i)| i.then_some(f)).collect()
        }
    };
    if faces.is_empty() {
        return Err(Error::Failed(format!("{id}: there are no faces of those patches there")));
    }
    let pairs: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let new = NewPatch { name, kind, groups: &groups, entries: &pairs, faces: &faces };
    let report = o.case.create_patch(o.fs.as_ref(), &region, &new)?;
    let warn = |code: &str, path: &str, message: String| json!({ "level": "warning", "code": code, "path": path, "region": region, "patch": name, "field": null, "message": message });
    let mut found: Vec<Value> = Vec::new();
    let command = umcf_model::create_patch_command(&region, o.case.layout.version);
    found.push(json!({
        "level": "info",
        "code": "patch.run",
        "path": report.dict,
        "region": region,
        "patch": name,
        "field": null,
        "message": format!(
            "the mesh gets the patch when `{command}` runs, once, on this mesh (with the other patches prepared in {}); then delete that dictionary: its face sets hold face labels of this mesh only",
            report.dict
        ),
    }));
    if o.case.workflow().meshes.iter().any(|m| m.region == region) {
        found.push(warn("patch.remesh", &report.dict, "the case's scripts make this region's mesh again, which makes it without the patch: run createPatch with that dictionary only on the mesh it was prepared on".to_string()));
    }
    for q in &report.emptied {
        let constraint = patches.iter().any(|x| x.name == *q && umcf_model::is_constraint_type(&x.kind));
        let what = if constraint {
            "OpenFOAM-12's createPatch keeps it without faces, OpenFOAM-8's removes it"
        } else {
            "createPatch removes it, and its conditions no longer apply"
        };
        found.push(warn("patch.emptied", &report.dict, format!("all faces of `{q}` move: {what}")));
    }
    // Faces of a constraint patch (`empty`, `symmetry`, …) given another type stop being treated
    // as such: likely picked by mistake (the front and back of a 2D mesh).
    for (q, n) in &report.from {
        if let Some(x) =
            patches.iter().find(|x| x.name == *q && umcf_model::is_constraint_type(&x.kind) && x.kind != kind)
        {
            found.push(warn(
                "patch.constraint",
                &report.dict,
                format!(
                    "{n} of the faces are of `{q}` ({}): as `{kind}` they are no longer treated as {}",
                    x.kind, x.kind
                ),
            ));
        }
    }
    for f in &report.maybe {
        found.push(json!({ "level": "info", "code": "patch.maybe", "path": f, "region": region, "patch": name, "field": null, "message": format!("a pattern entry or text from elsewhere (an #include) may give `{name}` a condition: check it has one before a run") }));
    }
    for f in &report.resized {
        found.push(warn("patch.resized", f, "has values per face on a patch the faces come from: their number is wrong once createPatch has run, so make this field again (from 0.orig, by setFields, …)".to_string()));
    }
    for f in &report.unfilled {
        found.push(warn(
            "patch.unfilled",
            f,
            format!("has no entry for `{name}`: give it one (bc.set after createPatch, or in this file) before a run"),
        ));
    }
    for f in &report.unread {
        found.push(warn(
            "patch.unread",
            f,
            format!("too large to be read here: check it has an entry for `{name}`, or give it one, before a run"),
        ));
    }
    if o.case.exists("0.orig") == Some(true) {
        found.push(warn(
            "patch.orig",
            "0.orig",
            format!("0.orig has no entry for `{name}`, and preparing the case copies it to 0: add one there too"),
        ));
    }
    if o.case.processors > 0 {
        found.push(warn(
            "patch.decomposed",
            "processor*",
            "the processor directories don't have the new patch: decompose the case again after createPatch"
                .to_string(),
        ));
    }
    // The picked faces are in the new patch now: they leave the selection.
    if from_selection {
        s.selection.retain(|x| !matches!(x, Sel::Face { region: r, .. } if *r == region));
    }
    if !KNOWN.contains(&kind) {
        found.push(json!({ "level": "info", "code": "patch.type", "path": "", "region": null, "patch": null, "field": null, "message": format!("`{kind}` is not a patch type umcfGui knows: check its spelling") }));
    }
    let at = if region.is_empty() { name.to_string() } else { format!("{region}/{name}") };
    Ok(Outcome::new(
        format!("{at}: {} faces prepared for createPatch", faces.len()),
        json!({
            "region": region,
            "patch": name,
            "faces": faces.len(),
            "from": report.from.iter().map(|(q, n)| json!({ "patch": q, "faces": n })).collect::<Vec<_>>(),
            "set": report.set,
            "dict": report.dict,
            "fields": report.fields,
            "diagnostics": found,
        }),
    ))
}

/// `faces`: face labels.
fn labels(id: &str, v: &Value) -> Result<Vec<u32>, Error> {
    let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `faces` is a list of face labels")))?;
    if list.len() > MAX_FACES {
        return Err(Error::Params(format!("{id}: more than {MAX_FACES} faces")));
    }
    list.iter()
        .map(|f| {
            f.as_u64()
                .and_then(|f| u32::try_from(f).ok())
                .ok_or_else(|| Error::Params(format!("{id}: a face is a label (a whole number), not {f}")))
        })
        .collect()
}

/// The patches `box` and `surface` take faces from: `from`, else the selected ones of the
/// region, else every patch that isn't coupled nor of a constraint type (`empty`, `symmetry`, …:
/// moved to another type, their faces would no longer be treated as such).
fn from(
    id: &str,
    p: &Params,
    region: &str,
    patches: &[umcf_model::Patch],
    selected: &[(String, String)],
) -> Result<Vec<String>, Error> {
    if let Ok(v) = p.required("from") {
        let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `from` is a list of patch names")))?;
        if list.len() > patches.len() + MAX_ENTRIES {
            return Err(Error::Params(format!("{id}: `from` has more names than region `{region}` has patches")));
        }
        let mut out: Vec<String> = Vec::new();
        for x in list {
            let x = x.as_str().ok_or_else(|| Error::Params(format!("{id}: a patch is a name, not {x}")))?;
            if !patches.iter().any(|q| q.name == x) {
                return Err(Error::Params(format!("{id}: region `{region}` has no patch `{x}`")));
            }
            if !out.iter().any(|o| o == x) {
                out.push(x.to_string());
            }
        }
        return Ok(out);
    }
    let mine: Vec<String> = selected.iter().filter(|(r, _)| r == region).map(|(_, q)| q.clone()).collect();
    if !mine.is_empty() {
        return Ok(mine);
    }
    Ok(patches
        .iter()
        .filter(|q| !umcf_model::COUPLED.contains(&q.kind.as_str()) && !umcf_model::is_constraint_type(&q.kind))
        .map(|q| q.name.clone())
        .collect())
}

/// The faces of the patches `from`, with their centres (the mean of their points).
fn centres(
    id: &str,
    fs: &(dyn umcf_core::CaseFs + Sync),
    region: &str,
    patches: &[umcf_model::Patch],
    from: &[String],
) -> Result<(Vec<u32>, Vec<Vec3>), Error> {
    let mesh = PolyMesh::read(fs, &mesh_dir(region), &Limits::default())
        .map_err(|e| Error::Failed(format!("{id}: the mesh of region `{region}` can't be read: {e}")))?;
    if mesh.boundary.len() != patches.len() || mesh.boundary.iter().zip(patches).any(|(a, b)| a.faces() != b.faces()) {
        return Err(Error::Failed(format!(
            "{id}: the mesh of region `{region}` on disk has other patches than the case: save or reopen it first"
        )));
    }
    let mut labels = Vec::new();
    let mut centres = Vec::new();
    for q in patches.iter().filter(|q| from.contains(&q.name)) {
        for f in q.faces() {
            let (Some(points), Ok(label)) = (mesh.face(f), u32::try_from(f)) else { continue };
            let points: Vec<Vec3> = points
                .iter()
                .filter_map(|&pt| mesh.points.get(pt as usize))
                .map(|x| Vec3::new(x[0], x[1], x[2]))
                .collect();
            if let Some(c) = face_centre(&points) {
                labels.push(label);
                centres.push(c);
            }
        }
    }
    Ok((labels, centres))
}

/// A face's centre as OpenFOAM computes it (`face::areaAndCentre`): the centres of the
/// triangles from each edge to the mean of the points, weighted by their area along the face's
/// normal; the mean itself for a triangle or a face without area.
fn face_centre(points: &[Vec3]) -> Option<Vec3> {
    let n = points.len();
    if n == 0 {
        return None;
    }
    let mean = points.iter().fold(Vec3::ZERO, |a, p| a + *p) * (1.0 / n as f64);
    if n == 3 {
        return Some(mean);
    }
    let edge = |i: usize| (points.get(i).copied().unwrap_or(mean), points.get((i + 1) % n).copied().unwrap_or(mean));
    let area = |i: usize| {
        let (p, q) = edge(i);
        (q - p).cross(mean - p)
    };
    let normal = (0..n).fold(Vec3::ZERO, |a, i| a + area(i));
    let len = normal.length();
    if !(len > 0.0 && len.is_finite()) {
        return Some(mean);
    }
    let unit = normal * (1.0 / len);
    let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
    for i in 0..n {
        let (p, q) = edge(i);
        let w = area(i).dot(unit);
        sum = sum + (p + q + mean) * w;
        weight += w;
    }
    Some(if weight > f64::MIN_POSITIVE { sum * (1.0 / (3.0 * weight)) } else { mean })
}

/// `box`: which centres are inside it.
fn in_box(id: &str, b: &Value, centres: &[Vec3]) -> Result<Vec<bool>, Error> {
    let o = b
        .as_object()
        .filter(|o| o.len() == 2 && o.contains_key("min") && o.contains_key("max"))
        .ok_or_else(|| Error::Params(format!("{id}: `box` is {{\"min\": [x, y, z], \"max\": [x, y, z]}}")))?;
    let lo = vector(id, "box.min", o.get("min"))?;
    let hi = vector(id, "box.max", o.get("max"))?;
    if lo.iter().zip(&hi).any(|(a, b)| a > b) {
        return Err(Error::Params(format!("{id}: `box.min` is above `box.max` in x, y or z")));
    }
    Ok(centres
        .iter()
        .map(|c| [c.x, c.y, c.z].iter().zip(lo.iter().zip(&hi)).all(|(x, (a, b))| x >= a && x <= b))
        .collect())
}

/// `surface`: its triangles (of one solid, when given) and the distance.
fn surface(
    id: &str,
    case: &umcf_model::Case,
    fs: &dyn umcf_core::CaseFs,
    v: &Value,
) -> Result<(Vec<[Vec3; 3]>, f64), Error> {
    let shape = || {
        Error::Params(format!(
            "{id}: `surface` is {{\"file\": \"buildings.stl\", \"solid\": \"roof\", \"distance\": 0.5}}"
        ))
    };
    let o = v.as_object().ok_or_else(shape)?;
    if o.keys().any(|k| !matches!(k.as_str(), "file" | "solid" | "distance")) {
        return Err(shape());
    }
    let file = o.get("file").and_then(Value::as_str).ok_or_else(shape)?;
    let distance = o
        .get("distance")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d >= 0.0)
        .ok_or_else(|| Error::Params(format!("{id}: `surface.distance` is a number, 0 or more")))?;
    let rel = surface_path(id, file, false)?;
    let bytes = case.file(fs, &rel)?.ok_or_else(|| Error::Failed(format!("{id}: there is no surface {rel}")))?;
    let stl = stl::read(&bytes, &geometry_limits()).map_err(|e| Error::Failed(format!("{id}: {rel}: {e}")))?;
    let solid = match o.get("solid") {
        None => None,
        Some(Value::String(name)) => {
            let ids: Vec<u32> = stl
                .solids
                .iter()
                .enumerate()
                .filter(|(_, n)| *n == name)
                .filter_map(|(i, _)| u32::try_from(i).ok())
                .collect();
            if ids.is_empty() {
                return Err(Error::Params(format!("{id}: {rel} has no solid `{name}`")));
            }
            Some(ids)
        }
        Some(_) => return Err(shape()),
    };
    let tris = stl
        .triangles
        .iter()
        .filter(|t| solid.as_ref().is_none_or(|ids| ids.contains(&t.solid)))
        .map(|t| t.points())
        .collect();
    Ok((tris, distance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_centres_are_area_weighted() {
        let v = |x: f64, y: f64| Vec3::new(x, y, 0.0);
        let c = face_centre(&[v(0.0, 0.0), v(4.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)]).unwrap();
        assert!((c.x - 1.4).abs() < 1e-12 && (c.y - 0.4).abs() < 1e-12, "{c:?}");
        let c = face_centre(&[v(0.0, 0.0), v(3.0, 0.0), v(0.0, 3.0)]).unwrap();
        assert!((c.x - 1.0).abs() < 1e-12 && (c.y - 1.0).abs() < 1e-12, "{c:?}");
        // Without area: the mean.
        let c = face_centre(&[v(0.0, 0.0), v(2.0, 0.0), v(4.0, 0.0), v(2.0, 0.0)]).unwrap();
        assert!((c.x - 2.0).abs() < 1e-12, "{c:?}");
        assert!(face_centre(&[]).is_none());
    }
}
