//! The case's workflow description, `system/umcfWorkflow` (see `docs/common-workflow.md`): an
//! OpenFOAM-style dictionary that travels with the case. It names every region with its kind,
//! how its mesh is built and the steps after, the patches with their roles and partners, a
//! solid's materials, the radiation pre-processing and the solver.
//!
//! [`Description::import`] makes one from a case (its scripts, read into a
//! [`crate::workflow::Workflow`], and its meshes); [`Description::write`] gives the file's text;
//! [`Description::read`] reads a file back, naming each problem; [`Description::workflow`] is the
//! workflow the scripts are written from.
//!
//! ```c++
//! version     12;
//! regions
//! {
//!     air
//!     {
//!         kind        fluid;
//!         mesh        ( blockMesh "system/blockMeshDict.air" createPatch snappyHexMesh "system/snappyHexMeshDict.air" );
//!         steps       ( changeDictionary topoSet setFields ( ( "0/air.bckp/LAD" LAD ) ) );
//!         patches
//!         {
//!             west        { role inlet; }
//!             street      { role coupled; partner ( street street ); }
//!         }
//!     }
//!     street
//!     {
//!         kind        solid;
//!         mesh        blocks;
//!         …
//!         materials
//!         {
//!             soil        { model Soil; }
//!             brick       { model Hamstad5Brick; }
//!         }
//!     }
//! }
//! radiation   { region vegetation; lai air; viewFactors yes; solarRayTracing yes; }
//! components  ( grass temperatureLimit );
//! solver      urbanMicroclimateFoam;
//! reconstruct yes;
//! ```

use std::collections::HashSet;
use std::fmt::Write as _;

use umcf_core::OfVersion;
use umcf_foam_dict::{Document, Entry, Node, unquote};

use crate::blocks::BlockGrid;
use crate::mesh_box::MeshBox;
use crate::workflow::{MeshBuild, Radiation, RegionMesh, RegionStep, RootStep, Split, Workflow};
use crate::{Case, Patch, RegionKind};

/// Where the description is.
pub const PATH: &str = "system/umcfWorkflow";

const BLOCK_DICT: &str = "system/blockMeshDict";
const SNAPPY_DICT: &str = "system/snappyHexMeshDict";

/// What a patch is for: its type, partner and boundary conditions follow from it (the coupling
/// rules of `docs/case-requirements.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inlet,
    Outlet,
    /// A side of the domain the wind passes along (`slip`, `symmetry`), or a solid's side.
    Side,
    Top,
    Ground,
    /// Coupled to a patch of another region (`mappedWall`): a fluid's wall to a solid, or the
    /// vegetation region's twin of such an air patch.
    Coupled,
    /// The vegetation region's patch facing the trees (`air_to_vegetation`).
    Canopy,
    /// A solid's inner side (the room).
    Interior,
    /// A solid's bottom (the ground's lower end).
    Bottom,
    /// The radiation region's open sides and top.
    Sky,
}

impl Role {
    pub const ALL: [Role; 10] = [
        Role::Inlet,
        Role::Outlet,
        Role::Side,
        Role::Top,
        Role::Ground,
        Role::Coupled,
        Role::Canopy,
        Role::Interior,
        Role::Bottom,
        Role::Sky,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Role::Inlet => "inlet",
            Role::Outlet => "outlet",
            Role::Side => "side",
            Role::Top => "top",
            Role::Ground => "ground",
            Role::Coupled => "coupled",
            Role::Canopy => "canopy",
            Role::Interior => "interior",
            Role::Bottom => "bottom",
            Role::Sky => "sky",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatchDesc {
    pub name: String,
    /// `None`: not known (the import couldn't tell; listed in its notes).
    pub role: Option<Role>,
    /// The region and patch it is coupled to.
    pub partner: Option<(String, String)>,
}

/// A solid's material: the cell zone of its name is built of it
/// (`solid/updatebuildingMaterials.H`), its coefficients in the region's `transportProperties`.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialDesc {
    pub name: String,
    /// Its `buildingMaterialModel` (`Hamstad5Brick`); `None`: not given.
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionDesc {
    pub name: String,
    /// Fluid, solid or vegetation (never [`RegionKind::Single`]).
    pub kind: RegionKind,
    /// How the mesh is built; `None` when the workflow doesn't build it.
    pub mesh: Option<MeshBuild>,
    /// The steps after the mesh.
    pub steps: Vec<RegionStep>,
    pub patches: Vec<PatchDesc>,
    /// A solid's materials, in the order of its `buildingMaterials`; none for other kinds.
    pub materials: Vec<MaterialDesc>,
    /// The grid of blocks its `blockMeshDict` is written from ([`crate::blocks`]); `None`: not
    /// described (the dictionary is the case's own).
    pub blocks: Option<BlockGrid>,
    /// The box around the case's surfaces its blocks were made from ([`crate::mesh_box`]);
    /// `None`: none (the blocks are given as they are, or there are none).
    pub mesh_box: Option<MeshBox>,
}

/// The components a case has that its regions don't tell (the air, the solids' heat and
/// moisture and the vegetation are its regions' kinds, the radiation is
/// [`Description::radiation`]). Each is switched on in the solver by a file or an entry of its
/// own (`umcf_schema::defaults`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Components {
    /// `grassProperties` in a fluid region, its `grassModel` not `none` nor `grass` off.
    pub grass: bool,
    /// A `limitTemperature` entry where the solver reads it ([`crate::settings::limit_file`]).
    pub temperature_limit: bool,
    /// The blending layer: `blending` in `controlDict`.
    pub blending: bool,
}

impl Components {
    /// The words of the description's `components` list, in its order, with whether each is
    /// on.
    pub fn words(&self) -> [(&'static str, bool); 3] {
        [("grass", self.grass), ("temperatureLimit", self.temperature_limit), ("blending", self.blending)]
    }

    /// Switches the component `word` names on; `false` when there is no such component.
    pub fn switch_on(&mut self, word: &str) -> bool {
        match word {
            "grass" => self.grass = true,
            "temperatureLimit" => self.temperature_limit = true,
            "blending" => self.blending = true,
            _ => return false,
        }
        true
    }
}

/// A case's `system/umcfWorkflow`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Description {
    /// The OpenFOAM version the scripts and dictionaries are for.
    pub version: Option<OfVersion>,
    /// In the order their meshes are built (regions without one last).
    pub regions: Vec<RegionDesc>,
    /// `surfaceFeatures` runs first.
    pub features: bool,
    pub radiation: Option<Radiation>,
    pub components: Components,
    /// The settings the case chose (`settings`); what isn't there is the default.
    pub settings: crate::settings::Settings,
    /// The solver run in parallel; `None`: the scripts run none.
    pub solver: Option<String>,
    pub reconstruct: bool,
}

/// One problem in a description file: where (`regions/air/mesh`) and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub at: String,
    pub message: String,
}

