use umcf_core::MemFs;

use super::*;
use crate::Case;

fn mat(n: &str, m: &str) -> MaterialDesc {
    MaterialDesc { name: n.into(), model: Some(m.into()) }
}

fn region(name: &str, kind: RegionKind) -> NewRegion {
    NewRegion { name: name.into(), kind, materials: vec![] }
}

/// Every component on: a fluid, two solids, the vegetation (given out of order).
pub(crate) fn canyon(version: OfVersion, profile: Profile) -> NewCase {
    NewCase {
        version,
        profile,
        regions: vec![
            region("air", RegionKind::Fluid),
            NewRegion {
                name: "street".into(),
                kind: RegionKind::Solid,
                materials: vec![mat("soil", "Soil"), mat("brick", "Hamstad5Brick")],
            },
            region("vegetation", RegionKind::Vegetation),
            NewRegion {
                name: "leeward".into(),
                kind: RegionKind::Solid,
                materials: vec![mat("wall", "Hamstad5Brick")],
            },
        ],
        radiation: Some("vegetation".into()),
        components: Components { grass: true, temperature_limit: true, blending: true },
        grass_patches: vec!["air_to_street".into(), "air_to_lawn".into()],
        up: Up::Z,
    }
}

/// Only the air: every component off.
fn bare(version: OfVersion, profile: Profile) -> NewCase {
    NewCase {
        version,
        profile,
        regions: vec![region("air", RegionKind::Fluid)],
        radiation: None,
        components: Components::default(),
        grass_patches: vec![],
        up: Up::Z,
    }
}

const COMBINATIONS: [(OfVersion, Profile); 3] =
    [(OfVersion::V8, Profile::Tutorial), (OfVersion::V12, Profile::Tutorial), (OfVersion::V8, Profile::Hk)];

fn file<'a>(m: &'a Made, path: &str) -> Option<&'a str> {
    m.files.iter().find(|(p, _)| p == path).map(|(_, t)| t.as_str())
}

fn opened(m: &Made) -> Case {
    let mut fs = MemFs::new();
    for (p, t) in &m.files {
        fs.insert(p, t.as_bytes());
    }
    Case::open(&fs).unwrap()
}

#[test]
fn a_made_case_opens_as_its_version_with_its_regions_and_reads_its_description_back() {
    for (v, p) in COMBINATIONS {
        let spec = canyon(v, p);
        let m = make(&spec).unwrap();
        let at = format!("{} {v:?}", p.name());
        for (path, text) in &m.files {
            let doc = Document::parse(text).unwrap_or_else(|e| panic!("{at} {path}: {e}"));
            assert!(doc.root.problems().is_empty(), "{at} {path}");
            assert!(!text.contains('\r'), "{at} {path}: CRLF");
        }
        let case = opened(&m);
        assert_eq!(case.layout.version, Some(v), "{at}");
        let regions: Vec<(&str, RegionKind)> = case.regions.iter().map(|r| (r.name.as_str(), r.kind)).collect();
        assert_eq!(
            regions,
            [
                ("air", RegionKind::Fluid),
                ("street", RegionKind::Solid),
                ("leeward", RegionKind::Solid),
                ("vegetation", RegionKind::Vegetation)
            ],
            "{at}"
        );
        let (back, problems) = case.description().unwrap();
        assert_eq!(problems, [], "{at}");
        assert_eq!(back, m.description, "{at}");
        assert_eq!(back.profile, Some(p));
        assert_eq!(back.components, spec.components);
        assert_eq!(back.radiation.as_ref().and_then(|r| r.lai_region.as_deref()), Some("air"), "{at}");
        // The import tells the components from the files.
        let (imported, _) = Description::import(&case);
        assert_eq!(imported.components, spec.components, "{at}");
        // The benchmark has no brick.
        let notes = if p == Profile::Hk { 2 } else { 0 };
        assert_eq!(m.notes.len(), notes, "{at}: {:?}", m.notes);
    }
}

