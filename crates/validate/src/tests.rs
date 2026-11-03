use umcf_core::MemFs;
use umcf_model::Case;

use super::*;

fn header(class: &str) -> String {
    format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n")
}

/// One patch of a `boundary` file.
fn patch(name: &str, kind: &str, faces: usize, start: usize, extra: &str) -> String {
    format!(
        "    {name}\n    {{\n        type {kind};\n        nFaces {faces};\n        startFace {start};\n{extra}    }}\n"
    )
}

fn boundary(patches: &[String]) -> String {
    format!("{}{}\n(\n{})\n", header("polyBoundaryMesh"), patches.len(), patches.concat())
}

fn field(class: &str, bcs: &str) -> String {
    format!("{}dimensions [0 0 0 1 0 0 0];\ninternalField uniform 300;\nboundaryField\n{{\n{bcs}}}\n", header(class))
}

const CONTROL: &str = "application urbanMicroclimateFoam;\nstartFrom startTime;\nstartTime 0;\nstopAt endTime;\nendTime 3600;\ndeltaT 60;\nwriteInterval 600;\nregions\n{\n    fluid (air);\n    solid (street);\n    vegetation (vegetation);\n}\n";

/// A small OF12 case that checks clean: air couples to street both ways, vegetation samples
/// the air's cells.
fn case() -> MemFs {
    let air = boundary(&[
        patch("inlet", "patch", 2, 10, ""),
        patch("ground", "wall", 2, 12, "        inGroups List<word> 1(wall);\n"),
        patch(
            "air_to_street",
            "mappedWall",
            2,
            14,
            "        neighbourRegion street;\n        neighbourPatch street_to_air;\n",
        ),
        patch("frontAndBack", "empty", 4, 16, ""),
        patch("defaultFaces", "empty", 0, 20, ""),
    ]);
    let street = boundary(&[patch(
        "street_to_air",
        "mappedWall",
        2,
        5,
        "        neighbourRegion air;\n        neighbourPatch air_to_street;\n",
    )]);
    let vegetation = boundary(&[patch("air_to_vegetation", "mappedInternal", 3, 2, "        neighbourRegion air;\n")]);
    let t_air = field(
        "volScalarField",
        "    inlet { type fixedValue; value uniform 300; }\n    wall { type zeroGradient; }\n    air_to_street { type compressible::CFDHAMfluidTemperatureCoupledMixed; value uniform 300; }\n    frontAndBack { type empty; }\n",
    );
    MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary")))
        .with("constant/air/polyMesh/boundary", air)
        .with("constant/street/polyMesh/boundary", street)
        .with("constant/vegetation/polyMesh/boundary", vegetation)
        .with("constant/air/fvModels", format!("{}\n", header("dictionary")))
        .with(
            "constant/air/vegetationProperties",
            format!("{}vegetationModel simplifiedVegetation;\n", header("dictionary")),
        )
        .with(
            "constant/street/transportProperties",
            format!(
                "{}buildingMaterials\n(\n    {{ name brick; buildingMaterialModel Hamstad5Brick; }}\n);\n",
                header("dictionary")
            ),
        )
        .with("0/air/T", t_air)
        .with("0/air/Tambient", "(\n(0 290)\n(3600 295)\n)\n")
        .with(
            "0/street/Ts",
            field("volScalarField", "    street_to_air { type compressible::CFDHAMsolidTemperatureCoupledMixed; }\n"),
        )
        .with("0/vegetation/Tl", field("volScalarField", "    air_to_vegetation { type zeroGradient; }\n"))
}

fn codes(fs: &MemFs) -> Vec<(Level, &'static str)> {
    let c = Case::open(fs).unwrap();
    check(&c).into_iter().map(|d| (d.level, d.code)).collect()
}

fn errors(fs: &MemFs) -> Vec<Diagnostic> {
    let c = Case::open(fs).unwrap();
    check(&c).into_iter().filter(|d| d.level == Level::Error).collect()
}

fn edit(fs: &mut MemFs, rel: &str, from: &str, to: &str) {
    let text = String::from_utf8(fs.read(rel, 1 << 20).unwrap().unwrap()).unwrap();
    assert!(text.contains(from), "{rel} has no `{from}`");
    fs.insert(rel, text.replacen(from, to, 1));
}

use umcf_core::CaseFs;

