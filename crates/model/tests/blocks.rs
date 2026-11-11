//! The `blockMeshDict`s of real cases read as grids of blocks and written again.
//!
//! - The examples (always): every `blockMeshDict` of `examples/of8` and `examples/of12`.
//! - `UMCF_CASES` (`:`-separated case directories): each case's `system/blockMeshDict*` and
//!   `system/<region>/blockMeshDict`, read only. What isn't a grid is printed with why
//!   (`--nocapture`).
//!
//! Each one that is a grid is written again from it, and the two compared by coordinates: the
//! same vertices, the same blocks (their corners, cells and gradings), the same faces in each
//! patch.
//!
//! `UMCF_SOLVER=1` (OpenFOAM sourced, `blockMesh` on the path) also runs blockMesh on both, in
//! scratch cases under the temporary directory, for the examples of the sourced version and the
//! first `UMCF_CASES` case of it, and compares the meshes: points, faces, cells, and each patch
//! with its type and size.

// Native-only test: it runs blockMesh.
#![allow(clippy::disallowed_methods)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use umcf_foam_dict::{Document, Node};
use umcf_model::blocks::{self, Imported};

type Point = [u64; 3];

/// A dictionary by coordinates: its vertices, its blocks (sorted corners, cells, gradings as
/// written) and its patches (each face's sorted corners).
#[derive(Debug, PartialEq)]
struct Expanded {
    points: BTreeSet<Point>,
    blocks: BTreeSet<(Vec<Point>, Vec<u64>, String)>,
    patches: BTreeMap<String, BTreeSet<Vec<Point>>>,
}

fn atom(n: &Node) -> &str {
    match n {
        Node::Atom(t) => &t.text,
        _ => panic!("not an atom: {}", n.compact()),
    }
}

fn items(n: &Node) -> &[Node] {
    match n {
        Node::List { items, .. } => items,
        _ => panic!("not a list: {}", n.compact()),
    }
}

fn entry_list<'a>(doc: &'a Document, key: &str) -> &'a [Node] {
    match doc.get(&[key]).map(|e| e.value.as_slice()) {
        Some([n]) => items(n),
        _ => &[],
    }
}

/// Numbers as their values, lists as they nest.
fn numbers(n: &Node) -> String {
    match n {
        Node::Atom(t) => format!("{}", t.text.parse::<f64>().unwrap() + 0.0),
        Node::List { items, .. } => format!("({})", items.iter().map(numbers).collect::<Vec<_>>().join(" ")),
        _ => panic!(),
    }
}

fn expand(doc: &Document) -> Expanded {
    let pts: Vec<Point> = entry_list(doc, "vertices")
        .iter()
        .map(|v| {
            let l = items(v);
            [0, 1, 2].map(|i| (atom(&l[i]).parse::<f64>().unwrap() + 0.0).to_bits())
        })
        .collect();
    let corners = |labels: &[Node]| -> Vec<Point> {
        let mut c: Vec<Point> = labels.iter().map(|x| pts[atom(x).parse::<usize>().unwrap()]).collect();
        c.sort_unstable();
        c
    };
    let mut blocks = BTreeSet::new();
    for b in entry_list(doc, "blocks").chunks(5) {
        let cells = items(&b[2]).iter().map(|x| atom(x).parse().unwrap()).collect();
        // In the hex's order, not sorted: a block whose numbering or grading were turned
        // around would differ.
        let ordered: Vec<Point> = items(&b[1]).iter().map(|x| pts[atom(x).parse::<usize>().unwrap()]).collect();
        blocks.insert((ordered, cells, format!("{} {}", atom(&b[3]), numbers(&b[4]))));
    }
    let mut patches = BTreeMap::new();
    for p in entry_list(doc, "boundary").chunks(2) {
        let Node::Dict { body, .. } = &p[1] else { panic!() };
        let faces = match body.get("faces").map(|e| e.value.as_slice()) {
            Some([n]) => items(n).iter().map(|f| corners(items(f))).collect(),
            _ => BTreeSet::new(),
        };
        patches.insert(atom(&p[0]).to_string(), faces);
    }
    Expanded { points: pts.into_iter().collect(), blocks, patches }
}

/// The dictionary written again from its grid, each patch of the type it had.
fn regenerate(imp: &Imported, location: &str) -> String {
    let types: BTreeMap<&str, &str> = imp.types.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
    imp.grid.block_mesh_dict("r", location, &|n| types.get(n).copied().unwrap_or("patch").to_string()).unwrap()
}

