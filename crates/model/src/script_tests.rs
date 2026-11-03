use super::*;

fn runs(s: &Script) -> Vec<String> {
    s.steps
        .iter()
        .filter_map(|st| match &st.kind {
            StepKind::Run(r) => Some(match &r.region {
                Some(reg) => format!("{} -region {reg}", r.app),
                None => r.app.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn allprepare_copies_moves_and_loops() {
    let s = parse(
        "Allprepare",
        "#!/bin/sh\n\
         cd ${0%/*} || exit 1    # run from this directory\n\
         . $WM_PROJECT_DIR/bin/tools/RunFunctions\n\
         rm -rf log\n\
         cp system/blockMeshDict.air system/blockMeshDict\n\
         blockMesh\n\
         snappyHexMesh -overwrite\n\
         mv constant/polyMesh constant/air/.\n\
         changeDictionary -region air > log.changeDictionary.air 2>&1\n\
         for i in buildings street roofs\n\
         do\n\
         \tblockMesh -region $i > log.blockMesh.$i 2>&1\n\
         \tsetSet -region $i -noVTK -batch system/$i/setset.batch > log.setSet.$i 2>&1\n\
         done\n\
         mkdir -p log\n",
    );
    assert!(s.shebang);
    assert_eq!(
        runs(&s),
        [
            "blockMesh",
            "snappyHexMesh",
            "changeDictionary -region air",
            "blockMesh -region buildings",
            "setSet -region buildings",
            "blockMesh -region street",
            "setSet -region street",
            "blockMesh -region roofs",
            "setSet -region roofs",
        ]
    );
    let kinds: Vec<&StepKind> = s.steps.iter().map(|s| &s.kind).collect();
    assert!(kinds.contains(&&StepKind::Copy {
        from: vec!["system/blockMeshDict.air".into()],
        to: "system/blockMeshDict".into()
    }));
    assert!(kinds.contains(&&StepKind::Move { from: vec!["constant/polyMesh".into()], to: "constant/air/".into() }));
    assert!(kinds.contains(&&StepKind::Remove { paths: vec!["log".into()] }));
    assert!(kinds.contains(&&StepKind::Write { path: "log.blockMesh.roofs".into() }));
    // The script's own `cd` keeps it in the case, so nothing is unresolved.
    assert!(s.steps.iter().all(|s| !s.unresolved), "{:?}", s.steps);
    // Line numbers count from 1 and follow the loop body.
    let set = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "setSet")).unwrap();
    assert_eq!(set.line, 13);
}

#[test]
fn mpirun_and_batch_jobs() {
    let s = parse(
        "job_run.sh",
        "#!/bin/bash\n#SBATCH --ntasks=20\n#SBATCH --mem-per-cpu=4G\nsource of.sh\ncd ${SLURM_SUBMIT_DIR}\n\
         mpirun -np 20 faceAgglomerate -region vegetation -parallel\n\
         mpirun --bind-to core -np 8 urbanMicroclimateFoam -parallel > log.UMC 2>&1\n\
         srun --ntasks=4 calcLAI -region air -parallel\n\
         decomposePar -allRegions\n./Allrun\n",
    );
    assert_eq!(s.ntasks, Some(20));
    let rs: Vec<&Run> = s
        .steps
        .iter()
        .filter_map(|s| match &s.kind {
            StepKind::Run(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(rs[0].np, Some(Np::Count(20)));
    assert_eq!(rs[0].region.as_deref(), Some("vegetation"));
    assert!(rs[0].parallel);
    assert_eq!((rs[1].app.as_str(), &rs[1].np), ("urbanMicroclimateFoam", &Some(Np::Count(8))));
    assert_eq!((rs[2].app.as_str(), &rs[2].np), ("calcLAI", &Some(Np::Count(4))));
    assert!(rs[3].all_regions && rs[3].np.is_none());
    assert!(s.steps.iter().any(|s| s.kind == StepKind::Call { script: "Allrun".into() }));
    let pbs = parse("job.pbs", "#PBS -l select=2:ncpus=48:mpiprocs=24\n");
    assert_eq!(pbs.ntasks, Some(48));
    let nodes = parse("job.sh", "#SBATCH -N 2\n#SBATCH --ntasks-per-node 16\n");
    assert_eq!(nodes.ntasks, Some(32));
}

#[test]
fn unresolved_words_are_marked_not_guessed() {
    let s = parse(
        "Allrun",
        "nProcs=$(foamDictionary system/decomposeParDict -entry numberOfSubdomains -value)\n\
         for i in $(seq 0 $((nProcs-1))); do\n\
         \x20   cp constant/air/g \\\n\
         \x20      processor${i}/constant/air/\n\
         done\n\
         mpirun -np $nProcs urbanMicroclimateFoam -parallel\n\
         region=air\n\
         topoSet -region $region\n\
         cd constant\n\
         rm -f polyMesh/sets/x\n",
    );
    let cp = s.steps.iter().find(|s| matches!(s.kind, StepKind::Copy { .. })).unwrap();
    assert!(cp.unresolved, "{cp:?}");
    assert_eq!(cp.line, 3);
    let mpi = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "urbanMicroclimateFoam")).unwrap();
    assert!(mpi.unresolved);
    assert!(matches!(&mpi.kind, StepKind::Run(r) if r.np == Some(Np::Unknown)));
    let topo = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "topoSet")).unwrap();
    assert!(!topo.unresolved);
    assert!(matches!(&topo.kind, StepKind::Run(r) if r.region.as_deref() == Some("air")));
    // After `cd constant`, paths are relative to somewhere else.
    assert!(s.steps.last().unwrap().unresolved);
}

#[test]
fn conditionals_heredocs_and_functions() {
    let s = parse(
        "Allprepare",
        "#!/bin/bash\nset -e\n\
         if [ -d constant/polyMesh ]; then\n    mv constant/polyMesh constant/air/.\nfi\n\
         if [ -f system/x/createPatchDict ]; then createPatch -region x -overwrite; fi\n\
         cat > system/extra <<EOF\nrm -rf everything\nEOF\n\
         f() {\n  rm -rf notRun\n}\n\
         runApplication -s air surfaceFeatures\n\
         runParallel urbanMicroclimateFoam\n\
         blockMesh || exit 1\n\
         echo 'cp a b' # not a copy\n",
    );
    let mv = s.steps.iter().find(|s| matches!(s.kind, StepKind::Move { .. })).unwrap();
    assert!(mv.conditional);
    let cp = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "createPatch")).unwrap();
    assert!(cp.conditional);
    assert!(s.steps.iter().any(|s| s.kind == StepKind::Write { path: "system/extra".into() }));
    // Neither the here-document nor the function body runs.
    assert!(!s.steps.iter().any(
        |s| matches!(&s.kind, StepKind::Remove { paths } if paths.iter().any(|p| p == "everything" || p == "notRun"))
    ));
    assert_eq!(runs(&s), ["createPatch -region x", "surfaceFeatures", "urbanMicroclimateFoam", "blockMesh"]);
    let par = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "urbanMicroclimateFoam")).unwrap();
    assert!(matches!(&par.kind, StepKind::Run(r) if r.parallel && r.np == Some(Np::FromDict)));
    // `blockMesh || exit 1`: blockMesh runs unconditionally.
    let bm = s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "blockMesh")).unwrap();
    assert!(!bm.conditional);
}

