use umcf_geom::check::analyse;
use umcf_geom::stl::{Limits, read};

use crate::Level;
use crate::surface::diagnostics;

fn codes(text: &[u8]) -> Vec<(Level, &'static str, String)> {
    let stl = read(text, &Limits::default()).unwrap();
    let r = analyse(&stl);
    diagnostics("constant/triSurface/x.stl", &stl, &r).into_iter().map(|d| (d.level, d.code, d.message)).collect()
}

fn facet(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> String {
    let v = |p: [f32; 3]| format!("vertex {} {} {}\n", p[0], p[1], p[2]);
    format!("facet normal 0 0 0\nouter loop\n{}{}{}endloop\nendfacet\n", v(a), v(b), v(c))
}

/// A tetrahedron, outward (`flip` false) or inward.
fn tet(o: [f32; 3], s: f32, flip: bool) -> String {
    let p = |d: [f32; 3]| [o[0] + s * d[0], o[1] + s * d[1], o[2] + s * d[2]];
    let (a, b, c, d) = (p([0.0, 0.0, 0.0]), p([1.0, 0.0, 0.0]), p([0.0, 1.0, 0.0]), p([0.0, 0.0, 1.0]));
    let mut faces = vec![[a, c, b], [a, b, d], [b, c, d], [a, d, c]];
    if flip {
        for f in &mut faces {
            f.swap(1, 2);
        }
    }
    faces.iter().map(|[x, y, z]| facet(*x, *y, *z)).collect()
}

#[test]
fn a_clean_closed_surface_has_only_its_summary() {
    let text = format!("solid tree\n{}endsolid tree\n", tet([0.0; 3], 1.0, false));
    let d = codes(text.as_bytes());
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!((d[0].0, d[0].1), (Level::Info, "surface.summary"));
    assert!(
        d[0].2.starts_with("ASCII 4 triangles, 1 region (tree), 1 × 1 × 1 m from (0 0 0); 1 part (1 closed)"),
        "{}",
        d[0].2
    );
}

#[test]
fn defects_are_reported() {
    let mut text = format!("solid Tree 1\n{}", tet([0.0; 3], 1.0, true));
    // A second, open part far away in millimetres, with a sliver and a copy.
    let (a, b, c) = ([0.0, 30_000.0, 0.0], [1000.0, 30_000.0, 0.0], [0.0, 31_000.0, 0.0]);
    text.push_str(&facet(a, b, c));
    text.push_str(&facet(a, b, c));
    text.push_str(&facet(a, [500.0, 30_000.0, 0.0], b));
    text.push_str("endsolid\n");
    let d = codes(text.as_bytes());
    let found: Vec<&str> = d.iter().map(|d| d.1).collect();
    assert_eq!(
        found,
        [
            "surface.summary",
            "surface.renamed",
            "surface.degenerate",
            "surface.duplicate",
            "surface.non-manifold",
            // The copy runs along the edges of the first the same way.
            "surface.normals",
            "surface.inward",
            "surface.open",
            "surface.units"
        ],
        "{d:?}"
    );
    assert!(d[1].2.contains("`Tree 1` is region `Tree1`"), "{}", d[1].2);
}

#[test]
fn files_openfoam_cant_read_are_errors() {
    let text = format!("solid a\n{}colour 1 0 0\nendsolid a\n", tet([0.0; 3], 1.0, false));
    let d = codes(text.as_bytes());
    assert_eq!((d[0].0, d[0].1), (Level::Error, "surface.openfoam"));
    assert_eq!(d[1].1, "surface.read");
}

#[test]
fn far_from_the_origin_and_inconsistent() {
    let mut text = String::from("solid a\n");
    let t = tet([2_600_000.0, 1_200_000.0, 400.0], 10.0, false);
    // Turn one facet over: its edges disagree with the other three.
    let mut facets: Vec<&str> = t.split_inclusive("endfacet\n").collect();
    let turned = {
        let f = facets[0];
        let lines: Vec<&str> = f.lines().collect();
        format!("{}\n{}\n{}\n{}\n{}\n{}\n{}\n", lines[0], lines[1], lines[2], lines[4], lines[3], lines[5], lines[6])
    };
    facets[0] = &turned;
    text.push_str(&facets.concat());
    text.push_str("endsolid a\n");
    let d = codes(text.as_bytes());
    let found: Vec<&str> = d.iter().map(|d| d.1).collect();
    assert_eq!(found, ["surface.summary", "surface.normals", "surface.far"], "{d:?}");
    assert!(d[2].2.contains("0.25 m apart"), "{}", d[2].2);
}
