//! Checks of the case's scripts and run set-up: what `Allprepare`, `Allrun` and the job
//! scripts run against what the case's OpenFOAM version has and what the case holds.

use std::collections::BTreeSet;

use umcf_core::OfVersion;
use umcf_model::script::{Np, Run, Script, Step, StepKind};
use umcf_model::workflow::describe_step;
use umcf_model::{CONTROL_DICT, Case, RegionKind};

use crate::utilities::{FORWARDED_IN_OF8, FORWARDED_IN_OF12, NOT_IN_OF8, NOT_IN_OF12};
use crate::{Diagnostic, Level};

pub(crate) fn check(case: &Case, out: &mut Vec<Diagnostic>) {
    let version = case.layout.version;
    let subdomains = subdomains(case, version, out);
    for s in &case.scripts {
        script(case, s, version, subdomains, out);
    }
    simulate(case, version, out);
    overwrites(case, out);
    let wf = case.workflow();
    for u in &wf.unmatched {
        let (code, what) = if u.redundant {
            ("workflow.not-needed", "is not needed (OpenFOAM reads these from the case, or an earlier step did it)")
        } else {
            ("workflow.custom", "doesn't fit the usual workflow; generated scripts would leave it out")
        };
        out.push(Diagnostic::new(Level::Info, code, &u.script, format!("line {}: `{}` {what}", u.line, u.what)));
    }
    fv(case, version, out);
}

/// The subdomain counts: (the regions are decomposed into, `system/decomposeParDict`'s).
///
/// OpenFOAM-8 `decomposePar -region r` reads `system/r/decomposeParDict` only (no fallback);
/// OpenFOAM-12 takes `numberOfSubdomains` from `system/decomposeParDict` for every region.
/// `runParallel` always reads `system/decomposeParDict`.
fn subdomains(case: &Case, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) -> (Option<u32>, Option<u32>) {
    let read = |rel: &str| case.dict(rel)?.value(&["numberOfSubdomains"])?.trim().parse::<u32>().ok();
    let global = read("system/decomposeParDict");
    let count = if version == Some(OfVersion::V8) {
        let counts: Vec<(String, u32)> = case
            .regions
            .iter()
            .filter(|r| !r.name.is_empty())
            .filter_map(|r| {
                let rel = format!("system/{}/decomposeParDict", r.name);
                read(&rel).map(|n| (rel, n))
            })
            .collect();
        // The count most regions use; the others are reported.
        let mut tally: Vec<(u32, usize)> = Vec::new();
        for (_, n) in &counts {
            match tally.iter_mut().find(|(m, _)| m == n) {
                Some((_, k)) => *k += 1,
                None => tally.push((*n, 1)),
            }
        }
        let common = tally.iter().max_by_key(|(_, k)| *k).map(|(n, _)| *n);
        if let Some(n) = common {
            for (rel, m) in counts.iter().filter(|(_, m)| *m != n) {
                out.push(Diagnostic::new(
                    Level::Warning,
                    "workflow.decompose",
                    rel,
                    format!(
                        "decomposes this region into {m} subdomains, the other regions into {n}: every region needs the same count, the parallel run uses one set of processor directories"
                    ),
                ));
            }
        }
        common.or(global)
    } else {
        global
    };
    if let Some(n) = count
        && case.processors > 0
        && case.processors != n as usize
    {
        out.push(Diagnostic::new(
            Level::Warning,
            "workflow.decomposed",
            "system/decomposeParDict",
            format!(
                "the case has {} processor directories but decomposes into {n}: they are from another decomposition (decompose again before a parallel run)",
                case.processors
            ),
        ));
    }
    (count, global)
}

fn missing_in(version: OfVersion) -> &'static [&'static str] {
    match version {
        OfVersion::V8 => NOT_IN_OF8,
        OfVersion::V12 => NOT_IN_OF12,
    }
}

fn forwarded_in(version: OfVersion) -> &'static [&'static str] {
    match version {
        OfVersion::V8 => FORWARDED_IN_OF8,
        OfVersion::V12 => FORWARDED_IN_OF12,
    }
}

