//! `material.*`: a solid's materials in its `transportProperties`.

use serde_json::{Value, json};

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SET: CommandSpec = CommandSpec {
    id: "material.set",
    label: "Set a material's coefficients",
    menu: "Case/Set a material's coefficients",
    shortcut: None,
    params: "region: a solid region; material: the name of one of its materials in constant/<region>/transportProperties' buildingMaterials; coefficients: {\"rho\": 1700, \"lambda1\": 0.7, \"wcap\": 2, \"mu\": null}: numbers, or null (takes it out), each a plain name in the material's dictionary, where the solver reads every coefficient (VanGenuchten's n and wcap too; none in a sub-dictionary). A coefficient there changes in place, keeping its comment and alignment; a new one goes at the end of the material's dictionary, aligned like its values (on its line, for a material written on one), with a note when the solver's catalog doesn't list it for the material's model (a typo?). Nothing else in the file changes. That material only: the case's other materials of the model, and the material library new materials start from (workflow.settings materials), stay as they are. Refused, with nothing changed: a region that isn't a solid, one without transportProperties or its buildingMaterials list, a material it doesn't list or lists twice (the solver stops on a cell zone given two materials), one whose dictionary has a macro or a directive, a name that isn't a plain word, name or buildingMaterialModel (workflow.material's), a value that isn't a number, rho, cap, lambda1 or lambda2 taken out (the solver reads them of every material), a value where the material has a dictionary. case.save writes it.",
    enabled: needs_case,
    run: set,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `material.set`: a material's coefficients.
fn set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(SET.id, p, &["region", "material", "coefficients"])?;
    let bad = |m: String| Error::Params(format!("{}: {m}", SET.id));
    let region = p.required_str("region")?;
    let material = p.required_str("material")?;
    let Some(Value::Object(given)) = p.value("coefficients") else {
        return Err(bad("coefficients is an object: {\"rho\": 1700, …}".into()));
    };
    let number = |k: &str, v: &Value| -> Result<Option<String>, Error> {
        match v {
            Value::Null => Ok(None),
            Value::Number(n) => Ok(Some(n.to_string())),
            _ => Err(bad(format!("{k} is a number, or null (takes it out)"))),
        }
    };
    let mut coeffs = Vec::new();
    for (k, v) in given {
        coeffs.push((vec![k.clone()], number(k, v)?));
    }
    let o = s.opened_mut()?;
    let e = o.case.set_material(region, material, &coeffs)?;
    let mut message = match &e.changed {
        Some(rel) => format!("material {material} of region {region}: coefficients changed in {rel}"),
        None => format!("material {material} of region {region}: as given already"),
    };
    for n in &e.notes {
        message.push_str(&format!("; {n}"));
    }
    Ok(Outcome::new(
        message,
        json!({ "region": region, "material": material, "model": e.model, "changed": e.changed, "notes": e.notes }),
    ))
}