#[test]
fn a_clean_case_has_no_diagnostics() {
    // Covered by name, by group (`wall`), `empty` patches with an entry and without one
    // (`defaultFaces`), a data table in the time directory (`Tambient`), one-way cell sampling.
    assert_eq!(codes(&case()), []);
}

#[test]
fn controls() {
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "endTime 3600;", "endTime 0;");
    edit(&mut fs, "system/controlDict", "deltaT 60;", "deltaT -1;");
    edit(&mut fs, "system/controlDict", "application urbanMicroclimateFoam;", "");
    let c = codes(&fs);
    assert!(c.contains(&(Level::Error, "control.end-time")), "{c:?}");
    assert!(c.contains(&(Level::Error, "control.step")), "{c:?}");
    assert!(c.contains(&(Level::Warning, "control.application")), "{c:?}");
    // `$macros` and other stop conditions are not judged.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "endTime 3600;", "endTime $end;");
    edit(&mut fs, "system/controlDict", "deltaT 60;", "deltaT $dt;");
    assert_eq!(codes(&fs), []);
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "endTime 3600;", "endTime 0;");
    edit(&mut fs, "system/controlDict", "stopAt endTime;", "stopAt writeNow;");
    assert_eq!(codes(&fs), []);
}

#[test]
fn a_patch_without_an_entry_is_an_error() {
    let mut fs = case();
    edit(&mut fs, "0/air/T", "    inlet { type fixedValue; value uniform 300; }\n", "");
    let e = errors(&fs);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!((e[0].code, e[0].patch.as_deref(), e[0].field.as_deref()), ("bc.missing", Some("inlet"), Some("T")));
    assert_eq!(e[0].path, "0/air/T");
    // With an `#include`, the entry may come from elsewhere: not an error.
    edit(&mut fs, "0/air/T", "boundaryField\n{\n", "boundaryField\n{\n    #include \"inletBCs\"\n");
    assert_eq!(codes(&fs), [(Level::Info, "bc.missing")]);
}

#[test]
fn entries_match_like_openfoam() {
    // A pattern covers patches, the last matching pattern wins, and the patch name beats both.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"in.*\" { type fixedValue; value uniform 300; }\n    \"inl.*\" { type zeroGradient; }\n",
    );
    assert_eq!(codes(&fs), []);
    // Review finding: OpenFOAM puts wall patches in group `wall` itself, also without
    // `inGroups` (wallPolyPatch.C), and mapped patches in `mapped` (OF12) / `mappedPatch` (OF8).
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "        inGroups List<word> 1(wall);\n",
        "        inGroups List<word> 0();\n",
    );
    assert_eq!(codes(&fs), []);
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    air_to_street { type compressible::CFDHAMfluidTemperatureCoupledMixed; value uniform 300; }\n",
        "",
    );
    edit(
        &mut fs,
        "0/air/T",
        "    wall { type zeroGradient; }\n",
        "    mapped { type compressible::CFDHAMfluidTemperatureCoupledMixed; }\n    wall { type zeroGradient; }\n",
    );
    // `air_to_street` is a mappedWall: in `wall` and `mapped`; the later entry, `wall`, wins.
    assert_eq!(codes(&fs), []);
    // An `empty` patch is matched by its implicit `empty` group too.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "    frontAndBack { type empty; }\n", "    empty { type empty; }\n");
    assert_eq!(codes(&fs), []);
}

#[test]
fn constraint_patches_need_their_type() {
    let mut fs = case();
    edit(&mut fs, "0/air/T", "frontAndBack { type empty; }", "frontAndBack { type zeroGradient; }");
    let e = errors(&fs);
    assert_eq!(e.iter().map(|d| d.code).collect::<Vec<_>>(), ["bc.constraint"], "{e:?}");
}

#[test]
fn entries_for_no_patch_and_bad_patterns() {
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "boundaryField\n{\n",
        "boundaryField\n{\n    outlet { type zeroGradient; }\n    \"(broken\" { type zeroGradient; }\n",
    );
    let c = codes(&fs);
    assert!(c.contains(&(Level::Warning, "bc.unused")), "{c:?}");
    assert!(c.contains(&(Level::Warning, "bc.pattern")), "{c:?}");
    // The patches an unreadable pattern may cover are not reported as missing errors.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"(?<=x)inlet\" { type fixedValue; }\n",
    );
    let c = codes(&fs);
    assert!(c.contains(&(Level::Info, "bc.missing")) && !c.iter().any(|(l, _)| *l == Level::Error), "{c:?}");
}

