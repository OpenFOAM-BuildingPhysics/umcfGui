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

/// The coefficients simplifiedVegetation reads when it is built.
const SIMPLIFIED_VEGETATION: &str = "simplifiedVegetationCoeffs\n{\n    a1 a1 [1 0 -3 0 0 0 0] 169;\n    a2 a2 [1 0 -3 0 0 0 0] 18;\n    a3 a3 [1 -1 -2 0 0 0 0] 0.005;\n    D0 D0 [1 -1 -2 0 0 0 0] 1.2;\n    C C [0 -1 0.5 0 0 0 0] 131;\n    H H [0 1 0 0 0 0 0] 1.5;\n    kc kc [0 0 0 0 0 0 0] 0.5;\n    l l [0 1 0 0 0 0 0] 0.1;\n    rsMin rsMin [0 -1 1 0 0 0 0] 150;\n    Rg0 Rg0 [1 0 -3 0 0 0 0] 800;\n    Rl0 Rl0 [1 0 -3 0 0 0 0] 350;\n    nEvapSides nEvapSides [0 0 0 0 0 0 0] 1;\n}\n";

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
            format!("{}vegetationModel simplifiedVegetation;\n{SIMPLIFIED_VEGETATION}", header("dictionary")),
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

/// `fs` with the other fields the solver reads in each region (`field.missing`) added where
/// they are missing, at each time the region has fields. Their entries come from the mesh as
/// it is then (a constraint patch gets its type, any other `zeroGradient`), so they follow a
/// test's edits and add no diagnostics of their own.
fn complete(fs: &MemFs) -> MemFs {
    let mut out = fs.clone();
    let Ok(case) = Case::open(fs) else { return out };
    let g = format!("{}dimensions [0 1 -2 0 0 0 0];\nvalue (0 0 -9.81);\n", header("uniformDimensionedVectorField"));
    let times: Vec<String> = fs.list("").unwrap().into_iter().filter(|t| t.parse::<f64>().is_ok()).collect();
    for r in &case.regions {
        // Added only where the region's temperature field is: a test without one is about that.
        let (template, names): (&str, &[&str]) = match r.kind {
            RegionKind::Fluid => ("T", &["U", "w", "gcr", "p_rgh", "p", "nut", "alphat", "k", "epsilon"]),
            RegionKind::Solid => ("Ts", &["ws", "pc"]),
            RegionKind::Vegetation => ("Tl", &["T"]),
            RegionKind::Single => continue,
        };
        let Some(patches) = &r.patches else { continue };
        let mut bcs = String::new();
        for p in patches {
            let kind =
                if ["empty", "symmetryPlane", "symmetry", "wedge", "cyclic", "processor"].contains(&p.kind.as_str()) {
                    p.kind.as_str()
                } else {
                    "zeroGradient"
                };
            bcs.push_str(&format!("    {} {{ type {kind}; }}\n", p.name));
        }
        let at = times.iter().map(String::as_str).filter(|t| fs.is_file(&format!("{t}/{}/{template}", r.name)));
        for t in at {
            let mut names: Vec<&str> = names.to_vec();
            // simplifiedVegetation reads LAD from 0.
            if r.kind == RegionKind::Fluid && t == "0" {
                names.push("LAD");
            }
            for n in names {
                let rel = format!("{t}/{}/{n}", r.name);
                if !fs.is_file(&rel) && !fs.is_file(&format!("{rel}.gz")) {
                    let class = if n == "U" { "volVectorField" } else { "volScalarField" };
                    out.insert(&rel, field(class, &bcs));
                }
            }
        }
        let rel = format!("constant/{}/g", r.name);
        if r.kind != RegionKind::Vegetation && !fs.is_file(&rel) {
            out.insert(&rel, g.clone());
        }
    }
    out
}

fn codes(fs: &MemFs) -> Vec<(Level, &'static str)> {
    let c = Case::open(&complete(fs)).unwrap();
    check(&c).into_iter().map(|d| (d.level, d.code)).collect()
}

fn errors(fs: &MemFs) -> Vec<Diagnostic> {
    let c = Case::open(&complete(fs)).unwrap();
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
    // An empty region is not a word: OpenFOAM stops, in both versions.
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "neighbourRegion street;", "neighbourRegion \"\";");
    assert!(errors(&fs).iter().any(|d| d.code == "coupling.region" && d.message.contains("empty `neighbourRegion`")));
    // A missing region is the patch's own, in both versions: here `air` has no `street_to_air`.
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "neighbourRegion street;", "");
    let e = errors(&fs);
    assert!(e.iter().any(|d| d.code == "coupling.patch") && !e.iter().any(|d| d.code == "coupling.region"), "{e:?}");
    let mut of8 = of8_case();
    assert_eq!(codes(&of8), []);
    edit(&mut of8, "constant/air/polyMesh/boundary", "sampleRegion street;", "");
    let e = errors(&of8);
    assert!(e.iter().any(|d| d.code == "coupling.patch") && !e.iter().any(|d| d.code == "coupling.region"), "{e:?}");
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

