//! Round trip over real cases: every dictionary under `system/`, `constant/` and `0/` must
//! parse and write back byte-identical.
//!
//! `UMCF_CASES` lists case directories, `:`-separated (OF8 and OF12 cases both). Without it the
//! test reports a skip and passes; real files never get committed to this repo.

use std::path::{Path, PathBuf};

use umcf_foam_dict::{Document, Error};

/// Directories with mesh or surface data, not dictionaries.
const SKIP_DIRS: &[&str] = &["polyMesh", "triSurface", "extendedFeatureEdgeMesh", "dynamicCode"];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                files(&p, out);
            }
        } else if !name.ends_with(".gz") && !name.ends_with(".orig.gz") {
            out.push(p);
        }
    }
}

#[test]
fn reference_cases_round_trip() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_cases_round_trip: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let (mut ok, mut binary, mut too_large) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for case in cases.split(':').filter(|c| !c.is_empty()) {
        let case = Path::new(case);
        assert!(case.join("system/controlDict").is_file(), "{} is not an OpenFOAM case", case.display());
        let mut list = Vec::new();
        for sub in ["system", "constant", "0"] {
            files(&case.join(sub), &mut list);
        }
        for f in list {
            let Ok(bytes) = std::fs::read(&f) else { continue };
            match Document::parse_bytes(&bytes) {
                Ok(doc) => {
                    if doc.to_string().as_bytes() == bytes.as_slice() {
                        ok += 1;
                    } else {
                        failures.push(format!("{}: round trip differs", f.display()));
                    }
                }
                Err(Error::NotText) => binary += 1,
                Err(Error::TooLarge { .. }) => too_large += 1,
                Err(e) => failures.push(format!("{}: {e}", f.display())),
            }
        }
    }
    eprintln!("corpus: {ok} files round-tripped, {binary} binary, {too_large} too large (field reader's job)");
    assert!(failures.is_empty(), "{} file(s) failed:\n{}", failures.len(), failures.join("\n"));
    assert!(ok > 0, "no dictionaries found in UMCF_CASES");
}

fn walk_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) {
                walk_all(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

/// Any tree of OpenFOAM files (e.g. `$FOAM_TUTORIALS` of OF8 and OF12): every file with a
/// `FoamFile` header must round-trip. `UMCF_DICT_TREES` lists the roots, `:`-separated.
#[test]
fn dictionary_trees_round_trip() {
    let Ok(trees) = std::env::var("UMCF_DICT_TREES") else {
        eprintln!("SKIP dictionary_trees_round_trip: set UMCF_DICT_TREES=<dir>:<dir>… to run it");
        return;
    };
    let (mut ok, mut skipped) = (0usize, 0usize);
    let mut failures = Vec::new();
    for tree in trees.split(':').filter(|t| !t.is_empty()) {
        let mut list = Vec::new();
        walk_all(Path::new(tree), &mut list);
        for f in list {
            let Ok(bytes) = std::fs::read(&f) else { continue };
            // m4 templates (blockMeshDict.m4) are macro sources, not dictionaries.
            let m4 = f.extension().is_some_and(|e| e == "m4");
            if m4 || !bytes.windows(8).take(4096).any(|w| w == b"FoamFile") {
                skipped += 1;
                continue;
            }
            match Document::parse_bytes(&bytes) {
                Ok(doc) if doc.to_string().as_bytes() == bytes.as_slice() => ok += 1,
                Ok(_) => failures.push(format!("{}: round trip differs", f.display())),
                Err(Error::NotText | Error::TooLarge { .. }) => skipped += 1,
                Err(e) => failures.push(format!("{}: {e}", f.display())),
            }
        }
    }
    eprintln!("trees: {ok} dictionaries round-tripped, {skipped} other files skipped");
    assert!(failures.is_empty(), "{} file(s) failed:\n{}", failures.len(), failures.join("\n"));
}
