use crate::check::{MAX_LISTED, analyse};
use crate::repair::{drop_degenerate, orient, scale, weld};
use crate::stl::{Format, Limits, Stl, StlError, Triangle, read};
use crate::{BBox, Vec3};

/// A unit cube, normals outward, as one ASCII solid.
fn cube() -> Stl {
    let p = |i: usize| {
        let b = |k: usize| if (i >> k) & 1 == 1 { 1.0f32 } else { 0.0 };
        [b(0), b(1), b(2)]
    };
    // Quads wound counter-clockwise seen from outside.
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut triangles = Vec::new();
    for q in quads {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            triangles.push(Triangle { vertices: t.map(p), normal: [0.0; 3], solid: 0 });
        }
    }
    Stl {
        format: Format::Ascii,
        solids: vec!["box".to_string()],
        triangles,
        notes: Vec::new(),
        openfoam: None,
        header_solid: false,
    }
}

fn reread(stl: &Stl) -> Stl {
    read(&stl.to_bytes(), &Limits::default()).unwrap()
}

#[test]
fn a_closed_cube_is_clean() {
    let r = analyse(&cube());
    assert_eq!((r.triangles, r.points, r.region_count, r.parts, r.closed_parts), (12, 8, 1, 1, 1));
    assert_eq!(r.regions, vec!["box"]);
    assert!(r.is_clean_closed(), "{r:?}");
    assert_eq!(r.bbox, Some(BBox { min: Vec3::ZERO, max: Vec3::new(1.0, 1.0, 1.0) }));
}

#[test]
fn ascii_and_binary_round_trip() {
    let mut c = cube();
    let back = reread(&c);
    assert_eq!(back.format, Format::Ascii);
    assert_eq!(back.solids, vec!["box"]);
    assert_eq!(
        back.triangles.iter().map(|t| t.vertices).collect::<Vec<_>>(),
        c.triangles.iter().map(|t| t.vertices).collect::<Vec<_>>()
    );
    // Normals are written from the winding.
    assert_eq!(back.triangles[0].normal, [0.0, 0.0, -1.0]);

    c.format = Format::Binary;
    c.solids.clear();
    c.triangles[3].solid = 7;
    let bytes = c.to_bytes();
    assert_eq!(bytes.len(), 84 + 50 * 12);
    let back = read(&bytes, &Limits::default()).unwrap();
    assert_eq!(back.format, Format::Binary);
    assert_eq!(back.triangles[3].solid, 7);
    let r = analyse(&back);
    assert_eq!(
        (r.regions.len(), r.region_count, r.empty_regions),
        (8, 8, 6),
        "patch0 to patch7, as OpenFOAM makes them"
    );
    assert!(analyse(&back).is_clean_closed());
}

#[test]
fn a_binary_header_starting_with_solid_is_still_binary() {
    let mut c = cube();
    c.format = Format::Binary;
    let mut bytes = c.to_bytes();
    assert_eq!(read(&bytes, &Limits::default()).unwrap().openfoam, None);
    bytes[..11].copy_from_slice(b"solid cube ");
    let back = read(&bytes, &Limits::default()).unwrap();
    assert_eq!((back.format, back.triangles.len()), (Format::Binary, 12));
    // OpenFOAM reads a `.stl` like that as ASCII and fails; written again, it reads.
    assert!(back.header_solid);
    assert!(!read(&back.to_bytes(), &Limits::default()).unwrap().header_solid);

    // With a `solid` header and bytes after the triangles, or a count of 0, still binary.
    let mut odd = bytes.clone();
    odd.extend_from_slice(b"\n\n");
    let back = read(&odd, &Limits::default()).unwrap();
    assert_eq!((back.format, back.triangles.len(), back.header_solid), (Format::Binary, 12, true));
    odd.truncate(bytes.len());
    odd[80..84].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(read(&odd, &Limits::default()).unwrap().triangles.len(), 12);

    // Bytes after the last triangle: read, with a note.
    bytes.extend_from_slice(b"junk");
    bytes[..5].copy_from_slice(b"xolid");
    let back = read(&bytes, &Limits::default()).unwrap();
    assert_eq!(back.triangles.len(), 12);
    assert!(back.notes[0].contains("4 bytes"), "{:?}", back.notes);
    assert_eq!(back.openfoam, None, "OpenFOAM accepts a few bytes too many");

    // A count of zero with triangles after it (some exporters).
    bytes.truncate(84 + 50 * 12);
    bytes[80..84].copy_from_slice(&0u32.to_le_bytes());
    let back = read(&bytes, &Limits::default()).unwrap();
    assert_eq!(back.triangles.len(), 12);
    assert!(back.notes[0].contains("holds 12"), "{:?}", back.notes);
    assert!(back.openfoam.as_deref().unwrap().contains("announces 0 triangles"));
}

