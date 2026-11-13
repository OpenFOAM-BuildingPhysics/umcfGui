use super::*;

fn read(text: &str) -> (Option<Snappy>, Vec<String>) {
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    let body = doc.get(&["snappy"]).unwrap().dict().unwrap();
    let mut problems = Vec::new();
    let s = Snappy::read(body, "snappy", &mut |at: &str, m: String| problems.push(format!("{at}: {m}")));
    (s, problems)
}

fn example() -> Snappy {
    Snappy {
        inside: [0.5, 0.0, 50.0],
        surfaces: vec![
            Surface {
                name: "buildings".into(),
                file: "buildings.stl".into(),
                mode: Mode::Cut,
                level: Some((3, 5)),
                distance: Vec::new(),
                layers: 3,
                features: Some(4),
                solids: Vec::new(),
            },
            Surface {
                name: "trees".into(),
                file: "my_trees.stl.gz".into(),
                mode: Mode::Refine,
                level: None,
                distance: vec![(2.0, 4), (4.5, 2)],
                layers: 0,
                features: None,
                solids: Vec::new(),
            },
        ],
        boxes: vec![RefineBox { name: "street".into(), min: [0.0, -1.0, 0.0], max: [100.0, 50.0, 10.0], level: 2 }],
        expansion: Some(1.3),
        first_layer: Some(0.15),
    }
}

#[test]
fn a_snappy_entry_reads_back_as_it_is_written() {
    let s = example();
    assert!(s.problems().is_empty(), "{:?}", s.problems());
    let text = s.entry("");
    let (back, problems) = read(&text);
    assert!(problems.is_empty(), "{problems:?}\n{text}");
    assert_eq!(back.as_ref(), Some(&s), "{text}");
    // The default file isn't written.
    assert!(!text.contains("file buildings.stl"), "{text}");
    let mut plain = s.clone();
    plain.expansion = None;
    plain.first_layer = None;
    plain.boxes.clear();
    let (back, problems) = read(&plain.entry("    "));
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(back, Some(plain));
}

#[test]
fn what_doesnt_read_is_a_problem() {
    let (s, p) = read("snappy { surfaces { b { mode cut; level ( 1 2 ); } } }");
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.starts_with("snappy: has no inside")), "{p:?}");
    let (s, p) = read("snappy { inside ( 0 0 1 ); surfaces { b { level ( 1 ); } } }");
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.starts_with("snappy/surfaces/b/level: is ( least most )")), "{p:?}");
    assert!(p.iter().any(|p| p == "snappy/surfaces/b: has no mode: cut or refine"), "{p:?}");
    let (s, p) = read("snappy { inside ( 0 0 1 ); boxes { x { min ( 0 0 0 ); level 2; } } colour red; }");
    assert!(s.is_none());
    assert!(p.iter().any(|p| p == "snappy/boxes/x: has no max"), "{p:?}");
    assert!(p.iter().any(|p| p.starts_with("snappy/colour: isn't a key")), "{p:?}");
    let (s, p) = read("snappy { inside ( 0 0 1 ); surfaces { b { mode cut; level ( 1 2 ); colour red; } } }");
    assert!(s.is_some(), "an unknown key is noted, not fatal: {p:?}");
    assert!(p.iter().any(|p| p.starts_with("snappy/surfaces/b/colour")), "{p:?}");
    let (s, p) = read("snappy { inside ( 0 0 1 ); surfaces { b { mode cut; distance ( 2 4 ); } } }");
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.starts_with("snappy/surfaces/b/distance")), "{p:?}");
}

#[test]
fn problems_say_what_snappy_would_refuse() {
    let has = |s: &Snappy, at: &str| s.problems().iter().any(|(a, _)| a == at);
    let mut s = example();
    s.surfaces[0].level = None;
    assert!(has(&s, "surfaces/buildings"));
    let mut s = example();
    s.surfaces[0].level = Some((5, 3));
    assert!(has(&s, "surfaces/buildings/level"));
    let mut s = example();
    s.surfaces[1].distance = vec![(4.0, 2), (2.0, 4)];
    assert!(has(&s, "surfaces/trees/distance"));
    let mut s = example();
    s.surfaces[1].distance = vec![(2.0, 2), (4.0, 4)];
    assert!(has(&s, "surfaces/trees/distance"), "the levels can't grow away from the surface");
    let mut s = example();
    s.surfaces[1].features = Some(2);
    assert!(has(&s, "surfaces/trees/features"), "a compressed surface has no plain .eMesh");
    let mut s = example();
    s.surfaces[1].layers = 2;
    assert!(has(&s, "surfaces/trees/layers"));
    let mut s = example();
    s.boxes[0].name = "buildings".into();
    assert!(has(&s, "boxes/buildings"));
    let mut s = example();
    s.boxes[0].max[2] = -1.0;
    assert!(has(&s, "boxes/street"));
    let mut s = example();
    s.inside[0] = f64::NAN;
    assert!(has(&s, "inside"));
    let mut s = example();
    s.expansion = Some(0.9);
    s.first_layer = Some(0.0);
    assert!(has(&s, "expansion") && has(&s, "firstLayer"));
    let mut s = example();
    s.surfaces.clear();
    s.boxes.clear();
    assert!(has(&s, ""));
}

