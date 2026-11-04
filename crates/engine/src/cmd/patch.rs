//! Patches of a region's mesh (`patch.set`): type, groups, other entries, name.

use serde_json::{Value, json};
use umcf_model::PatchChange;

use super::bc::entry_text;
use super::case::diagnostic;
use super::scene::Sel;
use crate::params::{Params, key};
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SET: CommandSpec = CommandSpec {
    id: "patch.set",
    label: "Set patch",
    menu: "Mesh/Patch…",
    shortcut: None,
    params: "type: the patch type (patch, wall, mappedWall, symmetry, symmetryPlane, empty, …). groups: [\"wall\", …], replacing inGroups. entries: other entries of the patch, {\"neighbourRegion\": \"air\", \"neighbourPatch\": \"buildings\"}. name: a new name (one patch only): the region's field files at the start time and the mapped patches of other regions that name it follow. patches: [{\"region\": \"air\", \"patch\": \"top\"}, …]; default the selected patches. Only those entries change in the mesh's boundary file.",
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

/// A patch name or type: one OpenFOAM word.
fn word<'a>(id: &str, what: &str, w: &'a str) -> Result<&'a str, Error> {
    if key(id, w).is_err() || w.contains(['(', ')']) {
        return Err(Error::Params(format!("{id}: `{w}` is not a {what}")));
    }
    Ok(w)
}

fn set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SET.id;
    let p = Params::new(id, p, &["patches", "name", "type", "groups", "entries"])?;
    let name = p.str("name")?.map(|n| word(id, "patch name", n)).transpose()?;
    let kind = p.str("type")?.map(|k| word(id, "patch type", k)).transpose()?;
    let groups: Option<Vec<&str>> = match p.required("groups") {
        Ok(v) => {
            let list = v.as_array().ok_or_else(|| Error::Params(format!("{id}: `groups` is a list of names")))?;
            if list.len() > MAX_ENTRIES {
                return Err(Error::Params(format!("{id}: more than {MAX_ENTRIES} groups")));
            }
            let mut out = Vec::new();
            for g in list {
                let g = g.as_str().ok_or_else(|| Error::Params(format!("{id}: a group is a name, not {g}")))?;
                out.push(word(id, "group name", g)?);
            }
            Some(out)
        }
        Err(_) => None,
    };
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
        renamed.extend(report.fields.iter().chain(&report.partners).cloned());
    }
    if let Some(to) = name {
        if o.case.exists("0.orig") == Some(true) {
            found.push(warn("patch.orig", "0.orig", "", to, "0.orig still names the patch by its old name, and preparing the case copies it to 0: rename it there too"));
        }
        found.push(json!({
            "level": "info",
            "code": "patch.elsewhere",
            "path": "",
            "region": null,
            "patch": to,
            "field": null,
            "message": "other files may name the patch by its old name: time directories other than the start, function objects in controlDict, viewFactorsDict, the meshing dictionaries",
        }));
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
                Sel::Solid { .. } => None,
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