#[test]
fn broken_files_are_errors_not_panics() {
    let l = Limits::default();
    assert_eq!(read(b"", &l), Err(StlError::NotStl));
    assert_eq!(read(b"hello", &l), Err(StlError::NotStl));
    let mut bytes = vec![0u8; 84 + 50];
    bytes[80..84].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(read(&bytes, &l), Err(StlError::Truncated { announced: 3, held: 1 }));
    bytes[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read(&bytes, &l).is_err());
    let small = Limits { max_triangles: 5, max_solids: 1, max_bytes: 100 };
    assert_eq!(read(&cube().to_bytes(), &small), Err(StlError::TooManyTriangles(5)));
    assert_eq!(read(b"solid a\nendsolid a\nsolid b\nendsolid b\n", &small), Err(StlError::TooManySolids(1)));
    // Every prefix of a valid file reads or fails cleanly.
    let text = cube().to_bytes();
    for k in (0..text.len()).step_by(7) {
        let _ = read(&text[..k], &l);
    }
}

#[test]
fn odd_ascii_is_read_with_notes() {
    let text = "  SOLID Building 1\n\
        facet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 0 0\n vertex 0 1 0\n endloop\n endfacet\n\
        facet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 0 0\n endloop\n endfacet\n\
        facet normal 0 0 1\n outer loop\n vertex 0 0 0\n vertex 1 x 0\n vertex 0 1 0\n endloop\n endfacet\n\
        colour red\n\
        endsolid Building 1\n\
        facet normal 0 0 1\n outer loop\n vertex 0 0 1e40\n vertex 1 0 0\n vertex 0 1 0\n endloop\n endfacet\n\
        solid Roof\n facet normal 0 0 1\n outer loop\n vertex 0 0 0\n";
    let s = read(text.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::Ascii);
    assert!(s.openfoam.is_some());
    assert_eq!(s.solids, vec!["Building 1", "", "Roof"]);
    assert_eq!(s.triangles.len(), 2);
    assert_eq!(s.triangles[1].solid, 1);
    assert!(s.notes[0].starts_with("3 facet(s) skipped"), "{:?}", s.notes);
    assert!(s.notes[1].starts_with("1 line(s)"), "{:?}", s.notes);
    let r = analyse(&s);
    assert_eq!(r.non_finite.triangles, vec![1]);
    assert_eq!(r.regions, vec!["Building1", "solid", "Roof"]);
    assert_eq!(r.empty_regions, 1);
    assert_eq!(r.renamed, vec![("Building 1".to_string(), "Building1".to_string())]);
    let long = format!("solid {}\nendsolid\n", "n".repeat(1000));
    let s = read(long.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(s.solids[0].len(), crate::stl::MAX_NAME);
    assert!(s.notes[0].contains("cut short"));
}

#[test]
fn holes_are_open_edges() {
    let mut c = cube();
    c.triangles.remove(5);
    let r = analyse(&c);
    assert_eq!((r.open_edges, r.open.count, r.closed_parts, r.inward_parts), (3, 3, 0, 0));
    assert!(!r.is_clean_closed());
}

#[test]
fn a_flipped_triangle_is_inconsistent_and_orient_fixes_it() {
    let mut c = cube();
    c.triangles[4].flip();
    let r = analyse(&c);
    assert_eq!((r.inconsistent_edges, r.inconsistent.triangles.len()), (3, 4));
    assert_eq!(r.inward_parts, 0, "an inconsistent part has no inside");
    let o = orient(&mut c);
    assert_eq!((o.flipped, o.outward, o.conflicts), (1, 0, 0));
    assert!(analyse(&c).is_clean_closed());
}

#[test]
fn an_inside_out_cube_is_inward_and_orient_turns_it() {
    let mut c = cube();
    for t in &mut c.triangles {
        t.flip();
    }
    let r = analyse(&c);
    assert_eq!((r.inward_parts, r.inward.count, r.inconsistent_edges), (1, 12, 0));
    let o = orient(&mut c);
    assert_eq!((o.flipped, o.outward), (12, 1));
    assert!(analyse(&c).is_clean_closed());
    // Far from the origin (projected coordinates) too.
    let mut far = cube();
    for t in &mut far.triangles {
        for v in &mut t.vertices {
            v[0] += 2_600_000.0;
            v[1] += 1_200_000.0;
        }
        t.flip();
    }
    assert_eq!(orient(&mut far).outward, 1);
}

#[test]
fn an_open_sheet_keeps_the_side_most_of_it_faces() {
    // A strip of four triangles in the plane z = 0, all facing up but the first.
    let mut s = cube();
    s.triangles.clear();
    for i in 0..2 {
        let x = i as f32;
        s.triangles.push(Triangle {
            vertices: [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x + 1.0, 1.0, 0.0]],
            normal: [0.0; 3],
            solid: 0,
        });
        s.triangles.push(Triangle {
            vertices: [[x, 0.0, 0.0], [x + 1.0, 1.0, 0.0], [x, 1.0, 0.0]],
            normal: [0.0; 3],
            solid: 0,
        });
    }
    s.triangles[0].flip();
    let o = orient(&mut s);
    assert_eq!((o.flipped, o.outward), (1, 0));
    assert!(s.triangles.iter().all(|t| t.cross().z > 0.0));
    let r = analyse(&s);
    assert_eq!((r.inconsistent_edges, r.open_edges, r.parts, r.closed_parts), (0, 6, 1, 0));
}

