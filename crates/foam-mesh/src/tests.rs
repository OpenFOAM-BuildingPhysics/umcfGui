use std::io::Write;

use umcf_core::MemFs;
use umcf_foam_field::Arch;

use super::*;

/// Two hex cells side by side along x: 12 points, 1 internal face, patches inlet (x=0),
/// outlet (x=2) and walls (8 faces).
fn points() -> Vec<[f64; 3]> {
    let mut p = Vec::new();
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..3 {
                p.push([f64::from(x), f64::from(y) * 0.5, f64::from(z) * 0.25]);
            }
        }
    }
    p
}

fn faces() -> Vec<Vec<u32>> {
    vec![
        vec![1, 4, 10, 7], // internal
        vec![0, 6, 9, 3],  // inlet
        vec![2, 5, 11, 8], // outlet
        vec![0, 3, 4, 1],  // walls…
        vec![1, 4, 5, 2],
        vec![6, 7, 10, 9],
        vec![7, 8, 11, 10],
        vec![0, 1, 7, 6],
        vec![1, 2, 8, 7],
        vec![3, 9, 10, 4],
        vec![4, 10, 11, 5],
    ]
}

const OWNER: [u32; 11] = [0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1];
const NEIGHBOUR: [u32; 1] = [1];

fn header(format: &str, class: &str, arch: Option<Arch>, note: &str) -> String {
    let arch = arch.map_or(String::new(), |a| {
        format!(
            "    arch        \"{};label={};scalar={}\";\n",
            if a.big_endian { "MSB" } else { "LSB" },
            a.label_bytes * 8,
            a.scalar_bytes * 8
        )
    });
    format!(
        "/*--------------------------------*- C++ -*----------------------------------*\\\n  banner\n\\*---------------------------------------------------------------------------*/\nFoamFile\n{{\n    format      {format};\n{arch}    class       {class};\n{note}    location    \"constant/polyMesh\";\n    object      x;\n}}\n// * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * //\n\n"
    )
}

const BOUNDARY: &str = "FoamFile { format ascii; class polyBoundaryMesh; object boundary; }\n3\n(\n    inlet\n    {\n        type            patch;\n        nFaces          1;\n        startFace       1;\n    }\n    outlet\n    {\n        type            patch;\n        nFaces          1;\n        startFace       2;\n    }\n    walls\n    {\n        type            wall;\n        inGroups        List<word> 1(wall);\n        nFaces          8;\n        startFace       3;\n    }\n)\n";

const ZONES: &str = "FoamFile { format ascii; class regIOobject; object cellZones; }\n2\n(\nleft\n{\n    type cellZone;\ncellLabels      List<label> 1(0);\n}\nboth\n{\n    type cellZone;\ncellLabels      List<label> 2(0 1);\n}\n)\n";

fn ascii_mesh() -> MemFs {
    let d = "constant/air/polyMesh";
    let pts: String = points().iter().map(|p| format!("({} {} {})\n", p[0], p[1], p[2])).collect();
    let fcs: String = faces().iter().map(|f| format!("4({} {} {} {})\n", f[0], f[1], f[2], f[3])).collect();
    let note = "    note        \"nPoints:12  nCells:2  nFaces:11  nInternalFaces:1\";\n";
    MemFs::new()
        .with(&format!("{d}/points"), format!("{}12\n(\n{pts})\n", header("ascii", "vectorField", None, "")))
        .with(&format!("{d}/faces"), format!("{}11\n(\n{fcs})\n", header("ascii", "faceList", None, "")))
        .with(
            &format!("{d}/owner"),
            format!(
                "{}11\n(\n{}\n)\n",
                header("ascii", "labelList", None, note),
                OWNER.map(|o| o.to_string()).join("\n")
            ),
        )
        .with(&format!("{d}/neighbour"), format!("{}1\n(\n1\n)\n", header("ascii", "labelList", None, note)))
        .with(&format!("{d}/boundary"), BOUNDARY)
        .with(&format!("{d}/cellZones"), ZONES)
}

fn label_bytes(a: Arch, v: u32) -> Vec<u8> {
    match (a.label_bytes, a.big_endian) {
        (8, true) => u64::from(v).to_be_bytes().to_vec(),
        (8, false) => u64::from(v).to_le_bytes().to_vec(),
        (_, true) => v.to_be_bytes().to_vec(),
        (_, false) => v.to_le_bytes().to_vec(),
    }
}

fn scalar_bytes(a: Arch, v: f64) -> Vec<u8> {
    match (a.scalar_bytes, a.big_endian) {
        (8, true) => v.to_be_bytes().to_vec(),
        (8, false) => v.to_le_bytes().to_vec(),
        (_, true) => (v as f32).to_be_bytes().to_vec(),
        (_, false) => (v as f32).to_le_bytes().to_vec(),
    }
}

fn binary_labels(a: Arch, v: &[u32]) -> Vec<u8> {
    let mut out = format!("{}(", v.len()).into_bytes();
    for x in v {
        out.extend(label_bytes(a, *x));
    }
    out.extend_from_slice(b")\n");
    out
}

