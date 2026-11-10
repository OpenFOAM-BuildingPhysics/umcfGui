use std::io::Write;

use umcf_core::{CaseFs, CaseWrite, FsError, MemFs};

use super::*;
use crate::PatchChange;

fn header(class: &str, object: &str) -> String {
    format!(
        "FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       {class};\n    object      {object};\n}}\n"
    )
}

const CONTROL: &str = "application urbanMicroclimateFoam;\nstartFrom startTime;\nstartTime 0;\nstopAt endTime;\nendTime 7200;\ndeltaT 3600;\nwriteControl timeStep;\nwriteInterval 1;\nregions { fluid (air); }\n";

fn boundary(patches: &[&str]) -> String {
    let mut b = format!("{}{}\n(\n", header("polyBoundaryMesh", "boundary"), patches.len());
    for (i, p) in patches.iter().enumerate() {
        b += &format!(
            "    {p}\n    {{\n        type            patch;\n        nFaces          1;\n        startFace       {};\n    }}\n",
            10 + i
        );
    }
    b + ")\n"
}

fn field(name: &str, patches: &[&str]) -> String {
    let mut f = format!(
        "{}// a comment\ninternalField   nonuniform List<scalar> 3(300 301 302);\nboundaryField\n{{\n",
        header("volScalarField", name)
    );
    for p in patches {
        f += &format!("    {p}\n    {{\n        type            zeroGradient; // kept\n    }}\n");
    }
    f + "}\n"
}

fn gz(text: &str) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(text.as_bytes()).unwrap();
    e.finish().unwrap()
}

fn text(fs: &MemFs, rel: &str) -> String {
    let b = fs.read(rel, 1 << 30).unwrap().unwrap();
    if b.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(b.as_slice()), &mut s).unwrap();
        s
    } else {
        String::from_utf8(b).unwrap()
    }
}

/// A case of one fluid `air` with results at 3600 and 7200 (7200's `T` compressed, a mesh of its
/// own at 3600), decomposed in two, inflow data for `inlet`.
fn case() -> MemFs {
    let mut fs = MemFs::new()
        .with("system/controlDict", format!("{}{CONTROL}", header("dictionary", "controlDict")))
        .with("system/air/fvSchemes", header("dictionary", "fvSchemes"))
        .with("constant/air/polyMesh/boundary", boundary(&["inlet", "top"]))
        .with("0/air/T", field("T", &["inlet", "top"]))
        .with("3600/air/T", field("T", &["inlet", "top"]))
        .with("3600/air/polyMesh/boundary", boundary(&["inlet", "top"]))
        .with("7200/air/T.gz", gz(&field("T", &["inlet", "top"])))
        .with("7200/air/notes", "no inlet here")
        .with("constant/air/boundaryData/inlet/points", "(\n(0 0 0)\n)\n")
        .with("constant/air/boundaryData/inlet/0/T", "(\n300\n)\n");
    for p in ["processor0", "processor1"] {
        fs.insert(&format!("{p}/constant/air/polyMesh/boundary"), boundary(&["inlet", "top", "procBoundary0to1"]));
        for t in ["0", "3600"] {
            fs.insert(&format!("{p}/{t}/air/T"), field("T", &["inlet", "top", "procBoundary0to1"]));
        }
    }
    fs
}

fn rename(c: &mut Case, fs: &MemFs, from: &str, to: &str) {
    c.set_patch(fs, "air", from, &PatchChange { name: Some(to), results: true, ..Default::default() }).unwrap();
}

