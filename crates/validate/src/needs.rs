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
    for (r, path, bc) in solid_temperatures(case) {
        let word = |k: &str| bc.words.iter().find(|(x, _)| x == k).map_or("none", |(_, v)| v.as_str());
        let reads: Vec<&str> = ["qrNbr", "qsNbr"].into_iter().filter(|k| word(k) != "none").collect();
        if bc.pattern || reads.is_empty() {
            continue;
        }
        let Some(p) = r.patches.iter().flatten().find(|p| p.name == bc.patch) else { continue };
        // Coupled by group, or not mapped (bc.patch-type): no partner name to follow.
        let Some((_, twin)) = partner(p, &r.name, version) else { continue };
        if veg_patches.iter().any(|q| q.name == twin) || said.contains(&twin) {
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
