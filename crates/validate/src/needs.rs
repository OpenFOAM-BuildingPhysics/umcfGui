//! What an urbanMicroclimateFoam case needs from its regions and patches beyond what OpenFOAM
//! itself checks ([What a case needs](../../../docs/case-requirements.md)): the names the solver
//! looks regions and files up by, the canopy patch, the vegetation patches the solids read their
//! radiation from, sky and surface patches agreeing between `air` and `vegetation`, and the
//! coupled conditions on patches that can carry them. Most of these stop the run where the solver
//! looks; a sky or surface mix-up runs wrong without a word.

use umcf_core::OfVersion;
use umcf_foam_mesh::Patch;
use umcf_model::{Bc, Case, Region, RegionKind};
use umcf_schema::CaseKind;

use crate::fields::selected;
use crate::{Diagnostic, Level, partner};

/// The fixed names (`viewFactorSky.C`, `simplifiedVegetation.C`, the solid temperature
/// conditions, `calcLAI`).
const AIR: &str = "air";
const VEGETATION: &str = "vegetation";
const CANOPY: &str = "air_to_vegetation";

/// The solid temperature conditions that read the radiation from the vegetation region (when
/// there is one) and the outdoor tables from `0/air` (`CFDHAMsolidTemperatureCoupledMixed…C`,
/// `CFDHAMsolidTemperatureTransferCoeff…C`, both versions).
const SOLID_TEMPERATURE: [&str; 2] =
    ["compressible::CFDHAMsolidTemperatureCoupledMixed", "compressible::CFDHAMsolidTemperatureTransferCoeff"];

/// urbanMicroclimateFoam's conditions that stop on a patch that isn't mapped (`isA` on
/// `mappedFvPatchBaseBase` in OpenFOAM-12, `mappedPatchBase` in OpenFOAM-8): every `CFDHAM…`
/// condition but `CFDHAMsolidTemperatureTransferCoeffInterior`, in both versions.
const NEED_MAPPED: [&str; 8] = [
    "compressible::CFDHAMfluidMoistureCoupledImpermeable",
    "compressible::CFDHAMfluidMoistureCoupledMixed",
    "compressible::CFDHAMfluidTemperatureCoupledMixed",
    "compressible::CFDHAMsolidMoistureCoupledImpermeable",
    "compressible::CFDHAMsolidMoistureCoupledMixed",
    "compressible::CFDHAMsolidMoistureTransferCoeff",
    "compressible::CFDHAMsolidTemperatureCoupledMixed",
    "compressible::CFDHAMsolidTemperatureTransferCoeff",
];

pub(crate) fn check(case: &Case, out: &mut Vec<Diagnostic>) {
    // A meshing step or a single-region case is not run by the solver as it is.
    if case.layout.kind != CaseKind::Solver {
        return;
    }
    let version = case.layout.version;
    let of = |k: RegionKind| case.regions.iter().filter(move |r| r.kind == k);
    let fluids: Vec<&Region> = of(RegionKind::Fluid).collect();
    let vegetation: Vec<&Region> = of(RegionKind::Vegetation).collect();
    let listed = case.layout.regions_from.clone().unwrap_or_default();
    names(case, &fluids, &vegetation, &listed, out);
    ambient(case, out);
    materials(case, out);
    tables(case, out);
    view_factors(case, out);
    if version == Some(OfVersion::V8) {
        vegetation_listed(case, out);
    }
    coupled_types(case, version, out);
    let Some(veg) = case.region(VEGETATION).filter(|r| r.kind == RegionKind::Vegetation) else { return };
    solid_reads(case, veg, version, out);
    let [fluid] = fluids.as_slice() else { return };
    if version == Some(OfVersion::V8) {
        grass_reads(case, fluid, veg, out);
    }
    if selected(case, &fluid.name, "vegetationProperties", &["vegetationModel"]).as_deref()
        == Some("simplifiedVegetation")
    {
        canopy(veg, version, out);
    }
    if selected(case, VEGETATION, "radiationProperties", &["radiationModel"]).as_deref() == Some("viewFactorSky") {
        sky_surface(fluid, veg, version, out);
    }
}

/// The patch types the radiation takes as surfaces: those derived from `wallFvPatch`, and
/// `mappedInternal` in OpenFOAM-12 (`viewFactorSky.C`, `directAndDiffuse.C`; OpenFOAM-8 tests
/// `wall` only). Version unknown: either version's.
fn surface(p: &Patch, version: Option<OfVersion>) -> bool {
    const V12: [&str; 5] = ["wall", "mappedWall", "mappedExtrudedWall", "nonConformalMappedWall", "mappedInternal"];
    const V8: [&str; 4] = ["wall", "mappedWall", "regionCoupledWall", "mappedWallVariableThickness"];
    let k = p.kind.as_str();
    match version {
        Some(OfVersion::V8) => V8.contains(&k),
        Some(OfVersion::V12) => V12.contains(&k),
        None => V8.contains(&k) || V12.contains(&k),
    }
}

/// The patch types a coupled condition can sample through: `mappedFvPatchBaseBase` in
/// OpenFOAM-12, `mappedPatchBase` in OpenFOAM-8 (what the conditions' `isA` tests take).
fn mapped(version: Option<OfVersion>) -> &'static [&'static str] {
    match version {
        Some(OfVersion::V8) => &["mappedPatch", "mappedWall", "mappedWallVariableThickness"],
        Some(OfVersion::V12) => &["mapped", "mappedWall", "mappedExtrudedWall", "nonConformalMappedWall"],
        None => &[
            "mapped",
            "mappedPatch",
            "mappedWall",
            "mappedExtrudedWall",
            "nonConformalMappedWall",
            "mappedWallVariableThickness",
        ],
    }
}

