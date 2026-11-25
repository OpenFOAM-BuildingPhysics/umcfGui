//! Each region's `changeDictionaryDict`, generated from `system/umcfWorkflow`: the patches
//! mapped to another region, typed by their roles and pointed at their partners as the coupling
//! rules say (docs/case-requirements.md, *Coupling rules*).
//!
//! - A patch with a partner is `mappedWall` to it when its role is a wall's (`ground`,
//!   `coupled`, `interior`, `bottom`), and `mapped` (`mappedPatch` in OpenFOAM-8) when it is an
//!   open side's (`inlet`, `outlet`, `side`, `top`, `sky`): the vegetation region's sides and
//!   top, which the radiation takes for the sky.
//! - A pair of a fluid's and a solid's patches is paired face by face with `method nearest`
//!   (OpenFOAM-12; OpenFOAM-8's `sampleMode nearestPatchFace` does the same), as the 3×3
//!   reference cases do (the tutorials give none: their meshes are conformal, paired by an
//!   octree, which `nearest` pairs the same); a vegetation patch and its air twin, made from
//!   the same mesh, are conformal and take no `method`.
//! - The vegetation region's `canopy` patch maps to the air's cells: `mappedInternal` with
//!   `offsetMode normal; distance 0;` in OpenFOAM-12, `mappedWall` with `sampleMode
//!   nearestCell` in OpenFOAM-8.
//!
//! Patches without a partner (and not the canopy) are left as the mesh makes them.
//! `changeDictionary` adds keys to a patch rather than replacing it, so keys the mesh had stay:
//! a vegetation region split from the air keeps the air patches' `neighbourPatch` and `method`
//! on its `mappedInternal` patches, which OpenFOAM-12 doesn't read (`mappedInternalPatchBase`
//! reads `neighbourRegion`, `offsetMode` and `distance` or `offset`).

use crate::RegionKind;
use crate::description::{Description, RegionDesc, Role};
use crate::workflow::RegionStep;
use std::fmt::Write as _;
use umcf_core::OfVersion;

/// One patch's entry in `boundary`: its name and its keys and values, in the order written.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub patch: String,
    pub keys: Vec<(&'static str, String)>,
}

impl Entry {
    /// The value of `key`, if the entry has it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }
}

/// Where a region's `changeDictionaryDict` is (the same in OpenFOAM-8 and -12).
pub fn dict_path(region: &str) -> String {
    format!("system/{region}/changeDictionaryDict")
}

/// Whether `region`'s steps run `changeDictionary`, so its `changeDictionaryDict` is read.
pub fn runs(r: &RegionDesc) -> bool {
    r.steps.iter().any(|s| matches!(s, RegionStep::ChangeDictionary))
}