#[test]
fn degenerate_and_duplicate_triangles_are_found_and_dropped() {
    let mut c = cube();
    let first = c.triangles[0];
    c.triangles.push(first);
    let mut twin = first;
    twin.flip();
    c.triangles.push(twin);
    c.triangles.push(Triangle { vertices: [[0.0; 3], [0.5, 0.0, 0.0], [1.0, 0.0, 0.0]], normal: [0.0; 3], solid: 0 });
    c.triangles.push(Triangle { vertices: [[0.0; 3], [0.0; 3], [1.0, 0.0, 0.0]], normal: [0.0; 3], solid: 0 });
    c.triangles.push(Triangle { vertices: [[f32::NAN; 3], [0.0; 3], [1.0, 0.0, 0.0]], normal: [0.0; 3], solid: 0 });
    let r = analyse(&c);
    assert_eq!(r.duplicate.triangles, vec![12, 13]);
    assert_eq!(r.degenerate.triangles, vec![14, 15]);
    assert_eq!(r.non_finite.triangles, vec![16]);
    let d = drop_degenerate(&mut c);
    // The in-line triangle is a fin on the cube's edge (0,0,0)-(1,0,0), a third triangle there
    // with its short edges open: removing it opens nothing.
    assert_eq!((d.non_finite, d.duplicate, d.degenerate, d.split, d.left), (1, 2, 2, 0, 0));
    assert_eq!(c.triangles.len(), 12);
    assert!(analyse(&c).is_clean_closed());
    assert!(analyse(&c).is_clean_closed());
}

#[test]
fn stored_normals_against_the_winding_are_listed() {
    let mut c = cube();
    c.triangles[2].normal = [0.0, 0.0, -1.0];
    c.triangles[3].normal = [0.0, -1.0, 0.0];
    let r = analyse(&c);
    assert_eq!(r.stored_normals.triangles.len(), 1);
}

