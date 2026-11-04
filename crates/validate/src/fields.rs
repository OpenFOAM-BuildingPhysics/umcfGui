//! The fields each region needs: what the solver reads for its kind of region, what the models
//! selected there read (both from the solver catalog), and what OpenFOAM itself reads for the
//! thermophysics and turbulence the solver sets up (the table below).

use umcf_core::OfVersion;
use umcf_model::{Case, Region, RegionKind};
use umcf_schema::CaseKind;
use umcf_schema::catalog::{Catalog, FieldInstance, FieldOwner, catalog};

use crate::{Diagnostic, Level, decomposed, region_dir};

/// A field OpenFOAM reads (`MUST_READ`) for something the solver builds, with the source file
/// in OpenFOAM-8 and OpenFOAM-12. Checked against those sources by the opt-in test
/// `openfoam_reads_match_the_sources` (`UMCF_OF8_SRC`, `UMCF_OF12_SRC`).
pub(crate) struct OpenFoamRead {
    /// `thermo` (any fluid thermophysics), `turbulence` (any RAS or LES model), or a RAS model.
    pub(crate) by: &'static str,
    pub(crate) field: &'static str,
    pub(crate) of8: &'static str,
    pub(crate) of12: &'static str,
}

pub(crate) const OPENFOAM: &[OpenFoamRead] = &[
    OpenFoamRead {
        by: "thermo",
        field: "T",
        of8: "src/thermophysicalModels/basic/basicThermo/basicThermo.C",
        of12: "src/thermophysicalModels/basic/basicThermo/basicThermo.C",
    },
    OpenFoamRead {
        by: "thermo",
        field: "p",
        of8: "src/thermophysicalModels/basic/fluidThermo/fluidThermo.C",
        of12: "src/thermophysicalModels/basic/fluidThermo/fluidThermo.C",
    },
    OpenFoamRead {
        by: "turbulence",
        field: "nut",
        of8: "src/MomentumTransportModels/momentumTransportModels/eddyViscosity/eddyViscosity.C",
        of12: "src/MomentumTransportModels/momentumTransportModels/eddyViscosity/eddyViscosity.C",
    },
    OpenFoamRead {
        by: "turbulence",
        field: "alphat",
        of8: "src/ThermophysicalTransportModels/turbulence/eddyDiffusivity/eddyDiffusivity.C",
        // OpenFOAM-12's default for RAS and LES; eddyDiffusivity, the other, reads it too.
        of12: "src/ThermophysicalTransportModels/fluid/turbulence/unityLewisEddyDiffusivity/unityLewisEddyDiffusivity.C",
    },
    OpenFoamRead { by: "kEpsilon", field: "k", of8: RAS_KE, of12: RAS_KE },
    OpenFoamRead { by: "kEpsilon", field: "epsilon", of8: RAS_KE, of12: RAS_KE },
    OpenFoamRead { by: "realizableKE", field: "k", of8: RAS_RKE, of12: RAS_RKE },
    OpenFoamRead { by: "realizableKE", field: "epsilon", of8: RAS_RKE, of12: RAS_RKE },
    OpenFoamRead { by: "RNGkEpsilon", field: "k", of8: RAS_RNG, of12: RAS_RNG },
    OpenFoamRead { by: "RNGkEpsilon", field: "epsilon", of8: RAS_RNG, of12: RAS_RNG },
    OpenFoamRead { by: "kOmega", field: "k", of8: RAS_KO, of12: RAS_KO },
    OpenFoamRead { by: "kOmega", field: "omega", of8: RAS_KO, of12: RAS_KO },
    OpenFoamRead { by: "kOmegaSST", field: "k", of8: RAS_SST, of12: RAS_SST },
    OpenFoamRead { by: "kOmegaSST", field: "omega", of8: RAS_SST, of12: RAS_SST },
];

