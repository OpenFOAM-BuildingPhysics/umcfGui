use super::*;

fn imported(text: &str) -> Result<(Snappy, Vec<String>), String> {
    imported_all(text).map_err(|(why, _)| why)
}

fn imported_all(text: &str) -> Result<(Snappy, Vec<String>), (String, Vec<String>)> {
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    import(&doc.root, "system/snappyHexMeshDict", None)
}

fn entry() -> Snappy {
    let surface = |name: &str, mode: Mode| Surface {
        name: name.into(),
        file: format!("{name}.stl"),
        mode,
        level: None,
        distance: Vec::new(),
        layers: 0,
        features: None,
        solids: Vec::new(),
    };
    let mut buildings = surface("buildings", Mode::Cut);
    buildings.level = Some((2, 4));
    buildings.layers = 3;
    buildings.features = Some(4);
    buildings.distance = vec![(5.0, 3)];
    let mut trees = surface("trees", Mode::Refine);
    trees.file = "my trees.stl.gz".replace(' ', "_");
    trees.distance = vec![(2.0, 3), (4.5, 1)];
    Snappy {
        inside: [0.5, -1.0, 50.0],
        surfaces: vec![buildings, trees],
        boxes: vec![RefineBox { name: "street".into(), min: [0.0, 0.0, 0.0], max: [100.0, 50.0, 10.0], level: 2 }],
        expansion: Some(1.1),
        first_layer: Some(0.2),
    }
}

#[test]
fn the_dictionary_an_entry_writes_reads_back_to_it() {
    let sn = entry();
    let text = sn.dict("air", "system", &|_| "wall".to_string());
    let (back, notes) = imported(&text).unwrap();
    assert_eq!(back, sn);
    assert_eq!(notes, Vec::<String>::new());
    // Relative layers and the default expansion: none given.
    let mut sn = entry();
    sn.expansion = None;
    sn.first_layer = None;
    let text = sn.dict("air", "system", &|_| "wall".to_string());
    assert_eq!(imported(&text).unwrap().0, sn);
}