fn script(
    case: &Case,
    s: &Script,
    version: Option<OfVersion>,
    (subdomains, global): (Option<u32>, Option<u32>),
    out: &mut Vec<Diagnostic>,
) {
    let diag = |level, code, line: usize, message: String| {
        Diagnostic::new(level, code, &s.path, format!("line {line}: {message}"))
    };
    if !s.shebang && !s.first_line.is_empty() {
        out.push(diag(
            Level::Warning,
            "workflow.shebang",
            1,
            format!("`{}` is not a `#!` line: the script only runs when a shell runs it itself (the line is then run as a command)", s.first_line),
        ));
    }
    let mut needs = None::<u32>;
    for st in &s.steps {
        match &st.kind {
            StepKind::Run(r) => {
                if let Some(v) = version
                    && missing_in(v).contains(&r.app.as_str())
                {
                    let level = if st.conditional { Level::Warning } else { Level::Error };
                    let hint = match r.app.as_str() {
                        "setSet" => ": use topoSet with a topoSetDict",
                        "foamCleanPolyMesh" => ": cleanCase removes the meshes, or rm -rf constant/<region>/polyMesh",
                        "surfaceFeatureExtract" => ": use surfaceFeatures with a surfaceFeaturesDict",
                        "foamCleanCase" => ": use cleanCase from CleanFunctions",
                        _ => "",
                    };
                    out.push(diag(
                        level,
                        "workflow.utility",
                        st.line,
                        format!("`{}` is not in {}{hint}", r.app, v.label()),
                    ));
                }
                if let Some(v) = version
                    && forwarded_in(v).contains(&r.app.as_str())
                {
                    out.push(diag(
                        Level::Warning,
                        "workflow.utility",
                        st.line,
                        format!("`{}` is superseded in {}: it runs its replacement (foamRun, foamPostProcess), which may need the case changed", r.app, v.label()),
                    ));
                }
                if r.all_regions {
                    all_regions(case, s, st, r, version, out);
                }
                if r.app == "calcLAI" && case.region("vegetation").is_none() {
                    out.push(diag(
                        Level::Error,
                        "workflow.calclai",
                        st.line,
                        "calcLAI reads the region named `vegetation` (the name is fixed in the solver's calcLAI), and the case has none".into(),
                    ));
                }
                if r.run_parallel && global.is_none() {
                    out.push(diag(
                        Level::Warning,
                        "workflow.np",
                        st.line,
                        format!("`runParallel {}` reads the process count from system/decomposeParDict (even with -np), which the case doesn't have or whose numberOfSubdomains isn't a number", r.app),
                    ));
                }
                let np = match &r.np {
                    Some(Np::Count(n)) => Some((*n, "")),
                    Some(Np::FromDict) => {
                        global.map(|g| (g, " (runParallel: numberOfSubdomains of system/decomposeParDict)"))
                    }
                    _ => None,
                };
                if let (Some((n, how)), Some(sub)) = (np, subdomains)
                    && n != sub
                    && r.parallel
                {
                    out.push(diag(
                        Level::Error,
                        "workflow.np",
                        st.line,
                        format!(
                            "`{}` runs on {n} processes{how}, but the case is decomposed into {sub}: OpenFOAM stops",
                            r.app
                        ),
                    ));
                }
                match r.np {
                    Some(Np::Count(n)) => needs = needs.max(Some(n)),
                    Some(Np::FromDict) => needs = needs.max(subdomains),
                    _ if r.parallel => needs = needs.max(subdomains),
                    _ => {}
                }
            }
            StepKind::Call { script } if script == "Allrun" => needs = needs.max(subdomains),
            _ => {}
        }
    }
    if let (Some(t), Some(n)) = (s.ntasks, needs)
        && t < n
    {
        out.push(Diagnostic::new(
            Level::Warning,
            "workflow.ntasks",
            &s.path,
            format!("the job asks the batch system for {t} tasks, but runs {n} processes"),
        ));
    }
}