/// The same mesh as OpenFOAM writes it in binary: faces as a faceCompactList.
fn binary_mesh(a: Arch) -> MemFs {
    let d = "constant/air/polyMesh";
    let mut pts = header("binary", "vectorField", Some(a), "").into_bytes();
    write!(pts, "12\n(").unwrap();
    for p in points() {
        for c in p {
            pts.extend(scalar_bytes(a, c));
        }
    }
    pts.extend_from_slice(b")\n");
    let mut offsets = vec![0u32];
    let mut flat = Vec::new();
    for f in faces() {
        flat.extend(f);
        offsets.push(flat.len() as u32);
    }
    let mut fcs = header("binary", "faceCompactList", Some(a), "").into_bytes();
    fcs.extend(binary_labels(a, &offsets));
    fcs.extend(binary_labels(a, &flat));
    let note = "    note        \"nPoints:12  nCells:2  nFaces:11  nInternalFaces:1\";\n";
    let mut own = header("binary", "labelList", Some(a), note).into_bytes();
    own.extend(binary_labels(a, &OWNER));
    let mut nbr = header("binary", "labelList", Some(a), note).into_bytes();
    nbr.extend(binary_labels(a, &NEIGHBOUR));
    MemFs::new()
        .with(&format!("{d}/points"), pts)
        .with(&format!("{d}/faces"), fcs)
        .with(&format!("{d}/owner"), own)
        .with(&format!("{d}/neighbour"), nbr)
        .with(&format!("{d}/boundary"), BOUNDARY)
}

fn read(fs: &MemFs) -> Result<PolyMesh> {
    PolyMesh::read(fs, &mesh_dir("air"), &Limits::default())
}

fn read_boundary_mesh(fs: &MemFs) -> Result<BoundaryMesh> {
    BoundaryMesh::read(fs, &mesh_dir("air"), &Limits::default())
}

/// The boundary read alone is the whole mesh's boundary: the same points, patches and boundary
/// faces, and no internal face.
fn assert_same_boundary(b: &BoundaryMesh, m: &PolyMesh) {
    assert_eq!(b.points, m.points);
    assert_eq!(b.boundary, m.boundary);
    assert_eq!((b.n_faces(), b.first_boundary_face()), (m.n_faces(), m.n_internal_faces()));
    for i in 0..m.n_internal_faces() {
        assert_eq!(b.face(i), None, "internal face {i}");
    }
    for i in m.n_internal_faces()..m.n_faces() + 1 {
        assert_eq!(b.face(i), m.face(i), "face {i}");
    }
}

fn assert_two_hexes(m: &PolyMesh) {
    assert_eq!((m.n_points(), m.n_faces(), m.n_internal_faces(), m.n_cells()), (12, 11, 1, 2));
    assert_eq!(m.points, points());
    assert_eq!(m.face(0), Some(&[1, 4, 10, 7][..]));
    assert_eq!(m.face(10), Some(&[4, 10, 11, 5][..]));
    assert_eq!(m.face(11), None);
    assert_eq!(m.owner, OWNER);
    assert_eq!(m.neighbour, NEIGHBOUR);
    let walls = m.patch("walls").unwrap();
    assert_eq!((walls.kind.as_str(), walls.faces(), walls.groups.clone()), ("wall", 3..11, vec!["wall".to_string()]));
    assert_eq!(m.bounds(), Some(([0.0, 0.0, 0.0], [2.0, 0.5, 0.25])));
}

#[test]
fn ascii_mesh_reads() {
    let m = read(&ascii_mesh()).unwrap();
    assert_two_hexes(&m);
    assert_same_boundary(&read_boundary_mesh(&ascii_mesh()).unwrap(), &m);
    assert_eq!(m.cell_zone("both").map(|z| z.cells.clone()), Some(vec![0, 1]));
    assert_eq!(m.cell_zone("left").map(|z| z.cells.clone()), Some(vec![0]));
}

#[test]
fn binary_mesh_reads_in_every_arch() {
    for big_endian in [false, true] {
        for label_bytes in [4, 8] {
            for scalar_bytes in [4, 8] {
                let a = Arch { big_endian, label_bytes, scalar_bytes };
                let m = read(&binary_mesh(a)).unwrap_or_else(|e| panic!("{a:?}: {e}"));
                assert_two_hexes(&m);
                let b = read_boundary_mesh(&binary_mesh(a)).unwrap_or_else(|e| panic!("{a:?}: {e}"));
                assert_same_boundary(&b, &m);
                assert!(m.cell_zones.is_empty());
            }
        }
    }
}

#[test]
fn polygon_faces_grow_past_the_quad_reservation() {
    // Review finding: faces with more than four points grow the point list beyond its first
    // reservation; that growth must work (and be fallible). The internal face becomes a 20-gon
    // through points added on its edges (they are listed but used by no other face, which the
    // reader accepts).
    let mut extra = String::new();
    for k in 0..16 {
        extra.push_str(&format!("(1 {} {})\n", 0.5 * f64::from(k + 1) / 17.0, 0.0));
    }
    let pts: String = points().iter().map(|p| format!("({} {} {})\n", p[0], p[1], p[2])).collect();
    let big: Vec<String> = [1u32, 4, 10, 7].iter().map(u32::to_string).chain((12..28).map(|i| i.to_string())).collect();
    let mut fcs = format!("{}(", big.len());
    fcs.push_str(&big.join(" "));
    fcs.push_str(")\n");
    for f in faces().iter().skip(1) {
        fcs.push_str(&format!("4({} {} {} {})\n", f[0], f[1], f[2], f[3]));
    }
    let h = |class| header("ascii", class, None, "");
    let fs = with(
        with(ascii_mesh(), "points", format!("{}28\n(\n{pts}{extra})\n", h("vectorField"))),
        "faces",
        format!("{}11\n(\n{fcs})\n", h("faceList")),
    );
    let m = read(&fs).unwrap();
    assert_eq!(m.face(0).map(<[u32]>::len), Some(20));
    assert_eq!(m.face(10), Some(&[4, 10, 11, 5][..]));
    // The 20-gon is internal: passed over, not read.
    assert_same_boundary(&read_boundary_mesh(&fs).unwrap(), &m);
}

