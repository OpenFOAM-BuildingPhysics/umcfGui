use super::*;
use crate::description::PatchDesc;

fn patch(name: &str, role: Option<Role>, from: &[&str]) -> PatchDesc {
    PatchDesc { name: name.into(), role, partner: None, from: from.iter().map(|s| s.to_string()).collect(), kind: None }
}

fn region(
    name: &str,
    kind: RegionKind,
    mesh: Option<MeshBuild>,
    steps: Vec<RegionStep>,
    patches: Vec<PatchDesc>,
) -> RegionDesc {
    RegionDesc {
        name: name.into(),
        kind,
        mesh,
        steps,
        patches,
        materials: Vec::new(),
        blocks: None,
        mesh_box: None,
        snappy: None,
    }
}

fn root_mesh() -> Option<MeshBuild> {
    Some(MeshBuild::Root(vec![
        RootStep::BlockMesh("system/blockMeshDict".into()),
        RootStep::CreatePatch,
        RootStep::SnappyHexMesh("system/snappyHexMeshDict".into()),
    ]))
}

/// The 3×3 reference cases: the air and the vegetation meshed in constant/polyMesh with a
/// createPatch making the buildings' walls of blockMesh's defaultFaces, each solid its sides.
fn three() -> Description {
    let buildings = |partner: bool| PatchDesc {
        partner: partner.then(|| ("air".to_string(), "buildings".to_string())),
        ..patch("buildings", Some(Role::Coupled), &["defaultFaces"])
    };
    Description {
        regions: vec![
            region(
                "air",
                RegionKind::Fluid,
                root_mesh(),
                vec![RegionStep::ChangeDictionary],
                vec![patch("west", Some(Role::Inlet), &[]), buildings(false)],
            ),
            region("vegetation", RegionKind::Vegetation, root_mesh(), vec![], vec![buildings(true)]),
            region(
                "street",
                RegionKind::Solid,
                Some(MeshBuild::Block),
                vec![RegionStep::CreatePatch, RegionStep::ChangeDictionary],
                vec![patch("street", Some(Role::Coupled), &[]), patch("sides", Some(Role::Side), &["defaultFaces"])],
            ),
        ],
        ..Default::default()
    }
}

fn root(text: &str) -> umcf_foam_dict::Dict {
    umcf_foam_dict::Document::parse(text).unwrap().root
}

#[test]
fn the_patches_made_are_gathered_per_dictionary() {
    let t = targets(&three(), None).unwrap();
    assert_eq!(
        t,
        [
            Target {
                path: "system/street/createPatchDict".into(),
                regions: vec!["street".into()],
                made: vec![Made { patch: "sides".into(), kind: "wall".into(), from: vec!["defaultFaces".into()] }],
            },
            Target {
                path: ROOT_PATH.into(),
                regions: vec!["air".into(), "vegetation".into()],
                made: vec![Made { patch: "buildings".into(), kind: "wall".into(), from: vec!["defaultFaces".into()] }],
            },
        ]
    );
    // A region whose steps run createPatch with nothing to make: an empty list, as createPatch
    // takes it.
    let mut d = three();
    d.regions[2].patches[1].from.clear();
    let t = targets(&d, None).unwrap();
    assert_eq!(t[0].made, []);
    assert!(dict(&t[0], OfVersion::V12).contains("patches\n(\n);"));
    // Types by role: an open side's patch, a wall's and the canopy wall, a solid's always wall.
    let d = Description {
        regions: vec![
            region(
                "vegetation",
                RegionKind::Vegetation,
                None,
                vec![RegionStep::CreatePatch],
                vec![
                    patch("air_to_vegetation", Some(Role::Canopy), &["\"region1_to_.*\""]),
                    patch("sky", Some(Role::Sky), &["top"]),
                    patch("floor", Some(Role::Ground), &["bottom"]),
                ],
            ),
            region(
                "roofs",
                RegionKind::Solid,
                None,
                vec![RegionStep::CreatePatch],
                vec![patch("top", Some(Role::Top), &["a", "b"])],
            ),
        ],
        ..Default::default()
    };
    let t = targets(&d, None).unwrap();
    let kinds: Vec<Vec<&str>> = t.iter().map(|t| t.made.iter().map(|m| m.kind.as_str()).collect()).collect();
    assert_eq!(kinds, [vec!["wall", "patch", "wall"], vec!["wall"]]);
}

