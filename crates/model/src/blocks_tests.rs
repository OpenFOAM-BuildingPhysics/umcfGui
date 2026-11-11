use umcf_foam_dict::Document;

use super::*;

/// The street canyon's air as a `blocks` entry: two buildings missing, the canyon's floor and
/// walls their own patches, the buildings' other faces left to blockMesh.
const CANYON: &str = "blocks
{
    x       ( 0 (5 1) 50 (3 1) 60 (3 1) 70 (3 1) 80 (6 0.00739296596393882) 230 );
    y       ( 0 (6 1) 100 (5 1) 150 (6 ((0.5 0.5 13.868520453722747) (0.5 0.5 0.07210574504590131))) 250 );
    z       ( 0 (3 1) 10 (5 1) 60 );
    missing ( (1 1 0) (3 1 0) );
    patches { west inlet; east outlet; south side1; north side2; ground ground; top top; missing defaultFaces; }
    faces   { air_to_street ( (2 1 0 ground) ); air_to_windward ( (2 1 0 east) ); air_to_leeward ( (2 1 0 west) ); }
    noFaces ( street );
}
";

fn read(text: &str) -> (Option<BlockGrid>, Vec<(String, String)>) {
    let doc = Document::parse(text).unwrap();
    let body = doc.get(&["blocks"]).unwrap().dict().unwrap();
    let mut problems = Vec::new();
    let g = BlockGrid::read(body, "blocks", &mut |at: &str, m: String| problems.push((at.to_string(), m)));
    (g, problems)
}

fn canyon() -> BlockGrid {
    let (g, problems) = read(CANYON);
    assert!(problems.is_empty(), "{problems:?}");
    g.unwrap()
}

#[test]
fn a_grid_reads_and_writes_back_the_same() {
    let g = canyon();
    assert_eq!(g.counts(), [5, 3, 2]);
    assert_eq!(g.missing, vec![[1, 1, 0], [3, 1, 0]]);
    assert_eq!(g.side(Side::Missing), Some(DEFAULT_FACES));
    assert_eq!(
        g.patch_names(),
        [
            "inlet",
            "outlet",
            "side1",
            "side2",
            "ground",
            "top",
            "air_to_street",
            "air_to_windward",
            "air_to_leeward",
            "street"
        ]
    );
    assert!(g.problems().is_empty(), "{:?}", g.problems());
    // Every number as it was given.
    let text = g.entry("");
    assert!(text.contains("(6 0.00739296596393882)"), "{text}");
    assert!(text.contains("((0.5 0.5 13.868520453722747) (0.5 0.5 0.07210574504590131))"), "{text}");
    let (again, problems) = read(&text);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(again.unwrap(), g);
    // The canyon's cells: all of them but the two buildings'.
    assert_eq!(g.cells(), Some(20 * 17 * 8 - 2 * 3 * 5 * 3));
}

#[test]
fn what_cant_be_read_is_a_problem_where_it_is() {
    let (g, p) = read("blocks { x ( 0 (5 1) 50 60 ); y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 ); }");
    assert!(g.is_none());
    assert_eq!(p.first().map(|x| x.0.as_str()), Some("blocks/x"), "{p:?}");
    let (g, p) = read("blocks { x ( 0 (5 1) 50 ); y ( 0 (1 1) 1 ); }");
    assert!(g.is_none());
    assert!(p.iter().any(|x| x.0 == "blocks/z" && x.1.contains("missing")), "{p:?}");
    let (g, p) = read(
        "blocks { x ( 0 (5 1) 50 ); y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 ); patches { up top; west a; west b; } faces { a ( (0 0 0 missing) ); } colour red; }",
    );
    assert!(g.is_some());
    let at: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(at, ["blocks/patches/up", "blocks/patches/west", "blocks/faces/a", "blocks/colour"], "{p:?}");
    for bad in ["( 0 (1 nan) 1 )", "( 0 (1.5 1) 1 )", "( 0 (1 1) )", "( 0 1 )", "( )", "( 0 (1 ((1 1))) 1 )"] {
        let (g, p) = read(&format!("blocks {{ x {bad}; y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 ); }}"));
        assert!(g.is_none() && p.iter().any(|x| x.0 == "blocks/x"), "{bad}: {p:?}");
    }
}

#[test]
fn problems_of_a_grid() {
    let mut g = canyon();
    g.axes[0].stations.swap(1, 2);
    g.axes[1].spans[0].cells = 0;
    g.missing.push([9, 0, 0]);
    g.missing.push([1, 1, 0]);
    g.sides.retain(|(s, _)| *s != Side::Top);
    g.faces.push(("x".into(), vec![BlockFace { block: [0, 0, 0], side: Side::East }]));
    let p = g.problems();
    let at: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
    // The axes first: with them wrong, the rest isn't checked.
    assert_eq!(at, ["x", "y"], "{p:?}");
    g.axes = canyon().axes;
    let p = g.problems();
    let at: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(at, ["missing", "missing", "faces/x", "patches/top"], "{p:?}");
    // A whole layer missing.
    let mut layer = canyon();
    layer.missing = (0..3).flat_map(|j| (0..2).map(move |k| [4, j, k])).collect();
    assert!(layer.problems().iter().any(|x| x.1.contains("x 80 and 230 is missing")), "{:?}", layer.problems());
    assert!(p[2].1.contains("inside the mesh"), "{p:?}");
    // Too many blocks.
    let mut big = canyon();
    big.axes[0] = Axis {
        stations: (0..=100_001).map(f64::from).collect(),
        spans: (0..100_001).map(|_| Span { cells: 1, grading: Grading::Ratio(1.0) }).collect(),
    };
    assert!(big.problems().iter().any(|x| x.1.contains("more than")), "{:?}", big.problems());
}

