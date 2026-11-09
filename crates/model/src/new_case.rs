//! A new case (`case.new`): its regions and components, from the one configuration's files
//! (`umcf_schema::defaults`, the tutorials'). [`make`] gives the files' text and the workflow description
//! (`system/umcfWorkflow`) that names them.
//!
//! What it writes: the region list (OpenFOAM-12's `controlDict`, OpenFOAM-8's
//! `constant/regionProperties`), the run controls (`controlDict`, without the tutorials' function
//! objects), and the settings of each region by its kind and of each component switched on.
//! What it leaves to later steps: the meshes and their dictionaries, the fields of `0/`, the
//! process counts (`decomposeParDict`), the weather and sun tables, the view factor dictionary.

use std::collections::HashSet;
use std::fmt::Write as _;

use umcf_core::OfVersion;
use umcf_foam_dict::{Document, Node};
use umcf_schema::Template;
use umcf_schema::defaults;

use crate::description::{Components, Description, MaterialDesc, RegionDesc};
use crate::description_edit::plain_name;
use crate::workflow::Radiation;
use crate::{CONTROL_DICT, Error, RegionKind, Result};

/// The fluid region the solver's radiation, grass and trees look up by name.
const AIR: &str = "air";

/// The solver a new case runs.
pub const SOLVER: &str = "urbanMicroclimateFoam";

/// Names a region can't have, compared without case: what else lives directly in `constant/`
/// or `system/` (directories, the case's own dictionaries and tables, a single-region case's
/// files), so that a region's directory never meets one of them.
const RESERVED: [&str; 41] = [
    "polyMesh",
    "triSurface",
    "boundaryData",
    "include",
    "sets",
    "extendedFeatureEdgeMesh",
    "controlDict",
    "umcfWorkflow",
    "regionProperties",
    "decomposeParDict",
    "fvSchemes",
    "fvSolution",
    "fvConstraints",
    "fvOptions",
    "fvModels",
    "g",
    "transportProperties",
    "thermophysicalProperties",
    "momentumTransport",
    "turbulenceProperties",
    "radiationProperties",
    "solarLoadProperties",
    "grassProperties",
    "vegetationProperties",
    "viewFactorsDict",
    "blockMeshDict",
    "snappyHexMeshDict",
    "createPatchDict",
    "changeDictionaryDict",
    "topoSetDict",
    "setFieldsDict",
    "surfaceFeaturesDict",
    "surfaceFeatureExtractDict",
    "extrudeMeshDict",
    "IDN",
    "Idif",
    "sunPosVector",
    "LW",
    "cloudCover",
    "Tambient",
    "wambient",
];

/// Names Windows keeps for its devices, with or without an extension (`con`, `nul.txt`).
fn windows_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.as_bytes().get(3).is_some_and(|b| (b'1'..=b'9').contains(b)))
}

/// One region of a new case.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRegion {
    pub name: String,
    /// Fluid, solid or vegetation.
    pub kind: RegionKind,
    /// A solid's materials, each with its model.
    pub materials: Vec<MaterialDesc>,
}

/// The axis that points up: gravity points down it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Up {
    X,
    Y,
    #[default]
    Z,
}

impl Up {
    pub fn parse(s: &str) -> Option<Up> {
        match s {
            "x" => Some(Up::X),
            "y" => Some(Up::Y),
            "z" => Some(Up::Z),
            _ => None,
        }
    }

    /// The value of a fluid's `g`.
    fn gravity(self) -> &'static str {
        match self {
            Up::X => "( -9.81 0 0 )",
            Up::Y => "( 0 -9.81 0 )",
            Up::Z => "( 0 0 -9.81 )",
        }
    }
}

/// What a new case is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCase {
    pub version: OfVersion,
    /// Fluids first, then solids, then vegetation, is the order the region list gets.
    pub regions: Vec<NewRegion>,
    /// The region the radiation is computed on (a fluid or the vegetation); `None`: none.
    pub radiation: Option<String>,
    pub components: Components,
    /// The patches `grassProperties` names, with [`Components::grass`].
    pub grass_patches: Vec<String>,
    pub up: Up,
}

/// A new case's files.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    /// Each file's path in the case and its text, the description first.
    pub files: Vec<(String, String)>,
    pub description: Description,
    /// What is left to do before it runs that the files can't say (a material without
    /// coefficients, grass without patches).
    pub notes: Vec<String>,
}

