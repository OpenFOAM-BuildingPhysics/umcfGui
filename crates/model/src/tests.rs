use umcf_core::{CaseFs, MemFs};

use super::*;

fn header(class: &str, object: &str) -> String {
    format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       {class};\n    object      {object};\n}}\n"
    )
}

const CONTROL: &str = "\
application     urbanMicroclimateFoam;   // the solver

startFrom       startTime;
startTime       0;
stopAt          endTime;
endTime         86400;
deltaT          3600;
writeControl    adjustableRunTime;
writeInterval   $deltaT;

regions
{
    fluid       (air);
    solid       (street);
    vegetation  (vegetation);
}
";

const BOUNDARY: &str = "2\n(\n    ground\n    {\n        type            wall;\n        nFaces          4;\n        startFace       10;\n    }\n    top\n    {\n        type            patch;\n        nFaces          2;\n        startFace       14;\n    }\n)\n";

fn case() -> MemFs {
    let h = |c, o| header(c, o);
    MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", h("dictionary", "controlDict")))
        .with("system/decomposeParDict", format!("{}numberOfSubdomains 4;\nmethod scotch;\n", h("dictionary", "decomposeParDict")))
        .with("system/controlDict.bak", "not { a dictionary")
        .with("system/.hidden", "x 1;")
        .with("system/air/fvSchemes", format!("{}ddtSchemes {{ default Euler; }}\n", h("dictionary", "fvSchemes")))
        .with("system/air/fvSolution", format!("{}PIMPLE {{ nOuterCorrectors 2; }}\n", h("dictionary", "fvSolution")))
        .with("system/street/fvSolution", format!("{}solvers {{}}\n", h("dictionary", "fvSolution")))
        .with("constant/air/g", format!("{}dimensions [0 1 -2 0 0 0 0];\nvalue (0 0 -9.81);\n", h("uniformDimensionedVectorField", "g")))
        .with("constant/air/fvModels", format!("{}\n", h("dictionary", "fvModels")))
        .with("constant/air/polyMesh/boundary", format!("{}{BOUNDARY}", h("polyBoundaryMesh", "boundary")))
        .with(
            "0/air/T",
            format!(
                "{}dimensions [0 0 0 1 0 0 0];\ninternalField uniform 300;\nboundaryField\n{{\n    ground {{ type fixedValue; value nonuniform List<scalar> 4(300 301 302 303); }}\n    top {{ type zeroGradient; }}\n    \"(wall|roof).*\" {{ type calculated; value $internalField; }}\n}}\n",
                h("volScalarField", "T")
            ),
        )
        .with("0/air/U.orig", "ignored")
        .with("0/street/T", format!("{}internalField uniform 290;\nboundaryField {{ }}\n", h("volScalarField", "T")))
        .with("3600/air/T", "a later time is not read")
}

#[test]
fn opens_a_multi_region_case() {
    let c = Case::open(&case()).unwrap();
    assert_eq!(c.layout.version, Some(umcf_core::OfVersion::V12));
    assert_eq!(c.controls.application.as_deref(), Some("urbanMicroclimateFoam"));
    assert_eq!(c.controls.end_time.as_deref(), Some("86400"));
    assert_eq!(c.controls.write_interval.as_deref(), Some("$deltaT"));
    assert_eq!(Controls::number(c.controls.write_interval.as_ref()), None);
    assert_eq!(c.start.as_ref().map(|t| t.name.as_str()), Some("0"));

    let kinds: Vec<_> = c.regions.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(
        kinds,
        [("air", RegionKind::Fluid), ("street", RegionKind::Solid), ("vegetation", RegionKind::Vegetation)]
    );
    let air = c.region("air").unwrap();
    let patches: Vec<_> = air.patches.as_ref().unwrap().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(patches, ["ground", "top"]);
    assert!(c.region("street").unwrap().patches.is_none(), "no mesh yet");

    let t = air.field("T").unwrap();
    assert_eq!(t.path, "0/air/T");
    assert_eq!(t.class.as_deref(), Some("volScalarField"));
    assert_eq!(t.internal.as_deref(), Some("uniform 300"));
    let bcs = t.bcs.as_ref().unwrap();
    let summary: Vec<_> =
        bcs.iter().map(|b| (b.patch.as_str(), b.pattern, b.kind.as_deref(), b.value.as_deref())).collect();
    assert_eq!(
        summary,
        [
            ("ground", false, Some("fixedValue"), Some("nonuniform List<scalar> 4")),
            ("top", false, Some("zeroGradient"), None),
            ("(wall|roof).*", true, Some("calculated"), Some("$internalField")),
        ]
    );
    assert!(air.field("U").is_none(), "U.orig is not a field");
    assert_eq!(c.region("street").unwrap().fields.len(), 1);

    let dicts: Vec<_> = c.dict_paths().collect();
    assert!(dicts.contains(&"system/controlDict") && dicts.contains(&"constant/air/g"), "{dicts:?}");
    assert!(dicts.contains(&"system/street/fvSolution"));
    assert!(!dicts.iter().any(|d| d.contains("polyMesh") || d.contains(".hidden") || d.ends_with(".bak")));
    assert!(c.notes.iter().any(|n| n.path == "system/controlDict.bak"), "{:?}", c.notes);
    assert!(c.changed().is_empty());
}