/// The corners of face `q` of a generated dictionary, as coordinates.
fn coords(points: &[[f64; 3]], q: &[usize]) -> Vec<[f64; 3]> {
    q.iter().map(|i| points[*i]).collect()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn centre(c: &[[f64; 3]]) -> [f64; 3] {
    let n = c.len() as f64;
    let s = c.iter().fold([0.0; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
    [s[0] / n, s[1] / n, s[2] / n]
}

fn labels(n: &umcf_foam_dict::Node) -> Vec<usize> {
    match n {
        umcf_foam_dict::Node::List { items, .. } => items.iter().map(|x| atom(x).unwrap().parse().unwrap()).collect(),
        _ => panic!("not a list"),
    }
}

#[test]
fn the_dictionary_has_right_handed_blocks_and_outward_faces() {
    let g = canyon();
    let text = g
        .block_mesh_dict("air", "system/air", &|n| match n {
            "ground" => "wall".into(),
            DEFAULT_FACES => "empty".into(),
            _ => "patch".into(),
        })
        .unwrap();
    assert!(text.starts_with(&format!("// {GENERATED}\n")), "{text}");
    let doc = Document::parse(&text).unwrap();
    let points: Vec<[f64; 3]> = match doc.get(&["vertices"]).unwrap().value.as_slice() {
        [umcf_foam_dict::Node::List { items, .. }] => items
            .iter()
            .map(|v| {
                let l = match v {
                    umcf_foam_dict::Node::List { items, .. } => items,
                    _ => panic!(),
                };
                [0, 1, 2].map(|i| atom(&l[i]).unwrap().parse::<f64>().unwrap())
            })
            .collect(),
        _ => panic!(),
    };
    assert_eq!(points.len(), 6 * 4 * 3);
    let blocks = match doc.get(&["blocks"]).unwrap().value.as_slice() {
        [umcf_foam_dict::Node::List { items, .. }] => items.clone(),
        _ => panic!(),
    };
    let hexes: Vec<Vec<usize>> = blocks.chunks(5).map(|b| labels(&b[1])).collect();
    assert_eq!(hexes.len(), 5 * 3 * 2 - 2);
    // The first block: x, then y, then z, counter-clockwise from below.
    assert_eq!(hexes[0], [0, 1, 7, 6, 24, 25, 31, 30]);
    for h in &hexes {
        let c = coords(&points, h);
        let det = dot(cross(sub(c[1], c[0]), sub(c[3], c[0])), sub(c[4], c[0]));
        assert!(det > 0.0, "{h:?}");
    }
    // Each boundary face points away from the block it is on.
    let boundary = match doc.get(&["boundary"]).unwrap().value.as_slice() {
        [umcf_foam_dict::Node::List { items, .. }] => items.clone(),
        _ => panic!(),
    };
    let mut n = 0;
    let mut names = Vec::new();
    for pair in boundary.chunks(2) {
        names.push(atom(&pair[0]).unwrap().to_string());
        let body = match &pair[1] {
            umcf_foam_dict::Node::Dict { body, .. } => body,
            _ => panic!(),
        };
        let t = body.get("type").unwrap().value_text().unwrap();
        assert_eq!(t, if names.last().unwrap() == "ground" { "wall" } else { "patch" });
        let faces = match body.get("faces").unwrap().value.as_slice() {
            [umcf_foam_dict::Node::List { items, .. }] => items.clone(),
            _ => panic!(),
        };
        for f in &faces {
            let q = labels(f);
            let c = coords(&points, &q);
            let normal = cross(sub(c[1], c[0]), sub(c[2], c[1]));
            // The block it is on: the one whose corners include all four.
            let h = hexes.iter().find(|h| q.iter().all(|v| h.contains(v))).unwrap();
            let out = sub(centre(&c), centre(&coords(&points, h)));
            assert!(dot(normal, out) > 0.0, "{q:?} of {h:?}");
            n += 1;
        }
    }
    assert_eq!(names, g.patch_names());
    // The faces next to the buildings that aren't the canyon's: left to blockMesh.
    let named = g.boundary().values().filter(|(_, p)| *p != Some(DEFAULT_FACES)).count();
    assert_eq!(n, named);
    assert!(n < g.boundary().len());
    assert!(text.contains("defaultPatch\n{\n    name defaultFaces;\n    type empty;\n}"), "{text}");
    // Read back as the same grid.
    let imp = import(&doc).unwrap();
    assert!(imp.grid.same_mesh(&g));
    assert!(untouched(&text));
    // Changed by hand, it isn't.
    let edited = Document::parse(&text.replacen("(5 6 3)", "(5 6 4)", 1)).unwrap();
    assert!(!untouched(&edited.to_string()));
}

#[test]
fn imported_dictionaries_that_are_no_grid_say_why() {
    let base = |extra: &str, blocks: &str| {
        format!(
            "vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) (2 0 0) (2 1 0) (2 0 1) (2 1 1) );\nblocks ( {blocks} );\n{extra}"
        )
    };
    let one = "hex (0 1 2 3 4 5 6 7) (2 2 2) simpleGrading (1 1 1)";
    let ok = import(&Document::parse(&base("", one)).unwrap()).unwrap();
    assert_eq!(ok.grid.counts(), [1, 1, 1]);
    assert_eq!(ok.grid.side(Side::West), Some(DEFAULT_FACES));
    for (extra, blocks, why) in [
        ("edges ( arc 0 1 (0.5 -0.1 0) );", one, "edges"),
        ("convertToMeters 0.001;", one, "convertToMeters"),
        ("#include \"x\"", one, "#include"),
        ("", "hex (0 1 2 3 4 5 6 7) zone (2 2 2) simpleGrading (1 1 1)", "cell zone"),
        (
            "",
            "hex (0 8 9 3 4 10 11 7) (2 2 2) simpleGrading (1 1 1) hex (0 1 2 3 4 5 6 7) (2 2 2) simpleGrading (1 1 1)",
            "more than one cell",
        ),
        ("", "hex (2 3 0 1 6 7 4 5) (5 2 2) simpleGrading (((0.5 0.5 2) (0.5 0.5 0.5)) 1 1)", "shares out otherwise"),
        ("", "hex (0 3 2 1 4 7 6 5) (2 2 2) simpleGrading (1 1 1)", "inside out"),
        ("", "hex (0 1 2 3 4 5 6 7) (2 2 2) edgeGrading (1 1 1 2 1 1 1 1 1 1 1 1)", "edgeGrading differs"),
        (
            "",
            "hex (0 1 2 3 4 5 6 7) (2 2 2) simpleGrading (1 1 1) hex (1 8 9 2 5 10 11 6) (2 3 2) simpleGrading (1 1 1)",
            "differ in cells",
        ),
        ("boundary ( a { type patch; faces ( (0 1 5 4) ); } b { type wall; faces ( (4 5 1 0) ); } );", one, "both"),
        ("boundary ( a { type cyclic; neighbourPatch b; faces ( ); } );", one, "neighbourPatch"),
    ] {
        let doc = Document::parse(&base(extra, blocks)).unwrap();
        let err = import(&doc).unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }
    // Rotated numbering is still a grid, read along x, y and z.
    let rotated = "hex (0 3 7 4 1 2 6 5) (4 2 3) simpleGrading (1 1 1)";
    let r = import(&Document::parse(&base("", rotated)).unwrap()).unwrap();
    assert_eq!(
        [r.grid.axes[0].spans[0].cells, r.grid.axes[1].spans[0].cells, r.grid.axes[2].spans[0].cells],
        [3, 4, 2]
    );
    // Numbered against x and y: the gradings seen from the axes' starts, sections in the
    // other order with their expansions inverted.
    let mirrored = "hex (2 3 0 1 6 7 4 5) (2 3 4) simpleGrading (2 ((0.2 0.8 4) (0.8 0.2 1)) 5)";
    let m = import(&Document::parse(&base("", mirrored)).unwrap()).unwrap();
    let spans: Vec<&Span> = m.grid.axes.iter().map(|a| &a.spans[0]).collect();
    assert_eq!(spans[0], &Span { cells: 2, grading: Grading::Ratio(0.5) });
    assert_eq!(spans[1], &Span { cells: 3, grading: Grading::Sections(vec![[0.8, 0.2, 1.0], [0.2, 0.8, 0.25]]) });
    assert_eq!(spans[2], &Span { cells: 4, grading: Grading::Ratio(5.0) });
    // Written again, the block runs along the axes with those gradings.
    let text = m.grid.block_mesh_dict("r", "system", &|_| "patch".into()).unwrap();
    assert!(
        text.contains("hex (0 1 3 2 4 5 7 6) (2 3 4) simpleGrading (0.5 ((0.8 0.2 1) (0.2 0.8 0.25)) 5)"),
        "{text}"
    );
    // An edgeGrading the same along each direction is a simpleGrading.
    let edge = "hex (0 1 2 3 4 5 6 7) (2 2 2) edgeGrading (3 3 3 3 1 1 1 1 2 2 2 2)";
    let e = import(&Document::parse(&base("", edge)).unwrap()).unwrap();
    assert_eq!(e.grid.axes[0].spans[0].grading, Grading::Ratio(3.0));
    assert_eq!(e.grid.axes[2].spans[0].grading, Grading::Ratio(2.0));
}

#[test]
fn import_names_each_side_by_most_of_its_faces() {
    let text =
        "vertices ( (0 0 0) (1 0 0) (2 0 0) (0 1 0) (1 1 0) (2 1 0) (0 0 1) (1 0 1) (2 0 1) (0 1 1) (1 1 1) (2 1 1) );
blocks ( hex (0 1 4 3 6 7 10 9) (1 1 1) simpleGrading (1 1 1) hex (1 2 5 4 7 8 11 10) (1 1 1) simpleGrading (1 1 1) );
boundary (
    hot { type wall; faces ( (1 4 5 2) ); }
    floor { type wall; faces ( (0 3 4 1) ); }
    empty { type patch; faces ( ); }
);";
    let imp = import(&Document::parse(text).unwrap()).unwrap();
    let g = &imp.grid;
    // The ground: one face in each, the first listed of the two its patch.
    assert_eq!(g.side(Side::Ground), Some("hot"));
    assert_eq!(g.side(Side::West), Some(DEFAULT_FACES));
    assert_eq!(g.faces, vec![("floor".to_string(), vec![BlockFace { block: [0, 0, 0], side: Side::Ground }])]);
    assert_eq!(g.no_faces, ["empty"]);
    assert_eq!(
        imp.types,
        [
            ("hot".into(), "wall".into()),
            ("floor".into(), "wall".into()),
            ("empty".into(), "patch".into()),
            ("defaultFaces".into(), "empty".into())
        ]
    );
    assert!(g.problems().is_empty(), "{:?}", g.problems());
}

#[test]
fn types_follow_roles() {
    assert_eq!(patch_type(RegionKind::Fluid, Some(Role::Ground)), "wall");
    assert_eq!(patch_type(RegionKind::Fluid, Some(Role::Coupled)), "wall");
    assert_eq!(patch_type(RegionKind::Fluid, Some(Role::Inlet)), "patch");
    assert_eq!(patch_type(RegionKind::Vegetation, Some(Role::Sky)), "patch");
    assert_eq!(patch_type(RegionKind::Fluid, None), "patch");
    assert_eq!(patch_type(RegionKind::Solid, Some(Role::Coupled)), "patch");
}

#[test]
fn numbers_round_trip() {
    for t in ["0.00739296596393882", "13.868520453722747", "0.07210574504590131", "200", "-2", "0.1", "1e-7"] {
        let x = parse_number(t).unwrap();
        assert_eq!(parse_number(&number(x)), Some(x), "{t}");
    }
    assert_eq!(number(parse_number("0.00739296596393882").unwrap()), "0.00739296596393882");
    assert_eq!(number(parse_number("60.00").unwrap()), "60");
    assert_eq!(number(-0.0), "0");
}

/// The canyon of the description tests with a `blockMeshDict` for its air that is a grid.
fn canyon_case() -> (umcf_core::MemFs, Case) {
    let grid = small();
    // Written as a hand-made one would be: its own types, no header of umcfGui's.
    let text = grid
        .block_mesh_dict("air", "system/air", &|n| {
            if n == "ground" || n == "air_to_street" { "wall".into() } else { "patch".into() }
        })
        .unwrap();
    let text = text.replace(&format!("// {GENERATED}\n"), "");
    let fs = crate::description::tests::canyon().with("system/air/blockMeshDict", text);
    let c = Case::open(&fs).unwrap();
    (fs, c)
}

fn small() -> BlockGrid {
    let (g, p) = read(
        "blocks { x ( 0 (2 1) 1 (2 1) 2 ); y ( 0 (1 1) 1 (1 1) 2 ); z ( 0 (1 1) 1 );
        patches { west west; east east; south odd; north north; ground ground; top top; }
        faces { air_to_street ( (1 0 0 ground) ); } }",
    );
    assert!(p.is_empty(), "{p:?}");
    g.unwrap()
}

#[test]
fn the_import_reads_the_blocks_and_the_description_keeps_them() {
    let (fs, mut c) = canyon_case();
    let (d, notes) = crate::description::Description::import(&c);
    let air = d.region("air").unwrap();
    assert!(air.blocks.as_ref().unwrap().same_mesh(&small()), "{notes:?}");
    // The dictionary's types are those of the roles: nothing noted.
    assert!(!notes.iter().any(|n| n.contains("blockMeshDict")), "{notes:?}");
    // The description written and read back.
    let text = d.write();
    assert!(text.contains("        blocks\n        {\n            x       ( 0 (2 1) 1 (2 1) 2 );"), "{text}");
    let (again, problems) = crate::description::Description::read(&Document::parse(&text).unwrap());
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(again.region("air").unwrap().blocks, air.blocks);
    // The dictionary is the case's own (no header), and makes the same mesh.
    c.put_description(&fs, &text, false).unwrap();
    let (d, _) = c.description().unwrap();
    let (path, state) = dict_state(&c, d.region("air").unwrap()).unwrap();
    assert_eq!((path.as_str(), state), ("system/air/blockMeshDict", DictState::Same));
}

#[test]
fn a_change_of_the_blocks_edits_only_its_keys() {
    let (fs, mut c) = canyon_case();
    let (d, _) = crate::description::Description::import(&c);
    let text = d.write().replace("            x       (", "            // the street's length\n            x       (");
    c.put_description(&fs, &text, false).unwrap();
    let mut g = small();
    g.axes[1].spans[0].cells = 4;
    g.missing.push([0, 0, 0]);
    c.describe_blocks("air", &g).unwrap();
    let after = c.dict(crate::description::PATH).unwrap().to_string();
    assert!(after.contains("// the street's length\n            x       ( 0 (2 1) 1 (2 1) 2 );"), "{after}");
    assert!(after.contains("            y       ( 0 (4 1) 1 (1 1) 2 );"), "{after}");
    assert!(after.contains("            missing ( (0 0 0) );"), "{after}");
    let before: Vec<&str> = text.lines().collect();
    let new: Vec<&str> = after.lines().filter(|l| !before.contains(l)).collect();
    assert_eq!(new.len(), 2, "{new:?}");
    let (d, problems) = c.description().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(d.region("air").unwrap().blocks.as_ref(), Some(&g));
    // The dictionary now differs, and isn't one umcfGui wrote.
    let (_, state) = dict_state(&c, d.region("air").unwrap()).unwrap();
    assert!(matches!(state, DictState::Differs { untouched: false, .. }), "{state:?}");
    // Written from the blocks, it is the same and untouched.
    let (path, text) = generate(d.region("air").unwrap()).unwrap();
    c.put_dict(&fs, &path, &text).unwrap();
    assert_eq!(dict_state(&c, d.region("air").unwrap()).unwrap().1, DictState::Same);
    assert!(untouched(&c.dict(&path).unwrap().to_string()));
    // Missing blocks taken away again: the key goes.
    g.missing.clear();
    c.describe_blocks("air", &g).unwrap();
    assert!(!c.dict(crate::description::PATH).unwrap().to_string().contains("missing"));
    // Taken out.
    c.undescribe_blocks("air").unwrap();
    assert!(c.description().unwrap().0.region("air").unwrap().blocks.is_none());
    assert!(c.undescribe_blocks("air").is_err());
}

#[test]
fn types_read_write_and_are_checked() {
    let (g, p) = read(
        "blocks { x ( 0 (1 1) 1 ); y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 );
        patches { west a; east a; south b; north b; ground c; top c; }
        types { b symmetryPlane; c slip; }
    }",
    );
    assert!(p.is_empty(), "{p:?}");
    let g = g.unwrap();
    assert_eq!(g.given_type("b"), Some("symmetryPlane"));
    assert!(g.entry("").contains("    types   { b symmetryPlane; c slip; }\n"), "{}", g.entry(""));
    assert_eq!(read(&g.entry("")).0.unwrap(), g);
    let mut bad = g.clone();
    bad.types.push(("nowhere".into(), "wall".into()));
    bad.types.push(("b".into(), "wall".into()));
    let at: Vec<String> = bad.problems().into_iter().map(|x| x.0).collect();
    assert_eq!(at, ["types/nowhere", "types/b"]);
    // Renamed with the patch.
    let mut r = g.clone();
    assert!(r.rename_patch("b", "sides"));
    assert_eq!((r.side(Side::South), r.given_type("sides")), (Some("sides"), Some("symmetryPlane")));
    assert!(!r.rename_patch("b", "x"));
}

#[test]
// Native only: timed with the clock.
#[allow(clippy::disallowed_methods)]
fn problems_of_a_hostile_grid_are_quick_and_few() {
    let axis = |n: usize| Axis {
        stations: (0..=n).map(|x| x as f64).collect(),
        spans: (0..n).map(|_| Span { cells: 1, grading: Grading::Ratio(1.0) }).collect(),
    };
    let mut g = BlockGrid { axes: [axis(40), axis(40), axis(10)], ..BlockGrid::default() };
    // Half the grid missing, each listed twice, as many outside.
    for k in 0..5 {
        for j in 0..40 {
            for i in 0..39 {
                g.missing.push([i, j, k]);
                g.missing.push([i, j, k]);
                g.missing.push([i + 1000, j, k]);
            }
        }
    }
    let start = std::time::Instant::now();
    let p = g.problems();
    assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    assert_eq!(p.len(), MAX_PROBLEMS + 1);
    assert!(p.last().unwrap().1.starts_with("and "), "{p:?}");
    // Too long a dictionary to write: a problem, and refused by the writer itself.
    let sections = Grading::Sections(vec![[1.0, 1.0, 1.5]; 1000]);
    let mut big = BlockGrid { axes: [axis(50), axis(20), axis(20)], ..BlockGrid::default() };
    for s in &mut big.axes[0].spans {
        s.grading = sections.clone();
    }
    assert!(big.problems().iter().any(|x| x.1.contains("MB")), "{:?}", big.problems());
    assert!(big.block_mesh_dict("r", "system", &|_| "patch".into()).is_err());
}

#[test]
fn a_renamed_patch_is_renamed_in_the_blocks() {
    let (fs, mut c) = canyon_case();
    let (d, _) = crate::description::Description::import(&c);
    c.put_description(&fs, &d.write(), false).unwrap();
    let edit = crate::description_edit::PatchEdit { rename: Some("floor".into()), ..Default::default() };
    c.describe_patch("air", "ground", &edit).unwrap();
    let (d, problems) = c.description().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let g = d.region("air").unwrap().blocks.clone().unwrap();
    assert_eq!(g.side(Side::Ground), Some("floor"));
    assert!(!g.patch_names().contains(&"ground"));
    // Not to `defaultFaces`: the faces left to blockMesh; nothing changes.
    let before = c.description().unwrap().0;
    let edit = crate::description_edit::PatchEdit { rename: Some(DEFAULT_FACES.into()), ..Default::default() };
    let e = c.describe_patch("air", "floor", &edit).unwrap_err().to_string();
    assert!(e.contains("faces left to blockMesh"), "{e}");
    assert_eq!(c.description().unwrap().0, before);
}

#[test]
fn line_divide_shares_cells_as_blockmesh_does() {
    // Rounded shares, what is lacking or too much to the first of the largest share.
    assert_eq!(line_divide_counts(&[[0.5, 0.5, 2.0], [0.5, 0.5, 0.5]], 5), Some(vec![2, 3]));
    assert_eq!(line_divide_counts(&[[0.3, 0.4, 2.0], [0.7, 0.6, 0.5]], 5), Some(vec![2, 3]));
    assert_eq!(line_divide_counts(&[[1.0, 1.0, 1.0], [1.0, 2.0, 1.0], [1.0, 1.0, 1.0]], 10), Some(vec![3, 4, 3]));
    // Fewer cells than sections: uniform.
    assert_eq!(line_divide_counts(&[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], 1), None);
    // A tie turned round shares otherwise: not mirrored. Unequal shares are.
    let tie = Grading::Sections(vec![[0.5, 0.5, 2.0], [0.5, 0.5, 0.5]]);
    assert!(tie.mirrored(5).unwrap_err().contains("shares out otherwise"));
    assert_eq!(tie.mirrored(4), Ok(Grading::Sections(vec![[0.5, 0.5, 2.0], [0.5, 0.5, 0.5]])));
    let ok = Grading::Sections(vec![[0.3, 0.4, 2.0], [0.7, 0.6, 0.5]]);
    assert_eq!(ok.mirrored(5), Ok(Grading::Sections(vec![[0.7, 0.6, 2.0], [0.3, 0.4, 0.5]])));
    // A ratio is always mirrored, even when its inverse isn't exact.
    assert_eq!(Grading::Ratio(49.0).mirrored(3), Ok(Grading::Ratio(1.0 / 49.0)));
}

#[test]
fn extreme_numbers_stay_short_and_exact() {
    for x in [1e-300, 1.5e300, -2.2250738585072014e-308, 1.7976931348623157e308, 5e-324, 123456789012345680000.0] {
        let t = number(x);
        assert!(t.len() <= NUMBER_LEN, "{t}");
        assert_eq!(parse_number(&t), Some(x), "{t}");
    }
    assert_eq!(number(1e-300), "1e-300");
    assert_eq!(number(200.0), "200");
    // The dictionary of a grid of such numbers stays within its estimate.
    let axis = |a: f64, n: usize| Axis {
        stations: (0..=n).map(|i| a * (1.0 + i as f64 * 1e-3)).collect(),
        spans: (0..n)
            .map(|_| Span { cells: 4_000_000_000, grading: Grading::Sections(vec![[1e-300, 1e-300, 1e300]; 3]) })
            .collect(),
    };
    let g = BlockGrid {
        axes: [axis(-1e-300, 20), axis(1e300, 20), axis(-1e-300, 20)],
        sides: Side::ALL.iter().map(|s| (*s, "a_long_patch_name_for_the_side".to_string())).collect(),
        ..BlockGrid::default()
    };
    let text = g.block_mesh_dict("region", "system/region", &|_| "symmetryPlane".into()).unwrap();
    assert!((text.len() as u64) <= g.estimated_len().unwrap(), "{} > {:?}", text.len(), g.estimated_len());
    assert!(text.contains("e-300"));
}

#[test]
fn a_patch_isnt_renamed_to_one_the_blocks_name() {
    let (fs, mut c) = canyon_case();
    let (d, _) = crate::description::Description::import(&c);
    c.put_description(&fs, &d.write(), false).unwrap();
    // A name only the blocks have (a patch without faces).
    let mut g = d.region("air").unwrap().blocks.clone().unwrap();
    g.no_faces.push("spare".into());
    c.describe_blocks("air", &g).unwrap();
    let before = c.dict(crate::description::PATH).unwrap().to_string();
    let edit = crate::description_edit::PatchEdit { rename: Some("spare".into()), ..Default::default() };
    let e = c.describe_patch("air", "ground", &edit).unwrap_err().to_string();
    assert!(e.contains("blocks name a patch spare"), "{e}");
    assert_eq!(c.dict(crate::description::PATH).unwrap().to_string(), before);
}

#[test]
fn hostile_sections_are_refused_not_a_panic() {
    // Panicked (an add overflow) before: a negative share made a saturated count.
    let text = "vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );
blocks ( hex (2 3 0 1 6 7 4 5) (5000 1 1) simpleGrading (((1 1 1) (1 1 1) (1 -1.9999999999999998 1)) 1 1) );";
    let e = import(&Document::parse(text).unwrap()).unwrap_err();
    assert!(e.contains("aren't all numbers above 0"), "{e}");
    for bad in [
        vec![[1.0, 1.0, 1.0], [1.0, -1.9999999999999998, 1.0]],
        vec![[1.0, f64::NAN, 1.0]],
        vec![[1.0, 1e308, 1.0], [1.0, 1e308, 1.0]],
        vec![],
    ] {
        assert_eq!(line_divide_counts(&bad, 5000), None, "{bad:?}");
    }
    assert_eq!(
        line_divide_counts(&[[1.0, 1e-300, 1.0], [1.0, 1.0, 1.0]], u32::MAX),
        Some(vec![0, i64::from(u32::MAX)])
    );
}

#[test]
fn a_section_blockmesh_gives_no_cells_is_a_problem() {
    let mut g = canyon();
    g.axes[2].spans[0] =
        Span { cells: 3, grading: Grading::Sections(vec![[0.9, 0.9, 1.0], [0.05, 0.05, 1.0], [0.05, 0.05, 1.0]]) };
    assert_eq!(line_divide_counts(&[[0.9, 0.9, 1.0], [0.05, 0.05, 1.0], [0.05, 0.05, 1.0]], 3), Some(vec![3, 0, 0]));
    let p = g.problems();
    assert!(p.iter().any(|x| x.0 == "z" && x.1.contains("would give a section no cells")), "{p:?}");
}

#[test]
fn the_estimate_holds_with_long_types() {
    // The reviewer's case: one block, six sides `p`, 60 patches without faces of 255-character
    // types.
    let long = "t".repeat(255);
    let one = Axis { stations: vec![0.0, 1.0], spans: vec![Span { cells: 1, grading: Grading::Ratio(1.0) }] };
    let g = BlockGrid {
        axes: [one.clone(), one.clone(), one],
        sides: Side::ALL[..6].iter().map(|s| (*s, "p".to_string())).collect(),
        no_faces: (0..60).map(|i| format!("n{i}")).collect(),
        types: (0..60).map(|i| (format!("n{i}"), long.clone())).collect(),
        ..BlockGrid::default()
    };
    let text = g.block_mesh_dict("r", "system/r", &|n| g.given_type(n).unwrap_or("patch").to_string()).unwrap();
    assert!((text.len() as u64) <= g.estimated_len().unwrap(), "{} > {:?}", text.len(), g.estimated_len());
}

#[test]
fn the_default_patch_is_seen_in_a_dictionary_that_is_no_grid() {
    // The 3×3 cases' buildings: boxes over several stations each, some faces named, the rest
    // left to blockMesh's defaultFaces (which createPatch renames).
    let boxes = "vertices ( (0 0 0) (2 0 0) (2 1 0) (0 1 0) (0 0 1) (2 0 1) (2 1 1) (0 1 1)
                   (1 0 1) (1 1 1) (0 0 2) (1 0 2) (1 1 2) (0 1 2) );
blocks ( hex (0 1 2 3 4 5 6 7) (4 2 2) simpleGrading (1 1 1) hex (4 8 9 7 10 11 12 13) (2 2 2) simpleGrading (1 1 1) );";
    // A box on half the top of another: the lower spans x 0 to 2, over the upper's station 1.
    let doc =
        Document::parse(&format!("{boxes}\nboundary ( buildings {{ type patch; faces ( (4 5 6 7) ); }} );")).unwrap();
    assert!(import(&doc).is_err());
    let p = dict_patches(&doc).unwrap();
    let names: Vec<(&str, &str)> = p.patches.iter().map(|q| (q.name.as_str(), q.kind.as_str())).collect();
    assert_eq!(names, [("buildings", "patch"), ("defaultFaces", "empty")]);
    // Every face named: no default patch, though defaultPatch is there.
    let all = "( (4 5 6 7) (0 3 2 1) (0 4 7 3) (1 2 6 5) (0 1 5 4) (3 7 6 2) (4 8 9 7) (10 11 12 13) (4 10 13 7) (8 9 12 11) (4 8 11 10) (7 13 12 9) )";
    let doc = Document::parse(&format!(
        "{boxes}\nboundary ( b {{ type wall; faces {all}; }} );\ndefaultPatch {{ name floor; type wall; }}"
    ))
    .unwrap();
    assert_eq!(dict_patches(&doc).unwrap().patches.len(), 1);
    // The older patches syntax, read instead of boundary as blockMesh does, said.
    let doc =
        Document::parse(&format!("{boxes}\npatches ( wall b {all} );\nboundary ( c {{ type patch; faces ( ); }} );"))
            .unwrap();
    let p = dict_patches(&doc).unwrap();
    assert_eq!(p.patches.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), ["b"]);
    assert!(p.notes[0].contains("blockMesh reads patches"), "{:?}", p.notes);
    // What can't be told: one point under two labels, a name twice, a block that isn't a hex.
    for (text, why) in [
        (
            "vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) (1 0 0) );
blocks ( hex (0 8 2 3 4 5 6 7) (1 1 1) simpleGrading (1 1 1) hex (0 1 2 3 4 5 6 7) (1 1 1) simpleGrading (1 1 1) );",
            "one point under two labels",
        ),
        (
            &format!("{boxes}\nboundary ( a {{ type patch; faces ( ); }} a {{ type wall; faces ( ); }} );"),
            "names a twice",
        ),
        ("vertices ( (0 0 0) );\nblocks ( prism (0 0 0 0 0 0) (1 1 1) simpleGrading (1 1 1) );", "isn't a hex"),
    ] {
        let e = dict_patches(&Document::parse(text).unwrap()).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
}