#[test]
fn gzip_files() {
    let mut fs = ascii_mesh();
    let d = "constant/air/polyMesh";
    let plain = fs.read(&format!("{d}/points"), 1 << 20).unwrap().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&plain).unwrap();
    let mut fs2 = MemFs::new();
    for name in ["faces", "owner", "neighbour", "boundary", "cellZones"] {
        fs2.insert(&format!("{d}/{name}"), fs.read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap());
    }
    fs2.insert(&format!("{d}/points.gz"), gz.finish().unwrap());
    assert_two_hexes(&read(&fs2).unwrap());
    assert_same_boundary(&read_boundary_mesh(&fs2).unwrap(), &read(&fs2).unwrap());
    fs.insert(&format!("{d}/points"), "garbage");
    assert!(read(&fs).is_err());
    assert!(read_boundary_mesh(&fs).is_err());
}

fn with(mut fs: MemFs, name: &str, contents: String) -> MemFs {
    fs.insert(&format!("constant/air/polyMesh/{name}"), contents);
    fs
}

#[test]
fn inconsistent_meshes_are_errors() {
    let h = |class| header("ascii", class, None, "");
    let cases = [
        // A face uses a point that doesn't exist.
        ("faces", format!("{}11\n(\n4(1 4 10 99)\n{})\n", h("faceList"), "4(0 6 9 3)\n".repeat(10)), "point 99"),
        // One owner short.
        ("owner", format!("{}10\n(\n0 0 1 0 1 0 1 0 1 0\n)\n", h("labelList")), "owners"),
        // Owner header promises 3 cells.
        (
            "owner",
            format!(
                "{}11\n(\n0 0 1 0 1 0 1 0 1 0 1\n)\n",
                header("ascii", "labelList", None, "    note \"nCells:3\";\n")
            ),
            "says 3 cells",
        ),
        // A gap between patches.
        (
            "boundary",
            BOUNDARY
                .replace("startFace       3;", "startFace       4;")
                .replace("nFaces          8;", "nFaces          7;"),
            "expected 3",
        ),
        // Patches stop short of the last face.
        ("boundary", BOUNDARY.replace("nFaces          8;", "nFaces          7;"), "cover faces up to 10"),
        // A zone uses a cell that doesn't exist.
        ("cellZones", ZONES.replace("2(0 1)", "2(0 5)"), "cell 5"),
        // A face with two points.
        ("faces", format!("{}11\n(\n2(1 4)\n{})\n", h("faceList"), "4(0 6 9 3)\n".repeat(10)), "fewer than 3"),
        // A sparse cell label: billions of cells without faces.
        ("owner", format!("{}11\n(\n0 0 1 0 1 0 1 0 1 0 4294967294\n)\n", h("labelList")), "cells without faces"),
    ];
    for (file, contents, expect) in cases {
        let e = read(&with(ascii_mesh(), file, contents)).unwrap_err().to_string();
        assert!(e.contains(expect), "{file}: `{e}` should mention `{expect}`");
    }
}

#[test]
fn hostile_files_are_errors() {
    let h = |class| header("ascii", class, None, "");
    // A huge count with no data: an error, not a giant allocation.
    let e =
        read(&with(ascii_mesh(), "points", format!("{}4000000000\n(\n(0 0 0)\n)\n", h("vectorField")))).unwrap_err();
    assert!(matches!(e, Error::Bad { .. } | Error::TooLarge { .. }), "{e}");
    let e = read(&with(ascii_mesh(), "owner", format!("{}11\n(\n0 0 -1 0 1 0 1 0 1 0 1\n)\n", h("labelList"))))
        .unwrap_err();
    assert!(e.to_string().contains("non-negative integer"), "{e}");
    let e =
        read(&with(ascii_mesh(), "owner", format!("{}11\n(\n0 0 99999999999 0 1 0 1 0 1 0 1\n)\n", h("labelList"))))
            .unwrap_err();
    assert!(e.to_string().contains("32 bits"), "{e}");
    // Review finding: `N{value}` asked for any size from a few bytes.
    let e = read(&with(ascii_mesh(), "owner", format!("{}4000000000{{1}}\n", h("labelList")))).unwrap_err();
    assert!(matches!(e, Error::TooLarge { what: "uniform list", .. }), "{e}");
    let ok = read(&with(ascii_mesh(), "neighbour", format!("{}1{{1}}\n", h("labelList"))));
    assert!(ok.is_ok(), "{ok:?}");
    // Review finding: a repeated `cellLabels` list skipped the mesh limits (field defaults only).
    let small = Limits { max_uniform: 4, ..Limits::default() };
    let zones = |n: usize| with(ascii_mesh(), "cellZones", ZONES.replace("2(0 1)", &format!("{n}{{1}}")));
    let e = PolyMesh::read(&zones(5), "constant/air/polyMesh", &small).unwrap_err();
    assert!(matches!(e, Error::TooLarge { what: "uniform list", .. }), "{e}");
    let ok = PolyMesh::read(&zones(4), "constant/air/polyMesh", &small).map(|m| m.cell_zones[1].cells.clone());
    assert_eq!(ok.ok(), Some(vec![1; 4]));
    // Review finding: many repeated lists, each under the cap, add up past it.
    let both = with(ascii_mesh(), "cellZones", ZONES.replace("1(0)", "3{0}").replace("2(0 1)", "3{1}"));
    let e = PolyMesh::read(&both, "constant/air/polyMesh", &small).unwrap_err();
    assert!(matches!(e, Error::TooLarge { what: "uniform list", .. }), "{e}");
    // Review finding: one written list reused by name from other zones.
    for word in ["__umcfList0", "__umcfListA0"] {
        let shared = ZONES.replace("List<label> 2(0 1)", word);
        let e = read(&with(ascii_mesh(), "cellZones", shared)).unwrap_err();
        assert!(e.to_string().contains("no cellLabels list"), "{e}");
    }
    // Review finding: digits after a list ran on into its placeholder (`0()1` named list 1).
    let run_on = ZONES.replace("List<label> 1(0)", "List<label> 0()1");
    assert!(read(&with(ascii_mesh(), "cellZones", run_on)).is_err());
    // Review finding: a cell label of u32::MAX overflowed `max + 1` (wrapping on wasm32).
    let e = read(&with(ascii_mesh(), "owner", format!("{}11\n(\n0 0 1 0 1 0 1 0 1 0 4294967295\n)\n", h("labelList"))))
        .unwrap_err();
    assert!(e.to_string().contains("out of range"), "{e}");
    // Truncated binary.
    let mut fs = binary_mesh(Arch::default());
    let pts = fs.read("constant/air/polyMesh/points", 1 << 20).unwrap().unwrap();
    fs.insert("constant/air/polyMesh/points", pts[..pts.len() - 20].to_vec());
    assert!(matches!(read(&fs), Err(Error::Truncated { .. } | Error::Bad { .. })));
    // A missing file names itself.
    let mut fs = MemFs::new();
    fs.insert("constant/air/polyMesh/points", "0()");
    let e = read(&fs).unwrap_err().to_string();
    assert!(e.contains("polyMesh/faces"), "{e}");
}

