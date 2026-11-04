//! Boundary conditions (`bc.set`): the picked patches' entries in a field of the start time.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use umcf_schema::catalog::{TypeKind, catalog};

use super::case::diagnostic;
use super::scene::Sel;
use crate::params::{Params, foam_text, key};
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SET: CommandSpec = CommandSpec {
    id: "bc.set",
    label: "Set boundary condition",
    menu: "Boundary conditions/Set condition…",
    shortcut: None,
    params: "field: a field of the start time (T, U). type: the condition (fixedValue, zeroGradient, compressible::CFDHAMfluidTemperatureCoupledMixed). value: its value (300 → uniform 300, [2, 0, 0] → uniform (2 0 0), or OpenFOAM text). entries: other keys, {\"inletValue\": \"uniform 0\"}. defaults: keys only a new entry gets ({\"value\": \"uniform 300\"}): a kept entry keeps its own. patches: [{\"region\": \"air\", \"patch\": \"inlet\"}, …]; default the selected patches. A patch whose entry has this type already keeps it, with value and entries set in it; so does an entry without a type of its own that brings text in from an #include or a $macro; an entry kept that brings text in gets no defaults (bc.unfilled warns); otherwise its entry is replaced. Nothing else in the file changes.",
    enabled: needs_case,
    run: set,
    journal: true,
    undoable: true,
};

/// The most entries and defaults one condition takes (OpenFOAM's take a handful).
const MAX_KEYS: usize = 64;

fn set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SET.id;
    let p = Params::new(id, p, &["field", "type", "value", "entries", "defaults", "patches"])?;
    let field = p.required_str("field")?;
    if key(id, field).is_err() || field.contains(['(', ')']) {
        return Err(Error::Params(format!("{id}: `{field}` is not a field name")));
    }
    let kind = p.required_str("type")?;
    if key(id, kind).is_err() || kind.contains(['(', ')']) {
        return Err(Error::Params(format!("{id}: `{kind}` is not a boundary condition type")));
    }
    // Counted before anything is converted.
    let count = |k: &str| p.required(k).ok().and_then(Value::as_object).map_or(0, |o| o.len());
    if count("entries") + count("defaults") + usize::from(p.required("value").is_ok()) > MAX_KEYS {
        return Err(Error::Params(format!("{id}: more than {MAX_KEYS} entries and defaults")));
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(v) = p.required("entries") {
        let o = v.as_object().ok_or_else(|| Error::Params(format!("{id}: `entries` is an object")))?;
        for (k, v) in o {
            let k = key(id, k)?;
            if k == "type" || (k == "value" && p.required("value").is_ok()) {
                return Err(Error::Params(format!("{id}: `{k}` is given twice")));
            }
            entries.push((k.to_string(), value_text(id, v)?));
        }
    }
    // `value` last, as OpenFOAM writes it.
    if let Ok(v) = p.required("value") {
        entries.push(("value".to_string(), value_text(id, v)?));
    }
    let mut defaults: Vec<(String, String)> = Vec::new();
    if let Ok(v) = p.required("defaults") {
        let o = v.as_object().ok_or_else(|| Error::Params(format!("{id}: `defaults` is an object")))?;
        for (k, v) in o {
            let k = key(id, k)?;
            if k == "type" {
                return Err(Error::Params(format!("{id}: `type` is not a default")));
            }
            defaults.push((k.to_string(), value_text(id, v)?));
        }
    }
    let patches = patches(s, &p)?;
    // One edit per region: each region's field is its own file.
    let mut by_region: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (r, patch) in &patches {
        by_region.entry(r.as_str()).or_default().push(patch.as_str());
    }
    let pairs: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let fills: Vec<(&str, &str)> = defaults.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    // A uniform value has as many components as the field's type.
    for region in by_region.keys() {
        let class = s.case().and_then(|c| c.region(region)).and_then(|r| r.field(field)).and_then(|f| f.class.clone());
        if let Some(n) = class.as_deref().and_then(components) {
            for (k, v) in entries.iter().chain(&defaults) {
                if let Some(got) = uniform_components(v)
                    && got != n
                    && (k == "value" || k.ends_with("Value"))
                {
                    let class = class.as_deref().unwrap_or("");
                    return Err(Error::Params(format!(
                        "{id}: {k} `{v}` has {got} component{}; {field} is a {class} ({n})",
                        if got == 1 { "" } else { "s" }
                    )));
                }
            }
        }
    }
    let o = s.opened_mut()?;
    let mut files = Vec::new();
    let mut dropped: Vec<(String, String)> = Vec::new();
    let mut unfilled: Vec<(String, String)> = Vec::new();
    for (region, names) in &by_region {
        let report = o.case.set_bc(
            o.fs.as_ref(),
            region,
            field,
            names,
            &umcf_model::Condition { kind, entries: &pairs, defaults: &fills },
        )?;
        dropped.extend(report.replaced.into_iter().map(|p| (region.to_string(), p)));
        unfilled.extend(report.unfilled.into_iter().map(|p| (region.to_string(), p)));
        if let Some(f) = o.case.region(region).and_then(|r| r.field(field)) {
            files.push(f.path.clone());
        }
    }
    // What the case check says now about these patches' conditions.
    let mut found: Vec<Value> = Vec::new();
    for (region, patch) in &dropped {
        let path = o.case.region(region).and_then(|r| r.field(field)).map(|f| f.path.clone()).unwrap_or_default();
        found.push(json!({
            "level": "warning",
            "code": "bc.replaced",
            "path": path,
            "region": region,
            "patch": patch,
            "field": field,
            "message": "the entry it had took text from an #include or a $macro: the new entry replaces it, and that text is no longer used here",
        }));
    }
    for (region, patch) in &unfilled {
        let path = o.case.region(region).and_then(|r| r.field(field)).map(|f| f.path.clone()).unwrap_or_default();
        found.push(json!({
            "level": "warning",
            "code": "bc.unfilled",
            "path": path,
            "region": region,
            "patch": patch,
            "field": field,
            "message": "the entry brings text in from an #include or a $macro, which isn't read here: it keeps that text, and the defaults (the value shown) were not set in it. Check that it has what the type needs, or give the value explicitly",
        }));
    }
    // Entries the file brings in from elsewhere can still override the ones set here.
    for region in by_region.keys() {
        if let Some(f) = o.case.region(region).and_then(|r| r.field(field)).filter(|f| f.indirect) {
            found.push(json!({
                "level": "warning",
                "code": "bc.indirect",
                "path": f.path,
                "region": region,
                "patch": null,
                "field": field,
                "message": "boundaryField also takes entries from an #include or a $macro: one of them may set these patches after the entries set here",
            }));
        }
    }
    found.extend(
        umcf_validate::check(&o.case)
            .iter()
            .filter(|d| {
                d.field.as_deref() == Some(field)
                    && d.patch
                        .as_deref()
                        .is_some_and(|x| patches.iter().any(|(r, p)| p == x && d.region.as_deref().unwrap_or("") == r))
            })
            .map(diagnostic),
    );
    let names: Vec<String> = patches
        .iter()
        .map(|(r, p)| if r.is_empty() || by_region.len() == 1 { p.clone() } else { format!("{r}/{p}") })
        .collect();
    let message = format!("{field}: {kind} on {}", names.join(", "));
    Ok(Outcome::new(
        message,
        json!({
            "field": field,
            "type": kind,
            "patches": patches.iter().map(|(r, p)| json!({ "region": r, "patch": p })).collect::<Vec<_>>(),
            "files": files,
            "diagnostics": found,
        }),
    ))
}

