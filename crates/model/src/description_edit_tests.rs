use umcf_core::MemFs;
use umcf_foam_dict::Document;

use super::*;
use crate::description::tests::canyon;
use crate::description::{Description, PATH, Role};

/// The canyon with its description written by the import and a comment added by hand.
fn described() -> (MemFs, Case) {
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    let (d, _) = Description::import(&c);
    let text = d.write().replace("regions\n{\n", "regions\n{\n    // kept by every edit\n");
    c.put_description(&fs, &text, false).unwrap();
    (fs, c)
}

/// The description's text as the case holds it.
fn text(c: &Case) -> String {
    c.dict(PATH).unwrap().to_string()
}

/// The description read back, which must have no problems.
fn read(c: &Case) -> Description {
    let (d, problems) = Description::read(&Document::parse(&text(c)).unwrap());
    assert_eq!(problems, [], "{}", text(c));
    d
}

/// The lines of `after` that `before` doesn't have.
fn changed_lines(before: &str, after: &str) -> Vec<String> {
    let b: Vec<&str> = before.lines().collect();
    after.lines().filter(|l| !b.contains(l)).map(str::to_string).collect()
}

/// Setting a role rewrites only that patch's line.
#[test]
fn a_role_changes_one_line_and_keeps_the_rest() {
    let (_, mut c) = described();
    let before = text(&c);
    let r =
        c.describe_patch("air", "odd", &PatchEdit { role: Some(Some(Role::Side)), ..PatchEdit::default() }).unwrap();
    assert!(!r.added);
    let after = text(&c);
    assert!(after.contains("// kept by every edit"));
    assert_eq!(changed_lines(&before, &after), ["            odd             { role side; }"], "{after}");
    assert_eq!(after.lines().count(), before.lines().count());
    let d = read(&c);
    assert_eq!(d.region("air").unwrap().patches.iter().find(|p| p.name == "odd").unwrap().role, Some(Role::Side));
    // The role taken away again.
    c.describe_patch("air", "odd", &PatchEdit { role: Some(None), ..PatchEdit::default() }).unwrap();
    assert_eq!(read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "odd").unwrap().role, None);
}

/// A patch the region doesn't list is added, with or without a role or partner.
#[test]
fn a_patch_is_added_and_partnered() {
    let (_, mut c) = described();
    let edit = PatchEdit {
        role: Some(Some(Role::Coupled)),
        partner: Some(Some(("street".into(), "roof".into()))),
        rename: None,
    };
    assert!(c.describe_patch("air", "roof", &edit).unwrap().added);
    let p = read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "roof").cloned().unwrap();
    assert_eq!((p.role, p.partner), (Some(Role::Coupled), Some(("street".into(), "roof".into()))));
    c.describe_patch("air", "roof", &PatchEdit { partner: Some(None), ..PatchEdit::default() }).unwrap();
    assert_eq!(read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "roof").unwrap().partner, None);
    // Only a partner given, to a patch the region doesn't list: added with it.
    let only = PatchEdit { partner: Some(Some(("air".into(), "x".into()))), ..PatchEdit::default() };
    assert!(c.describe_patch("street", "y", &only).unwrap().added);
    // A role taken from a patch that isn't there adds a patch without one.
    assert!(c.describe_patch("street", "z", &PatchEdit { role: Some(None), ..PatchEdit::default() }).unwrap().added);
    assert!(read(&c).region("street").unwrap().patches.iter().any(|p| p.name == "z" && p.role.is_none()));
}

/// A rename changes the partners that named the patch; a clash or a missing patch changes nothing.
#[test]
fn a_rename_is_followed_by_the_partners_that_named_it() {
    let (_, mut c) = described();
    let d = read(&c);
    let naming: Vec<String> = d
        .regions
        .iter()
        .flat_map(|r| {
            r.patches
                .iter()
                .filter(|p| p.partner == Some(("air".into(), "air_to_street".into())))
                .map(move |p| format!("{}/{}", r.name, p.name))
        })
        .collect();
    assert!(!naming.is_empty(), "the canyon couples to air/air_to_street");
    let r = c
        .describe_patch(
            "air",
            "air_to_street",
            &PatchEdit { rename: Some("air_to_road".into()), ..PatchEdit::default() },
        )
        .unwrap();
    assert_eq!(r.followed, naming);
    let d = read(&c);
    let air = d.region("air").unwrap();
    assert!(
        air.patches.iter().any(|p| p.name == "air_to_road") && !air.patches.iter().any(|p| p.name == "air_to_street")
    );
    for n in &naming {
        let (rn, pn) = n.split_once('/').unwrap();
        let p = d.region(rn).unwrap().patches.iter().find(|p| p.name == pn).unwrap();
        assert_eq!(p.partner, Some(("air".into(), "air_to_road".into())), "{n}");
    }
    assert!(text(&c).contains("// kept by every edit"));
    // Onto a name the region has, or a patch it doesn't list: refused, nothing changed.
    let before = text(&c);
    let onto = PatchEdit { rename: Some("west".into()), ..PatchEdit::default() };
    assert!(c.describe_patch("air", "air_to_road", &onto).is_err());
    assert!(
        c.describe_patch("air", "nothere", &PatchEdit { rename: Some("x".into()), ..PatchEdit::default() }).is_err()
    );
    assert_eq!(text(&c), before);
}

