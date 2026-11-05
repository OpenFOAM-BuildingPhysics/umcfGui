use umcf_core::{CaseFs, CaseWrite, FsError, MemFs};

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
    assert_eq!(name(c(Some("startTime"), Some("3.6e3"))).as_deref(), Some("3600"));
    // Without startFrom, OpenFOAM resumes from the latest time (Time::setControls).
    assert_eq!(name(c(None, Some("0"))).as_deref(), Some("7200"));
    assert_eq!(name(c(Some("lastTime"), Some("0"))), None, "unknown startFrom");
    assert_eq!(c(None, None).describe_start(), "latestTime, the default");
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
    let at = |t: &str| {
        Controls { start_from: Some("startTime".into()), start_time: Some(t.into()), ..Controls::default() }
            .start_dir(&times)
            .map(|d| d.name)
    };
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
    let c =
        Controls { start_from: Some("startTime".into()), start_time: Some("1.000004e5".into()), ..Controls::default() };
    assert_eq!(c.start_dir(&times).map(|d| d.name).as_deref(), Some("100000.4"));
}

/// A case in memory that can fail reads of one path, and change a file during a save.
struct Faulty {
    inner: MemFs,
    unreadable: Option<String>,
    /// On the first write, this file gets this text (another program writing meanwhile).
    meanwhile: Option<(String, Vec<u8>)>,
    /// On the first write, this path becomes unreadable.
    breaks: Option<String>,
}

impl CaseFs for Faulty {
    fn read(&self, rel: &str, limit: u64) -> std::result::Result<Option<Vec<u8>>, FsError> {
        if self.unreadable.as_deref() == Some(rel) {
            return Err(FsError::Io { path: rel.into(), source: std::io::Error::other("permission denied") });
        }
        self.inner.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.inner.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.inner.is_dir(rel)
    }
    fn list(&self, rel: &str) -> std::result::Result<Vec<String>, FsError> {
        self.inner.list(rel)
    }
}

impl CaseWrite for Faulty {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> std::result::Result<(), FsError> {
        if let Some((path, text)) = self.meanwhile.take() {
            self.inner.insert(&path, text);
        }
        if let Some(path) = self.breaks.take() {
            self.unreadable = Some(path);
        }
        self.inner.write(rel, bytes)
    }
}

#[test]
fn an_unreadable_dictionary_is_a_note() {
    // Review finding: one unreadable file (another user's, mode 600) aborted the whole open.
    let fs = Faulty { inner: case(), unreadable: Some("system/air/fvSchemes".into()), meanwhile: None, breaks: None };
    let c = Case::open(&fs).unwrap();
    assert!(c.dict("system/air/fvSchemes").is_none());
    assert!(c.notes.iter().any(|n| n.path == "system/air/fvSchemes" && n.message.contains("can't be read")));
}

#[test]
fn each_file_is_checked_again_right_before_it_is_replaced() {
    // Review finding: a file changed between the first check and its write was overwritten.
    let mut fs = Faulty {
        inner: case(),
        unreadable: None,
        meanwhile: Some((CONTROL_DICT.into(), b"written by another program".to_vec())),
        breaks: None,
    };
    let mut c = Case::open(&fs).unwrap();
    c.set("system/air/fvSolution", &["a"], "1").unwrap();
    c.set(CONTROL_DICT, &["endTime"], "5").unwrap();
    let e = c.save(&mut fs, None).unwrap_err();
    assert!(
        matches!(&e, Error::Partial { saved, source } if saved == &["system/air/fvSolution"] && matches!(**source, Error::Conflict { .. })),
        "{e}"
    );
    assert_eq!(fs.inner.read(CONTROL_DICT, 100).unwrap().unwrap(), b"written by another program");
    assert_eq!(c.changed(), [CONTROL_DICT], "the saved file is no longer a change");
}

#[test]
fn an_oversized_boundary_is_a_note() {
    // Review finding: `boundary` was read with the mesh limit (16 GiB), also after gunzip.
    use std::io::Write;
    let options = OpenOptions { dict_bytes: 4000, ..OpenOptions::default() };
    let mut fs = case();
    fs.insert(
        "constant/air/polyMesh/boundary",
        format!("{}{BOUNDARY}// {}\n", header("polyBoundaryMesh", "boundary"), "x".repeat(5000)),
    );
    let c = Case::open_with(&fs, options).unwrap();
    assert!(c.region("air").unwrap().patches.is_none());
    assert!(c.notes.iter().any(|n| n.path.ends_with("polyMesh/boundary")), "{:?}", c.notes);
    let over_limit =
        |c: &Case| c.notes.iter().any(|n| n.path.ends_with("polyMesh/boundary") && n.message.contains("limit"));
    assert!(over_limit(&c), "{:?}", c.notes);
    // A valid boundary that is small gzipped and too large unpacked: only the unpacked-size
    // cap rejects it (it parses fine otherwise).
    let text = format!("{}{BOUNDARY}// {}\n", header("polyBoundaryMesh", "boundary"), " ".repeat(1 << 20));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(text.as_bytes()).unwrap();
    let packed = gz.finish().unwrap();
    assert!(packed.len() < 4000);
    let base = case();
    let mut fs = MemFs::new();
    for rel in ["system/controlDict", "constant/air/g"] {
        fs.insert(rel, base.read(rel, 1 << 20).unwrap().unwrap());
    }
    fs.insert("constant/air/polyMesh/boundary.gz", packed.clone());
    let roomy = OpenOptions { dict_bytes: 2 << 20, ..options };
    assert_eq!(Case::open_with(&fs, roomy).unwrap().region("air").unwrap().patches.as_ref().map(Vec::len), Some(2));
    let c = Case::open_with(&fs, options).unwrap();
    assert!(c.region("air").unwrap().patches.is_none());
    assert!(over_limit(&c), "{:?}", c.notes);
}

#[test]
fn a_read_error_during_save_is_reported_as_itself() {
    // Review finding: a read error in the re-check was reported as "changed on disk".
    let mut fs = Faulty { inner: case(), unreadable: None, meanwhile: None, breaks: Some(CONTROL_DICT.into()) };
    let mut c = Case::open(&fs).unwrap();
    c.set("system/air/fvSolution", &["a"], "1").unwrap();
    c.set(CONTROL_DICT, &["endTime"], "5").unwrap();
    let e = c.save(&mut fs, None).unwrap_err();
    assert!(matches!(&e, Error::Partial { source, .. } if matches!(**source, Error::Fs(FsError::Io { .. }))), "{e}");
}