fn all_regions(case: &Case, s: &Script, st: &Step, r: &Run, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let message = match version {
        // OpenFOAM-12 setRegionNames.H: controlDict.subDict("regionSolvers").toc().
        Some(OfVersion::V12) => {
            let solvers: Option<BTreeSet<String>> = case
                .dict(CONTROL_DICT)
                .and_then(|d| d.get(&["regionSolvers"]))
                .and_then(|e| e.dict())
                .map(|d| d.keys().map(String::from).collect());
            match solvers {
                None => Some((
                    Level::Error,
                    format!(
                        "`{} -allRegions` reads the region list from controlDict `regionSolvers`, which the case doesn't have: OpenFOAM-12 stops (add `regionSolvers {{ <region> <solver>; … }}`, or run it with -region for each region)",
                        r.app
                    ),
                )),
                Some(names) => {
                    let regions: BTreeSet<String> =
                        case.regions.iter().filter(|r| r.kind != RegionKind::Single).map(|r| r.name.clone()).collect();
                    (names != regions).then(|| {
                        (
                            Level::Warning,
                            format!(
                                "`{} -allRegions` works on controlDict `regionSolvers` ({}), which differs from the case's regions ({})",
                                r.app,
                                names.into_iter().collect::<Vec<_>>().join(" "),
                                regions.into_iter().collect::<Vec<_>>().join(" ")
                            ),
                        )
                    })
                }
            }
        }
        // OpenFOAM-8: regionProperties.
        Some(OfVersion::V8) if case.exists("constant/regionProperties") == Some(false) => Some((
            Level::Error,
            format!("`{} -allRegions` reads constant/regionProperties, which the case doesn't have", r.app),
        )),
        _ => None,
    };
    if let Some((level, message)) = message {
        let level = if st.conditional && level == Level::Error { Level::Warning } else { level };
        out.push(Diagnostic::new(level, "workflow.all-regions", &s.path, format!("line {}: {message}", st.line)));
    }
}

// ---------------------------------------------------------------------------------------------
// Files at each step

/// The dictionary `app` reads, in `version`, for `region`: the places it looks, in order (it
/// uses the first that is there). Verified in the OpenFOAM-8 and 12 sources (systemDictIO,
/// setSystemMeshDictionaryIO.H, setConstantMeshDictionaryIO.H, blockMesh.C, decomposePar.C) and
/// the solver (`solarRayTracingGen`). Empty when the application reads none this knows.
fn dictionary(r: &Run, version: OfVersion, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let reg = r.region.as_deref();
    let name = match r.app.as_str() {
        "blockMesh" => "blockMeshDict",
        "snappyHexMesh" => "snappyHexMeshDict",
        "createPatch" => "createPatchDict",
        "changeDictionary" => "changeDictionaryDict",
        "topoSet" => "topoSetDict",
        "setFields" => "setFieldsDict",
        "extrudeMesh" => "extrudeMeshDict",
        "surfaceFeatures" => "surfaceFeaturesDict",
        "faceAgglomerate" | "viewFactorsGen" | "solarRayTracingGen" => "viewFactorsDict",
        "decomposePar" => "decomposeParDict",
        "setSet" => {
            let k = r.args.iter().position(|a| a == "-batch");
            return k.and_then(|k| r.args.get(k + 1)).cloned().into_iter().collect();
        }
        _ => return Vec::new(),
    };
    // -allRegions reads each region's own: not followed here.
    if r.all_regions {
        return Vec::new();
    }
    if let Some(d) = &r.dict {
        let inside = format!("{}/{name}", d.trim_end_matches('/'));
        return vec![if exists(&inside) { inside } else { d.trim_end_matches('/').to_string() }];
    }
    let sys = |reg: Option<&str>| match reg {
        Some(g) => format!("system/{g}/{name}"),
        None => format!("system/{name}"),
    };
    let constant = |reg: Option<&str>| match reg {
        Some(g) => format!("constant/{g}/{name}"),
        None => format!("constant/{name}"),
    };
    match (r.app.as_str(), version) {
        // blockMesh looks in constant/[<r>/]polyMesh/ first.
        ("blockMesh", _) => {
            let mesh = match reg {
                Some(g) => format!("constant/{g}/polyMesh/{name}"),
                None => format!("constant/polyMesh/{name}"),
            };
            vec![mesh, sys(reg)]
        }
        // Read with the case's Time, not the region's mesh.
        ("extrudeMesh" | "surfaceFeatures", _) => vec![sys(None)],
        ("faceAgglomerate" | "decomposePar", OfVersion::V12) => vec![sys(None)],
        ("faceAgglomerate" | "viewFactorsGen" | "solarRayTracingGen", OfVersion::V8) => vec![constant(reg)],
        _ => vec![sys(reg)],
    }
}

/// What a step leaves in the case, as far as later steps read it.
#[derive(Debug)]
enum Op {
    /// The path (and everything below it) exists.
    Made(String),
    /// Paths matching the glob (and everything below them) are gone.
    Gone(String),
}

