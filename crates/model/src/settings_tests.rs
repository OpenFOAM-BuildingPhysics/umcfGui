use umcf_core::{CaseFs, MemFs, OfVersion};
use umcf_foam_dict::Document;

use super::*;
use crate::description::{Components, Description};
use crate::new_case::{NewCase, NewRegion, Up, make};

fn chosen() -> Settings {
    Settings {
        delta_t: Some("1800".into()),
        end_time: Some("316800".into()),
        write_interval: Some("10".into()),
        turbulence: Some("realizableKE".into()),
        temperature_limit: Some(("283.15".into(), "370.15".into())),
        blending: Some(["60".into(), "0.3".into(), "0.25".into()]),
        materials: vec![],
        numerics: vec![],
    }
}

fn spec(version: OfVersion, settings: Settings, limit: bool, blending: bool) -> NewCase {
    NewCase {
        version,
        regions: vec![NewRegion { name: "air".into(), kind: RegionKind::Fluid, materials: vec![] }],
        radiation: None,
        components: Components { temperature_limit: limit, blending, ..Default::default() },
        grass_patches: vec![],
        up: Up::Z,
        settings,
    }
}

fn opened(spec: &NewCase) -> (MemFs, Case) {
    let mut fs = MemFs::new();
    for (p, t) in make(spec).unwrap().files {
        fs.insert(&p, t);
    }
    let c = Case::open(&fs).unwrap();
    (fs, c)
}

fn read(text: &str) -> (Settings, Vec<String>) {
    let doc = Document::parse(text).unwrap();
    let mut problems = Vec::new();
    let s = Settings::read(&doc.root, &mut |at: &str, m: String| problems.push(format!("{at}: {m}")));
    (s, problems)
}

#[test]
fn the_settings_block_reads_back_as_written() {
    let s = chosen();
    let text = s.write();
    assert!(text.starts_with("settings\n{\n    deltaT           1800;\n"), "{text}");
    assert!(text.contains("    temperatureLimit ( 283.15 370.15 );\n"), "{text}");
    assert!(
        text.contains("    blending         { dampingThickness 60; alphaCoeffU 0.3; alphaCoeffT 0.25; }\n"),
        "{text}"
    );
    let d = Description { settings: s.clone(), ..Default::default() };
    let (back, problems) = Description::read(&Document::parse(&d.write()).unwrap());
    assert_eq!(problems, vec![]);
    assert_eq!(back.settings, s);
    assert_eq!(Settings::default().write(), "");
}

#[test]
fn what_isn_t_a_setting_is_a_problem_and_left_out() {
    let (s, problems) = read(
        "deltaT -1; endTime 0; writeInterval x; turbulence \"k e\"; temperatureLimit ( 370 280 ); blending { dampingThickness 60; alpha 1; } colour red; endTime 5;",
    );
    assert_eq!(s, Settings { end_time: Some("0".into()), ..Default::default() });
    for want in [
        "settings/deltaT: is a number above 0",
        "settings/writeInterval: is a number above 0",
        "settings/turbulence: is the name of a RAS model",
        "settings/temperatureLimit: is ( min max )",
        "settings/blending/alpha: is not one of dampingThickness, alphaCoeffU, alphaCoeffT",
        "settings/colour: is not a setting",
        "settings/endTime: is given twice",
    ] {
        assert!(problems.iter().any(|p| p.starts_with(want)), "{want}: {problems:?}");
    }
    let (s, problems) = read("blending { dampingThickness 60; }");
    assert!(s.blending.is_none());
    assert_eq!(problems, ["settings/blending: gives dampingThickness, alphaCoeffU, alphaCoeffT"]);
}

#[test]
fn a_new_case_gets_the_settings_chosen_and_reads_them_back() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let (_, c) = opened(&spec(v, chosen(), true, true));
        let control = c.dict(CONTROL_DICT).unwrap();
        assert_eq!(control.value(&["deltaT"]).as_deref(), Some("1800"));
        assert_eq!(control.value(&["endTime"]).as_deref(), Some("316800"));
        assert_eq!(control.value(&["blendingCoeffs", "alphaCoeffT"]).as_deref(), Some("0.25"));
        let m = c.dict("constant/air/momentumTransport").unwrap();
        assert_eq!(m.value(&["RAS", "RASModel"]).as_deref(), Some("realizableKE"), "{v:?}");
        let (from_files, notes) = Settings::from_case(&c);
        assert_eq!(notes, Vec::<String>::new(), "{v:?}");
        assert_eq!(from_files, chosen(), "{v:?}");
        let (d, problems) = c.description().unwrap();
        assert_eq!(problems, vec![]);
        assert_eq!(d.settings, chosen());
    }
    // Nothing chosen: the configuration's, and the description has no block.
    let (_, c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    let (s, _) = Settings::from_case(&c);
    assert_eq!(s.delta_t.as_deref(), Some("3600"));
    assert_eq!(s.turbulence.as_deref(), Some("kEpsilon"));
    assert_eq!((s.temperature_limit, s.blending), (None, None));
    assert!(!c.dict(crate::description::PATH).unwrap().to_string().contains("settings"));
}

