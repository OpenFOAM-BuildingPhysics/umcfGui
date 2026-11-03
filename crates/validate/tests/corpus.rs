//! Checking real cases.
//!
//! `UMCF_CASES` (`:`-separated case directories): every case is checked and its diagnostics are
//! printed. These cases ran, so an error is a false alarm unless `UMCF_CHECK_EXPECT` lists it:
//! `<case>=<code>,<code>;<case>=<code>` (a real problem in that case, found by the check).

use umcf_core::DirFs;
use umcf_model::Case;
use umcf_validate::{Level, check};

#[test]
fn reference_cases_have_no_false_errors() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_cases_have_no_false_errors: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let expect = std::env::var("UMCF_CHECK_EXPECT").unwrap_or_default();
    let expected = |case: &str, code: &str| {
        expect
            .split(';')
            .filter_map(|e| e.split_once('='))
            .any(|(c, codes)| c == case && codes.split(',').any(|x| x == code))
    };
    let mut failures = Vec::new();
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        let case = match Case::open(&DirFs::new(dir)) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{dir}: {e}"));
                continue;
            }
        };
        let diags = check(&case);
        let count = |l| diags.iter().filter(|d| d.level == l).count();
        eprintln!(
            "{dir}: {} errors, {} warnings, {} info",
            count(Level::Error),
            count(Level::Warning),
            count(Level::Info)
        );
        for d in &diags {
            eprintln!(
                "    {:?} {} {} [{}{}{}]: {}",
                d.level,
                d.code,
                d.path,
                d.region.as_deref().unwrap_or(""),
                d.patch.as_deref().map(|p| format!(" patch {p}")).unwrap_or_default(),
                d.field.as_deref().map(|f| format!(" field {f}")).unwrap_or_default(),
                d.message
            );
            if d.level == Level::Error && !expected(dir, d.code) {
                failures.push(format!("{dir}: {} {}: {}", d.code, d.path, d.message));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
