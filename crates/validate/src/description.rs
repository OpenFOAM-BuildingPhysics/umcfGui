//! The workflow description (`system/umcfWorkflow`) against the case: what can't be read in it,
//! and where it and the case disagree (regions, their kinds, patches, partners, a solid's
//! materials, the version).

use std::collections::{HashMap, HashSet};

use umcf_model::blocks::{self, DEFAULT_FACES, DictState, Side};
use umcf_model::description::{CaseMaterial, Description, PATH, RegionDesc, Role, case_materials, materials_file};
use umcf_model::workflow::{MeshBuild, SPLIT_REST, part_name, zones_made, zones_told};
use umcf_model::{Case, RegionKind};

use crate::{Diagnostic, Level};

const CODE: &str = "workflow.description";
const BLOCKS: &str = "workflow.blocks";
const SPLIT: &str = "workflow.split";
const SNAPPY: &str = "workflow.snappy";

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
        mesh_box(case, r, out);
        split(case, &d, r, out);
        snappy(case, &d, r, out);
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

/// A region's box: one without blocks to make, problems of its own, surfaces the case doesn't
/// have.
fn mesh_box(case: &Case, r: &RegionDesc, out: &mut Vec<Diagnostic>) {
    let Some(b) = &r.mesh_box else { return };
    let name = &r.name;
    let warn = |m: String| Diagnostic::new(Level::Warning, BLOCKS, PATH, m).region(name);
    if r.blocks.is_none() {
        out.push(warn(format!(
            "region {name} has a box but no blocks: mesh.box makes them from it (with region {name})"
        )));
    }
    for (at, m) in b.problems() {
        out.push(warn(format!("region {name}, box/{at}: {m}")));
    }
    let put: HashSet<&str> = case.put_paths().collect();
    let gone: Vec<&str> = b
        .surfaces
        .iter()
        .map(String::as_str)
        .filter(|s| {
            let rel = format!("constant/triSurface/{s}");
            case.exists(&rel) == Some(false) && !put.contains(rel.as_str())
        })
        .collect();
    if !gone.is_empty() {
        out.push(warn(format!(
            "region {name}: the box is around surfaces the case doesn't have in constant/triSurface: {}",
            umcf_model::blocks::list_some(&gone)
        )));
    }
}

/// A region split from another by a cell zone: the other built before it, the zone named and
/// made by the dictionary, neither the zone nor the part outside it a region's name (the
/// scripts' clean-up removes their directories).
fn split(case: &Case, d: &Description, r: &RegionDesc, out: &mut Vec<Diagnostic>) {
    let Some(MeshBuild::Split(sp)) = &r.mesh else { return };
    let name = &r.name;
    let warn = |path: &str, m: String| Diagnostic::new(Level::Warning, SPLIT, path, m).region(name);
    let before = d.regions.iter().take_while(|x| x.name != *name).find(|x| x.name == sp.of);
    match before {
        Some(o) if o.mesh.is_some() => {}
        Some(_) => out.push(warn(
            PATH,
            format!("region {name} is split from {}, whose mesh the description doesn't build", sp.of),
        )),
        None if d.region(&sp.of).is_some() => out.push(warn(
            PATH,
            format!(
                "region {name} is split from {}, which comes after it: list {} first, its mesh is copied",
                sp.of, sp.of
            ),
        )),
        None => out.push(warn(
            PATH,
            format!("region {name} is split from {}, which is not a region of the description", sp.of),
        )),
    }
    let Some(zone) = sp.zone.as_deref() else {
        out.push(warn(
            PATH,
            format!("region {name}: the cell zone it is split by isn't named (mesh.split): no script splits it"),
        ));
        return;
    };
    for (n, what) in [(zone, "the zone"), (SPLIT_REST, "the part outside the zone")] {
        if d.region(n).is_some() || case.region(n).is_some() {
            out.push(warn(
                PATH,
                format!("region {name}: {what} is named {n}, as a region: the split's clean-up would remove that region's directories, so no script is written"),
            ));
        }
    }
    if let Some(n) = d
        .regions
        .iter()
        .map(|x| x.name.as_str())
        .chain(case.regions.iter().map(|x| x.name.as_str()))
        .find(|n| part_name(n))
    {
        out.push(warn(
            PATH,
            format!("region {name}: a region is named {n}, as splitMeshRegions names the parts outside the zone: no script splits it"),
        ));
    }
    match case.dict(&sp.dict) {
        Some(doc) => {
            if zones_told(doc).is_none() {
                out.push(warn(
                    &sp.dict,
                    format!("region {name}: which cell zones this topoSetDict makes can't be told (#include or a macro): no script splits it; write its actions out"),
                ));
            }
            let made = zones_made(doc);
            let others: Vec<&str> = made.iter().map(String::as_str).filter(|z| *z != zone).collect();
            if !others.is_empty() {
                out.push(warn(
                    &sp.dict,
                    format!("region {name} is split by the cell zone {zone}, but this topoSetDict makes others too ({}): splitMeshRegions would split them off as well, writing their meshes to constant/<zone>, so no script splits it", others.join(", ")),
                ));
            }
            if !made.iter().any(|z| z == zone) {
                let has = if made.is_empty() { "none".to_string() } else { made.join(", ") };
                out.push(warn(
                    &sp.dict,
                    format!("region {name} is split by the cell zone {zone}, which this topoSetDict doesn't make (its cellZoneSet actions: {has})"),
                ));
            }
        }
        None if case.exists(&sp.dict) == Some(true) => out.push(warn(
            &sp.dict,
            format!("region {name} is split by a zone this file makes, but it doesn't load as a dictionary"),
        )),
        None => out.push(warn(
            PATH,
            format!("region {name} is split by a zone {} makes, which the case doesn't have", sp.dict),
        )),
    }
}

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
    let (ground, top, by_g) = umcf_model::blocks::ground_and_top(case, name);
    let gravity = if by_g { "" } else { " (no g: z taken as up)" };
    let place = |side: Side| match side {
        Side::Missing => Place::Missing,
        s if s == ground => Place::Ground,
        s if s == top => Place::Top,
        _ => Place::Around,
    };
    // Each patch where it owns a face once the single faces have taken theirs: a side's patch
    // whose every face went to another isn't on that side.
    let all = grid.boundary();
    let mut pairs: HashSet<(Side, &str)> = HashSet::new();
    let named: Vec<(Side, &str)> =
        all.values().filter_map(|(c, n)| n.map(|n| (*c, n))).filter(|p| pairs.insert(*p)).collect();
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