#[test]
fn unknown_urban_microclimate_boundary_conditions() {
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "compressible::CFDHAMfluidTemperatureCoupledMixed",
        "compressible::CFDHAMfluidTemperatureCoupledMix",
    );
    let e = errors(&fs);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].code, "bc.unknown");
    assert!(
        e[0].message.contains("did you mean `compressible::CFDHAMfluidTemperatureCoupledMixed`"),
        "{}",
        e[0].message
    );
    // Standard OpenFOAM types are not in the catalog and are not judged.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "inlet { type fixedValue;", "inlet { type someCustomBC;");
    assert_eq!(codes(&fs), []);
}

#[test]
fn region_coupling() {
    // A partner region the case doesn't have.
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "neighbourRegion street;", "neighbourRegion streets;");
    assert!(errors(&fs).iter().any(|d| d.code == "coupling.region" && d.patch.as_deref() == Some("air_to_street")));
    // A partner patch that doesn't exist.
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "neighbourPatch street_to_air;", "neighbourPatch street_air;");
    assert!(errors(&fs).iter().any(|d| d.code == "coupling.patch"));
    // A partner that points back into the region, at another patch.
    let mut fs = case();
    edit(&mut fs, "constant/street/polyMesh/boundary", "neighbourPatch air_to_street;", "neighbourPatch ground;");
    let c = codes(&fs);
    assert!(c.contains(&(Level::Warning, "coupling.not-mutual")), "{c:?}");
    // OF8 keywords work the same.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "neighbourRegion street;\n        neighbourPatch street_to_air;",
        "sampleRegion street;\n        samplePatch street_to_air;",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn a_meshing_step_names_regions_that_come_later() {
    let fs =
        MemFs::new().with("system/controlDict", format!("{}application extrudeMesh;\n", header("dictionary"))).with(
            "constant/polyMesh/boundary",
            boundary(&[patch(
                "roofs_to_air",
                "mappedWall",
                4,
                0,
                "        sampleRegion air;\n        samplePatch roofs;\n",
            )]),
        );
    assert_eq!(codes(&fs), []);
}

#[test]
fn regions_without_mesh_or_fields() {
    let mut fs = case();
    fs.insert("constant/vegetation/polyMesh/boundary", "");
    let mut text = MemFs::new();
    for rel in [
        "system/controlDict",
        "constant/air/polyMesh/boundary",
        "constant/street/polyMesh/boundary",
        "0/air/T",
        "0/street/Ts",
    ] {
        text.insert(rel, fs.read(rel, 1 << 20).unwrap().unwrap());
    }
    let c = codes(&text);
    assert!(c.contains(&(Level::Warning, "region.no-mesh")), "{c:?}");
    // A region with a mesh but no fields at the start time.
    let mut fs = case();
    let mut no_fields = MemFs::new();
    for rel in [
        "system/controlDict",
        "constant/air/polyMesh/boundary",
        "constant/street/polyMesh/boundary",
        "constant/vegetation/polyMesh/boundary",
        "0/air/T",
        "0/street/Ts",
        "0/vegetation/Tambient",
    ] {
        if let Some(b) = fs.read(rel, 1 << 20).unwrap() {
            no_fields.insert(rel, b);
        }
    }
    no_fields.insert("0/vegetation/table", "(\n(0 1)\n)\n");
    fs = no_fields;
    let e = errors(&fs);
    assert!(e.iter().any(|d| d.code == "region.no-fields" && d.region.as_deref() == Some("vegetation")), "{e:?}");
}

#[test]
fn model_types_against_the_solver() {
    // A type the solver doesn't have, with a suggestion and the choices.
    let mut fs = case();
    edit(&mut fs, "constant/air/vegetationProperties", "simplifiedVegetation", "simplifiedVegetaton");
    let e = errors(&fs);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!((e[0].code, e[0].region.as_deref()), ("model.unknown", Some("air")));
    assert!(e[0].message.contains("did you mean `simplifiedVegetation`"), "{}", e[0].message);
    assert!(e[0].message.contains("none, simplifiedVegetation"), "{}", e[0].message);
    // Inside a list of dictionaries (building materials).
    let mut fs = case();
    edit(&mut fs, "constant/street/transportProperties", "Hamstad5Brick", "Brick");
    assert!(errors(&fs).iter().any(|d| d.code == "model.unknown" && d.path == "constant/street/transportProperties"));
    // Files with a `.` are not read by the solver: not checked.
    let mut fs = case();
    fs.insert("constant/air/vegetationProperties.orig", format!("{}vegetationModel oldModel;\n", header("dictionary")));
    assert_eq!(codes(&fs), []);
}