/// Regions are added, given another kind and taken out; a solid with materials stays one.
#[test]
fn regions_are_added_given_a_kind_and_taken_out() {
    let (_, mut c) = described();
    assert!(c.describe_region("roofs", RegionKind::Solid).unwrap().added);
    assert_eq!(read(&c).region("roofs").map(|r| r.kind), Some(RegionKind::Solid));
    assert!(!c.describe_region("roofs", RegionKind::Fluid).unwrap().added);
    assert_eq!(read(&c).region("roofs").map(|r| r.kind), Some(RegionKind::Fluid));
    assert!(c.describe_region("x", RegionKind::Single).is_err());
    // A solid with materials stays one.
    assert!(!read(&c).region("street").unwrap().materials.is_empty());
    assert!(c.describe_region("street", RegionKind::Fluid).is_err());
    // Out: the patches coupled to it are listed.
    let r = c.undescribe_region("street").unwrap();
    assert!(read(&c).region("street").is_none());
    assert!(r.dangling.contains(&"air/air_to_street".to_string()), "{r:?}");
    assert!(c.undescribe_region("street").is_err());
}

/// A solid's materials are added, given or lose a model, and are taken out; other kinds have none.
#[test]
fn a_solids_materials_are_added_given_a_model_and_taken_out() {
    let (_, mut c) = described();
    assert!(c.describe_material("street", "render", Some(Some("Hamstad5Brick"))).unwrap().added);
    let m = |c: &Case| {
        read(c)
            .region("street")
            .unwrap()
            .materials
            .iter()
            .map(|m| (m.name.clone(), m.model.clone()))
            .collect::<Vec<_>>()
    };
    assert!(m(&c).contains(&("render".into(), Some("Hamstad5Brick".into()))));
    c.describe_material("street", "render", Some(None)).unwrap();
    assert!(m(&c).contains(&("render".into(), None)));
    assert!(c.describe_material("street", "glass", None).unwrap().added);
    assert!(m(&c).contains(&("glass".into(), None)));
    c.undescribe_material("street", "glass").unwrap();
    assert!(!m(&c).iter().any(|(n, _)| n == "glass"));
    assert!(c.undescribe_material("street", "glass").is_err());
    // Only a solid has materials.
    assert!(c.describe_material("air", "brick", None).is_err());
    assert!(c.describe_material("nowhere", "brick", None).is_err());
}

/// No description, names that need quotes, nothing to change, a key given twice: refused.
#[test]
fn what_cant_be_edited_is_refused_and_changes_nothing() {
    // No description.
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    let e = c.describe_region("air", RegionKind::Fluid).unwrap_err().to_string();
    assert!(e.contains("workflow.import"), "{e}");
    // Names that need quotes; nothing to change.
    let (_, mut c) = described();
    let before = text(&c);
    for bad in ["", "with space", "q\"x", "1st", "-x", "a;b", "{x}"] {
        assert!(c.describe_region(bad, RegionKind::Solid).is_err(), "{bad}");
        assert!(c.describe_material("street", bad, None).is_err(), "{bad}");
        assert!(
            c.describe_patch("air", "west", &PatchEdit { rename: Some(bad.into()), ..PatchEdit::default() }).is_err()
        );
    }
    assert!(c.describe_patch("air", "west", &PatchEdit::default()).is_err());
    assert!(
        c.describe_patch("nowhere", "west", &PatchEdit { role: Some(Some(Role::Side)), ..PatchEdit::default() })
            .is_err()
    );
    assert_eq!(text(&c), before);
    // A region given twice: the reader takes the first, an edit would change the last.
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    let (d, _) = Description::import(&c);
    let twice = d.write().replace("regions\n{\n", "regions\n{\n    air { kind solid; }\n");
    c.put_description(&fs, &twice, false).unwrap();
    let e = c.describe_region("air", RegionKind::Fluid).unwrap_err().to_string();
    assert!(e.contains("`regions/air` is given 2 times"), "{e}");
}

