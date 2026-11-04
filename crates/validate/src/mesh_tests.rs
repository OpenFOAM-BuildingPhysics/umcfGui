//! Mesh quality on generated block meshes.

use umcf_core::MemFs;
use umcf_foam_mesh::{Limits, PolyMesh};

use super::mesh::*;

fn header(class: &str, object: &str) -> String {
    format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n    object {object};\n}}\n")
}

/// An `nx × ny × nz` block of unit cells, its points moved by `warp`, with one boundary patch
/// per side (`empty` on ±z when `two_d`), written as `constant/polyMesh`.
fn block(n: [usize; 3], two_d: bool, warp: &dyn Fn([f64; 3]) -> [f64; 3]) -> PolyMesh {
    let [nx, ny, nz] = n;
    let pid = |i: usize, j: usize, k: usize| (i + (nx + 1) * (j + (ny + 1) * k)) as u32;
    let cid = |i: usize, j: usize, k: usize| (i + nx * (j + ny * k)) as u32;
    let mut points = Vec::new();
    for k in 0..=nz {
        for j in 0..=ny {
            for i in 0..=nx {
                points.push(warp([i as f64, j as f64, k as f64]));
            }
        }
    }
    // The quad of the cell side at (i, j, k) normal to `axis`, ordered so that its normal
    // points along +axis.
    let quad = |axis: usize, i: usize, j: usize, k: usize| -> [u32; 4] {
        match axis {
            0 => [pid(i, j, k), pid(i, j + 1, k), pid(i, j + 1, k + 1), pid(i, j, k + 1)],
            1 => [pid(i, j, k), pid(i, j, k + 1), pid(i + 1, j, k + 1), pid(i + 1, j, k)],
            _ => [pid(i, j, k), pid(i + 1, j, k), pid(i + 1, j + 1, k), pid(i, j + 1, k)],
        }
    };
    let (mut faces, mut owner, mut neighbour) = (Vec::new(), Vec::new(), Vec::new());
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let c = cid(i, j, k);
                if i + 1 < nx {
                    faces.push(quad(0, i + 1, j, k));
                    owner.push(c);
                    neighbour.push(cid(i + 1, j, k));
                }
                if j + 1 < ny {
                    faces.push(quad(1, i, j + 1, k));
                    owner.push(c);
                    neighbour.push(cid(i, j + 1, k));
                }
                if k + 1 < nz {
                    faces.push(quad(2, i, j, k + 1));
                    owner.push(c);
                    neighbour.push(cid(i, j, k + 1));
                }
            }
        }
    }
    let mut patches = Vec::new();
    let rev = |q: [u32; 4]| [q[0], q[3], q[2], q[1]];
    let mut side = |name: &str, kind: &str, fs: Vec<([u32; 4], u32)>| {
        let start = faces.len();
        for (f, c) in fs {
            faces.push(f);
            owner.push(c);
        }
        patches.push(format!(
            "    {name}\n    {{\n        type {kind};\n        nFaces {};\n        startFace {start};\n    }}\n",
            faces.len() - start
        ));
    };
    let mut xm = Vec::new();
    let mut xp = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            xm.push((rev(quad(0, 0, j, k)), cid(0, j, k)));
            xp.push((quad(0, nx, j, k), cid(nx - 1, j, k)));
        }
    }
    side("xMin", "patch", xm);
    side("xMax", "patch", xp);
    let (mut ym, mut yp) = (Vec::new(), Vec::new());
    for k in 0..nz {
        for i in 0..nx {
            ym.push((rev(quad(1, i, 0, k)), cid(i, 0, k)));
            yp.push((quad(1, i, ny, k), cid(i, ny - 1, k)));
        }
    }
    side("yMin", "wall", ym);
    side("yMax", "wall", yp);
    let (mut zm, mut zp) = (Vec::new(), Vec::new());
    for j in 0..ny {
        for i in 0..nx {
            zm.push((rev(quad(2, i, j, 0)), cid(i, j, 0)));
            zp.push((quad(2, i, j, nz), cid(i, j, nz - 1)));
        }
    }
    let kind = if two_d { "empty" } else { "wall" };
    side("zMin", kind, zm);
    side("zMax", kind, zp);
    let list = |items: Vec<String>| format!("{}\n(\n{}\n)\n", items.len(), items.join("\n"));
    let fs = MemFs::new()
        .with(
            "constant/polyMesh/points",
            format!(
                "{}{}",
                header("vectorField", "points"),
                list(points.iter().map(|p| format!("({} {} {})", p[0], p[1], p[2])).collect())
            ),
        )
        .with(
            "constant/polyMesh/faces",
            format!(
                "{}{}",
                header("faceList", "faces"),
                list(faces.iter().map(|f| format!("4({} {} {} {})", f[0], f[1], f[2], f[3])).collect())
            ),
        )
        .with(
            "constant/polyMesh/owner",
            format!("{}{}", header("labelList", "owner"), list(owner.iter().map(u32::to_string).collect())),
        )
        .with(
            "constant/polyMesh/neighbour",
            format!("{}{}", header("labelList", "neighbour"), list(neighbour.iter().map(u32::to_string).collect())),
        )
        .with(
            "constant/polyMesh/boundary",
            format!("{}{}\n(\n{})\n", header("polyBoundaryMesh", "boundary"), patches.len(), patches.concat()),
        );
    PolyMesh::read(&fs, "constant/polyMesh", &Limits::default()).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

