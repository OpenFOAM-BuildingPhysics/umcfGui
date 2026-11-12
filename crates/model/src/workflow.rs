//! The case workflow: how a case is meshed, pre-processed, run and cleaned.
//!
//! urbanMicroclimateFoam cases follow one pattern, which their `Allprepare`, `Allrun` and
//! `Allclean` scripts spell out in shell:
//!
//! 1. **Prepare**, region by region: a fluid or vegetation region is built in `constant/polyMesh`
//!    from variant dictionaries (`system/blockMeshDict.air`, `system/snappyHexMeshDict.air`),
//!    then moved to `constant/<region>/polyMesh`; a solid region is built in place with
//!    `blockMesh -region`. Then per region: `createPatch`, `changeDictionary`, `topoSet`
//!    (`setSet` in OpenFOAM-8), and `setFields` from a backed-up field (the LAD).
//! 2. **Run**: decompose every region, the radiation pre-processing of the vegetation region
//!    (`faceAgglomerate`, `calcLAI`, `viewFactorsGen`, `solarRayTracingGen`), the solver, and
//!    reconstruct.
//! 3. **Clean**: `cleanCase`, the region meshes and the pre-processing outputs.
//!
//! [`Workflow::import`] reads a case's scripts into this pattern and lists what doesn't fit;
//! [`Workflow::generate`] writes the scripts back for an OpenFOAM version, with the choices the
//! version needs (where each utility reads its dictionary, `topoSet` or `setSet`).

use std::collections::BTreeMap;

use umcf_core::OfVersion;

use crate::script::{Np, Run, Script, StepKind};

/// A case's workflow.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workflow {
    /// Every region, in the order the scripts decompose and reconstruct them.
    pub regions: Vec<String>,
    /// `surfaceFeatures` runs first (feature edges for snappyHexMesh).
    pub features: bool,
    /// The region meshes, in the order they are built.
    pub meshes: Vec<RegionMesh>,
    pub radiation: Option<Radiation>,
    /// The solver run in parallel (`urbanMicroclimateFoam`); `None` when the scripts run none.
    pub solver: Option<String>,
    pub reconstruct: bool,
    /// Script steps that don't fit the pattern, or are not needed.
    pub unmatched: Vec<Unmatched>,
    /// The process counts the scripts start (`mpirun -np 20`), with where.
    pub np: Vec<(String, usize, u32)>,
    /// The dictionaries directly in `constant/<region>/`, by region: Allrun copies them into
    /// `processor*/constant/<region>/` (see [`Workflow::generate`]). Filled by
    /// [`crate::Case::workflow`].
    pub constant_files: Vec<(String, Vec<String>)>,
    /// The case's regions, also those the workflow leaves out: no script removes their
    /// directories (a split's clean-up names its zone's). Filled by [`crate::Case::workflow`]
    /// and [`crate::description::Description::scripts`].
    pub case_regions: Vec<String>,
    /// The cell zones each split region's `topoSetDict` makes, where it was read: no split is
    /// written for one that makes others than the split's zone (`splitMeshRegions -cellZones`
    /// would split those off too, writing their meshes over `constant/<zone>`). Filled by
    /// [`crate::Case::workflow`] and [`crate::description::Description::scripts`].
    pub split_zones: Vec<(String, DictZones)>,
    serial_agglomerate: Vec<Unmatched>,
    parallel_agglomerate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionMesh {
    pub region: String,
    pub build: MeshBuild,
    pub steps: Vec<RegionStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeshBuild {
    /// `blockMesh -region <r>` from `system/<r>/blockMeshDict`.
    Block,
    /// Built in `constant/polyMesh` by these steps, in order, and moved to
    /// `constant/<r>/polyMesh`.
    Root(Vec<RootStep>),
    /// Split from another region's mesh by a cell zone.
    Split(Split),
}

/// A region whose mesh is a copy of another region's, split by a cell zone: the copy's cells
/// outside the zone (the street canyon's vegetation is the air without the trees). The copy is
/// put in `constant/<r>/polyMesh`, `topoSet -region <r> -dict <dict>` makes the zone in it,
/// `splitMeshRegions -region <r> -cellZones -overwrite` splits it, and the part outside the
/// zone ([`SPLIT_REST`]) becomes the region's mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// The region whose mesh is copied; built before this one.
    pub of: String,
    /// The cell zone taken out; `None`: not known (an import that couldn't tell).
    pub zone: Option<String>,
    /// The `topoSetDict` that makes the zone, relative to the case (`system/air/topoSetDict`).
    pub dict: String,
}

impl Split {
    /// The `topoSetDict` a split of `of` reads unless it names another: `of`'s own.
    pub fn default_dict(of: &str) -> String {
        format!("system/{of}/topoSetDict")
    }
}

/// What a split region's `topoSetDict` makes, as far as the case tells.
#[derive(Debug, Clone, PartialEq)]
pub enum DictZones {
    /// The case doesn't have it: `topoSet` would stop the script after the region's mesh was
    /// replaced by the copy.
    Missing,
    /// There (or not known not to be) but which zones it makes can't be told: it doesn't read,
    /// or uses `#include` or a macro.
    Unclear,
    /// The cell zones it makes.
    Made(Vec<String>),
}

/// The name `splitMeshRegions -cellZones` gives the cells outside every zone when they are
/// one connected part: the split region's mesh. Its interface patch to the zone is
/// `region1_to_<zone>`.
pub const SPLIT_REST: &str = "region1";

/// Why [`Workflow::generate`] writes no scripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// A name that isn't one path component, or a path that leaves the case, or either with a
    /// control character or a leading `-`.
    Unsafe(String),
    /// A region split from another ([`MeshBuild::Split`]) the script can't make: what its
    /// clean-up would take away is another region, or what it needs isn't known.
    Split { region: String, why: String },
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::Unsafe(w) => write!(
                f,
                "`{w}` isn't safe in a script: not a plain name or a path inside the case (a control character, a leading `-`, `/` or `..`)"
            ),
            ScriptError::Split { region, why } => write!(f, "region {region}, split by a cell zone: {why}"),
        }
    }
}

impl std::error::Error for ScriptError {}

/// A step building a mesh in `constant/polyMesh`.
#[derive(Debug, Clone, PartialEq)]
pub enum RootStep {
    /// `blockMesh` from this dictionary (copied to `system/blockMeshDict` when it is another).
    BlockMesh(String),
    /// `createPatch -overwrite` (`system/createPatchDict`).
    CreatePatch,
    /// `snappyHexMesh -overwrite` from this dictionary (copied to `system/snappyHexMeshDict`).
    SnappyHexMesh(String),
}

const BLOCK_DICT: &str = "system/blockMeshDict";
const SNAPPY_DICT: &str = "system/snappyHexMeshDict";