#[test]
fn what_cant_be_written_is_refused() {
    let refused = |f: &dyn Fn(&mut Description)| {
        let mut d = three();
        f(&mut d);
        targets(&d, None).unwrap_err()
    };
    let e = refused(&|d| d.regions[2].patches[1].role = None);
    assert!(
        e.contains("patch sides of region street is made by createPatch, but has neither a role nor a type"),
        "{e}"
    );
    // A type of its own, with a role or without, is the type made.
    for role in [None, Some(Role::Side)] {
        let mut d = three();
        d.regions[2].patches[1].role = role;
        d.regions[2].patches[1].kind = Some("symmetryPlane".into());
        assert_eq!(targets(&d, None).unwrap()[0].made[0].kind, "symmetryPlane");
    }
    let e = refused(&|d| d.regions[2].steps.clear());
    assert!(e.contains("the region's mesh and steps don't run createPatch"), "{e}");
    let e = refused(&|d| d.regions[0].steps.push(RegionStep::CreatePatch));
    assert!(e.contains("region air runs createPatch both in its mesh"), "{e}");
    // The two regions meshed in constant/polyMesh read one dictionary: the same patches.
    let e = refused(&|d| d.regions[1].patches[0].from = vec!["other".into()]);
    assert!(e.contains("regions air and vegetation are meshed in constant/polyMesh with createPatch"), "{e}");
    let e = refused(&|d| d.regions[1].patches.clear());
    assert!(e.contains("the patches they make (from) must be the same"), "{e}");
    // The same sources in another order are the same, as createPatch takes their union.
    let mut d = three();
    d.regions[0].patches[1].from = vec!["a".into(), "b".into()];
    d.regions[1].patches[0].from = vec!["b".into(), "a".into(), "a".into()];
    assert!(targets(&d, None).is_ok());
    let e = refused(&|d| d.regions[2].patches[1].name = "two words".into());
    assert!(e.contains("patch `two words` of region street isn't a plain OpenFOAM word"), "{e}");
    let e = refused(&|d| d.regions[2].name = "con".into());
    assert!(e.contains("region `con` can't be a directory on every system"), "{e}");
    // The same patches in another order: refused, as any two may take the same faces (a
    // name may be a patch group).
    let mut d = three();
    d.regions[0].patches.push(patch("roofs", Some(Role::Coupled), &["x"]));
    d.regions[1].patches.insert(0, patch("roofs", Some(Role::Coupled), &["x"]));
    assert!(targets(&d, None).unwrap_err().contains("in another order"));
}

#[test]
fn the_dictionary_is_written_for_each_version() {
    let t = targets(&three(), None).unwrap();
    let v12 = dict(&t[1], OfVersion::V12);
    let v8 = dict(&t[1], OfVersion::V8);
    assert!(crate::blocks::untouched(&v12) && crate::blocks::untouched(&v8));
    // OpenFOAM-8 stops without pointSync; OpenFOAM-12 doesn't read it.
    assert!(!v12.contains("pointSync") && v8.contains("pointSync false;"));
    assert!(v12.contains("location    \"system\";") && v12.contains("regions air, vegetation"));
    assert!(dict(&t[0], OfVersion::V12).contains("location    \"system/street\";"));
    for (text, v) in [(&v12, OfVersion::V12), (&v8, OfVersion::V8)] {
        assert_eq!(differences(&root(text), &t[1].made, v), Vec::<String>::new());
    }
    assert!(v12.contains(
        "        name            buildings;\n        patchInfo\n        {\n            type            wall;\n        }\n        constructFrom   patches;\n        patches         ( defaultFaces );\n"
    ));
    // A pattern is written quoted, as createPatch reads one.
    let mut d = three();
    d.regions[2].patches[1].from = vec!["\"region1_to_.*\"".into(), "air.roof".into()];
    let t = targets(&d, None).unwrap();
    assert!(dict(&t[0], OfVersion::V12).contains("patches         ( \"region1_to_.*\" air.roof );"));
}