/// The test case as an OpenFOAM-8 case: `constant/regionProperties`, `fvOptions`, and the
/// `sample*` coupling keywords.
fn of8_case() -> MemFs {
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
        let text = fs.read(rel, 1 << 20).unwrap().unwrap();
        let text = String::from_utf8(text).unwrap();
        of8.insert(rel, text.replace("neighbourRegion", "sampleRegion").replace("neighbourPatch", "samplePatch"));
    }
    of8.insert("system/air/fvOptions", format!("{}\n", header("dictionary")));
    of8
}

#[test]
fn a_type_from_the_other_version() {
    // OSB exists only in the OpenFOAM-12 solver; this case is OpenFOAM-8 (regionProperties).
    let mut of8 = of8_case();
    let c = Case::open(&complete(&of8)).unwrap();
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
    let c: Vec<_> = Case::open(&complete(&fs))
        .map(|c| check(&c))
        .unwrap()
        .into_iter()
        .filter(|d| d.level == Level::Error)
        .collect();
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
    // `coupleGroup` finds the partner by group. OF12 refuses partner keywords next to it.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "neighbourRegion street;\n        neighbourPatch street_to_air;",
        "coupleGroup streetAir;",
    );
    assert_eq!(codes(&fs), []);
    let mut fs = case();
    edit(&mut fs, "constant/air/polyMesh/boundary", "neighbourRegion street;", "coupleGroup streetAir;");
    assert!(errors(&fs).iter().any(|d| d.code == "coupling.group" && d.message.contains("`neighbourPatch`")));
    // `samePatch false` is no partner keyword; `mappedInternal` reads no coupleGroup.
    let mut fs = case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "neighbourRegion street;\n        neighbourPatch street_to_air;",
        "coupleGroup streetAir;\n        samePatch false;",
    );
    assert_eq!(codes(&fs), []);
    let mut fs = case();
    edit(
        &mut fs,
        "constant/vegetation/polyMesh/boundary",
        "neighbourRegion air;",
        "coupleGroup g;\n        neighbourRegion air;",
    );
    assert_eq!(codes(&fs), []);
    edit(&mut fs, "constant/vegetation/polyMesh/boundary", "neighbourRegion air;", "neighbourRegion airs;");
    assert!(errors(&fs).iter().any(|d| d.code == "coupling.region"), "{:?}", errors(&fs));
    // OF8 reads an empty sampleRegion even next to a coupleGroup.
    let mut of8 = of8_case();
    edit(
        &mut of8,
        "constant/air/polyMesh/boundary",
        "sampleRegion street;",
        "coupleGroup g;\n        sampleRegion \"\";",
    );
    assert!(errors(&of8).iter().any(|d| d.code == "coupling.region"), "{:?}", errors(&of8));
    // OF8 ignores the partner keywords.
    let mut of8 = of8_case();
    edit(
        &mut of8,
        "constant/air/polyMesh/boundary",
        "sampleRegion street;",
        "coupleGroup streetAir;\n        sampleRegion old;",
    );
    assert_eq!(codes(&of8), []);
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
    let c = Case::open(&complete(&fs)).unwrap();
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
    let mut fs = of8_case();
    edit(
        &mut fs,
        "constant/air/polyMesh/boundary",
        "sampleRegion street;\n        samplePatch street_to_air;",
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

fn dict(body: &str) -> String {
    format!("{}{body}", header("dictionary"))
}

fn with_script(mut fs: MemFs, name: &str, text: &str) -> MemFs {
    fs.insert(name, text);
    fs
}

#[test]
fn all_regions_needs_region_solvers_in_openfoam_12() {
    let run = "#!/bin/sh\ndecomposePar -allRegions\nmpirun -np 2 urbanMicroclimateFoam -parallel\nreconstructPar -allRegions\n";
    let mut fs = with_script(case(), "Allrun", run);
    fs.insert("system/decomposeParDict", dict("numberOfSubdomains 2;\n"));
    let e = errors(&fs);
    assert_eq!(e.iter().filter(|d| d.code == "workflow.all-regions").count(), 2, "{e:?}");
    assert!(e[0].message.starts_with("line 2:"), "{}", e[0].message);
    // With regionSolvers naming the regions: fine.
    edit(
        &mut fs,
        "system/controlDict",
        "regions\n",
        "regionSolvers { air fluid; street solid; vegetation vegetation; }\nregions\n",
    );
    assert_eq!(codes(&fs), []);
    // Naming other regions: a warning.
    edit(&mut fs, "system/controlDict", "street solid;", "streets solid;");
    assert!(codes(&fs).contains(&(Level::Warning, "workflow.all-regions")));
}

#[test]
fn utilities_the_version_lacks_and_process_counts() {
    let mut fs = with_script(
        case(),
        "Allprepare",
        "#!/bin/sh\nsetSet -region street -batch system/street/setset.batch\nif [ -d x ]; then foamCleanPolyMesh -region air; fi\n",
    );
    fs.insert("system/street/setset.batch", "cellSet x new boxToCell (0 0 0) (1 1 1)\n");
    let c = codes(&fs);
    assert!(c.contains(&(Level::Error, "workflow.utility")), "{c:?}");
    // In an `if`, only a warning: it may not run.
    assert!(c.contains(&(Level::Warning, "workflow.utility")), "{c:?}");
    let e = errors(&fs);
    assert!(e[0].message.contains("`setSet` is not in OpenFOAM-12: use topoSet"), "{}", e[0].message);

    let mut fs = with_script(
        case(),
        "job_run.sh",
        "#!/bin/bash\n#SBATCH --ntasks=2\nmpirun -np 4 urbanMicroclimateFoam -parallel\n",
    );
    fs.insert("system/decomposeParDict", dict("numberOfSubdomains 4;\n"));
    assert!(codes(&fs).contains(&(Level::Warning, "workflow.ntasks")));
    edit(&mut fs, "system/decomposeParDict", "numberOfSubdomains 4;", "numberOfSubdomains 8;");
    let e = errors(&fs);
    assert!(e.iter().any(|d| d.code == "workflow.np" && d.message.contains("decomposed into 8")), "{e:?}");
}

#[test]
fn files_a_step_reads_must_be_there_at_that_step() {
    let prepare = "#!/bin/sh\n\
cp system/blockMeshDict.air system/blockMeshDict\n\
blockMesh\n\
mv constant/polyMesh constant/air/.\n\
rm -f system/blockMeshDict\n\
topoSet -region air\n\
cp 0/air.bckp/LAD 0/air/\n\
setFields -region air\n";
    let mut fs = with_script(case(), "Allprepare", prepare);
    fs.insert("system/blockMeshDict.air", dict(""));
    fs.insert("system/air/topoSetDict", dict(""));
    fs.insert("system/air/setFieldsDict", dict(""));
    fs.insert("0/air.bckp/LAD", field("volScalarField", ""));
    assert_eq!(codes(&fs), []);
    // A renamed variant, a missing dictionary, a missing backup: each at its line.
    let mut fs = with_script(case(), "Allprepare", prepare);
    fs.insert("system/blockMeshDict.veg", dict(""));
    fs.insert("system/air/setFieldsDict", dict(""));
    let c = Case::open(&fs).unwrap();
    let missing: Vec<String> =
        check(&c).into_iter().filter(|d| d.code == "workflow.missing").map(|d| d.message).collect();
    // The cause only: blockMesh reading the copy that wasn't made is not reported again.
    assert_eq!(missing.len(), 3, "{missing:#?}");
    assert!(missing[0].starts_with("line 2: `cp system/blockMeshDict.air system/blockMeshDict` copies"));
    assert!(missing[1].starts_with("line 6: `topoSet -region air` reads system/air/topoSetDict"));
    assert!(missing[2].contains("copies 0/air.bckp/LAD"));
    // A step after one the check doesn't follow is not judged.
    let mut fs = with_script(case(), "Allprepare", "#!/bin/sh\n./makeDicts.py\ntopoSet -region air\n");
    fs.insert("makeDicts.py", "");
    assert!(!codes(&fs).iter().any(|(_, c)| *c == "workflow.missing"));
}

#[test]
fn openfoam_8_reads_dict_paths_in_the_region() {
    // The OpenFOAM-8 vegetation tutorial: the path goes through the region's directory.
    let script = "#!/bin/sh\ntopoSet -region vegetation -dict system/../air/topoSetDict\n";
    let mut of8 = with_script(of8_case(), "Allprepare", script);
    of8.insert("system/air/topoSetDict", dict(""));
    assert!(!codes(&of8).iter().any(|(_, c)| *c == "workflow.missing"), "{:?}", codes(&of8));
    // An OpenFOAM-12 path in an OpenFOAM-8 case reads system/vegetation/air/topoSetDict.
    let mut of8 =
        with_script(of8_case(), "Allprepare", "#!/bin/sh\ntopoSet -region vegetation -dict system/air/topoSetDict\n");
    of8.insert("system/air/topoSetDict", dict(""));
    let c = Case::open(&of8).unwrap();
    let missing: Vec<String> =
        check(&c).into_iter().filter(|d| d.code == "workflow.missing").map(|d| d.message).collect();
    assert!(missing.iter().any(|m| m.contains("system/vegetation/air/topoSetDict")), "{missing:?}");
    // OpenFOAM-12 reads the path from the case: system/../air/topoSetDict is air/topoSetDict.
    let mut of12 = with_script(case(), "Allprepare", script);
    of12.insert("system/air/topoSetDict", dict(""));
    let c = Case::open(&of12).unwrap();
    let missing: Vec<String> =
        check(&c).into_iter().filter(|d| d.code == "workflow.missing").map(|d| d.message).collect();
    assert!(missing.iter().any(|m| m.contains("reads air/topoSetDict")), "{missing:?}");
    let mut of12 =
        with_script(case(), "Allprepare", "#!/bin/sh\ntopoSet -region vegetation -dict system/air/topoSetDict\n");
    of12.insert("system/air/topoSetDict", dict(""));
    assert!(!codes(&of12).iter().any(|(_, c)| *c == "workflow.missing"), "{:?}", codes(&of12));
}

/// What `workflow.missing` says `script` (a line of Allprepare) reads, in `fs`.
fn missing_reads(fs: MemFs, script: &str) -> Vec<String> {
    let fs = with_script(fs, "Allprepare", &format!("#!/bin/sh\n{script}\n"));
    let c = Case::open(&fs).unwrap();
    check(&c).into_iter().filter(|d| d.code == "workflow.missing").map(|d| d.message).collect()
}

#[test]
fn dict_paths_as_each_version_reads_them() {
    let reads = |fs: MemFs, script: &str, path: &str| {
        let m = missing_reads(fs, script);
        assert!(m.iter().any(|m| m.contains(&format!("reads {path},"))), "{script}: {m:?}");
    };
    // A directory: the dictionary in it, through the region in OpenFOAM-8.
    reads(of8_case(), "topoSet -region vegetation -dict system", "system/vegetation/topoSetDict");
    reads(case(), "topoSet -region vegetation -dict system", "system/topoSetDict");
    // A bare name: OpenFOAM-8 puts the region before it, OpenFOAM-12 looks in system/.
    reads(of8_case(), "topoSet -region vegetation -dict myDict", "vegetation/myDict");
    reads(case(), "topoSet -region vegetation -dict myDict", "system/vegetation/myDict");
    reads(case(), "topoSet -dict myDict", "system/myDict");
    reads(case(), "blockMesh -region air -dict myDict", "system/air/myDict");
    reads(case(), "faceAgglomerate -region vegetation -dict myDict", "system/myDict");
    // faceAgglomerate reads through the mesh too (setConstantMeshDictionaryIO.H).
    reads(
        of8_case(),
        "faceAgglomerate -region vegetation -dict constant/viewFactorsDict",
        "constant/vegetation/viewFactorsDict",
    );
    // blockMesh reads it through the Time: from the case.
    reads(of8_case(), "blockMesh -region air -dict system/blockMeshDict.air", "system/blockMeshDict.air");
    // An absolute path is itself.
    reads(of8_case(), "topoSet -region vegetation -dict /abs/topoSetDict", "/abs/topoSetDict");
}

#[test]
fn collapsing_paths() {
    use crate::workflow::collapse;
    assert_eq!(collapse("system/vegetation/../air/topoSetDict"), "system/air/topoSetDict");
    assert_eq!(collapse("./a//b/."), "a/b");
    assert_eq!(collapse("../x"), "../x");
    assert_eq!(collapse("a/../../b"), "../b");
    assert_eq!(collapse("/.."), "/");
    assert_eq!(collapse("/a/../b"), "/b");
}

#[test]
fn copies_over_dictionaries_and_broken_first_lines() {
    let mut fs = with_script(
        case(),
        "Allrun",
        "$#!/bin/sh\ncp constant/vegetation/viewFactorsDict system/vegetation/viewFactorsDict\n",
    );
    fs.insert("constant/vegetation/viewFactorsDict", dict("nFacesInCoarsestLevel 15000;\n"));
    fs.insert("system/vegetation/viewFactorsDict", dict("nFacesInCoarsestLevel 3000;\n"));
    let c = Case::open(&fs).unwrap();
    let d = check(&c);
    let over = d.iter().find(|d| d.code == "workflow.overwrites").unwrap();
    assert_eq!((over.level, over.path.as_str()), (Level::Warning, "system/vegetation/viewFactorsDict"));
    assert!(d.iter().any(|d| d.code == "workflow.shebang"));
    // The same text: nothing to say.
    fs.insert("system/vegetation/viewFactorsDict", dict("nFacesInCoarsestLevel 15000;\n"));
    assert!(!codes(&fs).contains(&(Level::Warning, "workflow.overwrites")));
}

#[test]
fn openfoam_12_fv_constraints_and_pressure_limits() {
    let mut fs = case();
    fs.insert("system/air/fvConstraints", dict("limitp { type limitPressure; minFactor 0.5; }\n"));
    fs.insert("system/air/fvOptions", dict("limitT { type limitTemperature; min 250; }\n"));
    fs.insert("system/air/fvSolution", dict("SIMPLE\n{\n    pMinFactor 0.5;\n    nNonOrthogonalCorrectors 0;\n}\n"));
    let c = codes(&fs);
    assert!(c.contains(&(Level::Warning, "fv.options-ignored")), "{c:?}");
    assert!(c.contains(&(Level::Warning, "fv.limit-pressure")), "{c:?}");
    assert!(c.contains(&(Level::Error, "control.pressure-limits")), "{c:?}");
    // OpenFOAM-8 reads fvOptions and the SIMPLE limits.
    let mut of8 = of8_case();
    of8.insert("system/air/fvSolution", dict("SIMPLE\n{\n    pMinFactor 0.5;\n}\n"));
    assert_eq!(codes(&of8), []);
}

#[test]
fn calc_lai_needs_a_region_named_vegetation() {
    let mut fs = with_script(case(), "Allrun", "#!/bin/sh\ncalcLAI -region air\n");
    assert_eq!(codes(&fs), []);
    edit(&mut fs, "system/controlDict", "vegetation (vegetation);", "vegetation (trees);");
    assert!(errors(&fs).iter().any(|d| d.code == "workflow.calclai"));
}

#[test]
fn review_false_alarms() {
    // A test guarding a copy: it may not run, so its source isn't required.
    let fs = with_script(case(), "Allprepare", "#!/bin/sh\n[ -f system/foo ] && cp system/foo system/bar\n");
    assert_eq!(codes(&fs), [(Level::Info, "workflow.custom")]);
    // blockMesh finds its dictionary in constant/<r>/polyMesh/ too.
    let mut fs = with_script(case(), "Allprepare", "#!/bin/sh\nblockMesh -region street\n");
    fs.insert("constant/street/polyMesh/blockMeshDict", dict(""));
    assert_eq!(codes(&fs), []);
    // OpenFOAM-8 decomposePar -allRegions reads each region's own decomposeParDict.
    let fs = with_script(of8_case(), "Allrun", "#!/bin/sh\ndecomposePar -allRegions\n");
    assert!(!codes(&fs).iter().any(|(_, c)| *c == "workflow.missing"), "{:?}", codes(&fs));
    // An OpenFOAM-12 script that runs the replacement: a warning, not an error.
    let not_info = |fs: &MemFs| codes(fs).into_iter().filter(|(l, _)| *l != Level::Info).collect::<Vec<_>>();
    let fs = with_script(case(), "Allrun", "#!/bin/sh\npostProcess -func yPlus\n");
    assert_eq!(not_info(&fs), [(Level::Warning, "workflow.utility")]);
    // runParallel without system/decomposeParDict.
    let fs = with_script(
        case(),
        "Allrun",
        "#!/bin/sh\n. $WM_PROJECT_DIR/bin/tools/RunFunctions\nrunParallel urbanMicroclimateFoam\n",
    );
    assert_eq!(not_info(&fs), [(Level::Warning, "workflow.np")]);
}

/// `field.missing` for the fields of `fs` (completed, then with `remove` taken out).
fn missing(fs: &MemFs, remove: &[&str]) -> Vec<String> {
    let mut fs = complete(fs);
    for r in remove {
        fs = without(&fs, r);
    }
    let c = Case::open(&fs).unwrap();
    check(&c).into_iter().filter(|d| d.code == "field.missing").map(|d| d.path).collect()
}

fn without(fs: &MemFs, rel: &str) -> MemFs {
    let mut out = MemFs::new();
    for (k, v) in all_files(fs, "") {
        if k != rel {
            out.insert(&k, v);
        }
    }
    out
}

fn all_files(fs: &MemFs, dir: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for n in fs.list(dir).unwrap() {
        let rel = if dir.is_empty() { n } else { format!("{dir}/{n}") };
        match fs.read(&rel, 1 << 20).unwrap() {
            Some(b) => out.push((rel, b)),
            None => out.extend(all_files(fs, &rel)),
        }
    }
    out
}

#[test]
fn fields_the_solver_reads_per_region() {
    let fs = case();
    assert_eq!(missing(&fs, &[]), Vec::<String>::new());
    assert_eq!(missing(&fs, &["0/air/w"]), ["0/air/w"]);
    assert_eq!(missing(&fs, &["0/street/Ts", "0/street/pc"]), ["0/street/Ts", "0/street/pc"]);
    assert_eq!(missing(&fs, &["0/vegetation/T"]), ["0/vegetation/T"]);
    assert_eq!(missing(&fs, &["constant/street/g"]), ["constant/street/g"]);
    // OpenFOAM's thermophysics reads p.
    assert_eq!(missing(&fs, &["0/air/p"]), ["0/air/p"]);
    // Optional reads (phi, Tl, Tg) are not required.
    assert_eq!(missing(&fs, &["0/vegetation/Tl"]), Vec::<String>::new());
    // A decomposed case runs from processor*/: a warning.
    let mut dec = complete(&fs);
    dec.insert("processor0/system/x", "");
    let mut dec = without(&dec, "0/air/w");
    dec.insert("processor0/0/air/w", "");
    let c = Case::open(&dec).unwrap();
    let d: Vec<_> = check(&c).into_iter().filter(|d| d.code == "field.missing").collect();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].level, Level::Warning);
}