#[test]
fn each_region_gets_the_files_of_its_kind_and_each_component_its_own() {
    for (v, p) in COMBINATIONS {
        let m = make(&canyon(v, p)).unwrap();
        let at = format!("{} {v:?}", p.name());
        let mut want = vec![
            "system/umcfWorkflow",
            "system/controlDict",
            "constant/air/thermophysicalProperties",
            "constant/air/momentumTransport",
            "system/air/fvSchemes",
            "system/air/fvSolution",
            "constant/air/g",
            "constant/street/g",
            "system/street/fvSchemes",
            "system/street/fvSolution",
            "constant/street/transportProperties",
            "constant/leeward/g",
            "system/leeward/fvSchemes",
            "system/leeward/fvSolution",
            "constant/leeward/transportProperties",
            "system/vegetation/fvSchemes",
            "system/vegetation/fvSolution",
            "constant/air/grassProperties",
            "constant/air/vegetationProperties",
            "constant/vegetation/radiationProperties",
            "constant/vegetation/solarLoadProperties",
        ];
        match v {
            OfVersion::V8 => want.extend(["constant/regionProperties", "system/air/fvOptions"]),
            OfVersion::V12 => want.push("system/air/fvConstraints"),
        }
        let mut got: Vec<&str> = m.files.iter().map(|(p, _)| p.as_str()).collect();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "{at}");
        // Every component off: the air's files only (and OpenFOAM-12's pressure limit).
        let m = make(&bare(v, p)).unwrap();
        let mut got: Vec<&str> = m.files.iter().map(|(p, _)| p.as_str()).collect();
        got.sort_unstable();
        let mut want = vec![
            "constant/air/g",
            "constant/air/momentumTransport",
            "constant/air/thermophysicalProperties",
            "system/air/fvSchemes",
            "system/air/fvSolution",
            "system/controlDict",
            "system/umcfWorkflow",
        ];
        match v {
            OfVersion::V8 => want.push("constant/regionProperties"),
            OfVersion::V12 => want.push("system/air/fvConstraints"),
        }
        want.sort_unstable();
        assert_eq!(got, want, "{at}: bare");
        let control = Document::parse(file(&m, "system/controlDict").unwrap()).unwrap();
        assert!(control.get(&["blending"]).is_none() && control.get(&["blendingCoeffs"]).is_none(), "{at}");
        if v == OfVersion::V12 {
            let c = Document::parse(file(&m, "system/air/fvConstraints").unwrap()).unwrap();
            assert!(c.get(&["limitT"]).is_none() && c.get(&["limitp"]).is_some(), "{at}");
        }
    }
}

#[test]
fn the_settings_are_the_profile_s_files_as_they_are() {
    // The tutorial's are the examples', byte for byte but for the line ends.
    let example = |v: &str, case: &str, path: &str| {
        let p = format!("{}/../../examples/{v}/streetCanyon_{case}/{path}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n")
    };
    for (v, dir) in [(OfVersion::V8, "of8"), (OfVersion::V12, "of12")] {
        let m = make(&canyon(v, Profile::Tutorial)).unwrap();
        for (made, case, from) in [
            ("system/air/fvSchemes", "CFDHAM", "system/air/fvSchemes"),
            ("system/air/fvSolution", "CFDHAM", "system/air/fvSolution"),
            ("constant/air/thermophysicalProperties", "CFDHAM", "constant/air/thermophysicalProperties"),
            ("constant/air/momentumTransport", "CFDHAM", "constant/air/momentumTransport"),
            ("system/leeward/fvSolution", "CFDHAM", "system/leeward/fvSolution"),
            ("constant/street/g", "CFDHAM", "constant/street/g"),
            ("system/vegetation/fvSchemes", "CFDHAM_veg", "system/vegetation/fvSchemes"),
            ("constant/air/vegetationProperties", "CFDHAM_veg", "constant/air/vegetationProperties"),
            ("constant/vegetation/radiationProperties", "CFDHAM", "constant/air/radiationProperties"),
        ] {
            assert_eq!(file(&m, made).unwrap(), example(dir, case, from), "{dir} {made}");
        }
    }
    let m = make(&canyon(OfVersion::V8, Profile::Hk)).unwrap();
    // As checked out: with CRLF line ends on Windows, say.
    let hk = |path: &str| {
        std::fs::read_to_string(format!("{}/../schema/templates/hk/{path}", env!("CARGO_MANIFEST_DIR")))
            .unwrap()
            .replace("\r\n", "\n")
    };
    assert_eq!(file(&m, "system/air/fvSchemes").unwrap(), hk("fluid/fvSchemes"));
    assert_eq!(file(&m, "system/street/fvSolution").unwrap(), hk("solid/fvSolution"));
    assert!(file(&m, "constant/air/momentumTransport").unwrap().contains("realizableKE"));
}

