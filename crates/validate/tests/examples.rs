//! The example cases under `examples/of8` and `examples/of12`, on every run (CI included): each
//! is a solver case of its directory's OpenFOAM version, the check finds no error in it (nor a
//! warning about its scripts), and every dictionary reads and writes back byte for byte. That
//! they run is `cargo xtask examples`.

use std::path::{Path, PathBuf};

use umcf_core::{DirFs, OfVersion};
use umcf_foam_dict::{Document, Error};
use umcf_model::Case;
use umcf_validate::{Level, check};

/// Every example case, with the version its directory names.
fn examples() -> Vec<(OfVersion, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut out = Vec::new();
    for (dir, version) in [("of8", OfVersion::V8), ("of12", OfVersion::V12)] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        let mut cases: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.join("system/controlDict").is_file()).collect();
        cases.sort();
        out.extend(cases.into_iter().map(|c| (version, c)));
    }
    out
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn examples_are_cases_of_their_version_without_errors() {
    let list = examples();
    let count = |v| list.iter().filter(|(x, _)| *x == v).count();
    assert!(count(OfVersion::V8) >= 4 && count(OfVersion::V12) >= 4, "examples missing: {list:?}");
    let mut failures = Vec::new();
    for (version, dir) in &list {
        let case = match Case::open(&DirFs::new(dir)) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        if case.layout.version != Some(*version) || case.layout.mixed() {
            failures.push(format!(
                "{}: detected {:?}{}, expected {version:?}",
                dir.display(),
                case.layout.version,
                if case.layout.mixed() { " (mixed)" } else { "" }
            ));
        }
        // No error, and no warning about the scripts: each step finds what it reads.
        let bad = |d: &umcf_validate::Diagnostic| {
            d.level == Level::Error || d.code.starts_with("workflow.") && d.level == Level::Warning
        };
        for d in check(&case).iter().filter(|d| bad(d)) {
            failures.push(format!("{}: {} {}: {}", dir.display(), d.code, d.path, d.message));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn example_dictionaries_round_trip() {
    let mut failures = Vec::new();
    let mut ok = 0usize;
    for (_, dir) in examples() {
        let mut list = Vec::new();
        for sub in ["system", "constant", "0"] {
            files(&dir.join(sub), &mut list);
        }
        for f in list {
            let Ok(bytes) = std::fs::read(&f) else { continue };
            match Document::parse_bytes(&bytes) {
                Ok(doc) if doc.to_string().as_bytes() == bytes.as_slice() => ok += 1,
                Ok(_) => failures.push(format!("{}: round trip differs", f.display())),
                Err(Error::NotText) => {}
                Err(e) => failures.push(format!("{}: {e}", f.display())),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(ok > 100, "only {ok} dictionaries in examples/");
}
