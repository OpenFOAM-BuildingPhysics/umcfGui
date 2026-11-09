//! `region.*`: the case's regions.

use serde_json::{Value, json};
use umcf_model::RegionKind;
use umcf_model::description::MaterialDesc;
use umcf_model::new_case::NewRegion;

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const ADD: CommandSpec = CommandSpec {
    id: "region.add",
    label: "Add a region…",
    menu: "Case/Add a region…",
    shortcut: None,
    params: "region: its name (required; a plain name that no region of the case has, nor one differing only by case, nor a file of constant/ or system/; the vegetation region is named vegetation, as the solver looks it up). kind: fluid, solid or vegetation (required). materials: a solid's, [{\"name\": \"brick\", \"model\": \"Hamstad5Brick\"}] (each the cell zone built of it, its model the solver's or one the case's solids have already; without them, those the workflow description gives it). Lists the region where the case's version reads it (OpenFOAM-12's controlDict, OpenFOAM-8's constant/regionProperties) and writes the settings of its kind as case.new does (docs/user-guide/new-case.md), the configuration's: a fluid's with the gravity of the case's fluids, the temperature limit when the case has one, and the RAS model and limit bounds of the description's settings, else of the case's fluids when they agree (a note when they don't: the configuration's); a solid's materials with the coefficients of the tutorials' material of their model; for the vegetation, air's vegetationProperties. A file that is there already is kept. The region has no mesh yet; system/umcfWorkflow describes it when the case has one. case.save writes it all.",
    enabled: needs_case,
    run: add,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `region.add`: a region added to the case.
fn add(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(ADD.id, p, &["region", "kind", "materials"])?;
    let bad = |m: &str| Error::Params(format!("{}: {m}", ADD.id));
    let name = p.required_str("region")?;
    let kind = match p.required_str("kind")? {
        "fluid" => RegionKind::Fluid,
        "solid" => RegionKind::Solid,
        "vegetation" => RegionKind::Vegetation,
        _ => return Err(bad("kind is fluid, solid or vegetation")),
    };
    let mut materials = Vec::new();
    match p.value("materials") {
        None => {}
        Some(Value::Array(ms)) => {
            for m in ms {
                let m = Params::new(ADD.id, m, &["name", "model"])?;
                materials.push(MaterialDesc {
                    name: m.required_str("name")?.to_string(),
                    model: Some(m.required_str("model")?.to_string()),
                });
            }
        }
        Some(_) => return Err(bad("materials is a list of {\"name\", \"model\"}")),
    }
    let region = NewRegion { name: name.to_string(), kind, materials };
    let o = s.opened_mut()?;
    let added = o.case.add_region(o.fs.as_ref(), &region)?;
    let word = match kind {
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Fluid | RegionKind::Single => "fluid",
    };
    let mut message =
        format!("region {name} added, {word}: listed in {}, {} files written", added.listed_in, added.written.len());
    if !added.kept.is_empty() {
        message.push_str(&format!(", {} kept as they were", added.kept.len()));
    }
    if added.described {
        message.push_str(", described in system/umcfWorkflow");
    }
    for n in &added.notes {
        message.push_str("; ");
        message.push_str(n);
    }
    Ok(Outcome::new(
        message,
        json!({
            "region": name,
            "kind": word,
            "listedIn": added.listed_in,
            "written": added.written,
            "kept": added.kept,
            "described": added.described,
            "notes": added.notes,
        }),
    ))
}