#[test]
fn settings_go_into_the_files_where_the_case_has_them_and_nothing_else_changes() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let (_, mut c) = opened(&spec(v, Settings::default(), true, true));
        let before: Vec<(String, String)> =
            c.changed().into_iter().map(|p| (p.clone(), c.dict(&p).unwrap().to_string())).collect();
        assert!(before.is_empty());
        let limit_file = match v {
            OfVersion::V8 => "system/air/fvOptions",
            OfVersion::V12 => "system/air/fvConstraints",
        };
        let limit_before = c.dict(limit_file).unwrap().to_string();
        let a = c.apply_settings(&chosen()).unwrap();
        assert_eq!(a.notes, Vec::<String>::new(), "{v:?}");
        let mut changed = a.changed.clone();
        changed.sort();
        let mut want = vec![CONTROL_DICT.to_string(), "constant/air/momentumTransport".into(), limit_file.into()];
        want.sort();
        assert_eq!(changed, want, "{v:?}");
        assert_eq!(Settings::from_case(&c).0, chosen(), "{v:?}");
        // Only the bounds changed in the limit's file.
        let after = c.dict(limit_file).unwrap().to_string();
        assert_eq!(after.replace("283.15", "273.15").replace("370.15", "373.15"), limit_before, "{v:?}");
    }
    // A case without a limit or blending layer: noted, not added.
    let (_, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    let a = c.apply_settings(&chosen()).unwrap();
    assert_eq!(a.notes.len(), 2, "{:?}", a.notes);
    assert!(c.dict(CONTROL_DICT).unwrap().get(&["blendingCoeffs"]).is_none());
    assert!(!c.dict("system/air/fvConstraints").unwrap().to_string().contains("limitTemperature"));
}