#[test]
fn the_run_controls_drop_the_profile_s_function_objects_and_name_the_regions() {
    for (v, p) in COMBINATIONS {
        let m = make(&canyon(v, p)).unwrap();
        let text = file(&m, "system/controlDict").unwrap();
        let doc = Document::parse(text).unwrap();
        assert!(doc.get(&["functions"]).is_none(), "{v:?}");
        assert_eq!(doc.value(&["application"]).as_deref(), Some(SOLVER));
        assert_eq!(doc.value(&["blending"]).as_deref(), Some("true"));
        assert_eq!(doc.value(&["blendingCoeffs", "dampingThickness"]).as_deref(), Some("60"));
        // A comment after the last entry stays on its line when entries come after it.
        if (v, p) == (OfVersion::V8, Profile::Tutorial) {
            assert!(text.contains("pcEqnForm    pc-based; //'pc-based' or 'mixed'"), "{text}");
        }
        match v {
            OfVersion::V12 => {
                assert_eq!(doc.value(&["regions", "fluid"]).as_deref(), Some("(air)"));
                assert_eq!(doc.value(&["regions", "solid"]).as_deref(), Some("(street leeward)"));
                assert_eq!(doc.value(&["regions", "vegetation"]).as_deref(), Some("(vegetation)"));
            }
            OfVersion::V8 => {
                assert!(doc.get(&["regions"]).is_none());
                let rp = file(&m, "constant/regionProperties").unwrap();
                assert!(
                    rp.contains(
                        "regions\n(\n    fluid       (air)\n    solid       (street leeward)\n    vegetation  (vegetation)\n);"
                    ),
                    "{rp}"
                );
            }
        }
    }
    // OpenFOAM-12 without solids or vegetation: no empty lists.
    let m = make(&bare(OfVersion::V12, Profile::Tutorial)).unwrap();
    let doc = Document::parse(file(&m, "system/controlDict").unwrap()).unwrap();
    assert!(doc.get(&["regions", "solid"]).is_none() && doc.get(&["regions", "vegetation"]).is_none());
    // OpenFOAM-8's has every kind, empty ones too: its solver looks each one up.
    let m = make(&bare(OfVersion::V8, Profile::Tutorial)).unwrap();
    let rp = file(&m, "constant/regionProperties").unwrap();
    assert!(rp.contains("regions\n(\n    fluid       (air)\n    solid       ()\n    vegetation  ()\n);"), "{rp}");
    let case = opened(&m);
    assert_eq!(case.regions.len(), 1);
}

#[test]
fn a_solid_s_materials_take_the_coefficients_of_the_profile_s_material_of_their_model() {
    let mut spec = canyon(OfVersion::V8, Profile::Hk);
    spec.regions.truncate(3);
    spec.regions[1].materials =
        vec![mat("soil", "Soil"), mat("concrete", "HamstadConcrete"), mat("b", "Hamstad5Brick")];
    let m = make(&spec).unwrap();
    let doc = Document::parse(file(&m, "constant/street/transportProperties").unwrap()).unwrap();
    let list = doc.value(&["buildingMaterials"]).unwrap();
    assert!(
        list.contains("{ name soil; buildingMaterialModel Soil; rho 1150; cap 650; lambda1 1.5; lambda2 0; }"),
        "{list}"
    );
    assert!(list.contains("{ name concrete; buildingMaterialModel HamstadConcrete; rho 1950; cap 850;"), "{list}");
    assert!(list.contains("{ name b; buildingMaterialModel Hamstad5Brick; }"), "{list}");
    assert_eq!(
        m.notes,
        [
            "region street, material b: the hk profile has no Hamstad5Brick material to take its coefficients from: add them to constant/street/transportProperties"
        ]
    );
    // None at all: an empty list, and a note.
    spec.regions[1].materials.clear();
    let m = make(&spec).unwrap();
    let doc = Document::parse(file(&m, "constant/street/transportProperties").unwrap()).unwrap();
    assert_eq!(doc.value(&["buildingMaterials"]).as_deref(), Some("()"));
    assert!(m.notes[0].starts_with("region street: no materials"), "{:?}", m.notes);
}

#[test]
fn the_components_settings_are_written_where_each_version_reads_them() {
    let m = make(&canyon(OfVersion::V8, Profile::Tutorial)).unwrap();
    let o = Document::parse(file(&m, "system/air/fvOptions").unwrap()).unwrap();
    assert_eq!(o.value(&["limitT", "type"]).as_deref(), Some("limitTemperature"));
    assert_eq!(o.value(&["limitT", "min"]).as_deref(), Some("273.15"));
    assert_eq!(o.value(&["limitT", "max"]).as_deref(), Some("373.15"));
    assert_eq!(o.header("location").as_deref(), Some("system"));
    let m = make(&canyon(OfVersion::V8, Profile::Hk)).unwrap();
    let o = Document::parse(file(&m, "system/air/fvOptions").unwrap()).unwrap();
    assert_eq!(o.value(&["limitT", "min"]).as_deref(), Some("283.15"));
    let m = make(&canyon(OfVersion::V12, Profile::Tutorial)).unwrap();
    let c = Document::parse(file(&m, "system/air/fvConstraints").unwrap()).unwrap();
    assert_eq!(c.value(&["limitT", "select"]).as_deref(), Some("all"));
    assert_eq!(c.value(&["limitp", "type"]).as_deref(), Some("limitPressure"));
    let g = Document::parse(file(&m, "constant/air/grassProperties").unwrap()).unwrap();
    assert_eq!(g.value(&["simpleGrassCoeffs", "grassPatches"]).as_deref(), Some("(air_to_street air_to_lawn)"));
    // Gravity down the axis that is up; a solid's is none.
    let mut spec = bare(OfVersion::V8, Profile::Tutorial);
    spec.up = Up::Y;
    let m = make(&spec).unwrap();
    let g = Document::parse(file(&m, "constant/air/g").unwrap()).unwrap();
    assert_eq!(g.value(&["value"]).as_deref(), Some("(0 -9.81 0)"));
    let m = make(&canyon(OfVersion::V12, Profile::Tutorial)).unwrap();
    let g = Document::parse(file(&m, "constant/street/g").unwrap()).unwrap();
    assert_eq!(g.value(&["value"]).as_deref(), Some("(0 0 0)"));
    // Grass without patches: a note.
    let mut spec = bare(OfVersion::V12, Profile::Tutorial);
    spec.components.grass = true;
    spec.radiation = Some("air".into());
    assert_eq!(make(&spec).unwrap().notes.len(), 1);
}

