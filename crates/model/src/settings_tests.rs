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