#[test]
fn fields_the_selected_models_read() {
    // simplifiedVegetation reads LAD from 0, whatever time the run starts from.
    let mut fs = case();
    edit(&mut fs, "system/controlDict", "startTime 0;", "startTime 3600;");
    let moved = complete(&fs);
    let mut at3600 = MemFs::new();
    for (k, v) in all_files(&moved, "") {
        let k = if k.starts_with("0/") && k != "0/air/LAD" { k.replacen("0/", "3600/", 1) } else { k };
        at3600.insert(&k, v);
    }
    assert_eq!(missing(&at3600, &[]), Vec::<String>::new());
    let d = missing(&at3600, &["0/air/LAD"]);
    assert_eq!(d, ["0/air/LAD"]);
    // noVegetation reads nothing.
    let mut fs = case();
    edit(&mut fs, "constant/air/vegetationProperties", "simplifiedVegetation", "noVegetation");
    assert_eq!(missing(&fs, &["0/air/LAD"]), Vec::<String>::new());

    // RAS: nut and alphat for any model, then the model's own fields.
    let mut fs = case();
    fs.insert("constant/air/momentumTransport", dict("simulationType RAS;\nRAS\n{\n    model realizableKE;\n}\n"));
    assert_eq!(missing(&fs, &[]), Vec::<String>::new());
    assert_eq!(missing(&fs, &["0/air/nut", "0/air/epsilon"]), ["0/air/epsilon", "0/air/nut"]);
    fs.insert("constant/air/momentumTransport", dict("simulationType RAS;\nRAS\n{\n    model kOmegaSST;\n}\n"));
    assert_eq!(missing(&fs, &[]), ["0/air/omega"]);
    // OpenFOAM-8 spells it RASModel; the solver's own model reads LAD from 0 there.
    let mut of8 = of8_case();
    of8.insert(
        "constant/air/momentumTransport",
        dict("simulationType RAS;\nRAS\n{\n    RASModel porousrealizableKE;\n}\n"),
    );
    assert_eq!(missing(&of8, &["0/air/LAD", "0/air/k"]), ["0/air/LAD", "0/air/k"]);
    // In OpenFOAM-12 it reads LAD from constant/.
    let mut fs = case();
    fs.insert(
        "constant/air/momentumTransport",
        dict("simulationType RAS;\nRAS\n{\n    model porousrealizableKE;\n}\n"),
    );
    assert_eq!(missing(&fs, &[]), ["constant/air/LAD"]);
    // Laminar: no turbulence fields.
    fs.insert("constant/air/momentumTransport", dict("simulationType laminar;\n"));
    assert_eq!(missing(&fs, &["0/air/nut", "0/air/k"]), Vec::<String>::new());

    // viewFactorSky (a radiation model) reads qr where it is selected.
    let mut fs = case();
    fs.insert("constant/vegetation/radiationProperties", dict("radiationModel viewFactorSky;\n"));
    assert_eq!(missing(&fs, &[]), ["0/vegetation/qr"]);
    // directAndDiffuse, built from a field, reads qs (its dictionary constructor reads Qs).
    let mut fs = case();
    fs.insert("constant/vegetation/solarLoadProperties", dict("solarLoadModel directAndDiffuse;\n"));
    assert_eq!(missing(&fs, &[]), ["0/vegetation/qs"]);
    // A `$macro` or an unknown type selects nothing that can be checked.
    let mut fs = case();
    fs.insert("constant/vegetation/radiationProperties", dict("m viewFactorSky;\nradiationModel $m;\n"));
    assert_eq!(missing(&fs, &[]), Vec::<String>::new());
}