#[test]
fn fuzz_mutations_never_panic() {
    let base = ascii_mesh();
    let d = "constant/air/polyMesh";
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    };
    let iters = std::env::var("UMCF_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(3_000usize);
    let names = ["points", "faces", "owner", "neighbour", "boundary", "cellZones"];
    for _ in 0..iters {
        let name = names[next(names.len())];
        let mut bytes = base.read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap();
        for _ in 0..1 + next(3) {
            if bytes.is_empty() {
                break;
            }
            let i = next(bytes.len());
            match next(3) {
                0 => {
                    bytes.remove(i);
                }
                1 => bytes.insert(i, b"0123456789(); "[next(14)]),
                _ => bytes[i] = next(256) as u8,
            }
        }
        let mut fs = base.clone();
        fs.insert(&format!("{d}/{name}"), bytes);
        if let Ok(m) = read(&fs) {
            // Whatever reads must be consistent: every face resolves, every point is in range.
            for i in 0..m.n_faces() {
                assert!(m.face(i).unwrap().iter().all(|p| (*p as usize) < m.n_points()));
            }
        }
        if let Ok(b) = read_boundary_mesh(&fs) {
            // The same for the boundary alone: every boundary face resolves, its points in range.
            for i in b.first_boundary_face()..b.n_faces() {
                let face = b.face(i).unwrap();
                assert!(face.len() >= 3 && face.iter().all(|p| (*p as usize) < b.n_points()));
            }
        }
    }
}

#[test]
fn empty_binary_lists_have_no_parentheses() {
    // Review finding: OpenFOAM writes an empty binary list as its size alone (`UListIO.C`), so a
    // processor without internal faces has a `neighbour` of `0`, and a zone that misses a
    // processor has `cellLabels List<label> 0;` (both from decomposePar on a binary case).
    let a = Arch::default();
    let lim = Limits::default();
    for class in ["labelList", "vectorField"] {
        let src = format!("{}\n0\n\n// ****** //\n", header("binary", class, Some(a), ""));
        let (h, start) = umcf_foam_field::read_header_end(src.as_bytes()).unwrap();
        let mut r = crate::lists::Reader::new("f", src.as_bytes(), start.unwrap(), &h);
        let n = if class == "labelList" { r.labels(&lim).unwrap().len() } else { r.vectors(&lim).unwrap().len() };
        assert_eq!(n, 0, "{class}");
        // `0()` is still read too.
        let src = format!("{}\n0()\n", header("binary", class, Some(a), ""));
        let (h, start) = umcf_foam_field::read_header_end(src.as_bytes()).unwrap();
        let mut r = crate::lists::Reader::new("f", src.as_bytes(), start.unwrap(), &h);
        let n = if class == "labelList" { r.labels(&lim).unwrap().len() } else { r.vectors(&lim).unwrap().len() };
        assert_eq!(n, 0, "{class} 0()");
    }
    // A binary faceCompactList with no faces: offsets `1(0)`, points a bare `0`.
    let mut src = header("binary", "faceCompactList", Some(a), "").into_bytes();
    src.extend(binary_labels(a, &[0]));
    src.extend_from_slice(b"\n0\n\n");
    let (h, start) = umcf_foam_field::read_header_end(&src).unwrap();
    let mut r = crate::lists::Reader::new("f", &src, start.unwrap(), &h);
    let (offsets, points) = r.face_compact_list(&lim).unwrap();
    assert_eq!((offsets, points.len()), (vec![0], 0));
    let zones = format!(
        "{}2\n(\nempty\n{{\n    type cellZone;\ncellLabels      List<label> \n0\n;\n}}\nfull\n{{\n    type cellZone;\ncellLabels      List<label> \n1\n(",
        header("binary", "regIOobject", Some(a), "")
    )
    .into_bytes();
    let mut zones = zones;
    zones.extend(label_bytes(a, 1));
    zones.extend_from_slice(b");\n}\n)\n");
    let m = read(&binary_mesh(a).with("constant/air/polyMesh/cellZones", zones)).unwrap();
    assert_eq!(m.cell_zones.iter().map(|z| z.cells.len()).collect::<Vec<_>>(), [0, 1]);
}