#[derive(Debug, Clone, PartialEq)]
pub enum RegionStep {
    CreatePatch,
    ChangeDictionary,
    /// Cell and face sets and zones: `topoSet` (`system/<r>/topoSetDict`), or in
    /// OpenFOAM-8 `setSet -batch` with this batch file.
    Sets {
        setset_batch: Option<String>,
    },
    /// `setFields`, after restoring each field from its backup: `(backup file, field name)`.
    SetFields {
        restore: Vec<(String, String)>,
    },
}

/// The radiation pre-processing.
#[derive(Debug, Clone, PartialEq)]
pub struct Radiation {
    /// The region `faceAgglomerate`, `viewFactorsGen` and `solarRayTracingGen` run on.
    pub region: String,
    /// The region `calcLAI` runs on (the fluid region with the LAD), if it runs.
    pub lai_region: Option<String>,
    pub view_factors: bool,
    pub solar_ray_tracing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unmatched {
    pub script: String,
    pub line: usize,
    pub what: String,
    /// It fits the pattern but is not needed (for example copying `constant/` files into
    /// `processor*/`, which OpenFOAM reads from the case).
    pub redundant: bool,
}

/// The files `faceAgglomerate`, `viewFactorsGen` and `solarRayTracingGen` write to
/// `constant/<radiation region>/` (serially; in `processor*/constant/` when run in parallel).
pub const RADIATION_OUTPUTS: &[&str] = &[
    "finalAgglom",
    "F",
    "globalFaceFaces",
    "constructMap",
    "constructMapDim",
    "subMap",
    "kcLAIboundary",
    "skyViewCoeff",
    "sunViewCoeff",
    "sunskyMap",
    "solarLoadFineFaces",
];

impl Workflow {
    /// Read `Allprepare` (or the script that builds meshes) and `Allrun` into the pattern.
    /// `regions` are the case's regions; `application` is `controlDict`'s.
    pub fn import(regions: &[String], application: Option<&str>, scripts: &[Script]) -> Workflow {
        let mut wf = Workflow { regions: regions.to_vec(), ..Workflow::default() };
        let solver = application.unwrap_or("urbanMicroclimateFoam");
        for s in scripts.iter().filter(|s| is_workflow_script(&s.path)) {
            wf.read(s, solver);
        }
        // A serial faceAgglomerate (Allprepare) is not needed when Allrun runs it in parallel:
        // the parallel run reads the agglomeration from processor*/.
        if wf.parallel_agglomerate {
            wf.unmatched.append(&mut wf.serial_agglomerate);
        }
        wf.unmatched.sort_by(|a, b| a.script.cmp(&b.script).then(a.line.cmp(&b.line)));
        wf
    }

    /// The zone a split's clean-up names among `paths` ([`split_zone`]), never a region's name.
    fn zone_hint(&self, paths: &[String], region: &str) -> Option<String> {
        let region_named = |z: &String| {
            self.regions.iter().chain(&self.case_regions).any(|r| r == z) || self.meshes.iter().any(|m| m.region == *z)
        };
        paths.iter().filter_map(|p| split_zone(p, region)).find(|z| !region_named(z))
    }

    /// The zone the split of `region` is known to take out.
    fn split_zone_of(&self, region: &str) -> Option<String> {
        self.meshes.iter().find(|m| m.region == region).and_then(|m| match &m.build {
            MeshBuild::Split(sp) => sp.zone.clone(),
            _ => None,
        })
    }

    fn mesh(&mut self, region: &str) -> Option<&mut RegionMesh> {
        self.meshes.iter_mut().find(|m| m.region == region)
    }