/// One fluid region (urbanMicroclimateFoam's cases have `air`), named `air` when a region
/// solves radiation with `viewFactorSky` (it looks up the mesh `air`); one vegetation region,
/// named `vegetation`.
fn names(case: &Case, fluids: &[&Region], vegetation: &[&Region], file: &str, out: &mut Vec<Diagnostic>) {
    if fluids.len() != 1 {
        let listed: Vec<&str> = fluids.iter().map(|r| r.name.as_str()).collect();
        out.push(Diagnostic::new(
            Level::Warning,
            "case.fluid",
            file,
            if listed.is_empty() {
                "the case lists no fluid region: urbanMicroclimateFoam cases have one, `air`".to_string()
            } else {
                format!(
                    "the case lists {} fluid regions ({}): urbanMicroclimateFoam cases have one, `air`",
                    listed.len(),
                    listed.join(", ")
                )
            },
        ));
    }
    // Only fluid and vegetation regions build a radiation model.
    let sky = case.regions.iter().filter(|r| matches!(r.kind, RegionKind::Fluid | RegionKind::Vegetation)).find(|r| {
        selected(case, &r.name, "radiationProperties", &["radiationModel"]).as_deref() == Some("viewFactorSky")
    });
    if let ([fluid], Some(r)) = (fluids, sky)
        && fluid.name != AIR
    {
        out.push(
            Diagnostic::new(
                Level::Error,
                "case.air",
                file,
                format!(
                    "the fluid region is `{}`, and region `{}` solves radiation with viewFactorSky, which looks up the \
                     mesh named `air`: the run stops there",
                    fluid.name, r.name
                ),
            )
            .region(&fluid.name),
        );
    }
    for v in vegetation.iter().filter(|v| v.name != VEGETATION) {
        out.push(
            Diagnostic::new(
                Level::Error,
                "case.vegetation",
                file,
                format!(
                    "the vegetation region is `{}`: the solver finds it by the name `vegetation` (the solid temperature \
                     conditions, simplifiedVegetation, calcLAI)",
                    v.name
                ),
            )
            .region(&v.name),
        );
    }
    if vegetation.len() > 1 {
        let listed: Vec<&str> = vegetation.iter().map(|r| r.name.as_str()).collect();
        out.push(Diagnostic::new(
            Level::Error,
            "case.vegetation",
            file,
            format!(
                "the case lists {} vegetation regions ({}): only one is supported",
                listed.len(),
                listed.join(", ")
            ),
        ));
    }
}

/// The solid temperature conditions in use, as (region, field path, condition); not in copies
/// and backups (`Ts.orig`, `Ts~`), which OpenFOAM doesn't read.
fn solid_temperatures(case: &Case) -> impl Iterator<Item = (&Region, &str, &Bc)> {
    case.regions.iter().filter(|r| r.kind == RegionKind::Solid).flat_map(|r| {
        r.fields.iter().filter(|f| crate::is_field(f)).flat_map(move |f| {
            f.bcs
                .iter()
                .flatten()
                .filter(|b| b.kind.as_deref().is_some_and(|k| SOLID_TEMPERATURE.contains(&k)))
                .map(move |b| (r, f.path.as_str(), b))
        })
    })
}