#[test]
fn edits_change_one_entry_and_save_only_that_file() {
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    let before = c.clone();
    c.set("system/controlDict", &["endTime"], "172800").unwrap();
    assert_eq!(c.controls.end_time.as_deref(), Some("172800"));
    assert_eq!(before.controls.end_time.as_deref(), Some("86400"), "a clone is a snapshot");
    c.set("system/air/fvSolution", &["PIMPLE", "nOuterCorrectors"], "3").unwrap();
    assert_eq!(c.changed(), ["system/air/fvSolution", "system/controlDict"]);

    let saved = c.save(&mut fs, Some(".umcfgui/backup/s1")).unwrap();
    assert_eq!(saved, ["system/air/fvSolution", "system/controlDict"]);
    let text = String::from_utf8(fs.read("system/controlDict", 1 << 20).unwrap().unwrap()).unwrap();
    let original = format!("{}{CONTROL}", header("dictionary", "controlDict"));
    assert_eq!(text, original.replace("endTime         86400;", "endTime         172800;"), "comments and layout kept");
    let backup = fs.read(".umcfgui/backup/s1/system/controlDict", 1 << 20).unwrap().unwrap();
    assert_eq!(backup, original.as_bytes());
    assert!(c.changed().is_empty());

    // A second save in the session keeps the first backup.
    c.set("system/controlDict", &["endTime"], "1").unwrap();
    c.save(&mut fs, Some(".umcfgui/backup/s1")).unwrap();
    assert_eq!(fs.read(".umcfgui/backup/s1/system/controlDict", 1 << 20).unwrap().unwrap(), original.as_bytes());

    // Setting an entry back to what it was is not a change.
    let mut c = Case::open(&fs).unwrap();
    c.set("system/controlDict", &["endTime"], "1").unwrap();
    assert!(c.changed().is_empty());
}

#[test]
fn save_refuses_to_overwrite_a_file_changed_on_disk() {
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.set("system/controlDict", &["endTime"], "5").unwrap();
    fs.insert("system/controlDict", "edited elsewhere");
    let e = c.save(&mut fs, None).unwrap_err();
    assert!(matches!(e, Error::Conflict { .. }), "{e}");
    assert_eq!(fs.read("system/controlDict", 100).unwrap().unwrap(), b"edited elsewhere");
    // Deleted on disk is a conflict too.
    let fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.set("system/air/fvSchemes", &["ddtSchemes", "default"], "steadyState").unwrap();
    let mut gone = MemFs::new().with("system/controlDict", fs.read("system/controlDict", 1 << 20).unwrap().unwrap());
    assert!(matches!(c.save(&mut gone, None), Err(Error::Conflict { .. })));
    assert!(!gone.is_file("system/air/fvSchemes"), "not recreated");
}

#[test]
fn edits_need_a_loaded_dictionary() {
    let mut c = Case::open(&case()).unwrap();
    assert!(matches!(c.set("0/air/T", &["internalField"], "uniform 1"), Err(Error::NotLoaded { .. })));
    assert!(matches!(c.set("system/controlDict.bak", &["a"], "1"), Err(Error::NotLoaded { .. })));
    assert!(matches!(c.remove("system/nope", &["a"]), Err(Error::NotLoaded { .. })));
    assert!(c.remove("system/controlDict", &["writeInterval"]).unwrap());
    assert_eq!(c.controls.write_interval, None);
    assert!(!c.remove("system/controlDict", &["writeInterval"]).unwrap());
}

