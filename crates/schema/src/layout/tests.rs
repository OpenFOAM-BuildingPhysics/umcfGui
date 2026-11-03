use umcf_core::MemFs;

use super::*;

const BANNER_222: &str = "/*\\\n| \\\\  /    A nd           | Version:  2.2.2                    |\n\\*/\n";
const CD_REGIONS: &str = "application urbanMicroclimateFoam;\nregions\n{\n    fluid ( air );\n    solid ( street roofs buildings );\n    vegetation ( vegetation );\n}\n";
const RP: &str = "regions\n(\n    fluid   (air)\n\tsolid   (street roofs buildings)\n    vegetation (vegetation)\n);\n";

fn with_region_dirs(mut fs: MemFs) -> MemFs {
    for r in ["air", "street", "roofs", "buildings", "vegetation"] {
        fs.insert(&format!("constant/{r}/g"), "");
    }
    fs
}

fn expected_regions() -> Regions {
    Regions {
        fluid: vec!["air".into()],
        solid: vec!["street".into(), "roofs".into(), "buildings".into()],
        vegetation: vec!["vegetation".into()],
    }
}

#[test]
fn of12_case() {
    // Like testcases/Mesh_3x3_small: regions in controlDict, fvConstraints and fvModels.
    let fs = with_region_dirs(
        MemFs::new()
            .with("system/controlDict", format!("{BANNER_222}{CD_REGIONS}"))
            .with("system/air/fvConstraints", "")
            .with("constant/air/fvModels", ""),
    );
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V12));
    assert_eq!(l.kind, CaseKind::Solver);
    assert_eq!(l.regions, expected_regions());
    assert_eq!(l.regions_from.as_deref(), Some("system/controlDict"));
    assert!(!l.mixed());
    assert!(l.notes.is_empty(), "{:?}", l.notes);
}

#[test]
fn of8_case() {
    // Like new_Mesh_3x3: constant/regionProperties and fvOptions.
    let fs = with_region_dirs(
        MemFs::new()
            .with("system/controlDict", format!("{BANNER_222}application urbanMicroclimateFoam;\n"))
            .with("constant/regionProperties", RP)
            .with("system/air/fvOptions", ""),
    );
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V8));
    assert_eq!(l.regions, expected_regions());
    assert_eq!(l.regions_from.as_deref(), Some("constant/regionProperties"));
    assert!(l.notes.is_empty(), "{:?}", l.notes);
}

#[test]
fn ported_case_with_leftovers() {
    // Like BIGTESTCASES/*: OF12 with a stale regionProperties, and Mesh_3x3: a stale fvOptions.
    let fs = with_region_dirs(
        MemFs::new()
            .with("system/controlDict", CD_REGIONS)
            .with("constant/regionProperties", RP)
            .with("system/air/fvOptions", "")
            .with("system/air/fvConstraints", "")
            .with("constant/air/fvModels", ""),
    );
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V12));
    assert!(l.mixed());
    assert_eq!(l.score(OfVersion::V12), 9);
    assert_eq!(l.score(OfVersion::V8), 7);
    let leftovers: Vec<&str> =
        l.notes.iter().filter(|n| n.severity == Severity::Info).map(|n| n.path.as_str()).collect();
    assert_eq!(leftovers, ["constant/regionProperties", "system/air/fvOptions"]);
    assert_eq!(l.warnings().count(), 0);
}

#[test]
fn of12_without_controldict_regions_warns() {
    let fs = with_region_dirs(
        MemFs::new()
            .with("system/controlDict", "application urbanMicroclimateFoam;\n")
            .with("constant/regionProperties", RP)
            .with("system/air/fvConstraints", "")
            .with("constant/air/fvModels", "")
            .with("constant/air/physicalProperties", ""),
    );
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V12));
    assert_eq!(l.regions, expected_regions());
    assert!(l.warnings().any(|n| n.message.contains("will not find its regions")), "{:?}", l.notes);
}

#[test]
fn banner_is_a_weak_hint() {
    let fs = MemFs::new().with("system/controlDict", "/* Version:  12 */\na 1;\n");
    assert_eq!(detect(&fs).unwrap().version, Some(OfVersion::V12));
    let fs = MemFs::new().with("system/controlDict", "/* Version: 8 */\na 1;\n");
    assert_eq!(detect(&fs).unwrap().version, Some(OfVersion::V8));
    let fs = MemFs::new().with("system/controlDict", format!("{BANNER_222}a 1;\n"));
    assert_eq!(detect(&fs).unwrap().version, None);
}

#[test]
fn not_a_case() {
    assert!(matches!(detect(&MemFs::new()), Err(Error::NotACase)));
}

