//! The settings a case chooses: the `settings` block of `system/umcfWorkflow`, each with its
//! default (the one configuration's, `umcf_schema::defaults`), read from a case's files and
//! written into them.
//!
//! ```c++
//! settings
//! {
//!     deltaT           3600;
//!     endTime          86400;
//!     writeInterval    1;
//!     turbulence       realizableKE;
//!     temperatureLimit ( 273.15 373.15 );
//!     blending         { dampingThickness 60; alphaCoeffU 0.3; alphaCoeffT 0.3; }
//! }
//! ```
//!
//! | Key | Where the case has it |
//! |---|---|
//! | `deltaT`, `endTime`, `writeInterval` | `system/controlDict` |
//! | `turbulence` | each `simulationType RAS` fluid's `constant/<r>/momentumTransport`, `RAS/model` (`RASModel` where the file has that) |
//! | `temperatureLimit` | the `limitTemperature` entries of each fluid's `fvOptions`, `fvConstraints` or `fvModels` (in `constant/` or `system/`), `min` and `max` (in its `limitTemperatureCoeffs` when it has one) |
//! | `blending` | `blendingCoeffs` in `system/controlDict` |

use std::collections::HashSet;
use std::fmt::Write as _;

use umcf_foam_dict::{Dict, Entry};

use crate::{CONTROL_DICT, Case, Error, RegionKind, Result};

/// A material library: each `buildingMaterialModel` with its coefficients (key, value as
/// written), in order.
pub type Library = Vec<(String, Vec<(String, String)>)>;

/// The keys of the blending layer's coefficients, in the order they are written.
pub const BLENDING_KEYS: [&str; 3] = ["dampingThickness", "alphaCoeffU", "alphaCoeffT"];

/// The settings a case chose; `None`: the default (the one configuration's, or what the case's
/// files say).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub delta_t: Option<String>,
    pub end_time: Option<String>,
    pub write_interval: Option<String>,
    /// The fluids' RAS model.
    pub turbulence: Option<String>,
    /// `min` and `max`, in kelvin.
    pub temperature_limit: Option<(String, String)>,
    /// The blending layer's coefficients, in the order of [`BLENDING_KEYS`].
    pub blending: Option<[String; 3]>,
    /// The material library: each `buildingMaterialModel` with the coefficients its new materials
    /// start from in `case.new` and `region.add` (every entry but `name` and the model, as
    /// written: a number, or a dictionary of them). The case's own materials keep theirs.
    pub materials: Library,
}

/// A number as the settings take it: finite, written as OpenFOAM reads it.
fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A run time as the settings take it: above 0, `endTime` 0 too.
fn run_time(key: &str, text: &str) -> bool {
    number(text).is_some_and(|n| n > 0.0 || (key == "endTime" && n >= 0.0))
}

/// Temperature-limit bounds as the settings take them: `0 < min < max`, in kelvin.
fn limit_bounds(min: &str, max: &str) -> bool {
    matches!((number(min), number(max)), (Some(x), Some(y)) if 0.0 < x && x < y)
}

/// A word as the settings take it: letters, digits and `_` (a model's name).
fn model_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn words(e: &Entry) -> Option<Vec<String>> {
    e.words().map(|w| w.iter().map(|s| s.to_string()).collect())
}

impl Settings {
    /// Whether nothing is chosen.
    pub fn is_empty(&self) -> bool {
        *self == Settings::default()
    }