impl Description {
    /// The description of `case` as its scripts and meshes have it, and notes on what the
    /// import couldn't tell (a patch without a known role, a region its scripts don't mesh, a
    /// step that doesn't fit).
    pub fn import(case: &Case) -> (Description, Vec<String>) {
        let wf = case.workflow();
        let mut notes = Vec::new();
        let mut order: Vec<&str> = wf.meshes.iter().map(|m| m.region.as_str()).collect();
        for r in &wf.regions {
            if !order.contains(&r.as_str()) {
                order.push(r);
            }
        }
        let air_roles = air_roles(case);
        let mut regions = Vec::new();
        for name in order {
            let Some(region) = case.region(name) else {
                notes.push(format!("region {name}: the scripts mesh it, but the case doesn't list it: left out"));
                continue;
            };
            if region.kind == RegionKind::Single {
                continue;
            }
            let mesh = wf.meshes.iter().find(|m| m.region == name);
            if mesh.is_none() {
                notes.push(format!("region {name}: the scripts don't build its mesh"));
            }
            let mut patches = Vec::new();
            for p in region.patches.iter().flatten() {
                if p.kind == "empty" {
                    continue;
                }
                let u = region
                    .field("U")
                    .and_then(|f| f.bcs.as_ref())
                    .and_then(|bcs| bcs.iter().find(|b| b.patch == p.name && !b.pattern))
                    .and_then(|b| b.kind.as_deref());
                let (role, partner) = infer(case, region.kind, p, u, &air_roles);
                if role.is_none() {
                    notes.push(format!("region {name}, patch {}: no role could be told ({})", p.name, p.kind));
                }
                patches.push(PatchDesc { name: p.name.clone(), role, partner });
            }
            let mut materials = Vec::new();
            if region.kind == RegionKind::Solid {
                match case_materials(case, name) {
                    Some((list, all)) => {
                        if !all {
                            notes.push(format!(
                                "region {name}: a material of its buildingMaterials isn't one plain entry with a plain name (a macro, a directive): left out"
                            ));
                        }
                        // Sets, not scans: the list may be very long.
                        let mut kept: HashSet<String> = HashSet::new();
                        let mut twice: HashSet<String> = HashSet::new();
                        for m in list {
                            if kept.contains(&m.name) {
                                // One note a name, however often it is listed again.
                                if !twice.contains(&m.name) {
                                    notes.push(format!(
                                        "region {name}, material {}: listed more than once: the first kept, the others (and their models) left out",
                                        m.name
                                    ));
                                    twice.insert(m.name);
                                }
                                continue;
                            }
                            kept.insert(m.name.clone());
                            if m.model.is_none() && m.model_given {
                                notes.push(format!(
                                    "region {name}, material {}: its buildingMaterialModel isn't a plain name (a macro, a directive): left out",
                                    m.name
                                ));
                            }
                            materials.push(MaterialDesc { name: m.name, model: m.model });
                        }
                    }
                    None => notes.push(format!("region {name}: no buildingMaterials list in {}", materials_file(name))),
                }
            }
            let blocks = mesh.and_then(|m| import_blocks(case, name, region.kind, &m.build, &patches, &mut notes));
            // A region without a mesh: its patches are those its blocks name, without a role.
            if region.patches.is_none()
                && let Some(g) = &blocks
            {
                for n in g.patch_names() {
                    if !patches.iter().any(|p| p.name == n) {
                        patches.push(PatchDesc { name: n.to_string(), role: None, partner: None });
                    }
                }
            }
            regions.push(RegionDesc {
                name: name.to_string(),
                kind: region.kind,
                mesh: mesh.map(|m| m.build.clone()),
                steps: mesh.map(|m| m.steps.clone()).unwrap_or_default(),
                patches,
                materials,
                blocks,
                mesh_box: None,
            });
        }
        // A blockMeshDict two regions' meshes read: written from one's blocks, it would change
        // the other's.
        let mut readers: std::collections::BTreeMap<String, Vec<&str>> = std::collections::BTreeMap::new();
        for r in &regions {
            if let Some(p) = r.mesh.as_ref().and_then(|m| crate::blocks::dict_path(&r.name, m)) {
                readers.entry(p).or_default().push(r.name.as_str());
            }
        }
        for (p, rs) in readers.iter().filter(|(_, rs)| rs.len() > 1) {
            notes.push(format!(
                "{p} is read by the meshes of regions {}: mesh.block writes it for none of them until each has its own",
                rs.join(", ")
            ));
        }
        for u in &wf.unmatched {
            if !u.redundant {
                notes.push(format!("{} line {}: not in the description: {}", u.script, u.line, u.what));
            }
        }
        let (settings, settings_notes) = crate::settings::Settings::from_case(case);
        notes.extend(settings_notes);
        let d = Description {
            version: case.layout.version,
            components: case_components(case),
            settings,
            regions,
            features: wf.features,
            radiation: wf.radiation.clone(),
            solver: wf.solver.clone(),
            reconstruct: wf.reconstruct,
        };
        (d, notes)
    }

