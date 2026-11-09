//! The workflow description (`system/umcfWorkflow`) against the case: what can't be read in it,
//! and where it and the case disagree (regions, their kinds, patches, partners, a solid's
//! materials, the version).

use std::collections::{HashMap, HashSet};

use umcf_model::description::{CaseMaterial, PATH, RegionDesc, Role, case_materials, materials_file};
use umcf_model::{Case, RegionKind};

use crate::{Diagnostic, Level};

const CODE: &str = "workflow.description";

pub(crate) fn check(case: &Case, out: &mut Vec<Diagnostic>) {
    let Some((d, problems)) = case.description() else { return };
    for p in &problems {
        out.push(Diagnostic::new(Level::Warning, CODE, PATH, format!("{}: {}", p.at, p.message)));
    }
    if let (Some(dv), Some(cv)) = (d.version, case.layout.version)
        && dv != cv
    {
        out.push(Diagnostic::new(
            Level::Warning,
            CODE,
            PATH,
            format!("it is for {dv}, the case is {cv}: the scripts written from it are for {dv}"),
        ));
    }
    for r in &d.regions {
        let Some(cr) = case.region(&r.name) else {
            out.push(
                Diagnostic::new(Level::Warning, CODE, PATH, format!("region {} is not one of the case's", r.name))
                    .region(&r.name),
            );
            continue;
        };
        if cr.kind != r.kind && cr.kind != RegionKind::Single {
            out.push(
                Diagnostic::new(
                    Level::Warning,
                    CODE,
                    PATH,
                    format!("region {} is {}, the case has it {}", r.name, kind(r.kind), kind(cr.kind)),
                )
                .region(&r.name),
            );
        }
        // What needs no mesh: partners, coupling, roles.
        for p in &r.patches {
            if let Some((pr, pp)) = &p.partner {
                let there =
                    case.region(pr).map(|x| x.patches.as_ref().is_none_or(|ps| ps.iter().any(|q| q.name == *pp)));
                if there != Some(true) {
                    out.push(
                        Diagnostic::new(
                            Level::Warning,
                            CODE,
                            PATH,
                            format!("patch {} of region {}: its partner {pr}/{pp} is not in the case", p.name, r.name),
                        )
                        .region(&r.name)
                        .patch(&p.name),
                    );
                }
            }
            if p.role == Some(Role::Coupled) && p.partner.is_none() {
                out.push(
                    Diagnostic::new(
                        Level::Warning,
                        CODE,
                        PATH,
                        format!("patch {} of region {} is coupled to no partner", p.name, r.name),
                    )
                    .region(&r.name)
                    .patch(&p.name),
                );
            }
            if p.role.is_none() {
                out.push(
                    Diagnostic::new(
                        Level::Info,
                        CODE,
                        PATH,
                        format!("patch {} of region {} has no role", p.name, r.name),
                    )
                    .region(&r.name)
                    .patch(&p.name),
                );
            }
        }
        materials(case, r, out);
        // The patches against the mesh, where the region has one.
        let Some(mesh) = cr.patches.as_ref() else { continue };
        for p in &r.patches {
            if !mesh.iter().any(|m| m.name == p.name) {
                out.push(
                    Diagnostic::new(
                        Level::Warning,
                        CODE,
                        PATH,
                        format!("patch {} of region {} is not in its mesh", p.name, r.name),
                    )
                    .region(&r.name)
                    .patch(&p.name),
                );
            }
        }
        for m in mesh {
            if m.kind != "empty" && !m.kind.starts_with("processor") && !r.patches.iter().any(|p| p.name == m.name) {
                out.push(
                    Diagnostic::new(
                        Level::Info,
                        CODE,
                        PATH,
                        format!("patch {} of region {} is not in the description", m.name, r.name),
                    )
                    .region(&r.name)
                    .patch(&m.name),
                );
            }
        }
    }
    for cr in &case.regions {
        if cr.kind != RegionKind::Single && d.region(&cr.name).is_none() {
            out.push(
                Diagnostic::new(Level::Warning, CODE, PATH, format!("region {} is not in the description", cr.name))
                    .region(&cr.name),
            );
        }
    }
}