#[test]
fn a_file_grown_past_the_limit_is_a_conflict() {
    // Review finding: a file that grew past the read limit during the save was reported as a
    // read error, not as changed.
    let options = OpenOptions { dict_bytes: 2000, ..OpenOptions::default() };
    let big = vec![b' '; 5000];
    // Grown before the save starts: the first check.
    let mut fs = Faulty { inner: case(), unreadable: None, meanwhile: None, breaks: None };
    let mut c = Case::open_with(&fs, options).unwrap();
    c.set("system/air/fvSolution", &["a"], "1").unwrap();
    fs.inner.insert("system/air/fvSolution", big.clone());
    assert!(matches!(c.save(&mut fs, None), Err(Error::Conflict { .. })));
    // Grown during the save: the check right before the write.
    let mut fs = Faulty { inner: case(), unreadable: None, meanwhile: Some((CONTROL_DICT.into(), big)), breaks: None };
    let mut c = Case::open_with(&fs, options).unwrap();
    c.set("system/air/fvSolution", &["a"], "1").unwrap();
    c.set(CONTROL_DICT, &["endTime"], "5").unwrap();
    let e = c.save(&mut fs, None).unwrap_err();
    assert!(matches!(&e, Error::Partial { source, .. } if matches!(**source, Error::Conflict { .. })), "{e}");
}

#[test]
fn boundary_fields_with_includes_are_marked_indirect() {
    let mut fs = case();
    fs.insert(
        "0/air/U",
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    #includeEtc \"caseDicts/setConstraintTypes\"\n    ground {{ type noSlip; }}\n}}\n",
            header("volVectorField", "U")
        ),
    );
    let c = Case::open(&fs).unwrap();
    let air = c.region("air").unwrap();
    assert!(air.field("U").unwrap().indirect);
    assert!(!air.field("T").unwrap().indirect);
}

#[test]
fn files_put_by_commands_are_saved_with_checks_and_backups() {
    let mut fs = case().with("constant/triSurface/old.stl", "solid old\nendsolid old\n");
    let mut c = Case::open(&fs).unwrap();
    let before = c.clone();
    c.put_file(&fs, "constant/triSurface/new.stl", b"solid new\nendsolid new\n".to_vec()).unwrap();
    c.put_file(&fs, "constant/triSurface/old.stl", b"solid fixed\nendsolid fixed\n".to_vec()).unwrap();
    assert_eq!(c.changed(), ["constant/triSurface/new.stl", "constant/triSurface/old.stl"]);
    assert!(before.changed().is_empty(), "a clone from before is a snapshot");
    assert_eq!(
        c.file(&fs, "constant/triSurface/old.stl").unwrap().unwrap().as_slice(),
        b"solid fixed\nendsolid fixed\n"
    );
    assert_eq!(
        before.file(&fs, "constant/triSurface/old.stl").unwrap().unwrap().as_slice(),
        b"solid old\nendsolid old\n"
    );
    assert!(c.file(&fs, "constant/triSurface/none.stl").unwrap().is_none());

    let saved = c.save(&mut fs, Some(".umcfgui/backup/s1")).unwrap();
    assert_eq!(saved, ["constant/triSurface/new.stl", "constant/triSurface/old.stl"]);
    assert_eq!(fs.read("constant/triSurface/new.stl", 100).unwrap().unwrap(), b"solid new\nendsolid new\n");
    assert_eq!(
        fs.read(".umcfgui/backup/s1/constant/triSurface/old.stl", 100).unwrap().unwrap(),
        b"solid old\nendsolid old\n"
    );
    assert!(!fs.is_file(".umcfgui/backup/s1/constant/triSurface/new.stl"), "nothing to back up for a new file");
    assert!(c.changed().is_empty());

    // Changed on disk since: a conflict, nothing written.
    c.put_file(&fs, "constant/triSurface/new.stl", b"solid again\nendsolid\n".to_vec()).unwrap();
    fs.insert("constant/triSurface/new.stl", "edited elsewhere");
    assert!(matches!(c.save(&mut fs, None), Err(Error::Conflict { .. })));
    assert_eq!(fs.read("constant/triSurface/new.stl", 100).unwrap().unwrap(), b"edited elsewhere");

    // A new file that appeared on disk in the meantime is a conflict too.
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.put_file(&fs, "constant/triSurface/a.stl", b"x".to_vec()).unwrap();
    fs.insert("constant/triSurface/a.stl", "someone else's");
    assert!(matches!(c.save(&mut fs, None), Err(Error::Conflict { .. })));

    // Not a place for a file, or a dictionary.
    for bad in ["", "/etc/passwd", "../x.stl", "constant//x", "constant/./x", ".umcfgui/x", "a\\b"] {
        assert!(matches!(c.put_file(&fs, bad, Vec::new()), Err(Error::BadFile(_))), "{bad}");
    }
    assert!(matches!(c.put_file(&fs, "system/controlDict", Vec::new()), Err(Error::IsDict(_))));
}

#[test]
fn a_file_changed_on_disk_after_undo_is_the_new_base() {
    let mut fs = case().with("constant/triSurface/box.stl", "v1");
    let opened = Case::open(&fs).unwrap();
    let mut c = opened.clone();
    c.put_file(&fs, "constant/triSurface/box.stl", b"v1 repaired".to_vec()).unwrap();
    // Undo, then the file is exported again outside.
    let mut c = opened.clone();
    fs.insert("constant/triSurface/box.stl", "v2");
    let read = c.file(&fs, "constant/triSurface/box.stl").unwrap().unwrap();
    assert_eq!(read.as_slice(), b"v2");
    c.put_file(&fs, "constant/triSurface/box.stl", b"v2 repaired".to_vec()).unwrap();
    assert_eq!(c.save(&mut fs, None).unwrap(), ["constant/triSurface/box.stl"]);
    assert_eq!(fs.read("constant/triSurface/box.stl", 100).unwrap().unwrap(), b"v2 repaired");
    // A version made from v1 that comes back (redo) can't overwrite what changed outside.
    let mut stale = opened.clone();
    stale
        .put_file(&MemFs::new().with("constant/triSurface/box.stl", "v1"), "constant/triSurface/box.stl", b"x".to_vec())
        .unwrap();
    fs.insert("constant/triSurface/box.stl", "v3");
    assert!(matches!(stale.save(&mut fs, None), Err(Error::Conflict { .. })));
}

#[test]
fn putting_back_the_first_bytes_over_a_newer_file_is_a_change() {
    let mut fs = case().with("constant/triSurface/box.stl", "O");
    let opened = Case::open(&fs).unwrap();
    let mut c = opened.clone();
    c.put_file(&fs, "constant/triSurface/box.stl", b"repaired".to_vec()).unwrap();
    // Undo; the file is rewritten outside; the original comes back in.
    let mut c2 = opened.clone();
    fs.insert("constant/triSurface/box.stl", "O2");
    c2.put_file(&fs, "constant/triSurface/box.stl", b"O".to_vec()).unwrap();
    assert_eq!(c2.changed(), ["constant/triSurface/box.stl"]);
    c2.save(&mut fs, None).unwrap();
    assert_eq!(fs.read("constant/triSurface/box.stl", 10).unwrap().unwrap(), b"O");
    assert!(c2.changed().is_empty());
    // Putting the same bytes as on disk is no change.
    let mut c3 = Case::open(&fs).unwrap();
    c3.put_file(&fs, "constant/triSurface/box.stl", b"O".to_vec()).unwrap();
    assert!(c3.changed().is_empty());
    drop(c);
}

