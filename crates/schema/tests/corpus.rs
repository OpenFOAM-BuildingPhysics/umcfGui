//! Layout detection over real cases. `UMCF_CASES` lists case directories (`:`-separated);
//! `UMCF_CASES_EXPECT` optionally pins versions: `<case path>=8,<case path>=12,…`.

use umcf_core::{DirFs, OfVersion};

#[test]
fn reference_cases_have_a_layout() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_cases_have_a_layout: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let expect = std::env::var("UMCF_CASES_EXPECT").unwrap_or_default();
    let mut failures = Vec::new();
    for case in cases.split(':').filter(|c| !c.is_empty()) {
        let name =
            std::path::Path::new(case.trim_end_matches('/')).file_name().map(|n| n.to_string_lossy().to_string());
        let name = name.unwrap_or_default();
        let l = match umcf_schema::detect(&DirFs::new(case)) {
            Ok(l) => l,
            Err(e) => {
                failures.push(format!("{case}: {e}"));
                continue;
            }
        };
        let v = l.version.map_or("?".to_string(), |v| v.to_string());
        eprintln!(
            "{name}: {:?} {v} (OF8 {}, OF12 {}{}), tools {:?}, regions from {}: {:?}",
            l.kind,
            l.score(OfVersion::V8),
            l.score(OfVersion::V12),
            if l.mixed() { ", mixed" } else { "" },
            l.tools,
            l.regions_from.as_deref().unwrap_or("-"),
            l.regions.all().collect::<Vec<_>>()
        );
        for n in &l.notes {
            eprintln!("    {:?} {}: {}", n.severity, n.path, n.message);
        }
        if l.kind == umcf_schema::CaseKind::Solver && (l.version.is_none() || l.regions.is_empty()) {
            failures.push(format!("{case}: solver case without a version or regions"));
        }
        for pin in expect.split(',').filter_map(|p| p.split_once('=')) {
            let want = match pin.1 {
                "8" => Some(OfVersion::V8),
                "12" => Some(OfVersion::V12),
                _ => None,
            };
            if pin.0.trim_end_matches('/') == case.trim_end_matches('/') && l.version != want {
                failures.push(format!("{case}: detected {v}, expected OF{}", pin.1));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The snappyHexMesh signal against every snappyHexMesh tutorial of both versions:
/// `UMCF_OF8_TUTORIALS` and `UMCF_OF12_TUTORIALS` point at the tutorial trees.
#[test]
fn tutorial_meshing_cases_get_their_version() {
    let (Ok(t8), Ok(t12)) = (std::env::var("UMCF_OF8_TUTORIALS"), std::env::var("UMCF_OF12_TUTORIALS")) else {
        eprintln!("SKIP tutorial_meshing_cases_get_their_version: set UMCF_OF8_TUTORIALS and UMCF_OF12_TUTORIALS");
        return;
    };
    fn cases(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if dir.join("system/snappyHexMeshDict").is_file() && dir.join("system/controlDict").is_file() {
            out.push(dir.to_path_buf());
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                cases(&e.path(), out);
            }
        }
    }
    let mut wrong = Vec::new();
    let mut checked = 0;
    for (tree, want) in [(t8, OfVersion::V8), (t12, OfVersion::V12)] {
        let mut list = Vec::new();
        cases(std::path::Path::new(&tree), &mut list);
        for c in list {
            checked += 1;
            match umcf_schema::detect(&DirFs::new(&c)) {
                Ok(l) if l.version == Some(want) => {}
                Ok(l) => wrong.push(format!("{}: {:?}, expected {want}", c.display(), l.version)),
                Err(e) => wrong.push(format!("{}: {e}", c.display())),
            }
        }
    }
    eprintln!("tutorials: {checked} snappyHexMesh cases checked");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