    /// The workflow the scripts are written from ([`Workflow::generate`]). The process counts
    /// and the files copied into `processor*/` come from the case, not the description.
    pub fn workflow(&self) -> Workflow {
        let mut wf = Workflow::default();
        wf.regions = self.regions.iter().map(|r| r.name.clone()).collect();
        wf.features = self.features;
        wf.meshes = self
            .regions
            .iter()
            .filter_map(|r| {
                let build = r.mesh.clone()?;
                Some(RegionMesh { region: r.name.clone(), build, steps: r.steps.clone() })
            })
            .collect();
        wf.radiation = self.radiation.clone();
        wf.solver = self.solver.clone();
        wf.reconstruct = self.reconstruct;
        wf
    }

    /// The scripts (`Allprepare`, `Allrun`, `Allclean`) written from the description for
    /// `version`, with what only the case can tell: the dictionaries in each described region's
    /// `constant/<region>/` that Allrun copies into `processor*/`. A name that isn't safe in a
    /// script is refused ([`Workflow::generate`]).
    pub fn scripts(
        &self,
        case: &Case,
        version: OfVersion,
    ) -> std::result::Result<Vec<(&'static str, String)>, crate::workflow::ScriptError> {
        let mut wf = self.workflow();
        wf.constant_files =
            case.workflow().constant_files.into_iter().filter(|(r, _)| self.region(r).is_some()).collect();
        wf.case_regions = case.regions.iter().map(|r| r.name.clone()).collect();
        wf.split_zones = self
            .regions
            .iter()
            .filter_map(|r| match &r.mesh {
                Some(MeshBuild::Split(sp)) => {
                    use crate::workflow::DictZones;
                    let zones = match case.dict(&sp.dict) {
                        Some(doc) => crate::workflow::zones_told(doc).map_or(DictZones::Unclear, DictZones::Made),
                        None if case.exists(&sp.dict) == Some(false) => DictZones::Missing,
                        // There, or not known not to be, but not read.
                        None => DictZones::Unclear,
                    };
                    Some((r.name.clone(), zones))
                }
                _ => None,
            })
            .collect();
        wf.generate(version)
    }

    pub fn region(&self, name: &str) -> Option<&RegionDesc> {
        self.regions.iter().find(|r| r.name == name)
    }

    /// The file's text.
    pub fn write(&self) -> String {
        let mut s = String::from(
            "/*--------------------------------*- C++ -*----------------------------------*\\\n\
             | umcfGui: the case's workflow (docs/common-workflow.md). Edit it with umcfGui's      |\n\
             | commands or by hand; the scripts and dictionaries are written from it.            |\n\
             \\*---------------------------------------------------------------------------*/\n\
             FoamFile\n{\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      umcfWorkflow;\n}\n\n",
        );
        if let Some(v) = self.version {
            let _ = writeln!(s, "version     {};\n", number(v));
        }
        s.push_str("regions\n{\n");
        for r in &self.regions {
            let _ = writeln!(s, "    {}\n    {{", word(&r.name));
            let _ = writeln!(s, "        kind        {};", kind_name(r.kind));
            match &r.mesh {
                None => {}
                Some(MeshBuild::Block) => s.push_str("        mesh        blocks;\n"),
                Some(MeshBuild::Root(steps)) => {
                    let _ = writeln!(s, "        mesh        ( {} );", root_steps(steps));
                }
                Some(MeshBuild::Split(sp)) => {
                    let _ = write!(s, "        mesh        {{ from split; of {};", word(&sp.of));
                    if let Some(z) = &sp.zone {
                        let _ = write!(s, " zone {};", word(z));
                    }
                    if sp.dict != Split::default_dict(&sp.of) {
                        let _ = write!(s, " dict {};", quote(&sp.dict));
                    }
                    s.push_str(" }\n");
                }
            }
            if let Some(g) = &r.blocks {
                s.push_str(&g.entry("        "));
            }
            if let Some(b) = &r.mesh_box {
                s.push_str(&b.entry("        "));
            }
            if !r.steps.is_empty() {
                let _ = writeln!(s, "        steps       ( {} );", region_steps(&r.steps));
            }
            if !r.patches.is_empty() {
                s.push_str("        patches\n        {\n");
                for p in &r.patches {
                    let mut body = String::new();
                    if let Some(role) = p.role {
                        let _ = write!(body, " role {};", role.name());
                    }
                    if let Some((region, patch)) = &p.partner {
                        let _ = write!(body, " partner ( {} {} );", word(region), word(patch));
                    }
                    let _ = writeln!(s, "            {:<15} {{{body} }}", word(&p.name));
                }
                s.push_str("        }\n");
            }
            if !r.materials.is_empty() {
                s.push_str("        materials\n        {\n");
                for m in &r.materials {
                    let body = m.model.as_ref().map(|x| format!(" model {};", word(x))).unwrap_or_default();
                    let _ = writeln!(s, "            {:<15} {{{body} }}", word(&m.name));
                }
                s.push_str("        }\n");
            }
            s.push_str("    }\n");
        }
        s.push_str("}\n\n");
        s.push_str("features    ");
        s.push_str(yes(self.features));
        s.push_str(";\n");
        if let Some(rad) = &self.radiation {
            let _ = write!(s, "radiation   {{ region {};", word(&rad.region));
            if let Some(l) = &rad.lai_region {
                let _ = write!(s, " lai {};", word(l));
            }
            let _ = writeln!(
                s,
                " viewFactors {}; solarRayTracing {}; }}",
                yes(rad.view_factors),
                yes(rad.solar_ray_tracing)
            );
        }
        let on: Vec<&str> = self.components.words().into_iter().filter(|(_, on)| *on).map(|(w, _)| w).collect();
        if !on.is_empty() {
            let _ = writeln!(s, "components  ( {} );", on.join(" "));
        }
        let settings = self.settings.write();
        if !settings.is_empty() {
            s.push('\n');
            s.push_str(&settings);
        }
        match &self.solver {
            Some(app) => {
                let _ = writeln!(s, "solver      {};", word(app));
            }
            None => s.push_str("solver      none;\n"),
        }
        let _ = writeln!(s, "reconstruct {};", yes(self.reconstruct));
        s.push_str("\n// ************************************************************************* //\n");
        s
    }