#[test]
fn undo_after_saving_a_surface_is_a_change_to_save_again() {
    let mut fs = case().with("constant/triSurface/box.stl", "O");
    let mut c = Case::open(&fs).unwrap();
    c.put_file(&fs, "constant/triSurface/box.stl", b"O".to_vec()).unwrap();
    let before = c.clone();
    c.put_file(&fs, "constant/triSurface/box.stl", b"scaled".to_vec()).unwrap();
    c.save(&mut fs, None).unwrap();
    // Undo: the case holds O again, the disk the scaled file.
    let mut c = before;
    assert_eq!(c.changed(), ["constant/triSurface/box.stl"]);
    c.save(&mut fs, None).unwrap();
    assert_eq!(fs.read("constant/triSurface/box.stl", 10).unwrap().unwrap(), b"O");
}

#[test]
fn a_boundary_condition_changes_its_entry_only() {
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    let before = fs.read("0/air/T", 1 << 20).unwrap().unwrap();
    c.set_bc(
        &fs,
        "air",
        "T",
        &["top", "inlet"],
        &Condition { kind: "fixedValue", entries: &[("value", "uniform 290")], defaults: &[] },
    )
    .unwrap();
    let bcs = c.region("air").unwrap().field("T").unwrap().bcs.clone().unwrap();
    let summary: Vec<_> = bcs.iter().map(|b| (b.patch.as_str(), b.kind.as_deref(), b.value.as_deref())).collect();
    assert_eq!(
        summary,
        [
            ("ground", Some("fixedValue"), Some("nonuniform List<scalar> 4")),
            ("top", Some("fixedValue"), Some("uniform 290")),
            ("(wall|roof).*", Some("calculated"), Some("$internalField")),
            ("inlet", Some("fixedValue"), Some("uniform 290")),
        ]
    );
    assert_eq!(c.changed(), ["0/air/T"]);
    let after = c.file(&fs, "0/air/T").unwrap().unwrap();
    let (before, after) = (String::from_utf8(before).unwrap(), String::from_utf8(after.to_vec()).unwrap());
    let head = before.split("    top").next().unwrap();
    assert!(after.starts_with(head), "{after}");
    assert!(after.contains("\"(wall|roof).*\" { type calculated; value $internalField; }"), "{after}");
    // Saved like any file a command wrote.
    assert_eq!(c.save(&mut fs, None).unwrap(), ["0/air/T"]);
    assert_eq!(fs.read("0/air/T", 1 << 20).unwrap().unwrap(), after.as_bytes());
    // What can't be edited says why.
    let e = |r: Result<BcReport>| r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        e(c.set_bc(&fs, "air", "U", &["top"], &Condition { kind: "fixedValue", entries: &[], defaults: &[] }))
            .contains("no field U of region air")
    );
    assert!(
        e(c.set_bc(&fs, "nowhere", "T", &["top"], &Condition { kind: "fixedValue", entries: &[], defaults: &[] }))
            .contains("no region")
    );
}

#[test]
fn a_boundary_field_from_elsewhere_is_not_edited() {
    let fs = case()
        .with("0/street/T", format!("{}internalField uniform 290;\n#include \"bcs\"\n", header("volScalarField", "T")));
    let mut c = Case::open(&fs).unwrap();
    let e = c
        .set_bc(&fs, "street", "T", &["top"], &Condition { kind: "zeroGradient", entries: &[], defaults: &[] })
        .unwrap_err()
        .to_string();
    assert!(e.contains("comes from elsewhere"), "{e}");
    assert!(c.changed().is_empty());
}

