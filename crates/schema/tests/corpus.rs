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
            "{name}: {v} (OF8 {}, OF12 {}{}), regions from {}: {:?}",
            l.score(OfVersion::V8),
            l.score(OfVersion::V12),
            if l.mixed() { ", mixed" } else { "" },
            l.regions_from.as_deref().unwrap_or("-"),
            l.regions.all().collect::<Vec<_>>()
        );
        for n in &l.notes {
            eprintln!("    {:?} {}: {}", n.severity, n.path, n.message);
        }
        if l.version.is_none() || l.regions.is_empty() {
            failures.push(format!("{case}: no version or no regions"));
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
