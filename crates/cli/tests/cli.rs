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