#[test]
fn the_same_type_keeps_its_entry() {
    let fs = case().with(
        "0/air/U",
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    ground\n    {{\n        type            atmBoundaryLayerInletVelocity;\n        #include        \"include/ABLConditions\"\n        value           uniform (1 0 0); // from the profile\n    }}\n}}\n",
            header("volVectorField", "U")
        ),
    );
    let mut c = Case::open(&fs).unwrap();
    c.set_bc(
        &fs,
        "air",
        "U",
        &["ground"],
        &Condition { kind: "atmBoundaryLayerInletVelocity", entries: &[("value", "uniform (2 0 0)")], defaults: &[] },
    )
    .unwrap();
    let text = String::from_utf8(c.file(&fs, "0/air/U").unwrap().unwrap().to_vec()).unwrap();
    assert!(
        text.contains("        type            atmBoundaryLayerInletVelocity;\n        #include        \"include/ABLConditions\"\n        value           uniform (2 0 0); // from the profile\n"),
        "{text}"
    );
    // Another type starts afresh, and says what it dropped.
    let dropped = c
        .set_bc(
            &fs,
            "air",
            "U",
            &["ground"],
            &Condition { kind: "fixedValue", entries: &[("value", "uniform (0 0 0)")], defaults: &[] },
        )
        .unwrap();
    assert_eq!(dropped.replaced, ["ground"]);
    let text = String::from_utf8(c.file(&fs, "0/air/U").unwrap().unwrap().to_vec()).unwrap();
    assert!(!text.contains("#include") && text.contains("type            fixedValue;"), "{text}");
}

#[test]
fn undo_past_a_saved_condition_writes_the_original_back() {
    let mut fs = case();
    let original = fs.read("0/air/T", 1 << 20).unwrap().unwrap();
    let before = Case::open(&fs).unwrap();
    let mut c = before.clone();
    c.set_bc(
        &fs,
        "air",
        "T",
        &["top"],
        &Condition { kind: "fixedValue", entries: &[("value", "uniform 1")], defaults: &[] },
    )
    .unwrap();
    assert_eq!(c.save(&mut fs, None).unwrap(), ["0/air/T"]);
    // The case from before the edit (what an undo puts back) is not what is on disk now.
    let mut undone = before;
    assert_eq!(undone.changed(), ["0/air/T"]);
    assert_eq!(undone.file(&fs, "0/air/T").unwrap().unwrap().as_slice(), original.as_slice());
    assert_eq!(undone.save(&mut fs, None).unwrap(), ["0/air/T"]);
    assert_eq!(fs.read("0/air/T", 1 << 20).unwrap().unwrap(), original);
    assert!(undone.changed().is_empty());
    // And the edited case is now the one with a change to save.
    assert_eq!(c.changed(), ["0/air/T"]);
    // An edit from the undone case starts from the original, not from what was saved.
    undone.set_bc(&fs, "air", "T", &["top"], &Condition { kind: "zeroGradient", entries: &[], defaults: &[] }).unwrap();
    let text = String::from_utf8(undone.file(&fs, "0/air/T").unwrap().unwrap().to_vec()).unwrap();
    assert!(!text.contains("uniform 1"), "{text}");
}

#[test]
fn an_entry_from_elsewhere_is_kept_and_placeholders_are_refused() {
    let fs = case().with(
        "0/air/U",
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    ground\n    {{\n        $inletCommon\n    }}\n    top {{ type fixedValue; value nonuniform List<vector> 2((1 0 0) (2 0 0)); }}\n}}\n",
            header("volVectorField", "U")
        ),
    );
    let mut c = Case::open(&fs).unwrap();
    c.set_bc(
        &fs,
        "air",
        "U",
        &["ground"],
        &Condition { kind: "fixedValue", entries: &[("value", "uniform (1 0 0)")], defaults: &[] },
    )
    .unwrap();
    let text = String::from_utf8(c.file(&fs, "0/air/U").unwrap().unwrap().to_vec()).unwrap();
    assert!(text.contains("$inletCommon") && text.contains("type") && text.contains("uniform (1 0 0)"), "{text}");
    // Such an entry gets no defaults: what it brings in may hold them.
    let fs = case().with(
        "0/air/U",
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    ground\n    {{\n        $inletCommon\n        value uniform (1 0 0);\n    }}\n}}\n",
            header("volVectorField", "U")
        ),
    );
    let mut c = Case::open(&fs).unwrap();
    let report = c
        .set_bc(
            &fs,
            "air",
            "U",
            &["ground"],
            &Condition {
                kind: "inletOutlet",
                entries: &[],
                defaults: &[("inletValue", "uniform (0 0 0)"), ("value", "uniform (9 9 9)")],
            },
        )
        .unwrap();
    assert_eq!(report.unfilled, ["ground"]);
    // The same with a type of its own: kept, and reported too.
    let fs2 = case().with(
        "0/air/U",
        format!(
            "{}internalField uniform (0 0 0);\nboundaryField\n{{\n    ground\n    {{\n        type fixedValue;\n        #include \"ground\"\n    }}\n}}\n",
            header("volVectorField", "U")
        ),
    );
    let mut c2 = Case::open(&fs2).unwrap();
    let report2 = c2
        .set_bc(
            &fs2,
            "air",
            "U",
            &["ground"],
            &Condition { kind: "fixedValue", entries: &[], defaults: &[("value", "uniform (1 0 0)")] },
        )
        .unwrap();
    assert_eq!(report2.unfilled, ["ground"]);
    // Every default given as an entry too: nothing missing.
    let report3 = c2
        .set_bc(
            &fs2,
            "air",
            "U",
            &["ground"],
            &Condition {
                kind: "fixedValue",
                entries: &[("value", "uniform (2 0 0)")],
                defaults: &[("value", "uniform (1 0 0)")],
            },
        )
        .unwrap();
    assert!(report3.unfilled.is_empty());
    let text = String::from_utf8(c.file(&fs, "0/air/U").unwrap().unwrap().to_vec()).unwrap();
    assert!(!text.contains("inletValue") && !text.contains("uniform (9 9 9)"), "{text}");
    assert!(text.contains("$inletCommon") && text.contains("inletOutlet"), "{text}");
    for (kind, k) in [("__umcfListA0", "value"), ("fixedValue", "__umcfListA0")] {
        let e = c
            .set_bc(&fs, "air", "U", &["top"], &Condition { kind, entries: &[(k, "uniform (0 0 0)")], defaults: &[] })
            .unwrap_err()
            .to_string();
        assert!(e.contains("can't be written"), "{e}");
    }
}

#[test]
fn an_outside_change_after_the_undone_original_is_saved_back_is_seen() {
    let mut fs = case();
    let before = Case::open(&fs).unwrap();
    let mut c = before.clone();
    c.set_bc(
        &fs,
        "air",
        "T",
        &["top"],
        &Condition { kind: "fixedValue", entries: &[("value", "uniform 1")], defaults: &[] },
    )
    .unwrap();
    c.save(&mut fs, None).unwrap();
    let mut undone = before;
    assert_eq!(undone.save(&mut fs, None).unwrap(), ["0/air/T"]);
    // The disk has the original again: a change made outside now is what the case sees.
    let outside =
        String::from_utf8(fs.read("0/air/T", 1 << 20).unwrap().unwrap()).unwrap().replace("uniform 300", "uniform 299");
    fs.insert("0/air/T", outside.clone());
    assert!(undone.changed().is_empty());
    assert_eq!(undone.file(&fs, "0/air/T").unwrap().unwrap().as_slice(), outside.as_bytes());
    undone.set_bc(&fs, "air", "T", &["top"], &Condition { kind: "zeroGradient", entries: &[], defaults: &[] }).unwrap();
    assert_eq!(undone.save(&mut fs, None).unwrap(), ["0/air/T"]);
    assert!(String::from_utf8(fs.read("0/air/T", 1 << 20).unwrap().unwrap()).unwrap().contains("uniform 299"));
}

#[test]
fn an_undo_never_writes_an_original_older_than_an_outside_change() {
    let mut fs = case();
    let s0 = Case::open(&fs).unwrap();
    let mut a = s0.clone();
    a.set_bc(
        &fs,
        "air",
        "T",
        &["top"],
        &Condition { kind: "fixedValue", entries: &[("value", "uniform 1")], defaults: &[] },
    )
    .unwrap();
    a.save(&mut fs, None).unwrap();
    let mut undone = s0.clone();
    undone.save(&mut fs, None).unwrap();
    // Changed outside, then edited and saved on top of that.
    let outside =
        String::from_utf8(fs.read("0/air/T", 1 << 20).unwrap().unwrap()).unwrap().replace("uniform 300", "uniform 299");
    fs.insert("0/air/T", outside.clone());
    let before_b = undone.clone();
    undone.set_bc(&fs, "air", "T", &["top"], &Condition { kind: "zeroGradient", entries: &[], defaults: &[] }).unwrap();
    undone.save(&mut fs, None).unwrap();
    // Undoing B gives back the outside version, not the first original.
    let mut back = before_b;
    assert_eq!(back.file(&fs, "0/air/T").unwrap().unwrap().as_slice(), outside.as_bytes());
    back.save(&mut fs, None).unwrap();
    assert_eq!(fs.read("0/air/T", 1 << 20).unwrap().unwrap(), outside.as_bytes());
}