#[test]
fn a_hand_written_dictionary_gives_what_the_entry_holds() {
    // The 3×3 case's vegetation dictionary, OpenFOAM-6 style.
    let veg = r#"
#includeEtc "caseDicts/mesh/generation/snappyHexMeshDict.cfg"
castellatedMesh true;
snap            true;
addLayers       false;
geometry
{
    air_to_vegetation{type triSurfaceMesh;  file "air_to_vegetation.stl";}
};
castellatedMeshControls
{
    features ( { file  "air_to_vegetation.eMesh"; level 3; } );
    refinementSurfaces
    {
        air_to_vegetation{ level (2 3); patchInfo { type wall; }}
    }
    refinementRegions
    {
        air_to_vegetation { mode distance; levels ((2.0 4)); }
    }
    locationInMesh (0 0 50);
}
addLayersControls
{
    relativeSizes       false;
    expansionRatio      1.3;
    firstLayerThickness 0.15;
    layers { air_to_vegetation { nSurfaceLayers 2; } }
}
meshQualityControls { #include "meshQualityDict" }
"#;
    let (sn, notes) = imported(veg).unwrap();
    assert_eq!(sn.inside, [0.0, 0.0, 50.0]);
    let s = &sn.surfaces[0];
    assert_eq!((s.mode, s.level, s.features, s.layers), (Mode::Cut, Some((2, 3)), Some(3), 0));
    assert_eq!(s.distance, vec![(2.0, 4)]);
    assert_eq!((sn.expansion, sn.first_layer), (Some(1.3), Some(0.15)));
    assert!(notes.iter().any(|n| n.contains("addLayers is off")), "{notes:?}");

    // OpenFOAM-8's form (the key names the file), features by distance, other geometry, a
    // box that isn't a refinement inside, layers by a pattern.
    let other = r#"
addLayers true;
geometry
{
    city.stl { type triSurfaceMesh; name city; }
    ball { type searchableSphere; centre (0 0 0); radius 1; }
    zone { type searchableBox; min (0 0 0); max (1 1 1); }
    hall { type searchableBox; min (0 0 0); max (2 2 2); }
    loose { type triSurfaceMesh; file "loose.stl"; }
}
castellatedMeshControls
{
    features ( { file "city.eMesh"; levels ((1.5 5) (4 3)); } { file "gone.eMesh"; level 1; } );
    refinementSurfaces { city { level (1 2); regions { roof { level (2 2); } } } }
    refinementRegions
    {
        zone { mode outside; levels ((1e15 1)); }
        hall { mode inside; levels ((1e15 3)); }
    }
    insidePoint (5 5 5);
}
addLayersControls { layers { "city.*" { nSurfaceLayers 2; } city_roof { nSurfaceLayers 1; } } }
"#;
    let (sn, notes) = imported(other).unwrap();
    let s = &sn.surfaces[0];
    assert_eq!((s.name.as_str(), s.file.as_str(), s.features, s.layers), ("city", "city.stl", Some(5), 1));
    assert_eq!(sn.surfaces.len(), 1);
    assert_eq!(sn.boxes, vec![RefineBox { name: "hall".into(), min: [0.0; 3], max: [2.0; 3], level: 3 }]);
    assert_eq!(sn.inside, [5.0; 3]);
    for want in [
        "geometry ball is a searchableSphere",
        "box zone isn't a refinement of the cells inside it",
        "surface loose neither cuts nor refines",
        "the features of city.eMesh are refined by distance",
        "features gone.eMesh aren't the <stem>.eMesh",
        "surface city has levels or types per solid",
        "the layers of city.* are given by a pattern",
    ] {
        assert!(notes.iter().any(|n| n.contains(want)), "{want}: {notes:?}");
    }
}

#[test]
fn a_dictionary_without_what_an_entry_needs_gives_none() {
    let e = imported("geometry {} castellatedMeshControls { locationsInMesh ( ((0 0 0) a) ); }").unwrap_err();
    assert!(e.contains("several inside points (locationsInMesh)"), "{e}");
    let e = imported("geometry {} castellatedMeshControls { insidePoints ((0 0 0) (1 1 1)); }").unwrap_err();
    assert!(e.contains("several inside points (insidePoints)"), "{e}");
    let e = imported("geometry {} castellatedMeshControls { locationInMesh (0 0 $z); }").unwrap_err();
    assert!(e.contains("isn't ( x y z ) in numbers"), "{e}");
    let e = imported("castellatedMeshControls { locationInMesh (0 0 0); }").unwrap_err();
    assert!(e.contains("no geometry"), "{e}");
    // Nothing it can hold: no entry.
    let e = imported("geometry { b { type searchableSphere; } } castellatedMeshControls { locationInMesh (0 0 0); }")
        .unwrap_err();
    assert!(e.contains("names no surface or box"), "{e}");
    let e = imported(
        "geometry { b { type triSurfaceMesh; } } castellatedMeshControls { refinementSurfaces { b { level (3 1); } } locationInMesh (0 0 0); }",
    )
    .unwrap_err();
    assert!(e.contains("names no surface or box"), "{e}");
}

#[test]
fn openfoam_12_s_inside_points_come_first() {
    let geometry = "geometry { b { type triSurfaceMesh; file \"b.stl\"; } }";
    let refine = "refinementRegions { b { mode distance; levels ((1 1)); } }";
    let with = |points: &str| imported(&format!("{geometry} castellatedMeshControls {{ {refine} {points} }}"));
    let (sn, _) = with("insidePoints ((5 5 5)); insidePoint (2 2 2); locationInMesh (1 1 1);").unwrap();
    assert_eq!(sn.inside, [5.0; 3]);
    let (sn, _) = with("insidePoint (2 2 2); locationInMesh (1 1 1);").unwrap();
    assert_eq!(sn.inside, [2.0; 3]);
    let (_, notes) = with("locationInMesh (1 1 1); outsidePoints ((9 9 9));").unwrap();
    assert!(notes.iter().any(|n| n.contains("outside points")), "{notes:?}");
}

#[test]
fn a_surface_the_entry_cant_hold_leaves_the_others() {
    let text = r#"
addLayers true;
geometry
{
    b { type triSurfaceMesh; file "b.stl"; }
    t { type triSurfaceMesh; file "t.obj"; }
    deep { type triSurfaceMesh; file "deep.stl"; }
    #include "moreGeometry"
}
castellatedMeshControls
{
    refinementSurfaces { b { level (1 2); } t { level (1 1); } deep { level (2 12); } }
    locationInMesh (0 0 0);
}
addLayersControls
{
    layers
    {
        "b_.*" { nSurfaceLayers 3; }
        b_roof { nSurfaceLayers 0; }
        b_walls { nSurfaceLayers 2; }
        b_base { nSurfaceLayers 1; }
    }
}
"#;
    let (sn, notes) = imported(text).unwrap();
    let names: Vec<&str> = sn.surfaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["b"]);
    assert_eq!(sn.surfaces[0].layers, 2, "the most, 0 not counted");
    for want in [
        "surface t can't be held by the entry (surfaces/t/file",
        "surface deep can't be held by the entry (surfaces/deep/level",
        "the layers of b_.* are given by a pattern",
        "the patches of surface b have different numbers of layers: the entry gives each 2",
        "its geometry has an #include or a $macro",
    ] {
        assert!(notes.iter().any(|n| n.contains(want)), "{want}: {notes:?}");
    }
    // Layers the other way round: the same.
    let swapped =
        text.replace("b_walls { nSurfaceLayers 2; }", "").replace("b_roof", "b_walls { nSurfaceLayers 2; } b_roof");
    assert_eq!(imported(&swapped).unwrap().0.surfaces[0].layers, 2);
}

