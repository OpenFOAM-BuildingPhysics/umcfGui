use umcf_core::MemFs;

use super::*;
use crate::description::{Components, MaterialDesc};
use crate::new_case::{NewCase, make};

/// A case made by `case.new`: the air, a street, every component but grass as asked.
fn made(version: OfVersion, up: Up, temperature_limit: bool) -> MemFs {
    made_with(Profile::Tutorial, version, up, temperature_limit)
}

fn made_with(profile: Profile, version: OfVersion, up: Up, temperature_limit: bool) -> MemFs {
    let spec = NewCase {
        version,
        profile,
        regions: vec![
            NewRegion { name: "air".into(), kind: RegionKind::Fluid, materials: vec![] },
            NewRegion { name: "street".into(), kind: RegionKind::Solid, materials: vec![] },
        ],
        radiation: None,
        components: Components { temperature_limit, ..Default::default() },
        grass_patches: vec![],
        up,
    };
    let mut fs = MemFs::new();
    for (p, t) in make(&spec).unwrap().files {
        fs.insert(&p, t);
    }
    fs
}

fn solid(name: &str, materials: &[(&str, &str)]) -> NewRegion {
    NewRegion {
        name: name.into(),
        kind: RegionKind::Solid,
        materials: materials
            .iter()
            .map(|(n, m)| MaterialDesc { name: (*n).into(), model: Some((*m).into()) })
            .collect(),
    }
}

#[test]
fn a_region_is_listed_where_its_version_reads_it_and_gets_its_files() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let mut fs = made(v, Up::Z, false);
        let mut c = Case::open(&fs).unwrap();
        let added = c.add_region(&fs, &solid("roofs", &[("brick", "Hamstad5Brick")])).unwrap();
        assert_eq!(added.kept, Vec::<String>::new());
        assert!(added.described);
        let mut want = vec![
            "constant/roofs/g",
            "constant/roofs/transportProperties",
            "system/roofs/fvSchemes",
            "system/roofs/fvSolution",
        ];
        want.sort_unstable();
        let mut got = added.written.clone();
        got.sort_unstable();
        assert_eq!(got, want, "{v:?}");
        match v {
            OfVersion::V12 => {
                assert_eq!(added.listed_in, "system/controlDict");
                let text = c.dict("system/controlDict").unwrap().to_string();
                assert!(text.contains("solid           ( street roofs );"), "{text}");
            }
            OfVersion::V8 => {
                assert_eq!(added.listed_in, "constant/regionProperties");
                let text = c.dict("constant/regionProperties").unwrap().to_string();
                assert!(text.contains("    solid       (street roofs)\n"), "{text}");
            }
        }
        let names: Vec<&str> = c.regions.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["air", "street", "roofs"]);
        assert!(c.region("roofs").is_some_and(|r| r.patches.is_none()));
        let (d, problems) = c.description().unwrap();
        assert_eq!(problems, []);
        let roofs = d.region("roofs").unwrap();
        assert_eq!(roofs.kind, RegionKind::Solid);
        assert_eq!(roofs.materials, [MaterialDesc { name: "brick".into(), model: Some("Hamstad5Brick".into()) }]);
        // Saved and opened again, it is a region of the case like the others.
        let mut changed = c.changed();
        changed.sort_unstable();
        assert!(changed.contains(&"constant/roofs/transportProperties".to_string()), "{changed:?}");
        c.save(&mut fs, Some(".umcfgui/backup/t")).unwrap();
        let again = Case::open(&fs).unwrap();
        assert_eq!(again.regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["air", "street", "roofs"]);
        assert_eq!(again.layout.regions.solid, ["street", "roofs"]);
    }
}

#[test]
fn a_new_fluid_falls_and_is_limited_as_the_case_s() {
    let fs = made(OfVersion::V12, Up::Y, true);
    let mut c = Case::open(&fs).unwrap();
    c.add_region(&fs, &NewRegion { name: "wind".into(), kind: RegionKind::Fluid, materials: vec![] }).unwrap();
    assert_eq!(c.dict("constant/wind/g").unwrap().value(&["value"]).as_deref(), Some("(0 -9.81 0)"));
    assert_eq!(
        c.dict("system/wind/fvConstraints").unwrap().value(&["limitT", "type"]).as_deref(),
        Some("limitTemperature")
    );
    let text = c.dict("system/controlDict").unwrap().to_string();
    assert!(text.contains("fluid           ( air wind );"), "{text}");
}