/// A patch over several lines is edited key by key, its comment kept.
#[test]
fn a_patch_laid_out_by_hand_keeps_its_comments() {
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    let (d, _) = Description::import(&c);
    let text = d.write().replace(
        "            west            { role inlet; }\n",
        "            west\n            {\n                // the wind comes from here\n                role inlet;\n            }\n",
    );
    assert!(text.contains("// the wind comes from here"));
    c.put_description(&fs, &text, false).unwrap();
    let edit =
        PatchEdit { role: Some(Some(Role::Side)), partner: Some(Some(("air".into(), "east".into()))), rename: None };
    c.describe_patch("air", "west", &edit).unwrap();
    let after = text_of(&c);
    assert!(after.contains("                // the wind comes from here\n                role side;\n                partner ( air east );\n"), "{after}");
    let p = read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "west").cloned().unwrap();
    assert_eq!((p.role, p.partner), (Some(Role::Side), Some(("air".into(), "east".into()))));
    // The partner taken away; the role, with the comment in front of it, stays.
    c.describe_patch("air", "west", &PatchEdit { partner: Some(None), ..PatchEdit::default() }).unwrap();
    assert!(
        text_of(&c).contains("// the wind comes from here\n                role side;\n            }"),
        "{}",
        text_of(&c)
    );
    assert_eq!(read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "west").unwrap().partner, None);
}

/// The description's text (as `text`).
fn text_of(c: &Case) -> String {
    text(c)
}

/// The canyon described, with `from` in the written file replaced by `to`.
fn described_with(from: &str, to: &str) -> Case {
    let fs = canyon();
    let mut c = Case::open(&fs).unwrap();
    let (d, _) = Description::import(&c);
    let text = d.write();
    assert!(text.contains(from), "{from}");
    c.put_description(&fs, &text.replacen(from, to, 1), false).unwrap();
    c
}

/// What the reader can't read in a one-line patch, a quoted name and a macro survive an edit.
#[test]
fn what_the_reader_cant_read_in_a_patch_is_kept() {
    // A role it can't read, kept when only the partner changes; a quoted partner kept when only
    // the role changes; a key it doesn't know kept.
    let mut c = described_with("west            { role inlet; }", "west            { role $inletRole; colour blue; }");
    let edit = PatchEdit { partner: Some(Some(("air".into(), "east".into()))), ..PatchEdit::default() };
    c.describe_patch("air", "west", &edit).unwrap();
    assert!(
        text(&c).contains("west            { role $inletRole; colour blue; partner ( air east ); }"),
        "{}",
        text(&c)
    );
    let mut c = described_with(
        "air_to_street   { role coupled; partner ( street street_to_air ); }",
        "air_to_street   { role coupled; partner ( street \"street to air\" ); }",
    );
    c.describe_patch("air", "air_to_street", &PatchEdit { role: Some(Some(Role::Ground)), ..PatchEdit::default() })
        .unwrap();
    assert!(
        text(&c).contains("air_to_street   { role ground; partner ( street \"street to air\" ); }"),
        "{}",
        text(&c)
    );
    // A macro in the entry: edited key by key, the macro kept.
    let mut c = described_with("west            { role inlet; }", "west            { $base; role inlet; }");
    c.describe_patch("air", "west", &PatchEdit { role: Some(Some(Role::Side)), ..PatchEdit::default() }).unwrap();
    let t = text(&c);
    assert!(t.contains("$base;") && t.contains("role side;") && !t.contains("role inlet"), "{t}");
    // A material's quoted model kept when it is only listed again.
    let mut c = described_with("brick           { model Hamstad5Brick; }", "brick           { model \"My Brick\"; }");
    c.describe_material("street", "brick", None).unwrap();
    assert!(text(&c).contains("brick           { model \"My Brick\"; }"), "{}", text(&c));
}