#[test]
fn settings_given_otherwise_in_a_case_are_noted_and_left_out() {
    let (mut fs, _) = opened(&spec(OfVersion::V12, Settings::default(), true, false));
    let control = String::from_utf8(fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert(CONTROL_DICT, control.replace("deltaT          3600;", "deltaT          $dt;"));
    let c = Case::open(&fs).unwrap();
    let (s, notes) = Settings::from_case(&c);
    assert!(s.delta_t.is_none());
    assert!(notes.iter().any(|n| n.contains("deltaT is $dt")), "{notes:?}");
}

#[test]
fn chosen_settings_go_into_the_files_and_the_description_entry_by_entry() {
    let (_, mut c) = opened(&spec(OfVersion::V8, Settings::default(), true, true));
    let path = crate::description::PATH;
    let before = c.dict(path).unwrap().to_string();
    let some = Settings {
        end_time: Some("86400".into()),
        temperature_limit: Some(("280".into(), "360".into())),
        ..Default::default()
    };
    let chosen = c.choose_settings(&some).unwrap();
    assert!(chosen.described);
    let (d, problems) = c.description().unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(d.settings, some);
    let after = c.dict(path).unwrap().to_string();
    // What was there stays, line for line, in its order.
    let mut lines = after.lines();
    assert!(before.lines().all(|l| lines.any(|a| a == l)), "{after}");
    assert!(after.contains("settings\n{\n    endTime         86400;"), "{after}");
    assert_eq!(c.dict(CONTROL_DICT).unwrap().value(&["endTime"]).as_deref(), Some("86400"));
    // Another one later: the first stays.
    c.choose_settings(&Settings { blending: chosen_blending(), ..Default::default() }).unwrap();
    let (d, _) = c.description().unwrap();
    assert_eq!(d.settings.end_time.as_deref(), Some("86400"));
    assert_eq!(d.settings.blending, chosen_blending());
}

fn chosen_blending() -> Option<[String; 3]> {
    Some(["50".into(), "0.2".into(), "0.2".into()])
}

/// The case `spec` makes, with `edit` applied to the text of file `rel`.
fn edited(spec: &NewCase, rel: &str, edit: impl Fn(String) -> String) -> Case {
    let (mut fs, _) = opened(spec);
    let text = String::from_utf8(fs.read(rel, 1 << 20).unwrap().unwrap()).unwrap();
    let new = edit(text.clone());
    assert_ne!(new, text, "{rel}: the edit changed nothing");
    fs.insert(rel, new);
    Case::open(&fs).unwrap()
}

#[test]
fn an_openfoam_8_limit_in_its_coeffs_dictionary_is_read_and_set_there() {
    let s = spec(OfVersion::V8, Settings::default(), true, false);
    let mut c = edited(&s, "system/air/fvOptions", |t| {
        t.replace(
            "    min             273.15;\n    max             373.15;",
            "    limitTemperatureCoeffs\n    {\n        min 280; // cold\n        max 360;\n    }",
        )
    });
    assert_eq!(Settings::from_case(&c).0.temperature_limit, Some(("280".into(), "360".into())));
    let before = c.dict("system/air/fvOptions").unwrap().to_string();
    c.apply_settings(&Settings { temperature_limit: Some(("283.15".into(), "370.15".into())), ..Default::default() })
        .unwrap();
    let after = c.dict("system/air/fvOptions").unwrap().to_string();
    assert_eq!(after, before.replace("min 280;", "min 283.15;").replace("max 360;", "max 370.15;"));
}

#[test]
fn the_ras_key_the_file_has_is_set_and_a_laminar_fluid_is_left_alone() {
    let s = spec(OfVersion::V12, Settings::default(), false, false);
    let rel = "constant/air/momentumTransport";
    let mut c = edited(&s, rel, |t| t.replace("RASModel            kEpsilon;", "model               kEpsilon;"));
    c.apply_settings(&Settings { turbulence: Some("realizableKE".into()), ..Default::default() }).unwrap();
    let m = c.dict(rel).unwrap();
    assert_eq!(m.value(&["RAS", "model"]).as_deref(), Some("realizableKE"));
    assert!(m.get(&["RAS", "RASModel"]).is_none());
    let mut c = edited(&s, rel, |t| t.replace("simulationType RAS;", "simulationType laminar;"));
    assert_eq!(Settings::from_case(&c).0.turbulence, None);
    let a = c.apply_settings(&Settings { turbulence: Some("realizableKE".into()), ..Default::default() }).unwrap();
    assert!(a.changed.is_empty() && a.notes[0].contains("not simulationType RAS"), "{a:?}");
}

#[test]
fn fluids_that_differ_are_noted_and_left_out() {
    let mut s = spec(OfVersion::V12, Settings::default(), true, false);
    s.regions.push(NewRegion { name: "air2".into(), kind: RegionKind::Fluid, materials: vec![] });
    let c = edited(&s, "constant/air2/momentumTransport", |t| t.replace("kEpsilon", "realizableKE"));
    let (got, notes) = Settings::from_case(&c);
    assert_eq!(got.turbulence, None);
    assert!(notes.iter().any(|n| n.contains("RAS models differ (kEpsilon, realizableKE)")), "{notes:?}");
}

#[test]
fn a_model_the_version_doesn_t_have_or_a_setting_given_twice_is_refused() {
    let (_, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    let e = c.choose_settings(&Settings { turbulence: Some("foo".into()), ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("turbulence foo is not a RAS model of OpenFOAM-12"), "{e}");
    c.choose_settings(&Settings { turbulence: Some("porousrealizableKE".into()), ..Default::default() }).unwrap();
    // A model of the case's own: kept when chosen again (the card starts with it).
    let s = spec(OfVersion::V12, Settings::default(), false, false);
    let mut own = edited(&s, "constant/air/momentumTransport", |t| t.replace("kEpsilon", "myKEpsilon"));
    let mine = Settings { turbulence: Some("myKEpsilon".into()), delta_t: Some("600".into()), ..Default::default() };
    own.choose_settings(&mine).unwrap();
    assert!(matches!(
        make(&spec(OfVersion::V8, Settings { turbulence: Some("kOmega2006".into()), ..Default::default() }, false, false)),
        Err(Error::NewCase(m)) if m.contains("not a RAS model of OpenFOAM-8")
    ));
    let s = spec(OfVersion::V12, Settings { delta_t: Some("1800".into()), ..Default::default() }, false, false);
    let mut c = edited(&s, crate::description::PATH, |t| {
        t.replace("    deltaT           1800;\n", "    deltaT           1800;\n    deltaT 900;\n")
    });
    let changed = c.changed();
    let e = c.choose_settings(&Settings { delta_t: Some("600".into()), ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("settings/deltaT is given twice"), "{e}");
    assert_eq!(c.changed(), changed);
}

#[test]
fn a_new_case_notes_a_limit_or_blending_it_has_no_place_for() {
    let chosen = Settings {
        temperature_limit: Some(("280".into(), "360".into())),
        blending: chosen_blending(),
        ..Default::default()
    };
    let m = make(&spec(OfVersion::V12, chosen.clone(), false, false)).unwrap();
    assert_eq!(m.notes.len(), 2, "{:?}", m.notes);
    assert_eq!(m.description.settings, chosen);
}

#[test]
fn values_a_case_has_that_the_settings_refuse_are_noted_and_left_out() {
    let s = spec(OfVersion::V12, Settings::default(), true, false);
    let c = edited(&s, CONTROL_DICT, |t| t.replace("deltaT          3600;", "deltaT          0;"));
    let (got, notes) = Settings::from_case(&c);
    assert!(got.delta_t.is_none());
    assert!(notes.iter().any(|n| n.contains("controlDict deltaT is 0")), "{notes:?}");
    let c = edited(&s, "system/air/fvConstraints", |t| t.replace("min 273.15; max 373.15;", "min 300; max 280;"));
    let (got, notes) = Settings::from_case(&c);
    assert!(got.temperature_limit.is_none());
    assert!(notes.iter().any(|n| n.contains("not 0 < min < max")), "{notes:?}");
    // The imported description has no problems, so the scripts can be written from it.
    let (d, _) = Description::import(&c);
    let (_, problems) = Description::read(&Document::parse(&d.write()).unwrap());
    assert_eq!(problems, vec![]);
}

#[test]
fn an_incomplete_temperature_limit_agrees_with_no_bounds() {
    let mut s = spec(OfVersion::V12, Settings::default(), true, false);
    s.regions.push(NewRegion { name: "air2".into(), kind: RegionKind::Fluid, materials: vec![] });
    let c = edited(&s, "system/air2/fvConstraints", |t| t.replace("min 273.15; ", ""));
    let (got, notes) = Settings::from_case(&c);
    assert!(got.temperature_limit.is_none());
    assert!(
        notes.iter().any(|n| n.contains("system/air2/fvConstraints: its limitTemperature entry has no min or no max")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("temperature limits differ")), "{notes:?}");
    // Every fluid's incomplete: one note each, no other.
    let c = edited(&spec(OfVersion::V12, Settings::default(), true, false), "system/air/fvConstraints", |t| {
        t.replace("min 273.15; ", "")
    });
    let (got, notes) = Settings::from_case(&c);
    assert!(got.temperature_limit.is_none());
    assert_eq!(notes, ["system/air/fvConstraints: its limitTemperature entry has no min or no max"]);
    // A complete entry whose value is `?` is a value: noted as one, not taken for an incomplete one.
    let c = edited(&spec(OfVersion::V12, Settings::default(), true, false), "system/air/fvConstraints", |t| {
        t.replace("min 273.15;", "min ?;")
    });
    let (got, notes) = Settings::from_case(&c);
    assert!(got.temperature_limit.is_none());
    assert!(notes.iter().any(|n| n.contains("( ? 373.15 ): not 0 < min < max")), "{notes:?}");
}

/// What the solver reads of every material, `rho` as given.
fn full(rho: &str) -> Vec<(String, String)> {
    [("rho", rho), ("cap", "850"), ("lambda1", "1.3"), ("lambda2", "0")]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn concrete() -> Library {
    vec![(
        "HamstadConcrete".into(),
        vec![
            ("rho".into(), "1950".into()),
            ("cap".into(), "850".into()),
            ("lambda1".into(), "1.3".into()),
            ("lambda2".into(), "0".into()),
        ],
    )]
}

fn with_solid(version: OfVersion, settings: Settings, materials: &[(&str, &str)]) -> NewCase {
    let mut s = spec(version, settings, false, false);
    s.regions.push(NewRegion {
        name: "street".into(),
        kind: RegionKind::Solid,
        materials: materials
            .iter()
            .map(|(n, m)| crate::description::MaterialDesc { name: (*n).into(), model: Some((*m).into()) })
            .collect(),
    });
    s
}

#[test]
fn the_material_library_reads_back_as_written_and_refuses_what_it_isn_t() {
    let mut lib = concrete();
    lib.push((
        "VanGenuchten".into(),
        vec![("rho".into(), "1000".into()), ("VanGenuchtenCoeffs".into(), "{ n 2; alpha 0.1; }".into())],
    ));
    let s = Settings { materials: lib.clone(), ..Default::default() };
    let text = s.write();
    assert!(
        text.contains(
            "    materials\n    {\n        HamstadConcrete  { rho 1950; cap 850; lambda1 1.3; lambda2 0; }\n"
        ),
        "{text}"
    );
    let (back, problems) = Description::read(
        &Document::parse(&Description { settings: s.clone(), ..Default::default() }.write()).unwrap(),
    );
    assert_eq!(problems, vec![]);
    assert_eq!(back.settings.materials, lib);
    let (s, problems) =
        read("materials { Soil { rho $r; } \"a b\" { rho 1; } Empty { } Brick { rho 1; rho 2; } Soil { rho 3; } }");
    assert!(s.materials.is_empty(), "{:?}", s.materials);
    for want in [
        "settings/materials/Soil/rho: is a number",
        "settings/materials/a b: is not a material model's name",
        "settings/materials/Empty: has no coefficients",
        "settings/materials/Brick/rho: is a number, or a dictionary of numbers, given once",
        "settings/materials/Soil: is given twice",
    ] {
        assert!(problems.iter().any(|p| p.starts_with(want)), "{want}: {problems:?}");
    }
}

#[test]
fn the_library_is_read_from_a_case_s_solids_and_leaves_their_materials_as_they_are() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let (_, mut c) = opened(&with_solid(v, Settings::default(), &[("soil", "Soil"), ("brick", "Hamstad5Brick")]));
        let (s, notes) = Settings::from_case(&c);
        assert_eq!(notes, Vec::<String>::new());
        let models: Vec<&str> = s.materials.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(models, ["Soil", "Hamstad5Brick"], "{v:?}");
        let soil = vec![("rho".to_string(), "1200".to_string())];
        let a = c.apply_settings(&Settings { materials: vec![("Soil".into(), soil)], ..Default::default() }).unwrap();
        assert!(a.changed.is_empty() && a.notes[0].contains("a material library is for new materials"), "{a:?}");
    }
    // Two materials of one model that differ (layers): one note, the first's taken.
    let s = with_solid(OfVersion::V12, Settings::default(), &[("a", "Soil"), ("b", "Soil"), ("c", "Soil")]);
    let c = edited(&s, "constant/street/transportProperties", |t| {
        let i = t.rfind("rho     1150;").unwrap();
        format!("{}rho     1300;{}", &t[..i], &t[i + "rho     1150;".len()..])
    });
    let (s, notes) = Settings::from_case(&c);
    // In the case's order.
    assert_eq!(s.materials[0].1[0], ("rho".to_string(), "1150".to_string()));
    assert_eq!(
        notes,
        ["Soil materials differ (street/c besides street/a): the library has the first's, for new Soil materials"]
    );
}

#[test]
fn a_new_solid_takes_the_library_s_coefficients_and_a_model_the_solver_lacks_is_refused() {
    let m = make(&with_solid(
        OfVersion::V8,
        Settings { materials: concrete(), ..Default::default() },
        &[("pavement", "HamstadConcrete")],
    ))
    .unwrap();
    assert_eq!(m.notes, Vec::<String>::new());
    let (_, text) = m.files.iter().find(|(p, _)| p == "constant/street/transportProperties").unwrap();
    assert!(
        text.contains("buildingMaterialModel HamstadConcrete;\n        rho     1950;\n        cap     850;"),
        "{text}"
    );
    let bad = Settings { materials: vec![("Marble".into(), full("1"))], ..Default::default() };
    assert!(
        matches!(make(&spec(OfVersion::V12, bad.clone(), false, false)), Err(Error::NewCase(m)) if m.contains("materials/Marble is not a material model"))
    );
    let (_, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    assert!(c.choose_settings(&bad).is_err());
    // region.add: the description's library first.
    let (fs, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    c.choose_settings(&Settings { materials: concrete(), ..Default::default() }).unwrap();
    let r = NewRegion {
        name: "ground".into(),
        kind: RegionKind::Solid,
        materials: vec![crate::description::MaterialDesc {
            name: "pavement".into(),
            model: Some("HamstadConcrete".into()),
        }],
    };
    let added = c.add_region(&fs, &r).unwrap();
    assert!(!added.notes.iter().any(|n| n.contains("coefficients")), "{:?}", added.notes);
    let tp = c.dict("constant/ground/transportProperties").unwrap().value(&["buildingMaterials"]).unwrap();
    assert!(tp.contains("rho 1950"), "{tp}");
}

#[test]
fn a_material_with_a_macro_or_directive_is_noted_and_left_out_of_the_library() {
    let s = with_solid(OfVersion::V12, Settings::default(), &[("soil", "Soil"), ("brick", "Hamstad5Brick")]);
    let c = edited(&s, "constant/street/transportProperties", |t| {
        t.replacen("        rho     1150;", "        $common;\n        rho     1150;", 1)
    });
    let (lib, notes) = Settings::from_case(&c);
    let models: Vec<&str> = lib.materials.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(models, ["Hamstad5Brick"]);
    assert!(notes.iter().any(|n| n.contains("material soil (Soil)")), "{notes:?}");
    let (_, problems) = read("materials { Soil { #include \"coeffs\" rho 1; } }");
    assert!(problems.iter().any(|p| p.contains("settings/materials/Soil: has a macro or a directive")), "{problems:?}");
}

#[test]
fn the_case_s_own_models_are_kept_and_a_materials_block_given_twice_is_refused() {
    let s = with_solid(OfVersion::V12, Settings::default(), &[("brick", "Hamstad5Brick")]);
    let mut c = edited(&s, "constant/street/transportProperties", |t| t.replace("Hamstad5Brick", "MyBrick"));
    // MyBrick is the case's own: the card's settings, given back, are taken.
    let (mine, _) = Settings::from_case(&c);
    assert_eq!(mine.materials[0].0, "MyBrick");
    c.choose_settings(&mine).unwrap();
    // region.add takes the description's MyBrick, without a note.
    let mut chosen = mine.clone();
    chosen.materials[0].1 = full("777");
    c.choose_settings(&chosen).unwrap();
    let fs2 = MemFs::new();
    let r = NewRegion {
        name: "walls".into(),
        kind: RegionKind::Solid,
        materials: vec![crate::description::MaterialDesc { name: "w".into(), model: Some("MyBrick".into()) }],
    };
    let added = c.add_region(&fs2, &r).unwrap();
    assert!(!added.notes.iter().any(|n| n.contains("MyBrick")), "{:?}", added.notes);
    assert!(
        c.dict("constant/walls/transportProperties")
            .unwrap()
            .value(&["buildingMaterials"])
            .unwrap()
            .contains("rho 777")
    );
    // A materials block given twice in the description: refused.
    let mut fs = MemFs::new();
    let text = Description { settings: Settings { materials: concrete(), ..Default::default() }, ..Default::default() }
        .write();
    fs.insert("system/controlDict", "FoamFile { class dictionary; object controlDict; }\nregions { fluid ( air ); }\n");
    let twice = text.replace("    materials\n", "    materials { Soil { rho 1; } }\n    materials\n");
    fs.insert(crate::description::PATH, twice);
    let mut c = Case::open(&fs).unwrap();
    let e = c.choose_settings(&Settings { materials: concrete(), ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("settings/materials is given twice"), "{e}");
    let once = text.replace("    materials\n    {\n", "    materials\n    {\n        HamstadConcrete  { rho 1; }\n");
    fs.insert(crate::description::PATH, once);
    let mut c = Case::open(&fs).unwrap();
    let e = c.choose_settings(&Settings { materials: concrete(), ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("settings/materials/HamstadConcrete is given twice"), "{e}");
}

#[test]
fn a_coefficients_dictionary_goes_into_a_new_material_whole() {
    let lib = vec![(
        "VanGenuchten".to_string(),
        vec![
            ("rho".to_string(), "1000".to_string()),
            ("cap".to_string(), "800".to_string()),
            ("lambda1".to_string(), "1".to_string()),
            ("lambda2".to_string(), "0".to_string()),
            ("VanGenuchtenCoeffs".to_string(), "{ n 2; alpha 0.1; }".to_string()),
        ],
    )];
    let m = make(&with_solid(
        OfVersion::V12,
        Settings { materials: lib.clone(), ..Default::default() },
        &[("loam", "VanGenuchten")],
    ))
    .unwrap();
    let (_, text) = m.files.iter().find(|(p, _)| p == "constant/street/transportProperties").unwrap();
    let doc = Document::parse(text).unwrap();
    assert!(doc.root.problems().is_empty());
    let list = doc.value(&["buildingMaterials"]).unwrap();
    assert!(list.contains("VanGenuchtenCoeffs { n 2; alpha 0.1; }"), "{list}");
}

#[test]
fn a_new_solid_takes_the_description_s_library_before_the_case_s_materials() {
    let s = with_solid(OfVersion::V12, Settings::default(), &[("soil", "Soil")]);
    let (fs, mut c) = opened(&s);
    let mine = vec![("Soil".to_string(), full("999"))];
    c.choose_settings(&Settings { materials: mine, ..Default::default() }).unwrap();
    let r = NewRegion {
        name: "ground".into(),
        kind: RegionKind::Solid,
        materials: vec![crate::description::MaterialDesc { name: "earth".into(), model: Some("Soil".into()) }],
    };
    c.add_region(&fs, &r).unwrap();
    let tp = c.dict("constant/ground/transportProperties").unwrap().value(&["buildingMaterials"]).unwrap();
    assert!(tp.contains("rho 999") && !tp.contains("1150"), "{tp}");
    // The case's own solid keeps its own.
    assert!(c.dict("constant/street/transportProperties").unwrap().to_string().contains("1150"));
}

#[test]
fn a_library_chosen_without_a_description_is_said_not_to_be_kept() {
    let mut fs = MemFs::new();
    for (p, t) in make(&with_solid(OfVersion::V12, Settings::default(), &[("soil", "Soil")])).unwrap().files {
        if p != crate::description::PATH {
            fs.insert(&p, t);
        }
    }
    let mut c = Case::open(&fs).unwrap();
    let chosen = c.choose_settings(&Settings { materials: concrete(), ..Default::default() }).unwrap();
    assert!(!chosen.described);
    assert!(
        chosen.applied.notes.iter().any(|n| n.contains("the material library is not kept")),
        "{:?}",
        chosen.applied.notes
    );
    assert!(!chosen.applied.notes.iter().any(|n| n.contains("is kept in")), "{:?}", chosen.applied.notes);
}

#[test]
fn a_library_entry_without_what_the_solver_reads_of_a_material_is_refused() {
    let partial = Settings {
        materials: vec![("HamstadConcrete".into(), vec![("rho".into(), "1950".into())])],
        ..Default::default()
    };
    assert_eq!(required_coefficients(OfVersion::V12), ["cap", "lambda1", "lambda2", "rho"]);
    let e = make(&with_solid(OfVersion::V12, partial.clone(), &[("pavement", "HamstadConcrete")])).unwrap_err();
    assert!(e.to_string().contains("materials/HamstadConcrete has no cap, lambda1, lambda2"), "{e}");
    let (_, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    let e = c.choose_settings(&partial).unwrap_err();
    assert!(e.to_string().contains("materials/HamstadConcrete has no cap"), "{e}");
    // A model of the case's own too: exempt from the model check, not from this one.
    let s = with_solid(OfVersion::V12, Settings::default(), &[("brick", "Hamstad5Brick")]);
    let mut own = edited(&s, "constant/street/transportProperties", |t| t.replace("Hamstad5Brick", "MyBrick"));
    let mine = Settings { materials: vec![("MyBrick".into(), vec![("rho".into(), "1".into())])], ..Default::default() };
    let e = own.choose_settings(&mine).unwrap_err();
    assert!(e.to_string().contains("materials/MyBrick has no cap"), "{e}");
    // One written into the description by hand: left out of a new region, with a note.
    let (fs, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    c.set(crate::description::PATH, &["settings", "materials", "HamstadConcrete", "rho"], "1950").unwrap();
    let r = NewRegion {
        name: "ground".into(),
        kind: RegionKind::Solid,
        materials: vec![crate::description::MaterialDesc {
            name: "pavement".into(),
            model: Some("HamstadConcrete".into()),
        }],
    };
    let added = c.add_region(&fs, &r).unwrap();
    assert!(
        added.notes.iter().any(|n| n.contains("materials/HamstadConcrete lacks coefficients the solver reads")),
        "{:?}",
        added.notes
    );
}

fn leaf(path: &[&str], v: &str) -> Leaf {
    (path.iter().map(|p| p.to_string()).collect(), v.to_string())
}

#[test]
fn the_numerics_read_back_as_written_and_refuse_what_they_aren_t() {
    let numerics = vec![(
        "fluid".to_string(),
        vec![
            leaf(&["relaxationFactors", "fields", "p_rgh"], "0.3"),
            leaf(&["relaxationFactors", "equations", "(h|e)"], "0.5"),
            leaf(&["SIMPLE", "nNonOrthogonalCorrectors"], "1"),
            leaf(&["solvers", "p_rgh", "solver"], "GAMG"),
        ],
    )];
    let s = Settings { numerics: numerics.clone(), ..Default::default() };
    let text = s.write();
    assert!(text.contains("fluid            { relaxationFactors { fields { p_rgh 0.3; } equations { \"(h|e)\" 0.5; } } SIMPLE { nNonOrthogonalCorrectors 1; } solvers { p_rgh { solver GAMG; } } }"), "{text}");
    let (back, problems) =
        Description::read(&Document::parse(&Description { settings: s, ..Default::default() }.write()).unwrap());
    assert_eq!(problems, vec![]);
    assert_eq!(back.settings.numerics, numerics);
    let (s, problems) = read(
        "numerics { gas { SIMPLE { x 1; } } fluid { schemes { x 1; } } solid { solvers { Ts { tolerance $t; } } } vegetation { solvers { a { b { c { d 1; } } } } } }",
    );
    assert!(s.numerics.is_empty(), "{:?}", s.numerics);
    for want in [
        "numerics/gas: is not a region kind",
        "numerics/fluid/schemes: is not one of",
        "numerics/solid/solvers: holds numbers",
        "numerics/vegetation/solvers: holds numbers",
    ] {
        assert!(problems.iter().any(|p| p.starts_with(&format!("settings/{want}"))), "{want}: {problems:?}");
    }
    // An entry or a root given twice: which counts can't be told. A key with unbalanced
    // brackets would be written unquoted and break the file.
    for (text, want) in [
        ("numerics { fluid { SIMPLE { n 1; n 2; } } }", "numerics/fluid/SIMPLE/n: is given twice"),
        ("numerics { fluid { SIMPLE { n 1; } SIMPLE { m 2; } } }", "numerics/fluid/SIMPLE: is given twice"),
    ] {
        let (s, problems) = read(text);
        assert!(s.numerics.is_empty(), "{text}: {:?}", s.numerics);
        assert!(problems.iter().any(|p| p.starts_with(&format!("settings/{want}"))), "{want}: {problems:?}");
    }
    assert!(numerics_key("(h|e)") && numerics_key("U.*"));
    assert!(!numerics_key("a)") && !numerics_key(")(") && !numerics_key("(a"));
}

#[test]
fn a_case_s_numerics_are_what_differ_from_the_configuration_and_go_where_they_belong() {
    for v in [OfVersion::V8, OfVersion::V12] {
        let s = with_solid(v, Settings::default(), &[("soil", "Soil")]);
        let (_, c) = opened(&s);
        let (got, notes) = Settings::from_case(&c);
        assert!(got.numerics.is_empty() && notes.is_empty(), "{v:?}: {:?} {notes:?}", got.numerics);
        let mut c = edited(&s, "system/air/fvSolution", |t| {
            t.replacen("nNonOrthogonalCorrectors 0;", "nNonOrthogonalCorrectors 2;", 1)
        });
        let (got, _) = Settings::from_case(&c);
        assert_eq!(
            got.numerics,
            vec![("fluid".to_string(), vec![leaf(&["SIMPLE", "nNonOrthogonalCorrectors"], "2")])],
            "{v:?}"
        );
        // Written into the solid's: only that line changes.
        let rel = "system/street/fvSolution";
        let before = c.dict(rel).unwrap().to_string();
        let chosen = vec![("solid".to_string(), vec![leaf(&["solvers", "Ts", "tolerance"], "1e-8")])];
        let a = c.apply_settings(&Settings { numerics: chosen.clone(), ..Default::default() }).unwrap();
        assert_eq!(a.changed, [rel], "{v:?}");
        let after = c.dict(rel).unwrap().to_string();
        let diff: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(x, y)| x != y).collect();
        assert_eq!(diff.len(), 1, "{v:?} {diff:?}");
        assert_eq!(before.lines().count(), after.lines().count());
        // A new case takes them.
        let m = make(&with_solid(v, Settings { numerics: chosen.clone(), ..Default::default() }, &[("soil", "Soil")]))
            .unwrap();
        let (_, text) = m.files.iter().find(|(p, _)| p == rel).unwrap();
        assert_eq!(Document::parse(text).unwrap().value(&["solvers", "Ts", "tolerance"]).as_deref(), Some("1e-8"));
    }
    // Two fluids whose numerics differ: noted, the first's taken.
    let mut s = spec(OfVersion::V12, Settings::default(), false, false);
    s.regions.push(NewRegion { name: "air2".into(), kind: RegionKind::Fluid, materials: vec![] });
    let c = edited(&s, "system/air2/fvSolution", |t| {
        t.replacen("nNonOrthogonalCorrectors 0;", "nNonOrthogonalCorrectors 2;", 1)
    });
    let (got, notes) = Settings::from_case(&c);
    assert!(got.numerics.is_empty());
    assert!(notes.iter().any(|n| n.contains("fluid regions' numerics differ (air2 besides air)")), "{notes:?}");
    // The same entries in another order: no difference.
    let c = edited(&s, "system/air2/fvSolution", |t| {
        let a = t.find("SIMPLE").unwrap();
        let b = t.find("relaxationFactors").unwrap();
        let end = b + t[b..].find("\n}").unwrap() + 2;
        format!("{}{}\n{}{}", &t[..a], &t[b..end], &t[a..b], &t[end..])
    });
    let (_, notes) = Settings::from_case(&c);
    assert!(!notes.iter().any(|n| n.contains("differ")), "{notes:?}");
    // What they share is taken, what differs isn't: chosen back, no file changes.
    let mut c = edited(&s, "system/air2/fvSolution", |t| {
        t.replacen("nNonOrthogonalCorrectors 0;", "nNonOrthogonalCorrectors 2;", 1).replacen(
            "tolerance        1e-9;",
            "tolerance        1e-7;",
            1,
        )
    });
    c.set("system/air/fvSolution", &["solvers", "p_rgh", "tolerance"], "1e-7").unwrap();
    let (got, _) = Settings::from_case(&c);
    assert_eq!(got.numerics, vec![("fluid".to_string(), vec![leaf(&["solvers", "p_rgh", "tolerance"], "1e-7")])]);
    let a = c.apply_settings(&Settings { numerics: got.numerics, ..Default::default() }).unwrap();
    assert!(a.changed.is_empty(), "{a:?}");
}

#[test]
fn numerics_never_replace_a_dictionary_nor_add_a_field_s_solver_dictionary() {
    let s = spec(OfVersion::V12, Settings::default(), false, false);
    let (_, mut c) = opened(&s);
    let rel = "system/air/fvSolution";
    let before = c.dict(rel).unwrap().to_string();
    let bad = vec![(
        "fluid".to_string(),
        vec![
            // A word where the file has p_rgh's dictionary.
            leaf(&["solvers", "p_rgh"], "GAMG"),
            // A value where the file has the equations' dictionary.
            leaf(&["relaxationFactors", "equations"], "0.5"),
            // A field the file has no entry of (a pattern covers it).
            leaf(&["solvers", "U", "tolerance"], "1e-8"),
        ],
    )];
    let a = c.apply_settings(&Settings { numerics: bad, ..Default::default() }).unwrap();
    assert!(a.changed.is_empty(), "{a:?}");
    assert_eq!(c.dict(rel).unwrap().to_string(), before);
    assert_eq!(a.notes.len(), 3, "{:?}", a.notes);
    assert!(a.notes[0].contains("solvers/p_rgh: is a dictionary in the file"), "{:?}", a.notes);
    assert!(a.notes[2].contains("the file has no solvers/U (a pattern may cover the field)"), "{:?}", a.notes);
    // Not even with its solver: it would hold only the entries given. A field the file has
    // changes; a pattern key is quoted.
    let good = vec![(
        "fluid".to_string(),
        vec![
            leaf(&["solvers", "T", "solver"], "PBiCGStab"),
            leaf(&["solvers", "p_rgh", "tolerance"], "1e-8"),
            leaf(&["relaxationFactors", "equations", "k.*"], "0.6"),
        ],
    )];
    let a = c.apply_settings(&Settings { numerics: good, ..Default::default() }).unwrap();
    assert!(a.notes.iter().any(|n| n.contains("solvers/T/solver")), "{:?}", a.notes);
    let after = c.dict(rel).unwrap().to_string();
    assert!(after.contains("\"k.*\""), "{after}");
    let doc = Document::parse(&after).unwrap();
    assert_eq!(doc.get(&["solvers", "T"]).map(|_| ()), None);
    assert_eq!(doc.value(&["solvers", "p_rgh", "tolerance"]).as_deref(), Some("1e-8"));
    assert_eq!(doc.value(&["solvers", "p_rgh", "smoother"]).as_deref(), Some("DICGaussSeidel"));
    // A file without solvers: none is added with a field's partial dictionary.
    let mut bare = Document::parse("FoamFile { class dictionary; object fvSolution; }\nSIMPLE { n 1; }\n").unwrap();
    let put = put_leaf(&mut bare, &["solvers".into(), "p_rgh".into(), "tolerance".into()], "1e-8").unwrap();
    assert!(matches!(&put, Put::Skipped(w) if w.contains("the file has no solvers (a pattern")), "{}", bare);
    assert!(bare.get(&["solvers"]).is_none());
    // A file with a directive at its top: a dictionary it lacks isn't added next to it, and
    // reading it notes the directive.
    let mut c = edited(&s, rel, |t| {
        let at = t.find("relaxationFactors").unwrap();
        let end = at + t[at..].find("\n}").unwrap() + 2;
        format!("{}#include \"../fvSolution.common\"\n{}", &t[..at], &t[end..])
    });
    let (_, notes) = Settings::from_case(&c);
    assert!(notes.iter().any(|n| n.contains("system/air/fvSolution: holds what the numerics don't take")), "{notes:?}");
    let rf = vec![("fluid".to_string(), vec![leaf(&["relaxationFactors", "fields", "p_rgh"], "0.3")])];
    let a = c.apply_settings(&Settings { numerics: rf, ..Default::default() }).unwrap();
    assert!(a.changed.is_empty(), "{a:?}");
    assert!(
        a.notes.iter().any(|n| n.contains("a directive or a macro at its top, which may bring it")),
        "{:?}",
        a.notes
    );
    // One level down too: a macro in relaxationFactors may bring its fields.
    let mut c = edited(&s, rel, |t| t.replacen("relaxationFactors\n{", "relaxationFactors\n{\n    $common;", 1));
    let rf = vec![("fluid".to_string(), vec![leaf(&["relaxationFactors", "other", "p_rgh"], "0.3")])];
    let a = c.apply_settings(&Settings { numerics: rf, ..Default::default() }).unwrap();
    assert!(a.changed.is_empty(), "{a:?}");
    assert!(a.notes.iter().any(|n| n.contains("a directive or a macro in relaxationFactors")), "{:?}", a.notes);
    // A value right under solvers: refused (a field's entry there is a dictionary).
    let (got, problems) = read("numerics { fluid { solvers { T GAMG; } } }");
    assert!(got.numerics.is_empty());
    assert!(
        problems.iter().any(|p| p.starts_with("settings/numerics/fluid/solvers: holds a dictionary per field")),
        "{problems:?}"
    );
    // A stray ; after a dictionary (OpenFOAM takes it) is no directive: read and written as any.
    let mut c = edited(&s, rel, |t| {
        let at = t.find("relaxationFactors").unwrap();
        let end = at + t[at..].find("\n}").unwrap() + 2;
        format!("{};{}", &t[..end], &t[end..])
    });
    let (_, notes) = Settings::from_case(&c);
    assert!(notes.is_empty(), "{notes:?}");
    let pimple = vec![("fluid".to_string(), vec![leaf(&["PISO", "nCorrectors"], "2")])];
    let a = c.apply_settings(&Settings { numerics: pimple, ..Default::default() }).unwrap();
    assert_eq!(a.changed, [rel], "{a:?}");
    // Given twice in the description: refused.
    let mut fs = MemFs::new();
    fs.insert("system/controlDict", "FoamFile { class dictionary; object controlDict; }\nregions { fluid ( air ); }\n");
    let one = Settings {
        numerics: vec![("fluid".into(), vec![leaf(&["SIMPLE", "nNonOrthogonalCorrectors"], "1")])],
        ..Default::default()
    };
    let text = Description { settings: one.clone(), ..Default::default() }.write();
    fs.insert(
        crate::description::PATH,
        text.replace("    numerics\n", "    numerics { solid { SIMPLE { x 1; } } }\n    numerics\n"),
    );
    let mut c = Case::open(&fs).unwrap();
    let e = c.choose_settings(&one).unwrap_err();
    assert!(e.to_string().contains("settings/numerics is given twice"), "{e}");
}

#[test]
fn a_new_region_takes_the_description_s_numerics_entry_by_entry_over_the_case_s() {
    let s = with_solid(OfVersion::V12, Settings::default(), &[("soil", "Soil")]);
    let mut c = edited(&s, "system/air/fvSolution", |t| {
        t.replacen("nNonOrthogonalCorrectors 0;", "nNonOrthogonalCorrectors 2;", 1)
    });
    // The description chooses another entry: the new fluid gets both.
    c.set(crate::description::PATH, &["settings", "numerics", "fluid", "relaxationFactors", "fields", "p_rgh"], "0.2")
        .unwrap();
    let mem = MemFs::new();
    c.add_region(&mem, &NewRegion { name: "wind".into(), kind: RegionKind::Fluid, materials: vec![] }).unwrap();
    let doc = c.dict("system/wind/fvSolution").unwrap();
    assert_eq!(doc.value(&["SIMPLE", "nNonOrthogonalCorrectors"]).as_deref(), Some("2"));
    assert_eq!(doc.value(&["relaxationFactors", "fields", "p_rgh"]).as_deref(), Some("0.2"));
    // Merged entry by entry: a value over a dictionary, or an entry under a value, replaces it.
    let mut base = vec![(
        "fluid".to_string(),
        vec![leaf(&["SIMPLE", "residualControl", "p_rgh"], "1e-3"), leaf(&["PIMPLE", "nOuterCorrectors"], "1")],
    )];
    let over = vec![(
        "fluid".to_string(),
        vec![leaf(&["SIMPLE", "residualControl"], "1e-4"), leaf(&["PIMPLE", "nOuterCorrectors", "x"], "2")],
    )];
    merge_numerics(&mut base, &over);
    assert_eq!(base, over);
}

#[test]
fn a_value_doesn_t_replace_a_dictionary_of_the_description() {
    let (_, mut c) = opened(&spec(OfVersion::V12, Settings::default(), false, false));
    let two = vec![(
        "fluid".to_string(),
        vec![
            leaf(&["relaxationFactors", "equations", "U"], "0.3"),
            leaf(&["relaxationFactors", "equations", "k"], "0.4"),
        ],
    )];
    c.choose_settings(&Settings { numerics: two.clone(), ..Default::default() }).unwrap();
    let one = vec![("fluid".to_string(), vec![leaf(&["relaxationFactors", "equations"], "0.5")])];
    let changed = c.changed();
    let before = c.dict(crate::description::PATH).unwrap().to_string();
    let e = c.choose_settings(&Settings { numerics: one, ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("relaxationFactors/equations is a dictionary"), "{e}");
    assert_eq!(c.changed(), changed);
    assert_eq!(c.dict(crate::description::PATH).unwrap().to_string(), before);
    assert_eq!(c.description().unwrap().0.settings.numerics, two);
}