#[test]
fn the_vegetation_gets_the_trees_model_and_a_note_on_its_radiation() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let fs = made(v, Up::Z, false);
        let mut c = Case::open(&fs).unwrap();
        let added = c
            .add_region(&fs, &NewRegion { name: "vegetation".into(), kind: RegionKind::Vegetation, materials: vec![] })
            .unwrap();
        assert!(added.written.contains(&"constant/air/vegetationProperties".to_string()), "{:?}", added.written);
        assert!(added.notes.iter().any(|n| n.contains("needs the radiation on it")), "{:?}", added.notes);
        if v == OfVersion::V8 {
            let text = c.dict("constant/regionProperties").unwrap().to_string();
            assert!(text.contains("    vegetation  (vegetation)\n"), "{text}");
        } else {
            let text = c.dict("system/controlDict").unwrap().to_string();
            assert!(
                text.contains("vegetation ( vegetation );") || text.contains("vegetation      ( vegetation );"),
                "{text}"
            );
        }
        assert_eq!(c.regions.last().map(|r| r.name.as_str()), Some("vegetation"));
    }
}

#[test]
fn files_that_are_there_are_kept() {
    let mut fs = made(OfVersion::V12, Up::Z, false);
    fs.insert("system/roofs/fvSchemes", "FoamFile { class dictionary; object fvSchemes; }\nmine 1;\n");
    let mut c = Case::open(&fs).unwrap();
    let added = c.add_region(&fs, &solid("roofs", &[])).unwrap();
    assert_eq!(added.kept, ["system/roofs/fvSchemes"]);
    assert!(!c.changed().contains(&"system/roofs/fvSchemes".to_string()));
    // Saved, it is as it was.
    c.save(&mut fs, Some(".umcfgui/backup/t")).unwrap();
    let text = String::from_utf8(fs.read("system/roofs/fvSchemes", 1 << 20).unwrap().unwrap()).unwrap();
    assert!(text.contains("mine 1;"), "{text}");
    assert!(added.notes.iter().any(|n| n.contains("no materials")), "{:?}", added.notes);
}

#[test]
fn what_can_t_be_added_is_refused_and_nothing_changes() {
    let fs = made(OfVersion::V12, Up::Z, false);
    let refused = |r: NewRegion, want: &str| {
        let mut c = Case::open(&fs).unwrap();
        match c.add_region(&fs, &r) {
            Err(Error::NewCase(m)) => assert!(m.contains(want), "{m} (wanted {want})"),
            other => panic!("{want}: {other:?}"),
        }
        assert_eq!(c.changed(), Vec::<String>::new(), "{want}");
        assert_eq!(c.regions.len(), 2);
    };
    refused(solid("street", &[]), "there already");
    refused(solid("Street", &[]), "only by case");
    refused(NewRegion { name: "trees".into(), kind: RegionKind::Vegetation, materials: vec![] }, "named `vegetation`");
    refused(solid("controlDict", &[]), "constant/ or system/");
    refused(solid("roofs", &[("x", "Marble")]), "not a material model");
    // A meshing step has no region list.
    let mut meshing = MemFs::new();
    meshing
        .insert("system/controlDict", "FoamFile { class dictionary; object controlDict; }\napplication blockMesh;\n");
    meshing.insert("system/blockMeshDict", "FoamFile { class dictionary; object blockMeshDict; }\n");
    let mut c = Case::open(&meshing).unwrap();
    assert!(c.add_region(&meshing, &solid("roofs", &[])).is_err());
}

#[test]
fn a_link_where_a_file_would_go_is_refused() {
    struct Linked(MemFs);
    impl CaseFs for Linked {
        fn read(&self, rel: &str, max: u64) -> std::result::Result<Option<Vec<u8>>, umcf_core::FsError> {
            self.0.read(rel, max)
        }
        fn list(&self, rel: &str) -> std::result::Result<Vec<String>, umcf_core::FsError> {
            self.0.list(rel)
        }
        fn is_dir(&self, rel: &str) -> bool {
            self.0.is_dir(rel)
        }
        fn is_file(&self, rel: &str) -> bool {
            self.0.is_file(rel)
        }
        fn is_link(&self, rel: &str) -> bool {
            rel == "system/roofs/fvSolution"
        }
    }
    let fs = Linked(made(OfVersion::V12, Up::Z, false));
    let mut c = Case::open(&fs).unwrap();
    assert!(matches!(c.add_region(&fs, &solid("roofs", &[])), Err(Error::IsLink(p)) if p == "system/roofs/fvSolution"));
    assert_eq!(c.regions.len(), 2);
}