/// Reads `path`, imports it and checks the written one against it; `Err(why)` when it isn't a
/// grid.
fn check(path: &Path) -> Result<(Imported, String), String> {
    let text = std::fs::read_to_string(path).unwrap();
    let doc = Document::parse(&text).unwrap();
    let imp = blocks::import(&doc)?;
    let again = regenerate(&imp, "system");
    let doc2 = Document::parse(&again).unwrap();
    assert_eq!(expand(&doc2), expand(&doc), "{}", path.display());
    // Read back as the same grid, and as untouched.
    let imp2 = blocks::import(&doc2).unwrap();
    assert!(imp2.grid.same_mesh(&imp.grid), "{}", path.display());
    // The same types; the patches in the grid's order (the sides', then single faces', then
    // those without faces).
    let sorted = |t: &[(String, String)]| t.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(sorted(&imp2.types), sorted(&imp.types), "{}", path.display());
    assert!(blocks::untouched(&again), "{}", path.display());
    Ok((imp, again))
}

fn summary(imp: &Imported) -> String {
    let g = &imp.grid;
    let c = g.counts();
    format!(
        "{}x{}x{} blocks, {} missing, {} cells; sides {:?}; single faces {:?}; no faces {:?}",
        c[0],
        c[1],
        c[2],
        g.missing.len(),
        g.cells().unwrap_or(0),
        g.sides.iter().map(|(s, n)| format!("{} {n}", s.name())).collect::<Vec<_>>(),
        g.faces.iter().map(|(n, f)| format!("{n} {}", f.len())).collect::<Vec<_>>(),
        g.no_faces
    )
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// The `blockMeshDict`s of a case: `system/blockMeshDict*` and `system/<region>/blockMeshDict`.
fn dicts(case: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(case.join("system")) else { return out };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if p.is_dir() {
            let d = p.join("blockMeshDict");
            if d.is_file() {
                out.push(d);
            }
        } else if name.starts_with("blockMeshDict") && !name.ends_with('~') {
            out.push(p);
        }
    }
    out
}

#[test]
fn the_examples_blockmeshdicts_are_grids_written_back_the_same() {
    let mut n = 0;
    for version in ["of8", "of12"] {
        let Ok(cases) = std::fs::read_dir(examples().join(version)) else { continue };
        for case in cases.filter_map(Result::ok) {
            for d in dicts(&case.path()) {
                match check(&d) {
                    Ok((imp, _)) => {
                        eprintln!("{}: {}", d.display(), summary(&imp));
                        n += 1;
                    }
                    Err(why) => panic!("{}: not a grid: {why}", d.display()),
                }
            }
        }
    }
    assert!(n >= 26, "{n} dictionaries");
}

#[test]
fn reference_case_blockmeshdicts() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_case_blockmeshdicts: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let mut grids = 0;
    for dir in cases.split(':').filter(|c| !c.is_empty()) {
        for d in dicts(Path::new(dir)) {
            match check(&d) {
                Ok((imp, _)) => {
                    grids += 1;
                    eprintln!("{}: {}", d.display(), summary(&imp));
                }
                Err(why) => eprintln!("{}: not a grid: {why}", d.display()),
            }
            // The patches blockMesh makes of it, read whole: a dictionary that leaves faces
            // to blockMesh lists its default patch, grid or not.
            let doc = Document::parse(&std::fs::read_to_string(&d).unwrap()).unwrap();
            match blocks::dict_patches(&doc) {
                Ok(p) => {
                    let names: Vec<String> = p.patches.iter().map(|q| format!("{} {}", q.name, q.kind)).collect();
                    eprintln!("{}: patches {names:?} {:?}", d.display(), p.notes);
                    let has_default = p.patches.iter().any(|q| q.name == blocks::DEFAULT_FACES);
                    if d.ends_with("buildings/blockMeshDict") {
                        assert!(has_default, "{}: {names:?}", d.display());
                    }
                }
                Err(why) => eprintln!("{}: patches can't all be read: {why}", d.display()),
            }
        }
        // The description read from the case: what it says about the blocks.
        let options = umcf_model::OpenOptions { field_bytes: 0, ..Default::default() };
        let Ok(case) = umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), options) else { continue };
        let (desc, notes) = umcf_model::description::Description::import(&case);
        for r in &desc.regions {
            eprintln!("{dir}: region {} has blocks: {}", r.name, r.blocks.is_some());
        }
        for n in notes.iter().filter(|n| n.contains("blockMeshDict") || n.contains("blocks")) {
            eprintln!("{dir}: note: {n}");
        }
    }
    assert!(grids > 0, "no blockMeshDict of UMCF_CASES is a grid");
}