/// The components of a field class's values: `volScalarField` 1, `volVectorField` 3, …;
/// `None` for a class it doesn't know (`volVector2DField`): the value isn't checked then.
fn components(class: &str) -> Option<usize> {
    let kind =
        ["vol", "surface", "point"].iter().find_map(|p| class.strip_prefix(p)).and_then(|k| k.strip_suffix("Field"))?;
    match kind {
        "Scalar" | "SphericalTensor" => Some(1),
        "Vector" => Some(3),
        "SymmTensor" => Some(6),
        "Tensor" => Some(9),
        _ => None,
    }
}

/// The components of a `uniform` value written out (`uniform 1`, `uniform (1 0 0)`); `None` for
/// anything else (`$internalField`, a list, a table).
fn uniform_components(text: &str) -> Option<usize> {
    let v = text.trim().strip_prefix("uniform")?.trim();
    match v.strip_prefix('(').and_then(|v| v.strip_suffix(')')) {
        Some(inner) => inner.split_whitespace().try_fold(0usize, |n, p| p.parse::<f64>().ok().map(|_| n + 1)),
        None => v.parse::<f64>().is_ok().then_some(1),
    }
}

/// A value: a number or a list of numbers is `uniform`, text is OpenFOAM text as given.
fn value_text(id: &str, v: &Value) -> Result<String, Error> {
    let numbers = v.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_number));
    let text = foam_text(id, v)?;
    if v.is_number() || numbers {
        return Ok(format!("uniform {text}"));
    }
    // The text goes into a dictionary entry: it must stay one value.
    let mut doc = umcf_foam_dict::Document::parse("").map_err(|e| Error::Failed(e.to_string()))?;
    let bad = || Error::Params(format!("{id}: `{text}` is not one OpenFOAM value"));
    doc.set(&["x"], &text).map_err(|_| bad())?;
    let back = umcf_foam_dict::Document::parse(&doc.to_string()).map_err(|_| bad())?;
    // A comment would swallow what follows the value in the entry written around it.
    if back.root.entries().count() != 1
        || !back.root.problems().is_empty()
        || text.contains("//")
        || text.contains("/*")
    {
        return Err(bad());
    }
    Ok(text)
}

