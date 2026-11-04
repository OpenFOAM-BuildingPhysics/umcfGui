use serde_json::{Value, json};
use umcf_core::MemFs;

use super::*;

const HEADER: &str = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class dictionary;\n}\n";

fn control() -> String {
    format!(
        "{HEADER}// run controls\napplication urbanMicroclimateFoam;\nstartFrom startTime;\nstartTime 0;\nstopAt endTime;\nendTime 3600;\ndeltaT 60;\nwriteInterval 600;\nregions\n{{\n    fluid (air);\n}}\n"
    )
}

fn boundary() -> String {
    "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class polyBoundaryMesh;\n}\n1\n(\n    inlet\n    {\n        type patch;\n        nFaces 1;\n        startFace 0;\n    }\n)\n".to_string()
}

fn fs() -> MemFs {
    MemFs::new()
        .with("system/controlDict", control())
        .with("system/air/fvSolution", format!("{HEADER}PIMPLE\n{{\n    nOuterCorrectors 1;\n}}\n"))
        .with("constant/air/polyMesh/boundary", boundary())
        .with(
            "0/air/T",
            "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ndimensions [0 0 0 1 0 0 0];\ninternalField uniform 300;\nboundaryField\n{\n    inlet { type fixedValue; value uniform 300; }\n}\n",
        )
}

fn session() -> Session {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs()), "mem", OpenOptions::default(), false).unwrap();
    s
}

fn text(s: &Session, rel: &str) -> String {
    String::from_utf8(s.fs().unwrap().read(rel, 1 << 20).unwrap().unwrap()).unwrap()
}

fn end_time(s: &Session) -> Option<String> {
    s.case().unwrap().dict(umcf_model::CONTROL_DICT).unwrap().value(&["endTime"])
}

#[test]
fn the_registry() {
    let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
    for id in &ids {
        let (family, verb) = id.split_once('.').unwrap();
        assert!(!family.is_empty() && !verb.is_empty() && !verb.contains('.'), "{id}");
        assert!(Session::command(id).is_some());
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), COMMANDS.len(), "ids are unique");
    assert!(matches!(Session::new().execute("case.nope", &Value::Null), Err(Error::Unknown(_))));
}

/// Every command, with no parameters, with or without a case, returns: it never panics.
#[test]
fn every_command_survives_empty_parameters() {
    for spec in COMMANDS {
        for params in [Value::Null, json!({})] {
            for mut s in [Session::with_backup(".umcfgui/backup/test"), session()] {
                let r = s.execute(spec.id, &params);
                assert!(!matches!(r, Err(Error::Panicked { .. })), "{} {params}: {r:?}", spec.id);
            }
        }
        // Parameters that aren't an object are refused, not misread.
        let r = session().execute(spec.id, &json!([1, 2]));
        assert!(!matches!(r, Err(Error::Panicked { .. })), "{}: {r:?}", spec.id);
    }
}

#[test]
fn disabled_commands_say_why() {
    let mut s = Session::new();
    assert_eq!(s.enabled("case.check"), Err("no case is open".to_string()));
    let e = s.execute("case.check", &Value::Null).unwrap_err();
    assert_eq!(e.to_string(), "case.check can't run: no case is open");
    let s = session();
    assert_eq!(s.enabled("edit.undo"), Err("nothing to undo".to_string()));
    assert_eq!(s.enabled("case.check"), Ok(()));
}