#[test]
fn the_dictionary_is_what_snappy_hex_mesh_reads() {
    let s = example();
    let text = s.dict("air", "system", &|n| if n == "buildings" { "wall".into() } else { "patch".into() });
    assert!(crate::blocks::untouched(&text), "{text}");
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    let w = |path: &[&str]| doc.get(path).and_then(|e| e.words()).map(|w| w.join(" "));
    assert_eq!(w(&["castellatedMesh"]).as_deref(), Some("true"));
    assert_eq!(w(&["snap"]).as_deref(), Some("true"));
    assert_eq!(w(&["addLayers"]).as_deref(), Some("true"));
    assert_eq!(w(&["geometry", "trees", "file"]).as_deref(), Some("\"my_trees.stl.gz\""));
    assert_eq!(w(&["geometry", "street", "type"]).as_deref(), Some("searchableBox"));
    let c = ["castellatedMeshControls"];
    assert!(doc.get(&[c[0], "refinementSurfaces", "buildings"]).is_some());
    assert!(doc.get(&[c[0], "refinementSurfaces", "trees"]).is_none(), "a refining surface doesn't cut");
    assert_eq!(w(&[c[0], "refinementSurfaces", "buildings", "patchInfo", "type"]).as_deref(), Some("wall"));
    assert_eq!(w(&[c[0], "refinementRegions", "trees", "mode"]).as_deref(), Some("distance"));
    assert_eq!(w(&[c[0], "refinementRegions", "street", "mode"]).as_deref(), Some("inside"));
    assert!(text.contains("levels ((2 4) (4.5 2));"), "{text}");
    assert!(text.contains("{ file \"buildings.eMesh\"; level 4; }"), "{text}");
    assert!(text.contains("locationInMesh ( 0.5 0 50 );"), "{text}");
    assert_eq!(w(&["snapControls", "explicitFeatureSnap"]).as_deref(), Some("true"));
    let l = ["addLayersControls"];
    assert_eq!(w(&[l[0], "relativeSizes"]).as_deref(), Some("false"));
    assert_eq!(w(&[l[0], "firstLayerThickness"]).as_deref(), Some("0.15"));
    assert_eq!(w(&[l[0], "minThickness"]).as_deref(), Some("0.015"));
    assert_eq!(w(&[l[0], "expansionRatio"]).as_deref(), Some("1.3"));
    assert_eq!(w(&[l[0], "layers", "buildings", "nSurfaceLayers"]).as_deref(), Some("3"));

    // Only refinement: nothing snaps, no layers, the features left to snappyHexMesh.
    let mut r = example();
    r.surfaces.remove(0);
    let text = r.dict("vegetation", "system", &|_| "patch".into());
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    let w = |path: &[&str]| doc.get(path).and_then(|e| e.words()).map(|w| w.join(" "));
    assert_eq!(w(&["snap"]).as_deref(), Some("false"));
    assert_eq!(w(&["addLayers"]).as_deref(), Some("false"));
    assert_eq!(w(&["addLayersControls", "relativeSizes"]).as_deref(), Some("false"));
    assert_eq!(w(&["snapControls", "implicitFeatureSnap"]).as_deref(), Some("true"));
}

#[test]
fn a_surface_of_several_solids_makes_a_patch_of_each() {
    assert_eq!(patch_names("b", &[]), ["b"]);
    assert_eq!(patch_names("b", &["roof".into()]), ["b"]);
    assert_eq!(patch_names("b", &["roof".into(), "wall".into()]), ["b_roof", "b_wall"]);
}

#[test]
fn the_dictionary_is_the_one_the_snappy_step_names() {
    use crate::workflow::{MeshBuild, RootStep};
    let m = MeshBuild::Root(vec![
        RootStep::BlockMesh("system/blockMeshDict.air".into()),
        RootStep::SnappyHexMesh("system/snappyHexMeshDict.air".into()),
    ]);
    assert_eq!(dict_path(&m).as_deref(), Some("system/snappyHexMeshDict.air"));
    assert_eq!(dict_path(&MeshBuild::Block), None);
}