fn refuse<T>(message: String) -> Result<T> {
    Err(Error::NewCase(message))
}

/// The files of the case `spec` describes. Refused (and nothing made): no fluid region, a name that isn't plain or is given twice (names that differ only
/// by case too: they can't live together on Windows or macOS), materials of a region that
/// isn't a solid or of a model the solver doesn't have, radiation on a region that isn't a
/// fluid or the vegetation, what the solver can't run as it looks regions up by name (see
/// `check`), grass without the radiation, grass patches without grass, the blending layer
/// without z up.
pub fn make(spec: &NewCase) -> Result<Made> {
    check(spec)?;
    let v = spec.version;
    let fluids: Vec<&NewRegion> = spec.regions.iter().filter(|r| r.kind == RegionKind::Fluid).collect();
    let vegetation = spec.regions.iter().any(|r| r.kind == RegionKind::Vegetation);
    let mut notes = Vec::new();

    let description = Description {
        version: Some(v),
        regions: ordered(spec)
            .into_iter()
            .map(|r| RegionDesc {
                name: r.name.clone(),
                kind: r.kind,
                mesh: None,
                steps: Vec::new(),
                patches: Vec::new(),
                materials: r.materials.clone(),
            })
            .collect(),
        features: false,
        radiation: spec.radiation.as_ref().map(|region| Radiation {
            region: region.clone(),
            // The leaf area density is computed on the air the trees stand in.
            lai_region: vegetation.then(|| AIR.to_string()),
            view_factors: true,
            solar_ray_tracing: true,
        }),
        components: spec.components,
        solver: Some(SOLVER.to_string()),
        reconstruct: true,
    };

    let mut files = vec![(crate::description::PATH.to_string(), description.write())];
    files.push((CONTROL_DICT.to_string(), control_dict(spec)?));
    if v == OfVersion::V8 {
        let mut doc = template(v, Template::RegionProperties)?;
        set(&mut doc, &["regions"], &region_properties(spec), Template::RegionProperties)?;
        files.push(("constant/regionProperties".to_string(), doc.to_string()));
    }
    for r in ordered(spec) {
        let n = &r.name;
        let mut put = |path: String, t: Template| -> Result<()> {
            files.push((path, template(v, t)?.to_string()));
            Ok(())
        };
        match r.kind {
            RegionKind::Fluid | RegionKind::Single => {
                put(format!("constant/{n}/thermophysicalProperties"), Template::FluidThermo)?;
                put(format!("constant/{n}/momentumTransport"), Template::FluidMomentum)?;
                put(format!("system/{n}/fvSchemes"), Template::FluidSchemes)?;
                put(format!("system/{n}/fvSolution"), Template::FluidSolution)?;
                let mut g = template(v, Template::FluidG)?;
                set(&mut g, &["value"], spec.up.gravity(), Template::FluidG)?;
                files.push((format!("constant/{n}/g"), g.to_string()));
                let (min, max) = defaults::TEMPERATURE_LIMIT;
                match v {
                    OfVersion::V12 => {
                        let mut c = template(v, Template::FluidConstraints)?;
                        if spec.components.temperature_limit {
                            let limit = format!("{{ type limitTemperature; select all; min {min}; max {max}; }}");
                            c.set_inline(&["limitT"], &limit).map_err(|e| dict_error(Template::FluidConstraints, e))?;
                        }
                        files.push((format!("system/{n}/fvConstraints"), c.to_string()));
                    }
                    OfVersion::V8 => {
                        if spec.components.temperature_limit {
                            let mut o = template(v, Template::FluidOptions)?;
                            set(&mut o, &["limitT", "min"], min, Template::FluidOptions)?;
                            set(&mut o, &["limitT", "max"], max, Template::FluidOptions)?;
                            files.push((format!("system/{n}/fvOptions"), o.to_string()));
                        }
                    }
                }
            }
            RegionKind::Solid => {
                put(format!("constant/{n}/g"), Template::SolidG)?;
                put(format!("system/{n}/fvSchemes"), Template::SolidSchemes)?;
                put(format!("system/{n}/fvSolution"), Template::SolidSolution)?;
                let (text, missing) = transport(v, &r.materials)?;
                files.push((format!("constant/{n}/transportProperties"), text));
                if r.materials.is_empty() {
                    notes.push(format!(
                        "region {n}: no materials: the solver needs one for each of its cell zones (workflow.material)"
                    ));
                }
                for (m, model) in missing {
                    notes.push(format!(
                        "region {n}, material {m}: the tutorials have no {model} material to take its coefficients from: add them to constant/{n}/transportProperties"
                    ));
                }
            }
            RegionKind::Vegetation => {
                put(format!("system/{n}/fvSchemes"), Template::VegetationSchemes)?;
                put(format!("system/{n}/fvSolution"), Template::VegetationSolution)?;
            }
        }
    }
    // The grass and the trees' model are `air`'s, which the solver looks them up in (there is one
    // whenever either is asked for: they need the radiation, which needs it).
    if spec.components.grass || vegetation {
        // `check` saw to it that there is one.
        if !fluids.iter().any(|f| f.name == AIR) {
            return refuse("the grass and the trees need a fluid region named `air`".to_string());
        }
        let air = AIR;
        if spec.components.grass {
            let mut g = template(v, Template::Grass)?;
            let model = g.value(&["grassModel"]).unwrap_or_default();
            let mut list = String::from("\n    (\n");
            for g in &spec.grass_patches {
                let _ = writeln!(list, "        {g}");
            }
            list.push_str("    )");
            set(&mut g, &[&format!("{model}Coeffs"), "grassPatches"], &list, Template::Grass)?;
            files.push((format!("constant/{air}/grassProperties"), g.to_string()));
            if spec.grass_patches.is_empty() {
                notes.push(format!(
                    "constant/{air}/grassProperties: no grass patches: name the patches the grass grows on"
                ));
            }
        }
        if vegetation {
            files
                .push((format!("constant/{air}/vegetationProperties"), template(v, Template::Vegetation)?.to_string()));
        }
    }
    if let Some(rad) = &spec.radiation {
        files.push((format!("constant/{rad}/radiationProperties"), template(v, Template::Radiation)?.to_string()));
        files.push((format!("constant/{rad}/solarLoadProperties"), template(v, Template::SolarLoad)?.to_string()));
    }
    Ok(Made { files, description, notes })
}