/// The entries of `region`'s `changeDictionaryDict` for `version`, from its patches' roles and
/// partners; an error says why one can't be written (a partner without a role, a partner
/// region the description doesn't have, a canopy with a partner or no air to map to, a name
/// that isn't a plain OpenFOAM word, a region's name that can't be a directory everywhere).
pub fn entries(d: &Description, region: &str, version: OfVersion) -> Result<Vec<Entry>, String> {
    let Some(r) = d.region(region) else {
        return Err(format!("region {region} isn't in the description"));
    };
    // The names are written as they are, unquoted: each a plain OpenFOAM word, so none changes
    // the dictionary's syntax. A region's name is a directory too (`system/<region>`): not `.`
    // or `..`, no `:`, not ending with `.`, not a name Windows keeps for a device.
    let plain = |what: &str, n: &str| {
        if crate::description_edit::plain_name(n) {
            Ok(())
        } else {
            Err(format!("{what} `{n}` isn't a plain OpenFOAM word: rename it to write the changeDictionaryDict"))
        }
    };
    let directory = |n: &str| {
        plain("region", n)?;
        if n.contains(':') || n.ends_with('.') || crate::new_case::windows_device(n) {
            return Err(format!(
                "region `{n}` can't be a directory on every system (a `:`, a `.` at its end, or a Windows device's name): rename it to write the changeDictionaryDict"
            ));
        }
        Ok(())
    };
    directory(region)?;
    let v8 = version == OfVersion::V8;
    let mut out = Vec::new();
    for p in &r.patches {
        let coupled = p.partner.is_some() || p.role == Some(Role::Canopy);
        if coupled {
            plain(&format!("patch of region {region}"), &p.name)?;
        }
        if p.role == Some(Role::Canopy) && p.partner.is_none() {
            let air = canopy_air(d, r)?;
            directory(&air)?;
            let keys = if v8 {
                vec![
                    ("type", "mappedWall".to_string()),
                    ("sampleMode", "nearestCell".to_string()),
                    ("sampleRegion", air),
                    ("offsetMode", "uniform".to_string()),
                    ("offset", "(0 0 0)".to_string()),
                ]
            } else {
                vec![
                    ("type", "mappedInternal".to_string()),
                    ("neighbourRegion", air),
                    ("offsetMode", "normal".to_string()),
                    ("distance", "0".to_string()),
                ]
            };
            out.push(Entry { patch: p.name.clone(), keys });
            continue;
        }
        let Some((of, with)) = &p.partner else { continue };
        let Some(other) = d.region(of) else {
            return Err(format!(
                "patch {} of region {region} is coupled to region {of}, which isn't in the description",
                p.name
            ));
        };
        directory(of)?;
        plain(&format!("patch of region {of}"), with)?;
        let open = match p.role {
            Some(Role::Inlet | Role::Outlet | Role::Side | Role::Top | Role::Sky) => true,
            Some(Role::Ground | Role::Coupled | Role::Interior | Role::Bottom) => false,
            Some(Role::Canopy) => {
                return Err(format!(
                    "patch {} of region {region} is the canopy and coupled to {of}/{with}: the canopy maps to the air's cells, not to a patch",
                    p.name
                ));
            }
            None => {
                return Err(format!(
                    "patch {} of region {region} is coupled to {of}/{with} but has no role: give it one (workflow.patch) to tell a wall from an open side",
                    p.name
                ));
            }
        };
        let ty = match (open, v8) {
            (false, _) => "mappedWall",
            (true, true) => "mappedPatch",
            (true, false) => "mapped",
        };
        // A fluid's patch and a solid's: paired by the nearest face, as the reference cases.
        let nearest = matches!(
            (r.kind, other.kind),
            (RegionKind::Fluid, RegionKind::Solid) | (RegionKind::Solid, RegionKind::Fluid)
        );
        let mut keys = vec![("type", ty.to_string())];
        if v8 {
            keys.push(("sampleMode", "nearestPatchFace".to_string()));
            keys.push(("sampleRegion", of.clone()));
            keys.push(("samplePatch", with.clone()));
        } else {
            if nearest {
                keys.push(("method", "nearest".to_string()));
            }
            keys.push(("neighbourRegion", of.clone()));
            keys.push(("neighbourPatch", with.clone()));
        }
        keys.push(("offsetMode", "uniform".to_string()));
        keys.push(("offset", "(0 0 0)".to_string()));
        out.push(Entry { patch: p.name.clone(), keys });
    }
    Ok(out)
}

/// The fluid region a vegetation region's canopy maps to: the one its other patches are
/// coupled to, else the one `calcLAI` runs on, else the case's only fluid region.
fn canopy_air(d: &Description, r: &RegionDesc) -> Result<String, String> {
    let fluid = |n: &str| d.region(n).is_some_and(|x| x.kind == RegionKind::Fluid);
    let fluid = |n: &str| n != r.name && fluid(n);
    if let Some((of, _)) = r.patches.iter().filter_map(|p| p.partner.as_ref()).find(|(of, _)| fluid(of)) {
        return Ok(of.clone());
    }
    if let Some(lai) = d.radiation.as_ref().and_then(|x| x.lai_region.as_ref()).filter(|n| fluid(n)) {
        return Ok(lai.clone());
    }
    let mut fluids = d.regions.iter().filter(|x| x.kind == RegionKind::Fluid && x.name != r.name);
    match (fluids.next(), fluids.next()) {
        (Some(only), None) => Ok(only.name.clone()),
        _ => Err(format!(
            "the canopy of region {} maps to the air's cells, but which fluid region isn't told: couple another of its patches to it",
            r.name
        )),
    }
}