#[test]
fn openfoam_12_s_forms_are_read_as_openfoam_reads_them() {
    // A box's `level` (OpenFOAM-12), or `levels` of one pair (OpenFOAM-8).
    for refine in ["mode inside; level 3;", "mode inside; levels ((1e15 3));", "mode inside; level 3; levels ((1 1));"]
    {
        let text = format!(
            "geometry {{ hall {{ type searchableBox; min (0 0 0); max (1 1 1); }} }} castellatedMeshControls {{ refinementRegions {{ hall {{ {refine} }} }} locationInMesh (5 5 5); }}"
        );
        let (sn, _) = imported(&text).unwrap();
        assert_eq!(sn.boxes[0].level, 3, "{refine}");
    }
    // Features: `levels` before `level`.
    let text = r#"
geometry { b { type triSurfaceMesh; file "b.stl"; } }
castellatedMeshControls
{
    features ( { file "b.eMesh"; level 1; levels ((0 4)); } );
    refinementRegions { b { mode distance; levels ((1 1)); } }
    locationInMesh (5 5 5);
}
"#;
    assert_eq!(imported(text).unwrap().0.surfaces[0].features, Some(4));
}

#[test]
fn why_nothing_is_held_is_told() {
    let (why, notes) = imported_all(
        "geometry { b { type triSurfaceMesh; file \"b.stl\"; } } castellatedMeshControls { refinementSurfaces { b { level (1 12); } } locationInMesh (0 0 0); }",
    )
    .unwrap_err();
    assert!(why.contains("names no surface or box"), "{why}");
    assert!(notes.iter().any(|n| n.contains("surface b can't be held by the entry (surfaces/b/level")), "{notes:?}");
}

