use umcf_core::{CaseFs, MemFs, OfVersion};

use super::*;
use crate::description::{Components, MaterialDesc};
use crate::new_case::{NewCase, NewRegion, Up, make};
use crate::settings::Settings;

const TP: &str = "constant/street/transportProperties";

fn case(v: OfVersion) -> (MemFs, Case) {
    let spec = NewCase {
        version: v,
        regions: vec![
            NewRegion { name: "air".into(), kind: RegionKind::Fluid, materials: vec![] },
            NewRegion {
                name: "street".into(),
                kind: RegionKind::Solid,
                materials: vec![
                    MaterialDesc { name: "soil".into(), model: Some("Soil".into()) },
                    MaterialDesc { name: "brick".into(), model: Some("Hamstad5Brick".into()) },
                ],
            },
        ],
        radiation: None,
        components: Components::default(),
        grass_patches: vec![],
        up: Up::Z,
        settings: Settings::default(),
    };
    let mut fs = MemFs::new();
    for (p, t) in make(&spec).unwrap().files {
        fs.insert(&p, t);
    }
    let c = Case::open(&fs).unwrap();
    (fs, c)
}

fn set(k: &str, v: &str) -> Coefficient {
    (k.split('/').map(str::to_string).collect(), Some(v.to_string()))
}

/// The coefficient `path` of material `name` as the file has it.
fn value(c: &Case, name: &str, path: &[&str]) -> Option<String> {
    let doc = c.dict(TP).unwrap();
    let [Node::List { items, .. }] = doc.get(&["buildingMaterials"]).unwrap().value.as_slice() else { panic!() };
    items.iter().find_map(|n| match n {
        Node::Dict { body, .. } if body.get("name").and_then(|e| e.value_text()).as_deref() == Some(name) => {
            body.get_path(path).and_then(|e| e.value_text()).map(|t| t.trim().to_string())
        }
        _ => None,
    })
}

#[test]
fn a_material_s_coefficients_change_in_place_and_nothing_else_does() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let (mut fs, mut c) = case(v);
        let before = c.dict(TP).unwrap().to_string();
        let e = c.set_material("street", "brick", &[set("rho", "1700"), set("lambda1", "0.7")]).unwrap();
        assert_eq!(e.changed.as_deref(), Some(TP), "{v:?}");
        assert_eq!(e.model.as_deref(), Some("Hamstad5Brick"));
        let after = c.dict(TP).unwrap().to_string();
        // Two lines changed, the soil's untouched.
        let diff: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(diff.len(), 2, "{v:?} {diff:?}");
        assert_eq!(before.lines().count(), after.lines().count());
        assert_eq!(value(&c, "brick", &["rho"]).as_deref(), Some("1700"));
        assert_eq!(value(&c, "soil", &["rho"]), value(&case(v).1, "soil", &["rho"]));
        // Saved and read again.
        c.save(&mut fs, None).unwrap();
        let c = Case::open(&fs).unwrap();
        assert_eq!(value(&c, "brick", &["lambda1"]).as_deref(), Some("0.7"));
    }
}

#[test]
fn a_new_coefficient_goes_at_the_end_and_one_is_taken_out() {
    let (_, mut c) = case(OfVersion::V12);
    let e = c.set_material("street", "soil", &[set("mu", "3.5"), set("lamda1", "2")]).unwrap();
    assert_eq!(value(&c, "soil", &["mu"]).as_deref(), Some("3.5"));
    // A name the catalog doesn't list for the model: written, with a note.
    assert!(
        e.notes.iter().any(|n| n.starts_with("lamda1: not a coefficient the solver's catalog lists for Soil")),
        "{e:?}"
    );
    let e = c.set_material("street", "soil", &[(vec!["mu".into()], None)]).unwrap();
    assert!(e.changed.is_some());
    assert_eq!(value(&c, "soil", &["mu"]), None);
    // Nothing to take out, or the value there already: nothing changes, a note.
    let e = c.set_material("street", "soil", &[(vec!["mu".into()], None)]).unwrap();
    assert_eq!(e.changed, None);
    assert!(e.notes.iter().any(|n| n.contains("has none")), "{e:?}");
    let rho = value(&c, "soil", &["rho"]).unwrap();
    assert_eq!(c.set_material("street", "soil", &[set("rho", &rho)]).unwrap().changed, None);
}

