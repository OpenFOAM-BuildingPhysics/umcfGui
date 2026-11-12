//! `mesh.split`: a region's mesh split from another region's by a cell zone (the street
//! canyon's vegetation: the air without the trees), kept in the region's `mesh` entry of the
//! workflow description; `workflow.scripts` writes the steps that make it.

use serde_json::{Value, json};
use umcf_model::description::PATH;
use umcf_model::workflow::{SPLIT_REST, Split, part_name, zones_made, zones_told};

use super::workflow::edited;
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SPLIT: CommandSpec = CommandSpec {
    id: "mesh.split",
    label: "Split mesh",
    menu: "Mesh/Split from another region",
    shortcut: None,
    params: "region: a region of system/umcfWorkflow, whose mesh becomes the cells of another region's mesh outside a cell zone (the street canyon's vegetation: the air without the trees). of: the region whose mesh is copied, built before it (listed before it in the description). zone: the cell zone taken out, made in the copy by a topoSetDict (its cellZoneSet action of that name); not region and a number (region1, region2: what splitMeshRegions names the parts outside it), neither it nor region1 a region's name (also the region split), and no directory of the case named constant/<zone>, 0/<zone> or system/<region>/<zone> (the split writes its parts there, and the scripts remove them); refused while a region of the case or the description is named region and a number (region2: splitMeshRegions names the parts outside the zone so). dict: that topoSetDict, a path in the case (default: system/<of>/topoSetDict, the copied region's own). The region's mesh entry becomes { from split; of <of>; zone <zone>; [dict \"<dict>\";] }, in place of the mesh it had; refused while the region has blocks or a box (take them out first, mesh.block remove: true). remove: true takes the split mesh out (the region is then built by no step). workflow.scripts writes the steps into Allprepare: the copy of the mesh without its cell zones and sets, the meshes of parts an earlier split left (constant/region1/polyMesh, constant/region2/polyMesh) removed, topoSet -region <region> -dict <dict> (OpenFOAM-8: the path from the region's directory, system/../<of>/topoSetDict), splitMeshRegions -region <region> -cellZones -overwrite -noFields, a stop when the part outside the zone isn't region1 alone, the part outside the zone (constant/region1/polyMesh) moved in, and only what the split made removed (the zone's mesh, OpenFOAM-8's fvSchemes and fvSolution of each part, the cellToRegion maps; the parts' directories when empty); then the region's own steps. The outcome is workflow.region's, with the zones the dictionary makes (zones; null when the case doesn't have it) and a note when it doesn't make the zone, makes others too (splitMeshRegions -cellZones would split those off as well), isn't there, or the zones it makes can't all be told; no script splits the region then. case.save writes the description; edit.undo takes it back. The case check (workflow.split) names a split from a region not built before it, a zone not named or not made by the dictionary, and a zone or region1 that is a region's name.",
    enabled: needs_case,
    run: split,
    journal: true,
    undoable: true,
    reads_only: false,
};

/// `mesh.split`: sets or takes out a region's split mesh.
fn split(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(SPLIT.id, p, &["region", "of", "zone", "dict", "remove"])?;
    let region = p.required_str("region")?;
    let remove = p.bool("remove")?.unwrap_or(false);
    let o = s.opened_mut()?;
    if remove {
        if p.str("of")?.is_some() || p.str("zone")?.is_some() || p.str("dict")?.is_some() {
            return Err(Error::Params(format!("{}: remove: true takes no of, zone or dict", SPLIT.id)));
        }
        let r = o.case.undescribe_split(region)?;
        return edited(s, format!("{PATH}: the split mesh of region {region} taken out"), &r);
    }
    let of = p.required_str("of")?;
    let zone = p.required_str("zone")?;
    let dict = p.str("dict")?.map(str::to_string).unwrap_or_else(|| Split::default_dict(of));
    let sp = Split { of: of.to_string(), zone: Some(zone.to_string()), dict };
    for name in [zone, SPLIT_REST] {
        if o.case.region(name).is_some() || described(&o.case, name) {
            let what = if name == zone { "the cell zone" } else { "the part outside the zone" };
            return Err(Error::Failed(format!(
                "{}: {what} is named {name}, as a region: the split's clean-up would remove that region's directories",
                SPLIT.id
            )));
        }
    }
    // A region named as splitMeshRegions names the parts outside the zone: its mesh would be
    // written over.
    let regions: Vec<String> = o.case.regions.iter().map(|r| r.name.clone()).collect();
    let described_regions: Vec<String> =
        o.case.description().map(|(d, _)| d.regions.iter().map(|r| r.name.clone()).collect()).unwrap_or_default();
    if let Some(n) = regions.iter().chain(&described_regions).find(|r| part_name(r)) {
        return Err(Error::Failed(format!(
            "{}: the case has a region named {n}, as splitMeshRegions names the parts outside the zone: rename it first",
            SPLIT.id
        )));
    }
    // A directory of the zone's name the case has (not a region's): the clean-up would take its
    // mesh, or the directory when it is empty.
    for dir in [format!("constant/{zone}"), format!("0/{zone}"), format!("system/{region}/{zone}")] {
        if o.fs.is_dir(&dir) {
            return Err(Error::Failed(format!(
                "{}: the case has {dir}: the split writes and removes {zone} there, name another zone",
                SPLIT.id
            )));
        }
    }
    let zones = o.case.dict(&sp.dict).map(zones_made);
    let r = o.case.describe_split(region, &sp)?;
    let others: Vec<&str> = zones.iter().flatten().map(String::as_str).filter(|z| *z != zone).collect();
    let note = match &zones {
        Some(z) if z.iter().any(|x| x == zone) && others.is_empty() => String::new(),
        Some(z) if z.iter().any(|x| x == zone) => format!(
            "; {} makes other cell zones too ({}): no script splits it until it makes only {zone}",
            sp.dict,
            others.join(", ")
        ),
        Some(_) => format!("; {} doesn't make the cell zone {zone}", sp.dict),
        None => format!("; the case has no {}", sp.dict),
    };
    // The zones offered are what can be read; the scripts need them all told.
    let unclear = if o.case.dict(&sp.dict).is_some_and(|d| zones_told(d).is_none()) {
        format!(
            "; which cell zones {} makes can't all be told (#include, a macro, a key given twice): no script splits it",
            sp.dict
        )
    } else {
        String::new()
    };
    let note = format!("{note}{unclear}");
    let mut out = edited(s, format!("{PATH}: region {region} split from {of} by the cell zone {zone}{note}"), &r)?;
    if let Some(data) = out.data.as_object_mut() {
        data.insert("zones".into(), json!(zones));
    }
    Ok(out)
}

/// Whether the description lists `name` as a region.
fn described(case: &umcf_model::Case, name: &str) -> bool {
    case.description().is_some_and(|(d, _)| d.region(name).is_some())
}