#[test]
fn what_can_t_be_made_is_refused() {
    let refused = |f: &dyn Fn(&mut NewCase), want: &str| {
        let mut spec = canyon(OfVersion::V12, Profile::Tutorial);
        f(&mut spec);
        match make(&spec) {
            Err(Error::NewCase(m)) => assert!(m.contains(want), "{m} (wanted {want})"),
            other => panic!("{want}: {other:?}"),
        }
    };
    refused(&|s| s.profile = Profile::Hk, "the hk profile has no files for OpenFOAM-12");
    refused(&|s| s.regions.retain(|r| r.kind != RegionKind::Fluid), "needs a fluid region");
    refused(&|s| s.regions.push(region("air", RegionKind::Solid)), "region air is given twice");
    refused(&|s| s.regions.push(region("Air", RegionKind::Solid)), "only by case");
    refused(&|s| s.regions.push(region("a b", RegionKind::Solid)), "letters, digits");
    refused(&|s| s.regions.push(region("x:y", RegionKind::Solid)), "letters, digits");
    refused(&|s| s.regions.push(region("polyMesh", RegionKind::Solid)), "constant/ or system/");
    refused(&|s| s.regions.push(region("PolyMesh", RegionKind::Solid)), "constant/ or system/");
    refused(&|s| s.regions.push(region("controlDict", RegionKind::Solid)), "constant/ or system/");
    refused(&|s| s.regions.push(region("fvSchemes", RegionKind::Solid)), "constant/ or system/");
    refused(&|s| s.regions.push(region(".", RegionKind::Solid)), "end with `.`");
    refused(&|s| s.regions.push(region("..", RegionKind::Solid)), "end with `.`");
    refused(&|s| s.regions.push(region("wall.", RegionKind::Solid)), "end with `.`");
    refused(&|s| s.regions.push(region("nul", RegionKind::Solid)), "device");
    refused(&|s| s.regions.push(region("COM1.x", RegionKind::Solid)), "device");
    refused(&|s| s.regions[2].name = "trees".into(), "named `vegetation`");
    refused(&|s| s.regions.push(region("vegetation2", RegionKind::Vegetation)), "named `vegetation`");
    refused(&|s| s.regions.push(region("vegetation", RegionKind::Vegetation)), "given twice");
    refused(&|s| s.radiation = Some("air".into()), "needs the radiation on it");
    refused(&|s| s.radiation = None, "needs the radiation on it");
    refused(
        &|s| {
            s.regions[0].name = "wind".into();
            s.regions.remove(2);
            s.radiation = Some("wind".into());
        },
        "a fluid region named `air`",
    );
    refused(&|s| s.up = Up::Y, "the blending layer needs z up");
    refused(
        &|s| {
            s.regions.remove(2);
            s.radiation = None;
        },
        "the grass needs the radiation",
    );
    refused(&|s| s.regions.push(region("one", RegionKind::Single)), "a fluid, a solid or the vegetation");
    refused(&|s| s.regions[0].materials.push(mat("m", "Soil")), "only a solid has materials");
    refused(&|s| s.regions[1].materials.push(mat("soil", "Soil")), "material soil is given twice");
    refused(&|s| s.regions[1].materials.push(mat("x", "Marble")), "Marble is not a material model");
    refused(&|s| s.regions[1].materials.push(MaterialDesc { name: "x".into(), model: None }), "model");
    refused(
        &|s| {
            s.regions.remove(2);
            s.radiation = Some("street".into());
        },
        "a solid has no radiation",
    );
    refused(
        &|s| {
            s.regions.remove(2);
            s.radiation = Some("nowhere".into());
        },
        "no such region",
    );
    refused(&|s| s.components.grass = false, "grass patches without grass");
    refused(&|s| s.grass_patches.push("air_to_street".into()), "given twice");
}