#[test]
fn a_type_from_the_other_version() {
    // OSB exists only in the OpenFOAM-12 solver; this case is OpenFOAM-8 (regionProperties).
    let mut fs = case();
    edit(
        &mut fs,
        "system/controlDict",
        "regions\n{\n    fluid (air);\n    solid (street);\n    vegetation (vegetation);\n}\n",
        "",
    );
    fs.insert(
        "constant/regionProperties",
        format!(
            "{}regions\n(\n    fluid (air)\n    solid (street)\n    vegetation (vegetation)\n);\n",
            header("dictionary")
        ),
    );
    fs.insert("constant/air/fvModels", "");
    let mut of8 = MemFs::new();
    for rel in [
        "system/controlDict",
        "constant/regionProperties",
        "constant/air/polyMesh/boundary",
        "constant/street/polyMesh/boundary",
        "constant/vegetation/polyMesh/boundary",
        "constant/air/vegetationProperties",
        "constant/street/transportProperties",
        "0/air/T",
        "0/street/Ts",
        "0/vegetation/Tl",
    ] {
        of8.insert(rel, fs.read(rel, 1 << 20).unwrap().unwrap());
    }
    of8.insert("system/air/fvOptions", format!("{}\n", header("dictionary")));
    let c = Case::open(&of8).unwrap();
    assert_eq!(c.layout.version, Some(OfVersion::V8));
    assert!(check(&c).iter().all(|d| d.level != Level::Error), "{:?}", check(&c));
    edit(&mut of8, "constant/street/transportProperties", "Hamstad5Brick", "OSB");
    let e = errors(&of8);
    assert_eq!(e.iter().map(|d| d.code).collect::<Vec<_>>(), ["model.version"], "{e:?}");
    assert!(e[0].message.contains("OpenFOAM-12"), "{}", e[0].message);
}

#[test]
fn hostile_input_does_not_panic() {
    // Deep nesting, huge patterns, odd names: diagnostics, never a panic.
    let mut fs = case();
    let deep = format!("{}vegetationModel {};\n", "{ ".repeat(200), "}".repeat(200));
    fs.insert("constant/air/vegetationProperties", format!("{}a {deep}", header("dictionary")));
    let long = "a".repeat(100_000);
    edit(&mut fs, "0/air/T", "boundaryField\n{\n", &format!("boundaryField\n{{\n    \"{long}(\" {{ type x; }}\n"));
    let c = Case::open(&fs).unwrap();
    let _ = check(&c);
    assert_eq!(distance("", ""), 0);
    assert_eq!(distance("kitten", "sitting"), 3);
}

#[test]
fn a_boundary_field_from_elsewhere() {
    // Review finding: a field whose whole boundaryField comes from an `#include` or `$macro`
    // got one error per patch.
    let mut fs = case();
    fs.insert(
        "0/air/T",
        format!("{}internalField uniform 300;\n#include \"boundaryField\"\n", header("volScalarField")),
    );
    assert!(codes(&fs).iter().all(|(l, _)| *l != Level::Error), "{:?}", codes(&fs));
    let mut fs = case();
    fs.insert("0/air/T", format!("{}internalField uniform 300;\nboundaryField $common;\n", header("volScalarField")));
    assert!(codes(&fs).iter().all(|(l, _)| *l != Level::Error), "{:?}", codes(&fs));
    // No boundaryField at all, and nothing that could bring one: one error, not one per patch.
    let mut fs = case();
    fs.insert("0/air/T", format!("{}internalField uniform 300;\n", header("volScalarField")));
    assert_eq!(codes(&fs), [(Level::Error, "bc.no-boundary-field")]);
}