#[test]
fn a_surface_of_several_solids_has_a_patch_and_layers_for_each() {
    let mut s = example();
    s.surfaces[0].solids = vec!["roof".into(), "wall".into()];
    assert!(s.problems().is_empty(), "{:?}", s.problems());
    let (back, problems) = read(&s.entry(""));
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(back, Some(s.clone()));
    // The same type for both: one patchInfo; the layers on each patch.
    let text = s.dict("air", "system", &|_| "wall".into());
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    let w = |path: &[&str]| doc.get(path).and_then(|e| e.words()).map(|w| w.join(" "));
    let l = "addLayersControls";
    assert_eq!(w(&[l, "layers", "buildings_roof", "nSurfaceLayers"]).as_deref(), Some("3"));
    assert_eq!(w(&[l, "layers", "buildings_wall", "nSurfaceLayers"]).as_deref(), Some("3"));
    assert!(doc.get(&[l, "layers", "buildings"]).is_none());
    let rs = ["castellatedMeshControls", "refinementSurfaces", "buildings"];
    assert_eq!(w(&[rs[0], rs[1], rs[2], "patchInfo", "type"]).as_deref(), Some("wall"));
    // Other types: each solid its own.
    let text = s.dict("air", "system", &|p| if p == "buildings_roof" { "patch".into() } else { "wall".into() });
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    let w = |path: &[&str]| doc.get(path).and_then(|e| e.words()).map(|w| w.join(" "));
    assert!(doc.get(&[rs[0], rs[1], rs[2], "patchInfo"]).is_none(), "{text}");
    assert_eq!(w(&[rs[0], rs[1], rs[2], "regions", "roof", "patchInfo", "type"]).as_deref(), Some("patch"));
    assert_eq!(w(&[rs[0], rs[1], rs[2], "regions", "wall", "patchInfo", "type"]).as_deref(), Some("wall"));
    assert!(text.contains("wall { level (3 5); patchInfo { type wall; } }"), "{text}");
    // One solid listed, or one twice: a problem.
    s.surfaces[0].solids = vec!["roof".into()];
    assert!(s.problems().iter().any(|(a, _)| a == "surfaces/buildings/solids"));
    s.surfaces[0].solids = vec!["roof".into(), "roof".into()];
    assert!(s.problems().iter().any(|(a, _)| a == "surfaces/buildings/solids"));
}

#[test]
fn a_file_name_the_dictionary_cant_hold_is_a_problem() {
    for bad in ["my trees.stl", "x\".stl", "a;b.stl", "a{.stl", "$x.stl", "#x.stl", "a(1).stl"] {
        let mut s = example();
        s.surfaces[0].file = bad.into();
        assert!(s.problems().iter().any(|(a, _)| a == "surfaces/buildings/file"), "{bad}");
    }
}

#[test]
fn too_many_entries_or_distances_are_refused_before_they_are_read() {
    let many: String = (0..=MAX_ITEMS).map(|i| format!("s{i} {{ mode cut; level ( 1 1 ); }} ")).collect();
    let (s, p) = read(&format!("snappy {{ inside ( 0 0 1 ); surfaces {{ {many} }} }}"));
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.contains("names more than 1000 surfaces and boxes")), "{p:?}");
    // Surfaces and boxes together: the boxes are refused once both pass the limit.
    let half = |k: &str, body: &str| -> String { (0..600).map(|i| format!("{k}{i} {{ {body} }} ")).collect() };
    let (s, p) = read(&format!(
        "snappy {{ inside ( 0 0 1 ); surfaces {{ {} }} boxes {{ {} }} }}",
        half("s", "mode cut; level ( 1 1 );"),
        half("b", "min ( 0 0 0 ); max ( 1 1 1 ); level 1;")
    ));
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.starts_with("snappy/boxes: names more than 1000")), "{p:?}");
    // The read message gives the limit as MAX_DISTANCES is.
    assert_eq!(MAX_DISTANCES, 100, "the read message says 100: change it with the limit");
    let pairs: String = (1..=MAX_DISTANCES + 1).map(|i| format!("( {i} 1 ) ")).collect();
    let (s, p) =
        read(&format!("snappy {{ inside ( 0 0 1 ); surfaces {{ t {{ mode refine; distance ( {pairs}); }} }} }}"));
    assert!(s.is_none());
    assert!(p.iter().any(|p| p.starts_with("snappy/surfaces/t/distance") && p.contains("at most 100")), "{p:?}");
    let mut e = example();
    e.surfaces[1].distance = (1..=MAX_DISTANCES + 1).map(|i| (i as f64, 1)).collect();
    assert!(e.problems().iter().any(|(a, m)| a == "surfaces/trees/distance" && m.contains("more than 100")));
}