    /// The `settings` block's body, each problem named (`settings/deltaT`).
    pub fn read(body: &Dict, problem: &mut impl FnMut(&str, String)) -> Settings {
        let mut s = Settings::default();
        let mut seen = HashSet::new();
        for e in body.entries() {
            let key = e.key();
            let at = format!("settings/{key}");
            if !seen.insert(key) {
                problem(&at, "is given twice: the later ones are left out".into());
                continue;
            }
            let one = || match words(e).as_deref() {
                Some([w]) => Some(w.clone()),
                _ => None,
            };
            match key {
                "deltaT" | "endTime" | "writeInterval" => {
                    let v = one().filter(|w| run_time(key, w));
                    let Some(v) = v else {
                        problem(&at, "is a number above 0".into());
                        continue;
                    };
                    match key {
                        "deltaT" => s.delta_t = Some(v),
                        "endTime" => s.end_time = Some(v),
                        _ => s.write_interval = Some(v),
                    }
                }
                "turbulence" => match one().filter(|w| model_word(w)) {
                    Some(w) => s.turbulence = Some(w),
                    None => problem(&at, "is the name of a RAS model (realizableKE, kEpsilon, …)".into()),
                },
                "temperatureLimit" => {
                    let pair = match e.value.as_slice() {
                        [umcf_foam_dict::Node::List { items, .. }] => match items.as_slice() {
                            [umcf_foam_dict::Node::Atom(a), umcf_foam_dict::Node::Atom(b)] => {
                                Some((a.text.clone(), b.text.clone()))
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    match pair.filter(|(a, b)| limit_bounds(a, b)) {
                        Some(p) => s.temperature_limit = Some(p),
                        None => problem(&at, "is ( min max ), in kelvin, min below max".into()),
                    }
                }
                "blending" => match e.dict() {
                    Some(b) => {
                        let mut got: [Option<String>; 3] = Default::default();
                        let mut ok = true;
                        for c in b.entries() {
                            let i = BLENDING_KEYS.iter().position(|k| *k == c.key());
                            let v = match words(c).as_deref() {
                                Some([w]) if number(w).is_some() => Some(w.clone()),
                                _ => None,
                            };
                            match (i, v) {
                                (Some(i), Some(v)) => got[i] = Some(v),
                                (None, _) => {
                                    problem(
                                        &format!("{at}/{}", c.key()),
                                        format!("is not one of {}", BLENDING_KEYS.join(", ")),
                                    );
                                    ok = false;
                                }
                                (Some(_), None) => {
                                    problem(&format!("{at}/{}", c.key()), "is a number".into());
                                    ok = false;
                                }
                            }
                        }
                        match got {
                            [Some(a), Some(b), Some(c)] if ok => s.blending = Some([a, b, c]),
                            _ if ok => problem(&at, format!("gives {}", BLENDING_KEYS.join(", "))),
                            _ => {}
                        }
                    }
                    None => problem(&at, "is a dictionary".into()),
                },
                "materials" => match e.dict() {
                    Some(b) => s.materials = read_library(b, &at, problem),
                    None => problem(&at, "is a dictionary of material models".into()),
                },
                _ => problem(
                    &at,
                    "is not a setting (deltaT, endTime, writeInterval, turbulence, temperatureLimit, blending, materials)".into(),
                ),
            }
        }
        s
    }

    /// Each chosen setting's key and value, as the `settings` block writes them, in its order.
    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (k, v) in [
            ("deltaT", &self.delta_t),
            ("endTime", &self.end_time),
            ("writeInterval", &self.write_interval),
            ("turbulence", &self.turbulence),
        ] {
            if let Some(v) = v {
                out.push((k, v.clone()));
            }
        }
        if let Some((a, b)) = &self.temperature_limit {
            out.push(("temperatureLimit", format!("( {a} {b} )")));
        }
        if let Some(c) = &self.blending {
            let body: Vec<String> = BLENDING_KEYS.iter().zip(c).map(|(k, v)| format!("{k} {v};")).collect();
            out.push(("blending", format!("{{ {} }}", body.join(" "))));
        }
        out
    }

    /// The `settings` block, or nothing when nothing is chosen.
    pub fn write(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut s = String::from("settings\n{\n");
        for (k, v) in self.entries() {
            let end = if v.starts_with('{') { "" } else { ";" };
            let _ = writeln!(s, "    {k:<16} {v}{end}");
        }
        if !self.materials.is_empty() {
            s.push_str("    materials\n    {\n");
            for (model, coeffs) in &self.materials {
                let _ = writeln!(s, "        {model:<16} {}", coefficients(coeffs));
            }
            s.push_str("    }\n");
        }
        s.push_str("}\n\n");
        s
    }

    /// The settings the case's files have, as written; one given by a macro, a directive or
    /// differently in two fluids is left out, with a note.
    pub fn from_case(case: &Case) -> (Settings, Vec<String>) {
        let mut s = Settings::default();
        let mut notes = Vec::new();
        // As `read` takes them: a value it would refuse is noted and left out.
        let plain = |k: &str, v: String, notes: &mut Vec<String>| -> Option<String> {
            if run_time(k, &v) {
                Some(v)
            } else {
                notes.push(format!("controlDict {k} is {v}: not a number the settings take, left out"));
                None
            }
        };
        if let Some(d) = case.dict(CONTROL_DICT) {
            let get = |k: &str| d.value(&[k]).map(|v| v.trim().to_string());
            s.delta_t = get("deltaT").and_then(|v| plain("deltaT", v, &mut notes));
            s.end_time = get("endTime").and_then(|v| plain("endTime", v, &mut notes));
            s.write_interval = get("writeInterval").and_then(|v| plain("writeInterval", v, &mut notes));
            if d.get(&["blendingCoeffs"]).is_some() {
                let c: Vec<Option<String>> = BLENDING_KEYS
                    .iter()
                    .map(|k| {
                        d.value(&["blendingCoeffs", k]).map(|v| v.trim().to_string()).filter(|v| number(v).is_some())
                    })
                    .collect();
                match c.as_slice() {
                    [Some(a), Some(b), Some(c)] => s.blending = Some([a.clone(), b.clone(), c.clone()]),
                    _ => notes.push(format!(
                        "controlDict blendingCoeffs: not all of {} as numbers, left out",
                        BLENDING_KEYS.join(", ")
                    )),
                }
            }
        }
        let fluids: Vec<&str> =
            case.regions.iter().filter(|r| r.kind == RegionKind::Fluid).map(|r| r.name.as_str()).collect();
        let mut models = Vec::new();
        let mut limits = Vec::new();
        for f in &fluids {
            if let Some((_, m)) = turbulence_at(case, f) {
                models.push(m);
            }
            for (rel, at) in limit_entries(case, f) {
                if let Some(d) = case.dict(&rel) {
                    let v = |k: &str| {
                        let mut path: Vec<&str> = at.iter().map(String::as_str).collect();
                        path.push(k);
                        d.value(&path).map(|v| v.trim().to_string())
                    };
                    match (v("min"), v("max")) {
                        (Some(a), Some(b)) => limits.push(Some((a, b))),
                        // Incomplete: noted, and no bounds agree with it.
                        _ => {
                            notes.push(format!("{rel}: its limitTemperature entry has no min or no max"));
                            limits.push(None);
                        }
                    }
                }
            }
        }
        let agreed = |vs: &[String]| vs.first().filter(|first| vs.iter().all(|v| v == *first)).cloned();
        match agreed(&models) {
            Some(m) if model_word(&m) => s.turbulence = Some(m),
            Some(m) => notes.push(format!("the fluids' RAS model is {m}: not a model's name, left out")),
            None if !models.is_empty() => {
                notes.push(format!("the fluids' RAS models differ ({}): left out", models.join(", ")))
            }
            None => {}
        }
        // An incomplete entry (`None`) agrees with no bounds; it was noted where it was found.
        let pairs: Vec<String> =
            limits.iter().map(|l| l.as_ref().map_or_else(|| "\0".to_string(), |(a, b)| format!("{a} {b}"))).collect();
        match agreed(&pairs) {
            Some(_) => match limits.swap_remove(0) {
                Some((a, b)) if limit_bounds(&a, &b) => s.temperature_limit = Some((a, b)),
                Some((a, b)) => {
                    notes.push(format!("the temperature limit is ( {a} {b} ): not 0 < min < max, left out"))
                }
                None => {}
            },
            None if !pairs.is_empty() => notes.push("the fluids' temperature limits differ: left out".to_string()),
            None => {}
        }
        let (materials, more) = case_library(case);
        s.materials = materials;
        notes.extend(more);
        (s, notes)
    }
}

/// The file and key of `region`'s RAS model (`RAS/model`, else `RAS/RASModel`), with the model.
/// A fluid that isn't `simulationType RAS` (laminar, LES) has none, whatever its `RAS` block says.
fn turbulence_at(case: &Case, region: &str) -> Option<(String, String)> {
    let rel = format!("constant/{region}/momentumTransport");
    let d = case.dict(&rel)?;
    if d.value(&["simulationType"]).map(|v| umcf_foam_dict::unquote(v.trim()).to_string()).as_deref() != Some("RAS") {
        return None;
    }
    ["model", "RASModel"].into_iter().find_map(|k| {
        let v = d.value(&["RAS", k])?;
        Some((k.to_string(), umcf_foam_dict::unquote(v.trim()).to_string()))
    })
}

/// The files of `region`'s `limitTemperature` entries (`fvOptions`, `fvConstraints`,
/// `fvModels`, in `constant/` or `system/`), each with the path of the dictionary its `min` and
/// `max` are read from: the entry's `limitTemperatureCoeffs` when it has one (OpenFOAM reads the
/// coefficients from there, `optionalSubDict`), else the entry.
fn limit_entries(case: &Case, region: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for rel in ["fvOptions", "fvConstraints", "fvModels"]
        .into_iter()
        .flat_map(|f| [format!("constant/{region}/{f}"), format!("system/{region}/{f}")])
    {
        let Some(d) = case.dict(&rel) else { continue };
        for e in d.root.entries() {
            let limit = e
                .dict()
                .and_then(|b| b.get("type"))
                .and_then(|t| t.words())
                .is_some_and(|w| matches!(w.as_slice(), [t] if umcf_foam_dict::unquote(t) == "limitTemperature"));
            if limit {
                let mut at = vec![e.key().to_string()];
                if e.dict().and_then(|b| b.get("limitTemperatureCoeffs")).is_some_and(|c| c.dict().is_some()) {
                    at.push("limitTemperatureCoeffs".to_string());
                }
                out.push((rel.clone(), at));
            }
        }
    }
    out
}

/// The RAS models `version` offers: OpenFOAM's own and the solver's (the catalog's).
pub fn ras_models(version: umcf_core::OfVersion) -> Vec<&'static str> {
    let mut m: Vec<&str> = umcf_schema::defaults::openfoam_ras_models(version).to_vec();
    m.extend(umcf_schema::catalog::catalog().choices("RASModel", version));
    m
}

/// Refuses a `turbulence` that isn't a RAS model of `version` ([`ras_models`]).
pub(crate) fn check_turbulence(s: &Settings, version: umcf_core::OfVersion) -> Result<()> {
    match &s.turbulence {
        Some(m) if !ras_models(version).contains(&m.as_str()) => Err(Error::NewCase(format!(
            "turbulence {m} is not a RAS model of {} ({})",
            version.label(),
            ras_models(version).join(", ")
        ))),
        _ => Ok(()),
    }
}

/// A material's coefficients as a dictionary on one line: `{ rho 1150; cap 650; }`.
pub fn coefficients(coeffs: &[(String, String)]) -> String {
    let body: Vec<String> =
        coeffs.iter().map(|(k, v)| if v.starts_with('{') { format!("{k} {v}") } else { format!("{k} {v};") }).collect();
    format!("{{ {} }}", body.join(" "))
}

/// A coefficient's value as the library takes it: a number, or a dictionary of numbers (a
/// model's `…Coeffs`), as one line; `None` for anything else (a macro, a directive, a list).
fn coefficient(e: &Entry) -> Option<String> {
    if let Some(d) = e.dict() {
        let mut inner = Vec::new();
        for c in d.entries() {
            match c.words().as_deref() {
                Some([w]) if number(w).is_some() && model_word(c.key()) => {
                    inner.push((c.key().to_string(), w.to_string()))
                }
                _ => return None,
            }
        }
        if d.items.len() != inner.len() {
            return None;
        }
        return Some(coefficients(&inner));
    }
    match e.words().as_deref() {
        Some([w]) if number(w).is_some() => Some(w.to_string()),
        _ => None,
    }
}

/// The `materials` block's body: each model with its coefficients.
fn read_library(body: &Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> Library {
    let mut out: Library = Vec::new();
    let mut seen = HashSet::new();
    for m in body.entries() {
        let model = m.key();
        let mat = format!("{at}/{model}");
        if !model_word(model) {
            problem(&mat, "is not a material model's name".into());
            continue;
        }
        if !seen.insert(model.to_string()) {
            problem(&mat, "is given twice: the later ones are left out".into());
            continue;
        }
        let Some(d) = m.dict() else {
            problem(&mat, "is a dictionary of coefficients ({ rho 1150; cap 650; … })".into());
            continue;
        };
        let mut coeffs = Vec::new();
        let mut ok = true;
        // A macro or a directive among them: what it gives can't be told here.
        if d.items.len() != d.entries().count() {
            problem(&mat, "has a macro or a directive: give its coefficients one by one".into());
            continue;
        }
        for c in d.entries() {
            match coefficient(c).filter(|_| model_word(c.key()) && !matches!(c.key(), "name" | "buildingMaterialModel"))
            {
                Some(v) if !coeffs.iter().any(|(k, _): &(String, String)| k == c.key()) => {
                    coeffs.push((c.key().to_string(), v))
                }
                _ => {
                    problem(
                        &format!("{mat}/{}", c.key()),
                        "is a number, or a dictionary of numbers, given once".into(),
                    );
                    ok = false;
                }
            }
        }
        if ok && !coeffs.is_empty() {
            out.push((model.to_string(), coeffs));
        } else if ok {
            problem(&mat, "has no coefficients".into());
        }
    }
    out
}

/// The solids' `transportProperties` and their `buildingMaterials` items.
fn solid_materials(case: &Case) -> Vec<(String, &umcf_foam_dict::Document)> {
    case.regions
        .iter()
        .filter(|r| r.kind == RegionKind::Solid)
        .filter_map(|r| {
            let rel = crate::description::materials_file(&r.name);
            let d = case.dict(&rel)?;
            Some((rel, d))
        })
        .collect()
}

fn items(doc: &umcf_foam_dict::Document) -> &[umcf_foam_dict::Node] {
    match doc.get(&["buildingMaterials"]).map(|e| e.value.as_slice()) {
        Some([umcf_foam_dict::Node::List { items, .. }]) => items,
        _ => &[],
    }
}

fn model_of(body: &Dict) -> Option<String> {
    match body.get("buildingMaterialModel")?.words().as_deref() {
        Some([w]) if model_word(w) => Some(w.to_string()),
        _ => None,
    }
}

/// The material library the case's solids have: each model with the coefficients of its first
/// material (in the order of the regions); a model whose materials differ (layers of one model,
/// say) is noted once, and a coefficient given otherwise (a macro) leaves its material out,
/// with a note.
fn case_library(case: &Case) -> (Library, Vec<String>) {
    let mut lib: Library = Vec::new();
    let mut notes = Vec::new();
    // Each model's first material, and the materials whose coefficients differ from it.
    let mut first: Vec<(String, String)> = Vec::new();
    let mut differ: Vec<(String, Vec<String>)> = Vec::new();
    for (rel, doc) in solid_materials(case) {
        for item in items(doc) {
            let umcf_foam_dict::Node::Dict { body, .. } = item else { continue };
            let Some(model) = model_of(body) else { continue };
            let name = body.get("name").and_then(|e| e.value_text()).unwrap_or_default();
            // `region/material`, for the notes.
            let region = rel.trim_start_matches("constant/").trim_end_matches("/transportProperties");
            let at = format!("{region}/{name}");
            let mut coeffs = Vec::new();
            // A macro or a directive among them (`$common;`, `#include`) gives what can't be told.
            let mut plain = body.items.len() == body.entries().count();
            for e in body.entries().filter(|e| !matches!(e.key(), "name" | "buildingMaterialModel")) {
                match coefficient(e).filter(|_| model_word(e.key())) {
                    Some(v) => coeffs.push((e.key().to_string(), v)),
                    None => plain = false,
                }
            }
            if !plain {
                notes.push(format!(
                    "{rel}, material {name} ({model}): a coefficient isn't a number the library takes, left out"
                ));
                continue;
            }
            if coeffs.is_empty() {
                continue;
            }
            match lib.iter().find(|(m, _)| *m == model) {
                None => {
                    first.push((model.clone(), at.clone()));
                    lib.push((model, coeffs));
                }
                // The same coefficients in another order are the same.
                Some((_, f)) if sorted(f) != sorted(&coeffs) => match differ.iter_mut().find(|(m, _)| *m == model) {
                    Some((_, names)) => names.push(at.clone()),
                    None => differ.push((model, vec![at.clone()])),
                },
                Some(_) => {}
            }
        }
    }
    for (model, names) in differ {
        let from = first.iter().find(|(m, _)| *m == model).map(|(_, f)| f.as_str()).unwrap_or_default();
        notes.push(format!(
            "{model} materials differ ({} besides {from}): the library has the first's, for new {model} materials",
            names.join(", ")
        ));
    }
    (lib, notes)
}

fn sorted(c: &[(String, String)]) -> Vec<(String, String)> {
    let mut c = c.to_vec();
    c.sort();
    c
}

/// The material models the case's solids have (every material's, a macro among its
/// coefficients or not): a library entry for one of them is taken even when the solver's catalog
/// lacks it (a model of the case's own library).
pub(crate) fn case_models(case: &Case) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, doc) in solid_materials(case) {
        for item in items(doc) {
            if let umcf_foam_dict::Node::Dict { body, .. } = item
                && let Some(m) = model_of(body)
                && !out.contains(&m)
            {
                out.push(m);
            }
        }
    }
    out
}