#[test]
fn start_time_resolution() {
    let times: Vec<TimeDir> =
        [0.0, 3600.0, 7200.0].iter().map(|v| TimeDir { name: v.to_string(), value: *v }).collect();
    let c = |from: Option<&str>, t: Option<&str>| Controls {
        start_from: from.map(str::to_string),
        start_time: t.map(str::to_string),
        ..Controls::default()
    };
    let name = |ctl: Controls| ctl.start_dir(&times).map(|d| d.name);
    assert_eq!(name(c(Some("firstTime"), None)).as_deref(), Some("0"));
    assert_eq!(name(c(Some("latestTime"), Some("0"))).as_deref(), Some("7200"));
    assert_eq!(name(c(Some("startTime"), Some("3600"))).as_deref(), Some("3600"));
    assert_eq!(name(c(None, Some("3.6e3"))).as_deref(), Some("3600"));
    assert_eq!(name(c(Some("startTime"), Some("100"))), None);
    assert_eq!(name(c(Some("startTime"), Some("$t0"))), None);
    assert_eq!(c(Some("startTime"), Some("100")).describe_start(), "startTime 100");
}

#[test]
fn a_missing_start_time_is_a_note() {
    let mut fs = case();
    let text = String::from_utf8(fs.read("system/controlDict", 1 << 20).unwrap().unwrap()).unwrap();
    fs.insert("system/controlDict", text.replace("startTime       0;", "startTime       100;"));
    let c = Case::open(&fs).unwrap();
    assert!(c.start.is_none());
    assert!(c.regions.iter().all(|r| r.fields.is_empty()));
    assert!(c.notes.iter().any(|n| n.message.contains("startTime 100")), "{:?}", c.notes);
}

#[test]
fn single_region_meshing_case() {
    let fs = MemFs::new()
        .with("system/controlDict", format!("{}application blockMesh;\n", header("dictionary", "controlDict")))
        .with("system/blockMeshDict", format!("{}vertices ();\n", header("dictionary", "blockMeshDict")))
        .with("constant/polyMesh/boundary", format!("{}{BOUNDARY}", header("polyBoundaryMesh", "boundary")));
    let c = Case::open(&fs).unwrap();
    assert_eq!(c.regions.len(), 1);
    assert_eq!((c.regions[0].name.as_str(), c.regions[0].kind), ("", RegionKind::Single));
    assert_eq!(c.regions[0].patches.as_ref().map(Vec::len), Some(2));
    assert!(c.dict("system/blockMeshDict").is_some());
    assert!(!c.notes.iter().any(|n| n.message.contains("start time")), "a meshing step needs no start time");
}

#[test]
fn hostile_and_large_files_are_notes_not_errors() {
    let mut fs = case();
    fs.insert("constant/air/polyMesh/boundary", "2\n(\n broken");
    fs.insert("0/air/k", "FoamFile { class volScalarField; }\ninternalField nonuniform List<scalar> 5(1 2);\n");
    fs.insert(
        "0/air/big",
        format!("{}internalField uniform 1;\n// {}\n", header("volScalarField", "big"), "x".repeat(4000)),
    );
    let options = OpenOptions { field_bytes: 2000, ..OpenOptions::default() };
    let c = Case::open_with(&fs, options).unwrap();
    let air = c.region("air").unwrap();
    assert!(air.patches.is_none());
    assert!(c.notes.iter().any(|n| n.path.ends_with("polyMesh/boundary")), "{:?}", c.notes);
    assert!(air.field("k").is_none() && c.notes.iter().any(|n| n.path == "0/air/k"));
    assert_eq!(air.field("big").map(|f| f.bcs.is_none()), Some(true));
    // A dictionary over the limit is listed in the notes, not loaded; controlDict always loads.
    let options = OpenOptions { dict_bytes: 100, ..OpenOptions::default() };
    let c = Case::open_with(&fs, options).unwrap();
    assert!(c.dict("system/controlDict").is_some() && c.controls.end_time.is_some());
    assert!(c.dict("system/air/fvSchemes").is_none());
    assert!(c.notes.iter().any(|n| n.path == "system/air/fvSchemes" && n.message.contains("too large")));
    // No controlDict: not a case.
    assert!(matches!(Case::open(&MemFs::new()), Err(Error::Layout(umcf_schema::Error::NotACase))));
}

#[test]
fn undo_after_save_is_a_change_to_save_again() {
    // Review finding: a snapshot taken before a save forgot the save: restoring it showed no
    // change while the disk kept the saved value.
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    let snapshot = c.clone();
    c.set(CONTROL_DICT, &["endTime"], "99").unwrap();
    c.save(&mut fs, None).unwrap();
    let mut c = snapshot;
    assert_eq!(c.changed(), [CONTROL_DICT]);
    assert_eq!(c.save(&mut fs, None).unwrap(), [CONTROL_DICT]);
    let text = String::from_utf8(fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap()).unwrap();
    assert!(text.contains("endTime         86400;"), "{text}");
}