/// The regions in the order of the region list: fluids, solids, vegetation, each as given.
fn ordered(spec: &NewCase) -> Vec<&NewRegion> {
    let of = |k: RegionKind| spec.regions.iter().filter(move |r| r.kind == k);
    of(RegionKind::Fluid).chain(of(RegionKind::Solid)).chain(of(RegionKind::Vegetation)).collect()
}

fn check(spec: &NewCase) -> Result<()> {
    if !spec.regions.iter().any(|r| r.kind == RegionKind::Fluid) {
        return refuse("a case needs a fluid region (the air)".to_string());
    }
    let mut names = HashSet::new();
    let mut folded = HashSet::new();
    let cat = umcf_schema::catalog::catalog();
    let family = cat.families.iter().find(|f| f.selector == Some("buildingMaterialModel")).map(|f| f.base);
    for r in &spec.regions {
        let n = r.name.as_str();
        if !plain_name(n) || n.contains(':') {
            return refuse(format!(
                "region `{n}`: a region's name is letters, digits, `_`, `.` and `-`, not starting with a digit or `-`"
            ));
        }
        if n.ends_with('.') {
            return refuse(format!("region `{n}`: a name can't end with `.` (Windows drops it)"));
        }
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(n)) {
            return refuse(format!("region `{n}`: the name of a file or directory of the case's constant/ or system/"));
        }
        if windows_device(n) {
            return refuse(format!("region `{n}`: a name Windows keeps for a device"));
        }
        if !names.insert(n) {
            return refuse(format!("region {n} is given twice"));
        }
        if !folded.insert(n.to_lowercase()) {
            return refuse(format!(
                "region {n}: another region's name differs from it only by case (they can't live together on Windows or macOS)"
            ));
        }
        if r.kind == RegionKind::Single {
            return refuse(format!("region {n}: a region is a fluid, a solid or the vegetation"));
        }
        if r.kind != RegionKind::Solid && !r.materials.is_empty() {
            return refuse(format!("region {n}: only a solid has materials"));
        }
        let mut materials = HashSet::new();
        for m in &r.materials {
            if !plain_name(&m.name) {
                return refuse(format!("region {n}, material `{}`: not a plain name", m.name));
            }
            if !materials.insert(m.name.as_str()) {
                return refuse(format!("region {n}, material {} is given twice", m.name));
            }
            let Some(model) = &m.model else {
                return refuse(format!(
                    "region {n}, material {}: its model (buildingMaterialModel) is missing",
                    m.name
                ));
            };
            let known = family.and_then(|f| cat.find(f, model)).is_some_and(|t| t.in_version(spec.version));
            if !known {
                let models = family.map(|f| cat.choices(f, spec.version).join(", ")).unwrap_or_default();
                return refuse(format!(
                    "region {n}, material {}: {model} is not a material model of the solver for {} ({models})",
                    m.name,
                    spec.version.label()
                ));
            }
        }
    }
    // What the solver looks up by name.
    let vegetation: Vec<&str> =
        spec.regions.iter().filter(|r| r.kind == RegionKind::Vegetation).map(|r| r.name.as_str()).collect();
    match vegetation.as_slice() {
        [] | ["vegetation"] => {}
        [one] => {
            return refuse(format!(
                "region {one}: the vegetation region is named `vegetation` (the solver's trees, grass and solid conditions look it up by that name)"
            ));
        }
        _ => return refuse("one vegetation region at most (the solver looks up `vegetation`)".to_string()),
    }
    if !vegetation.is_empty() && spec.radiation.as_deref() != Some("vegetation") {
        return refuse(
            "the vegetation needs the radiation on it (`radiation`: vegetation): the trees' model reads its qr on air_to_vegetation".to_string(),
        );
    }
    if spec.radiation.is_some() && !spec.regions.iter().any(|r| r.kind == RegionKind::Fluid && r.name == AIR) {
        return refuse(
            "the radiation needs a fluid region named `air` (viewFactorSky looks it up by that name)".to_string(),
        );
    }
    if spec.components.grass && spec.radiation.is_none() {
        return refuse("the grass needs the radiation (`radiation`): it reads qr and qs on its patches".to_string());
    }
    if spec.components.blending && spec.up != Up::Z {
        return refuse(
            "the blending layer needs z up: it takes the sides west and east along x, north and south along y"
                .to_string(),
        );
    }
    if let Some(rad) = &spec.radiation {
        match spec.regions.iter().find(|r| &r.name == rad) {
            Some(r) if matches!(r.kind, RegionKind::Fluid | RegionKind::Vegetation) => {}
            Some(_) => return refuse(format!("radiation on {rad}: a solid has no radiation of its own")),
            None => return refuse(format!("radiation on {rad}: there is no such region")),
        }
    }
    if !spec.components.grass && !spec.grass_patches.is_empty() {
        return refuse("grass patches without grass".to_string());
    }
    let mut patches = HashSet::new();
    for g in &spec.grass_patches {
        if !plain_name(g) {
            return refuse(format!("grass patch `{g}`: not a plain name"));
        }
        if !patches.insert(g.as_str()) {
            return refuse(format!("grass patch {g} is given twice"));
        }
    }
    Ok(())
}