#[test]
fn weld_closes_small_gaps() {
    let mut c = cube();
    // One corner written slightly differently in one triangle: two open edges' worth of gap.
    for v in &mut c.triangles[11].vertices {
        if *v == [1.0, 1.0, 1.0] {
            *v = [1.0 + 1e-5, 1.0, 1.0 - 1e-5];
        }
    }
    let r = analyse(&c);
    assert!(r.open_edges > 0);
    assert_eq!(weld(&mut c, 1e-9), Default::default());
    let w = weld(&mut c, 1e-4);
    assert_eq!((w.moved, w.removed), (1, 0));
    assert!(analyse(&c).is_clean_closed(), "{:?}", analyse(&c));
    // Collapsing triangles are removed.
    let mut s = cube();
    s.triangles.push(Triangle { vertices: [[0.0; 3], [0.0, 1e-6, 0.0], [1.0, 0.0, 0.0]], normal: [0.0; 3], solid: 0 });
    assert_eq!(weld(&mut s, 1e-3).removed, 1);
    assert_eq!(weld(&mut s, f64::NAN), Default::default());
    assert_eq!(weld(&mut s, -1.0), Default::default());
}

#[test]
fn scale_multiplies_and_refuses_mirroring() {
    let mut c = cube();
    assert!(scale(&mut c, 0.001));
    assert_eq!(c.bbox().unwrap().max, Vec3::from_f32([0.001; 3]));
    assert!(!scale(&mut c, -1.0));
    assert!(!scale(&mut c, 0.0));
    assert!(!scale(&mut c, f64::INFINITY));
}

#[test]
fn listed_triangles_are_capped_counts_are_not() {
    let mut s = cube();
    s.triangles.clear();
    for i in 0..(MAX_LISTED + 10) {
        let x = 2.0 * i as f32;
        s.triangles.push(Triangle {
            vertices: [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]],
            normal: [0.0; 3],
            solid: 0,
        });
    }
    let r = analyse(&s);
    assert_eq!((r.open.count, r.open.triangles.len(), r.parts), (MAX_LISTED + 10, MAX_LISTED, MAX_LISTED + 10));
}

#[test]
fn non_manifold_edges() {
    let mut c = cube();
    // A fin on the edge (0,0,0)-(1,0,0).
    c.triangles.push(Triangle { vertices: [[0.0; 3], [1.0, 0.0, 0.0], [0.5, -1.0, 0.5]], normal: [0.0; 3], solid: 0 });
    let r = analyse(&c);
    assert_eq!((r.non_manifold_edges, r.non_manifold.count, r.open_edges), (1, 3, 2));
    assert_eq!(r.closed_parts, 0);
}

#[test]
fn gzip_files_are_read_within_the_limit() {
    use std::io::Write;
    let text = cube().to_bytes();
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&text).unwrap();
    let gz = z.finish().unwrap();
    assert_eq!(read(&gz, &Limits::default()).unwrap().triangles.len(), 12);
    let small = Limits { max_bytes: 100, ..Limits::default() };
    assert_eq!(read(&gz, &small), Err(StlError::TooLarge(100)));
    assert!(matches!(read(&gz[..gz.len() / 2], &Limits::default()), Err(StlError::Gzip(_))));
}

#[test]
fn ascii_color_lines_are_stl_and_inf_is_not() {
    let text = "solid a\ncolor 1 0 0\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a\n";
    let s = read(text.as_bytes(), &Limits::default()).unwrap();
    assert_eq!((s.triangles.len(), s.openfoam.as_deref()), (1, None));
    let s = read(text.replace("vertex 1 0 0", "vertex inf 0 0").as_bytes(), &Limits::default()).unwrap();
    assert_eq!(s.triangles.len(), 0);
    assert!(s.openfoam.is_some());
}

#[test]
fn vertices_near_zero_join_as_openfoam_joins_them() {
    // A CAD exporter's cos(90°): 6e-17 where 0 is meant (OpenFOAM's mixerVessel-sparger).
    let mut c = cube();
    for t in &mut c.triangles[..2] {
        for v in &mut t.vertices {
            if v[0] == 0.0 {
                v[0] = 6.1e-17;
            }
        }
    }
    let r = analyse(&c);
    assert_eq!((r.points, r.open_edges), (8, 0));
    assert!(r.is_clean_closed());
    // Further apart than OpenFOAM joins: open.
    for v in &mut c.triangles[0].vertices {
        if v[0] != 1.0 {
            v[0] = 1e-12;
        }
    }
    assert!(analyse(&c).open_edges > 0);
}

#[test]
fn foam_names() {
    use crate::stl::foam_name;
    assert_eq!(foam_name("Building 1"), "Building1");
    assert_eq!(foam_name(" \"a/b\";{}$ "), "ab");
    assert_eq!(foam_name(""), "solid");
}