#[test]
fn what_else_a_dictionary_holds_is_named() {
    let text = "convertToMeters 0.001;
vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );
blocks ( hex (0 1 2 3 4 5 6 7) brick (1 1 1) simpleGrading (1 1 1) );
edges ( );
mergePatchPairs ( ( a b ) );";
    let s = dict_settings(&Document::parse(text).unwrap());
    assert_eq!(
        s,
        ["patches merged (mergePatchPairs)", "a scale (convertToMeters 0.001)", "cell zones of blocks (brick)"]
    );
    // What umcfGui writes holds none.
    let g = canyon();
    let t = g.block_mesh_dict("air", "system/air", &|_| "patch".into()).unwrap();
    assert!(dict_settings(&Document::parse(&t).unwrap()).is_empty());
}

#[test]
fn the_stations_bound_is_the_same_everywhere() {
    let axis = |n: usize| -> String {
        let mut s = String::from("( 0");
        for i in 1..n {
            s.push_str(&format!(" (1 1) {i}"));
        }
        s.push_str(" )");
        s
    };
    // MAX_STATIONS stations: read, no problem, written, read back.
    let (g, p) = read(&format!(
        "blocks {{ x {}; y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 ); patches {{ west a; east a; south a; north a; ground a; top a; }} }}",
        axis(MAX_STATIONS)
    ));
    assert!(p.is_empty(), "{p:?}");
    let g = g.unwrap();
    assert_eq!(g.axes[0].stations.len(), MAX_STATIONS);
    assert!(g.problems().is_empty(), "{:?}", g.problems());
    let imp =
        import(&Document::parse(&g.block_mesh_dict("r", "system", &|_| "patch".into()).unwrap()).unwrap()).unwrap();
    assert_eq!(imp.grid.axes[0].stations.len(), MAX_STATIONS);
    // One more: refused by each.
    let (g, p) = read(&format!("blocks {{ x {}; y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 ); }}", axis(MAX_STATIONS + 1)));
    assert!(g.is_none() && p.iter().any(|x| x.1.contains("more than")), "{p:?}");
    let mut big = canyon();
    big.axes[0] = Axis {
        stations: (0..=MAX_STATIONS).map(|x| x as f64).collect(),
        spans: (0..MAX_STATIONS).map(|_| Span { cells: 1, grading: Grading::Ratio(1.0) }).collect(),
    };
    assert!(big.problems().iter().any(|x| x.1.contains("more than")), "{:?}", big.problems());
}