#[test]
fn broken_first_line_and_clean_functions() {
    let s = parse("Allprepare", "$#!/bin/sh\ncleanCase\nfoamCleanPolyMesh -region air\nrestore0Dir\n");
    assert!(!s.shebang);
    assert_eq!(s.first_line, "$#!/bin/sh");
    // bash runs the broken line as a command (`0!/bin/sh`: not found).
    assert!(matches!(&s.steps[0].kind, StepKind::Other { .. }));
    assert_eq!(s.steps[1].kind, StepKind::Clean { function: "cleanCase".into() });
    assert_eq!(runs(&s), ["foamCleanPolyMesh -region air"]);
    assert!(s.steps.iter().any(|s| s.kind == StepKind::Copy { from: vec!["0.orig".into()], to: "0".into() }));
}

#[test]
fn script_names_and_paths() {
    for n in ["Allrun", "Allprepare", "Allclean", "Allrun.pre", "job_run.sh", "run.slurm"] {
        assert!(is_script_name(n), "{n}");
    }
    for n in ["Allrun.orig", "Allrun~", ".Allrun.swp", "README", "controlDict"] {
        assert!(!is_script_name(n), "{n}");
    }
    assert_eq!(normal("./constant//air/./polyMesh"), "constant/air/polyMesh");
    assert_eq!(normal("constant/air/."), "constant/air/");
    assert_eq!(normal("/abs/x/"), "/abs/x/");
}