#[test]
fn a_regular_block_is_clean() {
    let m = block([3, 2, 2], false, &|p| p);
    let q = quality(&m);
    assert_eq!(q.n_cells, 12);
    assert!(close(q.total_volume, 12.0) && close(q.min_volume, 1.0) && close(q.max_volume, 1.0), "{q:?}");
    assert!(close(q.min_face_area, 1.0) && close(q.max_face_area, 1.0));
    assert!(q.max_non_orthogonality < 1e-6 && q.max_skewness < 1e-9, "{q:?}");
    assert!(q.max_openness < 1e-12 && q.boundary_openness.iter().all(|x| x.abs() < 1e-12));
    assert!(close(q.max_aspect_ratio, 1.0), "{}", q.max_aspect_ratio);
    assert_eq!(q.directions, [true; 3]);
    assert!(diagnostics(&q, "air").is_empty());
}

#[test]
fn stretched_cells_and_two_dimensional_meshes() {
    // 2000:1 cells in x.
    let q = quality(&block([2, 1, 1], false, &|p| [p[0] * 2000.0, p[1], p[2]]));
    assert!(q.max_aspect_ratio > ASPECT_THRESHOLD);
    assert_eq!(q.high_aspect_cells.count, 2);
    let codes: Vec<&str> = diagnostics(&q, "").iter().map(|d| d.code).collect();
    assert_eq!(codes, ["mesh.aspect-ratio"]);
    // A 2D mesh: the empty direction doesn't count.
    let q = quality(&block([2, 2, 1], true, &|p| [p[0], p[1], p[2] * 5000.0]));
    assert_eq!(q.directions, [true, true, false]);
    assert!(close(q.max_aspect_ratio, 1.0), "{}", q.max_aspect_ratio);
}

#[test]
fn sheared_cells_are_non_orthogonal() {
    // x += 3 z: the cell centres of two layers are (3, 0, 1) apart across a horizontal face,
    // atan(3) = 71.565° from its normal.
    let q = quality(&block([1, 1, 3], false, &|p| [p[0] + 3.0 * p[2], p[1], p[2]]));
    assert!(close(q.max_non_orthogonality, 3f64.atan().to_degrees()), "{}", q.max_non_orthogonality);
    assert_eq!(q.severe_non_orthogonal_faces.count, 2);
    assert_eq!(q.non_orthogonality_errors.count, 0);
    assert!(close(q.total_volume, 3.0));
    let codes: Vec<&str> = diagnostics(&q, "").iter().map(|d| d.code).collect();
    assert!(codes.contains(&"mesh.non-orthogonal"), "{codes:?}");
    // x += 2 z: 63.4°, below the threshold.
    let q = quality(&block([1, 1, 3], false, &|p| [p[0] + 2.0 * p[2], p[1], p[2]]));
    assert_eq!(q.severe_non_orthogonal_faces.count, 0);
}