/// A rename onto its own name is refused; a patch coupled to itself follows its rename.
#[test]
fn a_rename_onto_itself_is_refused_and_a_self_partner_follows() {
    let (_, mut c) = described();
    let same = PatchEdit { rename: Some("west".into()), ..PatchEdit::default() };
    assert!(c.describe_patch("air", "west", &same).unwrap_err().to_string().contains("has that name already"));
    let selfish = PatchEdit { partner: Some(Some(("air".into(), "odd".into()))), ..PatchEdit::default() };
    c.describe_patch("air", "odd", &selfish).unwrap();
    let r = c.describe_patch("air", "odd", &PatchEdit { rename: Some("even".into()), ..PatchEdit::default() }).unwrap();
    assert!(r.followed.contains(&"air/even".to_string()), "{r:?}");
    let p = read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "even").cloned().unwrap();
    assert_eq!(p.partner, Some(("air".into(), "even".into())));
}

/// A rename whose follower can't be edited is refused before anything changes.
#[test]
fn a_rename_that_cant_follow_changes_nothing() {
    // A follower's region given twice: the reader takes the first, which names the patch.
    let mut c = described_with(
        "regions\n{\n",
        "regions\n{\n    shade { kind vegetation; patches { a { role coupled; partner ( air air_to_street ); } } }\n    shade { kind vegetation; }\n",
    );
    let before = text(&c);
    let e = c.describe_patch("air", "air_to_street", &PatchEdit { rename: Some("x".into()), ..PatchEdit::default() });
    assert!(e.unwrap_err().to_string().contains("given 2 times"));
    assert_eq!(text(&c), before);
}

/// A solid whose materials aren't one dictionary keeps its kind.
#[test]
fn a_solid_whose_materials_cant_all_be_seen_keeps_its_kind() {
    // Not a dictionary, or given twice (the reader takes the first, which has some).
    for materials in ["        materials $mats;\n", "        materials { brick { } }\n        materials { }\n"] {
        let mut c = described_with("        kind        solid;\n", &format!("        kind        solid;\n{materials}"));
        // street is the first solid the import writes; it has materials of its own as well.
        assert!(c.describe_region("street", RegionKind::Fluid).is_err(), "{materials}");
    }
}

/// Taking the radiation's region out lists the radiation.
#[test]
fn taking_a_region_out_lists_what_still_names_it() {
    let (_, mut c) = described();
    let rad = read(&c).radiation.clone().unwrap();
    let r = c.undescribe_region(&rad.region).unwrap();
    assert!(r.dangling.contains(&"radiation".to_string()), "{r:?}");
}

/// A patch or material with an entry ending without `;` is refused, one-line or not.
#[test]
fn an_entry_without_its_semicolon_is_not_added_to() {
    let mut c = described_with("west            { role inlet; }", "west            { role inlet }");
    let before = text(&c);
    let edit = PatchEdit { partner: Some(Some(("air".into(), "east".into()))), ..PatchEdit::default() };
    let e = c.describe_patch("air", "west", &edit).unwrap_err().to_string();
    assert!(e.contains("has no `;`"), "{e}");
    assert_eq!(text(&c), before);
    // Laid out by hand over lines, and a material: refused too.
    let mut c = described_with(
        "west            { role inlet; }",
        "west\n            {\n                role inlet\n            }",
    );
    let before = text(&c);
    assert!(c.describe_patch("air", "west", &edit).unwrap_err().to_string().contains("has no `;`"));
    assert_eq!(text(&c), before);
    let mut c = described_with("brick           { model Hamstad5Brick; }", "brick           { model Hamstad5Brick }");
    let before = text(&c);
    assert!(c.describe_material("street", "brick", Some(None)).unwrap_err().to_string().contains("has no `;`"));
    assert_eq!(text(&c), before);
}

/// A patch coupled to itself, renamed while its partner is set or taken away, keeps that.
#[test]
fn a_self_coupled_patch_renamed_with_a_new_partner_keeps_it() {
    let (_, mut c) = described();
    let selfish = PatchEdit { partner: Some(Some(("air".into(), "odd".into()))), ..PatchEdit::default() };
    c.describe_patch("air", "odd", &selfish).unwrap();
    let edit =
        PatchEdit { rename: Some("even".into()), partner: Some(Some(("street".into(), "x".into()))), role: None };
    let r = c.describe_patch("air", "odd", &edit).unwrap();
    assert!(!r.followed.contains(&"air/even".to_string()), "{r:?}");
    let p = read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "even").cloned().unwrap();
    assert_eq!(p.partner, Some(("street".into(), "x".into())));
    // Coupled to itself again, then renamed with its partner taken away: none.
    let selfish = PatchEdit { partner: Some(Some(("air".into(), "even".into()))), ..PatchEdit::default() };
    c.describe_patch("air", "even", &selfish).unwrap();
    let edit = PatchEdit { rename: Some("third".into()), partner: Some(None), role: None };
    c.describe_patch("air", "even", &edit).unwrap();
    let p = read(&c).region("air").unwrap().patches.iter().find(|p| p.name == "third").cloned().unwrap();
    assert_eq!(p.partner, None);
}

