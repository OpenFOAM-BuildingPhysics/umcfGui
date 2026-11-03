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
}

/// A name that isn't one path component, or a path that leaves the case, or either with a
/// control character or a leading `-`: [`Workflow::generate`] writes no script with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsafeName(pub String);

impl std::fmt::Display for UnsafeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} is not a plain name or a path inside the case (a control character, a leading `-`, `/` or `..`): no script is written with it",
            self.0
        )
    }
}

impl std::error::Error for UnsafeName {}

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
                StepKind::Remove { paths } if paths.iter().all(|p| is_temporary(p) || p == "constant/polyMesh") => {}
                StepKind::Remove { paths }
                    if paths.iter().all(|p| {
                        p.strip_prefix("constant/")
                            .and_then(|r| r.strip_suffix("/polyMesh"))
                            .is_some_and(|r| !r.contains('/'))
                    }) => {}
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
    pub fn generate(&self, version: OfVersion) -> Result<Vec<(&'static str, String)>, UnsafeName> {
        let (names, paths) = self.words();
        if let Some(bad) =
            names.into_iter().find(|w| !safe_name(w)).or_else(|| paths.into_iter().find(|p| !safe_path(p)))
        {
            return Err(UnsafeName(bad.to_string()));
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
            if let MeshBuild::Root(steps) = &m.build {
                for st in steps {
                    if let RootStep::BlockMesh(d) | RootStep::SnappyHexMesh(d) = st {
                        paths.push(d.as_str());
                    }
                }
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
        "#!/bin/sh\n# {what}\n# Written by umcfGui from the case workflow; edit the workflow, not this file.\ncd \"${{0%/*}}\" || exit 1\n. \"$WM_PROJECT_DIR/bin/tools/{functions}\"\n{stop}\n"
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