#[test]
fn loops_over_globs_keep_the_pattern() {
    let s = parse(
        "Allrun",
        "for p in processor*; do\n    [ -d \"$p\" ] || continue\n    mkdir -p \"$p/constant/air\"\n    cp constant/air/g \"$p/constant/air/\"\ndone\n",
    );
    let cp = s.steps.iter().find(|s| matches!(s.kind, StepKind::Copy { .. })).unwrap();
    assert_eq!(cp.kind, StepKind::Copy { from: vec!["constant/air/g".into()], to: "processor*/constant/air/".into() });
    assert!(cp.unresolved);
    assert!(!s.steps.iter().any(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "continue")));
}

#[test]
fn subshells_word_splitting_test_guards_and_arrays() {
    let s = parse(
        "Allprepare",
        "(cd constant && ls)\nblockMesh -region air\n\
         SOLIDS=\"buildings street\"\nfor i in $SOLIDS; do blockMesh -region $i; done\n\
         for i in \"$SOLIDS\"; do echo \"$i\"; done\n\
         [ -f system/foo ] && cp system/foo system/bar\n\
         cd . && cp system/a system/b\n\
         WARNINGS=(a b)\n",
    );
    // The subshell's `cd` stays inside it.
    let air =
        s.steps.iter().find(|s| matches!(&s.kind, StepKind::Run(r) if r.region.as_deref() == Some("air"))).unwrap();
    assert!(!air.unresolved);
    assert_eq!(runs(&s), ["blockMesh -region air", "blockMesh -region buildings", "blockMesh -region street"]);
    // After a test, `&&` makes the command conditional; after any other command it doesn't.
    let cps: Vec<bool> =
        s.steps.iter().filter(|s| matches!(s.kind, StepKind::Copy { .. })).map(|s| s.conditional).collect();
    assert_eq!(cps, [true, false]);
    // The array is an assignment, not a command `a b`.
    assert!(!s.steps.iter().any(|s| matches!(&s.kind, StepKind::Run(r) if r.app == "a")));
}

#[test]
fn conditions_carry_through_chains_and_groups() {
    let s = parse(
        "Allrun",
        "[ -f x ] && cp a b && ./foo\n[ -d y ] && ( cd y; topoSet )\nfalse || { cp c d; cp e f; }\nif (cd z && test -f w); then blockMesh; fi\nsetFields\n",
    );
    let flags: Vec<(String, bool)> = s.steps.iter().map(|st| (workflow_name(&st.kind), st.conditional)).collect();
    assert_eq!(
        flags,
        [
            ("cp".to_string(), true),
            ("./foo".into(), true),
            ("topoSet".into(), true),
            ("cp".into(), true),
            ("cp".into(), true),
            ("blockMesh".into(), true),
            ("setFields".into(), false),
        ]
    );
    // topoSet runs after `cd y` inside its subshell; the `cd`s (also the condition's) stay
    // inside: what follows is resolved, and the subshells are not programs.
    let unresolved: Vec<String> = s.steps.iter().filter(|st| st.unresolved).map(|st| workflow_name(&st.kind)).collect();
    assert_eq!(unresolved, ["topoSet"]);
    assert!(!s.steps.iter().any(|st| matches!(&st.kind, StepKind::Run(r) if r.app == "(" || r.app == ")")));
}