/// Two meshes' points, sorted, the same within a relative 1e-9 of the domain's size.
fn same_points(a: &[[f64; 3]], b: &[[f64; 3]], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}");
    let scale = a.iter().flatten().fold(1e-30_f64, |m, x| m.max(x.abs()));
    for (p, q) in a.iter().zip(b) {
        for (x, y) in p.iter().zip(q) {
            assert!((x - y).abs() <= 1e-9 * scale, "{what}: {p:?} and {q:?}");
        }
    }
}

/// The most cells of a mesh the blockMesh test makes.
const MAX_CELLS: u64 = 1_000_000;

/// The mesh blockMesh makes of `text`: points, faces, cells, and each patch's type and size.
/// The counts and patches of a mesh blockMesh made, and its points sorted.
type Made = ((usize, usize, usize, BTreeMap<String, (String, usize)>), Vec<[f64; 3]>);

fn block_mesh(text: &str, tag: &str) -> Made {
    let dir = std::env::temp_dir().join(format!("umcf-blocks-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("system")).unwrap();
    std::fs::write(
        dir.join("system/controlDict"),
        "FoamFile { version 2.0; format ascii; class dictionary; object controlDict; }\napplication blockMesh;\nstartFrom startTime;\nstartTime 0;\nstopAt endTime;\nendTime 1;\ndeltaT 1;\nwriteControl timeStep;\nwriteInterval 1;\nwriteFormat ascii;\nwritePrecision 6;\nwriteCompression off;\ntimeFormat general;\ntimePrecision 6;\nrunTimeModifiable false;\n",
    )
    .unwrap();
    std::fs::write(dir.join("system/blockMeshDict"), text).unwrap();
    let out = std::process::Command::new("blockMesh").arg("-case").arg(&dir).output().expect("blockMesh runs");
    assert!(
        out.status.success(),
        "blockMesh failed on {tag}:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let fs = umcf_core::DirFs::new(&dir);
    let mesh = umcf_foam_mesh::PolyMesh::read(&fs, "constant/polyMesh", &umcf_foam_mesh::Limits::default()).unwrap();
    let patches = mesh.boundary.iter().map(|p| (p.name.clone(), (p.kind.clone(), p.n_faces))).collect();
    let mut points = mesh.points.clone();
    // Sorted by coordinates rounded well below any cell, so points a rounding error apart sort
    // alike.
    let scale = points.iter().flatten().fold(1e-30_f64, |m, x| m.max(x.abs()));
    let key = |p: &[f64; 3]| p.map(|x| (x / scale * 1e9).round() as i64);
    points.sort_by_key(key);
    let r = ((mesh.n_points(), mesh.n_faces(), mesh.n_cells(), patches), points);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn blockmesh_makes_the_same_mesh_of_the_written_dictionary() {
    if std::env::var("UMCF_SOLVER").as_deref() != Ok("1") {
        eprintln!(
            "SKIP blockmesh_makes_the_same_mesh_of_the_written_dictionary: set UMCF_SOLVER=1 with OpenFOAM sourced"
        );
        return;
    }
    let version = std::env::var("WM_PROJECT_VERSION").expect("OpenFOAM sourced: WM_PROJECT_VERSION");
    let of = if version.starts_with('8') { "of8" } else { "of12" };
    let mut sources: Vec<PathBuf> = dicts(&examples().join(of).join("streetCanyon_CFDHAM"));
    // The first reference case of the sourced version.
    let wanted = if of == "of8" { umcf_core::OfVersion::V8 } else { umcf_core::OfVersion::V12 };
    for dir in std::env::var("UMCF_CASES").unwrap_or_default().split(':').filter(|c| !c.is_empty()) {
        let options = umcf_model::OpenOptions { field_bytes: 0, ..Default::default() };
        let Ok(case) = umcf_model::Case::open_with(&umcf_core::DirFs::new(dir), options) else { continue };
        if case.layout.version == Some(wanted) {
            sources.extend(dicts(Path::new(dir)));
            break;
        }
    }
    let mut compared = 0;
    for (k, d) in sources.iter().enumerate() {
        let Ok((imp, again)) = check(d) else { continue };
        // Quick: the big reference meshes are left out.
        if imp.grid.cells().is_none_or(|c| c > MAX_CELLS) {
            eprintln!("{}: more than {MAX_CELLS} cells, not run", d.display());
            continue;
        }
        let original = std::fs::read_to_string(d).unwrap();
        let (a, pa) = block_mesh(&original, &format!("{k}-original"));
        let (b, pb) = block_mesh(&again, &format!("{k}-written"));
        eprintln!("{version} {}: {} points, {} faces, {} cells, patches {:?}", d.display(), a.0, a.1, a.2, a.3);
        assert_eq!(a, b, "{}", d.display());
        same_points(&pa, &pb, &d.display().to_string());
        compared += 1;
    }
    assert!(compared > 0);
    // Blocks numbered against x and y with sections: read when blockMesh shares their cells
    // alike from both ends (the same points), refused on a tie.
    let one = |grading: &str| {
        format!(
            "FoamFile {{ version 2.0; format ascii; class dictionary; object blockMeshDict; }}\nvertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );\nblocks ( hex (2 3 0 1 6 7 4 5) (5 3 2) simpleGrading ({grading}) );\nedges ( );\nboundary ( );\nmergePatchPairs ( );\ndefaultPatch {{ name defaultFaces; type empty; }}\n"
        )
    };
    let tie = one("((0.5 0.5 2) (0.5 0.5 0.5)) 1 1");
    assert!(blocks::import(&Document::parse(&tie).unwrap()).unwrap_err().contains("shares out otherwise"));
    let accepted = one("((0.3 0.4 2) (0.7 0.6 0.5)) 3 1");
    let imp = blocks::import(&Document::parse(&accepted).unwrap()).unwrap();
    let again = regenerate(&imp, "system");
    let (a, pa) = block_mesh(&accepted, "reversed-original");
    let (b, pb) = block_mesh(&again, "reversed-written");
    assert_eq!(a, b);
    eprintln!("{version} reversed sections: {} points, the same within rounding", a.0);
    same_points(&pa, &pb, "reversed sections");
    // blockMesh's own sharing of cells among sections, against `line_divide_counts`: a block
    // of 5 cells along x, numbered along x (forward) and against it (reversed).
    let along = |hex: &str, sections: &str| -> Vec<f64> {
        let text = format!(
            "FoamFile {{ version 2.0; format ascii; class dictionary; object blockMeshDict; }}\nvertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );\nblocks ( hex {hex} (5 1 1) simpleGrading ({sections} 1 1) );\nedges ( );\nboundary ( );\nmergePatchPairs ( );\ndefaultPatch {{ name defaultFaces; type empty; }}\n"
        );
        let (_, points) = block_mesh(&text, "along");
        let mut xs: Vec<f64> = points.iter().filter(|p| p[1] == 0.0 && p[2] == 0.0).map(|p| p[0]).collect();
        xs.sort_by(f64::total_cmp);
        xs
    };
    let inside =
        |xs: &[f64], lo: f64, hi: f64| xs.iter().filter(|x| **x > lo + 1e-12 && **x < hi - 1e-12).count() as i64;
    for (sections, list, tie) in [
        ("((0.5 0.5 2) (0.5 0.5 0.5))", [[0.5, 0.5, 2.0], [0.5, 0.5, 0.5]], true),
        ("((0.3 0.4 2) (0.7 0.6 0.5))", [[0.3, 0.4, 2.0], [0.7, 0.6, 0.5]], false),
    ] {
        let counts = blocks::line_divide_counts(&list, 5).unwrap();
        let first = list[0][0] / (list[0][0] + list[1][0]);
        let forward = along("(0 1 2 3 4 5 6 7)", sections);
        let reversed = along("(2 3 0 1 6 7 4 5)", sections);
        eprintln!("{version} {sections}: forward x {forward:?}, reversed x {reversed:?}, counts {counts:?}");
        assert_eq!(forward.len(), 6);
        // Forward: the first section's cells from x = 0; reversed: from x = 1.
        assert_eq!(inside(&forward, 0.0, first) + 1, counts[0], "{sections}");
        assert_eq!(inside(&reversed, 1.0 - first, 1.0) + 1, counts[0], "{sections}");
        // The grading turned round, numbered along x: the reversed block's points, unless a
        // tie shares the cells otherwise (then the import refuses to turn it round).
        let turned = format!(
            "(({} {} {}) ({} {} {}))",
            list[1][0],
            list[1][1],
            1.0 / list[1][2],
            list[0][0],
            list[0][1],
            1.0 / list[0][2]
        );
        let mirrored = along("(0 1 2 3 4 5 6 7)", &turned);
        let same = mirrored.iter().zip(&reversed).all(|(a, b)| (a - b).abs() < 1e-9);
        assert_eq!(same, !tie, "{sections}: {mirrored:?} {reversed:?}");
        let refused = blocks::Grading::Sections(list.to_vec()).mirrored(5).is_err();
        assert_eq!(refused, tie, "{sections}");
    }
}
