//! The workflow description (`system/umcfWorkflow`) against the case: what can't be read in it,
//! and where it and the case disagree (regions, their kinds, patches, partners, the version).

use umcf_model::description::{PATH, Role};
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