/// Refuses a library model that isn't a material model of the solver for `version`.
pub(crate) fn check_materials(s: &Settings, version: umcf_core::OfVersion) -> Result<()> {
    let cat = umcf_schema::catalog::catalog();
    for (model, _) in &s.materials {
        if !cat.find("buildingMaterialModel", model).is_some_and(|t| t.in_version(version)) {
            return Err(Error::NewCase(format!(
                "materials/{model} is not a material model of the solver for {} ({})",
                version.label(),
                cat.choices("buildingMaterialModel", version).join(", ")
            )));
        }
    }
    Ok(())
}

/// What [`Case::apply_settings`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Applied {
    /// The files changed (not saved yet).
    pub changed: Vec<String>,
    /// What wasn't written, and why (no temperature limit in the case, say).
    pub notes: Vec<String>,
}

impl Case {
    /// Writes the chosen settings `s` into the case's files, where [the module](self) says;
    /// nothing else changes. A setting the case has no place for (a temperature limit in a
    /// case without one, a fluid without `momentumTransport`) is noted, not added: switching a
    /// component on is its own step.
    pub fn apply_settings(&mut self, s: &Settings) -> Result<Applied> {
        let mut next = self.clone();
        let applied = next.apply_settings_to(s)?;
        *self = next;
        Ok(applied)
    }