#[test]
fn the_cases_own_is_compared_by_what_it_makes() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let diff = |text: &str, v| differences(&root(text), &made, v);
    // As the reference cases have it (their `set f0;` unread, pointSync given in both versions).
    let own = "pointSync false; patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); set f0; } );";
    assert_eq!(diff(own, OfVersion::V12), Vec::<String>::new());
    assert_eq!(diff(own, OfVersion::V8), Vec::<String>::new());
    assert_eq!(
        diff(&own.replace("pointSync false; ", ""), OfVersion::V8),
        ["it has no pointSync, without which OpenFOAM-8's createPatch stops"]
    );
    // A quoted plain name is the same patch.
    assert_eq!(diff(&own.replace("(defaultFaces)", "(\"defaultFaces\")"), OfVersion::V12), Vec::<String>::new());
    assert_eq!(
        diff(&own.replace("(defaultFaces)", "(defaultFaces other)"), OfVersion::V12),
        ["patch buildings is made of ( defaultFaces other ) there, the description makes it of ( defaultFaces )"]
    );
    assert_eq!(
        diff(&own.replace("type wall", "type patch"), OfVersion::V12),
        ["patch buildings: type is patch there, the description makes it wall"]
    );
    assert_eq!(
        diff(&own.replace("constructFrom patches", "constructFrom set"), OfVersion::V12),
        ["patch buildings is made of a set there, the description makes it of ( defaultFaces )"]
    );
    assert_eq!(
        diff(&own.replace("constructFrom patches; ", ""), OfVersion::V12),
        ["patch buildings has no constructFrom there, which createPatch stops without"]
    );
    assert_eq!(
        diff(&own.replace("name buildings", "name walls"), OfVersion::V12),
        [
            "patch buildings isn't made there, the description makes it of ( defaultFaces )",
            "patch walls is made there, the description doesn't make it (from)"
        ]
    );
    assert_eq!(diff("pointSync false;", OfVersion::V12), ["it has no patches list, without which createPatch stops"]);
    assert_eq!(diff("patches ( defaultFaces );", OfVersion::V12), ["its patches isn't a list of dictionaries"]);
    assert_eq!(diff("patches 3;", OfVersion::V12), ["its patches isn't a list of dictionaries"]);
    // What a macro or directive gives isn't compared.
    assert_eq!(diff("patches $list;", OfVersion::V12), Vec::<String>::new());
    assert_eq!(diff(&own.replace("(defaultFaces)", "($faces)"), OfVersion::V12), Vec::<String>::new());
    assert_eq!(diff(&own.replace("type wall", "type $t"), OfVersion::V12), Vec::<String>::new());
    let included = "patches ( { #include \"made\" } );";
    assert_eq!(diff(included, OfVersion::V12), Vec::<String>::new());
    // Nothing to make: an empty list is fine, none isn't.
    assert_eq!(differences(&root("pointSync false; patches ( );"), &[], OfVersion::V8), Vec::<String>::new());
    assert_eq!(
        differences(&root("pointSync false;"), &[], OfVersion::V8),
        ["it has no patches list, without which createPatch stops"]
    );
}