struct Sim<'a> {
    case: &'a Case,
    ops: Vec<Op>,
}

impl Sim<'_> {
    fn exists(&self, p: &str) -> Option<bool> {
        let p = p.trim_end_matches('/');
        for op in self.ops.iter().rev() {
            match op {
                Op::Made(q) if p == q || under(p, q) => return Some(true),
                Op::Gone(g) if glob(g, p) || ancestors(p).any(|a| glob(g, a)) => return Some(false),
                _ => {}
            }
        }
        self.case.exists(p)
    }

    fn is_dir(&self, p: &str) -> bool {
        let p = p.trim_end_matches('/');
        self.ops.iter().any(|o| matches!(o, Op::Made(q) if under(q, p))) || self.case.is_dir(p)
    }
}

fn under(p: &str, dir: &str) -> bool {
    p.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

fn ancestors(p: &str) -> impl Iterator<Item = &str> {
    let mut cur = p;
    std::iter::from_fn(move || {
        let (parent, _) = cur.rsplit_once('/')?;
        cur = parent;
        Some(parent)
    })
}

/// Shell glob over a whole path: `*` and `?` (not crossing `/`), `[…]` as one character.
fn glob(pat: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len())
                .take_while(|&k| k == 0 || s.get(k - 1) != Some(&'/'))
                .any(|k| go(rest, s.get(k..).unwrap_or(&[]))),
            Some(('?', rest)) => s.first().is_some_and(|c| *c != '/') && go(rest, s.get(1..).unwrap_or(&[])),
            Some(('[', rest)) => match rest.iter().position(|c| *c == ']') {
                Some(end) => {
                    let set = rest.get(..end).unwrap_or(&[]);
                    s.first().is_some_and(|c| set.contains(c))
                        && go(rest.get(end + 1..).unwrap_or(&[]), s.get(1..).unwrap_or(&[]))
                }
                None => s.first() == Some(&'[') && go(rest, s.get(1..).unwrap_or(&[])),
            },
            Some((c, rest)) => s.first() == Some(c) && go(rest, s.get(1..).unwrap_or(&[])),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = s.chars().collect();
    go(&p, &s)
}

fn has_glob(p: &str) -> bool {
    p.contains(['*', '?', '['])
}

/// Applications whose effect on the case is known well enough to keep following the script.
fn known_app(app: &str, solver: &str) -> bool {
    app == solver
        || matches!(
            app,
            "blockMesh"
                | "snappyHexMesh"
                | "createPatch"
                | "changeDictionary"
                | "topoSet"
                | "setSet"
                | "setFields"
                | "extrudeMesh"
                | "surfaceFeatures"
                | "faceAgglomerate"
                | "viewFactorsGen"
                | "solarRayTracingGen"
                | "calcLAI"
                | "checkMesh"
                | "decomposePar"
                | "reconstructPar"
                | "reconstructParMesh"
                | "foamDictionary"
                | "urbanMicroclimateFoam"
                | "renumberMesh"
                | "foamToVTK"
                | "transformPoints"
        )
}

/// Follow the scripts step by step: a file a step reads (a copy's source, a utility's
/// dictionary) that isn't there at that point. `Allprepare` then `Allrun` are followed as one
/// sequence; other scripts each from the case as it is. `Allclean` is not followed.
fn simulate(case: &Case, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    let Some(version) = version else { return };
    let solver = case.controls.application.as_deref().unwrap_or("urbanMicroclimateFoam");
    let order = |p: &str| match p {
        "Allprepare" | "Allmesh" => 0,
        "Allrun.pre" => 1,
        "Allrun" => 2,
        _ => 3,
    };
    let mut scripts: Vec<&Script> = case.scripts.iter().filter(|s| s.path != "Allclean").collect();
    scripts.sort_by_key(|s| (order(&s.path), s.path.clone()));
    let mut chain = Sim { case, ops: Vec::new() };
    // Once a script of the chain can't be followed to its end, the next ones start from a case
    // that isn't known: they are not judged.
    let mut chain_known = true;
    for s in scripts {
        if order(&s.path) < 3 {
            if chain_known {
                chain_known = follow(&mut chain, s, version, solver, out);
            }
        } else {
            let mut own = Sim { case, ops: Vec::new() };
            let _ = follow(&mut own, s, version, solver, out);
        }
    }
}