    fn apply_settings_to(&mut self, s: &Settings) -> Result<Applied> {
        let mut a = Applied::default();
        let changed = |a: &mut Applied, rel: &str| {
            if !a.changed.iter().any(|c| c == rel) {
                a.changed.push(rel.to_string());
            }
        };
        let control = [("deltaT", &s.delta_t), ("endTime", &s.end_time), ("writeInterval", &s.write_interval)];
        for (k, v) in control {
            if let Some(v) = v {
                if self.dict(CONTROL_DICT).is_none() {
                    return Err(Error::NotLoaded { path: CONTROL_DICT.to_string() });
                }
                self.set(CONTROL_DICT, &[k], v)?;
                changed(&mut a, CONTROL_DICT);
            }
        }
        if let Some(c) = &s.blending {
            let has = self.dict(CONTROL_DICT).is_some_and(|d| d.get(&["blendingCoeffs"]).is_some());
            if has || crate::description::case_components(self).blending {
                for (k, v) in BLENDING_KEYS.iter().zip(c) {
                    self.set(CONTROL_DICT, &["blendingCoeffs", k], v)?;
                }
                changed(&mut a, CONTROL_DICT);
            } else {
                a.notes.push(
                    "the case has no blending layer: the coefficients are kept in the description for when it gets one"
                        .to_string(),
                );
            }
        }
        let fluids: Vec<String> =
            self.regions.iter().filter(|r| r.kind == RegionKind::Fluid).map(|r| r.name.clone()).collect();
        if let Some(m) = &s.turbulence {
            for f in &fluids {
                let rel = format!("constant/{f}/momentumTransport");
                match turbulence_at(self, f) {
                    Some((key, _)) => {
                        self.set(&rel, &["RAS", &key], m)?;
                        changed(&mut a, &rel);
                    }
                    None => a.notes.push(format!("{rel}: no RAS model to set (no file, or not simulationType RAS)")),
                }
            }
        }
        if let Some((min, max)) = &s.temperature_limit {
            let mut any = false;
            for f in &fluids {
                for (rel, at) in limit_entries(self, f) {
                    let mut path: Vec<&str> = at.iter().map(String::as_str).collect();
                    path.push("min");
                    self.set(&rel, &path, min)?;
                    path.pop();
                    path.push("max");
                    self.set(&rel, &path, max)?;
                    changed(&mut a, &rel);
                    any = true;
                }
            }
            if !any {
                a.notes.push(
                    "the case has no temperature limit: the bounds are kept in the description for when it gets one"
                        .to_string(),
                );
            }
        }
        if !s.materials.is_empty() {
            a.notes.push(
                "the material library is what new materials of its models start from (case.new, region.add): the case's materials keep theirs"
                    .to_string(),
            );
        }
        Ok(a)
    }
}