    /// A description file read back. What can't be read is a [`Problem`] and left out; the rest
    /// is read.
    pub fn read(doc: &Document) -> (Description, Vec<Problem>) {
        let mut d = Description::default();
        let mut problems = Vec::new();
        let mut problem = |at: &str, message: String| problems.push(Problem { at: at.to_string(), message });
        // Sets, not lists: a file near the size limit has many names, each checked once.
        let mut seen: HashSet<&str> = HashSet::new();
        for e in doc.root.entries() {
            let key = e.key();
            if !seen.insert(key) && key != "FoamFile" {
                problem(key, "is given twice: the later ones are left out".into());
                continue;
            }
            match key {
                "FoamFile" => {}
                "version" => match single(e).as_deref() {
                    Some("8") => d.version = Some(OfVersion::V8),
                    Some("12") => d.version = Some(OfVersion::V12),
                    _ => problem(key, "is 8 or 12".into()),
                },
                // Written by `case.new` before there was one configuration: read, and left out.
                "profile" => {}
                "settings" => match e.dict() {
                    Some(body) => d.settings = crate::settings::Settings::read(body, &mut problem),
                    None => problem(key, "is a dictionary".into()),
                },
                "components" => match list(e) {
                    Some(items) => {
                        let mut seen = HashSet::new();
                        for n in items {
                            match atom(n).map(unescaped) {
                                Some(w) if !seen.insert(w.clone()) => {
                                    problem(&format!("components/{w}"), "is given twice".into())
                                }
                                Some(w) if d.components.switch_on(&w) => {}
                                Some(w) => problem(
                                    &format!("components/{w}"),
                                    "is not a component (grass, temperatureLimit, blending)".into(),
                                ),
                                None => problem(key, "is a list of words".into()),
                            }
                        }
                    }
                    None => problem(key, "is a list: ( grass temperatureLimit blending ), or some of them".into()),
                },
                "features" => match flag(e) {
                    Some(b) => d.features = b,
                    None => problem(key, "is yes or no".into()),
                },
                "reconstruct" => match flag(e) {
                    Some(b) => d.reconstruct = b,
                    None => problem(key, "is yes or no".into()),
                },
                "solver" => match single(e).as_deref() {
                    Some("none") => d.solver = None,
                    Some(app) => d.solver = Some(app.to_string()),
                    None => problem(key, "is the solver's name, or none".into()),
                },
                "radiation" => match e.dict() {
                    Some(body) => d.radiation = read_radiation(body, &mut problem),
                    None => problem(key, "is a dictionary".into()),
                },
                "regions" => match e.dict() {
                    Some(body) => {
                        let mut names: HashSet<String> = HashSet::new();
                        for r in body.entries() {
                            let rname = entry_name(r);
                            let at =
                                if rname.is_empty() { "regions/\"\"".to_string() } else { format!("regions/{rname}") };
                            if rname.is_empty() {
                                problem(&at, "is not a name".into());
                                continue;
                            }
                            if !names.insert(rname.clone()) {
                                problem(&at, "is given twice: the later ones are left out".into());
                                continue;
                            }
                            match r.dict() {
                                Some(rb) => {
                                    if let Some(region) = read_region(&rname, rb, &at, &mut problem) {
                                        d.regions.push(region);
                                    }
                                }
                                None => problem(&at, "is a dictionary".into()),
                            }
                        }
                    }
                    None => problem(key, "is a dictionary".into()),
                },
                _ => problem(key, "is not a key of the description".into()),
            }
        }
        if !doc.root.items.is_empty() && doc.get(&["regions"]).is_none() {
            problem("regions", "is missing".into());
        }
        (d, problems)
    }
}

/// The components the case's files switch on (see [`Components`]): a fluid region's
/// `grassProperties` (its model not `none`, `grass` not off), a `limitTemperature` entry in the
/// file a fluid's limit is read from ([`crate::settings::limit_file`]), `blending` on in
/// `controlDict`.
pub(crate) fn case_components(case: &Case) -> Components {
    let mut c = Components::default();
    let fluids = case.regions.iter().filter(|r| r.kind == RegionKind::Fluid);
    for r in fluids {
        // `grassModel none` is the solver's noGrass, and `grass off` switches any model off.
        let rel = format!("constant/{}/grassProperties", r.name);
        let doc = case.dict(&rel);
        let off = doc.and_then(|d| d.value(&["grassModel"])).as_deref() == Some("none")
            || doc.and_then(|d| d.value(&["grass"])).is_some_and(|w| umcf_foam_dict::switch(&w) == Some(false));
        // A file made in this session counts before it is saved.
        if (doc.is_some() || case.exists(&rel) == Some(true)) && !off {
            c.grass = true;
        }
        // Where the solver reads it from (`settings::limit_file`).
        c.temperature_limit |= !crate::settings::limit_entries(case, &r.name).is_empty();
    }
    // Read as the solver reads it (`lookupOrDefault<bool>`), not as the description's yes/no.
    c.blending = case
        .dict(crate::CONTROL_DICT)
        .and_then(|d| d.get(&["blending"]))
        .and_then(|e| e.words())
        .is_some_and(|w| matches!(w.as_slice(), [v] if umcf_foam_dict::switch(v) == Some(true)));
    c
}