/// Follow one script; false when it stopped at a step whose effect isn't known.
fn follow(sim: &mut Sim, s: &Script, version: OfVersion, solver: &str, out: &mut Vec<Diagnostic>) -> bool {
    let mut report = |line: usize, message: String| {
        out.push(Diagnostic::new(Level::Warning, "workflow.missing", &s.path, format!("line {line}: {message}")));
    };
    for st in &s.steps {
        // Steps whose paths aren't known: only those that write into processor directories
        // leave the rest of the case as it was.
        if st.unresolved {
            let into_processors = match &st.kind {
                StepKind::Copy { to, .. } | StepKind::Move { to, .. } | StepKind::Write { path: to } => {
                    to.starts_with("processor") || to.starts_with("${")
                }
                StepKind::MakeDir { paths } => paths.iter().all(|p| p.starts_with("processor") || p.starts_with("${")),
                StepKind::Run(r) => known_app(&r.app, solver),
                _ => false,
            };
            if into_processors {
                continue;
            }
            return false;
        }
        let check = !st.conditional;
        match &st.kind {
            StepKind::Copy { from, to } | StepKind::Move { from, to } => {
                let mv = matches!(st.kind, StepKind::Move { .. });
                let to_dir = to.ends_with('/') || from.len() > 1 || sim.is_dir(to);
                for f in from {
                    if f.starts_with('/') || f.starts_with("..") {
                        continue;
                    }
                    if !has_glob(f) && check && sim.exists(f) == Some(false) {
                        let verb = if mv { "moves" } else { "copies" };
                        report(
                            st.line,
                            format!("`{}` {verb} {f}, which isn't there at that point", describe_step(&st.kind)),
                        );
                    }
                    let name = f.rsplit('/').next().unwrap_or(f);
                    let dest = if to_dir {
                        format!("{}/{name}", to.trim_end_matches('/'))
                    } else {
                        to.trim_end_matches('/').to_string()
                    };
                    if mv && !st.conditional {
                        sim.ops.push(Op::Gone(f.clone()));
                    }
                    if !has_glob(f) {
                        sim.ops.push(Op::Made(dest));
                    }
                }
            }
            StepKind::Remove { paths } => {
                if !st.conditional {
                    sim.ops.extend(paths.iter().map(|p| Op::Gone(p.trim_end_matches('/').to_string())));
                }
            }
            StepKind::MakeDir { paths } => {
                sim.ops.extend(paths.iter().map(|p| Op::Made(p.trim_end_matches('/').to_string())))
            }
            StepKind::Write { path } => sim.ops.push(Op::Made(path.trim_end_matches('/').to_string())),
            StepKind::Run(r) => {
                if !known_app(&r.app, solver) {
                    return false;
                }
                let places = dictionary(r, version, |p| sim.exists(p) == Some(true));
                if check
                    && let Some(d) = places.last()
                    && places.iter().all(|p| sim.exists(p) == Some(false))
                {
                    report(
                        st.line,
                        format!("`{}` reads {d}, which isn't there at that point", describe_step(&st.kind)),
                    );
                }
                let mesh = match &r.region {
                    Some(g) => format!("constant/{g}/polyMesh"),
                    None => "constant/polyMesh".to_string(),
                };
                match r.app.as_str() {
                    "blockMesh" | "snappyHexMesh" | "extrudeMesh" => sim.ops.push(Op::Made(mesh)),
                    "topoSet" | "setSet" => sim.ops.push(Op::Made(format!("{mesh}/sets"))),
                    "surfaceFeatures" => {
                        sim.ops.push(Op::Made("constant/triSurface".into()));
                        sim.ops.push(Op::Made("constant/extendedFeatureEdgeMesh".into()));
                    }
                    "decomposePar" => sim.ops.push(Op::Made("processor0".into())),
                    _ => {}
                }
            }
            // A clean function, another script, a command not followed: what is there after
            // it is not known.
            StepKind::Clean { .. } | StepKind::Call { .. } | StepKind::Other { .. } => return false,
        }
    }
    true
}