#[test]
fn quality_is_openfoams() {
    use crate::check::{is_degenerate, quality};
    // Triangles of real surfaces, with the quality OpenFOAM-12's surfaceCheck prints for them.
    let t = |bits: [u32; 9]| {
        let f = bits.map(f32::from_bits);
        Triangle { vertices: [[f[0], f[1], f[2]], [f[3], f[4], f[5]], [f[6], f[7], f[8]]], normal: [0.0; 3], solid: 0 }
    };
    let cases = [
        // A tree surface 2 km from the origin: its worst triangle, 1.53181e-15.
        (
            [
                0x4315a840, 0xc4d78708, 0x43bfbf5a, 0x43159b55, 0xc4d777d6, 0x43bfa969, 0x431566d9, 0xc4d73a1b,
                0x43bf5045,
            ],
            1.53181e-15,
        ),
        // Walls of the same project: 2.1443e-12.
        (
            [
                0x438e9d97, 0xc5133cd3, 0x43bfa2df, 0x438772c0, 0xc5140332, 0x43bfa312, 0x4394c8e5, 0xc5129214,
                0x43bfa312,
            ],
            2.1443e-12,
        ),
        // The 3×3 case's tree crowns: min 0.670084, max 0.933615.
        (
            [
                0x43870000, 0x437cc1e2, 0x40699bab, 0x43870000, 0x437f3e1e, 0x40699bab, 0x43870000, 0x437f3e1e,
                0x40a3322a,
            ],
            0.670084,
        ),
        (
            [
                0x43891f0f, 0x437cc1e2, 0x40c40000, 0x43891f0f, 0x437b0000, 0x40a3322a, 0x438a0000, 0x437cc1e2,
                0x40a3322a,
            ],
            0.933615,
        ),
    ];
    for (bits, expected) in cases {
        let q = quality(&t(bits));
        assert!((q / expected - 1.0).abs() < 1e-4, "{q} vs {expected}");
        assert!(!is_degenerate(&t(bits)), "above OpenFOAM's 1e-15");
    }
    // Fine triangles far from the origin are not degenerate.
    let far = Triangle {
        vertices: [[2_600_000.0, 0.0, 0.0], [2_600_000.5, 0.0, 0.0], [2_600_000.0, 0.5, 0.0]],
        normal: [0.0; 3],
        solid: 0,
    };
    assert!(!is_degenerate(&far));
    let line = Triangle { vertices: [[0.0; 3], [1.0, 0.0, 0.0], [3.0, 0.0, 0.0]], normal: [0.0; 3], solid: 0 };
    assert_eq!(quality(&line), 0.0);
    assert!(is_degenerate(&line));
}

#[test]
fn a_cap_is_removed_by_splitting_its_neighbour() {
    // Two triangles sharing the edge (0,0,0)-(2,0,0) form a square; a zero-area cap on that
    // edge, at (1,0,0), sits between the lower triangle and its neighbours. Built as a closed
    // fan around the square's border: a flat sheet with the cap in a T-junction.
    let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| Triangle { vertices: [a, b, c], normal: [0.0; 3], solid: 0 };
    let (o, m, e, top, low) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0]);
    let mut s = cube();
    // Above the edge: one triangle o-e-top. Below: two, o-low-m and m-low-e, meeting at m.
    // The cap o-m-e closes the T-junction at m.
    s.triangles = vec![tri(o, e, top), tri(o, low, m), tri(m, low, e), tri(o, m, e)];
    let before = analyse(&s);
    assert_eq!(before.degenerate.count, 1);
    let open_before = before.open_edges;
    let d = drop_degenerate(&mut s);
    assert_eq!((d.split, d.degenerate, d.left), (1, 0, 0));
    let after = analyse(&s);
    assert_eq!((after.degenerate.count, after.open_edges, after.inconsistent_edges), (0, open_before, 0));
    assert_eq!(s.triangles.len(), 4, "the cap is gone, the triangle above is two halves");
    assert!(s.triangles.iter().all(|t| t.cross().z > 0.0), "windings kept");
}