#[test]
fn a_source_keeps_whether_it_is_a_pattern() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let own = "pointSync false; patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); } );";
    let diff = |text: &str| differences(&root(text), &made, OfVersion::V12);
    // Quoted, a name with a regular expression's character is a pattern, not the name.
    let mut regex = made.clone();
    regex[0].from = vec!["region.1".into()];
    let quoted = own.replace("(defaultFaces)", "(\"region.1\")");
    assert_eq!(
        differences(&root(&quoted), &regex, OfVersion::V12),
        ["patch buildings is made of ( \"region.1\" ) there, the description makes it of ( region.1 )"]
    );
    regex[0].from = vec!["\"region.1\"".into()];
    assert_eq!(differences(&root(&quoted), &regex, OfVersion::V12), Vec::<String>::new());
    // A quoted name or type is the same.
    assert_eq!(
        diff(&own.replace("name buildings", "name \"buildings\"").replace("type wall", "type \"wall\"")),
        Vec::<String>::new()
    );
    // What the description holds: a bare word, or a quoted pattern without what would need escaping.
    for ok in ["defaultFaces", "air.roof", "\"region1_to_.*\"", "\"a|b\""] {
        assert!(source_ok(ok), "{ok}");
    }
    for bad in ["", "\"\"", "\"a\\.b\"", "\"a b\"", "a*", "\"a\"b\"", "\"open"] {
        assert!(!source_ok(bad), "{bad}");
    }
    assert_eq!(source_token("region1_to_.*"), "\"region1_to_.*\"");
    assert_eq!(source_token("defaultFaces"), "defaultFaces");
    assert_eq!(source_token("\"region.1\""), "\"region.1\"");
}

#[test]
fn what_a_macro_or_directive_gives_isnt_compared() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let diff = |text: &str| differences(&root(text), &made, OfVersion::V8);
    // At the top: pointSync and the list may come from it.
    assert_eq!(diff("#include \"common\""), Vec::<String>::new());
    assert_eq!(
        diff(
            "$common; patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); } );"
        ),
        Vec::<String>::new()
    );
    // In an item: compared by its name only.
    assert_eq!(diff("pointSync false; patches ( { name buildings; #include \"rest\" } );"), Vec::<String>::new());
    assert_eq!(diff("pointSync false; patches ( { name buildings; $rest; } );"), Vec::<String>::new());
}