/// A copy in `Allprepare`/`Allrun` over a dictionary of the case, from one that differs: the
/// copy is the one used, and edits to the destination are lost each time the script runs.
fn overwrites(case: &Case, out: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    for s in case.scripts.iter().filter(|s| umcf_model::workflow::is_workflow_script(&s.path)) {
        for st in s.steps.iter().filter(|st| !st.unresolved) {
            let StepKind::Copy { from, to } = &st.kind else { continue };
            let [src] = from.as_slice() else { continue };
            let name = src.rsplit('/').next().unwrap_or(src);
            let dest = if to.ends_with('/') { format!("{to}{name}") } else { to.clone() };
            let (Some(a), Some(b)) = (case.dict(src), case.dict(&dest)) else { continue };
            if a != b && seen.insert((src.clone(), dest.clone())) {
                // system/blockMeshDict and system/snappyHexMeshDict are the scripts' working
                // copies of the region variants (`blockMeshDict.air`).
                let (level, message) = if matches!(dest.as_str(), "system/blockMeshDict" | "system/snappyHexMeshDict") {
                    (
                        Level::Info,
                        format!(
                            "a working copy: {} line {} copies {src} here before meshing; edit {src} instead",
                            s.path, st.line
                        ),
                    )
                } else {
                    (
                        Level::Warning,
                        format!(
                            "{} line {} copies {src} over this file, and they differ: {src} is the one used, edits here are lost when the script runs",
                            s.path, st.line
                        ),
                    )
                };
                out.push(Diagnostic::new(level, "workflow.overwrites", &dest, message));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// fvConstraints and pressure limits (OpenFOAM-12)

fn fv(case: &Case, version: Option<OfVersion>, out: &mut Vec<Diagnostic>) {
    if version != Some(OfVersion::V12) {
        return;
    }
    for r in &case.regions {
        let dir = if r.name.is_empty() { "system".to_string() } else { format!("system/{}", r.name) };
        let constraints = format!("{dir}/fvConstraints");
        let options = format!("{dir}/fvOptions");
        // OpenFOAM-12 fvConstraints.C reads fvOptions only when fvConstraints is absent.
        if case.exists(&constraints) == Some(true) && case.exists(&options) == Some(true) {
            out.push(
                Diagnostic::new(
                    Level::Warning,
                    "fv.options-ignored",
                    &options,
                    format!("OpenFOAM-12 reads fvOptions only when there is no fvConstraints: with {constraints} there, this file is not read (move its constraints into fvConstraints)"),
                )
                .region(&r.name),
            );
        }
        if let Some(d) = case.dict(&constraints) {
            for e in d.root.entries() {
                let limit = e
                    .dict()
                    .and_then(|c| c.get("type"))
                    .and_then(|t| t.value_text())
                    .is_some_and(|t| umcf_foam_dict::unquote(t.trim()) == "limitPressure");
                if limit {
                    out.push(
                        Diagnostic::new(
                            Level::Warning,
                            "fv.limit-pressure",
                            &constraints,
                            format!("`{}` (limitPressure) has no effect: urbanMicroclimateFoam constrains the p_rgh equation and U, never p", e.key()),
                        )
                        .region(&r.name),
                    );
                }
            }
        }
        if r.kind == RegionKind::Fluid {
            let solution = format!("{dir}/fvSolution");
            // OpenFOAM-12 pressureReference: these keywords stop the run.
            let keys = ["pMin", "pMax", "pMinFactor", "pMaxFactor", "rhoMin", "rhoMax"];
            let found: Vec<&str> = keys
                .into_iter()
                .filter(|k| case.dict(&solution).is_some_and(|d| d.get(&["SIMPLE", k]).is_some()))
                .collect();
            if !found.is_empty() {
                out.push(
                    Diagnostic::new(
                        Level::Error,
                        "control.pressure-limits",
                        &solution,
                        format!(
                            "SIMPLE has {}: OpenFOAM-12 stops on these (it takes pressure limits from a limitPressure fvConstraint instead, which urbanMicroclimateFoam doesn't apply): remove them",
                            found.join(", ")
                        ),
                    )
                    .region(&r.name),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("0/air/LAD*", "0/air/LAD.gz"));
        assert!(glob("processor*", "processor12"));
        assert!(!glob("constant/air/F*", "constant/air/sub/F"));
        assert!(glob("constant/*/polyMesh", "constant/air/polyMesh"));
        assert!(glob("a[bc]d", "acd") && !glob("a[bc]d", "aed"));
        assert!(glob("log.*", "log.blockMesh.air"));
    }
}