#[test]
fn boundary_alone() {
    let fs = ascii_mesh();
    let patches = crate::read_boundary(&fs, "constant/air/polyMesh", &Limits::default()).unwrap().unwrap();
    assert_eq!(patches, read(&fs).unwrap().boundary);
    assert!(crate::read_boundary(&fs, "constant/none/polyMesh", &Limits::default()).unwrap().is_none());
}

#[test]
fn sets() {
    let mut fs = ascii_mesh();
    let d = "constant/air/polyMesh";
    fs.insert(&format!("{d}/sets/skewFaces"), format!("{}3\n(\n1\n4\n9\n)\n", header("ascii", "faceSet", None, "")));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(format!("{}1(0)\n", header("ascii", "cellSet", None, "")).as_bytes()).unwrap();
    fs.insert(&format!("{d}/sets/nonClosedCells.gz"), gz.finish().unwrap());
    assert_eq!(list_sets(&fs, d).unwrap(), ["nonClosedCells", "skewFaces"]);
    let s = read_set(&fs, d, "skewFaces", &Limits::default()).unwrap().unwrap();
    assert_eq!((s.class.as_str(), s.labels), ("faceSet", vec![1, 4, 9]));
    let s = read_set(&fs, d, "nonClosedCells", &Limits::default()).unwrap().unwrap();
    assert_eq!((s.class.as_str(), s.labels), ("cellSet", vec![0]));
    assert_eq!(read_set(&fs, d, "nope", &Limits::default()).unwrap(), None);
    assert!(list_sets(&fs, "constant/other/polyMesh").unwrap().is_empty());
    // `format binary` in the header, labels as text: how OpenFOAM writes a set.
    fs.insert(&format!("{d}/sets/bin"), format!("{}2\n(\n5 7\n)\n", header("binary", "faceSet", None, "")));
    assert_eq!(read_set(&fs, d, "bin", &Limits::default()).unwrap().unwrap().labels, vec![5, 7]);
    fs.insert(&format!("{d}/sets/bad"), format!("{}2\n(\n1 -4\n)\n", header("ascii", "faceSet", None, "")));
    assert!(read_set(&fs, d, "bad", &Limits::default()).is_err());
}

#[test]
fn boundary_mesh_needs_no_cells() {
    // No owner, no neighbour: the boundary still reads.
    let mut fs = MemFs::new();
    let d = "constant/air/polyMesh";
    for name in ["points", "faces", "boundary"] {
        fs.insert(&format!("{d}/{name}"), ascii_mesh().read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap());
    }
    let b = read_boundary_mesh(&fs).unwrap();
    assert_eq!((b.first_boundary_face(), b.n_faces()), (1, 11));
    assert!(read(&fs).is_err());
}

#[test]
fn boundary_mesh_checks_what_it_reads() {
    let h = |class| header("ascii", class, None, "");
    let faces = |list: &str| with(ascii_mesh(), "faces", format!("{}{list}", h("faceList")));
    // A boundary face with a point the mesh doesn't have.
    let mut fcs: Vec<String> = faces_text();
    fcs[3] = "4(0 3 4 99)".into();
    assert!(read_boundary_mesh(&faces(&format!("11\n(\n{}\n)\n", fcs.join("\n")))).is_err());
    // An internal face is passed over, so a bad label there doesn't matter to the boundary.
    let mut fcs: Vec<String> = faces_text();
    fcs[0] = "4(1 4 10 99)".into();
    let fs = faces(&format!("11\n(\n{}\n)\n", fcs.join("\n")));
    assert!(read(&fs).is_err());
    assert!(read_boundary_mesh(&fs).is_ok());
    // Fewer faces than the patches cover.
    assert!(read_boundary_mesh(&faces(&format!("10\n(\n{}\n)\n", faces_text()[..10].join("\n")))).is_err());
    // A face cut off before its `)`.
    assert!(read_boundary_mesh(&faces("11\n(\n4(1 4 10 7")).is_err());
}