/// Whether `file`, or its compressed `file.gz` (which OpenFOAM opens in its place), is there:
/// `None` when that can't be told.
fn there(case: &Case, file: &str) -> Option<bool> {
    match (case.exists(file), case.exists(&format!("{file}.gz"))) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// The solid temperature conditions take the rain's temperature from `0/air/rainTemp` when it is
/// there, else compute it from `$FOAM_CASE/0/air/Tambient` and `wambient`, read by path at every
/// update: the run stops without them. OpenFOAM-12's `CFDHAMsolidTemperatureCoupledMixed` looks
/// for rainTemp at the path `$FOAM_CASE/0/air/rainTemp` unexpanded (`isFile` doesn't expand it),
/// so it never finds it and always reads the two tables (the other conditions build the path from
/// the case's directory).
fn ambient(case: &Case, out: &mut Vec<Diagnostic>) {
    let rain = there(case, "0/air/rainTemp") == Some(true);
    // Only OpenFOAM-12's is known not to find it; a version not told takes it found.
    let blind = |b: &Bc| {
        case.layout.version == Some(OfVersion::V12)
            && b.kind.as_deref() == Some("compressible::CFDHAMsolidTemperatureCoupledMixed")
    };
    let Some((r, path, bc)) = solid_temperatures(case).find(|(_, _, b)| !rain || blind(b)) else { return };
    for table in ["Tambient", "wambient"] {
        let file = format!("0/air/{table}");
        if there(case, &file) == Some(false) {
            out.push(
                Diagnostic::new(
                    Level::Error,
                    "case.ambient",
                    &file,
                    format!(
                        "{file} is not there: `{}` ({path}, patch `{}`) reads it by that path for the rain's \
                         temperature when there is no 0/air/rainTemp, and the run stops",
                        bc.kind.as_deref().unwrap_or_default(),
                        bc.patch
                    ),
                )
                .region(&r.name),
            );
        }
    }
}

/// The vegetation patches the solids read their radiation from: a solid temperature condition
/// with `qrNbr` or `qsNbr` reads it from the vegetation patch named like its partner in the air.
/// (Both versions also index the vegetation boundary with that patch when neither is set, but
/// nothing is read through it then: an optimised build runs.)
fn solid_reads(case: &Case, veg: &Region, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let Some(veg_patches) = &veg.patches else { return };
    let mut said: Vec<String> = Vec::new();
    let mut said_fields: Vec<&str> = Vec::new();
    for (r, path, bc) in solid_temperatures(case) {
        let word = |k: &str| bc.words.iter().find(|(x, _)| x == k).map_or("none", |(_, v)| v.as_str());
        let reads: Vec<&str> = ["qrNbr", "qsNbr"].into_iter().filter(|k| word(k) != "none").collect();
        if bc.pattern || reads.is_empty() {
            continue;
        }
        let Some(p) = r.patches.iter().flatten().find(|p| p.name == bc.patch) else { continue };
        // Coupled by group, or not mapped (bc.patch-type): no partner name to follow.
        let Some((_, twin)) = partner(p, &r.name, version) else { continue };
        if said.contains(&twin) {
            continue;
        }
        if veg_patches.iter().any(|q| q.name == twin) {
            // The field each names, in the vegetation region (whose fields are listed).
            for k in &reads {
                let field = word(k);
                if !veg.fields.is_empty() && veg.field(field).is_none() && !said_fields.contains(&field) {
                    said_fields.push(field);
                    out.push(
                        Diagnostic::new(
                            Level::Error,
                            "case.vegetation-field",
                            path,
                            format!(
                                "`{}` on `{}` reads `{field}` ({k}) from the vegetation region, which has no field \
                                 `{field}`: the run stops",
                                bc.kind.as_deref().unwrap_or_default(),
                                bc.patch
                            ),
                        )
                        .region(&r.name)
                        .patch(&bc.patch),
                    );
                }
            }
            continue;
        }
        out.push(
            Diagnostic::new(
                Level::Error,
                "case.vegetation-patch",
                path,
                format!(
                    "`{}` on `{}` reads {} from the vegetation patch named like its partner `{twin}`, and the vegetation \
                     region has no such patch: the run fails there",
                    bc.kind.as_deref().unwrap_or_default(),
                    bc.patch,
                    reads.join(" and ")
                ),
            )
            .region(&r.name)
            .patch(&bc.patch),
        );
        said.push(twin);
    }
}

/// OpenFOAM-8's `simpleGrass` reads `qr` and `qs` from the vegetation patch named like each grass
/// patch of the fluid (`simpleGrass.C`, of-org_v8.0; OpenFOAM-12 falls back to the air's).
fn grass_reads(case: &Case, fluid: &Region, veg: &Region, out: &mut Vec<Diagnostic>) {
    let Some(veg_patches) = &veg.patches else { return };
    let file = format!("constant/{}/grassProperties", fluid.name);
    let Some(doc) = case.dict(&file) else { return };
    if selected(case, &fluid.name, "grassProperties", &["grassModel"]).as_deref() != Some("simpleGrass") {
        return;
    }
    let Some(list) = doc.value(&["simpleGrassCoeffs", "grassPatches"]) else { return };
    // `( a b )` or `2(a b)`; a `$macro` or anything else is not followed.
    let inner = list.trim().trim_start_matches(|c: char| c.is_ascii_digit()).trim();
    let Some(inner) = inner.strip_prefix('(').and_then(|x| x.strip_suffix(')')) else { return };
    for name in inner.split_whitespace().map(umcf_foam_dict::unquote) {
        if name.starts_with('$') || veg_patches.iter().any(|q| q.name == name) {
            continue;
        }
        out.push(
            Diagnostic::new(
                Level::Error,
                "case.vegetation-patch",
                &file,
                format!(
                    "grass patch `{name}`: OpenFOAM-8's simpleGrass reads qr and qs from the vegetation patch of the \
                     same name, and the vegetation region has none: the run fails there"
                ),
            )
            .region(&fluid.name)
            .patch(name),
        );
    }
}

/// A solid region's materials against its cell zones, as the solver assigns them
/// (`solid/updatebuildingMaterials.H`, both versions): each material's `name` is a cell zone,
/// no cell is in two materials' zones (a material listed twice counts twice), and every cell is
/// in one. Zones no material names don't count. The run stops on each.
fn materials(case: &Case, out: &mut Vec<Diagnostic>) {
    'regions: for r in case.regions.iter().filter(|r| r.kind == RegionKind::Solid) {
        let Some(zones) = &r.cell_zones else { continue };
        let file = format!("constant/{}/transportProperties", r.name);
        let Some(list) = case.dict(&file).and_then(|d| d.get(&["buildingMaterials"])) else { continue };
        // The materials' names in order. A list given otherwise (`$mats`, `#include`) or a name
        // given otherwise (`$m`, `#calc`) can't be judged: then only the plain names are, and
        // only for their zones.
        let mut names: Vec<String> = Vec::new();
        let items = match list.value.as_slice() {
            [umcf_foam_dict::Node::List { items, .. }] => items.as_slice(),
            _ => continue,
        };
        let mut judged = true;
        for item in items {
            let umcf_foam_dict::Node::Dict { body, .. } = item else {
                judged = false;
                continue;
            };
            let plain = |v: &str| !v.is_empty() && !v.starts_with('$') && !v.starts_with('#');
            match body.get("name").and_then(umcf_foam_dict::Entry::value_text) {
                Some(v) if plain(v.trim()) => names.push(umcf_foam_dict::unquote(v.trim()).to_string()),
                _ => judged = false,
            }
        }
        let diag = |code, message: String| Diagnostic::new(Level::Error, code, &file, message).region(&r.name);
        // The first zone of a name, as OpenFOAM finds it (a map: the files may name very many).
        let mut first_zone: std::collections::HashMap<&str, &umcf_foam_mesh::Zone> = Default::default();
        for z in zones.iter() {
            first_zone.entry(z.name.as_str()).or_insert(z);
        }
        let zone = |name: &str| first_zone.get(name).copied();
        let missing: Vec<&str> = names.iter().map(String::as_str).filter(|n| zone(n).is_none()).collect();
        for m in &missing {
            out.push(diag(
                "case.material-zone",
                format!(
                    "material `{m}` has no cell zone of that name in region `{}`: the solver assigns each material \
                     to the cells of its zone, and stops",
                    r.name
                ),
            ));
        }
        // Each label of the materials' zones with when the solver reaches it (the materials in
        // order, each zone's labels in order): 8 bytes a label, as much as the zones hold, not a
        // cell count from a header. Sorted by cell, a cell met twice is two neighbours. The solver
        // goes no further than a material without a zone (it stops there) or one whose zone it
        // has already assigned (it stops at that zone's first cell): neither do these.
        let mut met: Vec<(u32, u32)> = Vec::new();
        // Where each material's labels start, by when, to tell whose a label is.
        let mut starts: Vec<(u32, usize)> = Vec::new();
        let mut repeat = None;
        // The materials gathered so far, by name: the first index of each.
        let mut seen: std::collections::HashMap<&str, usize> = Default::default();
        for (i, name) in names.iter().enumerate() {
            let Some(z) = zone(name) else { break };
            let Ok(at) = u32::try_from(met.len()) else { continue 'regions };
            if let Some(&j) = seen.get(name.as_str()) {
                if let Some(&c) = z.cells.first() {
                    repeat = Some((c, at, j, i));
                    break;
                }
                continue;
            }
            if met.try_reserve(z.cells.len()).is_err() || u32::try_from(met.len() + z.cells.len()).is_err() {
                continue 'regions;
            }
            starts.push((at, i));
            seen.insert(name.as_str(), i);
            for (k, &c) in z.cells.iter().enumerate() {
                met.push((c, at + k as u32));
            }
        }
        let whose =
            |t: u32| starts.get(starts.partition_point(|(s, _)| *s <= t).wrapping_sub(1)).map_or(0, |(_, i)| *i);
        met.sort_unstable();
        let n = r.counts.cells;
        // Where the solver stops first: a cell past the region's (it indexes past them), a cell
        // met a second time, or a material's zone assigned again.
        let past = n.and_then(|n| met.iter().filter(|(c, _)| u64::from(*c) >= n).min_by_key(|(_, t)| *t).copied());
        let again = met
            .windows(2)
            .filter_map(|w| match *w {
                [a, b] if a.0 == b.0 => Some((a, b)),
                _ => None,
            })
            .min_by_key(|(_, b)| b.1);
        let mut events: Vec<(u32, (&'static str, String))> = Vec::new();
        if let Some((c, t)) = past {
            let name = names.get(whose(t)).map_or("?", String::as_str);
            let cells = n.unwrap_or_default();
            events.push((
                t,
                (
                    "case.material-zone",
                    format!(
                        "zone `{name}` lists cell {c}, and region `{}` has {cells} cells: the run fails there",
                        r.name
                    ),
                ),
            ));
        }
        if let Some(((_, ta), (c, tb))) = again {
            events.push((tb, overlap(&names, c, whose(ta), whose(tb))));
        }
        if let Some((c, t, j, i)) = repeat {
            events.push((t, overlap(&names, c, j, i)));
        }
        let stop = events.into_iter().min_by_key(|(t, _)| *t).map(|(_, e)| e);
        if let Some((code, message)) = stop {
            out.push(diag(code, message));
            continue;
        }
        if !missing.is_empty() || !judged || repeat.is_some() {
            continue;
        }
        // Every cell once: as many distinct cells as the region has (none past them, none twice).
        let Some(n) = n else { continue };
        let covered = met.len() as u64;
        if covered < n {
            // The first cell no zone lists.
            let mut first = 0u64;
            for (c, ..) in &met {
                if u64::from(*c) != first {
                    break;
                }
                first += 1;
            }
            out.push(diag(
                "case.material-cover",
                format!(
                    "{} of the {n} cells of region `{}` are in no material's zone (cell {first} first): the solver \
                     stops unless every cell has a material",
                    n - covered,
                    r.name
                ),
            ));
        }
    }
}

/// The message for cell `c` the solver meets a second time, first for material `first` (by its
/// index in `names`), then for `second`.
fn overlap(names: &[String], c: u32, first: usize, second: usize) -> (&'static str, String) {
    let name = |i: usize| names.get(i).map_or("?", String::as_str);
    let message = if first == second {
        format!("zone `{}` lists cell {c} twice: the solver stops on a cell assigned twice", name(first))
    } else if name(first) == name(second) {
        format!(
            "material `{}` is listed twice, so its zone's cells (cell {c} first) get two materials: the solver stops",
            name(first)
        )
    } else {
        format!(
            "cell {c} is in the zones of materials `{}` and `{}`: the solver stops on a cell with two materials",
            name(first),
            name(second)
        )
    };
    ("case.material-overlap", message)
}

/// The fluid and vegetation regions that solve radiation with `radiationModel`'s `model`, or
/// `vegetationModel`/`solarLoadModel` of that name (only those build them).
fn selecting<'a>(
    case: &'a Case,
    file: &'static str,
    key: &'static str,
    model: &'a str,
) -> impl Iterator<Item = &'a Region> {
    case.regions
        .iter()
        .filter(|r| matches!(r.kind, RegionKind::Fluid | RegionKind::Vegetation))
        .filter(move |r| selected(case, &r.name, file, &[key]).as_deref() == Some(model))
}

/// The data tables the solver reads by file name, for the readers in use: there, and covering
/// the run. Past a table's ends OpenFOAM holds its first or last value (`outOfBounds clamp`, the
/// default in both versions), so a table that ends early freezes the sun or the weather.
fn tables(case: &Case, out: &mut Vec<Diagnostic>) {
    let version = case.layout.version;
    let found = |name: &str| case.tables.iter().find(|t| t.path == name || t.path == format!("{name}.gz"));
    // Each table in use, and by whom.
    let mut used: Vec<(&str, String)> = Vec::new();
    if let Some(r) = selecting(case, "radiationProperties", "radiationModel", "viewFactorSky").next() {
        let by = format!("viewFactorSky (region `{}`)", r.name);
        // OpenFOAM-12 reads constant/LW when it is there; OpenFOAM-8 only Tambient.
        if version != Some(OfVersion::V8) && found("constant/LW").is_some() {
            used.push(("constant/LW", by));
        } else {
            used.push(("0/air/Tambient", format!("{by}, for the sky's temperature")));
            if found("0/air/cloudCover").is_some() {
                if version == Some(OfVersion::V8) {
                    used.push(("0/air/cloudCover", by));
                } else if version == Some(OfVersion::V12) {
                    out.push(Diagnostic::new(
                        Level::Info,
                        "case.cloud-cover",
                        "0/air/cloudCover",
                        "OpenFOAM-12's viewFactorSky looks for 0/air/cloudCover at `$FOAM_CASE/0/air/cloudCover` \
                         unexpanded, never finds it, and takes a cloud cover of 0",
                    ));
                }
            }
        }
    }
    let sun = selecting(case, "solarLoadProperties", "solarLoadModel", "directAndDiffuse")
        .next()
        .map(|r| format!("directAndDiffuse (region `{}`)", r.name))
        .or_else(|| {
            selecting(case, "vegetationProperties", "vegetationModel", "simplifiedVegetation")
                .next()
                .map(|r| format!("simplifiedVegetation (region `{}`)", r.name))
        });
    if let Some(by) = sun {
        used.push(("constant/sunPosVector", by));
    }
    // The solid temperature conditions: rainTemp when it is there, else Tambient and wambient;
    // OpenFOAM-12's CoupledMixed never finds rainTemp (`$FOAM_CASE` unexpanded) and always reads
    // the two. Missing ones are case.ambient's.
    let rain = found("0/air/rainTemp").is_some();
    let mut kinds: Vec<&str> = solid_temperatures(case).filter_map(|(_, _, b)| b.kind.as_deref()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for kind in kinds {
        let by = format!("`{kind}`");
        let blind = version == Some(OfVersion::V12) && kind == "compressible::CFDHAMsolidTemperatureCoupledMixed";
        if rain && !blind {
            used.push(("0/air/rainTemp", by));
        } else {
            used.push(("0/air/Tambient", by.clone()));
            used.push(("0/air/wambient", by.clone()));
            if rain {
                out.push(Diagnostic::new(
                    Level::Info,
                    "case.rain-temp",
                    "0/air/rainTemp",
                    format!(
                        "OpenFOAM-12's {by} looks for 0/air/rainTemp at `$FOAM_CASE/0/air/rainTemp` unexpanded, never \
                         finds it, and takes the rain's temperature from Tambient and wambient"
                    ),
                ));
            }
        }
    }
    // Whether a solid condition reads Tambient and wambient (then case.ambient says when one is
    // missing).
    let solid_ambient = solid_temperatures(case).any(|(_, _, b)| {
        let blind = version == Some(OfVersion::V12)
            && b.kind.as_deref() == Some("compressible::CFDHAMsolidTemperatureCoupledMixed");
        !rain || blind
    });
    let solid_reads_ambient = |name: &str| matches!(name, "0/air/Tambient" | "0/air/wambient") && solid_ambient;
    // The run's window: from the time it starts at to endTime.
    let c = &case.controls;
    let n = |v: &Option<String>| v.as_deref().and_then(|v| v.trim().parse::<f64>().ok()).filter(|x| x.is_finite());
    let start = case.start.as_ref().map(|t| t.value);
    let end = if c.stop_at.as_deref().is_none_or(|s| s == "endTime") { n(&c.end_time) } else { None };
    let mut said: Vec<&str> = Vec::new();
    for (name, by) in &used {
        if said.contains(name) {
            continue;
        }
        said.push(name);
        let Some(t) = found(name) else {
            // Tambient and wambient missing for the solid conditions are case.ambient's.
            if !solid_reads_ambient(name) {
                out.push(Diagnostic::new(
                    Level::Error,
                    "case.table",
                    *name,
                    format!("{by} reads {name}, which is not there: the run stops"),
                ));
            }
            continue;
        };
        match &t.span {
            Err(e) if e.fatal => out.push(Diagnostic::new(
                Level::Error,
                "case.table",
                &t.path,
                format!("{by} reads {}: {}", t.path, e.message),
            )),
            Err(e) => out.push(Diagnostic::new(
                Level::Warning,
                "case.table",
                &t.path,
                format!("{by} reads {}, which can't be read here ({}): its times are not checked", t.path, e.message),
            )),
            Ok(span) => {
                let early = start.filter(|s| *s < span.first);
                let late = end.filter(|e| *e > span.last);
                if early.is_some() || late.is_some() {
                    let (s, e) = (start.unwrap_or(span.first), end.unwrap_or(span.last));
                    out.push(Diagnostic::new(
                        Level::Warning,
                        "case.table-span",
                        &t.path,
                        format!(
                            "{} covers {} to {} s, and the run goes from {s} to {e} s: {}, read by {by}",
                            t.path,
                            span.first,
                            span.last,
                            [
                                early.map(|_| "before the start of it, OpenFOAM holds its first value"),
                                late.map(|_| "past the end of it, OpenFOAM holds its last value")
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(", and ")
                        ),
                    ));
                }
            }
        }
    }
}

/// The scalar condition types derived, directly or not, from `fixedValueFvPatchField` in
/// OpenFOAM-8 and 12 whose names don't end in `FixedValue` (those that do, `uniformFixedValue`,
/// `codedFixedValue`, `timeVaryingMappedFixedValue`, …, are taken by their name), with
/// viewFactorSky's own: the wall functions, the total pressure chain, the mapped values, ….
const FIXED_VALUE: [&str; 56] = [
    "fixedValue",
    "greyDiffusiveRadiationViewFactor",
    "alphaFixedPressure",
    "alphatFilmWallFunction",
    "cellMotion",
    "compressible::alphatJayatillekeWallFunction",
    "compressible::alphatWallFunction",
    "convectiveHeatTransfer",
    "dynamicPressure",
    "epsilonWallFunction",
    "fixedEnergy",
    "fixedMean",
    "fixedPressureCompressibleDensity",
    "fixedProfile",
    "fixedUnburntEnthalpy",
    "fixedValueInletOutlet",
    "fWallFunction",
    "inclinedFilmNusseltHeight",
    "interfaceCompression",
    "kLowReWallFunction",
    "lumpedMassTemperature",
    "mapped",
    "mappedField",
    "mappedInternalValue",
    "mappedValue",
    "nutWallFunction",
    "omegaWallFunction",
    "outletMappedUniformInlet",
    "plenumPressure",
    "pressure",
    "prghTotalHydrostaticPressure",
    "syringePressure",
    "totalTemperature",
    "turbulentInlet",
    "uniformDensityHydrostaticPressure",
    "uniformTotalPressure",
    "waveSurfacePressure",
    "zeroFixedValue",
    "v2WallFunction",
    "nutkWallFunction",
    "nutkRoughWallFunction",
    "nutkAtmRoughWallFunction",
    "nutkFilmWallFunction",
    "nutLowReWallFunction",
    "nutUWallFunction",
    "nutURoughWallFunction",
    "nutUSpaldingWallFunction",
    "nutTabulatedWallFunction",
    "epsilonmWallFunction",
    "totalPressure",
    "rotatingTotalPressure",
    "fanPressure",
    "entrainmentPressure",
    "mappedFixedInternalValue",
    "waveAlpha",
    "specieTransferVelocity",
];

/// The patches the radiation exchanges through without an entry in the `viewFactorsDict`
/// `faceAgglomerate` reads: it leaves them unagglomerated, a coarse face for each face, so
/// `viewFactorsGen` and the run are slow and big. The radiating patches are those whose `qr`
/// condition in the region solving viewFactorSky is a fixed value (viewFactorSky.C,
/// viewFactorsGen.C); viewFactorSky takes them as `greyDiffusiveRadiationViewFactor` and stops
/// on another (`case.qr-type`). The dictionary is the one the case's `faceAgglomerate` reads
/// (its `-dict`, else `system/` in OpenFOAM-12, `constant/<region>/` in OpenFOAM-8); its keys
/// are names or quoted patterns.
fn view_factors(case: &Case, out: &mut Vec<Diagnostic>) {
    let Some(r) = selecting(case, "radiationProperties", "radiationModel", "viewFactorSky").next() else { return };
    let Some(qr) = r.field("qr") else { return };
    let Some(patches) = &r.patches else { return };
    // Each patch's qr condition as OpenFOAM picks its entry; entries that can't all be read here
    // leave the check out.
    let Some(kinds) = crate::kinds_by_patch(case, r, qr) else { return };
    // A fixed value: viewFactorSky selects qr conditions by `isA<fixedValueFvPatchScalarField>`.
    let fixed = |k: &str| FIXED_VALUE.contains(&k) || k.ends_with("ixedValue");
    let fixed_patches: Vec<(&str, &str)> =
        kinds.iter().filter_map(|(p, k)| k.filter(|k| fixed(k)).map(|k| (*p, k))).collect();
    // A fixed value other than viewFactorSky's own: it stops on it, faces or not.
    let other: Vec<String> = fixed_patches
        .iter()
        .filter(|(_, k)| *k != "greyDiffusiveRadiationViewFactor")
        .map(|(p, k)| format!("`{p}` ({k})"))
        .collect();
    if !other.is_empty() {
        let shown: Vec<&str> = other.iter().take(10).map(String::as_str).collect();
        out.push(
            Diagnostic::new(
                Level::Error,
                "case.qr-type",
                &qr.path,
                format!(
                    "`qr` is a fixed value other than greyDiffusiveRadiationViewFactor on {} patch{} ({}{}): \
                     viewFactorSky takes every fixed-value qr as its own condition and stops",
                    other.len(),
                    if other.len() == 1 { "" } else { "es" },
                    shown.join(", "),
                    if other.len() > shown.len() { ", …" } else { "" }
                ),
            )
            .region(&r.name)
            .field("qr"),
        );
    }
    // The radiating patches with faces.
    let radiating: Vec<&str> = fixed_patches
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| patches.iter().any(|q| q.name == *p && q.n_faces > 0))
        .collect();
    // The dictionary faceAgglomerate reads for this region.
    let runs: Vec<&umcf_model::script::Run> = case
        .scripts
        .iter()
        .flat_map(|s| &s.steps)
        .filter_map(|st| match &st.kind {
            umcf_model::script::StepKind::Run(run) if run.app == "faceAgglomerate" => Some(run),
            _ => None,
        })
        .collect();
    // A run over all regions reads each region's own dictionary in a way not followed here
    // (as validate::workflow leaves it): not judged.
    if runs.iter().any(|run| run.all_regions) {
        return;
    }
    let runs: Vec<&umcf_model::script::Run> =
        runs.into_iter().filter(|run| run.region.as_deref().unwrap_or("") == r.name).collect();
    let default = umcf_model::script::Run {
        app: "faceAgglomerate".to_string(),
        args: Vec::new(),
        region: (!r.name.is_empty()).then(|| r.name.clone()),
        all_regions: false,
        parallel: false,
        dict: None,
        np: None,
        run_parallel: false,
    };
    let versions = match case.layout.version {
        Some(v) => vec![v],
        None => OfVersion::ALL.to_vec(),
    };
    let is_dir = |p: &str| case.exists(p) == Some(true) && case.dict(p).is_none();
    let mut candidates: Vec<String> = Vec::new();
    for run in runs.iter().copied().chain(runs.is_empty().then_some(&default)) {
        for v in &versions {
            candidates.extend(crate::workflow::dictionary(run, *v, is_dir));
        }
    }
    let Some((path, doc)) = candidates.iter().find_map(|p| case.dict(p).map(|d| (p, d))) else { return };
    // The dictionary's patch entries: a name, or a quoted pattern matched whole (one that can't
    // be read here leaves the check out).
    let mut names: Vec<&str> = Vec::new();
    let mut patterns: Vec<regex::Regex> = Vec::new();
    let mut cx = crate::Patterns::default();
    for e in doc.root.entries().filter(|e| e.dict().is_some()) {
        let key = e.key.text.as_str();
        if key.starts_with('"') {
            // Read as OpenFOAM's POSIX matcher reads it (as the boundary conditions' patterns).
            match cx.get(umcf_foam_dict::unquote(key)) {
                Ok(re) => patterns.push(re.clone()),
                Err(why) => {
                    out.push(
                        Diagnostic::new(
                            Level::Warning,
                            "case.view-factors",
                            path,
                            format!("`{key}` {why}: which patches have an entry isn't checked"),
                        )
                        .region(&r.name),
                    );
                    return;
                }
            }
        } else {
            names.push(key);
        }
    }
    let listed = |p: &str| names.contains(&p) || patterns.iter().any(|re| re.is_match(p));
    let missing: Vec<&str> = radiating.iter().copied().filter(|p| !listed(p)).collect();
    if !missing.is_empty() {
        let shown: Vec<&str> = missing.iter().take(10).copied().collect();
        out.push(
            Diagnostic::new(
                Level::Warning,
                "case.view-factors",
                path,
                format!(
                    "{} radiating patch{} of region `{}` ({}{}) {} no entry here: faceAgglomerate leaves {} \
                     unagglomerated, every face its own, which makes viewFactorsGen and the run slow and big",
                    missing.len(),
                    if missing.len() == 1 { "" } else { "es" },
                    r.name,
                    shown.join(", "),
                    if missing.len() > shown.len() { ", …" } else { "" },
                    if missing.len() == 1 { "has" } else { "have" },
                    if missing.len() == 1 { "it" } else { "them" }
                ),
            )
            .region(&r.name),
        );
    }
}

/// OpenFOAM-8's urbanMicroclimateFoam looks `fluid`, `solid` and `vegetation` up in
/// `regionProperties`' regions (`rp["fluid"]` in fluid/createFluidMeshes.H, `rp["solid"]`,
/// `rp["vegetation"]` in veg/createVegMeshes.H) and stops when one isn't there, even with no
/// trees or no solids.
fn vegetation_listed(case: &Case, out: &mut Vec<Diagnostic>) {
    const FILE: &str = "constant/regionProperties";
    if case.layout.regions_from.as_deref() != Some(FILE) {
        return;
    }
    let Some(list) = case.dict(FILE).and_then(|d| d.get(&["regions"])) else { return };
    let [umcf_foam_dict::Node::List { items, .. }] = list.value.as_slice() else { return };
    // Words at the list's top level: `fluid (air) solid (…) vegetation (…)`; a macro or a
    // directive there can't be judged.
    let words: Vec<&str> = items
        .iter()
        .filter_map(|n| match n {
            umcf_foam_dict::Node::Atom(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    if words.iter().any(|w| w.starts_with('$') || w.starts_with('#')) {
        return;
    }
    for kind in ["fluid", "solid", "vegetation"] {
        if !words.contains(&kind) {
            out.push(Diagnostic::new(
                Level::Error,
                "case.regions-entry",
                FILE,
                format!(
                    "the regions have no `{kind}` entry: OpenFOAM-8's urbanMicroclimateFoam looks it up and stops \
                     without it, even when there are none (add `{kind} ()`)"
                ),
            ));
        }
    }
}

/// The patches by name, the first of a name kept (OpenFOAM stops on a name given twice).
fn first_by_name(patches: &[Patch]) -> std::collections::HashMap<&str, &Patch> {
    let mut map = std::collections::HashMap::new();
    for p in patches {
        map.entry(p.name.as_str()).or_insert(p);
    }
    map
}

fn boundary(r: &Region) -> String {
    format!("{}/boundary", umcf_foam_mesh::mesh_dir(&r.name))
}

/// The canopy surface, which simplifiedVegetation finds by name: there, and a surface for the
/// radiation (`mappedInternal` in OpenFOAM-12, `mappedWall` in OpenFOAM-8).
fn canopy(veg: &Region, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let Some(patches) = &veg.patches else { return };
    let diag = |message: String| Diagnostic::new(Level::Error, "case.canopy", boundary(veg), message).region(&veg.name);
    match patches.iter().find(|p| p.name == CANOPY) {
        None => out.push(diag(format!(
            "the vegetation region has no patch `{CANOPY}`: simplifiedVegetation finds the trees' surface by that \
             name, and the run fails there"
        ))),
        Some(p) if !surface(p, version) => out.push(
            diag(format!(
                "`{CANOPY}` is `{}`, which the radiation takes as sky, not the trees' surface: it is mappedInternal \
                 (OpenFOAM-12) or mappedWall (OpenFOAM-8)",
                p.kind
            ))
            .patch(CANOPY),
        ),
        Some(_) => {}
    }
}

/// With radiation solved in the vegetation region, the patches named alike in the fluid and
/// there should agree on sky or surface: the ground and the buildings are surfaces, the
/// domain's sides and top sky. A mismatch means one of the two types is likely wrong; only the
/// vegetation region's counts for the radiation.
fn sky_surface(fluid: &Region, veg: &Region, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let (Some(fluid_patches), Some(veg_patches)) = (&fluid.patches, &veg.patches) else { return };
    let by_name = first_by_name(veg_patches);
    for p in fluid_patches {
        let Some(q) = by_name.get(p.name.as_str()).filter(|q| q.name != CANOPY) else { continue };
        let (a, b) = (surface(p, version), surface(q, version));
        if a == b {
            continue;
        }
        let what = |s: bool| if s { "a surface" } else { "sky" };
        out.push(
            Diagnostic::new(
                Level::Warning,
                "case.sky-surface",
                boundary(veg),
                format!(
                    "`{}` is {} in `{}` (`{}`) but {} in the vegetation region (`{}`), where the radiation is solved \
                     and takes it as {}: the ground and buildings are surfaces (wall types), the domain's sides and top \
                     sky; one of the two types is likely wrong",
                    p.name,
                    what(a),
                    fluid.name,
                    p.kind,
                    what(b),
                    q.kind,
                    what(b)
                ),
            )
            .region(&veg.name)
            .patch(&q.name),
        );
    }
}

/// The coupled conditions on patches that can carry them: urbanMicroclimateFoam's coupled
/// conditions (all `CFDHAM…` but `…TransferCoeffInterior`) need a mapped patch,
/// `mappedLeafTemperature` a `mappedInternal` one in OpenFOAM-12 (a mapped one in OpenFOAM-8).
/// The run stops where the condition is built.
fn coupled_types(case: &Case, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let mapped_types = mapped(version);
    for r in &case.regions {
        let Some(patches) = &r.patches else { continue };
        let by_name = first_by_name(patches);
        // Not copies and backups, which OpenFOAM doesn't read.
        for f in r.fields.iter().filter(|f| crate::is_field(f)) {
            for bc in f.bcs.iter().flatten().filter(|b| !b.pattern) {
                let (Some(kind), Some(p)) = (bc.kind.as_deref(), by_name.get(bc.patch.as_str())) else {
                    continue;
                };
                let on_mapped = mapped_types.contains(&p.kind.as_str());
                let need = if NEED_MAPPED.contains(&kind) {
                    (!on_mapped).then(|| format!("a mapped patch ({})", mapped_types.join(", ")))
                } else if kind == "mappedLeafTemperature" {
                    match version {
                        Some(OfVersion::V8) => {
                            (!on_mapped).then(|| format!("a mapped patch ({})", mapped_types.join(", ")))
                        }
                        Some(OfVersion::V12) => {
                            (p.kind != "mappedInternal").then(|| "a mappedInternal patch".to_string())
                        }
                        None => (!on_mapped && p.kind != "mappedInternal")
                            .then(|| "a mappedInternal (OpenFOAM-12) or mapped (OpenFOAM-8) patch".to_string()),
                    }
                } else {
                    None
                };
                if let Some(need) = need {
                    out.push(
                        Diagnostic::new(
                            Level::Error,
                            "bc.patch-type",
                            &f.path,
                            format!("`{kind}` needs {need}, and `{}` is `{}`: the run stops", p.name, p.kind),
                        )
                        .region(&r.name)
                        .patch(&p.name)
                        .field(&f.name),
                    );
                }
            }
        }
    }
}