const RAS_KE: &str = "src/MomentumTransportModels/momentumTransportModels/RAS/kEpsilon/kEpsilon.C";
const RAS_RKE: &str = "src/MomentumTransportModels/momentumTransportModels/RAS/realizableKE/realizableKE.C";
const RAS_RNG: &str = "src/MomentumTransportModels/momentumTransportModels/RAS/RNGkEpsilon/RNGkEpsilon.C";
const RAS_KO: &str = "src/MomentumTransportModels/momentumTransportModels/RAS/kOmega/kOmega.C";
const RAS_SST: &str = "src/MomentumTransportModels/momentumTransportModels/Base/kOmegaSST/kOmegaSSTBase.C";

/// The catalog's name for a kind of region.
pub(crate) fn kind_name(k: RegionKind) -> Option<&'static str> {
    match k {
        RegionKind::Fluid => Some("fluid"),
        RegionKind::Solid => Some("solid"),
        RegionKind::Vegetation => Some("vegetation"),
        RegionKind::Single => None,
    }
}

/// One field a region needs, and who reads it.
struct Need {
    name: &'static str,
    instance: FieldInstance,
    /// Who reads it, as a clause: "urbanMicroclimateFoam reads it for every fluid region".
    by: String,
    file: &'static str,
}

/// The word value of a top-level entry of `constant/<region>/<file>`, `$macros` skipped.
pub(crate) fn selected(case: &Case, region: &str, file: &str, path: &[&str]) -> Option<String> {
    let doc = case.dict(&format!("constant/{region}/{file}"))?;
    let v = doc.value(path)?;
    let v = umcf_foam_dict::unquote(v.trim()).to_string();
    (!v.is_empty() && !v.starts_with('$') && !v.contains(char::is_whitespace)).then_some(v)
}

pub(crate) fn in_version(versions: &[OfVersion], v: Option<OfVersion>) -> bool {
    match v {
        Some(v) => versions.contains(&v),
        // Version unknown: only what both versions do.
        None => OfVersion::ALL.iter().all(|x| versions.contains(x)),
    }
}

/// What a type of the catalog (and its family's base class) reads.
fn type_reads(cat: &Catalog, family: &str, name: &str, v: Option<OfVersion>, out: &mut Vec<Need>) {
    for f in cat.fields.iter().filter(|f| f.required && in_version(f.versions, v)) {
        let by = match f.owner {
            FieldOwner::Type { family: fam, name: n } if fam == family && n == name => {
                format!("{family} `{name}` reads it")
            }
            FieldOwner::Family(fam) if fam == family => format!("{family} reads it for every type (here `{name}`)"),
            _ => continue,
        };
        out.push(Need { name: f.name, instance: f.instance, by, file: f.file });
    }
}

fn openfoam(by: &str, v: Option<OfVersion>, what: String, out: &mut Vec<Need>) {
    for r in OPENFOAM.iter().filter(|r| r.by == by) {
        let file = if v == Some(OfVersion::V8) { r.of8 } else { r.of12 };
        out.push(Need { name: r.field, instance: FieldInstance::StartTime, by: what.clone(), file });
    }
}