/// The configuration's file, read; its line ends made `\n`.
fn template(v: OfVersion, t: Template) -> Result<Document> {
    let Some(text) = defaults::template(v, t) else {
        return refuse(format!("{} has no {t:?} file", v.label()));
    };
    Document::parse(&text.replace("\r\n", "\n")).map_err(|e| dict_error(t, e))
}

fn dict_error(t: Template, e: umcf_foam_dict::Error) -> Error {
    Error::NewCase(format!("the configuration's {t:?} file: {e}"))
}

/// Sets an entry the configuration's file has; one it doesn't have is an error (the file isn't the
/// one expected).
fn set(doc: &mut Document, path: &[&str], value: &str, t: Template) -> Result<()> {
    if doc.get(path).is_none() {
        return refuse(format!("the configuration's {t:?} file has no {}", path.join("/")));
    }
    doc.set(path, value).map_err(|e| dict_error(t, e))
}

/// The names of the regions of kind `k`, as a list's body.
fn names(spec: &NewCase, k: RegionKind) -> Vec<&str> {
    spec.regions.iter().filter(|r| r.kind == k).map(|r| r.name.as_str()).collect()
}

/// The kinds with regions and their names, in the order the solver lists them (OpenFOAM-12's
/// solver asks whether each is there).
fn kinds(spec: &NewCase) -> Vec<(&'static str, Vec<&str>)> {
    [("fluid", RegionKind::Fluid), ("solid", RegionKind::Solid), ("vegetation", RegionKind::Vegetation)]
        .into_iter()
        .map(|(w, k)| (w, names(spec, k)))
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

/// OpenFOAM-8's `regions` list: `( fluid (air) solid (…) vegetation () )`. Every kind, an
/// empty one too: the solver and its utilities look each one up and stop without it.
fn region_properties(spec: &NewCase) -> String {
    let mut s = String::from("\n(\n");
    let every = [("fluid", RegionKind::Fluid), ("solid", RegionKind::Solid), ("vegetation", RegionKind::Vegetation)]
        .map(|(w, k)| (w, names(spec, k)));
    for (w, n) in every {
        let _ = writeln!(s, "    {w:<11} ({})", n.join(" "));
    }
    s.push(')');
    s
}

/// `controlDict`: the tutorials', without their function objects (they name the tutorials'
/// regions and places), with the region list (OpenFOAM-12) and the blending layer as asked.
fn control_dict(spec: &NewCase) -> Result<String> {
    let t = Template::ControlDict;
    let mut doc = template(spec.version, t)?;
    let err = |e| dict_error(t, e);
    doc.remove(&["functions"]).map_err(err)?;
    doc.set(&["application"], SOLVER).map_err(err)?;
    if spec.components.blending {
        doc.set(&["blending"], "true").map_err(err)?;
        if doc.get(&["blendingCoeffs"]).is_none() {
            doc.set(&["blendingCoeffs"], defaults::BLENDING_COEFFS).map_err(err)?;
        }
    } else {
        doc.remove(&["blending"]).map_err(err)?;
        doc.remove(&["blendingCoeffs"]).map_err(err)?;
    }
    doc.remove(&["regions"]).map_err(err)?;
    if spec.version == OfVersion::V12 {
        for (w, n) in kinds(spec) {
            doc.set(&["regions", w], &format!("( {} )", n.join(" "))).map_err(err)?;
        }
    }
    Ok(doc.to_string())
}

/// A solid's `transportProperties`: the tutorials', with `buildingMaterials` the given ones, each
/// with the coefficients of the tutorials' first material of its model. Also the materials
/// whose model the tutorials have none of (name, model): they get no coefficients.
fn transport(v: OfVersion, materials: &[MaterialDesc]) -> Result<(String, Vec<(String, String)>)> {
    let t = Template::SolidTransport;
    let mut doc = template(v, t)?;
    // The tutorials' materials: model → its coefficients (every entry but the name and model).
    let mut library: Vec<(String, Vec<(String, String)>)> = Vec::new();
    if let Some(items) = doc.get(&["buildingMaterials"]).and_then(|e| match e.value.as_slice() {
        [Node::List { items, .. }] => Some(items),
        _ => None,
    }) {
        for item in items {
            let Node::Dict { body, .. } = item else { continue };
            let Some(model) = body.get("buildingMaterialModel").and_then(|e| e.value_text()) else { continue };
            if library.iter().any(|(m, _)| *m == model) {
                continue;
            }
            let coeffs = body
                .entries()
                .filter(|e| !matches!(e.key(), "name" | "buildingMaterialModel"))
                .filter_map(|e| Some((e.key().to_string(), e.value_text()?)))
                .collect();
            library.push((model, coeffs));
        }
    }
    let mut missing = Vec::new();
    let mut list = String::from("\n(\n");
    for m in materials {
        let model = m.model.as_deref().unwrap_or_default();
        let _ = writeln!(list, "    {{\n        name    {};\n        buildingMaterialModel {model};", m.name);
        match library.iter().find(|(lm, _)| lm == model) {
            Some((_, coeffs)) => {
                for (k, val) in coeffs {
                    let _ = writeln!(list, "        {k:<7} {val};");
                }
            }
            None => missing.push((m.name.clone(), model.to_string())),
        }
        list.push_str("    }\n");
    }
    list.push(')');
    set(&mut doc, &["buildingMaterials"], &list, t)?;
    Ok((doc.to_string(), missing))
}

#[cfg(test)]
#[path = "new_case_tests.rs"]
mod tests;