#[test]
fn a_conflict_in_any_file_saves_nothing() {
    // Review finding: files before the conflicting one were already written.
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.set(CONTROL_DICT, &["endTime"], "5").unwrap();
    c.set("system/air/fvSolution", &["PIMPLE", "nOuterCorrectors"], "9").unwrap();
    let control = fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap();
    fs.insert("system/air/fvSolution", "changed by a script");
    assert!(matches!(c.save(&mut fs, Some(".umcfgui/backup/s")), Err(Error::Conflict { .. })));
    assert_eq!(fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap(), control, "controlDict not written");
    assert!(!fs.is_dir(".umcfgui"), "no backups either");
}

#[test]
fn backups_are_never_skipped() {
    // Review finding: a backup directory that is the case itself, or a backup already there
    // from an earlier session (or planted), silently skipped the backup.
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.set(CONTROL_DICT, &["endTime"], "5").unwrap();
    for bad in [".", "", "system", ".umcfgui/../system", ".umcfgui//x", ".umcfgui/./x"] {
        assert!(matches!(c.save(&mut fs, Some(bad)), Err(Error::BadBackup(_))), "{bad}");
    }
    let original = fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap();
    fs.insert(".umcfgui/b/system/controlDict", "an older, different backup");
    fs.insert(".umcfgui/b/system/controlDict.1", "x".repeat(original.len() + 10));
    c.save(&mut fs, Some(".umcfgui/b")).unwrap();
    assert_eq!(fs.read(".umcfgui/b/system/controlDict", 1 << 20).unwrap().unwrap(), b"an older, different backup");
    assert_eq!(fs.read(".umcfgui/b/system/controlDict.2", 1 << 20).unwrap().unwrap(), original);
}

#[test]
fn single_region_fields_have_plain_paths() {
    // Review finding: fields of a single-region case were looked up as `0//T`.
    let fs = MemFs::new()
        .with(
            "system/controlDict",
            format!("{}application simpleFoam;\nstartTime 0;\n", header("dictionary", "controlDict")),
        )
        .with("constant/polyMesh/boundary", format!("{}{BOUNDARY}", header("polyBoundaryMesh", "boundary")))
        .with(
            "0/T",
            format!(
                "{}internalField uniform 1;\nboundaryField {{ top {{ type zeroGradient; }} }}\n",
                header("volScalarField", "T")
            ),
        );
    let c = Case::open(&fs).unwrap();
    let t = c.regions[0].field("T").unwrap();
    assert_eq!(t.path, "0/T");
    assert_eq!(t.bcs.as_ref().map(Vec::len), Some(1));
}

#[test]
fn start_time_matches_like_openfoam() {
    let times: Vec<TimeDir> =
        ["0", "3600", "0.333333"].iter().map(|n| TimeDir { name: n.to_string(), value: n.parse().unwrap() }).collect();
    let at = |t: &str| Controls { start_time: Some(t.into()), ..Controls::default() }.start_dir(&times).map(|d| d.name);
    // More digits than the directory name has: OpenFOAM still finds `0.333333`.
    assert_eq!(at("0.3333333333").as_deref(), Some("0.333333"));
    assert_eq!(at("3600").as_deref(), Some("3600"));
    // The worst rounding to 6 digits: 3600.0049 is written as `3600`.
    assert_eq!(at("3600.0049").as_deref(), Some("3600"));
    assert_eq!(at("1e-12"), None, "not the `0` directory");
}