#[test]
fn the_estimate_holds_with_long_sections() {
    let n = "0.0000012345678901234567";
    let x = parse_number(n).unwrap();
    assert_eq!(number(x).len(), NUMBER_LEN, "{}", number(x));
    let sections = Grading::Sections(vec![[x, x, x]; MAX_SECTIONS]);
    let axis = Axis { stations: vec![0.0, 1.0, 2.0], spans: vec![Span { cells: 5000, grading: sections.clone() }; 2] };
    let g = BlockGrid {
        axes: [axis.clone(), axis.clone(), axis],
        sides: Side::ALL[..6].iter().map(|s| (*s, "p".to_string())).collect(),
        ..BlockGrid::default()
    };
    let text = g.block_mesh_dict("r", "system/r", &|_| "patch".into()).unwrap();
    assert!((text.len() as u64) <= g.estimated_len().unwrap(), "{} > {:?}", text.len(), g.estimated_len());
}

#[test]
fn the_docs_examples_of_blocks_have_no_problem() {
    let docs = [include_str!("../../../docs/common-workflow.md"), include_str!("../../../docs/user-guide/workflow.md")];
    let mut n = 0;
    for d in docs {
        for (at, _) in d.match_indices("blocks") {
            let rest = &d[at..];
            // `blocks {` or `blocks      {` followed by its dictionary.
            let head = rest.trim_start_matches("blocks").trim_start_matches([' ', '\n']);
            if !head.starts_with('{') || !rest[..rest.len().min(30)].contains('{') {
                continue;
            }
            let start = rest.len() - head.len();
            let mut depth = 0usize;
            let mut end = None;
            for (i, c) in head.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { continue };
            let body = &rest[start..start + end + 1];
            if body.contains('…') {
                continue;
            }
            let (g, p) = read(&format!("blocks {body}"));
            assert!(p.is_empty(), "{body}: {p:?}");
            let g = g.unwrap();
            assert!(g.problems().is_empty(), "{body}: {:?}", g.problems());
            n += 1;
        }
    }
    assert!(n >= 2, "{n} examples");
}

