//! The workflow description (`system/umcfWorkflow`) against the case: what can't be read in it,
//! and where it and the case disagree (regions, their kinds, patches, partners, a solid's
//! materials, the version).

use std::collections::{HashMap, HashSet};

use umcf_model::blocks::{self, DEFAULT_FACES, DictState, Side};
use umcf_model::description::{CaseMaterial, PATH, RegionDesc, Role, case_materials, materials_file};
use umcf_model::{Case, RegionKind};

use crate::{Diagnostic, Level};

const CODE: &str = "workflow.description";
const BLOCKS: &str = "workflow.blocks";

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
        blocks(case, r, out);
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

/// Where a side of a grid of blocks is, seen from the gravity: around, on the ground, on top,
/// or around the missing blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Around,
    Ground,
    Top,
    Missing,
}

/// The sides on the ground and on top: those across the axis the gravity is along (the region's
/// `constant/<region>/g`, else `constant/g`; z down without either), the ground the one it
/// points to.
/// The third value: whether a `g` was read (else z is taken as up).
fn ground_and_top(case: &Case, region: &str) -> (Side, Side, bool) {
    let g = [format!("constant/{region}/g"), "constant/g".to_string()].iter().find_map(|rel| {
        let e = case.dict(rel)?.get(&["value"])?;
        let [umcf_foam_dict::Node::List { items, .. }] = e.value.as_slice() else { return None };
        let v: Vec<f64> = items
            .iter()
            .filter_map(|n| match n {
                umcf_foam_dict::Node::Atom(t) => t.text.parse::<f64>().ok(),
                _ => None,
            })
            .collect();
        match v.as_slice() {
            [x, y, z] if [x, y, z].iter().any(|c| c.is_finite() && **c != 0.0) => Some([*x, *y, *z]),
            _ => None,
        }
    });
    let read = g.is_some();
    let g = g.unwrap_or([0.0, 0.0, -1.0]);
    let axis = (0..3).max_by(|a, b| g[*a].abs().total_cmp(&g[*b].abs())).unwrap_or(2);
    let [low, high] = [[Side::West, Side::East], [Side::South, Side::North], [Side::Ground, Side::Top]][axis];
    if g[axis] < 0.0 { (low, high, read) } else { (high, low, read) }
}

/// Whether a patch of `role` belongs at `place`: an inlet, outlet or side around, the top on
/// top, the ground (or a solid's bottom) on the ground or around the missing blocks, the sky
/// around or on top; a coupled patch or a solid's interior anywhere; the canopy nowhere (it is
/// no face of the blocks).
fn fits(place: Place, role: Role) -> bool {
    match role {
        Role::Inlet | Role::Outlet | Role::Side => place == Place::Around,
        Role::Top => place == Place::Top,
        Role::Ground | Role::Bottom => matches!(place, Place::Ground | Place::Missing),
        Role::Sky => matches!(place, Place::Around | Place::Top),
        Role::Coupled | Role::Interior => true,
        Role::Canopy => false,
    }
}

/// The most warnings of one kind said one by one for a region's blocks.
const MAX_SAID: usize = 20;