fn faces_text() -> Vec<String> {
    faces()
        .iter()
        .map(|f| format!("{}({})", f.len(), f.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")))
        .collect()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(bytes).unwrap();
    gz.finish().unwrap()
}

/// The two hexes with `extra` copies of the internal face before it: a faces file of a few
/// megabytes, so that the stream passes the internal faces over in several chunks.
fn long_mesh(extra: usize) -> MemFs {
    let d = "constant/air/polyMesh";
    let h = |class| header("ascii", class, None, "");
    let mut fcs = String::new();
    for _ in 0..extra {
        fcs.push_str("4(1 4 10 7)\n");
    }
    for f in faces_text() {
        fcs.push_str(&f);
        fcs.push('\n');
    }
    let boundary = BOUNDARY
        .replace("startFace       1;", &format!("startFace       {};", extra + 1))
        .replace("startFace       2;", &format!("startFace       {};", extra + 2))
        .replace("startFace       3;", &format!("startFace       {};", extra + 3));
    // Points, faces and the boundary only: the two hexes' owner would note other counts.
    let mut fs = MemFs::new();
    fs.insert(&format!("{d}/points"), ascii_mesh().read(&format!("{d}/points"), 1 << 20).unwrap().unwrap());
    fs.insert(&format!("{d}/faces"), format!("{}{}\n(\n{fcs})\n", h("faceList"), extra + 11));
    fs.insert(&format!("{d}/boundary"), boundary);
    fs
}

/// `fs` with its faces file gzip-compressed.
fn faces_gz(fs: &MemFs) -> MemFs {
    let d = "constant/air/polyMesh";
    let mut out = MemFs::new();
    for name in ["points", "boundary"] {
        out.insert(&format!("{d}/{name}"), fs.read(&format!("{d}/{name}"), 1 << 30).unwrap().unwrap());
    }
    out.insert(&format!("{d}/faces.gz"), gzip(&fs.read(&format!("{d}/faces"), 1 << 30).unwrap().unwrap()));
    out
}

fn same_boundary_meshes(a: &BoundaryMesh, b: &BoundaryMesh) -> bool {
    a.points == b.points
        && a.boundary == b.boundary
        && (a.first_boundary_face(), a.n_faces()) == (b.first_boundary_face(), b.n_faces())
        && (a.first_boundary_face()..a.n_faces() + 1).all(|i| a.face(i) == b.face(i))
}

#[test]
fn compressed_faces_stream_past_the_internal_ones() {
    for extra in [0, 3, 400_000] {
        let plain = long_mesh(extra);
        let a = read_boundary_mesh(&plain).unwrap();
        let b = read_boundary_mesh(&faces_gz(&plain)).unwrap();
        assert!(same_boundary_meshes(&a, &b), "{extra} internal faces");
        assert_eq!((b.first_boundary_face(), b.n_faces()), (extra + 1, extra + 11));
        assert_eq!(b.face(extra + 1), Some(&[0, 6, 9, 3][..]));
    }
    // The two hexes as they are, compressed: the same as the whole mesh's boundary.
    assert_same_boundary(&read_boundary_mesh(&faces_gz(&ascii_mesh())).unwrap(), &read(&ascii_mesh()).unwrap());
    // Binary faces (faceCompactList) are read whole, compressed or not.
    let a = Arch { big_endian: false, label_bytes: 4, scalar_bytes: 8 };
    let b = read_boundary_mesh(&faces_gz(&binary_mesh(a))).unwrap();
    assert_same_boundary(&b, &read(&binary_mesh(a)).unwrap());
}

#[test]
fn compressed_faces_that_end_early_are_errors() {
    let d = "constant/air/polyMesh";
    let plain = long_mesh(1000);
    let text = plain.read(&format!("{d}/faces"), 1 << 30).unwrap().unwrap();
    // Cut among the internal faces, among the boundary ones, and before the list's `)`.
    for cut in [text.len() / 3, text.len() - 30, text.len() - 2] {
        let mut fs = faces_gz(&plain);
        fs.insert(&format!("{d}/faces.gz"), gzip(&text[..cut]));
        assert!(read_boundary_mesh(&fs).is_err(), "cut at {cut}");
    }
}

#[test]
fn compressed_fuzz_mutations_never_panic() {
    let d = "constant/air/polyMesh";
    let base = long_mesh(20);
    let text = base.read(&format!("{d}/faces"), 1 << 30).unwrap().unwrap();
    let plain_boundary = |fs: &MemFs| read_boundary_mesh(fs);
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    };
    let iters = std::env::var("UMCF_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(1_000usize) / 3;
    for _ in 0..iters {
        let mut bytes = text.clone();
        for _ in 0..1 + next(3) {
            let i = next(bytes.len());
            match next(3) {
                0 => {
                    bytes.remove(i);
                }
                1 => bytes.insert(i, b"0123456789(); /*"[next(16)]),
                _ => bytes[i] = next(256) as u8,
            }
        }
        let mut plain = base.clone();
        plain.insert(&format!("{d}/faces"), bytes.clone());
        let mut gz = faces_gz(&base);
        gz.insert(&format!("{d}/faces.gz"), gzip(&bytes));
        // Compressed or not, what reads is consistent; and when both read, they agree.
        let (a, b) = (plain_boundary(&plain), plain_boundary(&gz));
        if let Ok(b) = &b {
            for i in b.first_boundary_face()..b.n_faces() {
                let face = b.face(i).unwrap();
                assert!(face.len() >= 3 && face.iter().all(|p| (*p as usize) < b.n_points()));
            }
        }
        if let (Ok(a), Ok(b)) = (&a, &b) {
            assert!(same_boundary_meshes(a, b));
        }
    }
}

/// `fs` with its faces file replaced by `text`, compressed.
fn with_faces_gz(fs: &MemFs, text: &[u8]) -> MemFs {
    let mut out = faces_gz(fs);
    out.insert("constant/air/polyMesh/faces.gz", gzip(text));
    out
}

fn faces_text_of(fs: &MemFs) -> Vec<u8> {
    fs.read("constant/air/polyMesh/faces", 1 << 30).unwrap().unwrap()
}

#[test]
fn compressed_faces_keep_the_size_limit() {
    let plain = long_mesh(2000);
    let gz = faces_gz(&plain);
    let small = Limits { max_bytes: 2000, ..Limits::default() };
    let e = BoundaryMesh::read(&gz, &mesh_dir("air"), &small).unwrap_err().to_string();
    assert!(e.contains("exceeds the limit"), "{e}");
    // Just enough is enough.
    let size = faces_text_of(&plain).len() as u64;
    let enough = Limits { max_bytes: size.max(1 << 16), ..Limits::default() };
    assert!(BoundaryMesh::read(&gz, &mesh_dir("air"), &enough).is_ok());
}

#[test]
fn compressed_faces_start_after_comments_and_across_chunks() {
    let plain = long_mesh(10);
    let text = faces_text_of(&plain);
    let at = text.windows(4).position(|w| w == b"\n21\n").unwrap() + 1;
    let want = read_boundary_mesh(&plain).unwrap();
    // Comments and blank lines between the header and the count; the count just across the
    // first and the second megabyte the stream reads.
    for pad in [
        b"// a comment\n/* and\nanother */\n\n".to_vec(),
        vec![b' '; (1 << 20) - at - 1],
        vec![b' '; (1 << 20) - at],
        vec![b'\n'; (2 << 20) - at + 1],
    ] {
        let mut t = text[..at].to_vec();
        t.extend_from_slice(&pad);
        t.extend_from_slice(&text[at..]);
        let got = read_boundary_mesh(&with_faces_gz(&plain, &t)).unwrap();
        assert!(same_boundary_meshes(&got, &want), "{} bytes of padding", pad.len());
    }
}

#[test]
// A native test: the clock only guards against a hang.
#[allow(clippy::disallowed_methods)]
fn a_list_that_doesnt_start_soon_is_read_whole() {
    // Megabytes of blanks (or an open comment) before the list: the stream gives up after a
    // few megabytes and the file is read whole, the same mesh or the same error, quickly.
    let plain = long_mesh(3);
    let text = faces_text_of(&plain);
    let at = text.windows(4).position(|w| w == b"\n14\n").unwrap() + 1;
    let mut t = text[..at].to_vec();
    t.extend(std::iter::repeat_n(b' ', 6 << 20));
    t.extend_from_slice(&text[at..]);
    let started = std::time::Instant::now();
    let got = read_boundary_mesh(&with_faces_gz(&plain, &t)).unwrap();
    assert!(same_boundary_meshes(&got, &read_boundary_mesh(&plain).unwrap()));
    let mut open = text[..at].to_vec();
    open.extend_from_slice(b"/* never closed");
    open.extend(std::iter::repeat_n(b' ', 6 << 20));
    assert!(read_boundary_mesh(&with_faces_gz(&plain, &open)).is_err());
    assert!(started.elapsed().as_secs() < 20, "took {:?}", started.elapsed());
}

/// The line an error names, plain and compressed.
fn error_lines(plain: &MemFs, text: &str) -> (String, String) {
    let mut fs = plain.clone();
    fs.insert("constant/air/polyMesh/faces", text.to_string());
    let a = read_boundary_mesh(&fs).unwrap_err().to_string();
    let b = read_boundary_mesh(&with_faces_gz(plain, text.as_bytes())).unwrap_err().to_string();
    let line = |e: &str| e.split("line ").nth(1).and_then(|r| r.split(':').next()).unwrap_or("?").to_string();
    (line(&a), line(&b))
}

#[test]
fn compressed_errors_name_the_files_line() {
    // Faces passed over within one chunk, and across several.
    for extra in [5, 300_000] {
        let plain = long_mesh(extra);
        // A boundary face with a word for a label.
        let text = String::from_utf8(faces_text_of(&plain)).unwrap().replacen("4(0 6 9 3)", "4(0 6 x 3)", 1);
        let (a, b) = error_lines(&plain, &text);
        assert!(a != "?" && a == b, "{extra}: {a} / {b}");
    }
    // Nothing passed over: a mesh without internal faces.
    let h = header("ascii", "faceList", None, "");
    let fcs: Vec<String> = faces_text()[1..].to_vec();
    let boundary = BOUNDARY
        .replace("startFace       1;", "startFace       0;")
        .replace("startFace       2;", "startFace       1;")
        .replace("startFace       3;", "startFace       2;");
    // Without the two hexes' owner (it notes 11 faces with one internal).
    let no_note = format!("{}10\n(\n0 0 0 0 0 0 0 0 0 0\n)\n", header("ascii", "labelList", None, ""));
    let plain = with(
        with(with(ascii_mesh(), "boundary", boundary), "owner", no_note),
        "faces",
        format!("{h}10\n(\n{}\n)\n", fcs.join("\n")),
    );
    assert_eq!(read_boundary_mesh(&plain).unwrap().first_boundary_face(), 0);
    let text = String::from_utf8(faces_text_of(&plain)).unwrap().replacen("4(0 6 9 3)", "4(0 6 x 3)", 1);
    let (a, b) = error_lines(&plain, &text);
    assert!(a != "?" && a == b, "no internal faces: {a} / {b}");
}

#[test]
fn boundaries_without_faces() {
    let d = "constant/air/polyMesh";
    let no_patches = "FoamFile { format ascii; class polyBoundaryMesh; object boundary; }\n0\n(\n)\n";
    // No patches, and no owner to say otherwise: nothing to draw.
    let mut fs = faces_gz(&ascii_mesh());
    fs.insert(&format!("{d}/boundary"), no_patches);
    let b = read_boundary_mesh(&fs).unwrap();
    assert!(b.boundary.is_empty());
    assert_eq!(b.face(10), None);
    // No patches, but the owner header notes 10 boundary faces: they would go uncovered.
    let mut fs = ascii_mesh();
    fs.insert(&format!("{d}/boundary"), no_patches);
    assert!(read_boundary_mesh(&fs).unwrap_err().to_string().contains("1 internal faces"));
    // A patch with no faces at the end, as OpenFOAM writes one.
    let empty = BOUNDARY.replace(
        "\n)\n",
        "\n    unused\n    {\n        type            patch;\n        nFaces          0;\n        startFace       11;\n    }\n)\n",
    );
    let fs = with(ascii_mesh(), "boundary", empty.replacen("3\n(", "4\n(", 1));
    let b = read_boundary_mesh(&fs).unwrap();
    assert_eq!(b.boundary.len(), 4);
    assert_same_boundary(&b, &read(&fs).unwrap());
    assert!(read_boundary_mesh(&faces_gz(&fs)).is_ok());
}

#[test]
fn ascii_compact_faces_from_a_face_on() {
    let h = |class| header("ascii", class, None, "");
    let mut offsets = vec![0u32];
    let mut labels = Vec::new();
    for f in faces() {
        labels.extend(&f);
        offsets.push(labels.len() as u32);
    }
    let list = |v: &[u32]| format!("{}\n({})\n", v.len(), v.iter().map(u32::to_string).collect::<Vec<_>>().join(" "));
    let fs = with(ascii_mesh(), "faces", format!("{}{}{}", h("faceCompactList"), list(&offsets), list(&labels)));
    assert_same_boundary(&read_boundary_mesh(&fs).unwrap(), &read(&fs).unwrap());
    // Offsets that don't fit the labels.
    let bad = with(ascii_mesh(), "faces", format!("{}{}{}", h("faceCompactList"), list(&offsets), list(&labels[1..])));
    assert!(read_boundary_mesh(&bad).is_err());
}

#[test]
fn corrupt_compressed_faces_are_errors() {
    let mut fs = faces_gz(&long_mesh(1000));
    let mut bytes = fs.read("constant/air/polyMesh/faces.gz", 1 << 30).unwrap().unwrap();
    let n = bytes.len();
    for i in (n / 3..n / 3 + 64).step_by(7) {
        bytes[i] ^= 0x5a;
    }
    fs.insert("constant/air/polyMesh/faces.gz", bytes);
    assert!(read_boundary_mesh(&fs).is_err());
}

#[test]
fn a_boundary_from_another_mesh_is_an_error() {
    let d = "constant/air/polyMesh";
    // The patches start at face 0, the owner header notes 1 internal face.
    let stale = BOUNDARY
        .replace("nFaces          1;\n        startFace       1;", "nFaces          2;\n        startFace       0;");
    let fs = with(ascii_mesh(), "boundary", stale);
    let e = read_boundary_mesh(&fs).unwrap_err().to_string();
    assert!(e.contains("internal faces") && e.contains("another mesh"), "{e}");
    assert!(read(&fs).is_err());
    // The same with a compressed owner, read only as far as its header.
    let mut gz = fs.clone();
    let owner = fs.read(&format!("{d}/owner"), 1 << 20).unwrap().unwrap();
    gz.insert(&format!("{d}/owner.gz"), gzip(&owner));
    let mut only_gz = MemFs::new();
    for name in ["points", "faces", "boundary"] {
        only_gz.insert(&format!("{d}/{name}"), gz.read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap());
    }
    only_gz.insert(&format!("{d}/owner.gz"), gzip(&owner));
    assert!(read_boundary_mesh(&only_gz).unwrap_err().to_string().contains("another mesh"));
    // A faces file with other faces than the owner notes.
    let h = header("ascii", "faceList", None, "");
    let more = with(ascii_mesh(), "faces", format!("{h}12\n(\n{}\n4(0 1 2 3)\n)\n", faces_text().join("\n")));
    assert!(read_boundary_mesh(&more).unwrap_err().to_string().contains("owner header says 11 faces"));
    // No note (an owner written without one): nothing to check against.
    let plain = with(
        ascii_mesh(),
        "owner",
        format!("{}11\n(\n0 0 1 0 1 0 1 0 1 0 1\n)\n", header("ascii", "labelList", None, "")),
    );
    assert!(read_boundary_mesh(&plain).is_ok());
}

#[test]
fn a_compressed_owner_header_that_cant_be_read_is_an_error() {
    let d = "constant/air/polyMesh";
    let owner = ascii_mesh().read(&format!("{d}/owner"), 1 << 20).unwrap().unwrap();
    let mesh = |gz: Vec<u8>| {
        let mut fs = MemFs::new();
        for name in ["points", "faces", "boundary"] {
            fs.insert(&format!("{d}/{name}"), ascii_mesh().read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap());
        }
        fs.insert(&format!("{d}/owner.gz"), gz);
        fs
    };
    let mut gz = gzip(&owner);
    assert!(read_boundary_mesh(&mesh(gz.clone())).is_ok());
    // Cut inside the header (after `FoamFile {` began, before it ends): an error, not a mesh
    // without counts.
    let at = owner.windows(8).position(|w| w == b"FoamFile").unwrap() + 20;
    let cut = gzip(&owner[..at]);
    assert!(read_boundary_mesh(&mesh(cut[..cut.len() - 4].to_vec())).unwrap_err().to_string().contains("owner.gz"));
    // A bad checksum: an error too (deflate itself has none, so what came before may be wrong).
    let n = gz.len();
    gz[n - 6] ^= 0xff;
    assert!(read_boundary_mesh(&mesh(gz)).unwrap_err().to_string().contains("owner.gz"));
}

#[test]
fn an_owner_header_cut_short_is_an_error() {
    let d = "constant/air/polyMesh";
    let owner = ascii_mesh().read(&format!("{d}/owner"), 1 << 20).unwrap().unwrap();
    // The text cut inside `FoamFile { … }`: plain, and as a valid gzip file of that text.
    let at = owner.windows(8).position(|w| w == b"FoamFile").unwrap() + 20;
    let plain = with(ascii_mesh(), "owner", String::from_utf8(owner[..at].to_vec()).unwrap());
    let e = read_boundary_mesh(&plain).unwrap_err().to_string();
    assert!(e.contains("owner: its header can't be read"), "{e}");
    let mut gz = MemFs::new();
    for name in ["points", "faces", "boundary"] {
        gz.insert(&format!("{d}/{name}"), ascii_mesh().read(&format!("{d}/{name}"), 1 << 20).unwrap().unwrap());
    }
    gz.insert(&format!("{d}/owner.gz"), gzip(&owner[..at]));
    let e = read_boundary_mesh(&gz).unwrap_err().to_string();
    assert!(e.contains("owner.gz: its header can't be read"), "{e}");
    // An owner of plain data, without a header: nothing to check against.
    let data = with(ascii_mesh(), "owner", "11\n(\n0 0 1 0 1 0 1 0 1 0 1\n)\n".to_string());
    assert!(read_boundary_mesh(&data).is_ok());
}