fn number(v: OfVersion) -> u32 {
    match v {
        OfVersion::V8 => 8,
        OfVersion::V12 => 12,
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn kind_name(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Fluid | RegionKind::Single => "fluid",
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
    }
}

/// A name as a dictionary word: quoted when it isn't a plain word.
pub(crate) fn word(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
        && !s.starts_with(|c: char| c.is_ascii_digit() || c == '-');
    if plain { s.to_string() } else { quote(s) }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn root_steps(steps: &[RootStep]) -> String {
    let mut out = Vec::new();
    for st in steps {
        match st {
            RootStep::BlockMesh(d) if d == BLOCK_DICT => out.push("blockMesh".to_string()),
            RootStep::BlockMesh(d) => out.push(format!("blockMesh {}", quote(d))),
            RootStep::CreatePatch => out.push("createPatch".to_string()),
            RootStep::SnappyHexMesh(d) if d == SNAPPY_DICT => out.push("snappyHexMesh".to_string()),
            RootStep::SnappyHexMesh(d) => out.push(format!("snappyHexMesh {}", quote(d))),
        }
    }
    out.join(" ")
}

fn region_steps(steps: &[RegionStep]) -> String {
    let mut out = Vec::new();
    for st in steps {
        match st {
            RegionStep::CreatePatch => out.push("createPatch".to_string()),
            RegionStep::ChangeDictionary => out.push("changeDictionary".to_string()),
            RegionStep::Sets { setset_batch: None } => out.push("topoSet".to_string()),
            RegionStep::Sets { setset_batch: Some(b) } => out.push(format!("setSet {}", quote(b))),
            RegionStep::SetFields { restore } if restore.is_empty() => out.push("setFields".to_string()),
            RegionStep::SetFields { restore } => {
                let items: Vec<String> = restore.iter().map(|(b, f)| format!("( {} {} )", quote(b), word(f))).collect();
                out.push(format!("setFields ( {} )", items.join(" ")));
            }
        }
    }
    out.join(" ")
}

/// A word or a quoted string as the name it stands for: a quoted one without its quotes and
/// with the escapes [`quote`] writes undone (a backslash before a backslash or a quote).
pub(crate) fn unescaped(t: &str) -> String {
    match t.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) if t.len() >= 2 => {
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, chars.peek()) {
                    ('\\', Some(&n)) if n == '\\' || n == '"' => {
                        out.push(n);
                        chars.next();
                    }
                    _ => out.push(c),
                }
            }
            out
        }
        _ => t.to_string(),
    }
}

/// Whether `e`'s key came before in its dictionary (a problem at `at`, and the entry is left out).
pub(crate) fn twice<'a>(
    seen: &mut HashSet<&'a str>,
    e: &'a Entry,
    at: &str,
    problem: &mut impl FnMut(&str, String),
) -> bool {
    if seen.insert(e.key()) {
        return false;
    }
    problem(at, "is given twice: the later ones are left out".into());
    true
}

/// An entry's key as the name it stands for.
pub(crate) fn entry_name(e: &Entry) -> String {
    unescaped(&e.key.text)
}

/// The one word of `e`'s value, as a name.
pub(crate) fn single(e: &Entry) -> Option<String> {
    match e.words()?.as_slice() {
        [w] => Some(unescaped(w)),
        _ => None,
    }
}