#[test]
fn copies_and_backups_are_not_checked() {
    // Review finding: `T~` and `T.bak` (headers still saying `T`) were checked as fields, and
    // files the solver doesn't read were checked for model types.
    let mut fs = case();
    let named_t = |text: String| text.replacen("class volScalarField;", "class volScalarField;\n    object T;", 1);
    let t = String::from_utf8(fs.read("0/air/T", 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert("0/air/T", named_t(t));
    fs.insert("0/air/T~", named_t(field("volScalarField", "")));
    fs.insert("0/air/T.bak", named_t(field("volScalarField", "")));
    fs.insert("constant/air/vegetationPropertiesOld", format!("{}vegetationModel oldModel;\n", header("dictionary")));
    // A selector below the top level is not what the solver reads either.
    edit(
        &mut fs,
        "constant/air/vegetationProperties",
        "vegetationModel simplifiedVegetation;",
        "vegetationModel simplifiedVegetation;\nalternatives { vegetationModel other; }",
    );
    let c: Vec<_> =
        Case::open(&fs).map(|c| check(&c)).unwrap().into_iter().filter(|d| d.level == Level::Error).collect();
    assert!(c.is_empty(), "{c:?}");
}

#[test]
fn patch_type_lifts_the_constraint() {
    // Review finding: `type fixedJump; patchType cyclic;` is legal (fvPatchFieldNew.C).
    let mut fs = case();
    edit(&mut fs, "0/air/T", "frontAndBack { type empty; }", "frontAndBack { type fixedJump; patchType empty; }");
    assert_eq!(codes(&fs), []);
}

#[test]
fn included_constraint_types_beat_patterns() {
    // Review finding: with `#includeEtc "caseDicts/setConstraintTypes"`, the included group
    // entries win over a pattern written here, so a pattern's type on a constraint patch is not
    // an error.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    frontAndBack { type empty; }\n",
        "    #includeEtc \"caseDicts/setConstraintTypes\"\n    \".*\" { type zeroGradient; }\n",
    );
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "    frontAndBack\n    {\n        type empty;",
        "    frontAndBack\n    {\n        type symmetryPlane;",
    );
    assert!(codes(&fs).iter().all(|(l, _)| *l != Level::Error), "{:?}", codes(&fs));
}

#[test]
fn start_from_defaults_to_latest_time() {
    // Review finding: without startFrom, OpenFOAM resumes from the latest time; a stale
    // startTime is not an error.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "startFrom startTime;\nstartTime 0;", "startTime 7200;");
    assert_eq!(codes(&fs), []);
    // A case that has run to its end time, resumed from its latest time.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "startFrom startTime;", "startFrom latestTime;");
    for rel in ["0/air/T", "0/street/Ts", "0/vegetation/Tl"] {
        let b = fs.read(rel, 1 << 20).unwrap().unwrap();
        fs.insert(&rel.replacen("0/", "3600/", 1), b);
    }
    assert_eq!(codes(&fs), [(Level::Info, "control.finished")]);
    // Values OpenFOAM refuses.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "startFrom startTime;", "startFrom lastTime;");
    edit(&mut fs, "system/controlDict", "writeInterval 600;", "writeControl timeStep;\nwriteInterval 0.5;");
    let c = codes(&fs);
    assert!(c.contains(&(Level::Error, "control.value")) && c.contains(&(Level::Error, "control.step")), "{c:?}");
}

