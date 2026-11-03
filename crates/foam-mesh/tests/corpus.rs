//! Mesh reading over real cases.
//!
//! - `UMCF_CASES` (`:`-separated case directories): every `constant/<region>/polyMesh` (and
//!   `constant/polyMesh`) reads and passes the consistency checks. Each case must have a mesh.
//! - `UMCF_FIELD_PAIR=<ascii case>:<binary case>`: a copy converted to binary by OpenFOAM's
//!   `foamFormatConvert` reads back bit for bit the same mesh.

// Native-only test: timing with the clock is fine (the lint guards wasm builds).
#![allow(clippy::disallowed_methods)]

use std::time::Instant;

use umcf_core::{CaseFs, DirFs};
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};

/// Regions with a mesh: `constant/<r>/polyMesh` directories, and `""` for `constant/polyMesh`.
fn mesh_regions(fs: &DirFs) -> Vec<String> {
    let mut out: Vec<String> = fs
        .list("constant")
        .unwrap_or_default()
        .into_iter()
        .filter(|r| fs.is_dir(&format!("constant/{r}/polyMesh")))
        .collect();
    if fs.is_dir("constant/polyMesh") {
        out.push(String::new());
    }
    out
}

#[test]
fn reference_case_meshes() {
    let Ok(cases) = std::env::var("UMCF_CASES") else {
        eprintln!("SKIP reference_case_meshes: set UMCF_CASES=<case>:<case>… to run it");
        return;
    };
    let mut failures = Vec::new();
    for case in cases.split(':').filter(|c| !c.is_empty()) {
        let fs = DirFs::new(case);
        let regions = mesh_regions(&fs);
        if regions.is_empty() {
            failures.push(format!("{case}: no polyMesh"));
        }
        for r in regions {
            let started = Instant::now();
            match PolyMesh::read(&fs, &mesh_dir(&r), &Limits::default()) {
                Ok(m) => eprintln!(
                    "{case} {}: {} cells, {} faces ({} internal), {} points, {} patches, {} cell zones, {:.1}s",
                    if r.is_empty() { "(single region)" } else { &r },
                    m.n_cells(),
                    m.n_faces(),
                    m.n_internal_faces(),
                    m.n_points(),
                    m.boundary.len(),
                    m.cell_zones.len(),
                    started.elapsed().as_secs_f64()
                ),
                Err(e) => failures.push(format!("{case} {r}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ascii_and_binary_meshes_agree() {
    let Ok(pair) = std::env::var("UMCF_FIELD_PAIR") else {
        eprintln!("SKIP ascii_and_binary_meshes_agree: set UMCF_FIELD_PAIR=<ascii case>:<binary case>");
        return;
    };
    let Some((a, b)) = pair.split_once(':') else { panic!("UMCF_FIELD_PAIR must be <ascii case>:<binary case>") };
    let (fa, fb) = (DirFs::new(a), DirFs::new(b));
    let mut compared = 0;
    for r in mesh_regions(&fb) {
        let dir = mesh_dir(&r);
        let x = PolyMesh::read(&fa, &dir, &Limits::default()).unwrap_or_else(|e| panic!("{a} {r}: {e}"));
        let y = PolyMesh::read(&fb, &dir, &Limits::default()).unwrap_or_else(|e| panic!("{b} {r}: {e}"));
        let bits = |m: &PolyMesh| m.points.iter().flat_map(|p| p.map(f64::to_bits)).collect::<Vec<_>>();
        assert!(bits(&x) == bits(&y), "{r}: points differ");
        assert_eq!(x.n_faces(), y.n_faces(), "{r}");
        assert!((0..x.n_faces()).all(|i| x.face(i) == y.face(i)), "{r}: faces differ");
        assert_eq!((&x.owner, &x.neighbour), (&y.owner, &y.neighbour), "{r}");
        assert_eq!(x.boundary, y.boundary, "{r}");
        compared += 1;
        eprintln!("pair {r}: {} cells, {} faces identical", x.n_cells(), x.n_faces());
    }
    assert!(compared > 0, "no meshes compared");
}