#[test]
fn what_the_description_cant_hold_is_kept() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let kept = |text: &str| kept(&root(text), &made, OfVersion::V8);
    let own = "pointSync false; patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); } );";
    assert_eq!(kept(own), Vec::<String>::new());
    // Made of a set: the description can't hold it. The reference cases' `set` beside patches,
    // which createPatch doesn't read, is fine.
    assert_eq!(
        kept(&own.replace("constructFrom patches", "constructFrom set; set f0")),
        ["patch buildings, made of a set"]
    );
    assert_eq!(kept(&own.replace("(defaultFaces);", "(defaultFaces); set f0;")), Vec::<String>::new());
    assert_eq!(kept(&own.replace("constructFrom patches; ", "")), ["patch buildings, made of a what isn't said"]);
    assert_eq!(kept(&own.replace("(defaultFaces)", "(\"a b\")")), ["patch buildings's sources"]);
    assert_eq!(kept(&own.replace("(defaultFaces);", "(defaultFaces); extra 1;")), ["patch buildings's extra"]);
    assert_eq!(
        kept(&own.replace("patchInfo { type wall; }", "patchInfo wall;")),
        ["patch buildings's patchInfo, which isn't a dictionary"]
    );
    // Given twice: one patch of both, which written once loses the shape.
    let twice = own.replace(
        "(defaultFaces); } );",
        "(defaultFaces); } { name buildings; patchInfo { type wall; } constructFrom patches; patches (x); } );",
    );
    assert_eq!(kept(&twice), ["patch buildings, given twice (one item of both, by hand)"]);
    assert_eq!(
        differences(&root(&twice), &made, OfVersion::V8),
        ["patch buildings is made of ( defaultFaces x ) there, the description makes it of ( defaultFaces )"]
    );
    let mut both = made.clone();
    both[0].from = vec!["x".into(), "defaultFaces".into()];
    assert_eq!(differences(&root(&twice), &both, OfVersion::V8), Vec::<String>::new());
    // Other things at the top.
    assert_eq!(kept(&format!("{own} 3")).len(), 1);
    assert_eq!(kept(&format!("patches ( ); {own}")), ["its patches, given twice"]);
    // Long lists are cut short.
    let keys: String = (0..100).map(|i| format!("k{i} 1; ")).collect();
    let long = kept(&own.replace("type wall;", &format!("type wall; {keys}")));
    assert_eq!(long, ["patch buildings's patchInfo k0 k1 k2 k3 k4 … and 95 more"]);
    let many: String = (0..1000).map(|i| format!("{{ name p{i}; constructFrom patches; patches (a); }} ")).collect();
    let many = kept(&format!("pointSync false; patches ( {many} );"));
    assert_eq!(many.len(), 21);
    assert_eq!(many.last().map(String::as_str), Some("and more"));
    let more = own.replace(
        "(defaultFaces); } );",
        "(defaultFaces); } { name cyc; patchInfo { type cyclic; neighbourPatch cyc2; } constructFrom patches; patches (a); } );",
    );
    assert_eq!(kept(&more), ["patch cyc"]);
    assert_eq!(
        kept(&own.replace("type wall;", "type wall; inGroups (walls);")),
        ["patch buildings's patchInfo inGroups"]
    );
    assert_eq!(
        kept(&own.replace("name buildings;", "name buildings; $rest;")),
        ["patch buildings's macro or directive"]
    );
    assert_eq!(kept(&own.replace("name buildings;", "name $n;")), ["a patch without a plain name"]);
    assert_eq!(
        kept("#include \"made\""),
        ["a directive or macro at its top", "its patches given by a macro or directive"]
    );
    // A list that can't be read, a value from a macro, other entries at the top.
    assert_eq!(kept("pointSync false; patches 2 ( { name cyc; } );"), ["its patches list, which can't be read whole"]);
    assert_eq!(kept("pointSync false; patches 1 ( { name cyc; } );"), ["patch cyc"]);
    assert_eq!(kept("pointSync false; patches ( cyc );"), ["its patches list, which can't be read whole"]);
    assert_eq!(
        kept(&own.replace("patchInfo { type wall; }", "patchInfo $wallInfo;")),
        ["patch buildings's macro or directive"]
    );
    assert_eq!(kept(&own.replace("type wall;", "type $t;")), ["patch buildings's macro or directive"]);
    assert_eq!(kept(&own.replace("(defaultFaces)", "($faces)")), ["patch buildings's macro or directive"]);
    assert_eq!(kept(&own.replace("pointSync false;", "pointSync true;")), ["its pointSync, which isn't off"]);
    assert_eq!(
        kept(&own.replace("pointSync false;", "pointSync off; writeCyclicMatch true;")),
        ["its writeCyclicMatch"]
    );
    // OpenFOAM-12 doesn't read pointSync: dropped, nothing is lost.
    assert_eq!(
        super::kept(&root(&own.replace("pointSync false;", "pointSync true;")), &made, OfVersion::V12),
        Vec::<String>::new()
    );
}