#[test]
fn the_openfoam_12_list_keeps_its_layout_and_comments() {
    let mut fs = made(OfVersion::V12, Up::Z, false);
    let text = String::from_utf8(fs.read("system/controlDict", 1 << 20).unwrap().unwrap()).unwrap();
    let text = text.replace(
        "    solid           ( street );",
        "    solid\n        (\n            street // the street\n            /* end */\n        );",
    );
    fs.insert("system/controlDict", text);
    let mut c = Case::open(&fs).unwrap();
    c.add_region(&fs, &solid("roofs", &[])).unwrap();
    let text = c.dict("system/controlDict").unwrap().to_string();
    assert!(
        text.contains("    solid\n        (\n            street // the street\n            /* end */\n            roofs\n        );"),
        "{text}"
    );
}

#[test]
fn a_solid_described_already_brings_its_materials() {
    let fs = made(OfVersion::V12, Up::Z, false);
    let mut c = Case::open(&fs).unwrap();
    c.describe_region("roofs", RegionKind::Solid).unwrap();
    c.describe_material("roofs", "brick", Some(Some("Hamstad5Brick"))).unwrap();
    let added = c.add_region(&fs, &solid("roofs", &[])).unwrap();
    assert!(added.notes.iter().any(|n| n.contains("the description's")), "{:?}", added.notes);
    let tp = c.dict("constant/roofs/transportProperties").unwrap().value(&["buildingMaterials"]).unwrap();
    assert!(tp.contains("name brick; buildingMaterialModel Hamstad5Brick; rho 1600;"), "{tp}");
}

#[test]
fn a_described_material_the_version_lacks_is_named_as_the_descriptions() {
    let fs = made(OfVersion::V12, Up::Z, false);
    let mut c = Case::open(&fs).unwrap();
    c.describe_region("roofs", RegionKind::Solid).unwrap();
    c.describe_material("roofs", "brick", Some(Some("NoSuchBrick"))).unwrap();
    let e = c.add_region(&fs, &solid("roofs", &[])).unwrap_err().to_string();
    assert!(e.contains("NoSuchBrick") && e.contains("the description's"), "{e}");
    assert!(c.region("roofs").is_none());
}

#[test]
fn a_refusal_half_way_leaves_the_case_as_it_was() {
    let fs = made(OfVersion::V12, Up::Z, false);
    let mut c = Case::open(&fs).unwrap();
    c.describe_region("roofs", RegionKind::Solid).unwrap();
    c.describe_material("roofs", "brick", Some(Some("Hamstad5Brick"))).unwrap();
    let before = c.changed();
    let controls = c.dict("system/controlDict").unwrap().to_string();
    // The description refuses to make a solid with materials a fluid: after the list and the
    // files were made.
    let fluid = NewRegion { name: "roofs".into(), kind: RegionKind::Fluid, materials: vec![] };
    assert!(c.add_region(&fs, &fluid).is_err());
    assert_eq!(c.changed(), before);
    assert_eq!(c.dict("system/controlDict").unwrap().to_string(), controls);
    assert_eq!(c.regions.len(), 2);
    assert_eq!(c.layout.regions.fluid, ["air"]);
}

#[test]
fn gravity_given_by_a_macro_is_not_copied() {
    let mut fs = made(OfVersion::V12, Up::Y, false);
    let g = String::from_utf8(fs.read("constant/air/g", 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert("constant/air/g", g.replace("( 0 -9.81 0 )", "$down"));
    let mut c = Case::open(&fs).unwrap();
    c.add_region(&fs, &NewRegion { name: "wind".into(), kind: RegionKind::Fluid, materials: vec![] }).unwrap();
    assert_eq!(c.dict("constant/wind/g").unwrap().value(&["value"]).as_deref(), Some("(0 0 -9.81)"));
}

#[test]
fn the_settings_are_those_of_the_description_s_profile() {
    // A case made with hk gets hk's files.
    let fs = made_with(Profile::Hk, OfVersion::V8, Up::Z, false);
    let mut c = Case::open(&fs).unwrap();
    c.add_region(&fs, &solid("roofs", &[])).unwrap();
    // As checked out: with CRLF line ends on Windows, say; case.new makes them LF.
    let hk = include_str!("../../schema/templates/hk/solid/fvSolution").replace("\r\n", "\n");
    assert_eq!(c.dict("system/roofs/fvSolution").unwrap().to_string(), hk);
    // A profile without files for the case's version is refused, nothing changed.
    let mut fs = made(OfVersion::V12, Up::Z, false);
    let text = String::from_utf8(fs.read(crate::description::PATH, 1 << 20).unwrap().unwrap()).unwrap();
    assert!(text.contains("profile     tutorial;"), "{text}");
    fs.insert(crate::description::PATH, text.replace("profile     tutorial;", "profile     hk;"));
    let mut c = Case::open(&fs).unwrap();
    match c.add_region(&fs, &solid("roofs", &[])) {
        Err(Error::NewCase(m)) => assert!(m.contains("the hk profile has no files for OpenFOAM-12"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(c.changed(), Vec::<String>::new());
}