#[test]
fn a_patch_s_layers_go_to_the_surface_of_the_longest_name() {
    let text = r#"
addLayers true;
geometry { b { type triSurfaceMesh; file "b.stl"; } b_x { type triSurfaceMesh; file "bx.stl"; } }
castellatedMeshControls
{
    refinementSurfaces { b { level (1 1); } b_x { level (1 1); } ".*" { level (2 2); } }
    locationInMesh (0 0 0);
}
addLayersControls { layers { b_x_roof { nSurfaceLayers 4; expansionRatio 1.1; } b_walls { nSurfaceLayers 1; } } }
"#;
    let (sn, notes) = imported(text).unwrap();
    let layers: Vec<(&str, u32)> = sn.surfaces.iter().map(|s| (s.name.as_str(), s.layers)).collect();
    assert_eq!(layers, [("b", 1), ("b_x", 4)]);
    for want in
        ["name surfaces by a pattern (.*)", "the layers of b_x_roof have settings of their own (expansionRatio)"]
    {
        assert!(notes.iter().any(|n| n.contains(want)), "{want}: {notes:?}");
    }
}

#[test]
fn what_would_change_the_mesh_when_written_again_is_left_out() {
    let base = |geometry: &str, surfaces: &str, layers: &str| {
        format!(
            "addLayers true;\ngeometry {{ {geometry} keep {{ type triSurfaceMesh; file \"keep.stl\"; }} }}\ncastellatedMeshControls {{ refinementSurfaces {{ keep {{ level (1 1); }} {surfaces} }} locationInMesh (0 0 0); }}\naddLayersControls {{ layers {{ keep {{ nSurfaceLayers 2; }} }} {layers} }}"
        )
    };
    // A scaled surface.
    let (sn, notes) =
        imported(&base("b { type triSurfaceMesh; file \"b.stl\"; scale 0.001; }", "b { level (1 1); }", "")).unwrap();
    assert_eq!(sn.surfaces.len(), 1);
    assert!(notes.iter().any(|n| n.contains("geometry b is scaled")), "{notes:?}");
    // A scale of 1 is no scale; other keys are noted.
    let (sn, notes) = imported(&base(
        "b { type triSurfaceMesh; file \"b.stl\"; scale 1; minQuality 0.1; }",
        "b { level (1 1); }",
        "",
    ))
    .unwrap();
    assert_eq!(sn.surfaces.len(), 2);
    assert!(notes.iter().any(|n| n.contains("geometry b's minQuality aren't held")), "{notes:?}");
    // A surface making zones isn't a cutting one.
    let (sn, notes) = imported(&base(
        "z { type triSurfaceMesh; file \"z.stl\"; }",
        "z { level (2 2); faceZone z; cellZone z; mode insidePoint; }",
        "",
    ))
    .unwrap();
    assert_eq!(sn.surfaces.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["keep"]);
    assert!(notes.iter().any(|n| n.contains("surface z makes a faceZone or cellZone")), "{notes:?}");
    // Layer sizes other than the entry's, and an expansion it would refuse.
    let (sn, notes) =
        imported(&base("", "", "finalLayerThickness 0.3; minThickness 0.1; expansionRatio 0.9;")).unwrap();
    assert_eq!(sn.expansion, None);
    for want in ["layers' finalLayerThickness, minThickness aren't what the entry writes", "expansionRatio is below 1"]
    {
        assert!(notes.iter().any(|n| n.contains(want)), "{want}: {notes:?}");
    }
    let (_, notes) = imported(&base("", "", "finalLayerThickness 0.5; minThickness 0.25;")).unwrap();
    assert_eq!(notes, Vec::<String>::new());
}

#[test]
fn openfoam_8_reads_location_in_mesh_only() {
    let text = "geometry { b { type triSurfaceMesh; file \"b.stl\"; } } castellatedMeshControls { refinementRegions { b { mode distance; levels ((1 1)); } } insidePoint (5 5 5); locationInMesh (0 0 1); }";
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    let (sn, _) = import(&doc.root, "p", Some(umcf_core::OfVersion::V8)).unwrap();
    assert_eq!(sn.inside, [0.0, 0.0, 1.0]);
    let (sn, _) = import(&doc.root, "p", Some(umcf_core::OfVersion::V12)).unwrap();
    assert_eq!(sn.inside, [5.0; 3]);
}

