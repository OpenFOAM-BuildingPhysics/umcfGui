//! Reading the scripts of real cases.
//!
//! `UMCF_CASES` (`:`-separated case directories): every script of every case is read and its
//! steps are printed (`--nocapture`), to compare with what the script says.

use umcf_model::script::{self, StepKind};

#[test]
fn reference_case_scripts_read() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_case_scripts_read: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| script::is_script_name(n))
            .collect();
        names.sort();
        for n in names {
            let Ok(text) = std::fs::read_to_string(format!("{dir}/{n}")) else { continue };
            let s = script::parse(&n, &text);
            eprintln!("== {dir}/{n} (ntasks {:?}, shebang {})", s.ntasks, s.shebang);
            for st in &s.steps {
                let flags =
                    format!("{}{}", if st.conditional { "?" } else { "" }, if st.unresolved { "~" } else { "" });
                let what = match &st.kind {
                    StepKind::Run(r) => format!(
                        "run {}{}{}{}{}",
                        r.app,
                        r.region.as_deref().map(|x| format!(" -region {x}")).unwrap_or_default(),
                        if r.all_regions { " -allRegions" } else { "" },
                        if r.parallel { " -parallel" } else { "" },
                        r.np.as_ref().map(|n| format!(" np={n:?}")).unwrap_or_default()
                    ),
                    k => format!("{k:?}"),
                };
                eprintln!("  {:>3}{flags:2} {what}", st.line);
            }
        }
    }
}

/// Every case's scripts imported into the workflow pattern: what fits and what doesn't
/// (`--nocapture`). `UMCF_WORKFLOW_PRINT=<case>` also prints the scripts generated for it.
#[test]
fn reference_case_workflows() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_case_workflows: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let print = std::env::var("UMCF_WORKFLOW_PRINT").unwrap_or_default();
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        // Fields are not needed here: keep their limit at 0 so big cases open fast.
        let options = umcf_model::OpenOptions { field_bytes: 0, ..Default::default() };
        let Ok(case) = umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), options) else { continue };
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let scripts: Vec<script::Script> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| script::is_script_name(n))
            .filter_map(|n| Some(script::parse(&n, &std::fs::read_to_string(format!("{dir}/{n}")).ok()?)))
            .collect();
        let regions: Vec<String> = case.regions.iter().map(|r| r.name.clone()).filter(|n| !n.is_empty()).collect();
        let wf = umcf_model::workflow::Workflow::import(&regions, case.controls.application.as_deref(), &scripts);
        if wf.meshes.is_empty() && wf.solver.is_none() {
            continue;
        }
        eprintln!("== {dir} ({:?})", case.layout.version);
        for m in &wf.meshes {
            eprintln!("  mesh {}: {:?} {:?}", m.region, m.build, m.steps);
        }
        eprintln!(
            "  features {} radiation {:?} solver {:?} reconstruct {}",
            wf.features, wf.radiation, wf.solver, wf.reconstruct
        );
        for u in &wf.unmatched {
            eprintln!("  {} {}:{} {}", if u.redundant { "redundant" } else { "UNMATCHED" }, u.script, u.line, u.what);
        }
        if print == dir
            && let Some(v) = case.layout.version
        {
            for (n, t) in wf.generate(v) {
                eprintln!("---- {n}\n{t}");
            }
        }
    }
}