#[test]
fn ascii_openfoam_rejects_is_flagged() {
    let base = "solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a\n";
    assert_eq!(read(base.as_bytes(), &Limits::default()).unwrap().openfoam, None);
    for bad in [
        base.replace("normal 0 0 1", "normal nan nan nan"),
        base.replace("solid a\n", "Solid a\n"),
        base.replace("vertex 0 1 0", "vertex 0 1 0 7"),
        base.replace("solid a\n", "solid caf\u{e9}\n"),
    ] {
        let s = read(bad.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(s.triangles.len(), 1, "{bad}");
        assert!(s.openfoam.is_some(), "{bad}");
    }
}

#[test]
fn writing_leaves_out_what_no_reader_takes() {
    let mut c = cube();
    c.triangles[0].vertices[0][0] = f32::INFINITY;
    c.solids[0] = "caf\u{e9} 1".to_string();
    c.solids.push("empty".to_string());
    let back = reread(&c);
    assert_eq!((back.triangles.len(), back.openfoam.as_deref()), (11, None));
    assert_eq!(back.solids, vec!["caf1", "empty"]);
    c.format = Format::Binary;
    assert_eq!(reread(&c).triangles.len(), 11);
    // Scaling past what an f32 holds is refused.
    let mut c = cube();
    assert!(!scale(&mut c, 1e39));
    assert_eq!(c, cube());
}

#[test]
fn weld_moves_every_copy_of_a_vertex_together() {
    // v at the origin, used by two triangles that close on each other; q1 and w on either
    // side, within the tolerance of v but not of each other.
    let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| Triangle { vertices: [a, b, c], normal: [0.0; 3], solid: 0 };
    let (q1, v, w) = ([0.9, 0.0, 0.0], [0.0, 0.0, 0.0], [-0.5, 0.0, 0.0]);
    let (y, z) = ([0.0, 5.0, 0.0], [0.0, 0.0, 5.0]);
    let mut s = cube();
    s.triangles = vec![
        tri(q1, [5.0, 5.0, 0.0], [5.0, 0.0, 5.0]),
        tri(v, y, z),
        tri(w, [-5.0, 5.0, 0.0], [-5.0, 0.0, 5.0]),
        tri(v, z, y),
    ];
    let open = analyse(&s).open_edges;
    weld(&mut s, 1.0);
    assert_eq!(s.triangles[1].vertices[0], s.triangles[3].vertices[0]);
    assert!(analyse(&s).open_edges <= open);
}

#[test]
fn a_truncated_binary_with_a_solid_header_is_an_error() {
    let mut c = cube();
    c.format = Format::Binary;
    let mut bytes = c.to_bytes();
    bytes[..6].copy_from_slice(b"solid ");
    bytes.truncate(bytes.len() - 10);
    assert_eq!(read(&bytes, &Limits::default()), Err(StlError::Truncated { announced: 12, held: 11 }));
    // An ASCII file with a byte that isn't text is still ASCII.
    let text = String::from_utf8(cube().to_bytes()).unwrap().replacen("solid box", "solid caf\u{e9}", 1);
    let s = read(text.as_bytes(), &Limits::default()).unwrap();
    assert_eq!((s.format, s.triangles.len()), (Format::Ascii, 12));
}

#[test]
fn ascii_with_a_non_ascii_byte_where_a_count_would_be() {
    let mut name = "x".repeat(74);
    name.push_str("Zürich_Gebäude");
    let text = String::from_utf8(cube().to_bytes()).unwrap().replacen("solid box", &format!("solid {name}"), 1);
    assert!(!text.as_bytes()[80..84].is_ascii());
    let s = read(text.as_bytes(), &Limits::default()).unwrap();
    assert_eq!((s.format, s.triangles.len()), (Format::Ascii, 12));
    assert!(s.openfoam.is_some(), "the name isn't ASCII");
}

#[test]
fn empty_solids_keep_their_place() {
    let mut c = cube();
    c.solids = vec!["empty".to_string(), "box".to_string()];
    for t in &mut c.triangles {
        t.solid = 1;
    }
    c.triangles[0].solid = 9;
    let back = reread(&c);
    assert_eq!(back.solids, vec!["empty", "box", "solid"]);
    assert_eq!(back.triangles.iter().filter(|t| t.solid == 1).count(), 11);
}

/// A pseudo-random number in [0, 1), from a 64-bit state (xorshift).
fn rand(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 40) as f32 / (1u64 << 24) as f32
}