#[test]
fn too_many_entries_are_refused_before_they_are_read() {
    let many = |n: usize, f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>().join(" ");
    let geometry = many(1001, &|i| format!("s{i} {{ type triSurfaceMesh; }}"));
    let e = imported(&format!("geometry {{ {geometry} }} castellatedMeshControls {{ locationInMesh (0 0 0); }}"))
        .unwrap_err();
    assert!(e.contains("more than 1000"), "{e}");
    let layers = many(1001, &|i| format!("p{i} {{ nSurfaceLayers 1; }}"));
    let e = imported(&format!(
        "geometry {{ b {{ type triSurfaceMesh; }} }} castellatedMeshControls {{ refinementSurfaces {{ b {{ level (1 1); }} }} locationInMesh (0 0 0); }} addLayersControls {{ layers {{ {layers} }} }}"
    ))
    .unwrap_err();
    assert!(e.contains("more than 1000 patches"), "{e}");
}

#[test]
fn what_openfoam_would_name_and_type_otherwise_is_noted() {
    let text = r#"
addLayers true;
singleRegionName false;
geometry
{
    b { type triSurfaceMesh; file "b.stl"; }
    h { type triSurfaceMesh; file "h.stl"; }
    hall { type searchableBox; min (0 0 0); max (1 1 1); }
}
castellatedMeshControls
{
    refinementSurfaces
    {
        b { level (1 1); patchInfo { type patch; } regions { roof { patchInfo { type wall; } } } }
        h { level (1 1); }
    }
    refinementRegions { hall { mode inside; level 2; levels ((1e15 3)); } }
    locationInMesh (0 0 0);
}
addLayersControls { layers { b_roof { nSurfaceLayers 3; } h { nSurfaceLayers 1; } } }
"#;
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    for (version, box_level) in [(Some(umcf_core::OfVersion::V8), 3), (Some(umcf_core::OfVersion::V12), 2)] {
        let (sn, notes) = import(&doc.root, "p", version).unwrap();
        assert_eq!(sn.boxes[0].level, box_level, "{version:?}");
        for want in ["singleRegionName is off", "the layers of b_roof are taken as their surface's, by name"] {
            assert!(notes.iter().any(|n| n.contains(want)), "{want}: {notes:?}");
        }
        assert!(!notes.iter().any(|n| n.contains("the layers of h")), "{notes:?}");
    }
    // The types snappyHexMesh gives: a solid's own, the surface's, else wall; a regions entry
    // of one solid is that solid's.
    assert_eq!(patch_type(&doc.root, "b", Some("roof")), "wall");
    assert_eq!(patch_type(&doc.root, "b", Some("walls")), "patch");
    assert_eq!(patch_type(&doc.root, "b", None), "wall");
    assert_eq!(patch_type(&doc.root, "h", None), "wall");
}