/// An empty `materials { }` goes when the solid becomes another kind.
#[test]
fn an_empty_materials_goes_with_the_solid() {
    let (_, mut c) = described();
    c.describe_region("roofs", RegionKind::Solid).unwrap();
    c.set_inline(PATH, &["regions", "roofs", "materials"], "{ }").unwrap();
    c.describe_region("roofs", RegionKind::Fluid).unwrap();
    // `read` asserts the file has no problems: none about materials on a fluid region.
    assert_eq!(read(&c).region("roofs").map(|r| r.kind), Some(RegionKind::Fluid));
    // The roofs region holds no materials entry at all.
    let doc = Document::parse(&text(&c)).unwrap();
    assert!(doc.get(&["regions", "roofs", "materials"]).is_none(), "{}", text(&c));
}

/// A partner in a region the reader leaves out (its kind unreadable) follows a rename too.
#[test]
fn a_partner_the_reader_leaves_out_follows_a_rename() {
    let mut c = described_with(
        "regions\n{\n",
        "regions\n{\n    shade { kind $k; patches { a { role coupled; partner ( air air_to_street ); } } }\n",
    );
    let r = c
        .describe_patch("air", "air_to_street", &PatchEdit { rename: Some("x".into()), ..PatchEdit::default() })
        .unwrap();
    assert!(r.followed.contains(&"shade/a".to_string()), "{r:?}");
    assert!(text(&c).contains("a { role coupled; partner ( air x ); }"), "{}", text(&c));
    assert!(!text(&c).contains("air_to_street )"), "{}", text(&c));
}

/// A box changes key by key: a comment inside it stays, and the other way of cutting it into
/// cells goes.
#[test]
fn a_box_changes_key_by_key() {
    use crate::mesh_box::{BoxCells, MeshBox};
    let (fs, mut c) = described();
    let region = read(&c).regions[0].name.clone();
    let mut b = MeshBox {
        surfaces: vec!["a.stl".to_string()],
        margin: [1.0, 1.0, 1.0, 1.0, 0.0, 1.0],
        cells: BoxCells::Size(2.0),
        made: None,
    };
    c.describe_box(&region, &b).unwrap();
    assert_eq!(read(&c).region(&region).unwrap().mesh_box.as_ref(), Some(&b));
    // A comment by hand inside it, then the cells as counts.
    let t = text(&c).replace("surfaces ", "// my box\n            surfaces ");
    c.put_description(&fs, &t, true).unwrap();
    b.cells = BoxCells::Counts([3, 4, 5]);
    c.describe_box(&region, &b).unwrap();
    let after = text(&c);
    assert!(after.contains("// my box") && after.contains("( 3 4 5 )") && !after.contains("cellSize"), "{after}");
    assert!(after.contains("// kept by every edit"));
    assert_eq!(read(&c).region(&region).unwrap().mesh_box.as_ref(), Some(&b));
    // A box that isn't a dictionary, or doesn't read, isn't edited.
    let bad = text(&c).replacen("box\n", "box ( 1 2 );\n        boxed\n", 1);
    c.put_description(&fs, &bad, true).unwrap();
    assert!(c.describe_box(&region, &b).is_err());
    // Taken out: the region has none.
    let (_, mut c) = described();
    c.describe_box(&region, &b).unwrap();
    assert!(c.undescribe_box(&region).unwrap());
    assert!(!c.undescribe_box(&region).unwrap());
    assert!(read(&c).region(&region).unwrap().mesh_box.is_none());
}