/// `region`'s `changeDictionaryDict` for `version`, generated ([`crate::blocks::stamped`]).
pub fn dict(d: &Description, region: &str, version: OfVersion) -> Result<String, String> {
    let entries = entries(d, region, version)?;
    let mut s = format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system/{region}\";\n    object      changeDictionaryDict;\n}}\n\nboundary\n{{\n"
    );
    for (k, e) in entries.iter().enumerate() {
        if k > 0 {
            s.push('\n');
        }
        let _ = writeln!(s, "    {}\n    {{", e.patch);
        for (key, value) in &e.keys {
            let _ = writeln!(s, "        {key:<16}{value};");
        }
        s.push_str("    }\n");
    }
    s.push_str("}\n\n// ************************************************************************* //\n");
    Ok(crate::blocks::stamped(
        &format!(
            "The patches of region {region} in system/umcfWorkflow, by role and partner (docs/common-workflow.md)."
        ),
        &s,
    ))
}

/// The methods OpenFOAM-12 pairs a mapped patch's faces by (its `patchToPatch` types).
const METHODS_12: [&str; 4] = ["intersection", "inverseDistance", "matching", "nearest"];

/// OpenFOAM-8's sample modes (`mappedPatchBase::sampleModeNames_`).
const MODES_8: [&str; 6] =
    ["nearestCell", "nearestPatchFace", "nearestPatchFaceAMI", "nearestPatchPoint", "nearestFace", "nearestOnlyCell"];