#[test]
fn switches_are_read_as_openfoam_reads_them() {
    for (w, want) in [("y", Some(true)), ("t", Some(true)), ("on", Some(true)), ("1", Some(true))] {
        assert_eq!(switch(w), want, "{w}");
    }
    for (w, want) in [("n", Some(false)), ("f", Some(false)), ("none", Some(false)), ("0", Some(false))] {
        assert_eq!(switch(w), want, "{w}");
    }
    assert_eq!(switch("maybe"), None);
    // `singleRegionName 0;` and `addLayers 1;` as numbers; `relativeSizes n;` is absolute, so
    // the first layer is 0.3 m and its sizes are what the entry writes.
    let text = r#"
addLayers 1;
singleRegionName 0;
geometry { b { type triSurfaceMesh; file "b.stl"; } }
castellatedMeshControls { refinementSurfaces { b { level (1 1); } } locationInMesh (0 0 0); }
addLayersControls { relativeSizes n; firstLayerThickness 0.3; minThickness 0.03; layers { b { nSurfaceLayers 2; } } }
"#;
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    assert!(single_region_off(&doc.root));
    let (sn, notes) = import(&doc.root, "p", None).unwrap();
    assert_eq!(sn.surfaces[0].layers, 2, "{notes:?}");
    assert_eq!(sn.first_layer, Some(0.3), "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("aren't what the entry writes")), "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("addLayers is off")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("singleRegionName is off")), "{notes:?}");
}

#[test]
fn patch_info_beyond_its_type_is_noted() {
    let text = r#"
geometry { b { type triSurfaceMesh; file "b.stl"; } }
castellatedMeshControls
{
    refinementSurfaces
    {
        b
        {
            level (1 1);
            patchInfo { type wall; inGroups (buildings); }
            regions { roof { patchInfo { type wall; transform none; } } }
        }
    }
    locationInMesh (0 0 0);
}
"#;
    let (_, notes) = imported(text).unwrap();
    assert!(notes.iter().any(|n| n.contains("patchInfo gives inGroups, transform besides the type")), "{notes:?}");
}

#[test]
fn each_item_is_held_or_left_out_on_its_own() {
    // `b/roof` isn't a name the entry takes: it alone is left out, not `b`.
    let text = r#"
geometry
{
    b { type triSurfaceMesh; file "b.stl"; }
    "b/roof" { type triSurfaceMesh; file "r.stl"; }
}
castellatedMeshControls
{
    refinementSurfaces { b { level (1 1); } "b/roof" { level (1 1); } }
    locationInMesh (0 0 0);
}
"#;
    let (sn, notes) = imported(text).unwrap();
    assert_eq!(sn.surfaces.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b"], "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("surface b/roof can't be held")), "{notes:?}");
    // A name given to many items (OpenFOAM-8's `name`): noted once, each left out, the rest kept.
    let long = "n".repeat(1000);
    let mut geometry = String::from("k { type triSurfaceMesh; file \"k.stl\"; }\n");
    for i in 0..200 {
        geometry.push_str(&format!("\"s{i}.stl\" {{ type triSurfaceMesh; name {long}; }}\n"));
    }
    let text = format!(
        "geometry {{ {geometry} }}\ncastellatedMeshControls {{ refinementSurfaces {{ k {{ level (1 1); }} {long} {{ level (1 1); }} }} locationInMesh (0 0 0); }}\n"
    );
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    let (sn, notes) = import(&doc.root, "p", Some(umcf_core::OfVersion::V8)).unwrap();
    assert_eq!(sn.surfaces.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["k"], "{notes:?}");
    let twice: Vec<&String> = notes.iter().filter(|n| n.contains("names more than one surface or box")).collect();
    assert_eq!(twice.len(), 1);
    assert!(notes.iter().map(String::len).sum::<usize>() < 20_000, "the notes stay small");
}

#[test]
fn an_expansion_ratio_that_isnt_a_number_is_noted() {
    let text = r#"
ratio 1.3;
addLayers true;
geometry { b { type triSurfaceMesh; file "b.stl"; } }
castellatedMeshControls { refinementSurfaces { b { level (1 1); } } locationInMesh (0 0 0); }
addLayersControls { expansionRatio $ratio; layers { b { nSurfaceLayers 2; } } }
"#;
    let (sn, notes) = imported(text).unwrap();
    assert_eq!(sn.expansion, None);
    assert!(notes.iter().any(|n| n.contains("its expansionRatio isn't a number")), "{notes:?}");
}