fn kind(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Fluid => "fluid",
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Single => "the only region",
    }
}

/// A solid's materials against its `transportProperties`: each named there with the same
/// model, and each of the case's in the description; a model the solver doesn't have.
fn materials(case: &Case, r: &RegionDesc, out: &mut Vec<Diagnostic>) {
    // Another kind in the case is said already; its files are not a solid's.
    let case_kind = case.region(&r.name).map(|c| c.kind);
    if r.kind != RegionKind::Solid || case_kind.is_some_and(|k| k != RegionKind::Solid && k != RegionKind::Single) {
        return;
    }
    let diag = |level, message: String| Diagnostic::new(level, CODE, PATH, message).region(&r.name);
    let file = materials_file(&r.name);
    let case_list = case_materials(case, &r.name);
    // Maps and sets, not scans: both files may name very many materials. The case's first
    // entry of a name is the one compared, as the solver meets it first.
    let mut first: HashMap<&str, &CaseMaterial> = HashMap::new();
    let mut pairs: HashSet<(&str, &str)> = HashSet::new();
    for c in case_list.iter().flat_map(|(l, _)| l) {
        first.entry(c.name.as_str()).or_insert(c);
        if let Some(model) = &c.model {
            pairs.insert((c.name.as_str(), model.as_str()));
        }
    }
    let described: HashSet<&str> = r.materials.iter().map(|m| m.name.as_str()).collect();
    // A model the solver doesn't have, for the case's version. One the case's file gives as
    // well is the case check's to judge (`model.unknown`), not said twice.
    let cat = umcf_schema::catalog::catalog();
    let family = cat.families.iter().find(|f| f.selector == Some("buildingMaterialModel")).map(|f| f.base);
    let libs = case.dict("system/controlDict").is_some_and(|d| d.get(&["libs"]).is_some());
    for m in &r.materials {
        let Some(model) = &m.model else { continue };
        let in_case = pairs.contains(&(m.name.as_str(), model.as_str()));
        let Some(family) = family.filter(|_| !in_case) else { continue };
        let known = match (cat.find(family, model), case.layout.version) {
            (Some(t), Some(v)) => t.in_version(v),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !known {
            let version = case.layout.version.map(|v| format!(" for {v}")).unwrap_or_default();
            let also = if libs { " (unless one of the libraries in controlDict's `libs` adds it)" } else { "" };
            out.push(diag(
                Level::Warning,
                format!(
                    "material {} of region {}: {family} `{model}` is not one the solver has{version}{also}",
                    m.name, r.name
                ),
            ));
        }
    }
    let Some((listed, all)) = &case_list else {
        if !r.materials.is_empty() {
            out.push(diag(Level::Warning, format!("region {}: its materials are not listed in {file}", r.name)));
        }
        return;
    };
    for m in &r.materials {
        match first.get(m.name.as_str()) {
            // A list not read whole may name it otherwise.
            None if *all => out.push(diag(
                Level::Warning,
                format!("material {} of region {} is not in the buildingMaterials of {file}", m.name, r.name),
            )),
            None => {}
            Some(c) => {
                if let (Some(a), Some(b)) = (&m.model, &c.model)
                    && a != b
                {
                    out.push(diag(
                        Level::Warning,
                        format!("material {} of region {} is {a}, {file} has it {b}", m.name, r.name),
                    ));
                }
            }
        }
    }
    // Once a name, however often the case lists it.
    let mut said: HashSet<&str> = HashSet::new();
    for c in listed {
        if !described.contains(c.name.as_str()) && said.insert(c.name.as_str()) {
            out.push(diag(
                Level::Info,
                format!("material {} of region {} ({file}) is not in the description", c.name, r.name),
            ));
        }
    }
}