#[test]
fn hostile_dictionaries_become_warnings() {
    // Unparsable controlDict: an error with the position, not a crash.
    let fs = MemFs::new().with("system/controlDict", "regions { fluid (air; }");
    let e = detect(&fs).unwrap_err();
    assert!(matches!(e, Error::Dict { .. }), "{e}");
    assert!(e.to_string().starts_with("system/controlDict: "), "{e}");
    // Any other unparsable dictionary is a warning.
    let fs = MemFs::new().with("system/controlDict", "a 1;\n").with("constant/regionProperties", "regions (");
    let l = detect(&fs).unwrap();
    assert!(l.warnings().any(|n| n.path == "constant/regionProperties" && n.message.contains("cannot be parsed")));

    let fs = MemFs::new()
        .with("system/controlDict", "regions { fluid (air ../../etc); solid 3; weird (x); vegetation ((a)); }\n")
        .with("constant/air/g", "");
    let l = detect(&fs).unwrap();
    assert_eq!(l.regions.fluid, ["air"]);
    let msgs: Vec<&str> = l.notes.iter().map(|n| n.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("invalid region name `../../etc`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("regions/solid")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("unknown region kind `weird`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("regions/vegetation")), "{msgs:?}");

    let fs =
        MemFs::new().with("system/controlDict", "a 1;\n").with("constant/regionProperties", "regions ( fluid );\n");
    let l = detect(&fs).unwrap();
    assert!(l.notes.iter().any(|n| n.message.contains("alternate")), "{:?}", l.notes);
    let fs = MemFs::new().with("system/controlDict", "a 1;\n").with("constant/regionProperties", "regions { };\n");
    assert!(detect(&fs).unwrap().notes.iter().any(|n| n.message.contains("should be a list")));
}

#[test]
fn missing_region_directory_warns() {
    let fs = MemFs::new().with("system/controlDict", "regions { fluid (air); }\n");
    let l = detect(&fs).unwrap();
    assert!(l.warnings().any(|n| n.path == "constant/air"), "{:?}", l.notes);
}

#[test]
fn falls_back_to_region_directories_for_signals() {
    // No region list at all: fvOptions in system/air still identifies OF8.
    let fs = MemFs::new().with("system/controlDict", "a 1;\n").with("system/air/fvOptions", "");
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V8));
    assert_eq!(l.kind, CaseKind::Meshing);
    assert_eq!(l.warnings().count(), 0, "{:?}", l.notes);
}

#[test]
fn meshing_steps() {
    // Like HK 0c_mesh_sHM_of8: snappyHexMesh with locationInMesh, extrude variants.
    let shm8 = "castellatedMeshControls\n{\n    maxLocalCells 100;\n    locationInMesh (1000 1000 100);\n}\n";
    let fs = MemFs::new()
        .with("system/controlDict", "application cartesianMesh;\n")
        .with("system/snappyHexMeshDict", shm8)
        .with("system/blockMeshDict", "")
        .with("system/extrudeMeshDict", "")
        .with("system/extrudeMeshDict.east", "")
        .with("system/topoSetDict_cellZones", "")
        .with("system/fvSchemes", "");
    let l = detect(&fs).unwrap();
    assert_eq!(l.kind, CaseKind::Meshing);
    assert_eq!(l.version, Some(OfVersion::V8));
    assert_eq!(l.score(OfVersion::V8), 1);
    assert_eq!(l.tools, ["blockMesh", "snappyHexMesh", "extrudeMesh", "topoSet"]);
    assert!(l.notes.is_empty(), "{:?}", l.notes);

    let shm12 = "castellatedMeshControls { insidePoint (0 0 1); }\n";
    let fs = MemFs::new().with("system/controlDict", "a 1;\n").with("system/snappyHexMeshDict", shm12);
    assert_eq!(detect(&fs).unwrap().version, Some(OfVersion::V12));

    // An OF12 case may keep `locationInMesh` (OF12 accepts it): no leftover note.
    let fs =
        with_region_dirs(MemFs::new().with("system/controlDict", CD_REGIONS).with("system/snappyHexMeshDict", shm8));
    let l = detect(&fs).unwrap();
    assert_eq!(l.version, Some(OfVersion::V12));
    assert!(l.notes.is_empty(), "{:?}", l.notes);

    // Like HK 0d_*Extrusion: nothing tells the version apart.
    let fs = MemFs::new().with("system/controlDict", "a 1;\n").with("system/extrudeMeshDict", "");
    let l = detect(&fs).unwrap();
    assert_eq!((l.kind, l.version), (CaseKind::Meshing, None));
    assert_eq!(l.tools, ["extrudeMesh"]);
}
