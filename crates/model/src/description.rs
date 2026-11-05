//! The case's workflow description, `system/umcfWorkflow` (see `docs/common-workflow.md`): an
//! OpenFOAM-style dictionary that travels with the case. It names every region with its kind,
//! how its mesh is built and the steps after, the patches with their roles and partners, the
//! radiation pre-processing and the solver.
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
//!     }
//! }
//! radiation   { region vegetation; lai air; viewFactors yes; solarRayTracing yes; }
//! solver      urbanMicroclimateFoam;
//! reconstruct yes;
//! ```

use std::fmt::Write as _;

use umcf_core::OfVersion;
use umcf_foam_dict::{Document, Entry, Node, unquote};

use crate::workflow::{MeshBuild, Radiation, RegionMesh, RegionStep, RootStep, Workflow};
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
            regions.push(RegionDesc {
                name: name.to_string(),
                kind: region.kind,
                mesh: mesh.map(|m| m.build.clone()),
                steps: mesh.map(|m| m.steps.clone()).unwrap_or_default(),
                patches,
            });
        }
        for u in &wf.unmatched {
            if !u.redundant {
                notes.push(format!("{} line {}: not in the description: {}", u.script, u.line, u.what));
            }
        }
        let d = Description {
            version: case.layout.version,
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
        let mut seen: Vec<String> = Vec::new();
        for e in doc.root.entries() {
            let key = e.key();
            if key != "FoamFile" && seen.iter().any(|k| k == key) {
                problem(key, "is given twice: the first is read".into());
                continue;
            }
            seen.push(key.to_string());
            match key {
                "FoamFile" => {}
                "version" => match single(e).as_deref() {
                    Some("8") => d.version = Some(OfVersion::V8),
                    Some("12") => d.version = Some(OfVersion::V12),
                    _ => problem(key, "is 8 or 12".into()),
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
                        for r in body.entries() {
                            let rname = entry_name(r);
                            let at =
                                if rname.is_empty() { "regions/\"\"".to_string() } else { format!("regions/{rname}") };
                            if rname.is_empty() {
                                problem(&at, "is not a name".into());
                                continue;
                            }
                            if d.regions.iter().any(|x| x.name == rname) {
                                problem(&at, "is given twice: the first is read".into());
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
fn word(s: &str) -> String {
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
fn unescaped(t: &str) -> String {
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

/// An entry's key as the name it stands for.
fn entry_name(e: &Entry) -> String {
    unescaped(&e.key.text)
}

/// The one word of `e`'s value, as a name.
fn single(e: &Entry) -> Option<String> {
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

fn atom(n: &Node) -> Option<&str> {
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
fn list(e: &Entry) -> Option<&[Node]> {
    match e.value.as_slice() {
        [Node::List { items, .. }] => Some(items),
        _ => None,
    }
}

fn read_radiation(body: &umcf_foam_dict::Dict, problem: &mut impl FnMut(&str, String)) -> Option<Radiation> {
    let mut region = None;
    let mut rad = Radiation { region: String::new(), lai_region: None, view_factors: true, solar_ray_tracing: true };
    for e in body.entries() {
        let at = format!("radiation/{}", e.key());
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
    };
    let mut kind = None;
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
        match e.key() {
            "kind" => match single(e).as_deref() {
                Some("fluid") => kind = Some(RegionKind::Fluid),
                Some("solid") => kind = Some(RegionKind::Solid),
                Some("vegetation") => kind = Some(RegionKind::Vegetation),
                _ => problem(&here, "is fluid, solid or vegetation".into()),
            },
            "mesh" => match (single(e).as_deref(), list(e)) {
                (Some("blocks"), _) => r.mesh = Some(MeshBuild::Block),
                (_, Some(items)) => match read_root(items) {
                    Ok(steps) => r.mesh = Some(MeshBuild::Root(steps)),
                    Err(m) => problem(&here, m),
                },
                _ => problem(&here, "is blocks, or the steps that build it in constant/polyMesh: ( blockMesh [\"dict\"] createPatch snappyHexMesh [\"dict\"] )".into()),
            },
            "steps" => match list(e).map(read_steps) {
                Some(Ok(steps)) => r.steps = steps,
                Some(Err(m)) => problem(&here, m),
                None => problem(&here, "is a list: ( createPatch changeDictionary topoSet setSet \"batch\" setFields [( ( \"backup\" field ) )] )".into()),
            },
            "patches" => match e.dict() {
                Some(pb) => {
                    for p in pb.entries() {
                        let pname = entry_name(p);
                        let pat = if pname.is_empty() { format!("{here}/\"\"") } else { format!("{here}/{pname}") };
                        if pname.is_empty() {
                            problem(&pat, "is not a name".into());
                            continue;
                        }
                        if r.patches.iter().any(|x| x.name == pname) {
                            problem(&pat, "is given twice: the first is read".into());
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
            _ => problem(&here, "is not a key of a region".into()),
        }
    }
    match kind {
        Some(k) => {
            r.kind = k;
            Some(r)
        }
        None => {
            problem(&format!("{at}/kind"), "is missing: the region is left out".into());
            None
        }
    }
}

fn read_patch(name: &str, body: &umcf_foam_dict::Dict, at: &str, problem: &mut impl FnMut(&str, String)) -> PatchDesc {
    let mut p = PatchDesc { name: name.to_string(), role: None, partner: None };
    for e in body.entries() {
        let here = format!("{at}/{}", e.key());
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
mod tests;