#[test]
fn a_new_name_follows_into_the_results_when_saved_and_back_when_undone() {
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    let found = c.result_files(&fs, "air", "inlet").unwrap();
    assert_eq!(found.times, ["3600", "7200", "processor0/0", "processor0/3600", "processor1/0", "processor1/3600"]);
    assert_eq!(found.processors, ["processor0", "processor1"]);
    assert_eq!(found.dirs, ["constant/air/boundaryData/inlet"]);
    assert!(
        found.files.contains(&"7200/air/T.gz".to_string())
            && found.files.contains(&"processor1/constant/air/polyMesh/boundary".to_string()),
        "{found:?}"
    );
    let before = c.clone();
    rename(&mut c, &fs, "inlet", "west");
    assert!(c.changed().iter().any(|f| f == "results: air/inlet renamed west"), "{:?}", c.changed());
    let orig_3600 = text(&fs, "3600/air/T");
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    assert!(c.changed().is_empty(), "{:?}", c.changed());
    assert_eq!(saved.dirs, ["constant/air/boundaryData/west"]);
    assert!(saved.skipped.is_empty(), "{saved:?}");
    // Only the patch's entry: the comments, the list, the other entries as they were.
    assert_eq!(text(&fs, "3600/air/T"), orig_3600.replace("    inlet\n", "    west\n"));
    for f in ["7200/air/T.gz", "processor0/0/air/T", "processor1/3600/air/T"] {
        assert!(text(&fs, f).contains("    west\n    {\n        type            zeroGradient; // kept"), "{f}");
        assert!(!text(&fs, f).contains("inlet"), "{f}");
    }
    for f in ["3600/air/polyMesh/boundary", "processor0/constant/air/polyMesh/boundary"] {
        assert!(text(&fs, f).contains("    west\n    {") && !text(&fs, f).contains("inlet"), "{f}");
    }
    assert!(fs.is_file("constant/air/boundaryData/west/0/T") && !fs.is_dir("constant/air/boundaryData/inlet"));
    assert_eq!(saved.files.len(), 9, "{saved:?}");
    assert!(!saved.files.contains(&"7200/air/notes".to_string()));
    // Nothing more to do.
    assert_eq!(c.save_results(&mut fs, 1 << 30).unwrap().renames, []);
    // Undone after the save: saving takes it back out of the results too.
    let mut c = before;
    assert_eq!(c.results_pending()[0].to, "inlet");
    c.save(&mut fs, None).unwrap();
    c.save_results(&mut fs, 1 << 30).unwrap();
    assert_eq!(text(&fs, "3600/air/T"), orig_3600);
    assert!(fs.is_dir("constant/air/boundaryData/inlet") && !fs.is_dir("constant/air/boundaryData/west"));
    assert!(c.changed().is_empty(), "{:?}", c.changed());
}

#[test]
fn renames_in_a_row_and_files_that_can_t_be_renamed() {
    let mut fs = case();
    // A file with the new name already, one too big for the memory given.
    fs.insert("processor1/3600/air/T", field("T", &["inlet", "east"]));
    fs.insert("3600/air/U", field("U", &["inlet"]).replace("3(300 301 302)", &format!("3000({})", "1 ".repeat(3000))));
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs, "inlet", "west");
    rename(&mut c, &fs, "west", "east");
    assert_eq!(c.results().len(), 2);
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 2000).unwrap();
    let skipped: Vec<&str> = saved.skipped.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(skipped, ["3600/air/U", "processor1/3600/air/T"], "{saved:?}");
    assert!(saved.skipped[0].1.contains("more than the memory"), "{saved:?}");
    // In order: `inlet` to `west`, then `west` meets the `east` there.
    assert!(saved.skipped[1].1.contains("entries `west` and `east`"), "{saved:?}");
    // Each file once, both renames: `east` at the end.
    assert!(text(&fs, "3600/air/T").contains("    east\n") && !text(&fs, "3600/air/T").contains("west"));
    assert!(fs.is_dir("constant/air/boundaryData/east"));
    assert!(c.changed().is_empty());
}

#[test]
fn without_results_nothing_but_the_case_changes() {
    let mut fs = case();
    let mut c = Case::open(&fs).unwrap();
    c.set_patch(&fs, "air", "inlet", &PatchChange { name: Some("west"), ..Default::default() }).unwrap();
    assert!(c.results().is_empty());
    c.save(&mut fs, None).unwrap();
    assert!(c.save_results(&mut fs, 1 << 30).unwrap().files.is_empty());
    assert!(text(&fs, "3600/air/T").contains("    inlet\n"));
    assert!(text(&fs, "0/air/T").contains("    west\n"));
}