#[test]
// Native only: timed with the clock.
#[allow(clippy::disallowed_methods)]
fn many_names_are_quick() {
    let one = Axis { stations: vec![0.0, 1.0], spans: vec![Span { cells: 1, grading: Grading::Ratio(1.0) }] };
    let grid = |n: usize| BlockGrid {
        axes: [one.clone(), one.clone(), one.clone()],
        sides: Side::ALL[..6].iter().map(|s| (*s, "p".to_string())).collect(),
        no_faces: (0..n).map(|i| format!("n{i}")).collect(),
        types: (0..n).map(|i| (format!("n{i}"), "wall".to_string())).collect(),
        ..BlockGrid::default()
    };
    // As many as a grid may name: checked, estimated and written quickly.
    let g = grid(MAX_NAMES);
    let start = std::time::Instant::now();
    assert!(g.problems().is_empty(), "{:?}", g.problems());
    let types = g.type_map();
    let text = g.block_mesh_dict("r", "system", &|n| types.get(n).map_or("patch", |t| t).to_string()).unwrap();
    assert!(text.contains("n9999\n    {\n        type wall;"));
    assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    // Many more: refused at once.
    let g = grid(80_000);
    let start = std::time::Instant::now();
    let p = g.problems();
    assert!(p.iter().any(|x| x.0 == "noFaces" && x.1.contains("more than")), "{p:?}");
    assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
}