#[test]
fn sources_are_a_union_and_quoted_names_follow_openfoam() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let mut two = made.clone();
    two[0].from = vec!["a".into(), "b".into()];
    let own = |from: &str| {
        format!(
            "patches ( {{ name buildings; patchInfo {{ type wall; }} constructFrom patches; patches ({from}); }} );"
        )
    };
    let diff = |text: &str| differences(&root(text), &two, OfVersion::V12);
    // createPatch takes their union: order and repeats don't matter.
    assert_eq!(diff(&own("b a a")), Vec::<String>::new());
    assert_eq!(diff(&own("a")), ["patch buildings is made of ( a ) there, the description makes it of ( a b )"]);
    // Quoted: a name OpenFOAM doesn't take for a pattern is the name.
    assert_eq!(diff(&own("\"a\" b")), Vec::<String>::new());
    assert_eq!(diff(&own("\"a*\" b")).len(), 1);
    assert_eq!(
        diff("patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches a; } );"),
        ["patch buildings is made of what isn't a list of names there, the description makes it of ( a b )"]
    );
    // constructFrom quoted is the word.
    assert_eq!(diff(&own("a b").replace("constructFrom patches", "constructFrom \"patches\"")), Vec::<String>::new());
}

#[test]
fn one_region_is_written_without_the_others() {
    let mut d = three();
    // The air and the vegetation share system/createPatchDict but make other patches.
    d.regions[1].patches[0].from = vec!["other".into()];
    assert!(targets(&d, None).is_err());
    assert!(targets(&d, Some("air")).is_err());
    let t = targets(&d, Some("street")).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].path, "system/street/createPatchDict");
    // A region running createPatch both in and after its mesh, with nothing to make: both read.
    let mut d = three();
    d.regions[0].steps.push(RegionStep::CreatePatch);
    d.regions[0].patches[1].from.clear();
    d.regions[1].patches[0].from.clear();
    let paths: Vec<String> = targets(&d, None).unwrap().into_iter().map(|t| t.path).collect();
    assert_eq!(paths, ["system/air/createPatchDict", "system/street/createPatchDict", ROOT_PATH]);
}

#[test]
fn keys_given_twice_are_kept_and_summaries_count() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let own = "pointSync false; patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); } );";
    // OpenFOAM merges a sub-dictionary given twice: what the first holds would be lost.
    let twice = own.replace("patchInfo { type wall; }", "patchInfo { inGroups (g); } patchInfo { type wall; }");
    assert_eq!(
        kept(&root(&twice), &made, OfVersion::V8),
        ["patch buildings's keys given twice (OpenFOAM merges them)"]
    );
    let twice = own.replace("type wall;", "type wall; type wall;");
    assert_eq!(
        kept(&root(&twice), &made, OfVersion::V8),
        ["patch buildings's keys given twice (OpenFOAM merges them)"]
    );
    // A patch the description doesn't make, given twice, is said once.
    let other = own.replace("(defaultFaces); } );", "(defaultFaces); } { name x; constructFrom patches; patches (a); } { name x; constructFrom patches; patches (b); } );");
    assert_eq!(
        differences(&root(&other), &made, OfVersion::V8),
        ["patch x is made there, the description doesn't make it (from)"]
    );
    assert_eq!(summary(&[]), "");
    assert_eq!(summary(&["a".into()]), "a");
    assert_eq!(summary(&["a".into(), "b".into(), "c".into()]), "a, and 2 more");
    assert_eq!(summary(&["a".into(), "b".into(), "and more".into()]), "a, and more");
}