#[test]
fn a_patch_is_retyped_regrouped_and_renamed_everywhere_it_is_named() {
    let street = "1\n(\n    street_to_air\n    {\n        type            mappedWall;\n        inGroups        List<word> 1(wall);\n        nFaces          4;\n        startFace       10;\n        neighbourRegion air;\n        neighbourPatch  ground;\n    }\n)\n";
    let fs =
        case().with("constant/street/polyMesh/boundary", format!("{}{street}", header("polyBoundaryMesh", "boundary")));
    let mut c = Case::open(&fs).unwrap();
    let before = String::from_utf8(fs.read("constant/air/polyMesh/boundary", 1 << 20).unwrap().unwrap()).unwrap();
    // Type and groups.
    let report = c
        .set_patch(
            &fs,
            "air",
            "top",
            &PatchChange { kind: Some("symmetry"), groups: Some(&["sky"]), ..Default::default() },
        )
        .unwrap();
    assert_eq!(report, PatchReport::default());
    let top = c.region("air").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == "top").cloned().unwrap();
    assert_eq!((top.kind.as_str(), top.groups.as_slice()), ("symmetry", &["sky".to_string()][..]));
    let after = String::from_utf8(c.file(&fs, "constant/air/polyMesh/boundary").unwrap().unwrap().to_vec()).unwrap();
    let head = before.split("    top").next().unwrap();
    assert!(after.starts_with(head), "{after}");
    // A new name, in the mesh, the field files and the street's mapped patch.
    let report =
        c.set_patch(&fs, "air", "ground", &PatchChange { name: Some("terrain"), ..Default::default() }).unwrap();
    assert_eq!(report.fields, ["0/air/T"]);
    assert_eq!(report.partners, ["street/street_to_air"]);
    let air = c.region("air").unwrap();
    assert!(air.patches.as_ref().unwrap().iter().any(|p| p.name == "terrain"));
    assert!(air.field("T").unwrap().bcs.as_ref().unwrap().iter().any(|b| b.patch == "terrain"));
    let st = c.region("street").unwrap().patches.as_ref().unwrap().first().cloned().unwrap();
    assert!(st.params.iter().any(|(k, v)| k == "neighbourPatch" && v == "terrain"), "{:?}", st.params);
    let mut changed = c.changed();
    changed.sort();
    assert_eq!(changed, ["0/air/T", "constant/air/polyMesh/boundary", "constant/street/polyMesh/boundary"]);
    // What can't be done says why.
    let e = |r: Result<PatchReport>| r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        e(c.set_patch(&fs, "air", "top", &PatchChange { name: Some("terrain"), ..Default::default() }))
            .contains("already")
    );
    assert!(e(c.set_patch(&fs, "air", "nowhere", &PatchChange::default())).contains("no patch"));
    assert!(
        e(c.set_patch(
            &fs,
            "street",
            "street_to_air",
            &PatchChange { kind: Some("__umcfListA0"), ..Default::default() }
        ))
        .contains("can't be written")
    );
}

#[test]
fn renames_keep_cyclic_partners_gzip_and_refuse_taken_names() {
    use std::io::Write;
    let b = "3\n(\n    left\n    {\n        type            cyclic;\n        inGroups        List<word> 1(periodic);\n        nFaces          1;\n        startFace       10;\n        neighbourPatch  right;\n    }\n    right\n    {\n        type            cyclic;\n        nFaces          1;\n        startFace       11;\n        neighbourPatch  left;\n    }\n    top\n    {\n        type            patch;\n        nFaces          2;\n        startFace       12;\n    }\n)\n";
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(format!("{}{b}", header("polyBoundaryMesh", "boundary")).as_bytes()).unwrap();
    let fs = MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with("constant/air/polyMesh/boundary.gz", gz.finish().unwrap())
        .with(
            "0/air/T",
            format!(
                "{}internalField uniform 300;\nboundaryField\n{{\n    left {{ type cyclic; }}\n    right {{ type cyclic; }}\n    top {{ type zeroGradient; }}\n    leftover {{ type zeroGradient; }}\n}}\n",
                header("volScalarField", "T")
            ),
        );
    let mut c = Case::open(&fs).unwrap();
    assert_eq!(c.region("air").unwrap().patches.as_ref().unwrap().len(), 3);
    // The partner in the same region follows; the gzip file stays gzip.
    let report = c.set_patch(&fs, "air", "left", &PatchChange { name: Some("west"), ..Default::default() }).unwrap();
    assert_eq!(report.partners, ["air/right"]);
    let bytes = c.file(&fs, "constant/air/polyMesh/boundary.gz").unwrap().unwrap();
    assert!(bytes.starts_with(&[0x1f, 0x8b]));
    let right = c.region("air").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == "right").cloned().unwrap();
    assert!(right.params.iter().any(|(k, v)| k == "neighbourPatch" && v == "west"), "{:?}", right.params);
    // Names already used by a group or a field entry are refused.
    let e = |r: Result<PatchReport>| r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        e(c.set_patch(&fs, "air", "top", &PatchChange { name: Some("periodic"), ..Default::default() }))
            .contains("group")
    );
    assert!(
        e(c.set_patch(&fs, "air", "top", &PatchChange { name: Some("leftover"), ..Default::default() }))
            .contains("entry `leftover`")
    );
    assert!(
        e(c.set_patch(&fs, "air", "top", &PatchChange { name: Some("42"), ..Default::default() })).contains("number")
    );
    // A non-conformal patch's original patch follows in its own region only.
    let nc = "2\n(\n    wall_orig\n    {\n        type wall;\n        nFaces 1;\n        startFace 10;\n    }\n    nc_wall\n    {\n        type nonConformalMappedWall;\n        nFaces 0;\n        startFace 11;\n        neighbourRegion air;\n        neighbourPatch top;\n        originalPatch wall_orig;\n    }\n)\n";
    let fs = fs.with("constant/street/polyMesh/boundary", format!("{}{nc}", header("polyBoundaryMesh", "boundary")));
    let mut c = Case::open(&fs).unwrap();
    let report =
        c.set_patch(&fs, "street", "wall_orig", &PatchChange { name: Some("facade"), ..Default::default() }).unwrap();
    assert_eq!(report.partners, ["street/nc_wall"]);
    let ncw =
        c.region("street").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == "nc_wall").cloned().unwrap();
    assert!(ncw.params.contains(&("originalPatch".into(), "facade".into())), "{:?}", ncw.params);
    assert!(ncw.params.contains(&("neighbourPatch".into(), "top".into())), "{:?}", ncw.params);
}