fn workflow_name(k: &StepKind) -> String {
    match k {
        StepKind::Run(r) => r.app.clone(),
        StepKind::Copy { .. } => "cp".into(),
        StepKind::Call { script } => format!("./{script}"),
        k => format!("{k:?}"),
    }
}

#[test]
fn hostile_scripts_are_read_within_limits() {
    // 30 nested loops over 10 words would be 10^30 passes.
    let mut text = String::new();
    for k in 0..30 {
        text.push_str(&format!("for v{k} in a b c d e f g h i j; do\n"));
    }
    text.push_str("blockMesh -region $v0\n");
    text.push_str(&"done\n".repeat(30));
    let s = parse("Allrun", &text);
    assert!(s.truncated && s.steps.len() <= MAX_STEPS);
    // 100 000 nested ifs: no deep recursion, no copies per level.
    let deep = format!("{}blockMesh\n{}", "if true; then\n".repeat(100_000), "fi\n".repeat(100_000));
    let s = parse("Allrun", &deep);
    assert!(s.truncated && s.steps.is_empty());
    // A value doubled in a loop stops being resolved at MAX_VALUE, without the memory.
    let s = parse("Allrun", &format!("x=a\nfor i in {}; do x=$x$x; done\ncp $x b\n", "1 ".repeat(40)));
    assert!(s.steps.iter().all(|st| st.unresolved), "{}", s.steps.len());
    // Empty loop bodies count: 1000 × 1000 × 10 000 passes stop at MAX_WORK.
    let words = |n: usize| "w ".repeat(n);
    let s = parse(
        "Allrun",
        &format!(
            "for a in {}; do for b in {}; do for c in {}; do\ndone; done; done\n",
            words(1000),
            words(1000),
            words(10_000)
        ),
    );
    assert!(s.truncated);
    // A skipped body (`case`) scanned on every pass counts as work.
    let s = parse(
        "Allrun",
        &format!("for a in {}; do\ncase x in\n{}esac\ndone\n", words(1000), "y) echo ;;\n".repeat(10_000)),
    );
    assert!(s.truncated);
    // Subshells nested past the limit stop the reading.
    let s = parse("Allrun", &format!("{}cd x\n{}blockMesh\n", "( ".repeat(40), ") ".repeat(40)));
    assert!(s.truncated);
    // Also inside an `if` condition: nothing after the limit is read.
    let s = parse("Allrun", &format!("if true\n{}blockMesh\nthen\n  echo\nfi\n", "(\n".repeat(40)));
    assert!(s.truncated && s.steps.is_empty(), "{:?}", s.steps);
    // Many large variables stay within MAX_VARS, and copying them for each subshell is work.
    let mut big = format!("x={}\n", "a".repeat(60_000));
    for k in 0..200 {
        big.push_str(&format!("v{k}=$x\n"));
    }
    big.push_str(&format!("for i in {}; do ( ) ; done\n", words(100_000)));
    let s = parse("Allrun", &big);
    assert!(s.truncated);
    // Long names count too: the second megabyte-long name isn't kept.
    let n = "n".repeat(MAX_VARS - 10);
    let s = parse("Allrun", &format!("{n}=a\nm{n}=b\ncp $m{n} c\n"));
    assert!(s.steps.iter().all(|st| st.unresolved));
    // Within the limits nothing is cut.
    let s = parse("Allrun", "for i in a b; do for j in c d; do blockMesh -region $i$j; done; done\n");
    assert!(!s.truncated && s.steps.len() == 4);
}