fn flag(e: &Entry) -> Option<bool> {
    match single(e)?.as_str() {
        "yes" | "true" | "on" => Some(true),
        "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn atom(n: &Node) -> Option<&str> {
    match n {
        Node::Atom(t) => Some(t.text.as_str()),
        _ => None,
    }
}

/// A quoted string atom, as the text it stands for.
fn string(n: &Node) -> Option<String> {
    let t = atom(n)?;
    (t.starts_with('"') && t.ends_with('"') && t.len() >= 2).then(|| unescaped(t))
}

/// The items of `e`'s value when it is one list.
pub(crate) fn list(e: &Entry) -> Option<&[Node]> {
    match e.value.as_slice() {
        [Node::List { items, .. }] => Some(items),
        _ => None,
    }
}

fn read_radiation(body: &umcf_foam_dict::Dict, problem: &mut impl FnMut(&str, String)) -> Option<Radiation> {
    let mut region = None;
    let mut rad = Radiation { region: String::new(), lai_region: None, view_factors: true, solar_ray_tracing: true };
    let mut seen = HashSet::new();
    for e in body.entries() {
        let at = format!("radiation/{}", e.key());
        if twice(&mut seen, e, &at, problem) {
            continue;
        }
        match e.key() {
            "region" => match single(e) {
                Some(r) => region = Some(r),
                None => problem(&at, "is one region's name".into()),
            },
            "lai" => match single(e) {
                Some(r) => rad.lai_region = Some(r),
                None => problem(&at, "is one region's name".into()),
            },
            "viewFactors" => match flag(e) {
                Some(b) => rad.view_factors = b,
                None => problem(&at, "is yes or no".into()),
            },
            "solarRayTracing" => match flag(e) {
                Some(b) => rad.solar_ray_tracing = b,
                None => problem(&at, "is yes or no".into()),
            },
            _ => problem(&at, "is not a key of radiation".into()),
        }
    }
    match region {
        Some(r) => {
            rad.region = r;
            Some(rad)
        }
        None => {
            if body.get("region").is_none() {
                problem("radiation/region", "is missing: the region the radiation is computed on".into());
            }
            None
        }
    }
}

fn read_region(
    name: &str,
    body: &umcf_foam_dict::Dict,
    at: &str,
    problem: &mut impl FnMut(&str, String),
) -> Option<RegionDesc> {
    let mut r = RegionDesc {
        name: name.to_string(),
        kind: RegionKind::Fluid,
        mesh: None,
        steps: Vec::new(),
        patches: Vec::new(),
        materials: Vec::new(),
        blocks: None,
        mesh_box: None,
    };
    let mut kind = None;
    let mut materials_given = false;
    // One `patches` (or `kind`, …) per region: a second is a problem, not read.
    let mut seen = HashSet::new();
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        match e.key() {
            "kind" => match single(e).as_deref() {
                Some("fluid") => kind = Some(RegionKind::Fluid),
                Some("solid") => kind = Some(RegionKind::Solid),
                Some("vegetation") => kind = Some(RegionKind::Vegetation),
                _ => problem(&here, "is fluid, solid or vegetation".into()),
            },
            "mesh" => match (single(e).as_deref(), list(e)) {
                _ if e.dict().is_some() => {
                    if let Some(sp) = e.dict().and_then(|b| read_split(name, b, &here, problem)) {
                        r.mesh = Some(MeshBuild::Split(sp));
                    }
                }
                (Some("blocks"), _) => r.mesh = Some(MeshBuild::Block),
                (_, Some(items)) => match read_root(items) {
                    Ok(steps) => r.mesh = Some(MeshBuild::Root(steps)),
                    Err(m) => problem(&here, m),
                },
                _ => problem(&here, "is blocks, the steps that build it in constant/polyMesh: ( blockMesh [\"dict\"] createPatch snappyHexMesh [\"dict\"] ), or { from split; of <region>; zone <cellZone>; [dict \"system/<region>/topoSetDict\";] }".into()),
            },
            "blocks" => match e.dict() {
                Some(b) => r.blocks = BlockGrid::read(b, &here, problem),
                None => problem(&here, "is a dictionary: { x ( … ); y ( … ); z ( … ); patches { … } }".into()),
            },
            "box" => match e.dict() {
                Some(b) => r.mesh_box = MeshBox::read(b, &here, problem),
                None => problem(&here, "is a dictionary: { surfaces ( … ); margin ( … ); cellSize …; }".into()),
            },
            "steps" => match list(e).map(read_steps) {
                Some(Ok(steps)) => r.steps = steps,
                Some(Err(m)) => problem(&here, m),
                None => problem(&here, "is a list: ( createPatch changeDictionary topoSet setSet \"batch\" setFields [( ( \"backup\" field ) )] )".into()),
            },
            "patches" => match e.dict() {
                Some(pb) => {
                    let mut names: HashSet<String> = HashSet::new();
                    for p in pb.entries() {
                        let pname = entry_name(p);
                        let pat = if pname.is_empty() { format!("{here}/\"\"") } else { format!("{here}/{pname}") };
                        if pname.is_empty() {
                            problem(&pat, "is not a name".into());
                            continue;
                        }
                        if !names.insert(pname.clone()) {
                            problem(&pat, "is given twice: the later ones are left out".into());
                            continue;
                        }
                        match p.dict() {
                            Some(b) => r.patches.push(read_patch(&pname, b, &pat, problem)),
                            None => problem(&pat, "is a dictionary: { role …; [partner ( region patch );] }".into()),
                        }
                    }
                }
                None => problem(&here, "is a dictionary".into()),
            },
            "materials" => match e.dict() {
                Some(mb) => {
                    materials_given = true;
                    let mut names: HashSet<String> = HashSet::new();
                    for m in mb.entries() {
                        let mname = entry_name(m);
                        let mat = if mname.is_empty() { format!("{here}/\"\"") } else { format!("{here}/{mname}") };
                        if mname.is_empty() {
                            problem(&mat, "is not a name".into());
                            continue;
                        }
                        if !names.insert(mname.clone()) {
                            problem(&mat, "is given twice: the later ones are left out".into());
                            continue;
                        }
                        match m.dict() {
                            Some(b) => r.materials.push(read_material(&mname, b, &mat, problem)),
                            None => problem(&mat, "is a dictionary: { [model <buildingMaterialModel>;] }".into()),
                        }
                    }
                }
                None => problem(&here, "is a dictionary".into()),
            },
            _ => problem(&here, "is not a key of a region".into()),
        }
    }
    match kind {
        Some(k) => {
            r.kind = k;
            if k != RegionKind::Solid && materials_given {
                problem(&format!("{at}/materials"), "are a solid region's: left out".into());
                r.materials.clear();
            }
            Some(r)
        }
        None => {
            problem(&format!("{at}/kind"), "is missing: the region is left out".into());
            None
        }
    }
}

/// `{ from split; of air; zone trees; [dict "system/air/topoSetDict";] }`: a region split from
/// another's mesh by a cell zone.
fn read_split(
    name: &str,
    body: &umcf_foam_dict::Dict,
    at: &str,
    problem: &mut impl FnMut(&str, String),
) -> Option<Split> {
    let mut from = None;
    let mut of = None;
    let mut zone = None;
    let mut dict = None;
    let mut seen = HashSet::new();
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        match e.key() {
            "from" => match single(e) {
                Some(w) => from = Some(w),
                None => problem(&here, "is split".into()),
            },
            "of" => match single(e) {
                Some(w) if w == name => problem(&here, "is another region than this one".into()),
                Some(w) => of = Some(w),
                None => problem(&here, "is the region whose mesh is split, one name".into()),
            },
            "zone" => match single(e) {
                Some(w) if crate::workflow::part_name(&w) => problem(
                    &here,
                    format!("is not {w}: splitMeshRegions names the parts outside the zone so (region and a number)"),
                ),
                Some(w) => zone = Some(w),
                None => problem(&here, "is the cell zone taken out, one name".into()),
            },
            "dict" => match single(e) {
                Some(d) if d.starts_with('/') || d.split('/').any(|c| c == "..") || !d.contains('/') => {
                    problem(&here, "is a path in the case's directories, system/<region>/topoSetDict".into())
                }
                Some(d) => dict = Some(d),
                None => problem(&here, "is the topoSetDict that makes the zone, a path in the case".into()),
            },
            _ => problem(&here, "is not a key of a split: from, of, zone, dict".into()),
        }
    }
    match from.as_deref() {
        Some("split") => {}
        Some(other) => {
            problem(&format!("{at}/from"), format!("is split, not {other}: the mesh is left out"));
            return None;
        }
        None => {
            problem(&format!("{at}/from"), "is missing: { from split; of <region>; zone <cellZone>; }".into());
            return None;
        }
    }
    let Some(of) = of else {
        problem(&format!("{at}/of"), "is missing: the region whose mesh is split".into());
        return None;
    };
    if zone.is_none() {
        problem(&format!("{at}/zone"), "is missing: the cell zone taken out (no script splits it without)".into());
    }
    let dict = dict.unwrap_or_else(|| Split::default_dict(&of));
    Some(Split { of, zone, dict })
}