/// Everything region `r` needs.
fn needs(case: &Case, r: &Region, kind: &str) -> Vec<Need> {
    let cat = catalog();
    let v = case.layout.version;
    let mut out = Vec::new();
    for f in cat.fields.iter().filter(|f| f.required && in_version(f.versions, v)) {
        if matches!(f.owner, FieldOwner::Region(k) if k == kind) {
            out.push(Need {
                name: f.name,
                instance: f.instance,
                by: format!("urbanMicroclimateFoam reads it for every {kind} region"),
                file: f.file,
            });
        }
    }
    let builds =
        |class: &str| cat.builds.iter().any(|b| b.region == kind && b.class == class && in_version(b.versions, v));
    // The models selected in the region's dictionaries.
    for fam in cat.families.iter().filter(|f| builds(f.base)) {
        let chosen = match (fam.dict_file, fam.selector) {
            (Some(file), Some(key)) => selected(case, &r.name, file, &[key]),
            // OpenFOAM's radiation models: `radiationModel` in radiationProperties.
            _ if fam.base == "radiationModel" => selected(case, &r.name, "radiationProperties", &["radiationModel"]),
            _ => None,
        };
        if let Some(t) = chosen.as_deref().and_then(|t| cat.find(fam.base, t)) {
            type_reads(cat, fam.base, t.name, v, &mut out);
        }
    }
    // fluidThermo (OpenFOAM-12) and rhoThermo (OpenFOAM-8): with the version unknown, either.
    let thermo = |class: &str| cat.builds.iter().any(|b| b.region == kind && b.class == class);
    if builds("fluidThermo") || builds("rhoThermo") || (v.is_none() && (thermo("fluidThermo") || thermo("rhoThermo"))) {
        openfoam("thermo", v, "OpenFOAM's thermophysics reads it for every fluid region".to_string(), &mut out);
    }
    if builds("momentumTransportModel") {
        let file = if case.dict(&format!("constant/{}/momentumTransport", r.name)).is_some() {
            "momentumTransport"
        } else {
            "turbulenceProperties"
        };
        let sim = selected(case, &r.name, file, &["simulationType"]);
        if matches!(sim.as_deref(), Some("RAS" | "LES")) {
            let what = format!("OpenFOAM's {} turbulence reads it", sim.as_deref().unwrap_or(""));
            openfoam("turbulence", v, what, &mut out);
        }
        if sim.as_deref() == Some("RAS") {
            let model = selected(case, &r.name, file, &["RAS", "model"])
                .or_else(|| selected(case, &r.name, file, &["RAS", "RASModel"]));
            if let Some(m) = model {
                match cat.find("RASModel", &m) {
                    Some(t) => type_reads(cat, "RASModel", t.name, v, &mut out),
                    None => openfoam(&m, v, format!("OpenFOAM's RAS model `{m}` reads it"), &mut out),
                }
            }
        }
    }
    out
}

/// `field.missing`: a field a region needs is not where the solver reads it.
pub(crate) fn check(case: &Case, out: &mut Vec<Diagnostic>) {
    if case.layout.kind != CaseKind::Solver {
        return;
    }
    for r in &case.regions {
        let Some(kind) = kind_name(r.kind) else { continue };
        if r.name.is_empty() || r.patches.is_none() {
            continue;
        }
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for n in needs(case, r, kind) {
            let (dir, present) = match n.instance {
                // No start time, or no fields there: reported as such already.
                FieldInstance::StartTime => {
                    if case.start.is_none() || r.fields.is_empty() {
                        continue;
                    }
                    (region_dir(case, r), Some(r.field(n.name).is_some()))
                }
                FieldInstance::Zero | FieldInstance::Constant => {
                    let dir = if n.instance == FieldInstance::Zero {
                        format!("0/{}", r.name)
                    } else {
                        format!("constant/{}", r.name)
                    };
                    let file = format!("{dir}/{}", n.name);
                    let present = match (case.exists(&file), case.exists(&format!("{file}.gz"))) {
                        (Some(true), _) | (_, Some(true)) => Some(true),
                        (Some(false), Some(false)) => Some(false),
                        _ => None,
                    };
                    (dir, present)
                }
                FieldInstance::Other => continue,
            };
            if present != Some(false) {
                continue;
            }
            let key = (n.name, if n.instance == FieldInstance::Constant { "constant" } else { "time" });
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let level = if case.processors > 0 { Level::Warning } else { Level::Error };
            let whence = match n.instance {
                FieldInstance::Zero => ", always from `0` whatever time the run starts from",
                _ => "",
            };
            out.push(
                Diagnostic::new(
                    level,
                    "field.missing",
                    format!("{dir}/{}", n.name),
                    format!(
                        "`{}` is missing from {dir}: {} ({}){whence}, and the run stops without it{}",
                        n.name,
                        n.by,
                        n.file,
                        if level == Level::Warning { decomposed(case) } else { "" }
                    ),
                )
                .region(&r.name)
                .field(n.name),
            );
        }
    }
}