#[test]
fn inverted_cells_are_errors() {
    // Fold the top layer under the bottom one: its volume is negative.
    let q = quality(&block([1, 1, 2], false, &|p| if p[2] == 2.0 { [p[0], p[1], -1.0] } else { p }));
    assert!(q.negative_volume_cells.count >= 1, "{q:?}");
    assert!(q.wrong_pyramids.count >= 1);
    let d = diagnostics(&q, "street");
    assert!(d.iter().any(|d| d.code == "mesh.negative-volume" && d.level == crate::Level::Error));
    assert_eq!(d[0].region.as_deref(), Some("street"));
    assert_eq!(d[0].path, "constant/street/polyMesh");
}

#[test]
fn the_face_algorithm_matches_openfoam() {
    // A triangle, and a planar quad whose centre is its centroid, not the average of its points.
    let m = block([1, 1, 1], false, &|p| if p == [1.0, 1.0, 0.0] { [3.0, 3.0, 0.0] } else { p });
    // The zMin face: points (0,0,0) (0,1,0) (3,3,0) (1,0,0), reversed order: normal -z.
    let i = m.boundary.iter().find(|p| p.name == "zMin").unwrap().start_face;
    let (a, c) = face_area_centre(&m, i);
    assert!(close(a[2], -3.0) && a[0].abs() < 1e-12, "{a:?}");
    // The kite (0,0) (1,0) (3,3) (0,1) has area 3 and its centroid at (7/6, 7/6); OpenFOAM's
    // algorithm gives the exact centroid of a planar face.
    assert!(close(c[0], 7.0 / 6.0) && close(c[1], 7.0 / 6.0) && c[2].abs() < 1e-12, "{c:?}");
}