#[test]
fn counted_lists_switches_and_missing_types_are_read_as_openfoam_does() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let diff = |text: &str, v| differences(&root(text), &made, v);
    let own = "pointSync false; patches 1 ( { name buildings; patchInfo { type wall; } constructFrom patches; patches 1 (defaultFaces); } );";
    assert_eq!(diff(own, OfVersion::V8), Vec::<String>::new());
    assert_eq!(kept(&root(own), &made, OfVersion::V8), Vec::<String>::new());
    assert_eq!(
        diff(&own.replace("patches 1 ( {", "patches 2 ( {"), OfVersion::V12),
        ["its patches list's count isn't the number of its items, at which OpenFOAM stops"]
    );
    assert_eq!(
        diff(&own.replace("pointSync false", "pointSync banana"), OfVersion::V8),
        ["its pointSync isn't a switch (true or false), at which OpenFOAM-8's createPatch stops"]
    );
    assert_eq!(diff(&own.replace("pointSync false", "pointSync $sync"), OfVersion::V8), Vec::<String>::new());
    // A quoted string isn't a switch to OpenFOAM-8, nor off.
    let quoted = own.replace("pointSync false", "pointSync \"false\"");
    assert_eq!(
        diff(&quoted, OfVersion::V8),
        ["its pointSync isn't a switch (true or false), at which OpenFOAM-8's createPatch stops"]
    );
    assert_eq!(kept(&root(&quoted), &made, OfVersion::V8), ["its pointSync, which isn't off"]);
    assert_eq!(diff(&own.replace("pointSync false", "pointSync banana"), OfVersion::V12), Vec::<String>::new());
    assert_eq!(
        diff(&own.replace("{ type wall; }", "{ }"), OfVersion::V12),
        [
            "patch buildings has no type there (patchInfo), which createPatch needs for a new patch; the description makes it wall"
        ]
    );
}

#[test]
fn the_order_decides_where_shared_faces_go() {
    let two = |names: [&str; 2]| {
        let mut d = three();
        d.regions.truncate(2);
        for r in &mut d.regions {
            r.patches = names
                .iter()
                .map(|n| PatchDesc { kind: Some("wall".into()), ..patch(n, None, &["defaultFaces"]) })
                .collect();
        }
        d
    };
    assert!(targets(&two(["roof", "wall"]), None).is_ok());
    let mut d = two(["roof", "wall"]);
    d.regions[1].patches.reverse();
    let e = targets(&d, None).unwrap_err();
    assert!(e.contains("they make roof and wall in another order"), "{e}");
    // Sources by other names: a name may be a patch group holding the other, so any order
    // other than the same is refused too.
    let mut d = two(["roof", "wall"]);
    for r in &mut d.regions {
        r.patches[1].from = vec!["facades".into()];
    }
    assert!(targets(&d, None).is_ok());
    d.regions[1].patches.reverse();
    assert!(targets(&d, None).unwrap_err().contains("they make roof and wall in another order"));
    // The case's own in the other order: a difference, not what the description can't hold.
    let made = targets(&two(["roof", "wall"]), None).unwrap().remove(0).made;
    let item =
        |n: &str| format!("{{ name {n}; patchInfo {{ type wall; }} constructFrom patches; patches (defaultFaces); }}");
    let other = format!("patches ( {} {} );", item("wall"), item("roof"));
    assert_eq!(
        differences(&root(&other), &made, OfVersion::V12),
        [
            "patches wall and roof are made in the other order there, or one is given again around the other (createPatch gives a face to the last that takes it, and any two may take the same faces: a source both name, a pattern, a patch group)"
        ]
    );
    assert_eq!(kept(&root(&other), &made, OfVersion::V12), Vec::<String>::new());
    let same = format!("patches ( {} {} );", item("roof"), item("wall"));
    assert_eq!(differences(&root(&same), &made, OfVersion::V12), Vec::<String>::new());
    // A name given again right after itself: one item.
    let twice = format!("patches ( {} {} {} );", item("roof"), item("roof"), item("wall"));
    assert_eq!(differences(&root(&twice), &made, OfVersion::V12), Vec::<String>::new());
    // A name given again around the other: the faces both take may go elsewhere, whatever the
    // places of each name's first and last items.
    let again = format!("patches ( {} {} {} );", item("roof"), item("wall"), item("roof"));
    assert_eq!(differences(&root(&again), &made, OfVersion::V12).len(), 1);
    let at =
        |n: &str, s: &str| format!("{{ name {n}; patchInfo {{ type wall; }} constructFrom patches; patches ({s}); }}");
    let made = vec![
        Made { patch: "roof".into(), kind: "wall".into(), from: vec!["x".into(), "y".into()] },
        Made { patch: "wall".into(), kind: "wall".into(), from: vec!["y".into(), "z".into()] },
    ];
    let around =
        format!("patches ( {} {} {} {} );", at("roof", "x"), at("wall", "y"), at("roof", "y"), at("wall", "z"));
    let d = differences(&root(&around), &made, OfVersion::V12);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with("patches wall and roof are made in the other order there"), "{d:?}");
    // An empty list moves no faces, which the description can't say: kept.
    let empty = format!("patches ( {} {} );", at("roof", ""), at("wall", "y z"));
    assert_eq!(kept(&root(&empty), &made, OfVersion::V12), ["patch roof's sources"]);
    // Many patches, in one pass: the other order found wherever it is.
    let many = |swap: Option<(usize, usize)>| {
        let mut d = three();
        d.regions.truncate(2);
        for (k, r) in d.regions.iter_mut().enumerate() {
            r.patches = (0..20_000)
                .map(|i| PatchDesc { kind: Some("wall".into()), ..patch(&format!("p{i}"), None, &[&format!("s{i}")]) })
                .collect();
            if let Some((i, j)) = swap
                && k == 1
            {
                r.patches.swap(i, j);
            }
        }
        d
    };
    assert!(targets(&many(None), None).is_ok());
    assert!(targets(&many(Some((300, 301))), None).unwrap_err().contains("they make p300 and p301 in another order"));
    assert!(targets(&many(Some((0, 19_999))), None).is_err());
}