#[test]
fn the_partners_in_other_regions_meshes_follow() {
    let mapped = |name: &str, partner: &str| {
        format!(
            "{}2\n(\n    {name}\n    {{\n        type            mappedWall;\n        nFaces          1;\n        startFace       5;\n        sampleMode      nearestPatchFace;\n        neighbourRegion air;\n        neighbourPatch  {partner};\n    }}\n    bottom\n    {{\n        type            wall;\n        nFaces          1;\n        startFace       6;\n    }}\n)\n",
            header("polyBoundaryMesh", "boundary")
        )
    };
    let mut fs = case().with(
        "system/controlDict",
        format!(
            "{}{}",
            header("dictionary", "controlDict"),
            CONTROL.replace("fluid (air);", "fluid (air); solid (street);")
        ),
    );
    fs.insert("constant/street/polyMesh/boundary", mapped("street_to_air", "inlet"));
    fs.insert("system/street/fvSchemes", header("dictionary", "fvSchemes"));
    for f in ["processor0/constant/street/polyMesh/boundary", "3600/street/polyMesh/boundary"] {
        fs.insert(f, mapped("street_to_air", "inlet"));
    }
    // One that names another region's patch of that name is left alone.
    fs.insert(
        "processor1/constant/street/polyMesh/boundary",
        mapped("street_to_air", "inlet").replace("neighbourRegion air", "neighbourRegion roof"),
    );
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs, "inlet", "west");
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    for f in ["processor0/constant/street/polyMesh/boundary", "3600/street/polyMesh/boundary"] {
        assert!(text(&fs, f).contains("        neighbourPatch  west;\n"), "{f}: {}", text(&fs, f));
        assert!(saved.files.contains(&f.to_string()), "{saved:?}");
    }
    assert!(text(&fs, "processor1/constant/street/polyMesh/boundary").contains("neighbourPatch  inlet;"));
    // The case's own mesh followed with the command, as before.
    assert!(text(&fs, "constant/street/polyMesh/boundary").contains("neighbourPatch  west;"));
}

/// A case on `MemFs` whose writes to `fails` fail, as a file linked out of the case would.
struct Failing {
    fs: MemFs,
    fails: &'static str,
    /// Reading this one is cancelled (the flag set), while `cancel` holds.
    cancel: Option<&'static str>,
}

impl CaseFs for Failing {
    fn read(&self, rel: &str, limit: u64) -> std::result::Result<Option<Vec<u8>>, FsError> {
        if self.cancel == Some(rel) {
            return Err(FsError::Cancelled { path: rel.to_string() });
        }
        self.fs.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.fs.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.fs.is_dir(rel)
    }
    fn list(&self, rel: &str) -> std::result::Result<Vec<String>, FsError> {
        self.fs.list(rel)
    }
}

impl CaseWrite for Failing {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> std::result::Result<(), FsError> {
        if rel == self.fails { Err(FsError::BadPath(rel.to_string())) } else { self.fs.write(rel, bytes) }
    }
    fn rename_dir(&mut self, from: &str, to: &str) -> std::result::Result<(), FsError> {
        self.fs.rename_dir(from, to)
    }
}