/// A region's `snappy` entry against the case: its own problems, a mesh that doesn't run
/// snappyHexMesh, surfaces the case doesn't have, patches the description doesn't list, feature
/// edges no step makes, and its `snappyHexMeshDict` where it isn't what the entry writes.
/// The surfaces themselves are read by `mesh.snappy`, which checks them before it writes.
fn snappy(case: &Case, d: &Description, r: &RegionDesc, out: &mut Vec<Diagnostic>) {
    use umcf_model::snappy::{Mode, dict_path};
    let Some(sn) = &r.snappy else { return };
    let name = &r.name;
    let warn = |path: &str, m: String| Diagnostic::new(Level::Warning, SNAPPY, path, m).region(name);
    let problems = sn.problems();
    for (at, m) in &problems {
        let at = if at.is_empty() { "snappy".to_string() } else { format!("snappy/{at}") };
        out.push(warn(PATH, format!("region {name}, {at}: {m}")));
    }
    if r.kind == RegionKind::Solid {
        out.push(warn(
            PATH,
            format!("region {name} is a solid: snappyHexMesh doesn't mesh solids here (blocks or extrusion)"),
        ));
    }
    let path = r.mesh.as_ref().and_then(dict_path);
    if path.is_none() {
        out.push(warn(
            PATH,
            format!(
                "region {name} has a snappy entry but its mesh doesn't run snappyHexMesh: mesh.snappy gives it the step"
            ),
        ));
    }
    let put: HashSet<&str> = case.put_paths().collect();
    let mut missing = 0usize;
    for x in &sn.surfaces {
        let rel = format!("constant/triSurface/{}", x.file);
        if case.exists(&rel) == Some(false) && !put.contains(rel.as_str()) {
            missing += 1;
            if missing <= MAX_SAID {
                out.push(warn(
                    &rel,
                    format!("region {name}, snappy: surface {} reads {rel}, which the case doesn't have", x.name),
                ));
            }
        }
    }
    if missing > MAX_SAID {
        out.push(warn(
            PATH,
            format!("region {name}, snappy: and {} more surfaces the case doesn't have", missing - MAX_SAID),
        ));
    }
    let described: HashSet<&str> = r.patches.iter().map(|p| p.name.as_str()).collect();
    let patches: Vec<String> = sn.surfaces.iter().filter(|x| x.mode == Mode::Cut).flat_map(|x| x.patches()).collect();
    let unknown: Vec<&str> = patches.iter().map(String::as_str).filter(|p| !described.contains(p)).collect();
    if !unknown.is_empty() {
        out.push(warn(
            PATH,
            format!(
                "region {name}, snappy: the patches of {} aren't patches of the region in the description: without a role they are of type patch (mesh.snappy's role, or workflow.patch, gives one)",
                blocks::list_some(&unknown)
            ),
        ));
    }
    if !d.features && sn.surfaces.iter().any(|x| x.features.is_some()) {
        out.push(warn(
            PATH,
            format!("region {name}, snappy: features are asked for, but the workflow's features is no: surfaceFeatures doesn't run, and snappyHexMesh stops without the .eMesh files"),
        ));
    }
    let Some(path) = path else { return };
    if !problems.is_empty() {
        return;
    }
    let location = path.rsplit_once('/').map_or("system", |(dir, _)| dir).to_string();
    let want = sn.dict(name, &location, &blocks::type_lookup(r));
    let message = match case.dict(&path) {
        None if case.exists(&path) == Some(true) => format!(
            "doesn't load as a dictionary: mesh.snappy can't write it from the snappy entry of region {name} until it is mended or removed"
        ),
        None => format!("not there: mesh.snappy writes it from the snappy entry of region {name}"),
        Some(doc) => {
            let have = doc.to_string();
            if have == want {
                return;
            }
            if blocks::untouched(&have) {
                format!(
                    "written by umcfGui from another snappy entry (or other patch roles) than region {name}'s now: mesh.snappy writes it again"
                )
            } else {
                format!(
                    "not what the snappy entry of region {name} writes: edited by hand or not written by umcfGui; mesh.snappy with replace writes it from the entry (the one there is kept in the backup when the case is saved)"
                )
            }
        }
    };
    out.push(warn(&path, message));
}