#[test]
fn a_new_patch_is_prepared_for_create_patch() {
    let fs = case().with(
        "system/air/createPatchDict",
        format!(
            "{}// the case's own, run by Allprepare\npointSync false;\npatches ();\n",
            header("dictionary", "createPatchDict")
        ),
    );
    let mut c = Case::open(&fs).unwrap();
    let new = |name, faces| NewPatch { name, kind: "patch", groups: &["roofs"], entries: &[], faces };
    // One face of `top`: its entry in T is copied for the new patch.
    let report = c.create_patch(&fs, "air", &new("roof", &[15, 15])).unwrap();
    assert_eq!(report.set, "constant/air/polyMesh/sets/roof");
    assert_eq!((report.dict.as_str(), report.new_dict), ("system/air/createPatchDict.umcf", true));
    assert_eq!(report.from, [("top".to_string(), 1)]);
    assert!(report.emptied.is_empty() && report.resized.is_empty() && report.unfilled.is_empty());
    assert_eq!(report.fields, ["0/air/T"]);
    let set = String::from_utf8(c.file(&fs, "constant/air/polyMesh/sets/roof").unwrap().unwrap().to_vec()).unwrap();
    assert!(set.contains("class       faceSet;") && set.ends_with("}\n\n1\n(\n15\n)\n"), "{set}");
    let t = String::from_utf8(c.file(&fs, "0/air/T").unwrap().unwrap().to_vec()).unwrap();
    assert!(t.contains("    top { type zeroGradient; }\n    roof { type zeroGradient; }\n    \"(wall"), "{t}");
    assert!(c.region("air").unwrap().field("T").unwrap().bcs.as_ref().unwrap().iter().any(|b| b.patch == "roof"));
    // The mesh is unchanged until createPatch has run, and so is the case's own dictionary.
    assert!(!c.region("air").unwrap().patches.as_ref().unwrap().iter().any(|p| p.name == "roof"));
    assert!(!c.changed().contains(&"system/air/createPatchDict".to_string()));

    // All of `ground`, whose T entry holds a list: nothing to copy, and T must be made again.
    let report = c.create_patch(&fs, "air", &new("terrain", &[10, 11, 12, 13])).unwrap();
    assert!(!report.new_dict);
    assert_eq!(report.emptied, ["ground"]);
    // T's pattern entry may cover it.
    assert_eq!(
        (report.resized.as_slice(), report.maybe.as_slice()),
        (&["0/air/T".to_string()][..], &["0/air/T".to_string()][..])
    );
    assert!(report.unfilled.is_empty());
    assert!(report.fields.is_empty());
    let dict = c.dict("system/air/createPatchDict.umcf").unwrap().to_string();
    let again = Document::parse(&dict).unwrap();
    assert!(again.root.problems().is_empty(), "{dict}");
    assert!(dict.contains("createPatch -region air -dict system/air/createPatchDict.umcf -overwrite"), "{dict}");
    assert!(dict.contains("// prepared on a mesh of 16 faces\n"), "{dict}");
    assert_eq!(again.value(&["pointSync"]).as_deref(), Some("false"));
    let Some([umcf_foam_dict::Node::List { items, .. }]) = again.get(&["patches"]).map(|e| e.value.as_slice()) else {
        panic!("{dict}")
    };
    assert_eq!(items.len(), 2);
    assert!(dict.contains("        name            terrain;\n        patchInfo\n        {\n            type            patch;\n            inGroups        List<word> 1(roofs);\n        }\n        constructFrom   set;\n        set             terrain;\n"), "{dict}");

    let mut changed = c.changed();
    changed.sort();
    assert_eq!(
        changed,
        [
            "0/air/T",
            "constant/air/polyMesh/sets/roof",
            "constant/air/polyMesh/sets/terrain",
            "system/air/createPatchDict.umcf"
        ]
    );

    // What can't be done says why.
    let e = |r: Result<CreateReport>| r.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e(c.create_patch(&fs, "air", &new("roof2", &[9]))).contains("not a boundary face"));
    assert!(e(c.create_patch(&fs, "air", &new("roof2", &[]))).contains("needs faces"));
    assert!(e(c.create_patch(&fs, "air", &new("top", &[14]))).contains("already"));
    assert!(e(c.create_patch(&fs, "nowhere", &new("x", &[14]))).contains("no region"));
    // Faces already prepared for another patch, and a name prepared already.
    assert!(e(c.create_patch(&fs, "air", &new("x", &[14, 15]))).contains("face 15 is in `roof`"));
    assert!(e(c.create_patch(&fs, "air", &new("terrain", &[14]))).contains("already"));
    assert!(e(c.create_patch(&fs, "air", &NewPatch { kind: "cyclic", ..new("x", &[14]) })).contains("paired"));
    let fs2 = case().with("constant/air/polyMesh/sets/x", "1(14)");
    let mut c2 = Case::open(&fs2).unwrap();
    assert!(e(c2.create_patch(&fs2, "air", &new("x", &[14]))).contains("is there already"));

    // Saving writes the new files, and a case opened again sees the prepared faces.
    let mut fs = fs;
    c.save(&mut fs, None).unwrap();
    assert!(fs.is_file("system/air/createPatchDict.umcf") && fs.is_file("constant/air/polyMesh/sets/terrain"));
    let mut c = Case::open(&fs).unwrap();
    assert!(e(c.create_patch(&fs, "air", &new("x", &[12]))).contains("face 12 is in `terrain`"));
    // The last face of `top`: `top` goes, with the face prepared before.
    let report = c.create_patch(&fs, "air", &new("x", &[14])).unwrap();
    assert_eq!(report.emptied, ["top"]);
    // Prepared on another mesh: stopped.
    let other = fs.clone().with(
        "constant/air/polyMesh/boundary",
        format!(
            "{}{}",
            header("polyBoundaryMesh", "boundary"),
            BOUNDARY.replace("nFaces          2;", "nFaces          3;")
        ),
    );
    let mut c4 = Case::open(&other).unwrap();
    assert!(
        e(c4.create_patch(&other, "air", &new("y", &[16])))
            .contains("prepared on a mesh of 16 faces, and this one has 17")
    );
    // A dictionary that has run already (the mesh has its patch) stops it too.
    let ran = fs.clone().with(
        "constant/air/polyMesh/boundary",
        format!("{}{}", header("polyBoundaryMesh", "boundary"), BOUNDARY.replace("    top\n", "    terrain\n")),
    );
    let mut c5 = Case::open(&ran).unwrap();
    assert!(e(c5.create_patch(&ran, "air", &new("y", &[15]))).contains("which the mesh has already"));
    // A prepared set that has gone stops it.
    let fs3 = fs.clone().with(
        "system/air/createPatchDict.umcf",
        format!(
            "{}// prepared on a mesh of 16 faces\npatches ( {{ name gone; constructFrom set; set gone; }} );\n",
            header("dictionary", "createPatchDict")
        ),
    );
    let mut c3 = Case::open(&fs3).unwrap();
    assert!(e(c3.create_patch(&fs3, "air", &new("y", &[14]))).contains("isn't there"));
}