/// How a `changeDictionaryDict`'s `boundary` (`root`, the file's top level) differs from
/// `entries` (made for `version`) in what matters to the coupling, by the keys the version
/// reads and only those given: changeDictionary merges each block into the mesh's patch, so a
/// key left out keeps the mesh's value, not told here (a region or patch the version reads,
/// left out, is said to be unconfirmed; for OpenFOAM-8 not where OpenFOAM-12's key stands in
/// its place, said to be unread). A block with a `~key`, which takes the key out of the
/// mesh's patch, isn't compared, said so: what OpenFOAM reads then isn't told here. A name
/// that isn't a word (empty, or a dictionary), a type that is a dictionary and a `samePatch`
/// that isn't a switch, which OpenFOAM refuses, are said so and not compared.
/// - each entry's patch there, and its type;
/// - the region and patch it maps to: `sampleRegion`/`samplePatch` for OpenFOAM-8 (OpenFOAM-12's
///   keys in their place said to be unread); `neighbourRegion`/`neighbourPatch` where given,
///   else the older `sampleRegion`/`samplePatch`, for -12 (which reads the newer first, so an
///   older one alone, the description's, can't be confirmed: the mesh's patch keeps a newer
///   one it has). OpenFOAM-12's `mappedInternal`
///   reads no patch (its tutorials name one all the same); OpenFOAM-8 stops on one (other than
///   `none`) in the modes that sample no patch (`nearestCell`, the canopy's, `nearestOnlyCell`,
///   `nearestFace`), with a group too, and without a mode given the patch isn't compared. With
///   a `coupleGroup`, OpenFOAM-8 takes the region and patch given only when both are (in the
///   cells' modes the region alone, `nearestFace` asking the group for a patch not given,
///   refused; compared then; else the group's, not compared); OpenFOAM-12
///   stops on a group given with either. A partner named by the group alone isn't compared.
///   OpenFOAM-12's `samePatch` (a switch: `true`, `yes`, `on` and the like, or a number other
///   than 0) names the patch of the same name: refused with a patch named too (the patch not
///   compared then); alone, compared, and where it is the description's, unconfirmed, as
///   OpenFOAM-12 refuses it beside a `neighbourPatch` the mesh's patch may keep; one that isn't
///   a switch (quoted, too) refused, the patch not compared then;
/// - OpenFOAM-8's `sampleMode` it hasn't, or one sampling no patch where the entry's samples
///   one, or the other way round; OpenFOAM-12's `method` (or `sampleMode`, its older name) that
///   it hasn't;
/// - the patches it changes that `entries` doesn't map, and a value in `boundary` (which
///   changeDictionary adds to the mesh's `boundary` file), and a `~patch` there, quoted or
///   not, on which changeDictionary stops where the mesh has the patch (with a block for it
///   after, the block is written in its place without its faces; neither compared).
///
/// A method OpenFOAM-12 has, other sample modes and the offsets aren't compared, nor a value
/// given by a macro or directive (`$solid`, `#calc`), nor, with OpenFOAM-8's `sampleMode`
/// given by one, the `samplePatch` (whether it samples a patch isn't told). A key that is a
/// pattern (`"air_to_.*"`) or a group isn't matched to patches: a patch mapped only through
/// one is said not to be there. Keys a patch's block brings in (`$common;`, `#include`) aren't
/// seen, and of a patch given twice only the last is read (OpenFOAM merges them).
pub fn differences(root: &umcf_foam_dict::Dict, entries: &[Entry], version: OfVersion) -> Vec<String> {
    let Some(boundary) = root.get("boundary").and_then(umcf_foam_dict::Entry::dict) else {
        return vec!["it has no boundary dictionary".to_string()];
    };
    let v8 = version == OfVersion::V8;
    let mut out = Vec::new();
    let value = |d: &umcf_foam_dict::Dict, k: &str| {
        d.get(k).and_then(umcf_foam_dict::Entry::value_text).map(|v| v.trim_matches('"').to_string())
    };
    // A value from a macro or directive (`$mode`, `#calc`) is told only once it is expanded:
    // not compared.
    let untold = |v: &Option<String>| v.as_deref().is_some_and(|v| v.starts_with(['$', '#']));
    let blank = |v: &Option<String>| v.as_deref() == Some("");
    // A value as said: an empty one as `""`.
    let shown = |v: &str| if v.is_empty() { "\"\"".to_string() } else { v.to_string() };
    // OpenFOAM's switches: its words for true, or a number other than 0.
    let on = |v: &str| matches!(v, "true" | "yes" | "on" | "y" | "t" | "any") || v.parse::<i64>().is_ok_and(|n| n != 0);
    for e in entries {
        let Some(there) = boundary.get(&e.patch).and_then(umcf_foam_dict::Entry::dict) else {
            if boundary.get(&format!("~{}", e.patch)).is_none() {
                out.push(format!("patch {} isn't there", e.patch));
            }
            continue;
        };
        // `~patch` in boundary (said below): the block isn't merged into the mesh's patch.
        if boundary.get(&format!("~{}", e.patch)).is_some() {
            continue;
        }
        // `~key` takes a key out of the mesh's patch: what OpenFOAM reads then (the mesh's
        // other keys, the version's defaults, or a stop) isn't told here.
        let taken: Vec<&str> = there.entries().filter_map(|t| t.key().strip_prefix('~')).collect();
        if !taken.is_empty() {
            out.push(format!(
                "patch {}: it takes {} out of the mesh's patch there, after which what OpenFOAM reads isn't told: not compared",
                e.patch,
                taken.join(", ")
            ));
            continue;
        }
        let given = |k: &str| there.get(k).is_some();
        // changeDictionary merges the block into the patch the mesh has: a key left out keeps
        // the mesh's value, which isn't told here, so it isn't compared.
        let differ = |what: &str, want: Option<String>, have: Option<String>| {
            have.filter(|h| want.as_deref() != Some(h.as_str()) && !untold(&Some(h.clone()))).map(|h| {
                format!(
                    "patch {}: {what} is {} there, the description makes it {}",
                    e.patch,
                    shown(&h),
                    want.as_deref().unwrap_or("not given")
                )
            })
        };
        // The region or patch it maps to, from the key OpenFOAM reads (`key`): one left out is
        // the mesh's, which can't be confirmed (said so); a name that isn't a word (empty, or a
        // dictionary) is said once, not compared.
        let name_differ = |what: &str, want: Option<String>, key: &str, have: Option<String>| {
            if have.is_none() && there.get(key).is_none() {
                want.map(|w| {
                    format!(
                        "patch {}: {what} isn't given there, so the mesh's is kept, which can't be confirmed (the description makes it {w})",
                        e.patch
                    )
                })
            } else if blank(&have) || have.is_none() {
                None
            } else {
                differ(what, want, have)
            }
        };
        let want = |k: &str| e.get(k).map(str::to_string);
        // A type that isn't a word (a dictionary): OpenFOAM stops on it. One left out is the
        // mesh's: not compared.
        if there.get("type").is_some() && value(there, "type").is_none() {
            out.push(format!("patch {}: type is a dictionary there, which OpenFOAM refuses (not a word)", e.patch));
        }
        out.extend(differ("type", want("type"), value(there, "type")));
        let internal = !v8 && e.get("type") == Some("mappedInternal");
        // OpenFOAM-8's mode there; these sample no patch: the cells (the canopy's) or the faces.
        // Without one there (the mesh's, not told) or from a macro, whether the patch samples
        // one isn't told, nor so the samplePatch compared.
        let mode = value(there, "sampleMode");
        let unpatched = |m: Option<&str>| matches!(m, Some("nearestCell" | "nearestOnlyCell" | "nearestFace"));
        let cells = v8 && unpatched(mode.as_deref());
        let moot = v8 && (mode.is_none() || untold(&mode));
        // A patch coupled by its group (`coupleGroup`): OpenFOAM-8 takes the region and patch
        // given over the group's only when both are (in the cells' modes the region alone; else
        // both from the group: neither compared); OpenFOAM-12 stops on a group given with
        // either (its mappedInternal reads no group). A partner named by the group alone isn't
        // compared.
        let grouped = !internal && there.get("coupleGroup").is_some();
        // A name that isn't a word: OpenFOAM stops on it. OpenFOAM-12 reads the newer key where
        // given, else the older; no patch for its mappedInternal, and none with a group
        // (refused with either, said below).
        let read = |new: &'static str, old: &'static str| if v8 || !given(new) { old } else { new };
        let mut keys = match (v8, grouped, internal) {
            (false, true, _) => vec![],
            (false, false, true) => vec![read("neighbourRegion", "sampleRegion")],
            _ => vec![read("neighbourRegion", "sampleRegion"), read("neighbourPatch", "samplePatch")],
        };
        // The group's name too (OpenFOAM-12's mappedInternal reads none).
        if grouped {
            keys.insert(0, "coupleGroup");
        }
        for k in keys.iter().filter(|k| given(k)) {
            match value(there, k) {
                Some(v) if v.is_empty() => out.push(format!(
                    "patch {}: {k} is an empty name there, which OpenFOAM refuses (not a word)",
                    e.patch
                )),
                None => out
                    .push(format!("patch {}: {k} is a dictionary there, which OpenFOAM refuses (not a word)", e.patch)),
                Some(_) => {}
            }
        }
        // OpenFOAM-12's samePatch: a switch, a word (its words for true and false) or a number;
        // anything else (quoted, a dictionary) it refuses, said so, and the patch isn't
        // compared then.
        let raw = there.get("samePatch").and_then(umcf_foam_dict::Entry::value_text).map(|v| v.trim().to_string());
        let switch =
            |v: &str| on(v) || matches!(v, "false" | "no" | "off" | "n" | "f" | "none") || v.parse::<i64>().is_ok();
        let same = raw.as_deref().is_some_and(on);
        let odd_same = !v8
            && !internal
            && given("samePatch")
            && raw.as_deref().is_none_or(|v| !untold(&Some(v.to_string())) && !switch(v));
        if odd_same {
            out.push(format!(
                "patch {}: samePatch is {} there, which OpenFOAM-12 refuses (not a switch)",
                e.patch,
                raw.as_deref().map_or("a dictionary".to_string(), |v| if v.is_empty() {
                    "\"\"".to_string()
                } else {
                    v.to_string()
                })
            ));
        }
        if v8 {
            // OpenFOAM-12's keys in their place: OpenFOAM-8 doesn't read them.
            for (new, old) in [("neighbourRegion", "sampleRegion"), ("neighbourPatch", "samplePatch")] {
                if given(new) && !given(old) {
                    out.push(format!(
                        "patch {}: it has {new} there, which OpenFOAM-8 doesn't read (it reads {old})",
                        e.patch
                    ));
                }
            }
            for (what, key) in [("the region it maps to", "sampleRegion"), ("the patch it maps to", "samplePatch")] {
                // The cells' modes read the patch as given (none needed); nearestFace asks the
                // group for one not given.
                let reads_cells = matches!(mode.as_deref(), Some("nearestCell" | "nearestOnlyCell"));
                let both = given("sampleRegion") && (given("samplePatch") || reads_cells);
                if (key == "samplePatch" && (cells || moot)) || (grouped && !both) {
                    continue;
                }
                // OpenFOAM-12's key in its place is said above to be unread.
                let newer = if key == "sampleRegion" { "neighbourRegion" } else { "neighbourPatch" };
                if !given(key) && given(newer) {
                    continue;
                }
                out.extend(name_differ(what, want(key), key, value(there, key)));
            }
        } else if grouped {
            let keys = ["neighbourRegion", "neighbourPatch", "sampleRegion", "samplePatch"];
            if let Some(k) = keys.iter().find(|k| given(k)).copied().or(same.then_some("samePatch")) {
                out.push(format!(
                    "patch {}: it has a coupleGroup and {k} there, which OpenFOAM-12 refuses (one or the other)",
                    e.patch
                ));
            }
        } else {
            // `samePatch true` maps to the patch of the same name; with a patch named too,
            // OpenFOAM-12 stops.
            if same
                && !internal
                && let Some(k) = ["neighbourPatch", "samplePatch"].into_iter().find(|k| given(k))
            {
                out.push(format!(
                    "patch {}: it has samePatch and {k} there, which OpenFOAM-12 refuses (one or the other)",
                    e.patch
                ));
            }
            // The key OpenFOAM-12 reads: the newer where given (whatever it holds), else the older.
            let have = |new: &'static str, old: &'static str| {
                value(there, read(new, old)).or_else(|| (same && new == "neighbourPatch").then(|| e.patch.clone()))
            };
            // Only the older key given: OpenFOAM-12 reads the newer one first, and the mesh's
            // patch (`splitMeshRegions` writes the newer keys) keeps its own where it has one,
            // so the older one's value, the description's, can't be confirmed to be read.
            let shadowed = |what: &str, new: &str, old: &str, want: Option<String>, have: Option<String>| match (
                want, have,
            ) {
                (Some(w), Some(h)) if !given(new) && given(old) && w == h => Some(format!(
                    "patch {}: {what} is given there as {old} only, which OpenFOAM-12 reads after {new}, so the mesh's {new}, where it has one, is kept, which can't be confirmed (the description makes it {w})",
                    e.patch
                )),
                _ => None,
            };
            let mut pairs = vec![("the region it maps to", "neighbourRegion", "sampleRegion")];
            if !internal {
                pairs.push(("the patch it maps to", "neighbourPatch", "samplePatch"));
            }
            for (what, new, old) in pairs {
                // A samePatch OpenFOAM-12 refuses (said above): the patch isn't compared.
                if odd_same && new == "neighbourPatch" {
                    continue;
                }
                let said = if same && new == "neighbourPatch" {
                    // With a patch named too it is refused (said above), not compared. Alone,
                    // the mesh's patch keeps a neighbourPatch it has (OpenFOAM-12 writes one on
                    // every mapped patch), beside which OpenFOAM-12 refuses samePatch.
                    if given(new) || given(old) {
                        continue;
                    }
                    (want(new).as_deref() == Some(e.patch.as_str())).then(|| {
                        format!(
                            "patch {}: it has samePatch there, which OpenFOAM-12 refuses beside a neighbourPatch, and the mesh's patch keeps one it has (OpenFOAM-12 writes one on a mapped patch), which can't be confirmed",
                            e.patch
                        )
                    })
                } else {
                    shadowed(what, new, old, want(new), have(new, old))
                };
                out.extend(said.or_else(|| name_differ(what, want(new), read(new, old), have(new, old))));
            }
        }
        if v8 {
            if !untold(&mode) {
                // The cells or a face against a patch: the canopy's or a mapped side's.
                match mode.as_deref() {
                    Some(m) if !MODES_8.contains(&m) => out.push(format!(
                        "patch {}: its sampleMode is {} there, which OpenFOAM-8 hasn't (it has {})",
                        e.patch,
                        shown(m),
                        MODES_8.join(", ")
                    )),
                    Some(m) if unpatched(Some(m)) != unpatched(e.get("sampleMode")) => {
                        out.extend(differ("sampleMode", want("sampleMode"), mode.clone()));
                    }
                    _ => {}
                }
            }
            // nearestFace with a group and no patch given asks the group for one: refused.
            if grouped && mode.as_deref() == Some("nearestFace") && !given("samplePatch") {
                out.push(format!(
                    "patch {}: its coupleGroup names a patch, which OpenFOAM-8 refuses in sampleMode nearestFace (samplePatch none will do)",
                    e.patch
                ));
            }
            // In the modes that sample none, OpenFOAM-8 stops on a patch: the one given, or the
            // group's that replaces it.
            let patch = value(there, "samplePatch");
            if cells
                && !untold(&patch)
                && let Some(p) = patch.filter(|p| !p.is_empty() && p != "none")
            {
                out.push(format!(
                    "patch {}: samplePatch is {p} there, which OpenFOAM-8 refuses in sampleMode {}",
                    e.patch,
                    mode.as_deref().unwrap_or_default()
                ));
            }
        } else if !internal {
            // OpenFOAM-12 reads `sampleMode` only as an older `method`; one it hasn't stops the run.
            let method = value(there, "method").or_else(|| value(there, "sampleMode"));
            if let Some(m) = method.filter(|m| !m.starts_with(['$', '#']) && !METHODS_12.contains(&m.as_str())) {
                out.push(format!(
                    "patch {}: its method is {} there, which OpenFOAM-12 hasn't (it has {})",
                    e.patch,
                    shown(&m),
                    METHODS_12.join(", ")
                ));
            }
        }
    }
    let named: std::collections::HashSet<&str> = entries.iter().map(|e| e.patch.as_str()).collect();
    for entry in boundary.entries() {
        let k = entry.key();
        // `~patch`, quoted or not (OpenFOAM reads the `~` first), takes the mesh's patch out:
        // changeDictionary looks each of the mesh's patches up again after merging and stops
        // on one taken out, unless a block for it comes after, written in its place without
        // its faces. A pattern or group taken out does nothing.
        if let Some(p) = k.strip_prefix('~') {
            if p.chars().any(|c| ".*+?()[]{}|^$\\".contains(c)) {
                continue;
            }
            out.push(if boundary.get(p).and_then(umcf_foam_dict::Entry::dict).is_some() {
                format!(
                    "~{p} is in boundary there with a block for {p}: changeDictionary stops where the mesh has patch {p} (the block first), or writes the block in its place without its faces (the block after)"
                )
            } else {
                format!(
                    "~{p} is in boundary there: changeDictionary stops where the mesh has patch {p} (it can't take a patch out)"
                )
            });
            continue;
        }
        // A pattern is quoted (`"air_to_.*"`); a patch's name never is.
        if entry.key.text.starts_with('"') {
            continue;
        }
        if entry.dict().is_none() {
            // changeDictionary adds what matches no patch to polyMesh/boundary, where a value
            // breaks the mesh.
            if !k.starts_with(['$', '#']) {
                out.push(format!(
                    "{k} is a value in boundary there: changeDictionary adds it to the mesh's boundary file"
                ));
            }
        } else if !named.contains(k) {
            out.push(format!("patch {k} is changed there, the description doesn't map it"));
        }
    }
    out
}

#[cfg(test)]
#[path = "coupling_tests.rs"]
mod tests;