/// The patches to set, `(region, patch)`: as given, or the selected ones.
fn patches(s: &Session, p: &Params) -> Result<Vec<(String, String)>, Error> {
    let id = SET.id;
    let case = s.case().ok_or_else(|| Error::Failed("no case is open".to_string()))?;
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
    for (region, patch) in given {
        let r = case
            .regions
            .iter()
            .find(|r| r.name == region)
            .ok_or_else(|| Error::Params(format!("{id}: no region `{region}`")))?;
        let mesh = r.patches.as_ref().ok_or_else(|| Error::Params(format!("{id}: region `{region}` has no mesh")))?;
        if !mesh.iter().any(|x| x.name == patch) {
            return Err(Error::Params(format!("{id}: region `{region}` has no patch `{patch}`")));
        }
        if !out.contains(&(region.clone(), patch.clone())) {
            out.push((region, patch));
        }
    }
    Ok(out)
}

/// OpenFOAM's own boundary conditions offered with the solver's (the catalog has only the
/// solver's): `TypeName`s of OpenFOAM's `finiteVolume` library that urban cases use, for
/// suggestions only. A type that isn't here is still set as given; the case check judges it.
const OPENFOAM: &[(&str, Applies)] = &[
    ("calculated", Applies::Any),
    ("fixedValue", Applies::Any),
    ("zeroGradient", Applies::Any),
    ("fixedGradient", Applies::Any),
    ("mixed", Applies::Any),
    ("inletOutlet", Applies::Any),
    ("uniformFixedValue", Applies::Any),
    ("slip", Applies::Any),
    ("symmetry", Applies::Any),
    ("symmetryPlane", Applies::Any),
    ("empty", Applies::Any),
    ("cyclic", Applies::Any),
    ("noSlip", Applies::Vector),
    ("pressureInletOutletVelocity", Applies::Vector),
    ("fixedFluxPressure", Applies::Scalar),
    ("totalPressure", Applies::Scalar),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applies {
    Any,
    Scalar,
    Vector,
}

impl Session {
    /// The boundary condition types to offer for a field class (`volScalarField`,
    /// `volVectorField`): OpenFOAM's common ones, then the solver's for the case's version,
    /// from the catalog.
    pub fn bc_types(&self, class: &str) -> Vec<&'static str> {
        let (scalar, vector) = (class.contains("Scalar"), class.contains("Vector"));
        let mut out: Vec<&'static str> = OPENFOAM
            .iter()
            .filter(|(_, a)| match a {
                Applies::Any => true,
                Applies::Scalar => scalar,
                Applies::Vector => vector,
            })
            .map(|(n, _)| *n)
            .collect();
        if let Some(v) = self.case().and_then(|c| c.layout.version) {
            let mut solver: Vec<&'static str> = catalog()
                .boundary_conditions(v)
                .filter(|t| match t.kind {
                    TypeKind::ScalarBoundaryCondition => scalar,
                    TypeKind::VectorBoundaryCondition => vector,
                    _ => false,
                })
                .map(|t| t.name)
                .collect();
            solver.sort_unstable();
            out.extend(solver);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn components_by_class() {
        for (class, n) in [
            ("volScalarField", Some(1)),
            ("surfaceVectorField", Some(3)),
            ("volSymmTensorField", Some(6)),
            ("pointTensorField", Some(9)),
            ("volSphericalTensorField", Some(1)),
            ("volVector2DField", None),
            ("dictionary", None),
        ] {
            assert_eq!(super::components(class), n, "{class}");
        }
        for (v, n) in [
            ("uniform 1e-3", Some(1)),
            ("uniform(1 0 0)", Some(3)),
            ("uniform ( 1 0 0 )", Some(3)),
            ("uniform $x", None),
            ("$internalField", None),
        ] {
            assert_eq!(super::uniform_components(v), n, "{v}");
        }
    }
}
