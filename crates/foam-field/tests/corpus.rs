//! Field reading over real cases. `UMCF_CASES` lists case directories (`:`-separated). For the
//! first and last time directory of each case (every time with `UMCF_FIELDS_ALL_TIMES=1`), every
//! field of every region is read, and checked against the mesh: internal lists of volume fields
//! have `nCells` elements (from the `owner` header), patch value lists have the patch's `nFaces`
//! (from `boundary`).

// Native-only test: timing with the clock is fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::time::Instant;

use umcf_core::{CaseFs, DirFs};
use umcf_foam_dict::{Document, Node};
use umcf_foam_field::{Field, Limits, Value, read_header, times};

/// `nCells` and patch sizes of `constant/<region>/polyMesh` (or `constant/polyMesh` for "").
fn mesh_sizes(fs: &DirFs, region: &str) -> Option<(u64, BTreeMap<String, u64>)> {
    let base = if region.is_empty() { "constant/polyMesh".to_string() } else { format!("constant/{region}/polyMesh") };
    let read = |name: &str| -> Option<Vec<u8>> {
        for p in [format!("{base}/{name}"), format!("{base}/{name}.gz")] {
            if let Ok(Some(b)) = fs.read(&p, 8 << 30) {
                return if b.starts_with(&[0x1f, 0x8b]) { umcf_foam_field::gunzip(&b, 8 << 30).ok() } else { Some(b) };
            }
        }
        None
    };
    let cells = read_header(&read("owner")?).ok()?.count("nCells")?;
    let boundary = Document::parse_bytes(&read("boundary")?).ok()?;
    let mut patches = BTreeMap::new();
    for v in boundary.root.values() {
        let Node::List { items, .. } = v else { continue };
        let mut it = items.iter();
        while let (Some(Node::Atom(name)), Some(Node::Dict { body, .. })) = (it.next(), it.next()) {
            if let Some(n) = body.get("nFaces").and_then(|e| e.as_f64()) {
                patches.insert(name.text.clone(), n as u64);
            }
        }
    }
    Some((cells, patches))
}

#[test]
fn reference_case_fields() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_case_fields: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let all_times = std::env::var("UMCF_FIELDS_ALL_TIMES").is_ok_and(|v| v == "1");
    let limits = Limits::default();
    let mut failures = Vec::new();
    for case in cases.split(':').filter(|c| !c.is_empty()) {
        let fs = DirFs::new(case);
        let ts = times::time_dirs(&fs, "");
        let picked: Vec<_> = if all_times || ts.len() <= 2 {
            ts.iter().collect()
        } else {
            ts.first().into_iter().chain(ts.last()).collect()
        };
        let (mut files, mut values, mut checked) = (0usize, 0usize, 0usize);
        let started = Instant::now();
        for t in picked {
            let mut dirs: Vec<String> = fs
                .list(&t.name)
                .into_iter()
                .filter(|r| fs.is_dir(&format!("{}/{r}", t.name)) && r != "uniform")
                .collect();
            dirs.push(String::new());
            for region in dirs {
                let dir = if region.is_empty() { t.name.clone() } else { format!("{}/{region}", t.name) };
                let sizes = mesh_sizes(&fs, &region);
                for name in times::fields(&fs, &dir) {
                    let rel = format!("{dir}/{name}");
                    let f = match Field::read(&fs, &rel, &limits) {
                        Ok(Some(f)) => f,
                        Ok(None) => continue,
                        Err(e) => {
                            failures.push(format!("{case}/{rel}: {e}"));
                            continue;
                        }
                    };
                    files += 1;
                    values += f.lists().iter().map(|l| l.len()).sum::<usize>();
                    let Some((cells, patches)) = &sizes else { continue };
                    let class = f.class().unwrap_or("");
                    if class.starts_with("vol")
                        && let Some(Value::List(l)) = f.internal()
                    {
                        checked += 1;
                        if l.len() as u64 != *cells {
                            failures.push(format!(
                                "{case}/{rel}: internalField has {} values, mesh has {cells} cells",
                                l.len()
                            ));
                        }
                    }
                    if class.starts_with("vol") {
                        for p in f.patches() {
                            if let (Some(Value::List(l)), Some(n)) = (f.patch_value(p, "value"), patches.get(p)) {
                                checked += 1;
                                if l.len() as u64 != *n {
                                    failures.push(format!(
                                        "{case}/{rel}: patch {p} value has {} values, patch has {n} faces",
                                        l.len()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        eprintln!(
            "{case}: {files} fields, {values} list elements, {checked} lengths checked against the mesh, {:.1}s",
            started.elapsed().as_secs_f64()
        );
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
}

/// `UMCF_FIELD_PAIR=<ascii case>:<binary case>`: the binary case is a copy of the ASCII one
/// converted by OpenFOAM (`writeFormat binary;` + `foamFormatConvert`). OpenFOAM reads the ASCII
/// numbers and writes their exact bits, so every list must agree bit for bit.
#[test]
fn ascii_and_binary_copies_agree() {
    let Ok(pair) = std::env::var("UMCF_FIELD_PAIR") else {
        eprintln!("SKIP ascii_and_binary_copies_agree: set UMCF_FIELD_PAIR=<ascii case>:<binary case>");
        return;
    };
    let Some((a, b)) = pair.split_once(':') else { panic!("UMCF_FIELD_PAIR must be <ascii case>:<binary case>") };
    let (fa, fb) = (DirFs::new(a), DirFs::new(b));
    let limits = Limits::default();
    let (mut compared, mut values, mut left_ascii) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for t in times::time_dirs(&fb, "") {
        for region in fb.list(&t.name).into_iter().filter(|r| r != "uniform") {
            let dir = format!("{}/{region}", t.name);
            for name in times::fields(&fb, &dir) {
                let rel = format!("{dir}/{name}");
                let (Ok(Some(x)), Ok(Some(y))) = (Field::read(&fa, &rel, &limits), Field::read(&fb, &rel, &limits))
                else {
                    failures.push(format!("{rel}: could not read both copies"));
                    continue;
                };
                // OpenFOAM converts the fields of a region; data tables, backups and files of
                // directories that are not regions stay ASCII.
                if y.header.format != umcf_foam_field::Format::Binary {
                    left_ascii += 1;
                    continue;
                }
                if x.lists().len() != y.lists().len() {
                    failures.push(format!("{rel}: {} lists vs {}", x.lists().len(), y.lists().len()));
                    continue;
                }
                for (k, (lx, ly)) in x.lists().iter().zip(y.lists()).enumerate() {
                    let same = lx.len() == ly.len()
                        && lx.kind == ly.kind
                        && lx
                            .iter()
                            .zip(ly.iter())
                            .all(|(p, q)| p.iter().zip(q).all(|(u, v)| u.to_bits() == v.to_bits()));
                    if !same {
                        failures.push(format!("{rel}: list {k} differs"));
                    }
                    values += lx.len();
                }
                compared += 1;
            }
        }
    }
    eprintln!("pair: {compared} fields, {values} elements compared; {left_ascii} files OpenFOAM left in ASCII");
    assert!(compared > 0, "nothing compared");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