#[test]
fn nearest_face_and_no_patch_sampling() {
    // Review finding: OF8 `sampleMode nearestFace` with `samplePatch none` uses no patch.
    let mut fs = case();
    edit(&mut fs, "constant/vegetation/polyMesh/boundary", "type mappedInternal;", "type mappedPatch;");
    edit(
        &mut fs,
        "constant/vegetation/polyMesh/boundary",
        "        neighbourRegion air;\n",
        "        sampleMode nearestFace;\n        sampleRegion air;\n        samplePatch none;\n",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn patterns_are_read_like_posix() {
    // Review finding: `\d` is a literal `d` in OpenFOAM's POSIX patterns; an unbalanced pattern
    // was accepted inside the `^(?:…)$` wrapper; huge patterns were slow.
    for pattern in [r"wall\d", "in)|(let", "(a{1,1000}){1,50}"] {
        let mut fs = case();
        edit(
            &mut fs,
            "0/air/T",
            "boundaryField\n{\n",
            &format!("boundaryField\n{{\n    \"{pattern}\" {{ type zeroGradient; }}\n"),
        );
        let c = codes(&fs);
        assert!(c.contains(&(Level::Warning, "bc.pattern")), "{pattern}: {c:?}");
    }
}

#[test]
fn entries_that_cover_patches_are_not_unused() {
    // Review finding: a group entry whose patches all have their own entries, a `processor`
    // entry kept for the decomposed case, and a key written twice were "unused".
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "boundaryField\n{\n",
        "boundaryField\n{\n    ground { type fixedValue; value uniform 290; }\n    processor { type processor; }\n    inlet { type fixedValue; value uniform 1; }\n",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn one_report_per_entry() {
    // Review finding: a pattern covering many patches repeated its type error per patch.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n    wall { type zeroGradient; }\n",
        "    \"(inlet|ground)\" { type compressible::CFDHAMbogus; }\n",
    );
    let e = errors(&fs);
    assert_eq!(e.iter().filter(|d| d.code == "bc.unknown").count(), 1, "{e:?}");
}

#[test]
fn libraries_make_unknown_models_a_warning() {
    let mut fs = case();
    edit(
        &mut fs,
        "system/controlDict",
        "application urbanMicroclimateFoam;",
        "application urbanMicroclimateFoam;\nlibs (\"libmyVegetation.so\");",
    );
    edit(&mut fs, "constant/air/vegetationProperties", "simplifiedVegetation", "myVegetation");
    assert_eq!(codes(&fs), [(Level::Warning, "model.unknown")]);
}

#[test]
fn second_review_false_errors() {
    // A region whose fields are all too large to read here still has fields.
    let fs = case();
    let c = Case::open_with(&fs, umcf_model::OpenOptions { field_bytes: 10, ..Default::default() }).unwrap();
    assert!(check(&c).iter().all(|d| d.code != "region.no-fields"), "{:?}", check(&c));

    // Quoted words read like unquoted ones (OpenFOAM's word reader takes strings).
    let mut fs = case();
    edit(&mut fs, "0/air/T", "frontAndBack { type empty; }", "frontAndBack { type \"empty\"; }");
    edit(&mut fs, "system/controlDict", "startFrom startTime;", "startFrom \"startTime\";");
    assert_eq!(codes(&fs), []);

    // A `$macro` type is not judged.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "frontAndBack { type empty; }", "frontAndBack { type $emptyType; }");
    assert_eq!(codes(&fs), []);

    // `mappedWallVariableThickness` (OF8) is a wall.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "    ground\n    {\n        type wall;",
        "    ground\n    {\n        type mappedWallVariableThickness;",
    );
    assert!(errors(&fs).iter().all(|d| d.code != "bc.missing"), "{:?}", errors(&fs));

    // A patch of type `mappedPatch` is in group `mappedPatch`, whatever version was detected.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/vegetation/polyMesh/boundary",
        "type mappedInternal;",
        "type mappedPatch;\n        sampleMode nearestCell;",
    );
    edit(&mut fs, "0/vegetation/Tl", "air_to_vegetation { type zeroGradient; }", "mappedPatch { type zeroGradient; }");
    assert_eq!(codes(&fs), []);

    // A header `object` that differs: OpenFOAM reads the file by its name anyway.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "class volScalarField;", "class volScalarField;\n    object Ts;");
    assert_eq!(codes(&fs), [(Level::Info, "field.object")]);

    // `\w` and `\s` are GNU extensions glibc's matcher knows: fine. Only `\d` is not.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"in\\w+\" { type fixedValue; value uniform 300; }\n",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn duplicates_merge_like_openfoam() {
    // A key written twice keeps its first place: here the `symmetryPlane` group entry, written
    // after the first `g`, wins.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "    frontAndBack\n    {\n        type empty;",
        "    frontAndBack\n    {\n        type symmetryPlane;\n        inGroups List<word> 1(g);",
    );
    edit(
        &mut fs,
        "0/air/T",
        "    frontAndBack { type empty; }\n",
        "    g { type zeroGradient; }\n    symmetryPlane { type symmetryPlane; }\n    g { type zeroGradient; }\n",
    );
    assert!(errors(&fs).is_empty(), "{:?}", errors(&fs));
    // A selector written twice: the last counts.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/vegetationProperties",
        "vegetationModel simplifiedVegetation;",
        "vegetationModel foo;\nvegetationModel simplifiedVegetation;",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn couple_groups_and_first_time() {
    // OF12 `coupleGroup` finds the partner by group; a stale neighbourRegion is ignored.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "neighbourRegion street;",
        "coupleGroup streetAir;\n        neighbourRegion old;",
    );
    assert_eq!(codes(&fs), []);
    // startFrom firstTime with endTime at the first time: the run stops at once.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "startFrom startTime;", "startFrom firstTime;");
    edit(&mut fs, "system/controlDict", "endTime 3600;", "endTime 0;");
    assert!(errors(&fs).iter().any(|d| d.code == "control.end-time"));
}