#[test]
fn a_header_of_its_own_is_kept_and_long_lists_are_capped() {
    let made = targets(&three(), None).unwrap().remove(1).made;
    let own = "FoamFile { version 2.0; format ascii; class dictionary; object createPatchDict; note \"mine\"; } patches ( { name buildings; patchInfo { type wall; } constructFrom patches; patches (defaultFaces); } );";
    assert_eq!(kept(&root(own), &made, OfVersion::V12), ["its FoamFile note"]);
    let header: String = (0..1000).map(|i| format!("k{i} 1; ")).collect();
    let k = kept(&root(&format!("FoamFile {{ {header} }} patches ( );")), &[], OfVersion::V12);
    assert_eq!(k.len(), MAX_KEPT + 1);
    let many: String = (0..1000).map(|i| format!("k{i} 1; ")).collect();
    let k = kept(&root(&format!("{many} patches ( );")), &[], OfVersion::V12);
    assert_eq!(k.len(), MAX_KEPT + 1);
    assert_eq!(k.last().map(String::as_str), Some("and more"));
}

#[test]
fn patterns_that_may_not_compile_are_refused() {
    for ok in [
        "defaultFaces",
        "\"defaultFaces\"",
        "\"region1_to_.*\"",
        "\"(a|b)_wall\"",
        "\"[a-c]x+\"",
        "\"wall.?\"",
        "\"(?i)wall.*\"",
        "\"wall[[:digit:]]+\"",
        "\"[^[:space:]a-c]x\"",
        "\"wall[a-c-]\"",
        "\"[^a-c-]\"",
    ] {
        assert!(source_ok(ok), "{ok}");
    }
    for bad in [
        "\"[\"",
        "\"(a\"",
        "\"a)\"",
        "\"*a\"",
        "\"a**\"",
        "\"[]\"",
        "\"[z-a]\"",
        "\"(?:a)\"",
        "\"a{2}\"",
        "\"|*\"",
        "\"(?i)\"",
        "\"(?i)*\"",
        "\"[a-c-e]\"",
        "\"[a-c--]\"",
        "\"[[:digits:]]\"",
        "\"[[:digit:]\"",
        "\"[[:digit]]\"",
        "\"[[=a=]]\"",
        "\"[a-[:digit:]]\"",
    ] {
        assert!(!source_ok(bad), "{bad}");
    }
    assert_eq!(source_token("["), "\"[\"");
    assert!(!source_ok(&source_token("[")));
}