#[test]
fn a_file_that_can_t_be_written_is_skipped_and_an_undo_takes_back_only_what_was_done() {
    let mut fs = Failing {
        fs: case().with("3600/air/U", field("U", &["inlet", "top"])),
        fails: "processor0/0/air/T",
        cancel: None,
    };
    let mut c = Case::open(&fs).unwrap();
    let before = c.clone();
    rename(&mut c, &fs.fs, "inlet", "west");
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    // The rest went on; the one not written says so, and what a run from there needs.
    assert_eq!(saved.skipped.len(), 1, "{saved:?}");
    let (f, why) = &saved.skipped[0];
    assert_eq!(f, "processor0/0/air/T");
    assert!(
        why.contains("can't be written") && why.contains("a run from processor0/0 needs it renamed there by hand"),
        "{why}"
    );
    assert!(text(&fs.fs, "processor0/0/air/T").contains("    inlet\n"));
    assert!(text(&fs.fs, "processor1/0/air/T").contains("    west\n"));
    // Both U and T at 3600.
    assert!(text(&fs.fs, "3600/air/U").contains("    west\n") && text(&fs.fs, "3600/air/T").contains("    west\n"));
    assert!(c.changed().is_empty());
    // Undone and saved: back where it was renamed; the skipped file named the old name all along.
    let mut c = before;
    c.save(&mut fs, None).unwrap();
    let back = c.save_results(&mut fs, 1 << 30).unwrap();
    assert!(back.skipped.is_empty(), "{back:?}");
    for f in ["processor0/0/air/T", "processor1/0/air/T", "3600/air/U", "7200/air/T.gz"] {
        assert!(text(&fs.fs, f).contains("    inlet\n") && !text(&fs.fs, f).contains("west"), "{f}");
    }
    assert!(fs.fs.is_dir("constant/air/boundaryData/inlet"));
}

#[test]
fn a_field_beside_its_compressed_copy_both_follow() {
    let mut fs = case().with("3600/air/p", field("p", &["inlet"])).with("3600/air/p.gz", gz(&field("p", &["inlet"])));
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs, "inlet", "west");
    c.save(&mut fs, None).unwrap();
    c.save_results(&mut fs, 1 << 30).unwrap();
    assert!(text(&fs, "3600/air/p").contains("    west\n") && text(&fs, "3600/air/p.gz").contains("    west\n"));
}

#[test]
fn a_cancelled_save_is_finished_by_the_next_or_taken_back_by_an_undo() {
    for undo in [false, true] {
        let mut fs = Failing { fs: case(), fails: "", cancel: Some("processor0/3600/air/T") };
        let mut c = Case::open(&fs).unwrap();
        let before = c.clone();
        rename(&mut c, &fs.fs, "inlet", "west");
        c.save(&mut fs, None).unwrap();
        // Stopped part way: what it wrote is listed, and what to do.
        match c.save_results(&mut fs, 1 << 30) {
            Err(crate::Error::Partial { saved, source }) => {
                assert!(saved.contains(&"3600/air/T".to_string()), "{saved:?}");
                assert!(source.to_string().contains("save again to finish it"), "{source}");
            }
            r => panic!("{r:?}"),
        }
        // Half done, and still to do.
        assert!(text(&fs.fs, "3600/air/T").contains("    west\n"));
        assert!(text(&fs.fs, "processor1/0/air/T").contains("    inlet\n"));
        assert!(c.changed().iter().any(|l| l == "results: air/inlet renamed west"), "{:?}", c.changed());
        fs.cancel = None;
        let want = if undo {
            c = before.clone();
            c.save(&mut fs, None).unwrap();
            "inlet"
        } else {
            "west"
        };
        c.save_results(&mut fs, 1 << 30).unwrap();
        assert!(c.changed().is_empty(), "{:?}", c.changed());
        for f in [
            "3600/air/T",
            "7200/air/T.gz",
            "processor0/3600/air/T",
            "processor1/0/air/T",
            "processor1/constant/air/polyMesh/boundary",
        ] {
            assert!(text(&fs.fs, f).contains(&format!("    {want}\n")), "undo {undo}: {f}");
        }
        assert!(fs.fs.is_dir(&format!("constant/air/boundaryData/{want}")), "undo {undo}");
    }
}