#[test]
fn many_patches_stay_fast() {
    // Review finding: matching was quadratic in the number of patches.
    let n = 4000;
    let patches: Vec<String> = (0..n).map(|i| patch(&format!("b{i}"), "wall", 1, 10 + i, "")).collect();
    let entries: String = (0..n).map(|i| format!("    b{i} {{ type zeroGradient; }}\n")).collect();
    let mut fs = case();
    fs.insert("constant/air/polyMesh/boundary", boundary(&patches));
    // Without the air side of the coupling, the street side couples to nothing.
    fs.insert("constant/street/polyMesh/boundary", boundary(&[patch("street_to_air", "wall", 2, 5, "")]));
    for f in ["T", "U", "p", "k", "epsilon"] {
        fs.insert(&format!("0/air/{f}"), field("volScalarField", &entries));
    }
    let c = Case::open(&fs).unwrap();
    let d = check(&c);
    assert!(d.iter().all(|d| d.level != Level::Error), "{:?}", d.iter().take(3).collect::<Vec<_>>());
}

#[test]
fn third_review() {
    // Duplicates merge entry by entry: `patchType` from the first copy survives.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "frontAndBack { type empty; }",
        "frontAndBack { type fixedJump; patchType empty; }\n    frontAndBack { type fixedJump; }",
    );
    assert_eq!(codes(&fs), []);
    // A pattern written twice keeps its first place: here `"inlet"`… is a name; use two patterns.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"inl.*\" { type compressible::CFDHAMbogus; }\n    \"in.*\" { type fixedValue; }\n    \"inl.*\" { type compressible::CFDHAMbogus; }\n    \"zz.*\" { type zeroGradient; }\n    \"zz.*\" { type zeroGradient; }\n",
    );
    let c = codes(&fs);
    // `"in.*"` is placed after the merged `"inl.*"`, so it wins: no bc.unknown. `"zz.*"` once.
    assert!(!c.iter().any(|(_, code)| *code == "bc.unknown"), "{c:?}");
    assert_eq!(c.iter().filter(|(_, code)| *code == "bc.unused").count(), 1, "{c:?}");
    // A copy that says another field's name: its problems are Info.
    let mut fs = case();
    fs.insert(
        "0/air/T_old",
        field("volScalarField", "").replacen("class volScalarField;", "class volScalarField;\n    object T;", 1),
    );
    assert!(codes(&fs).iter().all(|(l, _)| *l != Level::Error), "{:?}", codes(&fs));
    // A leftover constraint entry after a rename is still unused.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "boundaryField\n{\n", "boundaryField\n{\n    oldSides { type empty; }\n");
    assert_eq!(codes(&fs), [(Level::Warning, "bc.unused")]);
    // An escaped backslash before `d` is not the `\d` class.
    let mut fs = case();
    edit(&mut fs, "0/air/T", "boundaryField\n{\n", "boundaryField\n{\n    \"inlet|x\\\\\\\\d\" { type fixedValue; }\n");
    assert!(!codes(&fs).contains(&(Level::Warning, "bc.pattern")), "{:?}", codes(&fs));
    // OF8: a coupleGroup patch with samplePatch but no sampleRegion is found by group.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "neighbourRegion street;\n        neighbourPatch street_to_air;",
        "coupleGroup g;\n        samplePatch street_to_air;",
    );
    assert_eq!(codes(&fs), []);
}

#[test]
fn a_quoted_key_with_the_patch_name_beats_patterns() {
    // `"inlet"` then `"in.*"`: OpenFOAM finds `"inlet"` by its text first.
    let mut fs = case();
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"inlet\" { type fixedValue; }\n    \"in.*\" { type compressible::CFDHAMbogus; }\n",
    );
    assert!(!codes(&fs).iter().any(|(_, c)| *c == "bc.unknown"), "{:?}", codes(&fs));
    // A name that isn't a valid match for itself as a regular expression.
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "    inlet\n", "    wall(1)\n");
    edit(
        &mut fs,
        "0/air/T",
        "    inlet { type fixedValue; value uniform 300; }\n",
        "    \"wall(1)\" { type fixedValue; }\n",
    );
    assert!(errors(&fs).is_empty(), "{:?}", errors(&fs));
}