/// The numbers `checkMesh` (OpenFOAM-12) prints for the 3×3 test case's meshes. Opt-in:
/// `UMCF_CHECKMESH_CASE` names a copy of `Mesh_3x3_small`.
#[test]
fn matches_checkmesh_on_the_3x3_case() {
    let Ok(dir) = std::env::var("UMCF_CHECKMESH_CASE") else {
        eprintln!("skipped: set UMCF_CHECKMESH_CASE to the Mesh_3x3_small case");
        return;
    };
    let fs = umcf_core::DirFs::new(dir);
    // region, cells, max aspect, high-aspect cells, min / max / total volume, min / max face
    // area, max non-orthogonality, average, max skewness (log of `checkMesh -region <r>`,
    // OpenFOAM-12).
    type Row = (&'static str, usize, f64, usize, [f64; 3], [f64; 2], f64, f64, f64);
    let rows: [Row; 5] = [
        ("air", 25588, 149.856, 0, [0.439155, 645010.0, 3.4585e7], [0.559504, 12564.6], 69.2356, 1.42874, 0.823455),
        ("buildings", 7200, 2045.56, 2880, [0.0608934, 0.568615, 1796.4], [0.0121787, 25.0], 41.9873, 23.3615, 2.5),
        ("vegetation", 14520, 5.7144, 0, [7.25043, 18368.1, 3.45848e7], [1.17894, 816.359], 38.9061, 1.79496, 1.38358),
        ("street", 6528, 1336.76, 1632, [0.0935094, 29.2079, 27200.0], [0.0187019, 25.0], 0.0, 0.0, 0.0),
        ("roofs", 4500, 1336.76, 1800, [0.0935094, 1.12882, 2250.0], [0.0187019, 25.0], 0.0, 0.0, 0.0),
    ];
    for (r, cells, aspect, n_aspect, vol, area, non_orth, avg, skew) in rows {
        let [min_vol, max_vol, total] = vol;
        let m = PolyMesh::read(&fs, &umcf_foam_mesh::mesh_dir(r), &Limits::default()).unwrap();
        let q = quality(&m);
        let near = |a: f64, b: f64| (a - b).abs() <= 5e-6 * b.abs().max(1e-3);
        assert_eq!(q.n_cells, cells, "{r}");
        assert!(near(q.max_aspect_ratio, aspect), "{r}: aspect {}", q.max_aspect_ratio);
        assert_eq!(q.high_aspect_cells.count, n_aspect, "{r}");
        assert!(near(q.min_volume, min_vol), "{r}: min volume {}", q.min_volume);
        assert!(near(q.max_volume, max_vol) && near(q.total_volume, total), "{r}: {} {}", q.max_volume, q.total_volume);
        assert!(near(q.min_face_area, area[0]) && near(q.max_face_area, area[1]), "{r}: areas");
        assert!(q.max_openness < 1e-12 && q.boundary_openness.iter().all(|x| x.abs() < 1e-12), "{r}: openness");
        assert!(near(q.max_non_orthogonality, non_orth) || non_orth == 0.0 && q.max_non_orthogonality < 1e-5, "{r}");
        assert!(near(q.average_non_orthogonality, avg) || avg == 0.0 && q.average_non_orthogonality < 1e-5, "{r}");
        assert!(near(q.max_skewness, skew) || skew == 0.0 && q.max_skewness < 1e-9, "{r}: skew {}", q.max_skewness);
    }
}

#[test]
fn points_that_are_not_numbers_are_an_error() {
    for bad in [f64::NAN, f64::INFINITY] {
        let q = quality(&block([2, 1, 1], false, &|p| if p == [1.0, 1.0, 1.0] { [bad, 1.0, 1.0] } else { p }));
        assert_eq!(q.non_finite_points.count, 1);
        let codes: Vec<&str> = diagnostics(&q, "").iter().map(|d| d.code).collect();
        assert!(codes.contains(&"mesh.non-finite"), "{codes:?}");
    }
}

#[test]
fn finite_points_that_overflow_are_an_error() {
    let q = quality(&block([2, 1, 1], false, &|p| if p == [1.0, 1.0, 1.0] { [1e300, 1.0, 1.0] } else { p }));
    assert_eq!(q.non_finite_points.count, 0);
    // The components stay finite; the area's magnitude does not.
    assert!(q.non_finite_faces.count > 0, "{q:?}");
    assert!(diagnostics(&q, "").iter().any(|d| d.code == "mesh.non-finite"));
}

#[test]
fn pyramids_are_counted_per_pyramid() {
    let q = quality(&block([1, 1, 2], false, &|p| if p[2] == 2.0 { [p[0], p[1], -1.0] } else { p }));
    assert!(
        q.wrong_pyramids.count >= q.wrong_pyramids.places && q.wrong_pyramids.places == q.wrong_pyramids.first.len()
    );
    let d = diagnostics(&q, "");
    let m = &d.iter().find(|d| d.code == "mesh.face-pyramids").unwrap().message;
    assert!(m.starts_with(&format!("{} pyramids on {} faces", q.wrong_pyramids.count, q.wrong_pyramids.places)), "{m}");
    assert!(!m.ends_with("…"), "{m}");
    assert_eq!(unreadable("air", "no points").code, "mesh.unreadable");
}

#[test]
fn aspect_ratios_print_readably() {
    let q = quality(&block([1, 1, 1], false, &|p| [p[0] * 1e9, p[1], p[2]]));
    let d = diagnostics(&q, "");
    let m = &d.iter().find(|d| d.code == "mesh.aspect-ratio").unwrap().message;
    assert!(m.contains("e"), "{m}");
    assert!(m.len() < 200, "{m}");
}

const OF8_LOG: &str = "Build  : 8\nExec   : checkMesh\nDate   : Nov 13 2025\nCreate polyMesh for time = constant\n\nChecking geometry...\n ***High aspect ratio cells found, Max aspect ratio: 1147.3909, number of cells 4025\n  <<Writing 4025 cells with high aspect ratio to set highAspectRatioCells\n\nFailed 1 mesh checks.\n\nEnd\n";
const OF12_LOG: &str = "Build  : 12\nExec   : checkMesh -region vegetation -noFunctionObjects -writeSets\n   *Number of regions: 9\n ***Error in face pyramids: 1440 faces are incorrectly oriented.\n  <<Writing 1440 faces with incorrect orientation to set wrongOrientedFaces\nFailed 1 mesh checks.\n";

#[test]
fn checkmesh_logs() {
    let l = parse_checkmesh_log("log.checkMesh.ground", OF8_LOG).pop().unwrap();
    assert_eq!((l.region.as_str(), l.version.as_deref(), l.failed, l.ok), ("", Some("8"), Some(1), false));
    assert_eq!(l.problems, ["High aspect ratio cells found, Max aspect ratio: 1147.3909, number of cells 4025"]);
    let d = checkmesh_log_diagnostics(&l, None);
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].level, d[0].code, d[0].path.as_str()),
        (crate::Level::Warning, "mesh.checkmesh", "log.checkMesh.ground")
    );
    assert!(d[0].message.starts_with("checkMesh (log.checkMesh.ground, OpenFOAM-8): High aspect"), "{}", d[0].message);

    let l = parse_checkmesh_log("log.checkMesh.vegetation", OF12_LOG).pop().unwrap();
    assert_eq!(l.region, "vegetation");
    assert_eq!(l.notes, ["Number of regions: 9"]);
    let d = checkmesh_log_diagnostics(&l, None);
    let levels: Vec<crate::Level> = d.iter().map(|d| d.level).collect();
    assert_eq!(levels, [crate::Level::Error, crate::Level::Info]);
    assert_eq!(d[0].region.as_deref(), Some("vegetation"));

    // Mesh OK; a log cut short; another utility's log.
    let ok = parse_checkmesh_log("log.checkMesh", "Exec   : checkMesh\nMesh OK.\n").pop().unwrap();
    assert!(ok.ok && checkmesh_log_diagnostics(&ok, None).is_empty());
    let cut =
        parse_checkmesh_log("log.checkMesh", "Exec   : checkMesh -allGeometry\nChecking geometry...\n").pop().unwrap();
    assert!(checkmesh_log_diagnostics(&cut, None)[0].message.contains("stops before"));
    assert_eq!(parse_checkmesh_log("log.blockMesh", "Exec   : blockMesh\n"), []);
    assert_eq!(parse_checkmesh_log("log.x", "no header at all"), []);
    // One result per time checked.
    let multi = "Build  : 8\nExec   : checkMesh -time 3,8\nCreate polyMesh for time = 3\n\nTime = 3\n ***Max skewness = 5.7, 7 highly skew faces\nFailed 1 mesh checks.\n\nTime = 8\n ***Max skewness = 5.8, 8 highly skew faces\nFailed 1 mesh checks.\n";
    let logs = parse_checkmesh_log("log.checkMesh", multi);
    let times: Vec<Option<&str>> = logs.iter().map(|l| l.time.as_deref()).collect();
    assert_eq!(times, [Some("3"), Some("8")]);
    assert_eq!(logs[1].problems, ["Max skewness = 5.8, 8 highly skew faces"]);
    assert_eq!(logs[1].version.as_deref(), Some("8"));
    assert!(checkmesh_log_diagnostics(&logs[0], None)[0].message.contains("OpenFOAM-8, time 3)"));
}