fn read_patch(name: &str, body: &umcf_foam_dict::Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> PatchDesc {
    let mut p = PatchDesc { name: name.to_string(), role: None, partner: None };
    let mut seen = HashSet::new();
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        match e.key() {
            "role" => match single(e).as_deref().and_then(Role::parse) {
                Some(role) => p.role = Some(role),
                None => {
                    let names: Vec<&str> = Role::ALL.iter().map(|r| r.name()).collect();
                    problem(&here, format!("is one of {}", names.join(", ")));
                }
            },
            "partner" => match list(e) {
                Some([a, b]) => match (atom(a), atom(b)) {
                    (Some(a), Some(b)) => p.partner = Some((unescaped(a), unescaped(b))),
                    _ => problem(&here, "is ( region patch )".into()),
                },
                _ => problem(&here, "is ( region patch )".into()),
            },
            _ => problem(&here, "is not a key of a patch".into()),
        }
    }
    p
}

fn read_material(
    name: &str,
    body: &umcf_foam_dict::Dict,
    at: &str,
    problem: &mut impl FnMut(&str, String),
) -> MaterialDesc {
    let mut m = MaterialDesc { name: name.to_string(), model: None };
    let mut seen = HashSet::new();
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        if twice(&mut seen, e, &here, problem) {
            continue;
        }
        match e.key() {
            "model" => match single(e) {
                Some(x) => m.model = Some(x),
                None => problem(&here, "is the material's buildingMaterialModel, one name".into()),
            },
            _ => problem(&here, "is not a key of a material".into()),
        }
    }
    m
}

/// The grid of blocks of region `name`'s `blockMeshDict` (the one its `mesh` reads), when it
/// is one; notes on what doesn't fit: a dictionary that isn't such a grid (left as it is), a
/// patch whose type there isn't the one its role gives.
fn import_blocks(
    case: &Case,
    name: &str,
    kind: RegionKind,
    build: &MeshBuild,
    patches: &[PatchDesc],
    notes: &mut Vec<String>,
) -> Option<BlockGrid> {
    let path = crate::blocks::dict_path(name, build)?;
    let Some(doc) = case.dict(&path) else {
        if case.exists(&path) == Some(true) {
            notes.push(format!("region {name}: {path} doesn't load as a dictionary: no blocks read from it"));
        }
        return None;
    };
    match crate::blocks::import(doc) {
        Ok(imp) => {
            let mut grid = imp.grid;
            // The types the roles wouldn't give, kept: the dictionary is written again with them.
            for (patch, t) in imp.types {
                let role = patches.iter().find(|p| p.name == patch).and_then(|p| p.role);
                // blockMesh's own patch is empty unless typed otherwise.
                let want =
                    if patch == crate::blocks::DEFAULT_FACES { "empty" } else { crate::blocks::patch_type(kind, role) };
                if unquote(&t) != want {
                    grid.types.push((patch, unquote(&t).to_string()));
                }
            }
            Some(grid)
        }
        Err(why) => {
            notes.push(format!(
                "region {name}: {path} is not a grid of blocks ({why}): left as it is, the description has no blocks for it"
            ));
            None
        }
    }
}

/// Where a solid region's materials are: its `transportProperties`.
pub fn materials_file(region: &str) -> String {
    format!("constant/{region}/transportProperties")
}

/// A material as a solid's `transportProperties` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseMaterial {
    pub name: String,
    /// Its `buildingMaterialModel`, when it is a plain name.
    pub model: Option<String>,
    /// Whether it has a `buildingMaterialModel` at all (a macro, say, when `model` is `None`).
    pub model_given: bool,
}

/// A solid region's materials as its `transportProperties` lists them (`buildingMaterials`),
/// in order (a name listed twice, twice), and whether every one was read: an entry that isn't a
/// dictionary, or a name given by a macro or a directive (`$m`, `#calc`), is left out. `None`
/// without the list.
pub fn case_materials(case: &Case, region: &str) -> Option<(Vec<CaseMaterial>, bool)> {
    let list = case.dict(&materials_file(region))?.get(&["buildingMaterials"])?;
    let [Node::List { items, .. }] = list.value.as_slice() else { return Some((Vec::new(), false)) };
    let plain = |v: &str| {
        let v = v.trim();
        (!v.is_empty() && !v.starts_with('$') && !v.starts_with('#')).then(|| unquote(v).to_string())
    };
    let mut out = Vec::new();
    let mut all = true;
    for item in items {
        let Node::Dict { body, .. } = item else {
            all = false;
            continue;
        };
        match body.get("name").and_then(Entry::value_text).and_then(|v| plain(&v)) {
            Some(name) => {
                let given = body.get("buildingMaterialModel");
                let model = given.and_then(Entry::value_text).and_then(|v| plain(&v));
                out.push(CaseMaterial { name, model, model_given: given.is_some() });
            }
            None => all = false,
        }
    }
    Some((out, all))
}

/// `blockMesh ["dict"] createPatch snappyHexMesh ["dict"]`, in any order.
fn read_root(items: &[Node]) -> Result<Vec<RootStep>, String> {
    let mut out = Vec::new();
    let mut it = items.iter().peekable();
    while let Some(n) = it.next() {
        let dict = |it: &mut std::iter::Peekable<std::slice::Iter<'_, Node>>, default: &str| match it
            .peek()
            .and_then(|n| string(n))
        {
            Some(d) => {
                it.next();
                d
            }
            None => default.to_string(),
        };
        match atom(n) {
            Some("blockMesh") => out.push(RootStep::BlockMesh(dict(&mut it, BLOCK_DICT))),
            Some("snappyHexMesh") => out.push(RootStep::SnappyHexMesh(dict(&mut it, SNAPPY_DICT))),
            Some("createPatch") => out.push(RootStep::CreatePatch),
            _ => return Err(format!("`{}` is not a step: blockMesh, createPatch or snappyHexMesh", n.compact())),
        }
    }
    if !matches!(out.first(), Some(RootStep::BlockMesh(_))) {
        return Err("starts with blockMesh, which makes the mesh the other steps change".into());
    }
    Ok(out)
}