/// The BVH finds what testing every triangle finds, nearest first, and skips triangles it
/// can't hit.
#[test]
fn bvh_casts_match_testing_every_triangle() {
    use crate::bvh::{Bvh, Ray};
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut points = Vec::new();
    let mut tris = Vec::new();
    for i in 0..3000u32 {
        let c = [rand(&mut s) * 10.0, rand(&mut s) * 10.0, rand(&mut s) * 10.0];
        for _ in 0..3 {
            points.push([c[0] + rand(&mut s) - 0.5, c[1] + rand(&mut s) - 0.5, c[2] + rand(&mut s) - 0.5]);
        }
        tris.push([3 * i, 3 * i + 1, 3 * i + 2]);
    }
    // One triangle not finite, one with an index out of range: never hit, never a panic.
    points.push([f32::NAN, 0.0, 0.0]);
    let n = points.len() as u32;
    tris.push([n - 1, 0, 1]);
    tris.push([0, 1, n + 5]);
    let bvh = Bvh::new(&points, &tris);
    assert_eq!(bvh.len(), 3000);
    for _ in 0..200 {
        let o = [rand(&mut s) as f64 * 14.0 - 2.0, rand(&mut s) as f64 * 14.0 - 2.0, -5.0];
        let d = [rand(&mut s) as f64 - 0.5, rand(&mut s) as f64 - 0.5, 1.0];
        let ray = Ray { origin: o, direction: d };
        let mut brute: Vec<(f64, u32)> = Vec::new();
        // The triangle test alone, without any box test.
        for k in 0..tris.len() as u32 {
            if let Some(t) = crate::bvh::hit(&points, &tris, &ray, k) {
                brute.push((t, k));
            }
        }
        brute.sort_by(|a, b| a.0.total_cmp(&b.0));
        let all = bvh.cast_all(&points, &tris, &ray, usize::MAX);
        assert_eq!(all.iter().map(|h| h.triangle).collect::<Vec<_>>(), brute.iter().map(|b| b.1).collect::<Vec<_>>());
        let first = bvh.cast(&points, &tris, &ray);
        assert_eq!(first.map(|h| h.triangle), brute.first().map(|b| b.1));
        let two = bvh.cast_all(&points, &tris, &ray, 2);
        assert_eq!(two.len(), brute.len().min(2));
    }
    // Rays along an axis, and from inside the boxes.
    let flat = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let one = Bvh::new(&flat, &[[0, 1, 2]]);
    let down = Ray { origin: [0.25, 0.25, 3.0], direction: [0.0, 0.0, -1.0] };
    assert_eq!(one.cast(&flat, &[[0, 1, 2]], &down).map(|h| h.t), Some(3.0));
    let away = Ray { origin: [0.25, 0.25, 3.0], direction: [0.0, 0.0, 1.0] };
    assert!(one.cast(&flat, &[[0, 1, 2]], &away).is_none());
    let edge_on = Ray { origin: [-1.0, 0.25, 0.0], direction: [1.0, 0.0, 0.0] };
    assert!(one.cast(&flat, &[[0, 1, 2]], &edge_on).is_none());
    assert!(Bvh::new(&[], &[]).cast(&[], &[], &down).is_none());
    // Many triangles with the same centroid still build (and halve by position).
    let same: Vec<[u32; 3]> = (0..100).map(|_| [0, 1, 2]).collect();
    let b = Bvh::new(&flat, &same);
    assert_eq!(b.cast_all(&flat, &same, &down, 1000).len(), 100);
}

/// Regions as OpenFOAM names them: solids joined by name, binary attribute values present.
#[test]
fn regions_follow_openfoams_names() {
    let mut s = cube();
    s.solids = vec!["a b".to_string(), "ab".to_string(), "c".to_string()];
    for (k, t) in s.triangles.iter_mut().enumerate() {
        t.solid = [0, 1, 2, 7][k % 4];
    }
    let (names, of) = s.regions();
    assert_eq!(names, ["ab", "c", "solid"]);
    assert_eq!(&of[..4], &[0, 0, 1, 2]);
    s.format = Format::Binary;
    let (names, of) = s.regions();
    assert_eq!(names, ["patch0", "patch1", "patch2", "patch7"]);
    assert_eq!(&of[..4], &[0, 1, 2, 3]);
}