/// The OpenFOAM reads in `fields::OPENFOAM` are where the table says, in both versions. Opt-in:
/// `UMCF_OF8_SRC` and `UMCF_OF12_SRC` name OpenFOAM source trees (`~/OpenFOAM-8`).
#[test]
fn openfoam_reads_match_the_sources() {
    let (Ok(of8), Ok(of12)) = (std::env::var("UMCF_OF8_SRC"), std::env::var("UMCF_OF12_SRC")) else {
        eprintln!("skipped: set UMCF_OF8_SRC and UMCF_OF12_SRC to OpenFOAM source trees");
        return;
    };
    for r in fields::OPENFOAM {
        for (root, file) in [(&of8, r.of8), (&of12, r.of12)] {
            let path = std::path::Path::new(root).join(file);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let name = format!("\"{}\"", r.field);
            assert!(text.contains(&name), "{}: no {name}", path.display());
            // basicThermo::lookupOrConstruct reads with MUST_READ; fluidThermo calls it for p.
            assert!(
                text.contains("MUST_READ") || text.contains(&format!("lookupOrConstruct(mesh, {name})")),
                "{}: {name} not read",
                path.display()
            );
        }
    }
}

fn keywords_missing(fs: &MemFs) -> Vec<String> {
    let c = Case::open(&complete(fs)).unwrap();
    check(&c)
        .into_iter()
        .filter(|d| d.code == "keyword.missing")
        .map(|d| format!("{} {}", d.path, d.message.split('`').nth(3).unwrap_or("")))
        .collect()
}