#[test]
fn what_isn_t_a_coefficient_of_that_material_is_refused_and_nothing_changes() {
    let (mut fs, _) = case(OfVersion::V8);
    let c0 = Case::open(&fs).unwrap();
    for (region, material, coeffs, want) in [
        ("air", "soil", vec![set("rho", "1")], "region air isn't a solid"),
        ("roof", "soil", vec![set("rho", "1")], "the case has no region roof"),
        ("street", "glass", vec![set("rho", "1")], "lists no material glass"),
        ("street", "soil", vec![], "give a coefficient"),
        ("street", "soil", vec![set("rho", "heavy")], "rho: heavy is not a number"),
        ("street", "soil", vec![set("rho", "inf")], "is not a number"),
        ("street", "soil", vec![set("a b", "1")], "a coefficient is a plain name"),
        ("street", "soil", vec![set("VanGenuchtenCoeffs/n", "1")], "none in a sub-dictionary"),
        ("street", "soil", vec![set("buildingMaterialModel", "1")], "isn't a coefficient"),
        ("street", "soil", vec![(vec!["cap".into()], None)], "cap: the solver reads it of every material"),
        ("street", "soil", vec![set("rho", "1"), set("rho", "2")], "rho is given twice"),
    ] {
        let mut c = c0.clone();
        let e = c.set_material(region, material, &coeffs).unwrap_err();
        assert!(e.to_string().contains(want), "{want}: {e}");
        assert!(c.changed().is_empty(), "{want}");
    }
    // A material with a macro; a name listed twice.
    let t = String::from_utf8(fs.read(TP, 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert(
        TP,
        t.replacen("name    soil;", "name    soil;\n        $common;", 1).replacen(
            "name soil;",
            "name soil; $common;",
            1,
        ),
    );
    let mut c = Case::open(&fs).unwrap();
    let e = c.set_material("street", "soil", &[set("rho", "1")]).unwrap_err();
    assert!(e.to_string().contains("has a macro or a directive"), "{e}");
}

#[test]
fn a_name_listed_twice_is_refused_as_the_solver_would_stop() {
    let (mut fs, _) = case(OfVersion::V12);
    let t = String::from_utf8(fs.read(TP, 1 << 20).unwrap().unwrap()).unwrap();
    let twice = t.replacen("name    brick;", "name    soil;", 1).replacen("name brick;", "name soil;", 1);
    assert_ne!(twice, t);
    fs.insert(TP, twice);
    let mut c = Case::open(&fs).unwrap();
    let e = c.set_material("street", "soil", &[set("rho", "1")]).unwrap_err();
    assert!(e.to_string().contains("lists material soil 2 times: the solver stops"), "{e}");
}

#[test]
fn the_comments_and_the_layout_around_a_coefficient_stay() {
    let (mut fs, _) = case(OfVersion::V8);
    let doc = "FoamFile { class dictionary; object transportProperties; }\nbuildingMaterials\n(\n    {\n        name    soil;\n        buildingMaterialModel Soil;\n        rho     1600; // kg/m3\n        cap     650;\n        lambda1 1.5;\n        lambda2 0;\n        mu      3.5;  // vapour\n    }\n    { name brick; buildingMaterialModel Hamstad5Brick; rho 1600; cap 1000; lambda1 0.682; lambda2 0; }\n);\n";
    fs.insert(TP, doc);
    let mut c = Case::open(&fs).unwrap();
    c.set_material("street", "soil", &[set("wcap", "2")]).unwrap();
    c.set_material("street", "brick", &[set("mu", "10")]).unwrap();
    let out = c.dict(TP).unwrap().to_string();
    assert!(out.contains("        mu      3.5;  // vapour\n        wcap    2;\n    }"), "{out}");
    assert!(
        out.contains(
            "{ name brick; buildingMaterialModel Hamstad5Brick; rho 1600; cap 1000; lambda1 0.682; lambda2 0; mu 10; }"
        ),
        "{out}"
    );
    // Taken out again: as it was.
    c.set_material("street", "soil", &[(vec!["wcap".into()], None)]).unwrap();
    c.set_material("street", "brick", &[(vec!["mu".into()], None)]).unwrap();
    assert_eq!(c.dict(TP).unwrap().to_string(), doc);
    // The last one taken out: the comment of the one before stays.
    c.set_material("street", "soil", &[(vec!["mu".into()], None)]).unwrap();
    let out = c.dict(TP).unwrap().to_string();
    assert!(out.contains("        lambda2 0;\n    }"), "{out}");
    assert!(out.contains("rho     1600; // kg/m3"), "{out}");
    assert!(!out.contains("vapour"), "{out}");
}

#[test]
fn a_coefficient_every_material_needs_stays_when_the_version_isn_t_known() {
    let (_, mut c) = case(OfVersion::V12);
    c.layout.version = None;
    let e = c.set_material("street", "soil", &[(vec!["rho".into()], None)]).unwrap_err();
    assert!(e.to_string().contains("rho: the solver reads it of every material"), "{e}");
    // And the required ones aren't noted as unknown.
    let e = c.set_material("street", "soil", &[set("rho", "1700")]).unwrap();
    assert!(e.notes.is_empty(), "{e:?}");
}

#[test]
fn a_solids_materials_and_their_coefficients_are_listed() {
    let (_, c) = case(OfVersion::V12);
    let m = c.material_coefficients("street");
    assert_eq!(m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["soil", "brick"]);
    assert_eq!(m[0].model.as_deref(), Some("Soil"));
    assert!(m[0].coefficients.iter().any(|(k, _)| k == "rho"), "{:?}", m[0].coefficients);
    assert!(!m[0].coefficients.iter().any(|(k, _)| k == "name" || k == "buildingMaterialModel"));
    // A region without the file, or not a solid: none.
    assert!(c.material_coefficients("air").is_empty());
    assert!(c.material_coefficients("nowhere").is_empty());
    // The names the solver reads: every material's first, then the model's own.
    let names = coefficient_names(Some(OfVersion::V12), Some("VanGenuchten"));
    let mut first: Vec<&str> = names[..4].to_vec();
    first.sort_unstable();
    assert_eq!(first, ["cap", "lambda1", "lambda2", "rho"], "{names:?}");
    assert!(names.contains(&"wcap") && names.contains(&"n"), "the model's own: {names:?}");
    assert!(coefficient_names(None, None).contains(&"rho"));
}