#[test]
fn stale_sets() {
    let d = stale_set("air", "skewFaces", 99, 10, SetOf::Faces);
    assert_eq!((d.code, d.path.as_str()), ("mesh.stale-set", "constant/air/polyMesh/sets/skewFaces"));
    assert!(CHECKMESH_SETS.iter().any(|(n, of, _)| *n == "wrongOrientedFaces" && *of == SetOf::Faces));
    assert!(!CHECKMESH_SETS.iter().any(|(n, _, _)| *n == "trees" || *n == "wrongFaces"));
}

#[test]
fn vegetation_regions_get_warnings() {
    let q = quality(&block([1, 1, 2], false, &|p| if p[2] == 2.0 { [p[0], p[1], -1.0] } else { p }));
    let mut d = diagnostics(&q, "vegetation");
    d.push(unreadable("vegetation", "no points"));
    assert!(d.iter().any(|d| d.level == crate::Level::Error && d.code == "mesh.face-pyramids"));
    soften_for_vegetation(&mut d);
    for x in &d {
        let expect = if x.code == "mesh.unreadable" { crate::Level::Error } else { crate::Level::Warning };
        assert_eq!(x.level, expect, "{}", x.code);
    }
    assert!(d.iter().find(|x| x.code == "mesh.face-pyramids").unwrap().message.contains("vegetation region"));
}