/// A region's mesh is split from another's by a cell zone, and taken out again.
#[test]
fn a_region_is_split_from_another_and_the_split_taken_out() {
    use crate::workflow::{MeshBuild, Split};
    let (_, mut c) = described();
    c.describe_region("canopy", RegionKind::Vegetation).unwrap();
    let before = text(&c);
    let sp = |zone: &str, dict: &str| Split { of: "air".into(), zone: Some(zone.into()), dict: dict.into() };
    assert!(c.describe_split("canopy", &sp("trees", "system/air/topoSetDict")).unwrap().added);
    assert_eq!(changed_lines(&before, &text(&c)), ["        mesh            { from split; of air; zone trees; }"]);
    assert_eq!(read(&c).region("canopy").unwrap().mesh, Some(MeshBuild::Split(sp("trees", "system/air/topoSetDict"))));
    // Another dictionary is named; the comment by hand stays.
    assert!(!c.describe_split("canopy", &sp("canopy", "system/vegetation/topoSetDict")).unwrap().added);
    assert_eq!(
        read(&c).region("canopy").unwrap().mesh,
        Some(MeshBuild::Split(sp("canopy", "system/vegetation/topoSetDict")))
    );
    assert!(text(&c).contains("// kept by every edit"));
    // What can't be split is refused, nothing changed.
    let now = text(&c);
    for (region, split) in [
        ("canopy", sp("region1", "system/air/topoSetDict")),
        ("canopy", sp("trees", "../topoSetDict")),
        ("canopy", sp("trees", "topoSetDict")),
        ("canopy", sp("tr ees", "system/air/topoSetDict")),
        ("canopy", Split { of: "canopy".into(), zone: Some("trees".into()), dict: "system/a/b".into() }),
        ("canopy", Split { of: "air".into(), zone: None, dict: "system/a/b".into() }),
        ("roofs", sp("trees", "system/air/topoSetDict")),
    ] {
        assert!(c.describe_split(region, &split).is_err(), "{region} {split:?}");
        assert_eq!(text(&c), now);
    }
    c.undescribe_split("canopy").unwrap();
    assert_eq!(read(&c).region("canopy").unwrap().mesh, None);
    assert!(c.undescribe_split("canopy").is_err());
    // Another mesh isn't taken out by it.
    assert!(c.undescribe_split("street").is_err());
    // A region with blocks: a split mesh wouldn't read them.
    c.set_inline(PATH, &["regions", "canopy", "blocks"], "{ x ( 0 ( 1 1 ) 1 ); }").unwrap();
    let now = text(&c);
    assert!(c.describe_split("canopy", &sp("trees", "system/air/topoSetDict")).is_err());
    assert_eq!(text(&c), now);
}

/// A snappy entry's surfaces and boxes change one by one: a comment by hand in another surface
/// stays, as do the entries the change doesn't touch.
#[test]
fn a_snappy_surface_changes_alone() {
    use crate::snappy::{Mode, RefineBox, Snappy, Surface};
    let (fs, mut c) = described();
    let region = read(&c).regions[0].name.clone();
    let surface = |name: &str, level: u32| Surface {
        name: name.into(),
        file: format!("{name}.stl"),
        mode: Mode::Cut,
        level: Some((level, level)),
        distance: Vec::new(),
        layers: 0,
        features: None,
        solids: Vec::new(),
    };
    let mut sn = Snappy {
        inside: [1.0, 1.0, 1.0],
        surfaces: vec![surface("a", 1), surface("b", 1)],
        boxes: vec![RefineBox { name: "x".into(), min: [0.0; 3], max: [1.0; 3], level: 1 }],
        expansion: None,
        first_layer: None,
    };
    c.describe_snappy(&region, &sn).unwrap();
    assert_eq!(read(&c).region(&region).unwrap().snappy.as_ref(), Some(&sn));
    // Comments by hand in surface b and in the box.
    let t = text(&c).replace("b { mode cut;", "b { /* my b */ mode cut;").replace("x { min", "x { /* my box */ min");
    assert!(t.contains("/* my b */") && t.contains("/* my box */"), "{t}");
    c.put_description(&fs, &t, true).unwrap();
    // Surface a changes, one is added, the box stays: b's and the box's comments with them.
    sn.surfaces[0].level = Some((2, 3));
    sn.surfaces.push(surface("c", 1));
    c.describe_snappy(&region, &sn).unwrap();
    let after = text(&c);
    assert!(after.contains("/* my b */") && after.contains("/* my box */"), "{after}");
    assert_eq!(read(&c).region(&region).unwrap().snappy.as_ref(), Some(&sn));
    // Surface a taken out: b's comment still there.
    sn.surfaces.remove(0);
    c.describe_snappy(&region, &sn).unwrap();
    let after = text(&c);
    assert!(after.contains("/* my b */"), "{after}");
    assert_eq!(read(&c).region(&region).unwrap().snappy.as_ref(), Some(&sn));
    // Every box taken out: the key goes.
    sn.boxes.clear();
    c.describe_snappy(&region, &sn).unwrap();
    assert!(!text(&c).contains("boxes"), "{}", text(&c));
    assert_eq!(read(&c).region(&region).unwrap().snappy.as_ref(), Some(&sn));
}