    fn read(&mut self, s: &Script, solver: &str) {
        // A mesh being built in constant/polyMesh, not yet moved to its region.
        let mut root: Option<Vec<RootStep>> = None;
        let mut copied_block: Option<String> = None;
        let mut copied_snappy: Option<String> = None;
        // Fields restored from a backup, waiting for the region's setFields.
        let mut restore: BTreeMap<String, Vec<(String, String, usize)>> = BTreeMap::new();
        // A copy of another region's mesh waiting for its split, and the region split last
        // (whose leftovers the script may then take away).
        let mut pending: Option<PendingSplit> = None;
        // The region split, and whether into a time directory (without `-overwrite`).
        let mut split: Option<(String, bool)> = None;
        // `mkdir constant/<r>`: part of a split when a copy of a mesh into it follows.
        let mut made: Vec<(usize, String, String)> = Vec::new();
        let miss = |wf: &mut Workflow, line: usize, what: String, redundant: bool| {
            wf.unmatched.push(Unmatched { script: s.path.clone(), line, what, redundant });
        };
        for st in &s.steps {
            let line = st.line;
            match &st.kind {
                // Logs, the script's own directory changes and messages are not the workflow.
                StepKind::Write { path } if path.starts_with("log") => {}
                StepKind::MakeDir { paths } if paths.iter().all(|p| p.starts_with("log")) => {}
                StepKind::Move { from, .. } if from.iter().all(|p| p.starts_with("log")) => {}
                StepKind::Remove { paths }
                    if paths.iter().all(|p| p.starts_with("log") || p.starts_with("processor")) => {}
                // What the split leaves: the time directory it wrote (without `-overwrite`),
                // the parts, their copies of the region's `system/` dictionaries, its maps.
                StepKind::Remove { paths }
                    if split.as_ref().is_some_and(|(reg, timed)| {
                        let zone = self.split_zone_of(reg).or_else(|| self.zone_hint(paths, reg));
                        paths.iter().all(|p| split_leftover(p, reg, zone.as_deref(), *timed))
                    }) =>
                {
                    if let Some((reg, _)) = split.as_ref() {
                        let zone = self.zone_hint(paths, reg);
                        if let (Some(z), Some(m)) = (zone, self.mesh(reg))
                            && let MeshBuild::Split(sp) = &mut m.build
                            && sp.zone.is_none()
                        {
                            sp.zone = Some(z);
                        }
                    }
                }
                StepKind::Remove { paths } if paths.iter().all(|p| is_temporary(p) || p == "constant/polyMesh") => {}
                StepKind::Remove { paths }
                    if paths.iter().all(|p| {
                        p.strip_prefix("constant/")
                            .and_then(|r| r.strip_suffix("/polyMesh"))
                            .is_some_and(|r| !r.contains('/'))
                    }) => {}
                // A region's mesh copied from another's, to be split by a cell zone:
                // `cp -r constant/air/polyMesh constant/vegetation/polyMesh`.
                StepKind::Copy { from, to } if split_copy(from, to).is_some() => {
                    if let Some((of, region)) = split_copy(from, to) {
                        if let Some(p) = pending.take() {
                            p.unused(self, &s.path);
                        }
                        made.retain(|(_, _, r)| *r != region);
                        pending =
                            Some(PendingSplit { region, of, dict: None, lines: vec![(line, describe_step(&st.kind))] });
                    }
                }
                // The zone made in the copy: `topoSet -region vegetation -dict system/air/topoSetDict`.
                // The copy's own zones taken away first, as the generated script does.
                StepKind::Remove { paths }
                    if pending
                        .as_ref()
                        .is_some_and(|p| p.dict.is_none() && paths.iter().all(|x| copy_zones(x, &p.region))) =>
                {
                    if let Some(p) = pending.as_mut() {
                        p.lines.push((line, describe_step(&st.kind)));
                    }
                }
                StepKind::Run(r)
                    if r.app == "topoSet"
                        && !r.parallel
                        && pending.as_ref().is_some_and(|p| {
                            p.dict.is_none()
                                && r.region.as_deref() == Some(p.region.as_str())
                                && toposet_args(&r.args, &p.region)
                        }) =>
                {
                    if let Some(p) = pending.as_mut() {
                        p.dict = Some(match r.dict.as_deref() {
                            Some(d) => region_dict(d),
                            None => format!("system/{}/topoSetDict", p.region),
                        });
                        p.lines.push((line, describe(r)));
                    }
                }
                StepKind::Run(r) if r.app == "splitMeshRegions" => match (pending.take(), r.region.as_deref()) {
                    (Some(PendingSplit { region, of, dict: Some(dict), .. }), Some(reg))
                        if region == reg && !r.parallel && split_args(&r.args, reg) =>
                    {
                        let build = MeshBuild::Split(Split { of, zone: None, dict });
                        self.meshes.retain(|m| m.region != reg);
                        self.meshes.push(RegionMesh { region: reg.to_string(), build, steps: Vec::new() });
                        split = Some((reg.to_string(), !r.args.iter().any(|a| a == "-overwrite")));
                    }
                    (p, _) => {
                        if let Some(p) = p {
                            p.unused(self, &s.path);
                        }
                        miss(self, line, describe(r), false);
                    }
                },
                // The split's part outside the zone put in place:
                // `cp -r 3600/region1/polyMesh constant/vegetation/`.
                StepKind::Copy { from, to } | StepKind::Move { from, to }
                    if split.as_ref().is_some_and(|(reg, _)| split_take(from, to, reg)) => {}
                // The region's directory for the copy: `mkdir -p constant/vegetation`, reported
                // unless a copy of a mesh into it follows.
                StepKind::MakeDir { paths }
                    if paths.len() == 1
                        && paths.iter().all(|p| {
                            p.trim_end_matches('/')
                                .strip_prefix("constant/")
                                .is_some_and(|r| !r.is_empty() && !r.contains(['/', '*', '?', '[']))
                        }) =>
                {
                    if let Some(r) = paths.first().and_then(|p| p.trim_end_matches('/').strip_prefix("constant/")) {
                        made.push((line, describe_step(&st.kind), r.to_string()));
                    }
                }
                StepKind::Copy { from, to } if from.len() == 1 && to == "system/blockMeshDict" => {
                    copied_block = from.first().cloned();
                }
                StepKind::Copy { from, to } if from.len() == 1 && to == "system/snappyHexMeshDict" => {
                    copied_snappy = from.first().cloned();
                }
                // OpenFOAM-12 faceAgglomerate reads system/viewFactorsDict: the scripts copy the
                // region's there. The generated script passes -dict instead.
                StepKind::Copy { from, to } if to == "system/viewFactorsDict" && from.len() == 1 => {
                    if from.iter().any(|f| f.starts_with("constant/")) {
                        miss(self, line, describe_step(&st.kind), false);
                    }
                }
                // Restoring a field before setFields: `cp 0/air.bckp/LAD 0/air/`.
                StepKind::Copy { from, to } if from.len() == 1 && backup_restore(from, to).is_some() => {
                    if let Some((region, backup, field)) = backup_restore(from, to) {
                        restore.entry(region).or_default().push((backup, field, line));
                    }
                }
                StepKind::Remove { paths }
                    if paths.iter().all(|p| p.starts_with("0/") && p.matches('/').count() == 2) => {}
                StepKind::Move { from, to }
                    if from.len() == 1 && from.first().is_some_and(|f| f == "constant/polyMesh") =>
                {
                    let region = to.trim_end_matches('/').trim_end_matches("/polyMesh");
                    let region = region.strip_prefix("constant/").unwrap_or(region);
                    match root.take() {
                        Some(steps) if !region.contains('/') && !region.is_empty() => {
                            let build = MeshBuild::Root(steps);
                            self.meshes.retain(|m| m.region != region);
                            self.meshes.push(RegionMesh { region: region.to_string(), build, steps: Vec::new() });
                        }
                        _ => miss(self, line, format!("mv constant/polyMesh {to}"), false),
                    }
                }
                StepKind::Run(r) if st.unresolved && !is_radiation(&r.app) && r.app != solver => {
                    miss(self, line, describe(r), false);
                }
                StepKind::Run(r) => {
                    self.run(r, line, &s.path, solver, &mut root, &mut copied_block, &mut copied_snappy, &mut restore)
                }
                // Copying constant/ files into processor*/ (needed by solver builds before
                // 40f31d4): part of the pattern.
                StepKind::Copy { to, .. } if to.starts_with("processor") => {}
                StepKind::MakeDir { paths } if paths.iter().all(|p| p.starts_with("processor")) => {}
                StepKind::Clean { .. } | StepKind::Call { .. } => {}
                k => miss(self, line, describe_step(k), false),
            }
        }
        if let Some(p) = pending {
            p.unused(self, &s.path);
        }
        for (line, what, _) in made {
            miss(self, line, what, false);
        }
        // Fields restored from a backup that no setFields of the region used.
        for (region, fields) in restore {
            for (backup, field, line) in fields {
                miss(
                    self,
                    line,
                    format!("cp {backup} 0/{region}/{field} (no setFields -region {region} after it)"),
                    false,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        r: &Run,
        line: usize,
        script: &str,
        solver: &str,
        root: &mut Option<Vec<RootStep>>,
        copied_block: &mut Option<String>,
        copied_snappy: &mut Option<String>,
        restore: &mut BTreeMap<String, Vec<(String, String, usize)>>,
    ) {
        if let Some(Np::Count(n)) = r.np {
            self.np.push((script.to_string(), line, n));
        }
        let miss = |wf: &mut Workflow, what: String, redundant: bool| {
            wf.unmatched.push(Unmatched { script: script.to_string(), line, what, redundant });
        };
        match (r.app.as_str(), r.region.as_deref()) {
            ("surfaceFeatures", None) => self.features = true,
            // `-dict` names the dictionary as a copy to the default place would.
            ("blockMesh", None) => {
                let dict = r.dict.clone().or_else(|| copied_block.take()).unwrap_or_else(|| BLOCK_DICT.to_string());
                *root = Some(vec![RootStep::BlockMesh(dict)]);
            }
            ("createPatch", None) => match root {
                Some(steps) if r.dict.is_none() => steps.push(RootStep::CreatePatch),
                _ => miss(self, describe(r), false),
            },
            ("snappyHexMesh", None) => match root {
                Some(steps) => {
                    let dict =
                        r.dict.clone().or_else(|| copied_snappy.take()).unwrap_or_else(|| SNAPPY_DICT.to_string());
                    steps.push(RootStep::SnappyHexMesh(dict));
                }
                None => miss(self, describe(r), false),
            },
            // A region step with its own dictionary doesn't fit the pattern.
            (_, Some(_)) if r.dict.is_some() && !is_radiation(&r.app) => miss(self, describe(r), false),
            ("blockMesh", Some(reg)) => {
                self.meshes.retain(|m| m.region != reg);
                self.meshes.push(RegionMesh { region: reg.to_string(), build: MeshBuild::Block, steps: Vec::new() });
            }
            ("createPatch" | "changeDictionary" | "topoSet" | "setSet" | "setFields", Some(reg)) => {
                let step = match r.app.as_str() {
                    "createPatch" => RegionStep::CreatePatch,
                    "changeDictionary" => RegionStep::ChangeDictionary,
                    "topoSet" => RegionStep::Sets { setset_batch: None },
                    "setSet" => {
                        let batch = r.args.iter().position(|a| a == "-batch").and_then(|k| r.args.get(k + 1)).cloned();
                        RegionStep::Sets { setset_batch: batch }
                    }
                    _ => RegionStep::SetFields {
                        restore: restore.remove(reg).unwrap_or_default().into_iter().map(|(b, f, _)| (b, f)).collect(),
                    },
                };
                match self.mesh(reg) {
                    Some(m) => m.steps.push(step),
                    None => miss(self, describe(r), false),
                }
            }
            ("faceAgglomerate", Some(reg)) => {
                if !r.parallel {
                    self.serial_agglomerate.push(Unmatched {
                        script: script.to_string(),
                        line,
                        what: describe(r),
                        redundant: true,
                    });
                }
                self.radiation_mut(reg);
                if r.parallel {
                    self.parallel_agglomerate = true;
                }
            }
            ("viewFactorsGen", Some(reg)) => self.radiation_mut(reg).view_factors = true,
            ("solarRayTracingGen", Some(reg)) => self.radiation_mut(reg).solar_ray_tracing = true,
            ("calcLAI", Some(reg)) => {
                let lai = reg.to_string();
                match &mut self.radiation {
                    Some(rad) => rad.lai_region = Some(lai),
                    // calcLAI reads the region `vegetation` (hard-coded in the solver's calcLAI).
                    None => {
                        self.radiation_mut("vegetation").lai_region = Some(lai);
                    }
                }
            }
            ("decomposePar", _) | ("foamDictionary", _) => {}
            ("reconstructPar", _) => self.reconstruct = true,
            (app, None) if app == solver => self.solver = Some(app.to_string()),
            _ => miss(self, describe(r), false),
        }
    }

    fn radiation_mut(&mut self, region: &str) -> &mut Radiation {
        let rad = self.radiation.get_or_insert_with(|| Radiation {
            region: region.to_string(),
            lai_region: None,
            view_factors: false,
            solar_ray_tracing: false,
        });
        rad.region = region.to_string();
        rad
    }

    /// The scripts for `version`: `(file name, text)` for `Allprepare`, `Allrun` and `Allclean`.
    ///
    /// Names and paths come from the case's scripts and files, so they are untrusted: a name
    /// that isn't one path component, a path that leaves the case, or either with a control
    /// character or a leading `-` is refused (see `safe_name`, `safe_path`); the rest are
    /// quoted where the shell needs it.
    pub fn generate(&self, version: OfVersion) -> Result<Vec<(&'static str, String)>, ScriptError> {
        let (names, paths) = self.words();
        if let Some(bad) =
            names.into_iter().find(|w| !safe_name(w)).or_else(|| paths.into_iter().find(|p| !safe_path(p)))
        {
            return Err(ScriptError::Unsafe(bad.to_string()));
        }
        for m in &self.meshes {
            if let MeshBuild::Split(sp) = &m.build
                && let Err(why) = self.split_ok(&m.region, sp)
            {
                return Err(ScriptError::Split { region: m.region.clone(), why });
            }
        }
        Ok(vec![
            ("Allprepare", self.allprepare(version)),
            ("Allrun", self.allrun(version)),
            ("Allclean", self.allclean()),
        ])
    }

    /// What the scripts are written with: (names: regions, fields, files, the solver — single
    /// path components; paths: dictionaries and backups, relative to the case).
    fn words(&self) -> (Vec<&str>, Vec<&str>) {
        let mut names: Vec<&str> = self.regions.iter().map(String::as_str).collect();
        let mut paths = Vec::new();
        for m in &self.meshes {
            names.push(&m.region);
            match &m.build {
                MeshBuild::Root(steps) => {
                    for st in steps {
                        if let RootStep::BlockMesh(d) | RootStep::SnappyHexMesh(d) = st {
                            paths.push(d.as_str());
                        }
                    }
                }
                MeshBuild::Split(sp) => {
                    names.push(&sp.of);
                    names.extend(sp.zone.as_deref());
                    paths.push(&sp.dict);
                }
                MeshBuild::Block => {}
            }
            for st in &m.steps {
                match st {
                    RegionStep::Sets { setset_batch: Some(b) } => paths.push(b),
                    RegionStep::SetFields { restore } => {
                        for (b, f) in restore {
                            paths.push(b);
                            names.push(f);
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(r) = &self.radiation {
            names.push(&r.region);
            names.extend(r.lai_region.as_deref());
        }
        names.extend(self.solver.as_deref());
        for (r, files) in &self.constant_files {
            names.push(r);
            names.extend(files.iter().map(String::as_str));
        }
        (names, paths)
    }

    /// Whether the script can split `region` as `sp` says: the zone known, and neither it nor
    /// [`SPLIT_REST`] a region of the case (the split's clean-up removes their directories), the
    /// copied region another one, built before it.
    fn split_ok(&self, region: &str, sp: &Split) -> Result<(), String> {
        let Some(zone) = sp.zone.as_deref() else {
            return Err("the cell zone it is split by isn't known: name it (mesh.split)".into());
        };
        if sp.of == region {
            return Err("it is split from its own mesh: name another region".into());
        }
        for (name, what) in [(zone, "the zone"), (SPLIT_REST, "the part outside the zone")] {
            if self.regions.iter().chain(&self.case_regions).any(|r| r == name)
                || self.meshes.iter().any(|m| m.region == name)
            {
                return Err(format!(
                    "{what} is named {name}, as a region of the case: the split's clean-up would remove that region's directories"
                ));
            }
        }
        if let Some(n) = self.regions.iter().chain(&self.case_regions).find(|r| part_name(r)) {
            return Err(format!(
                "the case has a region named {n}, as splitMeshRegions names the parts outside the zone: rename it"
            ));
        }
        match self.split_zones.iter().find(|(r, _)| r == region).map(|(_, z)| z) {
            Some(DictZones::Missing) => {
                return Err(format!("{} isn't there: the zone {zone} can't be made", sp.dict));
            }
            Some(DictZones::Unclear) => {
                return Err(format!(
                    "which cell zones {} makes can't be told (it doesn't read, or uses #include or a macro): splitMeshRegions would split off every one; write its actions out",
                    sp.dict
                ));
            }
            Some(DictZones::Made(made)) if !made.iter().any(|z| z == zone) => {
                return Err(format!(
                    "{} doesn't make the cell zone {zone}: splitMeshRegions would split nothing off; make {zone} there",
                    sp.dict
                ));
            }
            _ => {}
        }
        if let Some((_, DictZones::Made(made))) = self.split_zones.iter().find(|(r, _)| r == region) {
            let others: Vec<&str> = made.iter().map(String::as_str).filter(|z| *z != zone).collect();
            if !others.is_empty() {
                return Err(format!(
                    "{} makes other cell zones too ({}): splitMeshRegions would split them off as well (their meshes written to constant/<zone>); make only {zone} there",
                    sp.dict,
                    others.join(", ")
                ));
            }
        }
        if part_name(zone) {
            return Err(format!("the zone is named {zone}, as splitMeshRegions names the parts outside it"));
        }
        let built_before = self.meshes.iter().take_while(|m| m.region != region).any(|m| m.region == sp.of);
        if !built_before {
            return Err(format!("{} isn't built before it: its mesh can't be copied", sp.of));
        }
        if !sp.dict.contains('/') {
            return Err(format!("{} is not in a directory of the case (system/…)", sp.dict));
        }
        Ok(())
    }

    fn allprepare(&self, version: OfVersion) -> String {
        let mut s = header("RunFunctions", "Builds the region meshes, sets and fields.");
        if self.features {
            s.push_str("runApplication -o surfaceFeatures\n");
        }
        for m in &self.meshes {
            let r = q(&m.region);
            let rq = &r;
            s.push_str(&format!("\n# Region {}\n", m.region));
            match &m.build {
                MeshBuild::Block => s.push_str(&format!("runApplication -o -s {r} blockMesh -region {r}\n")),
                MeshBuild::Root(steps) => {
                    // Another dictionary than the default is passed with -dict: the case's own
                    // system/blockMeshDict is neither overwritten nor removed.
                    let dict =
                        |d: &str, default: &str| if d == default { String::new() } else { format!(" -dict {}", q(d)) };
                    for st in steps {
                        match st {
                            RootStep::BlockMesh(d) => {
                                s.push_str(&format!("runApplication -o -s {rq} blockMesh{}\n", dict(d, BLOCK_DICT)))
                            }
                            RootStep::CreatePatch => {
                                s.push_str(&format!("runApplication -o -s {rq} createPatch -overwrite\n"))
                            }
                            RootStep::SnappyHexMesh(d) => s.push_str(&format!(
                                "runApplication -o -s {rq} snappyHexMesh -overwrite{}\n",
                                dict(d, SNAPPY_DICT)
                            )),
                        }
                    }
                    s.push_str(&format!(
                        "rm -rf constant/{rq}/polyMesh\nmv constant/polyMesh constant/{rq}/polyMesh\n"
                    ));
                }
                MeshBuild::Split(sp) => s.push_str(&split_script(&m.region, sp, version)),
            }
            for st in &m.steps {
                match st {
                    RegionStep::CreatePatch => {
                        s.push_str(&format!("runApplication -o -s {r} createPatch -region {r} -overwrite\n"))
                    }
                    RegionStep::ChangeDictionary => {
                        s.push_str(&format!("runApplication -o -s {r} changeDictionary -region {r}\n"))
                    }
                    RegionStep::Sets { setset_batch: Some(b) } if version == OfVersion::V8 => {
                        s.push_str(&format!("runApplication -o -s {r} setSet -region {r} -batch {} -noVTK\n", q(b)));
                    }
                    RegionStep::Sets { .. } => s.push_str(&format!("runApplication -o -s {r} topoSet -region {r}\n")),
                    RegionStep::SetFields { restore } => {
                        for (backup, field) in restore {
                            let f = q(field);
                            s.push_str(&format!("rm -f 0/{r}/{f} 0/{r}/{f}.gz\ncp {} 0/{r}/{f}\n", q(backup)));
                        }
                        s.push_str(&format!("runApplication -o -s {r} setFields -region {r}\n"));
                    }
                }
            }
        }
        s
    }

    fn allrun(&self, version: OfVersion) -> String {
        let mut s = header(
            "RunFunctions",
            "Decomposes the regions, pre-processes the radiation and runs the solver in parallel.",
        );
        s.push_str("rm -rf processor*\n\n");
        // Region by region: -allRegions needs controlDict regionSolvers in OpenFOAM-12.
        let regions: Vec<String> = self.regions.iter().map(|r| q(r)).collect();
        let regions = regions.join(" ");
        s.push_str(&format!(
            "for r in {regions}\ndo\n    runApplication -o -s \"$r\" decomposePar -region \"$r\"\ndone\n"
        ));
        // Solver builds before 40f31d4 (June 2026) test for these dictionaries in
        // processor*/constant/<region>/ and run without the model when they're not there; later
        // builds read them from constant/. Copying them works with both.
        let copies: Vec<String> = self
            .constant_files
            .iter()
            .filter(|(_, files)| !files.is_empty())
            .map(|(r, files)| {
                let list: Vec<String> = files.iter().map(|f| format!("constant/{r}/{f}")).collect();
                let list: Vec<String> = list.iter().map(|f| q(f)).collect();
                format!("    cp {} \"$p\"/{}/\n", list.join(" "), q(&format!("constant/{r}")))
            })
            .collect();
        if !copies.is_empty() {
            s.push_str(&format!(
                "\n# Region dictionaries into each processor directory (urbanMicroclimateFoam before June 2026 reads them there)\nfor p in processor*\ndo\n{}done\n",
                copies.concat()
            ));
        }
        // runParallel takes the process count from system/decomposeParDict, and reads it even
        // when given -np. OpenFOAM-8 decomposes each region by its own
        // system/<r>/decomposeParDict, which a case may have instead: there mpirun is called
        // directly, with the count from the first region's.
        let v8 = version == OfVersion::V8;
        if let (true, Some(first)) = (v8, self.regions.first()) {
            s.push_str(&format!(
                "\nnProcs=$(foamDictionary -entry numberOfSubdomains -value {})\n",
                q(&format!("system/{first}/decomposeParDict"))
            ));
        }
        let par = |app: &str, region: Option<&str>, extra: &str| -> String {
            match (v8, region) {
                (true, Some(r)) => {
                    format!("mpirun -np \"$nProcs\" {app} -region {r}{extra} -parallel > log.{app}.{r} 2>&1\n")
                }
                (true, None) => format!("mpirun -np \"$nProcs\" {app}{extra} -parallel > log.{app} 2>&1\n"),
                (false, Some(r)) => format!("runParallel -o -s {r} {app} -region {r}{extra}\n"),
                (false, None) => format!("runParallel -o {app}{extra}\n"),
            }
        };
        if let Some(rad) = &self.radiation {
            let v = q(&rad.region);
            s.push('\n');
            // OpenFOAM-12 faceAgglomerate reads system/viewFactorsDict; viewFactorsGen and
            // solarRayTracingGen read system/<region>/. OpenFOAM-8 reads all from constant/<region>/.
            let dict = if v8 {
                String::new()
            } else {
                format!(" -dict {}", q(&format!("system/{}/viewFactorsDict", rad.region)))
            };
            s.push_str(&par("faceAgglomerate", Some(&v), &dict));
            if let Some(lai) = rad.lai_region.as_deref().map(q) {
                s.push_str(&par("calcLAI", Some(&lai), ""));
            }
            if rad.view_factors {
                s.push_str(&par("viewFactorsGen", Some(&v), ""));
            }
            if rad.solar_ray_tracing {
                s.push_str(&par("solarRayTracingGen", Some(&v), ""));
            }
        }
        if let Some(solver) = self.solver.as_deref().map(q) {
            s.push('\n');
            s.push_str(&par(&solver, None, ""));
        }
        if self.reconstruct {
            s.push_str(&format!(
                "\nfor r in {regions}\ndo\n    runApplication -o -s \"$r\" reconstructPar -region \"$r\"\ndone\n"
            ));
        }
        s
    }

    fn allclean(&self) -> String {
        let mut s = header("CleanFunctions", "Removes what Allprepare and Allrun made.");
        s.push_str("cleanCase\n");
        // The meshes Allprepare builds (OpenFOAM-8's cleanCase leaves region meshes); not the
        // meshes of regions it doesn't build.
        let built: Vec<String> = self.meshes.iter().map(|m| q(&m.region)).collect();
        if !built.is_empty() {
            s.push_str(&format!("\nfor r in {}\ndo\n    rm -rf \"constant/$r/polyMesh\"\ndone\n", built.join(" ")));
        }
        if let Some(rad) = &self.radiation {
            let v = q(&format!("constant/{}", rad.region));
            s.push_str(&format!(
                "\n# Radiation pre-processing run serially\nfor f in {}\ndo\n    rm -f {v}/\"$f\" {v}/\"$f.gz\"\ndone\n",
                RADIATION_OUTPUTS.join(" ")
            ));
            if let Some(lai) = &rad.lai_region {
                let l = q(&format!("constant/{lai}"));
                s.push_str(&format!("rm -f {l}/divqrsw {l}/divqrsw.gz\n"));
            }
        }
        s
    }
}

/// The lines that split `region` from `sp.of`'s mesh: the copy
/// (without the zones it had), the zone made in it, the split, the part outside the zone moved
/// in, and only what the split made removed. With `-overwrite` the parts are written to
/// `constant/` in both versions, not to a time directory that depends on the time step;
/// `-noFields` writes none of the region's fields into them (it would read every field of
/// `0/<r>` on the copy). The split stops the script when the part outside the zone isn't
/// `region1` alone (a zone that takes no cells, or cuts the rest in two). OpenFOAM-8 reads a
/// `-dict` path given with `-region` below its first directory's region directory
/// (`system/air/topoSetDict` as `system/<r>/air/topoSetDict`), so the path goes up one there
/// (`system/../air/topoSetDict`); OpenFOAM-8 also writes each part's `fvSchemes` and
/// `fvSolution` into `system/<r>/<part>/`.
fn split_script(region: &str, sp: &Split, version: OfVersion) -> String {
    let r = &q(region);
    let of = q(&sp.of);
    let dict = match (version, sp.dict.split_once('/')) {
        (OfVersion::V8, Some((first, rest))) => format!("{first}/../{rest}"),
        _ => sp.dict.clone(),
    };
    let zone = q(sp.zone.as_deref().unwrap_or_default());
    let rest = SPLIT_REST;
    let mut s = format!("# the cells of {of} outside the cell zone {zone}\n");
    s.push_str(&format!(
        "rm -rf constant/{r}/polyMesh\nmkdir -p constant/{r}\ncp -r constant/{of}/polyMesh constant/{r}/polyMesh\n"
    ));
    s.push_str(&format!(
        "rm -rf constant/{r}/polyMesh/cellZones constant/{r}/polyMesh/cellZones.gz constant/{r}/polyMesh/sets\n"
    ));
    // What an earlier split left (no region is named so: see `split_ok`), so that the check
    // after this one sees only its parts.
    s.push_str(&format!("rm -rf {}\n", PARTS_LEFT));
    s.push_str(&format!("runApplication -o -s {r} topoSet -region {r} -dict {}\n", q(&dict)));
    s.push_str(&format!("runApplication -o -s {r} splitMeshRegions -region {r} -cellZones -overwrite -noFields\n"));
    // The message quoted whole: the names in it come from the case.
    let message = format!(
        "region {}: the split of {} by the cell zone {} left no single part outside it: the zone has no cells, or cuts the rest in pieces (log.splitMeshRegions.{})",
        region,
        sp.of,
        sp.zone.as_deref().unwrap_or_default(),
        region
    );
    s.push_str(&format!(
        "if [ ! -d constant/{rest}/polyMesh ] || [ -d constant/region2/polyMesh ]\nthen\n    echo {} >&2\n    exit 1\nfi\n",
        q(&message)
    ));
    s.push_str(&format!("rm -rf constant/{r}/polyMesh\nmv constant/{rest}/polyMesh constant/{r}/polyMesh\n"));
    s.push_str(&format!("rm -rf constant/{zone}/polyMesh\n"));
    let mut dirs = format!("constant/{rest} constant/{zone}");
    if version == OfVersion::V8 {
        let files: Vec<String> = [rest, zone.as_str()]
            .iter()
            .flat_map(|p| {
                ["fvSchemes", "fvSchemes.gz", "fvSolution", "fvSolution.gz"].map(|f| format!("system/{r}/{p}/{f}"))
            })
            .collect();
        s.push_str(&format!("rm -f {}\n", files.join(" ")));
        dirs.push_str(&format!(" system/{r}/{rest} system/{r}/{zone}"));
    }
    // Directories with anything else in them stay.
    s.push_str(&format!("rmdir {dirs} || true\n"));
    s.push_str(&format!(
        "rm -f constant/{r}/cellToRegion constant/{r}/cellToRegion.gz 0/{r}/cellToRegion 0/{r}/cellToRegion.gz\n"
    ));
    s
}

/// The meshes of the parts an earlier split left that the check after a split looks at
/// (`splitMeshRegions` names those outside every zone `region1`, `region2`, …), removed before
/// a split. Named, not a pattern: a pattern would also take a region's (`region1Solid`).
const PARTS_LEFT: &str = "constant/region1/polyMesh constant/region2/polyMesh";

/// Whether `name` is one `splitMeshRegions` may give a part outside every zone: `region` and a
/// number. No region of a case with a split may be named so.
pub fn part_name(name: &str) -> bool {
    name.strip_prefix("region").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// A copy of one region's mesh into another's, `cp -r constant/air/polyMesh
/// constant/vegetation/polyMesh` (or `constant/vegetation/`): (from, to).
fn split_copy(from: &[String], to: &str) -> Option<(String, String)> {
    let [f] = from else { return None };
    let of = f.trim_end_matches('/').strip_prefix("constant/")?.strip_suffix("/polyMesh")?;
    let t = to.trim_end_matches('/').strip_prefix("constant/")?;
    let region = match t.strip_suffix("/polyMesh") {
        Some(r) => r,
        None if to.ends_with('/') => t,
        None => return None,
    };
    let plain = |n: &str| !n.is_empty() && !n.contains('/') && !n.contains('*');
    // The split's part outside the zone put in is not a copy to split.
    (plain(of) && plain(region) && of != region && of != SPLIT_REST).then(|| (of.to_string(), region.to_string()))
}

/// A `-dict` path given with `-region`, as the file it names: OpenFOAM-8's way up from the
/// region's directory (`system/../air/topoSetDict`) as the path itself (`system/air/topoSetDict`).
fn region_dict(d: &str) -> String {
    match d.split_once("/../") {
        Some((first, rest)) if !first.contains('/') => format!("{first}/{rest}"),
        _ => d.to_string(),
    }
}

/// `splitMeshRegions`'s options a split by a cell zone has: `-region <r> -cellZones
/// [-overwrite] [-noFields]`.
fn split_args(args: &[String], region: &str) -> bool {
    args.iter().any(|a| a == "-cellZones")
        && args
            .iter()
            .all(|a| matches!(a.as_str(), "-region" | "-cellZones" | "-overwrite" | "-noFields") || a == region)
}

/// `topoSet`'s options on the copy: `-region <r> [-dict <path>]` (another option doesn't fit).
fn toposet_args(args: &[String], region: &str) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-region" => {
                if it.next().map(String::as_str) != Some(region) {
                    return false;
                }
            }
            "-dict" => {
                if it.next().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// The zones and sets of a copy of a mesh, taken away before its split:
/// `constant/vegetation/polyMesh/cellZones`, `…/sets`.
fn copy_zones(p: &str, region: &str) -> bool {
    p.trim_end_matches('/')
        .strip_prefix(&format!("constant/{region}/polyMesh/"))
        .is_some_and(|x| matches!(x, "cellZones" | "cellZones.gz" | "sets"))
        || p == "constant/region1/polyMesh"
        || p == "constant/region2/polyMesh"
}

/// A time directory's name (not the start time `0`): where `splitMeshRegions` without
/// `-overwrite` writes the parts.
fn is_time(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+'))
        && s.parse::<f64>().is_ok_and(|t| t > 0.0)
}

/// The split's part outside the zone put in as region `reg`'s mesh: `cp -r
/// 3600/region1/polyMesh constant/vegetation/`, `mv constant/region1/polyMesh
/// constant/vegetation/polyMesh`.
fn split_take(from: &[String], to: &str, reg: &str) -> bool {
    let [f] = from else { return false };
    let Some(dir) = f.trim_end_matches('/').strip_suffix(&format!("/{SPLIT_REST}/polyMesh")) else {
        return false;
    };
    let t = to.trim_end_matches('/');
    (dir == "constant" || is_time(dir)) && (t == format!("constant/{reg}") || t == format!("constant/{reg}/polyMesh"))
}

/// What a split of region `reg` by `zone` leaves that a script may take away: the parts
/// (`region1`, the zone, `region*`) in `constant/` and `0/`, or their meshes; their copies of
/// the region's `fvSchemes` and `fvSolution` in `system/<reg>/` (OpenFOAM-8); the region's
/// `cellToRegion` maps; and, after a split into a time directory (`timed`), a time directory.
fn split_leftover(p: &str, reg: &str, zone: Option<&str>, timed: bool) -> bool {
    let p = p.trim_end_matches('/');
    let part = |name: &str| name == SPLIT_REST || name == "region*" || Some(name) == zone;
    let under = |dir: &str, rest: &[&str]| {
        p.strip_prefix(dir).is_some_and(|x| {
            let (head, tail) = x.split_once('/').unwrap_or((x, ""));
            part(head) && (tail.is_empty() || rest.contains(&tail))
        })
    };
    let dicts = ["fvSchemes", "fvSchemes.gz", "fvSolution", "fvSolution.gz"];
    let map = |dir: &str| p.strip_prefix(&format!("{dir}/{reg}/")).is_some_and(|x| x.starts_with("cellToRegion"));
    under("constant/", &["polyMesh"])
        || under("0/", &[])
        || under(&format!("system/{reg}/"), &dicts)
        || map("constant")
        || map("0")
        || (timed && is_time(p))
}

/// The zone a split's clean-up names: `system/vegetation/trees` or `constant/trees/polyMesh`
/// (not `region1`, not the region split, not a pattern, not a dictionary's name).
fn split_zone(p: &str, reg: &str) -> Option<String> {
    let p = p.trim_end_matches('/');
    let z = match p.strip_prefix(&format!("system/{reg}/")) {
        Some(z) => z,
        None => p.strip_prefix("constant/")?.strip_suffix("/polyMesh").filter(|z| *z != reg)?,
    };
    let dictionary = z.ends_with("Dict")
        || z.contains('.')
        || matches!(z, "fvSchemes" | "fvSolution" | "fvConstraints" | "fvModels" | "fvOptions" | "controlDict");
    let plain = !z.is_empty() && !z.contains(['/', '*', '?', '[']) && !z.starts_with("region") && !dictionary;
    plain.then(|| z.to_string())
}

/// A copy of a region's mesh, and the `topoSet` on it, waiting for `splitMeshRegions`.
struct PendingSplit {
    region: String,
    of: String,
    dict: Option<String>,
    /// The script's lines so far, with what they run: reported when no split follows.
    lines: Vec<(usize, String)>,
}

impl PendingSplit {
    fn unused(self, wf: &mut Workflow, script: &str) {
        for (line, what) in self.lines {
            wf.unmatched.push(Unmatched { script: script.to_string(), line, what, redundant: false });
        }
    }
}

/// The cell zones a `topoSetDict` makes: the names of its `cellZoneSet` actions.
pub fn zones_made(doc: &umcf_foam_dict::Document) -> Vec<String> {
    zones_told(doc).unwrap_or_else(|| zones_seen(doc))
}

/// The cell zones a `topoSetDict` makes, when they can be told (its `cellZoneSet` actions in
/// order: a `remove` takes a zone away): `None` when the file has a directive or macro, a key
/// given twice (at its top or in an action), a `cellZoneSet` action without one plain `action`,
/// or `actions` isn't a list of plain dictionaries whose `name` and `type`
/// are single words (an `#include`, a `$macro`: what OpenFOAM reads may make others).
pub fn zones_told(doc: &umcf_foam_dict::Document) -> Option<Vec<String>> {
    use umcf_foam_dict::{Item, Node};
    // Entries only, each key once (OpenFOAM takes the last of a key given twice, the reader the
    // first).
    let plain = |d: &umcf_foam_dict::Dict| {
        let mut keys = std::collections::HashSet::new();
        d.items.iter().all(|i| matches!(i, Item::Entry(_))) && d.keys().all(|k| keys.insert(k))
    };
    if !plain(&doc.root) {
        return None;
    }
    let Some(entry) = doc.get(&["actions"]) else { return Some(Vec::new()) };
    let [Node::List { items, .. }] = entry.value.as_slice() else { return None };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Node::Dict { body, .. } = item else { return None };
        if !plain(body) {
            return None;
        }
        let word = |k: &str| match body.get(k).and_then(|e| e.words()).as_deref() {
            Some([w]) if !w.starts_with('$') && !w.starts_with('#') => Some(crate::description::unescaped(w)),
            _ => None,
        };
        let (Some(kind), Some(name)) = (word("type"), word("name")) else { return None };
        if kind == "cellZoneSet" {
            // In order: `remove` takes the zone away, any other action leaves it made.
            let action = word("action")?;
            apply(&mut out, name, &action);
        }
    }
    Some(out)
}

/// A `cellZoneSet` action on the zones made so far: `remove` takes `name` away, any other
/// leaves it made (`new`, `add`, `subset`, `clear`, …: the zone is there, even empty).
fn apply(out: &mut Vec<String>, name: String, action: &str) {
    if action == "remove" {
        out.retain(|z| *z != name);
    } else if !out.contains(&name) {
        out.push(name);
    }
}

/// The cell zones a `topoSetDict` names in its plain `cellZoneSet` actions, what can be read.
fn zones_seen(doc: &umcf_foam_dict::Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(entry) = doc.get(&["actions"]) else { return out };
    let [umcf_foam_dict::Node::List { items, .. }] = entry.value.as_slice() else { return out };
    for item in items {
        let umcf_foam_dict::Node::Dict { body, .. } = item else { continue };
        let word = |k: &str| match body.get(k).and_then(|e| e.words()).as_deref() {
            Some([w]) => Some(crate::description::unescaped(w)),
            _ => None,
        };
        if word("type").as_deref() == Some("cellZoneSet")
            && let Some(name) = word("name")
        {
            apply(&mut out, name, word("action").as_deref().unwrap_or("new"));
        }
    }
    out
}

/// A name the scripts may be written with: one path component (no `/`, not `.` or `..`), no
/// control character (a newline would end a comment or command and start another), no leading
/// `-` (an option even when quoted).
fn safe_name(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && s != "." && s != ".." && safe_word(s)
}

/// A path the scripts may be written with: relative, inside the case (no `..` component).
fn safe_path(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.split('/').any(|c| c == "..") && safe_word(p)
}

fn safe_word(s: &str) -> bool {
    !s.chars().any(char::is_control) && !s.starts_with('-')
}

/// A word for the shell: as it is when it has only safe characters, else single-quoted.
fn q(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./+-:@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn header(functions: &str, what: &str) -> String {
    let stop = if functions == "RunFunctions" { "set -e\n" } else { "" };
    format!(
        "#!/bin/sh\n# {what}\n# {}\ncd \"${{0%/*}}\" || exit 1\n. \"$WM_PROJECT_DIR/bin/tools/{functions}\"\n{stop}\n",
        crate::blocks::GENERATED
    )
}

/// The scripts that make up the workflow (not job wrappers or migration helpers).
pub fn is_workflow_script(path: &str) -> bool {
    matches!(path, "Allprepare" | "Allrun" | "Allrun.pre" | "Allmesh")
}

fn is_radiation(app: &str) -> bool {
    matches!(app, "faceAgglomerate" | "calcLAI" | "viewFactorsGen" | "solarRayTracingGen")
}

/// Dictionaries copied into place for one step and removed after.
fn is_temporary(p: &str) -> bool {
    matches!(p, "system/blockMeshDict" | "system/snappyHexMeshDict")
}

/// `cp 0/air.bckp/LAD 0/air/` or `cp 0/air.bckp/LAD 0/air/LAD`: (region, backup, field). Not
/// a whole directory (`cp -r 0.orig/air 0/air`): the destination must be a directory in `0/`
/// (`0/<r>/`) or name the same file there.
fn backup_restore(from: &[String], to: &str) -> Option<(String, String, String)> {
    let src = from.first()?;
    let (dir, field) = src.rsplit_once('/')?;
    let rest = to.trim_end_matches('/').strip_prefix("0/")?;
    let region = if to.ends_with('/') {
        rest
    } else {
        let (r, f) = rest.split_once('/')?;
        if f != field {
            return None;
        }
        r
    };
    let ok = !region.is_empty() && !region.contains('/') && !dir.starts_with("constant");
    ok.then(|| (region.to_string(), src.clone(), field.to_string()))
}

/// A step as the shell command it came from.
pub fn describe_step(k: &StepKind) -> String {
    match k {
        StepKind::Run(r) => describe(r),
        StepKind::Copy { from, to } => format!("cp {} {to}", from.join(" ")),
        StepKind::Move { from, to } => format!("mv {} {to}", from.join(" ")),
        StepKind::Remove { paths } => format!("rm {}", paths.join(" ")),
        StepKind::MakeDir { paths } => format!("mkdir {}", paths.join(" ")),
        StepKind::Write { path } => format!("> {path}"),
        StepKind::Clean { function } => function.clone(),
        StepKind::Call { script } => format!("./{script}"),
        StepKind::Other { command } => command.clone(),
    }
}

fn describe(r: &Run) -> String {
    let mut s = r.app.clone();
    for a in &r.args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