#[test]
fn edits_undo_redo_and_the_journal() {
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200, "writeInterval": 1200})).unwrap();
    assert_eq!(end_time(&s).as_deref(), Some("7200"));
    assert_eq!(s.case().unwrap().controls.end_time.as_deref(), Some("7200"), "the typed controls follow the edit");
    s.execute("dict.set", &json!({"file": "system/air/fvSolution", "path": "PIMPLE/nOuterCorrectors", "value": 3}))
        .unwrap();
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(
        s.case().unwrap().dict("system/air/fvSolution").unwrap().value(&["PIMPLE", "nOuterCorrectors"]).as_deref(),
        Some("1")
    );
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(end_time(&s).as_deref(), Some("3600"));
    assert!(s.case().unwrap().changed().is_empty(), "back to what is on disk");
    assert!(matches!(s.execute("edit.undo", &Value::Null), Err(Error::Disabled { .. })));
    s.execute("edit.redo", &Value::Null).unwrap();
    assert_eq!(end_time(&s).as_deref(), Some("7200"));
    // A new edit drops what could be redone.
    s.execute("dict.remove", &json!({"file": "system/controlDict", "path": ["writeInterval"]})).unwrap();
    assert!(!s.can_redo());

    let journal: Vec<&str> = s.journal().iter().map(|e| e["command"].as_str().unwrap()).collect();
    assert_eq!(journal, ["control.set", "dict.set", "edit.undo", "edit.undo", "edit.redo", "dict.remove"]);
    assert_eq!(s.journal()[0]["params"], json!({"endTime": 7200, "writeInterval": 1200}));

    // Replaying the journal on the same case gives the same model.
    let mut again = session();
    for e in s.journal() {
        again.execute(e["command"].as_str().unwrap(), &e["params"]).unwrap();
    }
    assert_eq!(
        again.case().unwrap().dict("system/controlDict").unwrap().to_string(),
        s.case().unwrap().dict("system/controlDict").unwrap().to_string()
    );
}