#[cfg(unix)]
#[test]
fn a_symlinked_dictionary_is_one_file() {
    // Review finding: `system/street/fvSolution -> ../air/fvSolution` loaded as two documents;
    // saving edits to both wrote the file twice and silently lost the first edit.
    use umcf_core::{CaseWrite, DirFs};
    let root = std::env::temp_dir().join(format!("umcf-model-alias-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut fs = DirFs::new(&root);
    let mem = case();
    for dir in ["system", "system/air", "system/street", "constant/air", "constant/air/polyMesh", "0/air", "0/street"] {
        for name in mem.list(dir).unwrap() {
            let rel = format!("{dir}/{name}");
            if let Some(b) = mem.read(&rel, 1 << 20).unwrap() {
                fs.write(&rel, &b).unwrap();
            }
        }
    }
    std::fs::remove_file(root.join("system/street/fvSolution")).unwrap();
    std::os::unix::fs::symlink("../air/fvSolution", root.join("system/street/fvSolution")).unwrap();

    // Review finding: a controlDict linked to a file listed before it became the alias,
    // and the case lost its controls.
    std::fs::rename(root.join("system/controlDict"), root.join("system/ControlBase")).unwrap();
    std::os::unix::fs::symlink("ControlBase", root.join("system/controlDict")).unwrap();
    // A hard-linked dictionary is noted.
    std::fs::hard_link(root.join("system/decomposeParDict"), root.join("system/decomposeParDict.hard")).unwrap();

    let mut c = Case::open(&fs).unwrap();
    assert_eq!(c.resolve("system/ControlBase"), CONTROL_DICT);
    assert_eq!(c.controls.end_time.as_deref(), Some("86400"));
    c.set("system/ControlBase", &["endTime"], "7").unwrap();
    assert_eq!(c.controls.end_time.as_deref(), Some("7"), "an edit through the alias refreshes the controls");
    assert!(c.notes.iter().any(|n| n.path == "system/decomposeParDict" && n.message.contains("hard-linked")));
    assert!(c.aliases().any(|(a, p)| a == "system/street/fvSolution" && p == "system/air/fvSolution"));
    assert_eq!(c.resolve("system/street/fvSolution"), "system/air/fvSolution");
    assert!(c.notes.iter().any(|n| n.path == "system/street/fvSolution" && n.message.contains("same file")));
    c.set("system/air/fvSolution", &["a"], "2").unwrap();
    c.set("system/street/fvSolution", &["b"], "2").unwrap();
    assert_eq!(c.changed(), ["system/air/fvSolution", CONTROL_DICT]);
    assert_eq!(c.save(&mut fs, None).unwrap(), ["system/air/fvSolution", CONTROL_DICT]);
    assert!(std::fs::read_to_string(root.join("system/ControlBase")).unwrap().contains("endTime         7;"));
    let text = std::fs::read_to_string(root.join("system/air/fvSolution")).unwrap();
    let saved = Document::parse(&text).unwrap();
    assert_eq!((saved.value(&["a"]), saved.value(&["b"])), (Some("2".into()), Some("2".into())), "{text}");
    assert!(std::fs::symlink_metadata(root.join("system/street/fvSolution")).unwrap().file_type().is_symlink());
    assert!(c.changed().is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_file_is_backed_up_once_per_session() {
    // Review finding: every save backed up the previous version again, and after 1000 saves
    // saving was refused.
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    let original = fs.read(CONTROL_DICT, 1 << 20).unwrap().unwrap();
    for end in ["1", "2", "3"] {
        c.set(CONTROL_DICT, &["endTime"], end).unwrap();
        c.save(&mut fs, Some(".umcfgui/backup/s")).unwrap();
    }
    assert_eq!(fs.read(".umcfgui/backup/s/system/controlDict", 1 << 20).unwrap().unwrap(), original);
    assert!(!fs.is_file(".umcfgui/backup/s/system/controlDict.1"));
    // A new session backs up again (what the previous one saved).
    c.set(CONTROL_DICT, &["endTime"], "4").unwrap();
    c.save(&mut fs, Some(".umcfgui/backup/t")).unwrap();
    let t = String::from_utf8(fs.read(".umcfgui/backup/t/system/controlDict", 1 << 20).unwrap().unwrap()).unwrap();
    assert!(t.contains("endTime         3;"), "{t}");
}

#[test]
fn an_empty_original_is_still_backed_up() {
    // Review finding: an empty file compared equal to any existing backup.
    let mut fs = case();
    fs.insert("system/air/fvConstraints", "");
    let mut c = Case::open(&fs).unwrap();
    c.set("system/air/fvConstraints", &["x"], "1").unwrap();
    fs.insert(".umcfgui/b/system/air/fvConstraints", "something else");
    c.save(&mut fs, Some(".umcfgui/b")).unwrap();
    assert_eq!(fs.read(".umcfgui/b/system/air/fvConstraints.1", 10).unwrap().as_deref(), Some(&b""[..]));
}

#[test]
fn start_time_takes_the_nearest_directory() {
    let times: Vec<TimeDir> =
        ["100000", "100000.4"].iter().map(|n| TimeDir { name: n.to_string(), value: n.parse().unwrap() }).collect();
    let c = Controls { start_time: Some("1.000004e5".into()), ..Controls::default() };
    assert_eq!(c.start_dir(&times).map(|d| d.name).as_deref(), Some("100000.4"));
}