#[test]
fn checkmesh_levels_and_sizes() {
    let log = "Exec   : checkMesh\n    points:           12\n    faces:            20\n    cells:            4\n ***Duplicate boundary patch walls\n ***Max skewness = 5, 1 highly skew faces\n ***Cells with invalid face labels found, number of cells 2\n ***Boundary openness (1 0 0) possible hole in boundary description.\nFailed 3 mesh checks.\n";
    let l = parse_checkmesh_log("log.checkMesh", log).pop().unwrap();
    assert_eq!((l.cells, l.faces, l.points), (Some(4), Some(20), Some(12)));
    let levels: Vec<crate::Level> = checkmesh_log_diagnostics(&l, None).iter().map(|d| d.level).collect();
    use crate::Level::{Error, Warning};
    assert_eq!(levels, [Error, Warning, Error, Warning]);
    let fatal = "Exec   : checkMesh\n\n--> FOAM FATAL ERROR: \ncannot find file \"points\"\n\nFOAM exiting\n";
    let l = parse_checkmesh_log("log.checkMesh", fatal).pop().unwrap();
    assert_eq!(l.fatal.as_deref(), Some("cannot find file \"points\""));
    let d = checkmesh_log_diagnostics(&l, None);
    assert_eq!((d.len(), d[0].level), (1, Error));
    assert!(CHECKMESH_SETS.iter().any(|(n, _, _)| *n == "nonManifoldPoints"));
}

#[test]
fn checkmesh_findings_the_native_check_makes_are_notes() {
    let log = "Exec   : checkMesh\n ***Error in face pyramids: 3 faces are incorrectly oriented.\n ***Faces with small interpolation weight (< 0.05) found, number of faces: 2\n    ***Max skewness = 5, 1 highly skew faces detected\nFailed 3 mesh checks.\n";
    let l = parse_checkmesh_log("log.checkMesh", log).pop().unwrap();
    use crate::Level::{Error, Info, Warning};
    let levels = |native: Option<&Quality>| -> Vec<crate::Level> {
        checkmesh_log_diagnostics(&l, native).iter().map(|d| d.level).collect()
    };
    assert_eq!(levels(None), [Error, Warning, Warning]);
    // The native check found wrong pyramids: that one is the native check's; the weights
    // (-allGeometry) stay; the skewness it didn't find keeps checkMesh's level.
    let mut q = Quality::default();
    q.wrong_pyramids.count = 3;
    assert_eq!(levels(Some(&q)), [Info, Warning, Warning]);
    assert!(checkmesh_log_diagnostics(&l, Some(&q))[2].message.contains("doesn't find it"));
    // Found nothing natively: the pyramids stay an error.
    assert_eq!(levels(Some(&Quality::default())), [Error, Warning, Warning]);
    // The zip-up check (topology) has no native counterpart: it stays an error.
    let zip = "Exec   : checkMesh\n ***Open cells found, number of cells: 2. This problem may be fixable using the zipUpMesh utility.\n ***Open cells found, max cell openness: 0.1, number of open cells 1\nFailed 2 mesh checks.\n";
    let l = parse_checkmesh_log("log.checkMesh", zip).pop().unwrap();
    let mut q = Quality::default();
    q.open_cells.count = 1;
    let levels: Vec<crate::Level> = checkmesh_log_diagnostics(&l, Some(&q)).iter().map(|d| d.level).collect();
    assert_eq!(levels, [Error, Info]);
    // A parallel log's processor prefix.
    let par = "[0] Exec   : checkMesh -parallel\n[0] \n[0] --> FOAM FATAL ERROR: \n[0] cannot open file\n";
    let l = parse_checkmesh_log("log.checkMesh", par).pop().unwrap();
    assert_eq!(l.fatal.as_deref(), Some("cannot open file"));
}

#[test]
fn a_long_checkmesh_log_is_cut_with_a_note() {
    // With the banner before the first time, as in every real log.
    let mut log = String::from("Exec   : checkMesh -allTime\nCreate time\n\n");
    for _ in 0..(MAX_LOG_RESULTS + 50) {
        log.push_str("Time = 1\n");
    }
    let logs = parse_checkmesh_log("log.checkMesh", &log);
    assert_eq!(logs.len(), MAX_LOG_RESULTS);
    assert!(logs.last().unwrap().truncated);
    let d = checkmesh_log_diagnostics(logs.last().unwrap(), None);
    assert!(d.iter().any(|x| x.message.contains("longer than is read")));

    let mut log = String::from("Exec   : checkMesh\n");
    for _ in 0..(MAX_LOG_FINDINGS + 50) {
        log.push_str(" ***x\n *y\n");
    }
    let l = parse_checkmesh_log("log.checkMesh", &log).pop().unwrap();
    assert_eq!(l.problems.len() + l.notes.len(), MAX_LOG_FINDINGS);
    assert!(l.truncated);
}
