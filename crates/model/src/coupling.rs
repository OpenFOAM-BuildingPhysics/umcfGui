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
/// region the description doesn't have, a canopy with no air to map to).
pub fn entries(d: &Description, region: &str, version: OfVersion) -> Result<Vec<Entry>, String> {
    let Some(r) = d.region(region) else {
        return Err(format!("region {region} isn't in the description"));
    };
    let v8 = version == OfVersion::V8;
    let mut out = Vec::new();
    for p in &r.patches {
        if p.role == Some(Role::Canopy) && p.partner.is_none() {
            let air = canopy_air(d, r)?;
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

/// How a `changeDictionaryDict`'s `boundary` (`root`, the file's top level) differs from
/// `entries` in what matters to the coupling: each entry's patch there, its type, the region and
/// patch it maps to (`neighbourRegion`/`neighbourPatch`, or `sampleRegion`/`samplePatch`, which
/// OpenFOAM-12 reads too; no patch for one mapped to the air's cells, which reads none), and the
/// canopy's `sampleMode`; and the patches it changes that `entries` doesn't map. `method` and
/// the offsets aren't compared. A key that is a pattern (`"air_to_.*"`) or a group isn't
/// matched to patches: a patch mapped only through one is said not to be there.
pub fn differences(root: &umcf_foam_dict::Dict, entries: &[Entry]) -> Vec<String> {
    let Some(boundary) = root.get("boundary").and_then(umcf_foam_dict::Entry::dict) else {
        return vec!["it has no boundary dictionary".to_string()];
    };
    let mut out = Vec::new();
    let value = |d: &umcf_foam_dict::Dict, k: &str| {
        d.get(k).and_then(umcf_foam_dict::Entry::value_text).map(|v| v.trim_matches('"').to_string())
    };
    for e in entries {
        let Some(there) = boundary.get(&e.patch).and_then(umcf_foam_dict::Entry::dict) else {
            out.push(format!("patch {} isn't there", e.patch));
            continue;
        };
        let internal = e.get("type") == Some("mappedInternal");
        let compared = [
            ("type", e.get("type").map(str::to_string), value(there, "type")),
            (
                "the region it maps to",
                e.get("neighbourRegion").or(e.get("sampleRegion")).map(str::to_string),
                value(there, "neighbourRegion").or_else(|| value(there, "sampleRegion")),
            ),
            (
                "the patch it maps to",
                e.get("neighbourPatch").or(e.get("samplePatch")).map(str::to_string),
                value(there, "neighbourPatch").or_else(|| value(there, "samplePatch")),
            ),
            ("sampleMode", e.get("sampleMode").map(str::to_string), value(there, "sampleMode")),
        ];
        for (what, want, have) in compared {
            // A patch mapped to the air's cells reads no patch of it (OpenFOAM-12's tutorials
            // name one all the same).
            if what == "the patch it maps to" && internal {
                continue;
            }
            // The sample mode of a mapped patch is written as the reference cases do; only the
            // canopy's (the air's cells, not a patch) matters.
            if what == "sampleMode" && want.as_deref() != Some("nearestCell") && have.as_deref() != Some("nearestCell")
            {
                continue;
            }
            if want != have {
                out.push(format!(
                    "patch {}: {what} is {} there, the description makes it {}",
                    e.patch,
                    have.as_deref().unwrap_or("not given"),
                    want.as_deref().unwrap_or("not given")
                ));
            }
        }
    }
    for entry in boundary.entries() {
        // A pattern is quoted (`"air_to_.*"`); a patch's name never is.
        if entry.key.text.starts_with('"') {
            continue;
        }
        let k = entry.key();
        if !entries.iter().any(|e| e.patch == k) {
            out.push(format!("patch {k} is changed there, the description doesn't map it"));
        }
    }
    out
}

#[cfg(test)]
#[path = "coupling_tests.rs"]
mod tests;
