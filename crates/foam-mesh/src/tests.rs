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
    fs.insert(&format!("{d}/points"), "garbage");
    assert!(read(&fs).is_err());
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
    }
}
