use umcf_core::{MemFs, OfVersion};

use super::*;
use crate::description::{Components, case_components};
use crate::new_case::{NewCase, NewRegion, Up, make};
use crate::settings::Settings;

fn spec(version: OfVersion, components: Components) -> NewCase {
    NewCase {
        version,
        regions: vec![NewRegion { name: "air".into(), kind: RegionKind::Fluid, materials: vec![] }],
        radiation: Some("air".into()),
        components,
        grass_patches: vec![],
        up: Up::Z,
        settings: Settings::default(),
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

fn one(word: &str) -> Components {
    let mut c = Components::default();
    assert!(c.switch_on(word));
    c
}

/// The case saved and opened again: what its files say, read afresh.
fn reopened(fs: &mut MemFs, c: &mut Case) -> Case {
    c.save(fs, None).unwrap();
    Case::open(fs).unwrap()
}

#[test]
fn a_component_switched_on_is_as_a_new_case_with_it_and_off_again_as_one_without() {
    for v in [OfVersion::V8, OfVersion::V12] {
        for word in COMPONENTS {
            let (mut fs, mut c) = opened(&spec(v, Components::default()));
            let (_, with) = opened(&spec(v, one(word)));
            let s = c.switch_component(&fs, word, true, None).unwrap();
            assert!(s.described, "{v:?} {word}");
            assert!(s.changed.contains(&crate::description::PATH.to_string()), "{v:?} {word}: {s:?}");
            let back = reopened(&mut fs, &mut c);
            assert_eq!(case_components(&back), one(word), "{v:?} {word}");
            assert_eq!(back.description().unwrap().0.components, one(word), "{v:?} {word}");
            // The settings the files give are the new case's.
            assert_eq!(Settings::from_case(&back).0, Settings::from_case(&with).0, "{v:?} {word}");
            // Again: nothing more to do.
            let mut again = back.clone();
            let s = again.switch_component(&fs, word, true, None).unwrap();
            assert!(s.changed.is_empty(), "{v:?} {word}: {s:?}");
            // Off: as a case without it.
            let mut c = back;
            c.switch_component(&fs, word, false, None).unwrap();
            let back = reopened(&mut fs, &mut c);
            assert_eq!(case_components(&back), Components::default(), "{v:?} {word}");
            assert_eq!(back.description().unwrap().0.components, Components::default(), "{v:?} {word}");
            assert!(!back.dict(crate::description::PATH).unwrap().to_string().contains("components"));
        }
    }
}

#[test]
fn what_a_component_had_is_kept_for_switching_it_back_on() {
    // The temperature limit's bounds go into the description's settings.
    let (mut fs, mut c) = opened(&spec(OfVersion::V12, one("temperatureLimit")));
    c.apply_settings(&Settings { temperature_limit: Some(("280".into(), "360".into())), ..Default::default() })
        .unwrap();
    let s = c.switch_component(&fs, "temperatureLimit", false, None).unwrap();
    assert!(s.notes.iter().any(|n| n.contains("( 280 360 ) kept")), "{s:?}");
    let mut c = reopened(&mut fs, &mut c);
    assert!(!c.dict("system/air/fvConstraints").unwrap().to_string().contains("limitTemperature"));
    c.switch_component(&fs, "temperatureLimit", true, None).unwrap();
    assert_eq!(Settings::from_case(&c).0.temperature_limit, Some(("280".into(), "360".into())));
    // The blending layer keeps its coefficients.
    let (fs, mut c) = opened(&spec(OfVersion::V8, one("blending")));
    c.set(CONTROL_DICT, &["blendingCoeffs", "dampingThickness"], "45").unwrap();
    c.switch_component(&fs, "blending", false, None).unwrap();
    assert_eq!(c.dict(CONTROL_DICT).unwrap().value(&["blending"]).as_deref(), Some("false"));
    c.switch_component(&fs, "blending", true, None).unwrap();
    assert_eq!(c.dict(CONTROL_DICT).unwrap().value(&["blendingCoeffs", "dampingThickness"]).as_deref(), Some("45"));
    // The grass keeps its model's coefficients and patches.
    let mut s = spec(OfVersion::V12, one("grass"));
    s.grass_patches = vec!["ground".into()];
    let (fs, mut c) = opened(&s);
    c.switch_component(&fs, "grass", false, None).unwrap();
    let g = c.dict("constant/air/grassProperties").unwrap();
    assert_eq!(g.value(&["grassModel"]).as_deref(), Some("none"));
    assert!(g.to_string().contains("ground"));
    let on = c.switch_component(&fs, "grass", true, None).unwrap();
    assert!(!on.notes.iter().any(|n| n.contains("no grass patches")), "{on:?}");
    assert_eq!(c.dict("constant/air/grassProperties").unwrap().value(&["grassModel"]).as_deref(), Some("simpleGrass"));
}

#[test]
fn the_grass_s_patches_are_air_s() {
    let (mut fs, _) = opened(&spec(OfVersion::V12, Components::default()));
    fs.insert(
        "constant/air/polyMesh/boundary",
        "FoamFile { class polyBoundaryMesh; object boundary; }\n2\n(\n    ground { type wall; nFaces 1; startFace 0; }\n    top { type patch; nFaces 1; startFace 1; }\n)\n",
    );
    let mut c = Case::open(&fs).unwrap();
    let e = c.switch_component(&fs, "grass", true, Some(&["lawn".into()])).unwrap_err();
    assert!(e.to_string().contains("grass patch lawn: region air has no such patch"), "{e}");
    let e = c.switch_component(&fs, "grass", true, Some(&["ground".into(), "ground".into()])).unwrap_err();
    assert!(e.to_string().contains("given twice"), "{e}");
    let s = c.switch_component(&fs, "grass", true, Some(&["ground".into()])).unwrap();
    assert!(!s.notes.iter().any(|n| n.contains("no grass patches")), "{s:?}");
    let g = c.dict("constant/air/grassProperties").unwrap();
    assert_eq!(g.value(&["simpleGrassCoeffs", "grassPatches"]).as_deref(), Some("(ground)"));
    // Without patches: the tutorial's aren't taken, and a note says to name them.
    let (fs, mut c) = opened(&spec(OfVersion::V8, Components::default()));
    let s = c.switch_component(&fs, "grass", true, None).unwrap();
    assert!(s.notes.iter().any(|n| n.contains("no grass patches")), "{s:?}");
    assert!(!c.dict("constant/air/grassProperties").unwrap().to_string().contains("air_to_street"));
}

#[test]
fn what_a_component_needs_is_refused_without_and_nothing_changes() {
    // The grass without the radiation.
    let mut s = spec(OfVersion::V12, Components::default());
    s.radiation = None;
    let (fs, mut c) = opened(&s);
    let before = c.clone();
    let e = c.switch_component(&fs, "grass", true, None).unwrap_err();
    assert!(e.to_string().contains("the grass needs the radiation"), "{e}");
    assert_eq!(c.changed(), before.changed());
    // The blending layer with the gravity along y.
    let mut s = spec(OfVersion::V8, Components::default());
    s.up = Up::Y;
    let (fs, mut c) = opened(&s);
    let e = c.switch_component(&fs, "blending", true, None).unwrap_err();
    assert!(e.to_string().contains("the blending layer needs z up"), "{e}");
    assert!(c.changed().is_empty());
    // Not a component; patches when switching off.
    let e = c.switch_component(&fs, "trees", true, None).unwrap_err();
    assert!(e.to_string().contains("trees is not a component (grass, temperatureLimit, blending)"), "{e}");
    let e = c.switch_component(&fs, "grass", false, Some(&["a".into()])).unwrap_err();
    assert!(e.to_string().contains("patches are the grass's"), "{e}");
    // A case without a description: the files only.
    let (mut fs, _) = opened(&spec(OfVersion::V12, Components::default()));
    fs.remove(crate::description::PATH);
    let mut c = Case::open(&fs).unwrap();
    let s = c.switch_component(&fs, "temperatureLimit", true, None).unwrap();
    assert!(!s.described);
    assert_eq!(s.changed, ["system/air/fvConstraints"]);
}

#[test]
fn the_blending_layer_needs_the_four_sides_in_the_air_s_mesh() {
    let boundary = |names: &[&str]| {
        let mut b = format!("FoamFile {{ class polyBoundaryMesh; object boundary; }}\n{}\n(\n", names.len());
        for (i, n) in names.iter().enumerate() {
            b.push_str(&format!("    {n} {{ type patch; nFaces 1; startFace {i}; }}\n"));
        }
        b + ")\n"
    };
    let (mut fs, _) = opened(&spec(OfVersion::V12, Components::default()));
    fs.insert("constant/air/polyMesh/boundary", boundary(&["inlet", "outlet", "north", "south"]));
    let mut c = Case::open(&fs).unwrap();
    let e = c.switch_component(&fs, "blending", true, None).unwrap_err();
    assert!(e.to_string().contains("region air has no west, east"), "{e}");
    assert!(c.changed().is_empty());
    fs.insert("constant/air/polyMesh/boundary", boundary(&["west", "east", "north", "south", "top"]));
    let mut c = Case::open(&fs).unwrap();
    let s = c.switch_component(&fs, "blending", true, None).unwrap();
    assert!(!s.notes.iter().any(|n| n.contains("no mesh yet")), "{s:?}");
    // Without a mesh: a note.
    let (fs, mut c) = opened(&spec(OfVersion::V12, Components::default()));
    let s = c.switch_component(&fs, "blending", true, None).unwrap();
    assert!(s.notes.iter().any(|n| n.contains("region air: no mesh yet")), "{s:?}");
}

/// `spec`'s case with `edit` done to its files before it is opened.
fn with_files(spec: &NewCase, edit: impl FnOnce(&mut MemFs)) -> (MemFs, Case) {
    let (mut fs, _) = opened(spec);
    edit(&mut fs);
    let c = Case::open(&fs).unwrap();
    (fs, c)
}

fn text(fs: &MemFs, rel: &str) -> String {
    use umcf_core::CaseFs;
    String::from_utf8(fs.read(rel, 1 << 20).unwrap().unwrap()).unwrap()
}

#[test]
fn the_limit_goes_into_the_file_the_solver_reads_it_from() {
    // OpenFOAM-12 with its constraints in system/fvOptions: there, no fvConstraints that would
    // hide them.
    let s12 = spec(OfVersion::V12, Components::default());
    let (_, mut c) = with_files(&s12, |fs| {
        let t = text(fs, "system/air/fvConstraints");
        fs.remove("system/air/fvConstraints");
        fs.insert("system/air/fvOptions", t.replace("fvConstraints", "fvOptions"));
    });
    let mem = MemFs::new();
    let s = c.switch_component(&mem, "temperatureLimit", true, None).unwrap();
    assert!(s.changed.contains(&"system/air/fvOptions".to_string()), "{s:?}");
    let o = c.dict("system/air/fvOptions").unwrap().to_string();
    assert!(o.contains("limitTemperature") && o.contains("limitp"), "{o}");
    assert!(c.dict("system/air/fvConstraints").is_none());
    // OpenFOAM-12 never reads constant/fvConstraints: a new system/fvConstraints.
    let (_, mut c) = with_files(&s12, |fs| {
        let t = text(fs, "system/air/fvConstraints");
        fs.remove("system/air/fvConstraints");
        fs.insert("constant/air/fvConstraints", t + "limitT { type limitTemperature; select all; min 1; max 2; }\n");
    });
    let s = c.switch_component(&mem, "temperatureLimit", true, None).unwrap();
    assert!(s.changed.contains(&"system/air/fvConstraints".to_string()), "{s:?}");
    // Off again: the one in constant/fvConstraints is left, with a note.
    let s = c.switch_component(&mem, "temperatureLimit", false, None).unwrap();
    assert!(
        s.notes
            .iter()
            .any(|n| n
                .starts_with("constant/air/fvConstraints: a limitTemperature entry in a file the solver doesn't read")),
        "{s:?}"
    );
    assert!(!s.notes.iter().any(|n| n.starts_with("system/air/fvConstraints: a limitTemperature")), "{s:?}");
    assert!(!s.notes.iter().any(|n| n.contains("have no limitTemperature entry")), "{s:?}");
    // A comment naming it is no entry.
    let (fs, mut c) = with_files(&s12, |fs| {
        let t = text(fs, "system/air/fvConstraints");
        fs.insert("system/air/fvOptions", t.replace("fvConstraints", "fvOptions") + "// limitTemperature moved\n");
    });
    let s = c.switch_component(&fs, "temperatureLimit", false, None).unwrap();
    assert!(!s.notes.iter().any(|n| n.contains("doesn't read")), "{s:?}");
    // OpenFOAM-8 reads constant/fvOptions before system/'s.
    let s8 = spec(OfVersion::V8, Components::default());
    let (_, mut c) = with_files(&s8, |fs| {
        let empty = "FoamFile { class dictionary; object fvOptions; }\n";
        fs.insert("constant/air/fvOptions", empty);
        fs.insert("system/air/fvOptions", empty);
    });
    let s = c.switch_component(&mem, "temperatureLimit", true, None).unwrap();
    assert!(s.changed.contains(&"constant/air/fvOptions".to_string()), "{s:?}");
    assert!(!c.dict("system/air/fvOptions").unwrap().to_string().contains("limitTemperature"));
}

#[test]
fn a_refusal_half_way_changes_nothing() {
    // Two fluids; the second's fvConstraints is there but doesn't load.
    let mut s = spec(OfVersion::V12, Components::default());
    s.regions.push(NewRegion { name: "wind".into(), kind: RegionKind::Fluid, materials: vec![] });
    let (fs, mut c) = with_files(&s, |fs| fs.insert("system/wind/fvConstraints", "limitT { "));
    let before = c.dict("system/air/fvConstraints").unwrap().to_string();
    let e = c.switch_component(&fs, "temperatureLimit", true, None).unwrap_err();
    assert!(e.to_string().contains("system/wind/fvConstraints doesn't load"), "{e}");
    assert_eq!(c.dict("system/air/fvConstraints").unwrap().to_string(), before);
    assert!(c.changed().is_empty());
}

#[test]
fn bounds_not_kept_are_noted_and_grass_goes_off_in_every_fluid() {
    // Two fluids whose bounds differ: neither kept, both noted.
    let mut s = spec(OfVersion::V8, one("temperatureLimit"));
    s.regions.push(NewRegion { name: "wind".into(), kind: RegionKind::Fluid, materials: vec![] });
    let (fs, mut c) = with_files(&s, |fs| {
        let t = text(fs, "system/wind/fvOptions").replace("273.15", "280");
        fs.insert("system/wind/fvOptions", t);
    });
    let sw = c.switch_component(&fs, "temperatureLimit", false, None).unwrap();
    assert!(
        sw.notes.iter().any(|n| n.contains("system/air/fvOptions: limitT had the bounds ( 273.15 373.15 )")),
        "{sw:?}"
    );
    assert!(
        sw.notes.iter().any(|n| n.contains("system/wind/fvOptions: limitT had the bounds ( 280 373.15 )")),
        "{sw:?}"
    );
    // Grass in a fluid other than air: switched off too.
    let (fs, mut c) = with_files(&s, |fs| {
        fs.insert(
            "constant/wind/grassProperties",
            "FoamFile { class dictionary; object grassProperties; }\ngrassModel simpleGrass;\n",
        );
    });
    assert!(case_components(&c).grass);
    c.switch_component(&fs, "grass", false, None).unwrap();
    assert_eq!(c.dict("constant/wind/grassProperties").unwrap().value(&["grassModel"]).as_deref(), Some("none"));
    assert!(!case_components(&c).grass);
}

#[test]
fn a_bare_grass_file_gets_what_the_configuration_has() {
    let s = spec(OfVersion::V12, Components::default());
    let (fs, mut c) = with_files(&s, |fs| {
        fs.insert(
            "constant/air/grassProperties",
            "FoamFile { class dictionary; object grassProperties; }\ngrassModel none;\ngrass off;\n",
        );
    });
    assert!(!case_components(&c).grass);
    let sw = c.switch_component(&fs, "grass", true, Some(&["ground".into()])).unwrap();
    assert!(sw.notes.iter().any(|n| n.contains("from the configuration")), "{sw:?}");
    let g = c.dict("constant/air/grassProperties").unwrap();
    assert_eq!(g.value(&["grassModel"]).as_deref(), Some("simpleGrass"));
    assert_eq!(g.value(&["simpleGrassCoeffs", "betaLW"]).as_deref(), Some("1000"), "{g}");
    assert_eq!(g.value(&["simpleGrassCoeffs", "grassPatches"]).as_deref(), Some("(ground)"), "{g}");
    assert!(!g.to_string().contains("air_to_street"), "{g}");
    assert!(case_components(&c).grass);
    // As asked already: nothing changes, and a note says so.
    let sw = c.switch_component(&fs, "grass", true, None).unwrap();
    assert!(sw.changed.is_empty() && sw.notes.iter().any(|n| n.contains("on already")), "{sw:?}");
    c.switch_component(&fs, "grass", false, None).unwrap();
    let sw = c.switch_component(&fs, "grass", false, None).unwrap();
    assert!(sw.changed.is_empty() && sw.notes.iter().any(|n| n.contains("off already")), "{sw:?}");
}