#[test]
fn blocks_joined_by_points_under_two_labels_are_no_grid() {
    // Two boxes side by side whose shared face has its own labels on each side: blockMesh
    // keeps a baffle there, the grid would join them.
    let text = "vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1)
                   (1 0 0) (2 0 0) (2 1 0) (1 1 0) (1 0 1) (2 0 1) (2 1 1) (1 1 1) );
blocks ( hex (0 1 2 3 4 5 6 7) (1 1 1) simpleGrading (1 1 1) hex (8 9 10 11 12 13 14 15) (1 1 1) simpleGrading (1 1 1) );";
    let e = import(&Document::parse(text).unwrap()).unwrap_err();
    assert!(e.contains("one point under two labels"), "{e}");
}

#[test]
fn other_entries_and_unread_blocks_are_said() {
    let text = "fastMerge yes;
myLength 3;
vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );
blocks ( hex (0 1 2 3 4 5 6 7) brick (1 1 1) simpleGrading (1 1 1) hex (0 1 2 3 4 5 6 7) other 1 1 1 );";
    let s = dict_settings(&Document::parse(text).unwrap());
    assert_eq!(
        s,
        [
            "other entries (fastMerge, myLength)",
            "cell zones of blocks (brick, other)",
            "blocks not fully read (cell zones in them may be lost)"
        ]
    );
}