#[test]
fn keywords_the_selected_models_read() {
    let fs = case();
    assert_eq!(keywords_missing(&fs), Vec::<String>::new());
    // One coefficient missing.
    let mut one = case();
    edit(&mut one, "constant/air/vegetationProperties", "    a1 a1 [1 0 -3 0 0 0 0] 169;\n", "");
    assert_eq!(keywords_missing(&one), ["constant/air/vegetationProperties a1"]);
    // No coefficients at all: every one is missing (the type reads an empty dictionary).
    let mut none = case();
    none.insert("constant/air/vegetationProperties", dict("vegetationModel simplifiedVegetation;\n"));
    assert_eq!(keywords_missing(&none).len(), 12);
    // Keys that may come from elsewhere or match a pattern: not judged.
    for body in ["#include \"common\"\n", "\"a.*\" a [1 0 -3 0 0 0 0] 1;\n", "$common\n"] {
        let mut fs = case();
        edit(&mut fs, "constant/air/vegetationProperties", "    a1 a1 [1 0 -3 0 0 0 0] 169;\n", body);
        assert_eq!(keywords_missing(&fs), Vec::<String>::new(), "{body}");
    }
    let mut fs = case();
    fs.insert(
        "constant/air/vegetationProperties",
        dict("vegetationModel simplifiedVegetation;\nsimplifiedVegetationCoeffs $c;\n"),
    );
    assert_eq!(keywords_missing(&fs), Vec::<String>::new());

    // Written twice, the blocks merge.
    let mut fs = case();
    edit(&mut fs, "constant/air/vegetationProperties", "    a1 a1 [1 0 -3 0 0 0 0] 169;\n", "");
    let text = String::from_utf8(fs.read("constant/air/vegetationProperties", 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert(
        "constant/air/vegetationProperties",
        text.replacen(
            "simplifiedVegetationCoeffs",
            "simplifiedVegetationCoeffs\n{\n    a1 a1 [1 0 -3 0 0 0 0] 169;\n}\nsimplifiedVegetationCoeffs",
            1,
        ),
    );
    assert_eq!(keywords_missing(&fs), Vec::<String>::new());

    // viewFactorSky (radiationProperties), in the vegetation region.
    let mut fs = case();
    fs.insert(
        "constant/vegetation/radiationProperties",
        dict("radiationModel viewFactorSky;\nviewFactorSkyCoeffs\n{\n    smoothing true;\n}\n"),
    );
    assert_eq!(keywords_missing(&fs), ["constant/vegetation/radiationProperties constantEmissivity"]);
    // directAndDiffuse reads its coefficients at start in both versions (in OpenFOAM-8 on the
    // master process, which every run has).
    let mut fs = case();
    fs.insert("constant/vegetation/solarLoadProperties", dict("solarLoadModel directAndDiffuse;\n"));
    assert_eq!(
        keywords_missing(&fs),
        ["constant/vegetation/solarLoadProperties constantAlbedo", "constant/vegetation/solarLoadProperties smoothing"]
    );
    let mut of8 = of8_case();
    of8.insert("constant/vegetation/solarLoadProperties", dict("solarLoadModel directAndDiffuse;\n"));
    assert_eq!(
        keywords_missing(&of8),
        ["constant/vegetation/solarLoadProperties constantAlbedo", "constant/vegetation/solarLoadProperties smoothing"]
    );
}

#[test]
fn the_workflow_description_against_the_case() {
    use umcf_model::description::{Description, PATH};
    let messages = |fs: &MemFs| -> Vec<(Level, String)> {
        let c = Case::open(&complete(fs)).unwrap();
        check(&c).into_iter().filter(|d| d.code == "workflow.description").map(|d| (d.level, d.message)).collect()
    };
    // As the import writes it from the case: nothing to say but the patches it couldn't tell.
    let mut fs = case();
    let c = Case::open(&complete(&fs)).unwrap();
    let (d, _) = Description::import(&c);
    fs.insert(PATH, d.write());
    let m = messages(&fs);
    assert!(m.iter().all(|(l, _)| *l == Level::Info), "{m:#?}");

    // Edited by hand: wrong kinds, regions, patches and partners, and a key it doesn't know.
    let text = d
        .write()
        .replace("kind        solid;", "kind        fluid;")
        .replace("air_to_street   {", "air_to_road     {")
        .replace("partner ( street street_to_air )", "partner ( road street_to_air )")
        .replace("regions\n{\n", "colour blue;\nregions\n{\n    garden { kind solid; }\n");
    fs.insert(PATH, text);
    let m: Vec<String> = messages(&fs).into_iter().filter(|(l, _)| *l == Level::Warning).map(|(_, m)| m).collect();
    let has = |s: &str| m.iter().any(|x| x.contains(s));
    assert!(has("colour: is not a key of the description"), "{m:#?}");
    assert!(has("region garden is not one of the case's"), "{m:#?}");
    assert!(has("region street is fluid, the case has it solid"), "{m:#?}");
    assert!(has("patch air_to_road of region air is not in its mesh"), "{m:#?}");
    assert!(has("its partner road/street_to_air is not in the case"), "{m:#?}");
    // Not in the description: the mesh's air_to_street (Info), not a warning.
    let infos: Vec<String> = messages(&fs).into_iter().filter(|(l, _)| *l == Level::Info).map(|(_, m)| m).collect();
    assert!(
        infos.iter().any(|x| x.contains("patch air_to_street of region air is not in the description")),
        "{infos:#?}"
    );

    // A region of the case the description leaves out, and another version.
    let text = d.write().replace("version     12;", "version     8;");
    let cut = text.find("    vegetation\n    {").and_then(|a| text[a..].find("    }\n").map(|b| (a, a + b + 6)));
    let text = match cut {
        Some((a, b)) => format!("{}{}", &text[..a], &text[b..]),
        None => text,
    };
    fs.insert(PATH, text);
    let m: Vec<String> = messages(&fs).into_iter().map(|(_, m)| m).collect();
    assert!(m.iter().any(|x| x.contains("region vegetation is not in the description")), "{m:#?}");
    assert!(m.iter().any(|x| x.contains("it is for OpenFOAM-8, the case is OpenFOAM-12")), "{m:#?}");
}

#[test]
fn description_partners_and_roles_are_checked_without_a_mesh() {
    use umcf_model::description::PATH;
    let mut fs = case();
    // A solid region without a mesh yet.
    edit(&mut fs, "system/controlDict", "solid (street);", "solid (street roofs);");
    fs.insert(
        PATH,
        dict("version 12;\nregions\n{\n    roofs { kind solid; mesh blocks; patches { a { role coupled; } b { role side; partner ( nowhere b ); } } }\n}\n"),
    );
    let c = Case::open(&complete(&fs)).unwrap();
    let m: Vec<String> =
        check(&c).into_iter().filter(|d| d.code == "workflow.description").map(|d| d.message).collect();
    assert!(m.iter().any(|x| x.contains("patch a of region roofs is coupled to no partner")), "{m:#?}");
    assert!(m.iter().any(|x| x.contains("its partner nowhere/b is not in the case")), "{m:#?}");
    assert!(!m.iter().any(|x| x.contains("is not in its mesh")), "{m:#?}");
}