/// Rays through the edges and corners of flat, axis-aligned faces (what a mesh's boundary is
/// made of) at coordinates that are not round in binary: the BVH finds what testing every
/// triangle finds, including the triangles on both sides of the edge.
#[test]
fn bvh_finds_hits_on_the_edges_of_flat_boxes() {
    use crate::bvh::{Bvh, Ray};
    let (x0, step) = (-45.3f32, 0.7f32);
    let mut points = Vec::new();
    let mut tris = Vec::new();
    // A wall in the plane x = x0 and a floor in z = 0.1, each a grid of quads split in two.
    for wall in [true, false] {
        for i in 0..20u32 {
            for j in 0..20u32 {
                let base = points.len() as u32;
                for (a, b) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                    let (u, v) = (x0 + (i + a) as f32 * step, 0.1 + (j + b) as f32 * step);
                    points.push(if wall { [x0, u, v] } else { [u, v - 0.1 + x0, 0.1] });
                }
                tris.push([base, base + 1, base + 2]);
                tris.push([base, base + 2, base + 3]);
            }
        }
    }
    let bvh = Bvh::new(&points, &tris);
    let mut s = 0x1234_5678_9abc_def1u64;
    let mut checked = 0;
    for target in points.iter().step_by(3) {
        let o = [
            f64::from(target[0]) + 50.0 * (f64::from(rand(&mut s)) + 0.2),
            f64::from(target[1]) - 37.0 * f64::from(rand(&mut s)),
            f64::from(target[2]) + 61.0 * f64::from(rand(&mut s)),
        ];
        let d = [f64::from(target[0]) - o[0], f64::from(target[1]) - o[1], f64::from(target[2]) - o[2]];
        let ray = Ray { origin: o, direction: d };
        let mut brute: Vec<(f64, u32)> =
            (0..tris.len() as u32).filter_map(|k| crate::bvh::hit(&points, &tris, &ray, k).map(|t| (t, k))).collect();
        brute.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut all: Vec<(f64, u32)> =
            bvh.cast_all(&points, &tris, &ray, usize::MAX).iter().map(|h| (h.t, h.triangle)).collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        assert_eq!(all, brute, "ray {ray:?}");
        let first = bvh.cast(&points, &tris, &ray).map(|h| h.t);
        assert!(first.zip(brute.first()).map_or(first.is_none() && brute.is_empty(), |(f, b)| (f - b.0).abs() < 1e-12));
        checked += usize::from(!brute.is_empty());
    }
    assert!(checked > 200, "{checked}");
}

#[test]
fn a_point_is_inside_a_closed_surface_by_the_crossings() {
    use crate::check::contains;
    // A cube from 0 to 1, as twelve triangles.
    let v = |x: f32, y: f32, z: f32| [x, y, z];
    let quads = [
        [v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.), v(1., 0., 0.)],
        [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
        [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
        [v(0., 1., 0.), v(0., 1., 1.), v(1., 1., 1.), v(1., 1., 0.)],
        [v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.)],
        [v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.), v(1., 0., 1.)],
    ];
    let mut triangles = Vec::new();
    for q in quads {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            triangles.push(crate::stl::Triangle { vertices: t, normal: [0.0; 3], solid: 0 });
        }
    }
    let cube = crate::stl::Stl {
        format: crate::stl::Format::Binary,
        solids: Vec::new(),
        triangles,
        notes: Vec::new(),
        openfoam: None,
        header_solid: false,
    };
    assert_eq!(contains(&cube, [0.5, 0.5, 0.5]), Some(true));
    // On a diagonal of every face's triangles: the first rays pass edges.
    assert_eq!(contains(&cube, [0.25, 0.25, 0.25]), Some(true));
    assert_eq!(contains(&cube, [2.0, 0.5, 0.5]), Some(false));
    assert_eq!(contains(&cube, [-3.0, -3.0, -3.0]), Some(false));
    assert_eq!(contains(&cube, [0.5, 0.5, 1.0]), None, "on the surface");
    assert_eq!(contains(&cube, [f64::NAN, 0.0, 0.0]), None);
}