#[test]
fn types_needing_entries_of_their_own_are_refused() {
    for t in [
        "cyclic",
        "cyclicAMI",
        "nonConformalCyclic",
        "nonConformalError",
        "nonConformalMappedWall",
        "mappedWall",
        "mappedPatch",
        "processor",
    ] {
        assert!(needs_own_entries(t), "{t}");
        let mut g = canyon();
        g.types.push(("side1".into(), t.into()));
        assert!(g.problems().iter().any(|x| x.0 == "types/side1" && x.1.contains("entries of its own")), "{t}");
    }
    for t in ["patch", "wall", "symmetryPlane", "empty", "slip"] {
        assert!(!needs_own_entries(t), "{t}");
    }
    for t in [
        "symmetryPlane",
        "symmetry",
        "empty",
        "wedge",
        "cyclic",
        "nonConformalError",
        "nonConformalProcessorCyclic",
        "processor",
    ] {
        assert!(is_constraint_type(t), "{t}");
    }
}

#[test]
fn a_grid_s_dictionary_reads_back() {
    // The bounds are below what the parser reads.
    let parser = umcf_foam_dict::Limits::default();
    assert!(MAX_DICT_BYTES < parser.max_bytes as u64 && MAX_DICT_TOKENS < parser.max_tokens as u64);
    // The token estimate is an upper bound: the dictionary parses within it.
    let mut g = canyon();
    g.axes[1].spans[2].grading = Grading::Sections(vec![[1.0, 1.0, 2.0]; 7]);
    let text = g.block_mesh_dict("air", "system/air", &|_| "patch".into()).unwrap();
    let limits = umcf_foam_dict::Limits {
        max_tokens: g.estimated_tokens().unwrap() as usize,
        max_bytes: g.estimated_len().unwrap() as usize + 1000,
        ..parser
    };
    Document::parse_with(&text, &limits).unwrap();
    // 200×20×5 blocks, graded in ten sections along x: within the blocks' count, but its
    // dictionary wouldn't read back (too many tokens).
    let axis = |n: usize| Axis {
        stations: (0..=n).map(|x| x as f64).collect(),
        spans: (0..n).map(|_| Span { cells: 1, grading: Grading::Ratio(1.0) }).collect(),
    };
    let mut ten = axis(200);
    for s in &mut ten.spans {
        s.cells = 10;
        s.grading = Grading::Sections(vec![[1.0, 1.0, 1.0]; 10]);
    }
    let big = BlockGrid {
        axes: [ten, axis(20), axis(5)],
        sides: Side::ALL[..6].iter().map(|s| (*s, "p".to_string())).collect(),
        ..BlockGrid::default()
    };
    assert!(big.problems().iter().any(|x| x.1.contains("read back")), "{:?}", big.problems());
    assert!(big.block_mesh_dict("r", "system", &|_| "patch".into()).unwrap_err().contains("read back"));
}

