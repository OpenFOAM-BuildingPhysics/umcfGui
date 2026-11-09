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

fn text(c: &Case) -> String {
    c.dict(PATH).unwrap().to_string()
}

/// The description read back, which must have no problems.
fn read(c: &Case) -> Description {
    let (d, problems) = Description::read(&Document::parse(&text(c)).unwrap());
    assert_eq!(problems, [], "{}", text(c));
    d
}

fn changed_lines(before: &str, after: &str) -> Vec<String> {
    let b: Vec<&str> = before.lines().collect();
    after.lines().filter(|l| !b.contains(l)).map(str::to_string).collect()
}

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
    assert!(after.contains("                // the wind comes from here\n                role side;\n                partner         ( air east );\n"), "{after}");
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

#[test]
fn a_solid_whose_materials_cant_all_be_seen_keeps_its_kind() {
    // Not a dictionary, or given twice (the reader takes the first, which has some).
    for materials in ["        materials $mats;\n", "        materials { brick { } }\n        materials { }\n"] {
        let mut c = described_with("        kind        solid;\n", &format!("        kind        solid;\n{materials}"));
        // street is the first solid the import writes; it has materials of its own as well.
        assert!(c.describe_region("street", RegionKind::Fluid).is_err(), "{materials}");
    }
}

#[test]
fn taking_a_region_out_lists_what_still_names_it() {
    let (_, mut c) = described();
    let rad = read(&c).radiation.clone().unwrap();
    let r = c.undescribe_region(&rad.region).unwrap();
    assert!(r.dangling.contains(&"radiation".to_string()), "{r:?}");
}

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