/// A region's blocks: what is wrong with the grid, the patches it names against the region's,
/// and its `blockMeshDict` against it (stale, edited by hand, missing).
fn blocks(case: &Case, r: &RegionDesc, out: &mut Vec<Diagnostic>) {
    let Some(grid) = &r.blocks else { return };
    let name = &r.name;
    let warn = |path: &str, m: String| Diagnostic::new(Level::Warning, BLOCKS, path, m).region(name);
    if r.mesh.is_none() {
        out.push(warn(
            PATH,
            format!("region {name} has blocks but no mesh: give it mesh blocks; (or the steps that build it) for them to be meshed"),
        ));
    }
    let problems = grid.problems();
    for (at, m) in &problems {
        let at = if at.is_empty() { "blocks".to_string() } else { format!("blocks/{at}") };
        out.push(warn(PATH, format!("region {name}, {at}: {m}")));
    }
    // The patches it names: the region's, in a place their role fits.
    let (ground, top, by_g) = ground_and_top(case, name);
    let gravity = if by_g { "" } else { " (no g: z taken as up)" };
    let place = |side: Side| match side {
        Side::Missing => Place::Missing,
        s if s == ground => Place::Ground,
        s if s == top => Place::Top,
        _ => Place::Around,
    };
    let mut named: Vec<(Side, &str)> = grid.sides.iter().map(|(s, n)| (*s, n.as_str())).collect();
    let all = grid.boundary();
    for (n, faces) in &grid.faces {
        for f in faces {
            if let Some((cat, _)) = all.get(f) {
                named.push((*cat, n.as_str()));
            }
        }
    }
    let described: HashMap<&str, Option<Role>> = r.patches.iter().map(|p| (p.name.as_str(), p.role)).collect();
    let unknown: Vec<&str> = grid.patch_names().into_iter().filter(|n| !described.contains_key(n)).collect();
    for n in unknown.iter().take(MAX_SAID) {
        out.push(
            warn(
                PATH,
                format!("region {name}, blocks: patch {n} is not one of the region's patches in the description"),
            )
            .patch(n),
        );
    }
    if unknown.len() > MAX_SAID {
        out.push(warn(
            PATH,
            format!(
                "region {name}, blocks: and {} more patches the description doesn't list",
                unknown.len() - MAX_SAID
            ),
        ));
    }
    let mut misfits: HashSet<(&str, Side)> = HashSet::new();
    let mut said = 0usize;
    for (side, n) in named {
        if n == DEFAULT_FACES || misfits.contains(&(n, side)) {
            continue;
        }
        let Some(Some(role)) = described.get(n).copied() else { continue };
        let at = place(side);
        if !fits(at, role) {
            misfits.insert((n, side));
            said += 1;
            if said > MAX_SAID {
                continue;
            }
            let where_ = match at {
                Place::Missing => "around the missing blocks".to_string(),
                Place::Ground if side != Side::Ground => {
                    format!("on the {} side, the ground (by the gravity)", side.name())
                }
                Place::Top if side != Side::Top => format!("on the {} side, the top (by the gravity)", side.name()),
                Place::Around if matches!(side, Side::Ground | Side::Top) => {
                    format!("on the {} side, a side (by the gravity)", side.name())
                }
                _ => format!("on the {} side", side.name()),
            };
            out.push(
                warn(
                    PATH,
                    format!(
                        "region {name}, blocks: patch {n} is {where_}, where its role ({}) doesn't fit{gravity}",
                        role.name()
                    ),
                )
                .patch(n),
            );
        }
    }
    if said > MAX_SAID {
        out.push(warn(
            PATH,
            format!("region {name}, blocks: and {} more patches where their role doesn't fit", said - MAX_SAID),
        ));
    }
    if !problems.is_empty() {
        return;
    }
    let Some((path, state)) = blocks::dict_state(case, r) else { return };
    let message = match state {
        DictState::Same => return,
        DictState::Missing => format!("not there: mesh.block writes it from the blocks of region {name}"),
        DictState::NotLoaded => format!(
            "doesn't load as a dictionary: mesh.block can't write it from the blocks of region {name} until it is mended or removed"
        ),
        DictState::Differs { untouched: true, why, types_only: true } => format!(
            "written by umcfGui from region {name}'s blocks as they are, but a patch's type changed since in the blocks' types or by its role ({why}): mesh.block writes it again, listing the type it changes (one from wall or a constraint type only when its types names the patch)"
        ),
        DictState::Differs { untouched: true, why, types_only: false } => format!(
            "written by umcfGui from other blocks than region {name}'s now ({why}): mesh.block writes it again, listing any patch type it changes"
        ),
        DictState::Differs { untouched: false, why, .. } => format!(
            "not what the blocks of region {name} make ({why}): edited by hand or not written by umcfGui; mesh.block with replace writes it from them, listing any patch type it changes and any patch it drops or adds, and refusing to change a wall or constraint type (symmetryPlane, empty, …) unless the call's types names the patch, or to drop such a patch unless dropPatches names it (the one there is kept in the backup when the case is saved)"
        ),
    };
    out.push(warn(&path, message));
}