/// `createPatch changeDictionary topoSet setSet "batch" setFields [( ( "backup" field ) … )]`.
fn read_steps(items: &[Node]) -> Result<Vec<RegionStep>, String> {
    let mut out = Vec::new();
    let mut it = items.iter().peekable();
    while let Some(n) = it.next() {
        match atom(n) {
            Some("createPatch") => out.push(RegionStep::CreatePatch),
            Some("changeDictionary") => out.push(RegionStep::ChangeDictionary),
            Some("topoSet") => out.push(RegionStep::Sets { setset_batch: None }),
            Some("setSet") => match it.next().and_then(string) {
                Some(b) => out.push(RegionStep::Sets { setset_batch: Some(b) }),
                None => return Err("setSet is followed by its batch file, quoted".into()),
            },
            Some("setFields") => {
                let mut restore = Vec::new();
                if let Some(Node::List { items: pairs, .. }) = it.peek() {
                    for pair in pairs.iter() {
                        let ok = match pair {
                            Node::List { items: two, .. } => match two.as_slice() {
                                [b, f] => string(b).zip(atom(f).map(unescaped)),
                                _ => None,
                            },
                            _ => None,
                        };
                        match ok {
                            Some(x) => restore.push(x),
                            None => {
                                return Err(format!(
                                    "setFields restores ( \"backup\" field ) pairs, not {}",
                                    pair.compact()
                                ));
                            }
                        }
                    }
                    it.next();
                }
                out.push(RegionStep::SetFields { restore });
            }
            _ => {
                return Err(format!(
                    "`{}` is not a step: createPatch, changeDictionary, topoSet, setSet or setFields",
                    n.compact()
                ));
            }
        }
    }
    Ok(out)
}

/// The roles of the air region's patches, by name, for the vegetation region's twins.
fn air_roles(case: &Case) -> Vec<(String, Role)> {
    let Some(air) = case.regions.iter().find(|r| r.kind == RegionKind::Fluid) else { return Vec::new() };
    let mut out = Vec::new();
    for p in air.patches.iter().flatten() {
        let u = air
            .field("U")
            .and_then(|f| f.bcs.as_ref())
            .and_then(|bcs| bcs.iter().find(|b| b.patch == p.name && !b.pattern))
            .and_then(|b| b.kind.as_deref());
        if let (Some(role), _) = infer(case, RegionKind::Fluid, p, u, &[]) {
            out.push((p.name.clone(), role));
        }
    }
    out
}

/// A patch's role and partner, as far as the mesh and the velocity condition (`u`) tell.
fn infer(
    case: &Case,
    kind: RegionKind,
    p: &Patch,
    u: Option<&str>,
    air_roles: &[(String, Role)],
) -> (Option<Role>, Option<(String, String)>) {
    let partner_region = p.param("neighbourRegion").or_else(|| p.param("sampleRegion")).map(|s| unquote(s).to_string());
    // The canopy maps to the neighbour's cells, not to a patch of it: OpenFOAM-12's
    // mappedInternal, OpenFOAM-8's mapped patches with `sampleMode nearestCell`.
    let to_cells = p.kind == "mappedInternal" || p.param("sampleMode").map(unquote) == Some("nearestCell");
    let partner_region = partner_region.filter(|_| !to_cells);
    let partner = partner_region.map(|r| {
        let patch = p.param("neighbourPatch").or_else(|| p.param("samplePatch")).map_or(p.name.as_str(), unquote);
        (r, patch.to_string())
    });
    let partner_kind = partner.as_ref().and_then(|(r, _)| case.region(r)).map(|r| r.kind);
    let name = p.name.to_ascii_lowercase();
    let role = match kind {
        RegionKind::Vegetation => {
            if to_cells || name == "air_to_vegetation" {
                Some(Role::Canopy)
            } else {
                match air_roles.iter().find(|(n, _)| *n == p.name).map(|(_, r)| *r) {
                    Some(Role::Inlet | Role::Outlet | Role::Side | Role::Top) => Some(Role::Sky),
                    Some(r @ (Role::Ground | Role::Coupled)) => Some(r),
                    _ => None,
                }
            }
        }
        RegionKind::Solid => {
            if matches!(partner_kind, Some(RegionKind::Fluid | RegionKind::Vegetation)) {
                Some(Role::Coupled)
            } else if name == "interior" || name.ends_with("_interior") {
                Some(Role::Interior)
            } else if name == "bottom" {
                Some(Role::Bottom)
            } else if name == "sides" || name == "side" {
                Some(Role::Side)
            } else {
                None
            }
        }
        RegionKind::Fluid | RegionKind::Single => {
            if matches!(partner_kind, Some(RegionKind::Solid)) {
                Some(Role::Coupled)
            } else if name == "top" || name == "sky" {
                Some(Role::Top)
            } else if name == "ground" || name == "floor" || name == "terrain" {
                Some(Role::Ground)
            } else {
                match u {
                    Some(
                        "atmBoundaryLayerInletVelocity"
                        | "fixedValue"
                        | "uniformFixedValue"
                        | "timeVaryingMappedFixedValue"
                        | "flowRateInletVelocity"
                        | "surfaceNormalFixedValue"
                        | "turbulentInlet",
                    ) if p.kind != "wall" => Some(Role::Inlet),
                    Some(
                        "inletOutlet" | "zeroGradient" | "pressureInletOutletVelocity" | "outletInlet" | "advective",
                    ) => Some(Role::Outlet),
                    Some("slip" | "symmetry" | "symmetryPlane") => Some(Role::Side),
                    _ if matches!(p.kind.as_str(), "symmetry" | "symmetryPlane") => Some(Role::Side),
                    _ => None,
                }
            }
        }
    };
    (role, partner)
}

#[cfg(test)]
#[path = "description_tests.rs"]
pub(crate) mod tests;
