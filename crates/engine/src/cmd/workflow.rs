//! `workflow.*`: the case's workflow description, `system/umcfWorkflow`.

use serde_json::{Value, json};
use umcf_model::RegionKind;
use umcf_model::description::{self, Description, PATH};
use umcf_model::workflow::{MeshBuild, RegionStep, RootStep};

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const SHOW: CommandSpec = CommandSpec {
    id: "workflow.show",
    label: "Workflow description",
    menu: "Case/Workflow description",
    shortcut: None,
    params: "none. Returns the case's workflow description: system/umcfWorkflow as read (source \"file\", with the problems in it), or, when the case has none, as workflow.import would write it from the scripts and meshes (source \"scripts\", with what the import couldn't tell; \"unreadable\" when the file is there but isn't a dictionary, \"disk\" when it is on disk but not in the open case, after an undo past a save). Each region: kind (fluid, solid, vegetation), mesh (\"blocks\", or the steps that build it in constant/polyMesh), steps, and patches with their role (inlet, outlet, side, top, ground, coupled, canopy, interior, bottom, sky) and partner.",
    enabled: needs_case,
    run: show,
    journal: false,
    undoable: false,
};

fn show(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(SHOW.id, p, &[])?;
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let elsewhere = if case.description().is_none() { on_disk(case, fs) } else { OnDisk::None };
    let (d, source, problems, notes) = match case.description() {
        Some((d, problems)) => (d, "file", problems, Vec::new()),
        None => {
            let (d, notes) = Description::import(case);
            let source = match elsewhere {
                OnDisk::None => "scripts",
                OnDisk::Unreadable => "unreadable",
                OnDisk::NotOpen => "disk",
            };
            (d, source, Vec::new(), notes)
        }
    };
    let message = match source {
        "file" => format!("{PATH}: {}, {}", count(d.regions.len(), "region"), count(problems.len(), "problem")),
        "unreadable" => format!(
            "{PATH} is there but doesn't read as a dictionary: as the scripts and meshes have it, {}",
            count(d.regions.len(), "region")
        ),
        "disk" => format!(
            "{PATH} is on disk but not in the open case (an undo took it back after it was saved; open the case again to read it): as the scripts and meshes have it, {}",
            count(d.regions.len(), "region")
        ),
        _ => format!(
            "no {PATH}: as the scripts and meshes have it, {}, {}",
            count(d.regions.len(), "region"),
            count(notes.len(), "note")
        ),
    };
    let data = json!({
        "file": PATH,
        "source": source,
        "description": to_json(&d),
        "problems": problems.iter().map(|p| json!({ "at": p.at, "message": p.message })).collect::<Vec<_>>(),
        "notes": notes,
    });
    Ok(Outcome::new(message, data))
}

pub(super) const IMPORT: CommandSpec = CommandSpec {
    id: "workflow.import",
    label: "Write workflow description",
    menu: "Case/Write workflow description",
    shortcut: None,
    params: "replace: true to write over the case's system/umcfWorkflow. Writes the workflow description from the case: its scripts (Allprepare, Allrun) read into the workflow, the region kinds, and each patch's role and partner as its mesh and velocity condition tell. What it couldn't tell (a patch without a role, a region the scripts don't mesh, a step that doesn't fit) is listed in notes. case.save writes it.",
    enabled: needs_case,
    run: import,
    journal: true,
    undoable: true,
};