#[test]
fn a_failing_command_changes_nothing() {
    let mut s = session();
    // The first entry is fine, the second isn't: the first must not stay.
    let r = s.execute("control.set", &json!({"endTime": 9000, "bad key": 1}));
    assert!(matches!(r, Err(Error::Params(_))), "{r:?}");
    assert_eq!(end_time(&s).as_deref(), Some("3600"));
    assert!(!s.can_undo());
    assert!(s.journal().is_empty());
    for bad in [
        json!({"endTime": null}),
        json!({"endTime": {"a": 1}}),
        json!({"endTime": ""}),
        json!({"endTime;": 1}),
        json!({"a/b": 1}),
    ] {
        assert!(s.execute("control.set", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("dict.set", &json!({"file": "system/nothere", "path": "a", "value": 1})).is_err());
    assert!(s.execute("dict.set", &json!({"file": "system/air/fvSolution", "path": "a//b", "value": 1})).is_err());
    assert!(s.execute("dict.set", &json!({"file": "system/air/fvSolution", "path": "a", "value": 1, "x": 1})).is_err());
    assert!(s.case().unwrap().changed().is_empty());
}

#[test]
fn values_cannot_break_out_of_their_entry() {
    let mut s = session();
    for v in ["1; deltaT 5", "1 }", "{ a 1; } extra 2"] {
        let r = s.execute("control.set", &json!({ "endTime": v }));
        assert!(r.is_err(), "{v}: {r:?}");
    }
    assert!(s.case().unwrap().changed().is_empty());
    // A list, a dictionary and a word are fine.
    s.execute("dict.set", &json!({"file": "system/air/fvSolution", "path": "g", "value": [0, 0, -9.81]})).unwrap();
    s.execute("dict.set", &json!({"file": "system/air/fvSolution", "path": "d", "value": "{ type fixedValue; }"}))
        .unwrap();
    let doc = s.case().unwrap().dict("system/air/fvSolution").unwrap();
    assert_eq!(doc.value(&["g"]).as_deref(), Some("(0 0 -9.81)"));
    assert_eq!(doc.value(&["d", "type"]).as_deref(), Some("fixedValue"));
}

#[test]
fn keys_cannot_break_the_file() {
    let mut s = session();
    for k in ["a)", "(", ")", "a(b", "#include", "#codeStream", "$endTime", "a[0]", "'a'", "a;b", "", "a b"] {
        let r = s.execute("control.set", &json!({ k: 1 }));
        assert!(matches!(r, Err(Error::Params(_))), "{k:?}: {r:?}");
        let r = s.execute("dict.set", &json!({"file": "system/controlDict", "path": ["regions", k], "value": 1}));
        assert!(matches!(r, Err(Error::Params(_))), "{k:?}: {r:?}");
    }
    assert!(s.case().unwrap().changed().is_empty());
    // OpenFOAM keys with brackets, commas and dots are fine.
    let file = "system/air/fvSolution";
    for k in ["div(phi,U)", "laplacian((rho*nuEff),U)", "p_rgh.*", "U|k", "(U|k|epsilon)", "(U|T)Final"] {
        s.execute("dict.set", &json!({"file": file, "path": ["solvers", k], "value": "PCG"})).unwrap();
        let text = s.case().unwrap().dict(file).unwrap().to_string();
        let back = umcf_foam_dict::Document::parse(&text).unwrap();
        assert_eq!(back.value(&["solvers", k]).as_deref(), Some("PCG"), "{k}: {text}");
    }
}

/// A pattern key already in the file is reached by its text without the quotes.
#[test]
fn pattern_keys_already_in_the_file() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let fs = fs().with(
        "system/air/fvSolution",
        format!("{HEADER}solvers\n{{\n    \"(U|k)\"\n    {{\n        solver PBiCG;\n    }}\n}}\n"),
    );
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let file = "system/air/fvSolution";
    s.execute("dict.set", &json!({"file": file, "path": ["solvers", "(U|k)", "solver"], "value": "smoothSolver"}))
        .unwrap();
    let text = s.case().unwrap().dict(file).unwrap().to_string();
    assert!(text.contains("\"(U|k)\"\n    {\n        solver smoothSolver;"), "{text}");
}

#[test]
fn the_message_shows_what_was_stored() {
    let mut s = session();
    let out = s.execute("control.set", &json!({"endTime": "7200 // two hours"})).unwrap();
    assert_eq!(out.message, "system/controlDict: endTime 7200");
}

#[test]
fn opening_in_memory_protects_unsaved_changes() {
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    assert!(matches!(s.open(Box::new(fs()), "other", OpenOptions::default(), false), Err(Error::Failed(_))));
    assert_eq!(s.location(), Some("mem"));
    s.open(Box::new(fs()), "other", OpenOptions::default(), true).unwrap();
    assert_eq!(s.location(), Some("other"));
}

/// A command that replaced the case and then failed keeps the new case, without the old
/// case's history (undoing would put the old model onto the new files).
#[test]
fn a_failure_after_replacing_the_case_drops_the_history() {
    fn reopen(s: &mut Session, _: &Value) -> Result<Outcome, Error> {
        s.open(Box::new(fs()), "other", OpenOptions::default(), true)?;
        Err(Error::Failed("after the open".to_string()))
    }
    let spec = CommandSpec {
        id: "test.reopen",
        label: "",
        menu: "",
        shortcut: None,
        params: "",
        enabled: needs_case,
        run: reopen,
        journal: true,
        undoable: false,
    };
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    s.execute("case.save", &Value::Null).unwrap();
    assert!(s.run_spec(&spec, &Value::Null).is_err());
    assert_eq!(s.location(), Some("other"));
    assert!(!s.can_undo() && !s.can_redo());
    assert!(s.journal().iter().all(|e| e["command"] != "test.reopen"));
}

#[test]
fn deep_lists_are_refused() {
    let mut v = json!(1);
    for _ in 0..100 {
        v = json!([v]);
    }
    let r = session().execute("control.set", &json!({ "x": v }));
    assert!(matches!(r, Err(Error::Params(_))), "{r:?}");
}

#[test]
fn save_writes_the_changes_and_keeps_a_backup() {
    let mut s = session();
    assert_eq!(s.execute("case.save", &Value::Null).unwrap().message, "nothing to save");
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    let info = s.execute("case.info", &Value::Null).unwrap();
    assert_eq!(info.data["changed"], json!(["system/controlDict"]));
    let out = s.execute("case.save", &Value::Null).unwrap();
    assert_eq!(out.data["saved"], json!(["system/controlDict"]));
    let saved = text(&s, "system/controlDict");
    assert!(saved.contains("endTime 7200;") && saved.contains("// run controls"), "{saved}");
    assert_eq!(text(&s, ".umcfgui/backup/test/system/controlDict"), control());
    assert!(s.case().unwrap().changed().is_empty());
}

#[test]
fn check_reports_diagnostics() {
    let mut s = session();
    let clean = s.execute("case.check", &Value::Null).unwrap();
    assert_eq!(clean.data["errors"], json!(0), "{}", clean.data);
    s.execute("control.set", &json!({"endTime": -1})).unwrap();
    let out = s.execute("case.check", &Value::Null).unwrap();
    assert!(out.data["errors"].as_u64().unwrap() >= 1, "{}", out.data);
    let d = &out.data["diagnostics"][0];
    assert_eq!(d["level"], "error");
    assert_eq!(d["path"], "system/controlDict");
    assert!(out.message.starts_with("1 error"), "{}", out.message);
}

/// A command that panics leaves the case and the history as they were.
#[test]
fn a_panic_is_caught_and_the_case_restored() {
    fn boom(s: &mut Session, _: &Value) -> Result<Outcome, Error> {
        s.opened_mut()?.case.set("system/controlDict", &["endTime"], "1")?;
        s.undo.clear();
        let v: Vec<u8> = Vec::new();
        #[allow(clippy::indexing_slicing)]
        let x = v[1];
        Ok(Outcome::new(format!("{x}"), Value::Null))
    }
    let spec = CommandSpec {
        id: "test.boom",
        label: "",
        menu: "",
        shortcut: None,
        params: "",
        enabled: needs_case,
        run: boom,
        journal: true,
        undoable: true,
    };
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    let r = s.run_spec(&spec, &Value::Null);
    match r {
        Err(Error::Panicked { id, message }) => {
            assert_eq!(id, "test.boom");
            assert!(message.contains("index out of bounds"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(end_time(&s).as_deref(), Some("7200"));
    assert!(s.can_undo(), "the history is back too");
    assert_eq!(s.journal().len(), 1);
    // The session still works.
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(end_time(&s).as_deref(), Some("3600"));
}

#[test]
fn undo_keeps_a_bounded_history() {
    let mut s = session();
    for k in 0..UNDO_DEPTH + 20 {
        s.execute("control.set", &json!({ "endTime": k })).unwrap();
    }
    let mut steps = 0;
    while s.can_undo() {
        s.execute("edit.undo", &Value::Null).unwrap();
        steps += 1;
    }
    assert_eq!(steps, UNDO_DEPTH);
    assert_eq!(end_time(&s).as_deref(), Some("19"));
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, bytes) in [
        ("system/controlDict", control()),
        ("constant/air/polyMesh/boundary", boundary()),
        (
            "0/air/T",
            "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ninternalField uniform 300;\nboundaryField\n{\n    inlet { type zeroGradient; }\n}\n".to_string(),
        ),
    ] {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    dir
}

#[test]
fn opening_from_disk_protects_unsaved_changes() {
    let a = scratch("a");
    let b = scratch("b");
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let out = s.execute("case.open", &json!({ "path": a.to_str().unwrap() })).unwrap();
    assert!(out.message.contains("1 regions"), "{}", out.message);
    assert_eq!(out.data["regions"][0]["name"], "air");
    s.execute("control.set", &json!({"endTime": 10})).unwrap();
    let r = s.execute("case.open", &json!({ "path": b.to_str().unwrap() }));
    assert!(matches!(&r, Err(Error::Failed(m)) if m.contains("unsaved")), "{r:?}");
    assert_eq!(s.location(), a.to_str(), "still the first case");
    assert_eq!(end_time(&s).as_deref(), Some("10"));
    assert!(s.execute("case.open", &json!({ "path": a.join("nothere").to_str().unwrap() })).is_err());
    s.execute("case.open", &json!({ "path": b.to_str().unwrap(), "discard": true, "fields": false })).unwrap();
    assert_eq!(s.location(), b.to_str());
    assert!(!s.can_undo(), "a new case starts a new history");
    let _ = std::fs::remove_dir_all(a);
    let _ = std::fs::remove_dir_all(b);
}