#[test]
fn the_copied_entry_keeps_comments_and_constraint_types_get_none() {
    let b = "2\n(\n    inlet\n    {\n        type            patch;\n        nFaces          2;\n        startFace       10;\n    }\n    side\n    {\n        type            empty;\n        nFaces          2;\n        startFace       12;\n    }\n)\n";
    let t = "boundaryField\n{\n    #includeEtc \"caseDicts/setConstraintTypes\"\n    inlet { type fixedValue; value uniform 1; } // the inlet\n    // the side\n    side { type empty; }\n}\n";
    let fs = MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with("constant/air/polyMesh/boundary", format!("{}{b}", header("polyBoundaryMesh", "boundary")))
        .with("0/air/T", format!("{}internalField uniform 1;\n{t}", header("volScalarField", "T")));
    let mut c = Case::open(&fs).unwrap();
    let new = |name, kind, faces| NewPatch { name, kind, groups: &[], entries: &[], faces };
    c.create_patch(&fs, "air", &new("half", "patch", &[10])).unwrap();
    let out = String::from_utf8(c.file(&fs, "0/air/T").unwrap().unwrap().to_vec()).unwrap();
    assert!(
        out.contains("    inlet { type fixedValue; value uniform 1; } // the inlet\n    half { type fixedValue; value uniform 1; }\n    // the side\n    side"),
        "{out}"
    );
    // A constraint type takes no copy; the included constraint types may give it one.
    let report = c.create_patch(&fs, "air", &new("flat", "symmetry", &[11])).unwrap();
    assert!(report.fields.is_empty() && report.unfilled.is_empty());
    assert_eq!(report.maybe, ["0/air/T"]);
    // From two patches: no entry to copy. An entry for one of its groups (`wall` for a wall)
    // covers it; without one, the field needs one.
    let walls = "boundaryField\n{\n    wall { type zeroGradient; }\n}\n";
    let fs = fs.with("0/air/T", format!("{}internalField uniform 1;\n{walls}", header("volScalarField", "T")));
    let mut c = Case::open(&fs).unwrap();
    let report = c.create_patch(&fs, "air", &new("edge", "wall", &[11, 12])).unwrap();
    assert!(report.unfilled.is_empty() && report.maybe.is_empty() && report.fields.is_empty(), "{report:?}");
    let report = c.create_patch(&fs, "air", &new("edge2", "patch", &[10])).unwrap();
    assert_eq!(report.unfilled, ["0/air/T"]);
    // OpenFOAM gives an empty patch its condition.
    let report = c.create_patch(&fs, "air", &new("edge3", "empty", &[13])).unwrap();
    assert!(report.unfilled.is_empty() && report.maybe.is_empty(), "{report:?}");
}

#[test]
fn an_entry_is_copied_between_patches_of_one_kind_only() {
    let b = "3\n(\n    roof\n    {\n        type            wall;\n        nFaces          2;\n        startFace       10;\n    }\n    side\n    {\n        type            empty;\n        nFaces          2;\n        startFace       12;\n    }\n    other\n    {\n        type            patch;\n        nFaces          2;\n        startFace       14;\n    }\n)\n";
    let t = "boundaryField\n{\n    roof { type nutkWallFunction; value uniform 0; } /* a note\n       on two lines */\n    side { type empty; }\n}\n";
    let fs = MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with(
            "system/air/createPatchDict.umcf",
            format!(
                "{}// prepared on a mesh of 16 faces\npatches\n(\n    {{ name old; constructFrom patches; patches (other); }}\n);\n",
                header("dictionary", "createPatchDict")
            ),
        )
        .with("constant/air/polyMesh/boundary", format!("{}{b}", header("polyBoundaryMesh", "boundary")))
        .with("0/air/nut", format!("{}internalField uniform 0;\n{t}", header("volScalarField", "nut")));
    let mut c = Case::open(&fs).unwrap();
    let new = |name, kind, faces| NewPatch { name, kind, groups: &[], entries: &[], faces };
    // A wall's condition doesn't go to a patch, nor an empty one's.
    let report = c.create_patch(&fs, "air", &new("vent", "patch", &[10])).unwrap();
    assert!(report.fields.is_empty() && report.unfilled == ["0/air/nut"], "{report:?}");
    let report = c.create_patch(&fs, "air", &new("hole", "patch", &[12])).unwrap();
    assert!(report.fields.is_empty() && report.unfilled == ["0/air/nut"], "{report:?}");
    // A wall's does go to a mappedWall, after the block comment, not into it. The item made
    // from patches by hand is kept and passed over.
    let report = c.create_patch(&fs, "air", &new("eave", "mappedWall", &[11])).unwrap();
    assert_eq!(report.fields, ["0/air/nut"]);
    let out = String::from_utf8(c.file(&fs, "0/air/nut").unwrap().unwrap().to_vec()).unwrap();
    assert!(out.contains("    roof { type nutkWallFunction; value uniform 0; }\n    eave { type nutkWallFunction; value uniform 0; } /* a note\n       on two lines */\n    side"), "{out}");
    let dict = c.dict("system/air/createPatchDict.umcf").unwrap().to_string();
    assert!(
        dict.contains("{ name old; constructFrom patches; patches (other); }\n    {\n        name            vent;"),
        "{dict}"
    );
    // The faces of `other` are taken by `old`.
    let e = c.create_patch(&fs, "air", &new("taken", "patch", &[14])).unwrap_err().to_string();
    assert!(e.contains("face 14 is in `old`"), "{e}");
    // By the group OpenFOAM gives a type: `wall` takes the faces of `roof`.
    let group = fs.clone().with(
        "system/air/createPatchDict.umcf",
        format!(
            "{}// prepared on a mesh of 16 faces\npatches ( {{ name w; constructFrom patches; patches (wall); }} );\n",
            header("dictionary", "createPatchDict")
        ),
    );
    let mut c = Case::open(&group).unwrap();
    let e = c.create_patch(&group, "air", &new("x", "patch", &[10])).unwrap_err().to_string();
    assert!(e.contains("face 10 is in `w`"), "{e}");
    // One whose faces can't be told (a pattern) is refused.
    let pattern = fs.clone().with(
        "system/air/createPatchDict.umcf",
        format!(
            "{}// prepared on a mesh of 16 faces\npatches ( {{ name p; constructFrom patches; patches (\"oth.*\"); }} );\n",
            header("dictionary", "createPatchDict")
        ),
    );
    let mut c = Case::open(&pattern).unwrap();
    let e = c.create_patch(&pattern, "air", &new("x", "patch", &[10])).unwrap_err().to_string();
    assert!(e.contains("which is no patch or group of this mesh"), "{e}");
    // A dictionary without the line saying its mesh is refused.
    let bare = fs
        .clone()
        .with("system/air/createPatchDict.umcf", format!("{}patches ();\n", header("dictionary", "createPatchDict")));
    let mut c = Case::open(&bare).unwrap();
    let e = c.create_patch(&bare, "air", &new("x", "patch", &[14])).unwrap_err().to_string();
    assert!(e.contains("doesn't say the mesh it was prepared on"), "{e}");
    assert!(dict.contains("pointSync       false;"), "{dict}");
}

