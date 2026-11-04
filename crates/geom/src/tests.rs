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
    Stl { format: Format::Ascii, solids: vec!["box".to_string()], triangles, notes: Vec::new(), openfoam: None }
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
    assert_eq!(analyse(&back).regions, vec!["patch0", "patch7"]);
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
    // OpenFOAM reads it as ASCII and fails; written again, it reads.
    assert!(back.openfoam.as_deref().unwrap().contains("starts with `solid`"));
    assert_eq!(read(&back.to_bytes(), &Limits::default()).unwrap().openfoam, None);

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
    assert_eq!(r.regions, vec!["Building1", "solid"]);
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
fn needles_as_thin_as_the_coordinates_are_degenerate() {
    // From a real tree surface: about one f32 step high, 2 km from the origin.
    let t = Triangle {
        vertices: [
            [0x3fe5989d, 0xc5024dea, 0x43c19c6a],
            [0x3fe5989f, 0xc5024deb, 0x43c1a3d4],
            [0x3fe5989f, 0xc5024deb, 0x43c2da88],
        ]
        .map(|p| p.map(f32::from_bits)),
        normal: [0.0; 3],
        solid: 0,
    };
    assert!(crate::check::is_degenerate(&t));
    let mut fine = t;
    fine.vertices[1][0] += 0.01;
    assert!(!crate::check::is_degenerate(&fine));
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
