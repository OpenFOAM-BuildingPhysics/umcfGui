//! Field reading over real cases. `UMCF_CASES` lists case directories (`:`-separated). For the
//! first and last time directory of each case (every time with `UMCF_FIELDS_ALL_TIMES=1`), every
//! field of every region is read, and checked against the mesh: internal lists of volume fields
//! have `nCells` elements (from the `owner` header), patch value lists have the patch's `nFaces`
//! (from `boundary`). Every configured case must yield fields; a mesh that exists but can't be
//! read is a failure, not a skipped check.

// Native-only test: timing with the clock is fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::time::Instant;

use umcf_core::{CaseFs, DirFs};
use umcf_foam_dict::{Document, Node};
use umcf_foam_field::{Field, Format, Limits, Value, read_header, times};

const LIMIT: u64 = 8 << 30;

/// The bytes of `<base>/<name>` or `<base>/<name>.gz`, decompressed. `Ok(None)` if neither
/// exists.
fn read_mesh_file(fs: &DirFs, base: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
    for p in [format!("{base}/{name}"), format!("{base}/{name}.gz")] {
        let bytes = fs.read(&p, LIMIT).map_err(|e| e.to_string())?;
        if let Some(b) = bytes {
            return if b.starts_with(&[0x1f, 0x8b]) {
                umcf_foam_field::gunzip(&b, LIMIT).map(Some).map_err(|e| format!("{p}: {e}"))
            } else {
                Ok(Some(b))
            };
        }
    }
    Ok(None)
}

type MeshSizes = (u64, BTreeMap<String, u64>);

/// `nCells` and patch sizes of `constant/<region>/polyMesh` (or `constant/polyMesh` for "").
/// `Ok(None)` when the region has no mesh; an error when the mesh is there but unreadable.
fn mesh_sizes(fs: &DirFs, region: &str) -> Result<Option<MeshSizes>, String> {
    let base = if region.is_empty() { "constant/polyMesh".to_string() } else { format!("constant/{region}/polyMesh") };
    let (Some(owner), Some(boundary)) = (read_mesh_file(fs, &base, "owner")?, read_mesh_file(fs, &base, "boundary")?)
    else {
        return Ok(None);
    };
    let header = read_header(&owner).map_err(|e| format!("{base}/owner: {e}"))?;
    let cells = header.count("nCells").ok_or_else(|| format!("{base}/owner: no nCells in the header note"))?;
    let boundary = Document::parse_bytes(&boundary).map_err(|e| format!("{base}/boundary: {e}"))?;
    let mut patches = BTreeMap::new();
    // `boundary` is `N ( name { … } … )`: the count parses as a keyword with the list as its
    // value; a file without the count has the list alone.
    let lists = boundary.root.values().chain(boundary.root.entries().filter_map(|e| match e.value.as_slice() {
        [list @ Node::List { .. }] => Some(list),
        _ => None,
    }));
    for v in lists {
        let Node::List { items, .. } = v else { continue };
        let mut it = items.iter();
        while let (Some(Node::Atom(name)), Some(Node::Dict { body, .. })) = (it.next(), it.next()) {
            let n = body.get("nFaces").and_then(|e| e.as_f64());
            let n = n.ok_or_else(|| format!("{base}/boundary: patch {} has no nFaces", name.text))?;
            patches.insert(name.text.clone(), n as u64);
        }
    }
    if patches.is_empty() {
        return Err(format!("{base}/boundary: no patches"));
    }
    Ok(Some((cells, patches)))
}

/// Region directories of a time directory, plus `""` for fields directly under it.
fn regions(fs: &DirFs, time: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = fs
        .list(time)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| r != "uniform" && fs.is_dir(&format!("{time}/{r}")))
        .collect();
    out.push(String::new());
    Ok(out)
}

fn join(time: &str, region: &str) -> String {
    if region.is_empty() { time.to_string() } else { format!("{time}/{region}") }
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
        let ts = match times::time_dirs(&fs, "") {
            Ok(ts) => ts,
            Err(e) => {
                failures.push(format!("{case}: {e}"));
                continue;
            }
        };
        let picked: Vec<_> = if all_times || ts.len() <= 2 {
            ts.iter().collect()
        } else {
            ts.first().into_iter().chain(ts.last()).collect()
        };
        let (mut files, mut values, mut checked) = (0usize, 0usize, 0usize);
        let started = Instant::now();
        // The mesh lives in constant/: read it once per region, report a broken one once.
        let mut meshes: BTreeMap<String, Option<MeshSizes>> = BTreeMap::new();
        for t in picked {
            let regions = match regions(&fs, &t.name) {
                Ok(r) => r,
                Err(e) => {
                    failures.push(format!("{case}/{}: {e}", t.name));
                    continue;
                }
            };
            for region in regions {
                let dir = join(&t.name, &region);
                let sizes = meshes
                    .entry(region.clone())
                    .or_insert_with(|| {
                        mesh_sizes(&fs, &region).unwrap_or_else(|e| {
                            failures.push(format!("{case}: {e}"));
                            None
                        })
                    })
                    .clone();
                let names = match times::fields(&fs, &dir) {
                    Ok(n) => n,
                    Err(e) => {
                        failures.push(format!("{case}/{dir}: {e}"));
                        continue;
                    }
                };
                for name in names {
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
                    if !f.class().unwrap_or("").starts_with("vol") {
                        continue;
                    }
                    if let Some(Value::List(l)) = f.internal() {
                        checked += 1;
                        if l.len() as u64 != *cells {
                            failures.push(format!(
                                "{case}/{rel}: internalField has {} values, mesh has {cells} cells",
                                l.len()
                            ));
                        }
                    }
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
        if files == 0 {
            failures.push(format!("{case}: no fields read (no time directories, or no case there)"));
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
/// numbers and writes their exact bits, so every list must agree bit for bit. Fields directly
/// under a time directory (single-region cases) count too.
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
    for t in times::time_dirs(&fb, "").unwrap() {
        for region in regions(&fb, &t.name).unwrap() {
            let dir = join(&t.name, &region);
            for name in times::fields(&fb, &dir).unwrap() {
                let rel = format!("{dir}/{name}");
                let (Ok(Some(x)), Ok(Some(y))) = (Field::read(&fa, &rel, &limits), Field::read(&fb, &rel, &limits))
                else {
                    failures.push(format!("{rel}: could not read both copies"));
                    continue;
                };
                // OpenFOAM converts the fields of a region; data tables, backups and files of
                // directories that are not regions stay ASCII.
                if y.header.format != Format::Binary {
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