#[test]
fn the_create_patch_command_follows_the_version() {
    use umcf_core::OfVersion;
    assert_eq!(create_patch_command("", None), "createPatch -dict system/createPatchDict.umcf -overwrite");
    assert_eq!(
        create_patch_command("air", Some(OfVersion::V12)),
        "createPatch -region air -dict system/air/createPatchDict.umcf -overwrite"
    );
    assert_eq!(
        create_patch_command("air", Some(OfVersion::V8)),
        "createPatch -region air -dict system/createPatchDict.umcf -overwrite"
    );
    assert!(
        create_patch_command("air", None)
            .contains("OpenFOAM-8: createPatch -region air -dict system/createPatchDict.umcf")
    );
}

#[test]
fn faces_of_coupled_patches_stay() {
    let b = "3\n(\n    left\n    {\n        type            cyclic;\n        nFaces          1;\n        startFace       10;\n        neighbourPatch  right;\n    }\n    right\n    {\n        type            cyclic;\n        nFaces          1;\n        startFace       11;\n        neighbourPatch  left;\n    }\n    top\n    {\n        type            patch;\n        inGroups        List<word> 1(sky);\n        nFaces          2;\n        startFace       12;\n    }\n)\n";
    let fs = MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with("constant/air/polyMesh/boundary", format!("{}{b}", header("polyBoundaryMesh", "boundary")));
    let mut c = Case::open(&fs).unwrap();
    let new = |name, faces| NewPatch { name, kind: "patch", groups: &[], entries: &[], faces };
    let e = c.create_patch(&fs, "air", &new("x", &[10])).unwrap_err().to_string();
    assert!(e.contains("cyclic"), "{e}");
    let e = c.create_patch(&fs, "air", &new("sky", &[12])).unwrap_err().to_string();
    assert!(e.contains("group"), "{e}");
    assert!(c.changed().is_empty());
}

#[test]
fn block_comments_left_open_are_told_apart() {
    for (text, open) in [
        (" // a note", false),
        (" // files /*.stl", false),
        (" /* see http://x */", false),
        (" /* a // b */ // c", false),
        (" /* a note", true),
        (" /* a */ /* b", true),
        ("", false),
        (" /*/", true),
        (" //*", false),
        (" /**/", false),
    ] {
        assert_eq!(opens_block_comment(text), open, "{text:?}");
    }
}

/// A case whose `0.orig/air` can't be listed and whose `0.orig/notes.txt` can't be read.
struct Failing(MemFs);

impl umcf_core::CaseFs for Failing {
    fn read(&self, rel: &str, limit: u64) -> std::result::Result<Option<Vec<u8>>, FsError> {
        if rel == "0.orig/notes.txt" {
            return Err(FsError::Io { path: rel.to_string(), source: std::io::Error::other("denied") });
        }
        self.0.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.0.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.0.is_dir(rel)
    }
    fn list(&self, rel: &str) -> std::result::Result<Vec<String>, FsError> {
        if rel == "0.orig/air" {
            return Err(FsError::Io { path: rel.to_string(), source: std::io::Error::other("denied") });
        }
        self.0.list(rel)
    }
}

#[test]
fn a_rename_says_what_it_could_not_list_or_read() {
    let mem = case()
        .with("0.orig/air/T", "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\nboundaryField\n{\n    ground { type zeroGradient; }\n}\n")
        .with("0.orig/notes.txt", "ground\n");
    // Opened with the failures: `0.orig/air` couldn't be listed then either.
    let fs = Failing(mem);
    let mut c = Case::open(&fs).unwrap();
    assert_eq!(c.exists("0.orig/air/T"), None);
    let report =
        c.set_patch(&fs, "air", "ground", &PatchChange { name: Some("terrain"), ..Default::default() }).unwrap();
    assert!(report.orig.is_empty());
    assert_eq!(report.not_renamed.len(), 1, "{:?}", report.not_renamed);
    assert_eq!(report.not_renamed[0].0, "0.orig/air");
    assert!(report.not_renamed[0].1.starts_with("it can't be listed"), "{:?}", report.not_renamed);
    assert_eq!(report.unsearched, ["0.orig/air", "0.orig/notes.txt"]);
}

#[test]
fn a_rename_follows_into_a_compressed_view_factor_dictionary_and_searches_held_ones() {
    use std::io::Write;
    let vf = format!("{}ground\n{{\n    nFacesInCoarsestLevel 15;\n}}\n", header("dictionary", "viewFactorsDict"));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(vf.as_bytes()).unwrap();
    let fs = case().with("constant/air/viewFactorsDict.gz", gz.finish().unwrap());
    let mut c = Case::open(&fs).unwrap();
    // A dictionary the case holds since a command made it: the pending createPatch dictionary,
    // given an entry naming the patch.
    let ground =
        c.region("air").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == "ground").cloned().unwrap();
    let faces = [ground.start_face as u32];
    c.create_patch(&fs, "air", &NewPatch { name: "lid", kind: "patch", groups: &[], entries: &[], faces: &faces })
        .unwrap();
    c.set("system/air/createPatchDict.umcf", &["note"], "ground").unwrap();
    assert!(c.put_bytes("system/air/createPatchDict.umcf").is_none(), "held, not put");
    // A view factor dictionary put by a command and not saved yet.
    c.put_file(&fs, "system/air/viewFactorsDict", vf.clone().into_bytes()).unwrap();
    let report =
        c.set_patch(&fs, "air", "ground", &PatchChange { name: Some("terrain"), ..Default::default() }).unwrap();
    assert_eq!(report.others, ["system/air/viewFactorsDict", "constant/air/viewFactorsDict.gz"]);
    let put = String::from_utf8(c.put_bytes("system/air/viewFactorsDict").unwrap().to_vec()).unwrap();
    assert!(put.contains("terrain\n{"), "{put}");
    let bytes = c.put_bytes("constant/air/viewFactorsDict.gz").unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut text).unwrap();
    assert!(text.contains("terrain\n{\n    nFacesInCoarsestLevel 15;"), "{text}");
    assert!(report.mentions.iter().any(|(f, _)| f == "system/air/createPatchDict.umcf"), "{:?}", report.mentions);
}