fn import(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(IMPORT.id, p, &["replace"])?;
    let replace = p.bool("replace")?.unwrap_or(false);
    let o = s.opened_mut()?;
    let elsewhere = if o.case.description().is_none() { on_disk(&o.case, o.fs.as_ref()) } else { OnDisk::None };
    let (d, notes) = Description::import(&o.case);
    if d.regions.is_empty() {
        return Err(Error::Failed(format!("{}: the case has no regions to describe", IMPORT.id)));
    }
    o.case.put_description(o.fs.as_ref(), &d.write(), replace).map_err(|e| match e {
        umcf_model::Error::Exists(_) => {
            Error::Failed(format!("{PATH} is there already: pass \"replace\": true to write over it"))
        }
        umcf_model::Error::NotLoaded { .. } => match elsewhere {
            OnDisk::NotOpen => Error::Failed(format!(
                "{PATH} is on disk but not in the open case (an undo took it back after it was saved): open the case again first"
            )),
            _ => Error::Failed(format!(
                "{PATH} is there but doesn't read as a dictionary: mend or remove it first (it isn't written over)"
            )),
        },
        e => e.into(),
    })?;
    let message =
        format!("{PATH}: written from the case, {}, {}", count(d.regions.len(), "region"), count(notes.len(), "note"));
    Ok(Outcome::new(message, json!({ "file": PATH, "description": to_json(&d), "notes": notes })))
}

/// What is on disk at the description's path when the open case holds none.
enum OnDisk {
    None,
    /// Not a dictionary.
    Unreadable,
    /// A dictionary the open case doesn't hold (taken back by an undo after a save).
    NotOpen,
}

fn on_disk(case: &umcf_model::Case, fs: &dyn umcf_core::CaseFs) -> OnDisk {
    match case.file(fs, PATH) {
        Ok(Some(bytes)) => match umcf_foam_dict::Document::parse_bytes(&bytes) {
            Ok(_) => OnDisk::NotOpen,
            Err(_) => OnDisk::Unreadable,
        },
        Ok(None) => OnDisk::None,
        Err(_) => OnDisk::Unreadable,
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// The description as JSON, as `workflow.show` returns it.
pub(crate) fn to_json(d: &Description) -> Value {
    let regions: Vec<Value> = d
        .regions
        .iter()
        .map(|r| {
            let kind = match r.kind {
                RegionKind::Solid => "solid",
                RegionKind::Vegetation => "vegetation",
                RegionKind::Fluid | RegionKind::Single => "fluid",
            };
            let mesh = match &r.mesh {
                None => Value::Null,
                Some(MeshBuild::Block) => json!("blocks"),
                Some(MeshBuild::Root(steps)) => steps
                    .iter()
                    .map(|st| match st {
                        RootStep::BlockMesh(d) => json!({ "step": "blockMesh", "dict": d }),
                        RootStep::CreatePatch => json!({ "step": "createPatch" }),
                        RootStep::SnappyHexMesh(d) => json!({ "step": "snappyHexMesh", "dict": d }),
                    })
                    .collect(),
            };
            let steps: Vec<Value> = r
                .steps
                .iter()
                .map(|st| match st {
                    RegionStep::CreatePatch => json!({ "step": "createPatch" }),
                    RegionStep::ChangeDictionary => json!({ "step": "changeDictionary" }),
                    RegionStep::Sets { setset_batch: None } => json!({ "step": "topoSet" }),
                    RegionStep::Sets { setset_batch: Some(b) } => json!({ "step": "setSet", "batch": b }),
                    RegionStep::SetFields { restore } => json!({
                        "step": "setFields",
                        "restore": restore.iter().map(|(b, f)| json!({ "backup": b, "field": f })).collect::<Vec<_>>(),
                    }),
                })
                .collect();
            let patches: Vec<Value> = r
                .patches
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "role": p.role.map(description::Role::name),
                        "partner": p.partner.as_ref().map(|(r, q)| json!({ "region": r, "patch": q })),
                    })
                })
                .collect();
            json!({ "name": r.name, "kind": kind, "mesh": mesh, "steps": steps, "patches": patches })
        })
        .collect();
    json!({
        "version": d.version.map(|v| v.label()),
        "regions": regions,
        "features": d.features,
        "radiation": d.radiation.as_ref().map(|r| json!({
            "region": r.region,
            "lai": r.lai_region,
            "viewFactors": r.view_factors,
            "solarRayTracing": r.solar_ray_tracing,
        })),
        "solver": d.solver,
        "reconstruct": d.reconstruct,
    })
}