#[test]
fn a_swap_through_a_third_name_cancelled_half_way_ends_a_swap_or_as_it_was() {
    for undo in [false, true] {
        let mut fs = Failing { fs: case(), fails: "", cancel: Some("processor0/3600/air/T") };
        let mut c = Case::open(&fs).unwrap();
        let before = c.clone();
        rename(&mut c, &fs.fs, "inlet", "tmp");
        rename(&mut c, &fs.fs, "top", "inlet");
        rename(&mut c, &fs.fs, "tmp", "top");
        c.save(&mut fs, None).unwrap();
        assert!(c.save_results(&mut fs, 1 << 30).is_err());
        // Cancelled half way, twice over: the files done stay done.
        assert!(c.save_results(&mut fs, 1 << 30).is_err());
        fs.cancel = None;
        let (first, second) = if undo {
            c = before.clone();
            c.save(&mut fs, None).unwrap();
            ("inlet", "top")
        } else {
            ("top", "inlet")
        };
        c.save_results(&mut fs, 1 << 30).unwrap();
        assert!(c.changed().is_empty(), "{:?}", c.changed());
        // The fields list `inlet` then `top` as written: swapped, their entries' names are.
        for f in ["3600/air/T", "7200/air/T.gz", "processor0/0/air/T", "processor0/3600/air/T", "processor1/3600/air/T"]
        {
            let t = text(&fs.fs, f);
            let a = t.find(&format!("    {first}\n")).unwrap_or_else(|| panic!("undo {undo}: {f}: {t}"));
            let b = t.find(&format!("    {second}\n")).unwrap();
            assert!(a < b && !t.contains("tmp"), "undo {undo}: {f}: {t}");
        }
        let dir = if undo { "inlet" } else { "top" };
        assert!(fs.fs.is_dir(&format!("constant/air/boundaryData/{dir}")), "undo {undo}");
    }
}

#[test]
fn cancels_further_on_and_a_rename_asked_in_between() {
    let mut fs = Failing { fs: case(), fails: "", cancel: Some("processor0/0/air/T") };
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs.fs, "inlet", "tmp");
    rename(&mut c, &fs.fs, "top", "inlet");
    rename(&mut c, &fs.fs, "tmp", "top");
    c.save(&mut fs, None).unwrap();
    assert!(c.save_results(&mut fs, 1 << 30).is_err());
    // Further on the second time: the files between have had the swap since.
    fs.cancel = Some("processor1/0/air/T");
    assert!(c.save_results(&mut fs, 1 << 30).is_err());
    assert!(text(&fs.fs, "processor0/3600/air/T").contains("    top\n"));
    assert!(text(&fs.fs, "processor1/3600/air/T").contains("    inlet\n    {"));
    // A rename asked before it's finished: after the swap, everywhere.
    rename(&mut c, &fs.fs, "top", "west");
    fs.cancel = None;
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    // Each file once, though written by more than one call.
    let mut once = saved.files.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), saved.files.len(), "{saved:?}");
    assert!(saved.files.contains(&"3600/air/T".to_string()), "{saved:?}");
    assert!(c.changed().is_empty(), "{:?}", c.changed());
    for f in [
        "3600/air/T",
        "7200/air/T.gz",
        "processor0/0/air/T",
        "processor0/3600/air/T",
        "processor1/0/air/T",
        "processor1/3600/air/T",
    ] {
        let t = text(&fs.fs, f);
        let (a, b) = (t.find("    west\n").unwrap_or_else(|| panic!("{f}: {t}")), t.find("    inlet\n").unwrap());
        assert!(a < b && !t.contains("top") && !t.contains("tmp"), "{f}: {t}");
    }
    assert!(fs.fs.is_dir("constant/air/boundaryData/west"));
}

#[test]
fn what_was_skipped_before_a_cancel_is_reported_when_finished() {
    let big = field("U", &["inlet"]).replace("3(300 301 302)", &format!("3000({})", "1 ".repeat(3000)));
    let mut fs = Failing { fs: case().with("3600/air/U", big), fails: "", cancel: Some("processor0/0/air/T") };
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs.fs, "inlet", "west");
    c.save(&mut fs, None).unwrap();
    assert!(c.save_results(&mut fs, 2000).is_err());
    fs.cancel = None;
    let saved = c.save_results(&mut fs, 2000).unwrap();
    assert!(saved.skipped.iter().any(|(f, _)| f == "3600/air/U"), "{saved:?}");
    // The files of both calls.
    assert!(
        saved.files.contains(&"3600/air/T".to_string()) && saved.files.contains(&"processor0/0/air/T".to_string()),
        "{saved:?}"
    );
    // Reported once.
    rename(&mut c, &fs.fs, "west", "east");
    c.save(&mut fs, None).unwrap();
    let next = c.save_results(&mut fs, 2000).unwrap();
    assert_eq!(next.skipped.iter().filter(|(f, _)| f == "3600/air/U").count(), 1, "{next:?}");
}