/// What [`Case::choose_settings`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chosen {
    pub applied: Applied,
    /// Whether the workflow description has them now (the case has one).
    pub described: bool,
}

impl Case {
    /// Chooses the settings `s` gives (the others stay as they are): written into the case's
    /// files ([`Case::apply_settings`]) and into the `settings` block of the workflow
    /// description when the case has one, entry by entry (its comments and layout stay).
    /// Refused, with nothing changed: what [`Case::apply_settings`] refuses.
    pub fn choose_settings(&mut self, s: &Settings) -> Result<Chosen> {
        // A model the case's fluids have already (one of its own library, say) is kept as it is.
        let had = Settings::from_case(self).0.turbulence;
        if let Some(v) = self.layout.version
            && s.turbulence != had
        {
            check_turbulence(s, v)?;
        }
        if let Some(v) = self.layout.version {
            // The models the case's solids have already (of its own library, say) are kept.
            let own = case_models(self);
            let new = Settings {
                materials: s.materials.iter().filter(|(m, _)| !own.contains(m)).cloned().collect(),
                ..Default::default()
            };
            check_materials(&new, v)?;
        }
        let path = crate::description::PATH;
        // A key given twice: the reader takes the first, an edit would change the last.
        if let Some(block) = self.dict(path).and_then(|d| d.get(&["settings"])).and_then(|e| e.dict()) {
            let given_twice =
                |at: String| Err(Error::NewCase(format!("{path}: {at} is given twice: mend the file by hand first")));
            for (k, _) in s.entries() {
                if block.entries().filter(|e| e.key() == k).count() > 1 {
                    return given_twice(format!("settings/{k}"));
                }
            }
            if !s.materials.is_empty() {
                let blocks: Vec<&Dict> =
                    block.entries().filter(|e| e.key() == "materials").filter_map(|e| e.dict()).collect();
                if block.entries().filter(|e| e.key() == "materials").count() > 1 {
                    return given_twice("settings/materials".to_string());
                }
                for (m, _) in &s.materials {
                    if blocks.iter().flat_map(|b| b.entries()).filter(|e| e.key() == m).count() > 1 {
                        return given_twice(format!("settings/materials/{m}"));
                    }
                }
            }
        }
        let mut next = self.clone();
        let applied = next.apply_settings_to(s)?;
        let described =
            next.description().is_some() && next.resolve(path) == path && !next.aliases().any(|(_, p)| p == path);
        if described {
            for (k, v) in s.entries() {
                next.set_inline(path, &["settings", k], &v)?;
            }
            for (model, coeffs) in &s.materials {
                next.set_inline(path, &["settings", "materials", model], &coefficients(coeffs))?;
            }
        }
        *self = next;
        Ok(Chosen { applied, described })
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
