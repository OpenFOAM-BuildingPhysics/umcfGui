//! `umcf-cli` end to end, on a small case written to a scratch directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HEADER: &str = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class dictionary;\n}\n";

fn case(name: &str, end_time: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let control = format!(
        "{HEADER}application urbanMicroclimateFoam;\nstartFrom startTime;\nstartTime 0;\nstopAt endTime;\nendTime {end_time};\ndeltaT 60;\nwriteInterval 600;\nregions\n{{\n    fluid (air);\n}}\n"
    );
    let boundary = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class polyBoundaryMesh;\n}\n1\n(\n    inlet\n    {\n        type patch;\n        nFaces 1;\n        startFace 0;\n    }\n)\n";
    let t = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ninternalField uniform 300;\nboundaryField\n{\n    inlet { type zeroGradient; }\n}\n";
    let g = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class uniformDimensionedVectorField;\n}\ndimensions [0 1 -2 0 0 0 0];\nvalue (0 0 -9.81);\n";
    let mut files = vec![
        ("system/controlDict".to_string(), control.clone()),
        ("constant/air/polyMesh/boundary".to_string(), boundary.to_string()),
        ("constant/air/g".to_string(), g.to_string()),
    ];
    // The fields the solver reads in a fluid region.
    for f in ["T", "U", "w", "gcr", "p_rgh", "p"] {
        let class = if f == "U" { "volVectorField" } else { "volScalarField" };
        files.push((format!("0/air/{f}"), t.replace("volScalarField", class)));
    }
    for (rel, text) in &files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_umcf-cli")).args(args).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn check_a_clean_case() {
    let dir = case("clean", "3600");
    let o = cli(&["check", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(stdout(&o).trim_end().ends_with("0 errors, 0 warnings, 0 notes"), "{}", stdout(&o));
}

#[test]
fn check_finds_errors() {
    let dir = case("errors", "-1");
    let o = cli(&["check", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let text = stdout(&o);
    assert!(text.lines().next().unwrap().starts_with("error"), "{text}");
    assert!(text.contains("system/controlDict"), "{text}");

    let o = cli(&["check", "--json", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(v["errors"].as_u64().unwrap() >= 1);
    assert_eq!(v["diagnostics"][0]["level"], "error");
}

#[test]
fn check_bad_arguments() {
    assert_eq!(cli(&["check"]).status.code(), Some(2));
    assert_eq!(cli(&["check", "/no/such/case"]).status.code(), Some(2));
    assert_eq!(cli(&["check", "--bogus", "x"]).status.code(), Some(2));
    assert_eq!(cli(&["nope"]).status.code(), Some(2));
    assert_eq!(cli(&[]).status.code(), Some(2));
    assert_eq!(cli(&["--help"]).status.code(), Some(0));
}

#[test]
fn run_edits_saves_and_journals() {
    let dir = case("run", "3600");
    let journal = dir.with_extension("jsonl");
    let o = cli(&[
        "run",
        "--case",
        dir.to_str().unwrap(),
        "--cmd",
        r#"control.set={"endTime": 7200}"#,
        "--cmd",
        "case.save",
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let control = std::fs::read_to_string(dir.join("system/controlDict")).unwrap();
    assert!(control.contains("endTime 7200;"), "{control}");
    assert!(dir.join(".umcfgui/backup").is_dir(), "a backup was kept");
    let lines: Vec<String> = std::fs::read_to_string(&journal).unwrap().lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "the edit and the save, not the open: {lines:?}");
    assert!(lines[0].contains("control.set") && lines[1].contains("case.save"));
    assert!(!lines.iter().any(|l| l.contains(dir.to_str().unwrap())), "no path in the journal");

    // The journal replays on another copy, and only there.
    let other = case("replay", "3600");
    let replay = other.with_extension("jsonl");
    let text = std::fs::read_to_string(&journal).unwrap();
    std::fs::write(&replay, format!("# replay\n\n{text}")).unwrap();
    std::fs::write(dir.join("system/controlDict"), "untouched").unwrap();
    let o = cli(&["run", "--case", other.to_str().unwrap(), "--cmds", replay.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(std::fs::read_to_string(other.join("system/controlDict")).unwrap().contains("endTime 7200;"));
    assert_eq!(std::fs::read_to_string(dir.join("system/controlDict")).unwrap(), "untouched");
}

#[test]
fn run_stops_at_the_first_failure() {
    let dir = case("fail", "3600");
    let o = cli(&[
        "run",
        "--json",
        "--case",
        dir.to_str().unwrap(),
        "--cmd",
        r#"control.set={"endTime": null}"#,
        "--cmd",
        "case.save",
    ]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let lines: Vec<serde_json::Value> = stdout(&o).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "case.save never ran");
    assert_eq!(lines[1]["ok"], false);
    assert!(std::fs::read_to_string(dir.join("system/controlDict")).unwrap().contains("endTime 3600;"));
    assert_eq!(cli(&["run"]).status.code(), Some(2));
    // A journal that can't be written doesn't turn a failed command into a usage error.
    let bad = dir.join("no/such/dir/j.jsonl");
    let o = cli(&["run", "--case", dir.to_str().unwrap(), "--cmd", "edit.undo", "--journal", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let o = cli(&["run", "--case", dir.to_str().unwrap(), "--journal", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    // The command's id is not printed twice.
    let o = cli(&["run", "--case", dir.to_str().unwrap(), "--cmd", r#"control.set={"a b": 1}"#]);
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.starts_with("umcf-cli: control.set: `a b`"), "{err}");
    assert_eq!(cli(&["run", "--cmd", "control.set={bad"]).status.code(), Some(2));
}

/// The cap on a commands file holds for files with no length (pipes, devices).
#[cfg(unix)]
#[test]
fn a_commands_file_without_a_length_is_capped() {
    let o = cli(&["run", "--cmds", "/dev/zero"]);
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(String::from_utf8(o.stderr).unwrap().contains("larger than"));
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let o = Command::new(env!("CARGO_BIN_EXE_umcf-cli"))
        .arg("check")
        .arg(std::ffi::OsStr::from_bytes(b"case\xff"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
}

#[test]
fn a_second_case_starts_a_new_journal() {
    let a = case("journal-a", "3600");
    let b = case("journal-b", "3600");
    let journal = a.with_extension("second.jsonl");
    let o = cli(&[
        "run",
        "--case",
        a.to_str().unwrap(),
        "--cmd",
        r#"control.set={"endTime": 1}"#,
        "--cmd",
        "case.save",
        "--case",
        b.to_str().unwrap(),
        "--cmd",
        r#"control.set={"deltaT": 2}"#,
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let text = std::fs::read_to_string(&journal).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("deltaT"), "{text}");
}

#[test]
fn commands_lists_the_registry() {
    let o = cli(&["commands", "--json"]);
    assert_eq!(o.status.code(), Some(0));
    let ids: Vec<String> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"case.check".to_string()) && ids.contains(&"edit.undo".to_string()), "{ids:?}");
}

/// A one-cell case: a unit cube (or, with `flip`, one whose top face is pushed below the
/// bottom: a negative volume).
fn cube(name: &str, flip: bool) -> PathBuf {
    let dir = case(name, "3600");
    let top = if flip { -1.0 } else { 1.0 };
    let pts = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, top],
        [1.0, 0.0, top],
        [1.0, 1.0, top],
        [0.0, 1.0, top],
    ];
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    let points: Vec<String> = pts.iter().map(|p| format!("({} {} {})", p[0], p[1], p[2])).collect();
    // Outward normals for the unit cube.
    let faces = ["4(0 3 2 1)", "4(4 5 6 7)", "4(0 1 5 4)", "4(3 7 6 2)", "4(0 4 7 3)", "4(1 2 6 5)"];
    let files = [
        ("points", format!("{}8\n(\n{}\n)\n", h("vectorField"), points.join("\n"))),
        ("faces", format!("{}6\n(\n{}\n)\n", h("faceList"), faces.join("\n"))),
        ("owner", format!("{}6\n(\n0 0 0 0 0 0\n)\n", h("labelList"))),
        ("neighbour", format!("{}0\n(\n)\n", h("labelList"))),
        (
            "boundary",
            format!(
                "{}1\n(\n    inlet\n    {{\n        type patch;\n        nFaces 6;\n        startFace 0;\n    }}\n)\n",
                h("polyBoundaryMesh")
            ),
        ),
    ];
    for (f, text) in files {
        std::fs::write(dir.join("constant/air/polyMesh").join(f), text).unwrap();
    }
    dir
}

#[test]
fn mesh_checks_the_mesh() {
    let dir = cube("mesh-ok", false);
    let o = cli(&["mesh", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(stdout(&o).starts_with("air: 1 cells"), "{}", stdout(&o));
    let o = cli(&["mesh", "--json", "--region", "air", dir.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["regions"][0]["quality"]["cells"], 1);
    assert!((v["regions"][0]["quality"]["total_volume"].as_f64().unwrap() - 1.0).abs() < 1e-12);
    assert!(v["regions"][0]["quality"]["all_geometry"].is_null(), "only with --all-geometry");
    let o = cli(&["mesh", "--json", "--all-geometry", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let g = &v["regions"][0]["quality"]["all_geometry"];
    assert_eq!((g["concave_cells"]["count"].as_u64(), g["min_flatness"].as_f64()), (Some(0), Some(1.0)), "{g}");

    let bad = cube("mesh-bad", true);
    let o = cli(&["mesh", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    assert!(stdout(&o).contains("mesh.negative-volume"), "{}", stdout(&o));

    assert_eq!(cli(&["mesh", "--region", "nope", dir.to_str().unwrap()]).status.code(), Some(2));
    // A case whose mesh is only a boundary file: that region's error, 1.
    let only = case("mesh-none", "3600");
    let o = cli(&["mesh", only.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    assert!(
        stdout(&o).contains("mesh.unreadable") && stdout(&o).contains("air: the mesh could not be read"),
        "{}",
        stdout(&o)
    );
}

/// checkMesh's own results: its log in the case, and the sets `-writeSets` left.
#[test]
fn mesh_reads_checkmesh_results() {
    let dir = cube("mesh-cm", false);
    std::fs::write(
        dir.join("log.checkMesh.air"),
        "Build  : 12\nExec   : checkMesh -region air -writeSets\n ***Max skewness = 5, 1 highly skew faces detected\nFailed 1 mesh checks.\n",
    )
    .unwrap();
    let sets = dir.join("constant/air/polyMesh/sets");
    std::fs::create_dir_all(&sets).unwrap();
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    std::fs::write(sets.join("skewFaces"), format!("{}1(3)\n", h("faceSet"))).unwrap();
    std::fs::write(sets.join("nonClosedCells"), format!("{}1(7)\n", h("cellSet"))).unwrap();
    // Not a checkMesh set: ignored.
    std::fs::write(sets.join("trees"), format!("{}1(0)\n", h("cellSet"))).unwrap();
    let o = cli(&["mesh", "--json", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let cm = &v["regions"][0]["checkmesh"];
    assert_eq!(cm["logs"][0]["file"], "log.checkMesh.air");
    assert_eq!(cm["logs"][0]["failed"], 1);
    let names: Vec<&str> = cm["sets"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["nonClosedCells", "skewFaces"]);
    assert_eq!(cm["sets"][0]["stale"], true, "cell 7 of a one-cell mesh");
    assert_eq!(cm["sets"][1]["first"], serde_json::json!([3]));
    let codes: Vec<&str> =
        v["regions"][0]["diagnostics"].as_array().unwrap().iter().map(|d| d["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"mesh.checkmesh") && codes.contains(&"mesh.stale-set"), "{codes:?}");
    // The native check finds no skew face in the cube: the log's finding counts, and says so.
    assert_eq!(v["warnings"], 2, "the skewness and the stale set: {}", v["regions"][0]["diagnostics"]);
    assert!(v["regions"][0]["diagnostics"].to_string().contains("doesn't find it"));

    // A log of another mesh (more cells) is left out, with a note; one for no region too.
    std::fs::write(
        dir.join("log.checkMesh.air"),
        "Build  : 12\nExec   : checkMesh -region air\n    cells:            5\n ***Zero or negative cell volume detected.\nFailed 1 mesh checks.\n",
    )
    .unwrap();
    std::fs::write(dir.join("log.checkMesh.solid"), "Exec   : checkMesh -region solid\nMesh OK.\n").unwrap();
    // A log with no `Exec` line (empty, cut, compressed): a note, not silently dropped.
    std::fs::write(dir.join("log.checkMesh.gz"), "").unwrap();
    let o = cli(&["mesh", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "the error was about another mesh: {o:?}");
    let text = stdout(&o);
    assert!(text.contains("not used: it checked a mesh of 5 cells, ? faces, ? points"), "{text}");
    assert!(text.contains("checked region `solid`, which this case doesn't have"), "{text}");
    assert!(text.contains("log.checkMesh.gz") && text.contains("no `Exec : checkMesh` line"), "{text}");
}

/// snappyHexMesh's layer summary, from a log in `log/` named for the region.
#[test]
fn mesh_reports_layer_coverage() {
    let dir = cube("mesh-layers", false);
    std::fs::create_dir_all(dir.join("log")).unwrap();
    let log = |cells: u32| {
        format!(
            "Build  : 12\nExec   : snappyHexMesh -overwrite\nShrinking and layer addition phase\n\npatch faces    layers avg thickness[m]\n                 near-wall overall\n----- -----    ------ --------- -------\nwalls 6 3 0.01 0.03\n\npatch faces    layers   overall thickness\n                   [m]       [%]\n----- -----    ------   ---       ---\nwalls 6 1.5 0.012 40\n\nLayer mesh : cells:{cells}  faces:6  points:8\nFinished meshing without any errors\n"
        )
    };
    std::fs::write(dir.join("log/log.snappyHexMesh.air"), log(1)).unwrap();
    let o = cli(&["mesh", "--json", dir.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let logs = &v["regions"][0]["snappyhexmesh"]["logs"];
    assert_eq!(logs[0]["file"], "log/log.snappyHexMesh.air");
    assert_eq!(logs[0]["layers"][0]["coverage"], 0.5);
    assert_eq!(v["warnings"], 1, "{}", v["regions"][0]["diagnostics"]);
    // A log of another mesh (meshed again since) is left out, with a note.
    std::fs::write(dir.join("log/log.snappyHexMesh.air"), log(500)).unwrap();
    let o = cli(&["mesh", dir.to_str().unwrap()]);
    let text = stdout(&o);
    assert!(
        text.contains("not used: it wrote a mesh of 500 cells, 6 faces, 8 points, constant/ has 1 cells"),
        "{text}"
    );
    assert!(!text.contains("mesh.layers"), "{text}");
    std::fs::remove_file(dir.join("log/log.snappyHexMesh.air")).unwrap();

    // Meshed as the default mesh and moved into `air` (no -region, no suffix): matched by size.
    std::fs::write(dir.join("log/log.snappyHexMesh"), log(1)).unwrap();
    let o = cli(&["mesh", "--json", dir.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["regions"][0]["snappyhexmesh"]["logs"][0]["file"], "log/log.snappyHexMesh", "{v}");
    std::fs::write(dir.join("log/log.snappyHexMesh"), log(500)).unwrap();
    let text = stdout(&cli(&["mesh", dir.to_str().unwrap()]));
    assert!(text.contains("no region's mesh has its size"), "{text}");

    // The same log in the case directory and in log/: the case directory's is used.
    std::fs::write(dir.join("log.snappyHexMesh"), log(1)).unwrap();
    let o = cli(&["mesh", "--json", dir.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["regions"][0]["snappyhexmesh"]["logs"][0]["file"], "log.snappyHexMesh", "{v}");
    assert_eq!(v["warnings"], 1);
    assert!(v["diagnostics"].to_string().contains("the case directory has a `log.snappyHexMesh` too"), "{v}");
    assert!(v["diagnostics"].as_array().unwrap().iter().all(|d| d["code"] == "mesh.snappyhexmesh"), "{v}");

    // A run that stopped before its layer phase is reported too.
    std::fs::remove_file(dir.join("log/log.snappyHexMesh")).unwrap();
    std::fs::write(
        dir.join("log.snappyHexMesh"),
        "Exec   : snappyHexMesh -overwrite\nRefined mesh : cells:1  faces:6  points:8\n--> FOAM FATAL ERROR:\ncannot find surface\n",
    )
    .unwrap();
    let text = stdout(&cli(&["mesh", dir.to_str().unwrap()]));
    assert!(text.contains("mesh.snappyhexmesh") && text.contains("cannot find surface"), "{text}");
    // One that failed before writing any mesh can't be placed by size: a warning for the case.
    std::fs::write(
        dir.join("log.snappyHexMesh"),
        "Exec   : snappyHexMesh -overwrite\n--> FOAM FATAL ERROR:\ncannot open snappyHexMeshDict\n",
    )
    .unwrap();
    let o = cli(&["mesh", "--json", dir.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let d = v["diagnostics"].to_string();
    assert!(d.contains("before it wrote a mesh") && d.contains("cannot open snappyHexMeshDict"), "{v}");
    assert_eq!(v["warnings"], 1, "{v}");
    // Or after printing the size of a mesh no region has (blockMesh's, before refining).
    std::fs::write(
        dir.join("log.snappyHexMesh"),
        "Exec   : snappyHexMesh -overwrite\nInitial mesh : cells:4000  faces:12800  points:4851\n--> FOAM FATAL ERROR:\nout of memory\n",
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout(&cli(&["mesh", "--json", dir.to_str().unwrap()]))).unwrap();
    assert!(v["diagnostics"].to_string().contains("with a mesh of 4000 cells"), "{v}");
    assert_eq!(v["warnings"], 1, "{v}");
}

/// A closed tetrahedron as ASCII STL, wound inward when `inward`.
fn tet_stl(inward: bool) -> String {
    let (a, b, c, d) = ("0 0 0", "1 0 0", "0 1 0", "0 0 1");
    let mut faces = vec![[a, c, b], [a, b, d], [b, c, d], [a, d, c]];
    if inward {
        for f in &mut faces {
            f.swap(1, 2);
        }
    }
    let mut s = String::from("solid tree 1\n");
    for [x, y, z] in faces {
        s.push_str(&format!("facet normal 0 0 0\nouter loop\nvertex {x}\nvertex {y}\nvertex {z}\nendloop\nendfacet\n"));
    }
    s + "endsolid\n"
}

#[test]
fn surface_checks_and_repairs_stl_files() {
    let dir = case("surface", "3600");
    let source = dir.join("tree.stl");
    std::fs::write(&source, tet_stl(true)).unwrap();
    let source = source.to_str().unwrap();

    // A file outside a case.
    let o = cli(&["surface", source]);
    assert_eq!(o.status.code(), Some(0), "warnings only");
    let out = stdout(&o);
    assert!(out.contains("surface.inward"), "{out}");
    assert!(out.contains("surface.renamed"), "{out}");
    assert!(out.ends_with("1 surface; 0 errors, 1 warning, 2 notes\n"), "{out}");

    // Imported, repaired and saved; then the case is clean.
    let case = dir.to_str().unwrap();
    // As JSON: a Windows path's backslashes are escaped.
    let import = format!("geometry.import={}", serde_json::json!({ "path": source }));
    let o = cli(&[
        "run",
        "--case",
        case,
        "--cmd",
        &import,
        "--cmd",
        "geometry.repair={\"file\": \"tree.stl\"}",
        "--cmd",
        "case.save",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("4 triangles turned over (1 closed part turned outward)"), "{}", stdout(&o));
    let o = cli(&["surface", case, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["warnings"], 0, "{v}");
    assert_eq!(v["surfaces"][0]["regions"], serde_json::json!(["tree1"]));
    assert_eq!(v["surfaces"][0]["closed_parts"], 1);

    // Errors: OpenFOAM can't read the file; bad arguments.
    std::fs::write(
        dir.join("constant/triSurface/bad.stl"),
        "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nendloop\nendfacet\nendsolid\n",
    )
    .unwrap();
    let o = cli(&["surface", case, "--file", "bad.stl"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("surface.openfoam"), "{}", stdout(&o));
    assert_eq!(cli(&["surface"]).status.code(), Some(2));
    assert_eq!(cli(&["surface", source, "--file", "x.stl"]).status.code(), Some(2));
    assert_eq!(cli(&["surface", "/nonexistent/x.stl"]).status.code(), Some(2));
}

/// `render --pick` with the style flags: they style the image, `select.pick` takes none of them.
#[test]
fn render_picks_with_the_style_flags() {
    let dir = cube("render-pick", false);
    let out = dir.join("shot.png");
    let o = cli(&[
        "render",
        "--edges",
        "--no-grid",
        "--no-origin",
        "--light",
        "--size",
        "64x48",
        "--pick",
        "32,24",
        "--out",
        out.to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(out.exists());
}