#[test]
fn a_decomposed_cyclic_s_processor_patches_follow() {
    let pc = "    procBoundary0to1throughleft\n    {\n        type            processorCyclic;\n        nFaces          1;\n        startFace       20;\n        myProcNo        0;\n        neighbProcNo    1;\n        referPatch      left;\n    }\n";
    let mut fs = case();
    let b = boundary(&["inlet", "top", "left"])
        .replace("3\n(\n", "4\n(\n")
        .replace("    }\n)\n", &format!("    }}\n{pc})\n"));
    assert!(b.contains("referPatch"), "{b}");
    fs.insert("processor0/constant/air/polyMesh/boundary", b);
    fs.insert("constant/air/polyMesh/boundary", boundary(&["inlet", "top", "left"]));
    let mut c = Case::open(&fs).unwrap();
    rename(&mut c, &fs, "left", "west");
    c.save(&mut fs, None).unwrap();
    c.save_results(&mut fs, 1 << 30).unwrap();
    let t = text(&fs, "processor0/constant/air/polyMesh/boundary");
    assert!(t.contains("        referPatch      west;\n") && t.contains("    west\n    {"), "{t}");
    // The processor patch keeps its name: its fields' entries are under it.
    assert!(t.contains("procBoundary0to1throughleft"), "{t}");
}

#[test]
fn a_case_of_one_region_has_its_results_right_there() {
    let control = CONTROL.replace("regions { fluid (air); }\n", "");
    let mut fs = MemFs::new()
        .with("system/controlDict", format!("{}{control}", header("dictionary", "controlDict")))
        .with("constant/polyMesh/boundary", boundary(&["inlet", "top"]))
        .with("0/T", field("T", &["inlet", "top"]))
        .with("3600/T", field("T", &["inlet", "top"]))
        .with("constant/boundaryData/inlet/points", "()\n")
        .with("processor0/constant/polyMesh/boundary", boundary(&["inlet", "top"]))
        .with("processor0/3600/T", field("T", &["inlet", "top"]));
    let mut c = Case::open(&fs).unwrap();
    c.set_patch(&fs, "", "inlet", &PatchChange { name: Some("west"), results: true, ..Default::default() }).unwrap();
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    for f in ["3600/T", "processor0/3600/T", "processor0/constant/polyMesh/boundary"] {
        assert!(text(&fs, f).contains("    west\n"), "{f}: {saved:?}");
    }
    assert!(fs.is_dir("constant/boundaryData/west"), "{saved:?}");
}

#[cfg(unix)]
#[test]
fn a_file_reached_through_a_linked_directory_is_renamed_once() {
    let root = std::env::temp_dir().join(format!("umcf-results-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mem = case();
    for f in ["system/controlDict", "system/air/fvSchemes", "constant/air/polyMesh/boundary", "0/air/T", "3600/air/T"] {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, mem.read(f, 1 << 20).unwrap().unwrap()).unwrap();
    }
    // 7200 is 3600 under another name.
    std::os::unix::fs::symlink(root.join("3600"), root.join("7200")).unwrap();
    let mut fs = umcf_core::DirFs::new(&root);
    let mut c = Case::open(&fs).unwrap();
    // A swap: done twice to the one file, it would come back as it was.
    for (from, to) in [("inlet", "tmp"), ("top", "inlet"), ("tmp", "top")] {
        c.set_patch(&fs, "air", from, &PatchChange { name: Some(to), results: true, ..Default::default() }).unwrap();
    }
    c.save(&mut fs, None).unwrap();
    let saved = c.save_results(&mut fs, 1 << 30).unwrap();
    assert_eq!(saved.files.len(), 1, "{saved:?}");
    let t = std::fs::read_to_string(root.join("3600/air/T")).unwrap();
    assert!(t.find("    top\n").unwrap() < t.find("    inlet\n").unwrap(), "{t}");
    std::fs::remove_dir_all(&root).unwrap();
}