#[test]
fn what_the_blocks_can_t_hold_inside_their_keys_is_a_problem() {
    let (g, p) = read(
        "blocks { x ( 0 (1 1) 1 ); y ( 0 (1 1) 1 ); z ( 0 (1 1) 1 );
        patches
        {
            west a;
            #include \"myPatches\"
            $extraSides;
            east $b;
            south a; north a; ground a; top a;
        }
        faces
        {
            #include \"moreFaces\"
        }
        types
        {
            #include \"myTypes\"
            a $t;
        }
        noFaces ( $more c );
    }",
    );
    assert!(g.is_some());
    let at: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
    for want in
        ["blocks/patches", "blocks/patches/east", "blocks/faces", "blocks/types", "blocks/types/a", "blocks/noFaces"]
    {
        assert!(at.contains(&want), "{want}: {p:?}");
    }
    assert!(p.iter().any(|x| x.1.contains("#include") && x.0 == "blocks/patches"), "{p:?}");
    assert!(p.iter().any(|x| x.1.contains("$extraSides")), "{p:?}");
}

#[test]
fn a_rename_waits_for_blocks_that_read_whole() {
    let (fs, mut c) = canyon_case();
    let (d, _) = crate::description::Description::import(&c);
    let text = d.write().replacen("            faces   {", "            faces   { #include \"moreFaces\"", 1);
    c.put_description(&fs, &text, false).unwrap();
    let before = c.dict(crate::description::PATH).unwrap().to_string();
    assert!(before.contains("#include \"moreFaces\""), "{before}");
    let edit = crate::description_edit::PatchEdit { rename: Some("floor".into()), ..Default::default() };
    let e = c.describe_patch("air", "ground", &edit).unwrap_err().to_string();
    assert!(e.contains("don't read") && e.contains("blocks/faces"), "{e}");
    assert_eq!(c.dict(crate::description::PATH).unwrap().to_string(), before);
    // Nor are the blocks set from what was read.
    let g = c.description().unwrap().0.region("air").unwrap().blocks.clone().unwrap();
    assert!(c.describe_blocks("air", &g).unwrap_err().to_string().contains("don't read"));
}

#[test]
fn keys_given_twice_and_expressions_are_said() {
    let text = "vertices ( (0 0 0) (1 0 0) (1 1 0) (0 1 0) (0 0 1) (1 0 1) (1 1 1) (0 1 1) );
blocks ( hex (0 1 2 3 4 5 6 7) ($n 1 1) simpleGrading (1 1 #calc \"2*3\") );
boundary ( );
boundary ( );";
    let doc = Document::parse(text).unwrap();
    assert!(dict_patches(&doc).unwrap_err().contains("gives boundary twice"));
    let s = dict_settings(&doc);
    assert!(s.contains(&"entries given twice (boundary)".to_string()), "{s:?}");
    assert!(s.contains(&"expressions in its blocks ($macros, #calc)".to_string()), "{s:?}");
}