#[test]
fn relative_sizes_given_as_any_switch_are_relative() {
    let text = r#"
addLayers true;
geometry { b { type triSurfaceMesh; file "b.stl"; } }
castellatedMeshControls { refinementSurfaces { b { level (1 1); } } locationInMesh (0 0 0); }
addLayersControls { relativeSizes y; finalLayerThickness 0.5; minThickness 0.25; layers { b { nSurfaceLayers 2; } } }
"#;
    let (sn, notes) = imported(text).unwrap();
    assert_eq!(sn.first_layer, None, "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("aren't what the entry writes") || n.contains("absolute")), "{notes:?}");
}

#[test]
fn the_surfaces_whose_features_a_dictionary_reads_are_read_from_it_alone() {
    // Two inside points (no entry can hold it): the features are read all the same, by
    // OpenFOAM-12's `file` and OpenFOAM-8's key.
    let text = r#"
geometry
{
    b { type triSurfaceMesh; file "b.stl"; }
    "t.stlb" { type triSurfaceMesh; name t; }
    hall { type searchableBox; min (0 0 0); max (1 1 1); }
}
castellatedMeshControls
{
    features ( { file "b.eMesh"; level 1; } { file "t.eMesh"; levels ((0 2)); } { file "b.eMesh"; level 3; } );
    refinementSurfaces { b { level (1 1); } }
    insidePoints ((0 0 0) (1 1 1));
}
"#;
    let doc = umcf_foam_dict::Document::parse(text).unwrap();
    assert!(import(&doc.root, "p", Some(umcf_core::OfVersion::V12)).is_err());
    assert_eq!(features_read(&doc.root), Ok(vec!["b.stl".to_string(), "t.stlb".to_string()]));
    // An .eMesh of no surface it names: not told.
    let text = text.replace("{ file \"b.eMesh\"; level 3; }", "{ file \"odd.eMesh\"; level 3; }");
    let doc = umcf_foam_dict::Document::parse(&text).unwrap();
    assert_eq!(features_read(&doc.root), Err(vec!["odd.eMesh".to_string()]));
    // What can't be read in full isn't taken as none, each for its own reason.
    let text = text.replace("odd.eMesh", "b.eMesh");
    let reason = |t: &str| -> String {
        let doc = umcf_foam_dict::Document::parse(t).unwrap();
        features_read(&doc.root).err().map(|e| e.join("; ")).unwrap_or_default()
    };
    assert_eq!(reason(&text), "");
    // OpenFOAM's header brings no features; another #include may.
    let header = "#inputSyntax slash;\n#includeEtc \"caseDicts/mesh/generation/snappyHexMeshDict.cfg\"\nmeshQualityControls { #include \"meshQualityDict\" }\n";
    assert_eq!(reason(&format!("{header}{text}")), "");
    for (from, to, why) in [
        ("geometry", "#include \"more\"\ngeometry", "an #include"),
        ("geometry", "$more\ngeometry", "an #include or a $macro"),
        ("features (", "#include \"more\"\n    features (", "an #include"),
        ("hall {", "$g\n    hall {", "an #include"),
        ("features (", "features $feats; unused (", "features that aren't a list"),
        ("{ file \"b.eMesh\"; level 3; }", "$f1", "isn't { file"),
        ("{ file \"b.eMesh\"; level 3; }", "{ level 3; }", "without a file"),
        ("{ file \"b.eMesh\"; level 3; }", "{ file \"b.eMesh\"; level 3; #include \"c\" }", "with an #include"),
        ("file \"b.stl\";", "file (b.stl);", "b.eMesh"),
        ("castellatedMeshControls\n{", "castellatedMeshControls { }\ncastellatedMeshControls\n{", "given twice"),
    ] {
        let t = text.replacen(from, to, 1);
        assert_ne!(t, text, "{from}");
        let r = reason(&t);
        assert!(r.contains(why), "{to}: {r}");
    }
    let many: String = (0..1001).map(|_| "{ file \"b.eMesh\"; level 1; } ").collect();
    let t = format!(
        "geometry {{ b {{ type triSurfaceMesh; file \"b.stl\"; }} }}\ncastellatedMeshControls {{ features ( {many}); }}"
    );
    assert!(reason(&t).contains("more than 1000 features"), "{}", reason(&t));
    let boxes: String =
        (0..1001).map(|i| format!("x{i} {{ type searchableBox; min (0 0 0); max (1 1 1); }} ")).collect();
    let t = format!("geometry {{ {boxes} }}\ncastellatedMeshControls {{ features ( ); }}");
    assert!(reason(&t).contains("more than 1000 surfaces and boxes"), "{}", reason(&t));
}
