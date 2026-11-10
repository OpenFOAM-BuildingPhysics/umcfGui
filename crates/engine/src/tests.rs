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
    let mut fs = MemFs::new();
    // The fields the solver reads in a fluid region, besides T.
    for f in ["U", "w", "gcr", "p_rgh", "p"] {
        let class = if f == "U" { "volVectorField" } else { "volScalarField" };
        fs.insert(
            &format!("0/air/{f}"),
            format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\ninternalField uniform 0;\nboundaryField\n{{\n    inlet {{ type zeroGradient; }}\n}}\n"),
        );
    }
    fs.insert("constant/air/g", "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class uniformDimensionedVectorField;\n}\nvalue (0 0 -9.81);\n");
    fs.with("system/controlDict", control())
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
    assert!(s.journal().is_empty(), "a new case starts a new journal");
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
        reads_only: false,
    };
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    s.execute("case.save", &Value::Null).unwrap();
    assert!(s.run_spec(&spec, &Value::Null).is_err());
    assert_eq!(s.location(), Some("other"));
    assert!(!s.can_undo() && !s.can_redo());
    assert!(s.journal().iter().all(|e| e["command"] != "test.reopen"));
}

/// Opening the same location again and then failing: the history and the journal still
/// belong to the model that is open.
#[test]
fn a_failure_after_reopening_the_same_case() {
    fn reopen(s: &mut Session, _: &Value) -> Result<Outcome, Error> {
        s.open(Box::new(fs()), "mem", OpenOptions::default(), true)?;
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
        reads_only: false,
    };
    let mut s = session();
    s.execute("control.set", &json!({"endTime": 7200})).unwrap();
    assert!(s.run_spec(&spec, &Value::Null).is_err());
    assert_eq!(end_time(&s).as_deref(), Some("3600"), "the reopened case from disk");
    assert!(!s.can_undo() && s.journal().is_empty());
    assert!(s.case().unwrap().changed().is_empty(), "no unsaved edit missing from the journal");
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
        reads_only: false,
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

/// A unit-cube mesh: 8 points, 6 boundary faces of one cell, all in one patch `inlet`.
fn cube_mesh(fs: &mut MemFs, dir: &str) {
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    fs.insert(
        &format!("{dir}/points"),
        format!(
            "{}8\n(\n(0 0 0)\n(1 0 0)\n(1 1 0)\n(0 1 0)\n(0 0 1)\n(1 0 1)\n(1 1 1)\n(0 1 1)\n)\n",
            h("vectorField")
        ),
    );
    fs.insert(
        &format!("{dir}/faces"),
        format!("{}6\n(\n4(0 3 2 1)\n4(4 5 6 7)\n4(0 1 5 4)\n4(3 7 6 2)\n4(0 4 7 3)\n4(1 2 6 5)\n)\n", h("faceList")),
    );
    fs.insert(&format!("{dir}/owner"), format!("{}6\n(\n0 0 0 0 0 0\n)\n", h("labelList")));
    fs.insert(&format!("{dir}/neighbour"), format!("{}0\n(\n)\n", h("labelList")));
    fs.insert(
        &format!("{dir}/boundary"),
        format!(
            "{}1\n(\n    inlet\n    {{\n        type patch;\n        nFaces 6;\n        startFace 0;\n    }}\n)\n",
            h("polyBoundaryMesh")
        ),
    );
}

/// A region whose mesh doesn't read is that region's error; the next region is still checked.
#[test]
fn mesh_check_goes_on_after_an_unreadable_region() {
    assert_eq!(Session::new().enabled("mesh.check"), Err("no case is open".to_string()));
    let mut fs = fs();
    fs.insert("system/controlDict", control().replace("    fluid (air);\n", "    fluid (air);\n    solid (wall);\n"));
    // air: only its boundary file and an owner header noting its size (unreadable); wall: a
    // whole cube.
    cube_mesh(&mut fs, "constant/wall/polyMesh");
    fs.insert(
        "constant/air/polyMesh/owner",
        "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class labelList;\n    note \"nPoints:8 nCells:1 nFaces:6 nInternalFaces:0\";\n    object owner;\n}\n6\n(\n0 0 0 0 0 0\n)\n",
    );
    fs.insert(
        "0/wall/Ts",
        "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ninternalField uniform 300;\nboundaryField\n{\n    inlet { type zeroGradient; }\n}\n",
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let out = s.execute("mesh.check", &Value::Null).unwrap();
    // Every quality pass of the readable region counted; the files it expected to read named.
    let now = s.canceller().progress().get();
    assert!(now.total > 0 && now.done == now.total && now.expected > 0, "{now:?}");
    assert_eq!(out.data["errors"], json!(1), "{}", out.data);
    let regions = out.data["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0]["region"], "air");
    assert!(regions[0]["quality"].is_null());
    assert_eq!(regions[0]["diagnostics"][0]["code"], "mesh.unreadable");
    assert_eq!(regions[1]["region"], "wall");
    assert_eq!(regions[1]["quality"]["cells"], json!(1));
    assert!(regions[1]["diagnostics"].as_array().unwrap().is_empty());
    // One region by name; an unknown one is a parameter error.
    let one = s.execute("mesh.check", &json!({"region": "wall"})).unwrap();
    assert_eq!(one.data["errors"], json!(0));
    assert!(matches!(s.execute("mesh.check", &json!({"region": "nope"})), Err(Error::Params(_))));
}

/// A unit cube as STL: outward normals, or inside out.
fn cube_stl(format: umcf_geom::stl::Format, inward: bool) -> Vec<u8> {
    use umcf_geom::stl::{Stl, Triangle};
    let p = |i: usize| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32];
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut triangles = Vec::new();
    for q in quads {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            let mut t = Triangle { vertices: t.map(p), normal: [0.0; 3], solid: 0 };
            if inward {
                t.flip();
            }
            triangles.push(t);
        }
    }
    let solids = if format == umcf_geom::stl::Format::Ascii { vec!["box".to_string()] } else { Vec::new() };
    Stl { format, solids, triangles, notes: Vec::new(), openfoam: None, header_solid: false }.to_bytes()
}

fn codes(out: &Outcome) -> Vec<String> {
    out.data["diagnostics"].as_array().unwrap().iter().map(|d| d["code"].as_str().unwrap().to_string()).collect()
}

#[test]
fn geometry_import_check_repair_and_save() {
    use umcf_geom::stl::Format;
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-stl", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Binary, inside out, and with a header OpenFOAM takes for ASCII.
    let mut bytes = cube_stl(Format::Binary, true);
    bytes[..6].copy_from_slice(b"solid ");
    let source = dir.join("Box.stl");
    std::fs::write(&source, &bytes).unwrap();
    let source = source.to_str().unwrap();

    // Checked where it is, without a case.
    let out = Session::new().execute("geometry.check", &json!({ "path": source })).unwrap();
    assert_eq!(codes(&out), ["surface.openfoam", "surface.summary", "surface.inward"]);
    assert_eq!(out.data["errors"], 1);

    let mut s = session();
    let out = s.execute("geometry.import", &json!({ "path": source, "name": "box.stl" })).unwrap();
    assert!(out.message.starts_with("constant/triSurface/box.stl: imported from"), "{}", out.message);
    assert!(out.message.contains("written again"), "{}", out.message);
    assert_eq!(codes(&out), ["surface.summary", "surface.inward"], "written again, OpenFOAM reads it");
    assert_eq!(out.data["surface"]["inward"]["count"], 12);
    assert_eq!(s.case().unwrap().changed(), ["constant/triSurface/box.stl"]);
    let e = s.execute("geometry.import", &json!({ "path": source, "name": "box.stl" })).unwrap_err();
    assert!(e.to_string().contains("already there"), "{e}");

    // geometry.check sees the surface before it is saved.
    let out = s.execute("geometry.check", &Value::Null).unwrap();
    assert_eq!(out.data["surfaces"].as_array().unwrap().len(), 1);
    assert_eq!(out.message, "1 surface; 0 errors, 1 warning, 1 note");

    let out = s.execute("geometry.repair", &json!({ "file": "box.stl" })).unwrap();
    assert_eq!(
        out.message,
        "constant/triSurface/box.stl: 0 triangles removed (0 without area, 0 copies, 0 not finite); 12 triangles turned over (1 closed part turned outward); 0 errors, 0 warnings, 1 note"
    );
    let out = s.execute("geometry.repair", &json!({ "file": "box.stl" })).unwrap();
    assert!(out.message.contains("nothing to change"), "{}", out.message);

    // Undo goes back through the repairs to the import, and before it.
    s.execute("edit.undo", &Value::Null).unwrap();
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(
        codes(&s.execute("geometry.check", &json!({ "file": "box.stl" })).unwrap()),
        ["surface.summary", "surface.inward"]
    );
    s.execute("edit.undo", &Value::Null).unwrap();
    assert!(s.case().unwrap().changed().is_empty());
    assert_eq!(s.execute("geometry.check", &Value::Null).unwrap().message, "no STL surface in constant/triSurface");
    s.execute("edit.redo", &Value::Null).unwrap();
    s.execute("edit.redo", &Value::Null).unwrap();

    let out = s.execute("geometry.scale", &json!({ "file": "box.stl", "factor": 1000 })).unwrap();
    assert_eq!(out.data["surface"]["bbox"]["max"], json!([1000.0, 1000.0, 1000.0]));
    assert!(s.execute("geometry.scale", &json!({ "file": "box.stl", "factor": -1 })).is_err());
    assert!(s.execute("geometry.scale", &json!({ "file": "box.stl" })).is_err());

    let saved = s.execute("case.save", &Value::Null).unwrap();
    assert_eq!(saved.data["saved"], json!(["constant/triSurface/box.stl"]));
    let on_disk = s.fs().unwrap().read("constant/triSurface/box.stl", 1 << 20).unwrap().unwrap();
    assert_eq!(on_disk.len(), 84 + 50 * 12, "still binary");
    assert!(s.case().unwrap().changed().is_empty());

    // Replacing a saved surface keeps a backup of it at the next save.
    let ascii = dir.join("box.stl.gz");
    std::fs::write(&ascii, cube_stl(Format::Ascii, false)).unwrap();
    let out = s
        .execute("geometry.import", &json!({ "path": ascii.to_str().unwrap(), "name": "box.stl", "replace": true }))
        .unwrap();
    assert_eq!(out.data["rewritten"], json!([]), "copied as it is");
    s.execute("case.save", &Value::Null).unwrap();
    assert_eq!(
        s.fs().unwrap().read(".umcfgui/backup/test/constant/triSurface/box.stl", 1 << 20).unwrap().unwrap(),
        on_disk
    );

    // A compressed name gets compressed bytes.
    let out = s.execute("geometry.import", &json!({ "path": ascii.to_str().unwrap() })).unwrap();
    assert_eq!(out.data["rewritten"], json!(["compressed"]));
    let gz = s.case().unwrap().file(s.fs().unwrap(), "constant/triSurface/box.stl.gz").unwrap().unwrap();
    assert!(gz.starts_with(&[0x1f, 0x8b]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn geometry_parameters_are_checked() {
    let mut s = session();
    for (id, p) in [
        ("geometry.check", json!({ "file": "../x.stl" })),
        ("geometry.check", json!({ "file": "x.obj" })),
        ("geometry.check", json!({ "file": ".x.stl" })),
        ("geometry.check", json!({ "file": "../x.STL" })),
        ("geometry.check", json!({ "file": "a/b.STL" })),
        ("geometry.check", json!({ "file": "..\\x.STL" })),
        ("geometry.repair", json!({ "file": "x.STL" })),
        ("geometry.check", json!({ "file": "a.stl", "path": "/tmp/a.stl" })),
        ("geometry.repair", json!({ "file": "a.stl", "weld": 0 })),
        ("geometry.repair", json!({ "file": "a.stl", "weld": "1" })),
        ("geometry.import", json!({ "path": "/tmp/a.stl", "scale": -2 })),
        ("geometry.import", json!({ "path": "/" })),
    ] {
        assert!(matches!(s.execute(id, &p), Err(Error::Params(_))), "{id} {p}");
    }
    for (id, p) in [
        ("geometry.check", json!({ "file": "none.stl" })),
        ("geometry.repair", json!({ "file": "none.stl" })),
        ("geometry.import", json!({ "path": "/nonexistent/a.stl" })),
    ] {
        assert!(matches!(s.execute(id, &p), Err(Error::Failed(_))), "{id} {p}");
    }
    assert!(Session::new().execute("geometry.check", &Value::Null).is_err(), "no case and no path");
}

#[test]
fn geometry_names_follow_openfoams_extensions() {
    use umcf_geom::stl::Format;
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-ext", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let windows = dir.join("Building.STL");
    std::fs::write(&windows, cube_stl(Format::Ascii, false)).unwrap();
    let mut s = session();
    // `.STL` comes in as `.stl`, which OpenFOAM knows.
    let out = s.execute("geometry.import", &json!({ "path": windows.to_str().unwrap() })).unwrap();
    assert_eq!(out.data["file"], "constant/triSurface/Building.stl");
    assert!(matches!(
        s.execute("geometry.import", &json!({ "path": windows.to_str().unwrap(), "name": "b.STL" })),
        Err(Error::Params(_))
    ));
    // An ASCII file as `.stlb` is written binary: OpenFOAM reads `.stlb` as binary.
    let out = s.execute("geometry.import", &json!({ "path": windows.to_str().unwrap(), "name": "b.stlb" })).unwrap();
    assert!(out.message.contains("written again"), "{}", out.message);
    let b = s.case().unwrap().file(s.fs().unwrap(), "constant/triSurface/b.stlb").unwrap().unwrap();
    assert_eq!(b.len(), 84 + 50 * 12);
    // Scaling past what STL holds is refused.
    let e = s.execute("geometry.scale", &json!({ "file": "b.stlb", "factor": 1e39 })).unwrap_err();
    assert!(matches!(e, Error::Params(_)), "{e}");
    let _ = std::fs::remove_dir_all(&dir);

    // A `.STL` already in the case is reported.
    let mut fs = fs();
    fs.insert("constant/triSurface/Old.STL", cube_stl(Format::Ascii, false));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let out = s.execute("geometry.check", &Value::Null).unwrap();
    assert_eq!(codes(&out), ["surface.extension", "surface.summary"]);
    assert_eq!(codes(&s.execute("geometry.check", &json!({ "file": "Old.STL" })).unwrap())[0], "surface.extension");
}

/// A case whose air region is a unit cube: patches ground (bottom), sky (top), walls; and an
/// ASCII STL box in constant/triSurface.
fn scene_session() -> Session {
    let mut fs = fs();
    cube_mesh(&mut fs, "constant/air/polyMesh");
    let h = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class polyBoundaryMesh;\n}\n";
    let patch = |n: &str, count: u32, start: u32| {
        format!("    {n}\n    {{\n        type wall;\n        nFaces {count};\n        startFace {start};\n    }}\n")
    };
    fs.insert(
        "constant/air/polyMesh/boundary",
        format!("{h}3\n(\n{}{}{})\n", patch("ground", 1, 0), patch("sky", 1, 1), patch("walls", 4, 2)),
    );
    fs.insert("constant/triSurface/box.stl", cube_stl(umcf_geom::stl::Format::Ascii, false));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    s
}

fn base64_decode(s: &str) -> Vec<u8> {
    let abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let v: Vec<u32> = s.bytes().filter(|b| *b != b'=').map(|b| abc.find(b as char).unwrap() as u32).collect();
    let mut out = Vec::new();
    for c in v.chunks(4) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, x)| n | (x << (18 - 6 * i)));
        out.extend_from_slice(&n.to_be_bytes()[1..c.len()]);
    }
    out
}

#[test]
fn scene_snapshots_draw_the_mesh_or_the_surfaces() {
    assert_eq!(Session::new().enabled("scene.snapshot"), Err("no case is open".to_string()));
    let mut s = scene_session();
    let out = s.execute("scene.snapshot", &json!({ "width": 64, "height": 48, "view": "top" })).unwrap();
    assert_eq!(out.message, "64 × 48 of mesh air (12 triangles)");
    let png = base64_decode(out.data["png"].as_str().unwrap());
    assert_eq!(png.len(), out.data["bytes"].as_u64().unwrap() as usize);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        out.data["objects"],
        json!([{ "region": "air", "triangles": 12, "classes": ["ground", "sky", "walls"] }])
    );
    assert_eq!(out.data["camera"]["elevation"], json!(90.0));
    // Only the surfaces, or both.
    let out = s.execute("scene.snapshot", &json!({ "surfaces": ["box.stl"], "width": 8, "height": 8 })).unwrap();
    assert_eq!(out.data["objects"][0]["surface"], "constant/triSurface/box.stl");
    assert_eq!(out.data["objects"][0]["classes"], json!(["box"]));
    let out = s
        .execute("scene.snapshot", &json!({ "regions": ["air"], "surfaces": ["box.stl"], "width": 8, "height": 8 }))
        .unwrap();
    assert_eq!(out.data["objects"].as_array().unwrap().len(), 2);
    // Bad parameters.
    for (p, says) in [
        (json!({ "view": "Top" }), "no view `Top`"),
        (json!({ "width": 0 }), "from 1 to 8192"),
        (json!({ "width": 1.5 }), "whole number"),
        (json!({ "width": 8192, "height": 4096 }), "more than 16777216 pixels"),
        (json!({ "regions": ["water"] }), "no region `water`"),
        (json!({ "regions": "air" }), "a list of names"),
        (json!({ "surfaces": ["../x.stl"] }), "not a surface of the case"),
        (json!({ "surfaces": ["none.stl"] }), "there is no surface"),
        (json!({ "elevation": 91 }), "-90 to 90"),
        (json!({ "zoom": 0 }), "positive"),
        (json!({ "colour_by": "field" }), "patch or object"),
        (json!({ "colour": "patch" }), "colour"),
    ] {
        let e = s.execute("scene.snapshot", &p).unwrap_err().to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
    // Snapshots are not edits.
    assert!(s.journal().is_empty());
}

#[test]
fn a_cut_shows_the_cells_a_plane_crosses() {
    let mut s = scene_session();
    let cut = |x: f64| json!([{ "region": "air", "point": [x, 0.5, 0.5], "normal": [1, 0, 0] }]);
    let out = s.execute("scene.snapshot", &json!({ "width": 32, "height": 32, "cuts": cut(0.5) })).unwrap();
    // The region's boundary and its cut: the one cell, its six faces.
    assert_eq!(
        out.data["objects"],
        json!([
            { "region": "air", "triangles": 12, "classes": ["ground", "sky", "walls"] },
            { "cut": "air", "triangles": 12, "classes": ["cells"] },
        ])
    );
    // Only the cut; moved; past the mesh: none of the cell.
    let out = s.execute("scene.snapshot", &json!({ "regions": [], "cuts": cut(0.25), "width": 8, "height": 8 }));
    let out = out.unwrap();
    assert_eq!(out.data["objects"], json!([{ "cut": "air", "triangles": 12, "classes": ["cells"] }]));
    let out =
        s.execute("scene.snapshot", &json!({ "regions": [], "cuts": cut(5.0), "width": 8, "height": 8 })).unwrap();
    assert_eq!(out.data["objects"][0]["triangles"], 0);
    // Its own display entry; a region's entry doesn't reach it.
    let display = json!([{ "region": "air", "mode": "hidden" }, { "cut": "air", "mode": "wireframe", "opacity": 0.5 }]);
    s.execute("scene.snapshot", &json!({ "cuts": cut(0.5), "display": display, "width": 8, "height": 8 })).unwrap();
    // A pick goes through the cut to the patch behind it.
    let pick = json!({ "cuts": cut(0.5), "ray": { "origin": [0.5, 0.5, -3], "direction": [0, 0, 1] } });
    let out = s.execute("select.pick", &pick).unwrap();
    assert_eq!(out.data["picked"]["patch"], "ground");
    // Refused.
    for (cuts, says) in [
        (json!({ "region": "air" }), "is a list"),
        (json!([{ "region": "air", "point": [0, 0, 0] }]), "`normal` is [x, y, z]"),
        (json!([{ "region": "air", "point": [0, 0], "normal": [1, 0, 0] }]), "`point` is three numbers"),
        (json!([{ "region": "air", "point": [0, 0, 0], "normal": [0, 0, 0] }]), "zero length"),
        (json!([{ "region": "air", "point": [0, 0, 0], "normal": [1, 0, 0], "at": 1 }]), "unknown `at`"),
        (json!([{ "region": "water", "point": [0, 0, 0], "normal": [1, 0, 0] }]), "no mesh to cut"),
        (
            json!([{ "region": "air", "point": [0, 0, 0], "normal": [1, 0, 0] }, { "region": "air", "point": [0, 0, 0], "normal": [0, 1, 0] }]),
            "cut twice",
        ),
    ] {
        let e = s.execute("scene.snapshot", &json!({ "cuts": cuts })).unwrap_err().to_string();
        assert!(e.contains(says), "{cuts}: {e}");
    }
    let e = s.execute("scene.snapshot", &json!({ "display": [{ "cut": "air", "patch": "sky", "mode": "hidden" }] }));
    assert!(e.is_err());
}

/// A case on `MemFs` that counts the reads of one file.
struct Counting {
    fs: MemFs,
    file: &'static str,
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl umcf_core::CaseFs for Counting {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, umcf_core::FsError> {
        if rel == self.file {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.fs.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.fs.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.fs.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, umcf_core::FsError> {
        self.fs.list(rel)
    }
}

impl umcf_core::CaseWrite for Counting {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), umcf_core::FsError> {
        self.fs.write(rel, bytes)
    }
}

#[test]
fn a_region_s_whole_mesh_is_read_once_for_its_cuts() {
    let mut fs = fs();
    cube_mesh(&mut fs, "constant/air/polyMesh");
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = Counting { fs, file: "constant/air/polyMesh/neighbour", reads: std::sync::Arc::clone(&reads) };
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(counting), "mem", OpenOptions::default(), false).unwrap();
    let at = |x: f64| json!({ "width": 8, "height": 8, "cuts": [{ "region": "air", "point": [x, 0.5, 0.5], "normal": [1, 0, 0] }] });
    let before = reads.load(std::sync::atomic::Ordering::Relaxed);
    // There and back, the region alone, then on: one read.
    for x in [0.5, 0.25, 0.5, 0.75] {
        s.execute("scene.snapshot", &at(x)).unwrap();
    }
    s.execute("scene.snapshot", &json!({ "width": 8, "height": 8 })).unwrap();
    s.execute("scene.snapshot", &at(0.6)).unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed) - before, 1);
    // A patch renamed: the cut and its mesh as they were.
    s.execute("patch.set", &json!({ "name": "west", "patches": [{ "region": "air", "patch": "inlet" }] })).unwrap();
    s.execute("scene.snapshot", &at(0.4)).unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed) - before, 1);
    // A rectangle selects through the cut.
    let rect = json!({ "view": "top", "width": 50, "height": 50, "rect": [0, 0, 50, 50], "cuts": at(0.5)["cuts"] });
    let out = s.execute("select.rect", &rect).unwrap();
    assert_eq!(out.data["in"], 1, "{}", out.data);
}

#[test]
fn picking_selects_patches_and_lists_what_is_behind() {
    let mut s = scene_session();
    let top = json!({ "view": "top", "width": 100, "height": 100, "pixel": [50, 50] });
    let out = s.execute("select.pick", &top).unwrap();
    assert_eq!(out.data["picked"], json!({ "region": "air", "patch": "sky" }));
    let hits: Vec<&str> = out.data["hits"].as_array().unwrap().iter().map(|h| h["patch"].as_str().unwrap()).collect();
    assert_eq!(hits, ["sky", "ground"]);
    assert_eq!(out.data["hits"][0]["face"], 1);
    assert_eq!(out.message, "patch sky of region air (2 under the cursor); 1 selected");
    assert_eq!(s.selection(), [Sel::Patch { region: "air".into(), patch: "sky".into() }]);

    // "Select other": the one behind, added.
    let mut behind = top.clone();
    behind["depth"] = json!(1);
    behind["mode"] = json!("add");
    s.execute("select.pick", &behind).unwrap();
    assert_eq!(s.selection().len(), 2);
    // Toggling takes it out again; a ray in world coordinates works as a pixel does.
    let ray = json!({ "ray": { "origin": [0.5, 0.5, -3], "direction": [0, 0, 1] }, "mode": "toggle" });
    let out = s.execute("select.pick", &ray).unwrap();
    assert_eq!(out.data["picked"]["patch"], "ground");
    assert_eq!(s.selection(), [Sel::Patch { region: "air".into(), patch: "sky".into() }]);

    // The snapshot draws the selection.
    let out = s.execute("scene.snapshot", &json!({ "width": 16, "height": 16 })).unwrap();
    assert!(out.message.ends_with(", 1 selected"), "{}", out.message);

    // Nothing under the cursor: replace clears, add keeps, none only lists.
    let mut miss = top.clone();
    miss["pixel"] = json!([0, 0]);
    miss["mode"] = json!("add");
    s.execute("select.pick", &miss).unwrap();
    assert_eq!(s.selection().len(), 1);
    let mut look = top.clone();
    look["mode"] = json!("none");
    look["depth"] = json!(5);
    let out = s.execute("select.pick", &look).unwrap();
    assert_eq!(out.message, "only 2 under the cursor; 1 selected");
    // A depth past what is under the cursor changes nothing, even with replace.
    let mut past = top.clone();
    past["depth"] = json!(2);
    let out = s.execute("select.pick", &past).unwrap();
    assert_eq!(out.data["picked"], Value::Null);
    assert_eq!(s.selection().len(), 1);
    miss["mode"] = json!("replace");
    let out = s.execute("select.pick", &miss).unwrap();
    assert_eq!(out.message, "nothing under the cursor; 0 selected");
    assert!(s.selection().is_empty());

    for (p, says) in [
        (json!({}), "give `pixel` or `ray`"),
        (json!({ "pixel": [1, 2], "ray": { "origin": [0, 0, 0], "direction": [0, 0, 1] } }), "one of them"),
        (json!({ "pixel": [1] }), "two finite numbers"),
        (json!({ "ray": { "origin": [0, 0, 0], "direction": [0, 0, 0] } }), "not zero"),
        (json!({ "ray": { "origin": [0, 0], "direction": [0, 0, 1] } }), "three finite numbers"),
        (json!({ "ray": { "origin": [0, 0, 0], "direction": [0, 0, 1], "up": 1 } }), "`ray` is"),
        (json!({ "pixel": [1, 2], "mode": "remove" }), "`mode` is replace, add, toggle, none"),
        (json!({ "pixel": [1, 2], "depth": -1 }), "`depth`"),
    ] {
        let e = s.execute("select.pick", &p).unwrap_err().to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
    // Picks are in the journal: they are what later commands act on.
    assert_eq!(s.journal().iter().filter(|j| j["command"] == "select.pick").count(), 7);
}

#[test]
fn selections_are_checked_and_reset_by_opening_a_case() {
    let mut s = scene_session();
    let out = s
        .execute(
            "select.set",
            &json!({ "items": [{ "region": "air", "patch": "walls" }, { "surface": "box.stl", "solid": "box" }] }),
        )
        .unwrap();
    assert_eq!(out.message, "2 items selected");
    assert_eq!(out.data["selection"][1], json!({ "surface": "constant/triSurface/box.stl", "solid": "box" }));
    s.execute("select.set", &json!({ "items": [{ "region": "air", "patch": "walls" }], "mode": "remove" })).unwrap();
    assert_eq!(s.selection(), [Sel::Solid { file: "constant/triSurface/box.stl".into(), solid: "box".into() }]);
    for (items, says) in [
        (json!([{ "region": "air", "patch": "roof" }]), "no patch `roof`"),
        (json!([{ "region": "x", "patch": "walls" }]), "no region `x`"),
        (json!([{ "surface": "box.stl", "solid": "lid" }]), "no region `lid`"),
        (json!([{ "surface": "none.stl", "solid": "box" }]), "there is no surface"),
        (json!([{ "region": "air", "patch": "walls", "extra": 1 }]), "an item is"),
        (json!(["walls"]), "each item is an object"),
    ] {
        let e = s.execute("select.set", &json!({ "items": items })).unwrap_err().to_string();
        assert!(e.contains(says), "{items}: {e}");
    }
    // The same surface named many times is read once; duplicates are selected once.
    let many: Vec<Value> = (0..500).map(|_| json!({ "surface": "box.stl", "solid": "box" })).collect();
    let out = s.execute("select.set", &json!({ "items": many, "mode": "add" })).unwrap();
    assert_eq!(out.message, "1 item selected");
    // A failed select.set leaves the selection as it was.
    assert_eq!(s.selection().len(), 1);
    s.execute("select.clear", &Value::Null).unwrap();
    assert!(s.selection().is_empty());
    s.execute("select.set", &json!({ "items": [{ "region": "air", "patch": "sky" }] })).unwrap();
    s.open(Box::new(fs()), "mem2", OpenOptions::default(), false).unwrap();
    assert!(s.selection().is_empty());
}

/// The scene follows the surfaces commands change, without reading the disk again.
#[test]
fn the_scene_follows_surface_edits() {
    let mut s = scene_session();
    let p = json!({ "surfaces": ["box.stl"], "width": 8, "height": 8 });
    let d0 = s.execute("scene.snapshot", &p).unwrap().data["camera"]["distance"].as_f64().unwrap();
    s.execute("geometry.scale", &json!({ "file": "box.stl", "factor": 3 })).unwrap();
    let d1 = s.execute("scene.snapshot", &p).unwrap().data["camera"]["distance"].as_f64().unwrap();
    assert!((d1 / d0 - 3.0).abs() < 1e-9, "{d0} {d1}");
    s.execute("edit.undo", &Value::Null).unwrap();
    let d2 = s.execute("scene.snapshot", &p).unwrap().data["camera"]["distance"].as_f64().unwrap();
    assert!((d2 - d0).abs() < 1e-9);
}

/// The undo history names each edit; undo and redo move it between the two lists.
#[test]
fn the_history_names_the_edits() {
    let mut s = session();
    assert_eq!(s.history(), (Vec::new(), Vec::new()));
    s.execute("control.set", &json!({ "endTime": 7200 })).unwrap();
    s.execute("dict.set", &json!({ "file": "system/air/fvSolution", "path": "PIMPLE/nOuterCorrectors", "value": 2 }))
        .unwrap();
    // Commands that don't change the case are not in it.
    s.execute("case.check", &Value::Null).unwrap();
    let names = |l: Vec<&Edit>| l.iter().map(|e| (e.command, e.detail.clone())).collect::<Vec<_>>();
    let (done, undone) = s.history();
    assert_eq!(
        names(done),
        [
            ("control.set", "endTime 7200".to_string()),
            ("dict.set", "fvSolution, nOuterCorrectors, value 2".to_string())
        ]
    );
    assert!(undone.is_empty());
    s.execute("edit.undo", &Value::Null).unwrap();
    s.execute("edit.undo", &Value::Null).unwrap();
    let (done, undone) = s.history();
    assert!(done.is_empty());
    // Next to redo first.
    assert_eq!(undone.iter().map(|e| e.label).collect::<Vec<_>>(), ["Set run controls", "Set dictionary entry"]);
    s.execute("edit.redo", &Value::Null).unwrap();
    assert_eq!(s.history().0.len(), 1);
}

fn bc_of(s: &Session, field: &str, patch: &str) -> Option<(Option<String>, Option<String>)> {
    let f = s.case()?.region("air")?.field(field)?;
    f.bcs.as_ref()?.iter().find(|b| b.patch == patch).map(|b| (b.kind.clone(), b.value.clone()))
}

#[test]
fn boundary_conditions_on_the_selected_patches() {
    let mut s = session();
    // Nothing selected: nothing to set.
    let e = s.execute("bc.set", &json!({"field": "T", "type": "zeroGradient"})).unwrap_err().to_string();
    assert!(e.contains("no patches"), "{e}");
    s.execute("select.set", &json!({"items": [{"region": "air", "patch": "inlet"}]})).unwrap();
    let o = s
        .execute(
            "bc.set",
            &json!({"field": "T", "type": "inletOutlet", "value": 290, "entries": {"inletValue": "uniform 290"}}),
        )
        .unwrap();
    assert_eq!(o.message, "T: inletOutlet on inlet");
    assert_eq!(o.data["files"], json!(["0/air/T"]));
    assert_eq!(bc_of(&s, "T", "inlet"), Some((Some("inletOutlet".into()), Some("uniform 290".into()))));
    assert_eq!(s.case().unwrap().changed(), ["0/air/T"]);
    // A vector as a list of numbers; patches given explicitly.
    s.execute("bc.set", &json!({"field": "U", "type": "fixedValue", "value": [2, 0, 0], "patches": [{"region": "air", "patch": "inlet"}]}))
        .unwrap();
    assert_eq!(bc_of(&s, "U", "inlet"), Some((Some("fixedValue".into()), Some("uniform (2 0 0)".into()))));
    // Undo puts the file back as it was.
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(bc_of(&s, "T", "inlet"), Some((Some("fixedValue".into()), Some("uniform 300".into()))));
    assert!(s.case().unwrap().changed().is_empty());
    // Saved, only the entry changed.
    s.execute("edit.redo", &json!({})).unwrap();
    s.execute("case.save", &json!({})).unwrap();
    let t = text(&s, "0/air/T");
    assert!(t.starts_with("FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ndimensions [0 0 0 1 0 0 0];\ninternalField uniform 300;\nboundaryField\n{\n    inlet"), "{t}");
    assert!(
        t.ends_with("    inlet\n    {\n        type            inletOutlet;\n        inletValue      uniform 290;\n        value           uniform 290;\n    }\n}\n"),
        "{t}"
    );
}

#[test]
fn boundary_conditions_refuse_what_would_break_the_file() {
    let mut s = session();
    let p = json!([{"region": "air", "patch": "inlet"}]);
    for bad in [
        json!({"field": "T", "type": "fixed Value", "patches": p}),
        json!({"field": "T", "type": "a;b", "patches": p}),
        json!({"field": "T", "type": "fixedValue", "value": "uniform 1; type zeroGradient", "patches": p}),
        json!({"field": "T", "type": "fixedValue", "value": "{ x 1; }; y", "patches": p}),
        json!({"field": "T", "type": "fixedValue", "value": "uniform 1 // note", "patches": p}),
        json!({"field": "T", "type": "__umcfListA0", "patches": p}),
        json!({"field": "U", "type": "fixedValue", "value": 300, "patches": p}),
        json!({"field": "T", "type": "fixedValue", "value": [2, 0, 0], "patches": p}),
        json!({"field": "U", "type": "inletOutlet", "entries": {"inletValue": "uniform (1 0)"}, "patches": p}),
        json!({"field": "T", "type": "fixedValue", "entries": {"type": "x"}, "patches": p}),
        json!({"field": "T", "type": "fixedValue", "patches": [{"region": "air", "patch": "nowhere"}]}),
        json!({"field": "T", "type": "fixedValue", "patches": [{"region": "street", "patch": "inlet"}]}),
        json!({"field": "nothere", "type": "fixedValue", "patches": p}),
    ] {
        assert!(s.execute("bc.set", &bad).is_err(), "{bad}");
    }
    let e = s
        .execute("bc.set", &json!({"field": "U", "type": "fixedValue", "value": 300, "patches": p}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("has 1 component; U is a volVectorField (3)"), "{e}");
    assert!(s.case().unwrap().changed().is_empty());
    assert!(!s.can_undo());
}

#[test]
fn boundary_condition_types_offered() {
    let s = session();
    let scalar = s.bc_types("volScalarField");
    assert!(scalar.contains(&"fixedValue") && scalar.contains(&"fixedFluxPressure") && !scalar.contains(&"noSlip"));
    assert!(scalar.contains(&"compressible::CFDHAMfluidTemperatureCoupledMixed"), "{scalar:?}");
    let vector = s.bc_types("volVectorField");
    assert!(vector.contains(&"noSlip") && !vector.contains(&"compressible::CFDHAMfluidTemperatureCoupledMixed"));
}

#[test]
fn undoing_a_saved_condition_is_a_change_to_save() {
    let mut s = session();
    let original = text(&s, "0/air/T");
    let p = json!([{"region": "air", "patch": "inlet"}]);
    s.execute("bc.set", &json!({"field": "T", "type": "zeroGradient", "patches": p})).unwrap();
    s.execute("case.save", &json!({})).unwrap();
    assert_ne!(text(&s, "0/air/T"), original);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.case().unwrap().changed(), ["0/air/T"]);
    assert_eq!(bc_of(&s, "T", "inlet"), Some((Some("fixedValue".into()), Some("uniform 300".into()))));
    s.execute("case.save", &json!({})).unwrap();
    assert_eq!(text(&s, "0/air/T"), original);
    assert!(s.case().unwrap().changed().is_empty());
}

#[test]
fn defaults_go_into_new_entries_only() {
    // inlet keeps its fixedValue (uniform 300); outlet gets a new fixedValue entry.
    let fs = fs().with(
        "constant/air/polyMesh/boundary",
        "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class polyBoundaryMesh;\n}\n2\n(\n    inlet { type patch; nFaces 1; startFace 0; }\n    outlet { type patch; nFaces 1; startFace 1; }\n)\n",
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let p = json!([{"region": "air", "patch": "inlet"}, {"region": "air", "patch": "outlet"}]);
    s.execute(
        "bc.set",
        &json!({"field": "T", "type": "fixedValue", "defaults": {"value": "uniform 280"}, "entries": {"note": "\"set\""}, "patches": p}),
    )
    .unwrap();
    assert_eq!(bc_of(&s, "T", "inlet"), Some((Some("fixedValue".into()), Some("uniform 300".into()))));
    assert_eq!(bc_of(&s, "T", "outlet"), Some((Some("fixedValue".into()), Some("uniform 280".into()))));
    // A key given as an entry wins over the same default in a new entry.
    s.execute(
        "bc.set",
        &json!({"field": "T", "type": "zeroGradient", "patches": [{"region": "air", "patch": "outlet"}]}),
    )
    .unwrap();
    s.execute(
        "bc.set",
        &json!({"field": "T", "type": "fixedValue", "entries": {"value": "uniform 310"}, "defaults": {"value": "uniform 280"}, "patches": [{"region": "air", "patch": "outlet"}]}),
    )
    .unwrap();
    assert_eq!(bc_of(&s, "T", "outlet"), Some((Some("fixedValue".into()), Some("uniform 310".into()))));
    // So many keys are refused.
    let many: serde_json::Map<String, Value> = (0..65).map(|k| (format!("k{k}"), json!("1"))).collect();
    let e = s
        .execute("bc.set", &json!({"field": "T", "type": "fixedValue", "defaults": many, "patches": p}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("more than 64"), "{e}");
    // Defaults are checked like values.
    let bad = json!({"field": "U", "type": "fixedValue", "defaults": {"value": "uniform 1"}, "patches": p});
    assert!(s.execute("bc.set", &bad).is_err());
}

#[test]
fn patches_are_set_renamed_and_undone() {
    let mut s = session();
    s.execute("select.set", &json!({"items": [{"region": "air", "patch": "inlet"}]})).unwrap();
    let o = s.execute("patch.set", &json!({"type": "wall", "groups": ["wall", "ground"]})).unwrap();
    assert_eq!(o.message, "air/inlet: type wall; groups (wall ground)");
    let inlet = |s: &Session, name: &str| {
        s.case().unwrap().region("air").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == name).cloned()
    };
    let p = inlet(&s, "inlet").unwrap();
    assert_eq!((p.kind.as_str(), p.groups.len()), ("wall", 2));
    // Renamed: the fields follow, and it stays selected.
    let o = s.execute("patch.set", &json!({"name": "west"})).unwrap();
    assert!(o.data["renamed_in"].as_array().unwrap().iter().any(|f| f == "0/air/T"), "{}", o.data);
    assert!(inlet(&s, "west").is_some() && inlet(&s, "inlet").is_none());
    assert_eq!(bc_of(&s, "T", "west"), Some((Some("fixedValue".into()), Some("uniform 300".into()))));
    assert_eq!(s.selection(), [Sel::Patch { region: "air".into(), patch: "west".into() }]);
    // Undone: the old name everywhere, and the selection has nothing stale.
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(inlet(&s, "inlet").is_some());
    assert!(s.selection().is_empty());
    // Refused.
    let p = json!([{"region": "air", "patch": "inlet"}]);
    for bad in [
        json!({"patches": p}),
        json!({"type": "a b", "patches": p}),
        json!({"name": "x;y", "patches": p}),
        json!({"groups": "wall", "patches": p}),
        json!({"entries": {"nFaces": 3}, "patches": p}),
        json!({"type": "wall", "patches": [{"region": "air", "patch": "nowhere"}]}),
        json!({"type": "wall"}),
    ] {
        assert!(s.execute("patch.set", &bad).is_err(), "{bad}");
    }
}

#[test]
fn a_rename_follows_into_0_orig_and_view_factors_and_says_where_else_the_name_is() {
    let field = |entries: &str| {
        format!(
            "FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}}\ninternalField uniform 0;\nboundaryField\n{{\n{entries}}}\n"
        )
    };
    let mut fs = fs();
    fs.insert("0.orig/air/T", field("    inlet { type fixedValue; value uniform 290; } // the west side\n"));
    fs.insert("0.orig/air/p", field("    \"(inlet|outlet)\" { type zeroGradient; }\n"));
    fs.insert("0.orig/air/notes.txt", "not a dictionary: inlet");
    fs.insert(
        "system/air/viewFactorsDict",
        format!("{HEADER}inlet\n{{\n    nFacesInCoarsestLevel 15;\n}}\ninletSide 1;\n"),
    );
    fs.insert(
        "system/air/createPatchDict",
        format!("{HEADER}patches\n(\n    {{ name roof; constructFrom patches; patches (inlet); }}\n);\n"),
    );
    fs.insert("3600/air/T", field("    inlet { type fixedValue; value uniform 291; }\n"));
    fs.insert("7200/air/T", field("    inlet { type fixedValue; value uniform 292; }\n"));
    fs.insert("constant/air/boundaryData/inlet/points", "((0 0 0))\n");
    // In 0.orig the new name is taken already: that file is left as it is, and said so.
    fs.insert("0.orig/air/U", field("    inlet { type zeroGradient; }\n    west { type slip; }\n"));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let o = s.execute("patch.set", &json!({"name": "west", "patches": [{"region": "air", "patch": "inlet"}]})).unwrap();
    let renamed: Vec<&str> = o.data["renamed_in"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(renamed.contains(&"0.orig/air/T") && renamed.contains(&"system/air/viewFactorsDict"), "{renamed:?}");
    let put = |s: &Session, f: &str| String::from_utf8(s.case().unwrap().put_bytes(f).unwrap().to_vec()).unwrap();
    // Only the entry's name changes, comment and all.
    assert!(put(&s, "0.orig/air/T").contains("    west { type fixedValue; value uniform 290; } // the west side\n"));
    let vf = s.case().unwrap().dict("system/air/viewFactorsDict").unwrap().to_string();
    assert!(vf.contains("west\n{\n    nFacesInCoarsestLevel 15;") && vf.contains("inletSide 1;"), "{vf}");
    // The pattern in 0.orig, the createPatchDict and the results are only named.
    assert!(s.case().unwrap().put_bytes("0.orig/air/p").is_none());
    let codes: Vec<(String, String, String)> = o.data["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["code"].as_str().unwrap().to_string(),
                d["path"].as_str().unwrap().to_string(),
                d["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let has = |code: &str, path: &str| codes.iter().find(|(c, p, _)| c == code && p == path).map(|(_, _, m)| m.clone());
    assert_eq!(
        has("patch.mention", "system/air/createPatchDict").as_deref(),
        Some("line 9 names `inlet`: rename it there if that is the patch")
    );
    assert!(has("patch.mention", "0.orig/air/p").is_some(), "{codes:?}");
    assert!(has("patch.mention", "0.orig/air/notes.txt").is_some(), "{codes:?}");
    assert!(has("patch.mention", "system/air/viewFactorsDict").is_none(), "{codes:?}");
    assert_eq!(
        has("patch.unrenamed", "0.orig/air/U").as_deref(),
        Some("not changed, as it has an entry `west` already: rename the patch `inlet` there by hand if it names it")
    );
    assert!(s.case().unwrap().put_bytes("0.orig/air/U").is_none());
    let times = has("patch.times", "").unwrap();
    assert!(
        times.contains("time directories 3600, 7200 aren't read here and may still name the patch `inlet`"),
        "{times}"
    );
    // The data mapped onto the patch by its name.
    assert!(
        has("patch.mention", "constant/air/boundaryData/inlet").unwrap().contains("this directory is named `inlet`"),
        "{codes:?}"
    );
    assert!(!codes.iter().any(|(c, _, _)| c == "patch.orig" || c == "patch.elsewhere"));
    // The results are not touched.
    assert!(s.case().unwrap().put_bytes("3600/air/T").is_none());
    // Undone: 0.orig and the view factors as they were.
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.case().unwrap().put_bytes("0.orig/air/T").is_none());
    let vf = s.case().unwrap().dict("system/air/viewFactorsDict").unwrap().to_string();
    assert!(vf.contains("inlet\n{") && !vf.contains("west"), "{vf}");
}

#[test]
fn a_rename_follows_into_the_results_when_asked_and_saved() {
    let field = |entries: &str| {
        format!(
            "FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}}\ninternalField uniform 0;\nboundaryField\n{{\n{entries}}}\n"
        )
    };
    let mut fs = fs();
    fs.insert("3600/air/T", field("    inlet { type fixedValue; value uniform 291; }\n"));
    fs.insert("constant/air/boundaryData/inlet/points", "((0 0 0))\n");
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let p = json!([{"region": "air", "patch": "inlet"}]);
    // Only with a new name.
    assert!(s.execute("patch.set", &json!({"type": "wall", "results": true, "patches": p})).is_err());
    let o = s.execute("patch.set", &json!({"name": "west", "results": true, "patches": p})).unwrap();
    let codes: Vec<(&str, &str)> = o.data["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["code"].as_str().unwrap(), d["message"].as_str().unwrap()))
        .collect();
    // What the save will rename, not what is left with the old name.
    let what = codes.iter().find(|(c, _)| *c == "patch.results").map(|(_, m)| *m).unwrap();
    assert!(what.contains("1 file") && what.contains("constant/air/boundaryData/inlet, renamed west"), "{what}");
    assert!(!codes.iter().any(|(c, m)| *c == "patch.times" || m.contains("this directory is named")), "{codes:?}");
    let read = |s: &Session, f: &str| s.fs().unwrap().read(f, 1 << 20).unwrap().map(|b| String::from_utf8(b).unwrap());
    assert!(read(&s, "3600/air/T").unwrap().contains("inlet {"));
    let o = s.execute("case.save", &json!({})).unwrap();
    assert!(
        o.message
            .contains("the patch names followed into the results: 1 file rewritten, constant/air/boundaryData/west"),
        "{}",
        o.message
    );
    assert!(read(&s, "3600/air/T").unwrap().contains("    west { type fixedValue; value uniform 291; }"));
    assert!(s.fs().unwrap().is_dir("constant/air/boundaryData/west"));
    // Undone and saved: back in the results too.
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.case().unwrap().changed().iter().any(|c| c == "results: air/west renamed inlet"));
    s.execute("case.save", &json!({})).unwrap();
    assert!(read(&s, "3600/air/T").unwrap().contains("    inlet { type"));
    assert!(s.fs().unwrap().is_dir("constant/air/boundaryData/inlet"));
}

#[test]
fn a_rename_follows_into_the_results_of_a_case_on_disk() {
    let dir = scratch("results");
    let t = std::fs::read_to_string(dir.join("0/air/T")).unwrap();
    for (rel, text) in [("3600/air/T", t.as_str()), ("constant/air/boundaryData/inlet/points", "((0 0 0))\n")] {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute("case.open", &json!({"path": dir.to_str().unwrap()})).unwrap();
    s.execute("patch.set", &json!({"name": "west", "results": true, "patches": [{"region": "air", "patch": "inlet"}]}))
        .unwrap();
    let o = s.execute("case.save", &json!({})).unwrap();
    assert!(o.message.contains("1 file rewritten, constant/air/boundaryData/west"), "{}", o.message);
    assert!(std::fs::read_to_string(dir.join("3600/air/T")).unwrap().contains("    west { type zeroGradient; }"));
    assert!(dir.join("constant/air/boundaryData/west/points").is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn patch_entries_are_written_as_given() {
    let mut s = session();
    s.execute(
        "patch.set",
        &json!({"type": "mappedWall", "entries": {"neighbourRegion": "air", "offset": [0, 0, 1], "distance": 0.5}, "patches": [{"region": "air", "patch": "inlet"}]}),
    )
    .unwrap();
    let p = s.case().unwrap().region("air").unwrap().patches.as_ref().unwrap().first().cloned().unwrap();
    assert!(p.params.contains(&("offset".into(), "(0 0 1)".into())), "{:?}", p.params);
    assert!(p.params.contains(&("distance".into(), "0.5".into())), "{:?}", p.params);
}

#[test]
fn new_patches_are_prepared_from_a_box_a_surface_or_labels() {
    use umcf_geom::stl::Format;
    let mut fs = fs();
    cube_mesh(&mut fs, "constant/air/polyMesh");
    fs.insert("constant/triSurface/box.stl", cube_stl(Format::Ascii, false));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    s.execute("select.set", &json!({"items": [{"region": "air", "patch": "inlet"}]})).unwrap();
    // The top of the cube, by a box: the region and the patch are the selection's.
    let o =
        s.execute("patch.create", &json!({"name": "roof", "box": {"min": [-1, -1, 0.9], "max": [2, 2, 2]}})).unwrap();
    assert_eq!(o.message, "air/roof: 1 faces prepared for createPatch");
    assert_eq!(o.data["from"], json!([{"patch": "inlet", "faces": 1}]));
    assert_eq!(o.data["set"], "constant/air/polyMesh/sets/roof");
    assert_eq!(o.data["fields"].as_array().unwrap().len(), 6, "{}", o.data);
    assert_eq!(codes(&o), ["patch.run"]);
    let run = o.data["diagnostics"][0]["message"].as_str().unwrap();
    assert!(run.contains("createPatch -region air -dict system/air/createPatchDict.umcf -overwrite"), "{run}");
    assert!(s.case().unwrap().put_bytes("constant/air/polyMesh/sets/roof").is_some());
    assert_eq!(bc_of(&s, "T", "roof"), Some((Some("fixedValue".into()), Some("uniform 300".into()))));
    let set =
        String::from_utf8(s.case().unwrap().put_bytes("constant/air/polyMesh/sets/roof").unwrap().to_vec()).unwrap();
    assert!(set.ends_with("\n1\n(\n1\n)\n"), "{set}");
    // Face labels.
    let o = s.execute("patch.create", &json!({"name": "floor", "faces": [0], "region": "air"})).unwrap();
    assert_eq!(o.data["faces"], json!(1));
    // A surface: every face of the cube is on it, two of them prepared already.
    let skin = json!({"name": "skin", "surface": {"file": "box.stl", "solid": "box", "distance": 0.01}});
    let e = s.execute("patch.create", &skin).unwrap_err().to_string();
    assert!(e.contains("prepared in system/air/createPatchDict.umcf already"), "{e}");
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.case().unwrap().dict("system/air/createPatchDict.umcf").is_none());
    assert!(s.case().unwrap().put_bytes("constant/air/polyMesh/sets/roof").is_none());
    // On its own it takes them all, so `inlet` goes.
    let o = s.execute("patch.create", &skin).unwrap();
    assert_eq!(o.data["faces"], json!(6));
    assert!(codes(&o).contains(&"patch.emptied".to_string()), "{:?}", codes(&o));
    s.execute("edit.undo", &json!({})).unwrap();
    // A patch named twice in `from` gives its faces once.
    let o = s
        .execute(
            "patch.create",
            &json!({"name": "top", "box": {"min": [-1, -1, 0.9], "max": [2, 2, 2]}, "from": ["inlet", "inlet"]}),
        )
        .unwrap();
    assert_eq!(o.data["from"], json!([{"patch": "inlet", "faces": 1}]));
    s.execute("edit.undo", &json!({})).unwrap();
    // Refused.
    for bad in [
        json!({"faces": [0]}),
        json!({"name": "x"}),
        json!({"name": "x", "faces": [0], "box": {"min": [0, 0, 0], "max": [1, 1, 1]}}),
        json!({"name": "x", "faces": [-1]}),
        json!({"name": "x", "faces": [99]}),
        json!({"name": "x", "faces": [0], "from": ["inlet"]}),
        json!({"name": "x", "box": {"min": [1, 1, 1], "max": [0, 0, 0]}}),
        json!({"name": "x", "box": {"min": [0, 0, 0]}}),
        json!({"name": "x", "box": {"min": [5, 5, 5], "max": [6, 6, 6]}}),
        json!({"name": "x", "box": {"min": [0, 0, 0], "max": [1, 1, 1]}, "from": ["nowhere"]}),
        json!({"name": "x", "box": {"min": [0, 0, 0], "max": [1, 1, 1]}, "from": vec!["inlet"; 100]}),
        json!({"name": "x", "surface": {"file": "box.stl"}}),
        json!({"name": "x", "surface": {"file": "box.stl", "solid": "lid", "distance": 1}}),
        json!({"name": "x", "surface": {"file": "nowhere.stl", "distance": 1}}),
        json!({"name": "inlet", "faces": [0]}),
        json!({"name": "1", "faces": [0]}),
    ] {
        assert!(s.execute("patch.create", &bad).is_err(), "{bad}");
    }
    assert!(s.case().unwrap().changed().is_empty());
}

#[test]
fn faces_are_picked_selected_and_made_a_patch() {
    let mut s = scene_session();
    let top = json!({ "view": "top", "width": 100, "height": 100, "pixel": [50, 50], "faces": true });
    let out = s.execute("select.pick", &top).unwrap();
    assert_eq!(out.data["picked"], json!({ "region": "air", "face": 1 }));
    assert_eq!(
        out.data["hits"][0],
        json!({ "region": "air", "face": 1, "patch": "sky", "distance": out.data["hits"][0]["distance"], "point": out.data["hits"][0]["point"] })
    );
    assert_eq!(out.data["hits"][1]["face"], 0);
    assert_eq!(out.message, "face 1 of region air (2 under the cursor); 1 selected");
    // The one behind, added; select.set takes faces by label too.
    let mut behind = top.clone();
    behind["depth"] = json!(1);
    behind["mode"] = json!("add");
    s.execute("select.pick", &behind).unwrap();
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 2 }], "mode": "add" })).unwrap();
    assert_eq!(s.selection().len(), 3);
    for bad in [
        json!({ "region": "air", "face": 99 }),
        json!({ "region": "air", "face": -1 }),
        json!({ "region": "air", "face": 1.5 }),
        json!({ "region": "air", "face": 4_294_967_296u64 }),
        json!({ "region": "nowhere", "face": 0 }),
    ] {
        assert!(s.execute("select.set", &json!({ "items": [bad] })).is_err(), "{bad}");
    }
    // The snapshot draws them.
    let out = s.execute("scene.snapshot", &json!({ "width": 16, "height": 16 })).unwrap();
    assert!(out.message.ends_with(", 3 selected"), "{}", out.message);
    // Taken out again, by toggle and remove.
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 2 }], "mode": "remove" })).unwrap();
    assert_eq!(s.selection().len(), 2);
    let mut toggle = top.clone();
    toggle["mode"] = json!("toggle");
    s.execute("select.pick", &toggle).unwrap();
    assert_eq!(s.selection(), [Sel::Face { region: "air".into(), face: 0 }]);
    s.execute("select.pick", &toggle).unwrap();
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 2 }], "mode": "add" })).unwrap();
    assert_eq!(s.selection().len(), 3);
    // `none` only lists.
    let mut look = top.clone();
    look["mode"] = json!("none");
    s.execute("select.pick", &look).unwrap();
    assert_eq!(s.selection().len(), 3);
    // A surface in front is passed over: the mesh face behind is picked.
    let mut through = look.clone();
    through["surfaces"] = json!(["box.stl"]);
    through["regions"] = json!(["air"]);
    let out = s.execute("select.pick", &through).unwrap();
    let hits = out.data["hits"].as_array().unwrap();
    assert!(
        !hits.is_empty() && hits.iter().all(|h| h.get("face").is_some() && h.get("triangle").is_none()),
        "{}",
        out.data
    );
    // patch.create takes the selected faces, and their region.
    let o = s.execute("patch.create", &json!({ "name": "picked" })).unwrap();
    assert_eq!(o.data["faces"], json!(3));
    assert_eq!(o.data["region"], "air");
    let from: Vec<&str> = o.data["from"].as_array().unwrap().iter().map(|f| f["patch"].as_str().unwrap()).collect();
    assert_eq!(from, ["ground", "sky", "walls"]);
    // They are in the patch now: out of the selection.
    assert!(s.selection().is_empty());
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 3 }] })).unwrap();
    assert!(s.execute("patch.create", &json!({ "name": "again", "from": ["sky"] })).is_err());
    s.execute("edit.undo", &json!({})).unwrap();
    // Given as labels, selected faces in the patch leave the selection too.
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 3 }] })).unwrap();
    s.execute("patch.create", &json!({ "name": "given", "region": "air", "faces": [3] })).unwrap();
    assert!(s.selection().is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    // A mesh read again drops the faces picked on it.
    let mut reload = look.clone();
    reload["reload"] = json!(true);
    s.execute("select.pick", &reload).unwrap();
    assert!(s.selection().is_empty());
    // Nothing picked and no faces given: says so.
    s.execute("select.clear", &json!({})).unwrap();
    let e = s.execute("patch.create", &json!({ "name": "x", "region": "air" })).unwrap_err().to_string();
    assert!(e.contains("select faces"), "{e}");
}

#[test]
fn a_brush_selects_the_faces_seen_under_its_stroke() {
    let mut s = scene_session();
    // From above, a dab in the middle: the sky's face (faces by default); through it, the ground
    // under it too.
    let mut dab = json!({ "view": "top", "width": 100, "height": 100, "stroke": [[50, 50]], "radius": 3 });
    let out = s.execute("select.brush", &dab).unwrap();
    assert_eq!(out.data["items"], json!([{ "region": "air", "face": 1 }]));
    assert_eq!(out.message, "1 face in the stroke; 1 selected");
    dab["through"] = json!(true);
    dab["mode"] = json!("add");
    s.execute("select.brush", &dab).unwrap();
    assert_eq!(s.selection().len(), 2);
    // Patches, not faces; a stroke over the background only, with replace, clears.
    let patches = json!({ "view": "top", "width": 100, "height": 100, "stroke": [[50, 50], [60, 60]], "faces": false });
    let out = s.execute("select.brush", &patches).unwrap();
    assert_eq!(out.data["items"], json!([{ "region": "air", "patch": "sky" }]));
    let off = json!({ "view": "top", "width": 100, "height": 100, "stroke": [[1, 1], [1, 3]], "radius": 1 });
    assert_eq!(s.execute("select.brush", &off).unwrap().data["in"], 0);
    assert!(s.selection().is_empty());
    for (p, says) in [
        (json!({}), "`stroke`"),
        (json!({ "stroke": [] }), "1 to 4096 points"),
        (json!({ "stroke": [[0, 0, 0]] }), "1 to 4096 points"),
        (json!({ "stroke": [[0, "a"]] }), "1 to 4096 points"),
        (json!({ "stroke": vec![[0, 0]; 4097] }), "1 to 4096 points"),
        (json!({ "stroke": [[0, 0]], "radius": -1 }), "`radius` is a number of pixels"),
        (json!({ "stroke": [[0, 0]], "radius": 1e9 }), "`radius` is a number of pixels"),
        (json!({ "stroke": [[0, 0]], "radius": "big" }), "`radius` is a number"),
        (json!({ "stroke": [[0, 0]], "rect": [0, 0, 1, 1] }), "rect"),
    ] {
        let e = s.execute("select.brush", &p).unwrap_err().to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
}

#[test]
fn a_rectangle_selects_what_is_seen_in_it_or_everything_through_it() {
    let mut s = scene_session();
    let all = json!({ "view": "top", "width": 100, "height": 100, "rect": [0, 0, 100, 100] });
    // Seen from above: the sky; the walls and the ground are under it.
    let out = s.execute("select.rect", &all).unwrap();
    let patches: Vec<&str> =
        out.data["items"].as_array().unwrap().iter().map(|i| i["patch"].as_str().unwrap()).collect();
    assert_eq!(patches, ["sky"]);
    assert_eq!(out.message, "1 item in the rectangle; 1 selected");
    // Through it: all three.
    let mut through = all.clone();
    through["through"] = json!(true);
    through["mode"] = json!("add");
    let out = s.execute("select.rect", &through).unwrap();
    assert_eq!(out.data["in"], 3);
    assert_eq!(s.selection().len(), 3);
    // Faces: the sky's in the middle, both faces under it through it; added, then taken off.
    let mut mid = json!({ "view": "top", "width": 100, "height": 100, "rect": [45, 45, 55, 55], "faces": true });
    s.execute("select.clear", &json!({})).unwrap();
    s.execute("select.rect", &mid).unwrap();
    assert_eq!(s.selection(), [Sel::Face { region: "air".into(), face: 1 }]);
    mid["through"] = json!(true);
    mid["mode"] = json!("add");
    let out = s.execute("select.rect", &mid).unwrap();
    assert_eq!(out.data["items"], json!([{ "region": "air", "face": 0 }, { "region": "air", "face": 1 }]));
    assert_eq!(s.selection().len(), 2);
    mid["through"] = json!(false);
    mid["mode"] = json!("remove");
    let out = s.execute("select.rect", &mid).unwrap();
    assert_eq!(out.message, "1 face in the rectangle, 1 taken off; 1 selected");
    assert_eq!(s.selection(), [Sel::Face { region: "air".into(), face: 0 }]);
    // `none` only counts; an empty rectangle with replace clears.
    mid["mode"] = json!("none");
    assert_eq!(s.execute("select.rect", &mid).unwrap().data["in"], 1);
    assert_eq!(s.selection().len(), 1);
    let empty = json!({ "view": "top", "width": 100, "height": 100, "rect": [0, 0, 3, 3] });
    assert_eq!(s.execute("select.rect", &empty).unwrap().data["in"], 0);
    assert!(s.selection().is_empty());
    // A surface has regions to give, and no faces.
    let surface =
        json!({ "view": "top", "width": 100, "height": 100, "rect": [45, 45, 55, 55], "surfaces": ["box.stl"] });
    let out = s.execute("select.rect", &surface).unwrap();
    assert_eq!(out.data["items"], json!([{ "surface": "constant/triSurface/box.stl", "solid": "box" }]));
    let mut faces = surface.clone();
    faces["faces"] = json!(true);
    assert_eq!(s.execute("select.rect", &faces).unwrap().data["in"], 0);
    // Picking faces, a surface in front doesn't hide the mesh (as a click passes over it): the
    // box made twice the size covers the cube from above.
    s.execute("geometry.scale", &json!({ "file": "box.stl", "factor": 2 })).unwrap();
    faces["regions"] = json!(["air"]);
    faces.as_object_mut().unwrap().remove("view");
    faces["camera"] = json!({ "target": [0.5, 0.5, 0.5], "distance": 10, "azimuth": 0, "elevation": 90 });
    let mut patches = faces.clone();
    patches["faces"] = json!(false);
    let out = s.execute("select.rect", &patches).unwrap();
    assert_eq!(out.data["items"], json!([{ "surface": "constant/triSurface/box.stl", "solid": "box" }]));
    let out = s.execute("select.rect", &faces).unwrap();
    assert_eq!(out.data["items"], json!([{ "region": "air", "face": 1 }]));
    s.execute("edit.undo", &json!({})).unwrap();

    // The camera as scene.snapshot returns it places the view exactly.
    let shot = s.execute("scene.snapshot", &json!({ "view": "top", "width": 100, "height": 100, "zoom": 3 })).unwrap();
    let cam = shot.data["camera"].clone();
    let mut given = json!({ "camera": cam, "width": 100, "height": 100, "rect": [0, 0, 100, 100], "faces": true });
    // Zoomed in on the sky, nothing else is seen.
    assert_eq!(s.execute("select.rect", &given).unwrap().data["items"], json!([{ "region": "air", "face": 1 }]));
    given["camera"].as_object_mut().unwrap().remove("fov");
    assert!(s.execute("select.rect", &given).is_ok());
    let pick = json!({ "camera": shot.data["camera"], "width": 100, "height": 100, "pixel": [50, 50] });
    assert_eq!(s.execute("select.pick", &pick).unwrap().data["picked"]["patch"], "sky");
    for (p, says) in [
        (json!({}), "`rect`"),
        (json!({ "rect": [0, 0, 1] }), "four finite numbers"),
        (json!({ "rect": [0, 0, 1, "a"] }), "four finite numbers"),
        (json!({ "rect": [0, 0, 1, 1], "mode": "toggle" }), "`mode` is replace, add, remove, none"),
        (json!({ "rect": [0, 0, 1, 1], "camera": cam, "view": "top" }), "`view` is not given with it"),
        (json!({ "rect": [0, 0, 1, 1], "camera": { "target": [0, 0, 0] } }), "`camera` is"),
        (
            json!({ "rect": [0, 0, 1, 1], "camera": { "target": [0, 0, 0], "distance": 0, "azimuth": 0, "elevation": 0 } }),
            "`camera` is",
        ),
        (
            json!({ "rect": [0, 0, 1, 1], "camera": { "target": [0, 0, 0], "distance": 1, "azimuth": 0, "elevation": 95 } }),
            "`camera` is",
        ),
        (
            json!({ "rect": [0, 0, 1, 1], "camera": { "target": [0, 0], "distance": 1, "azimuth": 0, "elevation": 0 } }),
            "camera.target",
        ),
        (
            json!({ "rect": [0, 0, 1, 1], "camera": { "target": [0, 0, 0], "distance": 1, "azimuth": 0, "elevation": 0, "up": 1 } }),
            "`camera` is",
        ),
    ] {
        let e = s.execute("select.rect", &p).unwrap_err().to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
}

#[test]
fn layers_are_shown_as_surfaces_wireframes_points_or_hidden() {
    let mut s = scene_session();
    let top = json!({ "view": "top", "width": 100, "height": 100, "pixel": [50, 50] });
    // The sky hidden: a click picks the ground under it, and nothing lists the sky.
    let mut p = top.clone();
    p["display"] = json!([{ "region": "air", "patch": "sky", "mode": "hidden" }]);
    let out = s.execute("select.pick", &p).unwrap();
    assert_eq!(out.data["picked"]["patch"], "ground");
    assert_eq!(out.data["hits"].as_array().unwrap().len(), 1);
    // A wireframe or a transparent sky is still picked, nearest first.
    for d in [json!({ "mode": "wireframe" }), json!({ "opacity": 0.3 }), json!({ "mode": "points", "opacity": 0 })] {
        let mut e = json!({ "region": "air", "patch": "sky" });
        e.as_object_mut().unwrap().extend(d.as_object().unwrap().clone());
        p["display"] = json!([e]);
        assert_eq!(s.execute("select.pick", &p).unwrap().data["picked"]["patch"], "sky", "{e}");
    }
    // Later entries win, field by field: the region hidden, then its ground shown again.
    p["display"] = json!([
        { "region": "air", "mode": "hidden", "opacity": 0.5 },
        { "region": "air", "patch": "ground", "mode": "surface" }
    ]);
    let out = s.execute("select.pick", &p).unwrap();
    assert_eq!(out.data["picked"]["patch"], "ground");
    // A rectangle sees through what isn't an opaque surface, and never what is hidden.
    let rect = |d: Value| json!({ "view": "top", "width": 100, "height": 100, "rect": [45, 45, 55, 55], "mode": "none", "display": d });
    let items = |s: &mut Session, d: Value| -> Vec<String> {
        let out = s.execute("select.rect", &rect(d)).unwrap();
        out.data["items"].as_array().unwrap().iter().map(|i| i["patch"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(items(&mut s, json!([])), ["sky"]);
    assert_eq!(items(&mut s, json!([{ "region": "air", "patch": "sky", "mode": "wireframe" }])), ["ground", "sky"]);
    assert_eq!(items(&mut s, json!([{ "region": "air", "patch": "sky", "mode": "hidden" }])), ["ground"]);
    // The snapshot draws them so: the sky hidden shows the ground in the middle.
    let shot = |s: &mut Session, d: Value| {
        let out =
            s.execute("scene.snapshot", &json!({ "view": "top", "width": 40, "height": 40, "display": d })).unwrap();
        out.data["png"].as_str().unwrap().to_string()
    };
    let plain = shot(&mut s, json!([]));
    let hidden = shot(&mut s, json!([{ "region": "air", "patch": "sky", "mode": "hidden" }]));
    let wire = shot(&mut s, json!([{ "region": "air", "mode": "wireframe" }]));
    assert!(plain != hidden && plain != wire && hidden != wire);
    // A surface and its region.
    let both =
        json!([{ "surface": "box.stl", "mode": "points" }, { "surface": "box.stl", "solid": "box", "opacity": 0.5 }]);
    let out = s
        .execute("scene.snapshot", &json!({ "surfaces": ["box.stl"], "width": 8, "height": 8, "display": both }))
        .unwrap();
    assert!(out.data["png"].is_string());
    // What doesn't exist, or isn't one, is refused.
    for (d, says) in [
        (json!({ "region": "water", "mode": "hidden" }), "no region `water`"),
        (json!({ "region": "air", "patch": "roof", "mode": "hidden" }), "no patch `roof`"),
        (json!({ "region": "air", "group": "buildings", "mode": "hidden" }), "no patch in group `buildings`"),
        (json!({ "surface": "none.stl", "mode": "hidden" }), "there is no surface"),
        (json!({ "surface": "box.stl", "solid": "lid", "mode": "hidden" }), "has no region `lid`"),
        (json!({ "region": "air", "mode": "solid" }), "a `display` entry is"),
        (json!({ "region": "air", "opacity": 2 }), "a `display` entry is"),
        (json!({ "region": "air" }), "a `display` entry is"),
        (json!({ "region": "air", "surface": "box.stl", "mode": "hidden" }), "a `display` entry is"),
        (json!({ "region": "air", "colour": "red", "mode": "hidden" }), "a `display` entry is"),
    ] {
        let e = s.execute("scene.snapshot", &json!({ "display": [d] })).unwrap_err().to_string();
        assert!(e.contains(says), "{d}: {e}");
    }
    let e = s.execute("scene.snapshot", &json!({ "display": { "region": "air" } })).unwrap_err().to_string();
    assert!(e.contains("`display` is a list"), "{e}");
}

#[test]
fn a_patch_group_is_shown_as_one_layer() {
    let mut fs = fs();
    cube_mesh(&mut fs, "constant/air/polyMesh");
    let h = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class polyBoundaryMesh;\n}\n";
    let patch = |n: &str, g: &str, count: u32, start: u32| {
        format!(
            "    {n}\n    {{\n        type wall;\n        inGroups List<word> 1({g});\n        nFaces {count};\n        startFace {start};\n    }}\n"
        )
    };
    fs.insert(
        "constant/air/polyMesh/boundary",
        format!(
            "{h}3\n(\n{}{}{})\n",
            patch("ground", "floor", 1, 0),
            patch("sky", "lid", 1, 1),
            patch("walls", "lid", 4, 2)
        ),
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let p = json!({ "view": "top", "width": 100, "height": 100, "pixel": [50, 50], "display": [{ "region": "air", "group": "lid", "mode": "hidden" }] });
    assert_eq!(s.execute("select.pick", &p).unwrap().data["picked"]["patch"], "ground");
}

#[test]
fn picked_faces_make_a_patch_of_one_region() {
    let mut fs = fs();
    fs.insert("system/controlDict", control().replace("    fluid (air);\n", "    fluid (air);\n    solid (wall);\n"));
    cube_mesh(&mut fs, "constant/air/polyMesh");
    cube_mesh(&mut fs, "constant/wall/polyMesh");
    // The wall's one patch is `empty`: faces picked off it are pointed out.
    let b =
        String::from_utf8(umcf_core::CaseFs::read(&fs, "constant/wall/polyMesh/boundary", 1 << 20).unwrap().unwrap())
            .unwrap();
    fs.insert("constant/wall/polyMesh/boundary", b.replace("type patch;", "type empty;"));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    s.execute("select.set", &json!({ "items": [{ "region": "air", "face": 0 }, { "region": "wall", "face": 1 }] }))
        .unwrap();
    let e = s.execute("patch.create", &json!({ "name": "x" })).unwrap_err().to_string();
    assert!(e.contains("one region"), "{e}");
    let e = s.execute("patch.create", &json!({ "name": "x", "region": "air" })).unwrap_err().to_string();
    assert!(e.contains("1 of the picked faces are in other regions"), "{e}");
    s.execute("select.set", &json!({ "items": [{ "region": "wall", "face": 1 }] })).unwrap();
    let e = s.execute("patch.create", &json!({ "name": "x", "region": "air" })).unwrap_err().to_string();
    assert!(e.contains("none of the picked faces is in region `air`"), "{e}");
    let o = s.execute("patch.create", &json!({ "name": "x" })).unwrap();
    assert_eq!(o.data["region"], "wall");
    assert!(codes(&o).contains(&"patch.constraint".to_string()), "{:?}", codes(&o));
}

#[test]
fn geometry_trees_from_a_table_a_file_or_a_list() {
    let table = "x\ty\tz\tCD\tCH\n260\t260\t2.5\t5.5\t5.5\n330\t330\t2.5\t5.5\t5.5\n";
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs().with("constant/triSurface/trees.txt", table)), "mem", OpenOptions::default(), false).unwrap();
    let out = s.execute("geometry.trees", &json!({ "table": "constant/triSurface/trees.txt" })).unwrap();
    assert!(out.message.starts_with("constant/triSurface/trees.stl: 2 trees as crowns (trees)"), "{}", out.message);
    // Closed, outward and without empty triangles: only the summary.
    assert_eq!(codes(&out), ["surface.summary"]);
    assert_eq!(out.data["surface"]["triangles"], 88);
    assert_eq!(s.case().unwrap().changed(), ["constant/triSurface/trees.stl"]);
    let e = s.execute("geometry.trees", &json!({ "table": "constant/triSurface/trees.txt" })).unwrap_err();
    assert!(e.to_string().contains("already there"), "{e}");
    // Undone, it is gone.
    s.execute("edit.undo", &Value::Null).unwrap();
    assert!(s.case().unwrap().changed().is_empty());

    // A list, its own name and region.
    let out = s
        .execute(
            "geometry.trees",
            &json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "table": null, "name": "oaks.stl", "solid": "oaks" }),
        )
        .unwrap();
    assert_eq!(out.data["surface"]["triangles"], 44);
    assert_eq!(out.data["solid"], "oaks");

    // A file outside the case.
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-trees", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("trees.txt");
    std::fs::write(&path, table).unwrap();
    let out = s.execute("geometry.trees", &json!({ "path": path.to_str().unwrap(), "name": "more.stl" })).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.data["trees"], 2);
    // Binary: no region name, and none claimed.
    let out = s
        .execute(
            "geometry.trees",
            &json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "name": "bin.stlb" }),
        )
        .unwrap();
    assert_eq!(out.data["solid"], Value::Null);
    assert!(out.message.contains("binary: no region name"), "{}", out.message);

    for (p, says) in [
        (json!({}), "one way"),
        (json!({ "table": "a", "trees": [] }), "one way"),
        (json!({ "trees": [] }), "no trees"),
        (json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 0, "height": 6 }] }), "positive"),
        (json!({ "trees": [{ "x": 1e39, "y": 2, "z": 0, "diameter": 4, "height": 6 }] }), "beyond"),
        (
            json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "name": "b.stlb", "solid": "oaks" }),
            "no region names",
        ),
        (
            json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "solid": "Bäume" }),
            "printable ASCII only",
        ),
        (
            json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "solid": "ä".repeat(100_000) }),
            "at most 255 characters",
        ),
        (json!({ "trees": [{ "x": 1, "y": 2, "diameter": 4, "height": 6 }] }), "`z` is a number"),
        (json!({ "table": "constant/triSurface/none.txt" }), "no such file"),
        (
            json!({ "trees": [{ "x": 1, "y": 2, "z": 0, "diameter": 4, "height": 6 }], "name": "../x.stl" }),
            "not a surface",
        ),
    ] {
        let e = s.execute("geometry.trees", &p).unwrap_err();
        assert!(e.to_string().contains(says), "{p}: {e}");
    }
}

#[test]
fn workflow_show_and_import() {
    let mut s = session();
    // No description yet: as the case's scripts and meshes have it.
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "scripts");
    assert!(out.message.starts_with("no system/umcfWorkflow: as the scripts and meshes have it"), "{}", out.message);
    let regions = out.data["description"]["regions"].as_array().unwrap().clone();
    assert!(regions.iter().any(|r| r["name"] == "air" && r["kind"] == "fluid"), "{regions:?}");

    let out = s.execute("workflow.import", &Value::Null).unwrap();
    assert!(out.message.starts_with("system/umcfWorkflow: written from the case"), "{}", out.message);
    assert_eq!(s.case().unwrap().changed(), ["system/umcfWorkflow"]);
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "file");
    assert_eq!(out.data["problems"], json!([]));
    assert_eq!(out.data["description"]["regions"].as_array().unwrap().len(), regions.len());

    let e = s.execute("workflow.import", &Value::Null).unwrap_err();
    assert!(e.to_string().contains("pass \"replace\": true"), "{e}");
    let out = s.execute("workflow.import", &json!({ "replace": true })).unwrap();
    assert!(out.message.contains("over the one there: its comments and hand edits are gone"), "{}", out.message);
    // Undone twice, the case has none again.
    s.execute("edit.undo", &Value::Null).unwrap();
    s.execute("edit.undo", &Value::Null).unwrap();
    assert!(s.case().unwrap().changed().is_empty());
    assert_eq!(s.execute("workflow.show", &Value::Null).unwrap().data["source"], "scripts");

    s.execute("workflow.import", &Value::Null).unwrap();
    s.execute("case.save", &Value::Null).unwrap();
    assert!(text(&s, "system/umcfWorkflow").contains("object      umcfWorkflow;"));
    assert!(s.execute("workflow.import", &json!({ "replace": "yes" })).is_err());
}

/// The three edit commands through the session: each change, its history and journal, saving, and the refusals.
#[test]
fn the_description_is_edited_entry_by_entry() {
    let mut s = session();
    // Without a description: refused, saying how to get one.
    let e = s.execute("workflow.region", &json!({ "region": "roofs", "kind": "solid" })).unwrap_err();
    assert!(e.to_string().contains("workflow.import"), "{e}");
    s.execute("workflow.import", &Value::Null).unwrap();
    let region = |s: &mut Session, name: &str| -> Value {
        let out = s.execute("workflow.show", &Value::Null).unwrap();
        assert_eq!(out.data["problems"], json!([]));
        out.data["description"]["regions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // A region, a material, a patch with a role and a partner, then renamed.
    let out = s.execute("workflow.region", &json!({ "region": "roofs", "kind": "solid" })).unwrap();
    assert_eq!(out.message, "system/umcfWorkflow: region roofs added, solid");
    assert_eq!(region(&mut s, "roofs")["kind"], "solid");
    s.execute("workflow.material", &json!({ "region": "roofs", "material": "tiles", "model": "Hamstad5Brick" }))
        .unwrap();
    assert_eq!(region(&mut s, "roofs")["materials"], json!([{ "name": "tiles", "model": "Hamstad5Brick" }]));
    let out = s
        .execute(
            "workflow.patch",
            &json!({ "region": "roofs", "patch": "top", "role": "coupled", "partner": { "region": "air", "patch": "roofs" } }),
        )
        .unwrap();
    assert_eq!(out.message, "system/umcfWorkflow: patch top of region roofs added");
    let out = s.execute("workflow.patch", &json!({ "region": "roofs", "patch": "top", "name": "outside" })).unwrap();
    assert_eq!(out.message, "system/umcfWorkflow: patch top of region roofs is now outside");
    assert_eq!(
        region(&mut s, "roofs")["patches"],
        json!([{ "name": "outside", "role": "coupled", "partner": { "region": "air", "patch": "roofs" } }])
    );
    s.execute("workflow.patch", &json!({ "region": "roofs", "patch": "outside", "role": null, "partner": null }))
        .unwrap();
    assert_eq!(region(&mut s, "roofs")["patches"], json!([{ "name": "outside", "role": null, "partner": null }]));
    s.execute("workflow.material", &json!({ "region": "roofs", "material": "tiles", "remove": true })).unwrap();
    assert_eq!(region(&mut s, "roofs")["materials"], json!([]));
    // Each is one edit in the history and the journal.
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(region(&mut s, "roofs")["materials"].as_array().map(Vec::len), Some(1));
    let out = s.execute("workflow.region", &json!({ "region": "roofs", "remove": true })).unwrap();
    assert!(out.message.starts_with("system/umcfWorkflow: region roofs taken out"), "{}", out.message);
    assert_eq!(region(&mut s, "roofs"), Value::Null);
    let journaled: Vec<&str> =
        s.journal().iter().filter_map(|j| j["command"].as_str()).filter(|c| c.starts_with("workflow.")).collect();
    assert!(journaled.contains(&"workflow.patch") && journaled.contains(&"workflow.material"), "{journaled:?}");
    // Saved, the file reads back the same.
    s.execute("case.save", &Value::Null).unwrap();
    assert!(!text(&s, "system/umcfWorkflow").contains("roofs"));
    // Wrong parameters: refused, nothing changed.
    let before = text(&s, "system/umcfWorkflow");
    for (id, p, says) in [
        ("workflow.region", json!({ "region": "x" }), "give kind"),
        ("workflow.region", json!({ "region": "x", "kind": "solid", "remove": true }), "give kind"),
        ("workflow.region", json!({ "region": "x", "kind": "rock" }), "fluid, solid or vegetation"),
        ("workflow.region", json!({ "region": "a b", "kind": "solid" }), "not a plain OpenFOAM name"),
        ("workflow.patch", json!({ "region": "air", "patch": "west" }), "nothing to change"),
        ("workflow.patch", json!({ "region": "air", "patch": "west", "role": "door" }), "role is one of"),
        ("workflow.patch", json!({ "region": "air", "patch": "west", "partner": "street" }), "partner is"),
        ("workflow.material", json!({ "region": "air", "material": "brick" }), "not a solid"),
        ("workflow.material", json!({ "region": "street", "material": "brick", "model": 3 }), "model is a name"),
    ] {
        let e = s.execute(id, &p).unwrap_err();
        assert!(e.to_string().contains(says), "{id} {p}: {e}");
    }
    assert_eq!(text(&s, "system/umcfWorkflow"), before);
    assert!(s.case().unwrap().changed().is_empty());
}

/// The scripts written from the description: refused without one or with problems in it,
/// previewed, written new, refused over different ones without `replace`, saved, undone.
#[test]
fn the_scripts_are_written_from_the_description() {
    let mut s = session();
    let e = s.execute("workflow.scripts", &Value::Null).unwrap_err();
    assert!(e.to_string().contains("workflow.import"), "{e}");
    // Nor previewed from the import a show makes up.
    let out = s.execute("workflow.show", &json!({ "scripts": true })).unwrap();
    assert!(
        out.data["scripts"].is_null()
            && out.data["scriptsError"].as_str().is_some_and(|m| m.contains("workflow.import"))
    );
    s.execute("workflow.import", &Value::Null).unwrap();
    // Shown with the description, they change nothing and leave no step in the history.
    let undo = s.can_undo();
    let out = s.execute("workflow.show", &json!({ "scripts": true })).unwrap();
    let names: Vec<&str> = out.data["scripts"].as_array().unwrap().iter().filter_map(|x| x["file"].as_str()).collect();
    assert_eq!(names, ["Allprepare", "Allrun", "Allclean"]);
    assert_eq!(s.case().unwrap().put_paths().count(), 0);
    assert_eq!(s.can_undo(), undo);
    assert!(s.execute("workflow.scripts", &json!({ "preview": true })).is_err(), "no preview here");
    // None there yet: written.
    let out = s.execute("workflow.scripts", &Value::Null).unwrap();
    assert!(
        out.message.starts_with("Allprepare, Allrun, Allclean: written from system/umcfWorkflow"),
        "{}",
        out.message
    );
    let allrun = out.data["scripts"][1]["text"].as_str().unwrap().to_string();
    assert!(allrun.starts_with("#!"), "{allrun}");
    // As written: nothing to do.
    let out = s.execute("workflow.scripts", &Value::Null).unwrap();
    assert!(out.message.contains("nothing to write"), "{}", out.message);
    s.execute("case.save", &Value::Null).unwrap();
    assert_eq!(text(&s, "Allrun"), allrun);
    // The case reads the scripts as written: its workflow is theirs.
    assert!(s.case().unwrap().scripts.iter().any(|x| x.path == "Allrun" && !x.steps.is_empty()));
    // The description changes: the scripts there differ, and are written over only on request.
    s.execute("workflow.region", &json!({ "region": "roofs", "kind": "solid" })).unwrap();
    let e = s.execute("workflow.scripts", &Value::Null).unwrap_err();
    assert!(e.to_string().contains("pass \"replace\": true"), "{e}");
    let out = s.execute("workflow.scripts", &json!({ "replace": true })).unwrap();
    assert!(!out.data["changed"].as_array().unwrap().is_empty());
    assert!(out.data["scripts"][1]["text"].as_str().unwrap().contains("roofs"));
    s.execute("edit.undo", &Value::Null).unwrap();
    assert!(s.execute("workflow.show", &json!({ "scripts": true })).unwrap().data["scripts"].is_array());
    // A description with a problem: refused.
    s.execute("dict.set", &json!({ "file": "system/umcfWorkflow", "path": ["colour"], "value": "blue" })).unwrap();
    let e = s.execute("workflow.scripts", &json!({ "replace": true })).unwrap_err();
    assert!(e.to_string().contains("1 problem (colour: is not a key of the description)"), "{e}");
    let out = s.execute("workflow.show", &json!({ "scripts": true })).unwrap();
    assert!(out.data["scripts"].is_null() && out.data["scriptsError"].as_str().is_some_and(|m| m.contains("problems")));
}

#[test]
fn workflow_show_says_a_directory_there_is_unreadable() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs().with("system/umcfWorkflow/x", "a")), "mem", OpenOptions::default(), false).unwrap();
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "unreadable");
    let e = s.execute("workflow.import", &json!({ "replace": true })).unwrap_err();
    assert!(e.to_string().contains("doesn't load"), "{e}");
}

#[test]
fn workflow_show_says_when_the_file_is_there_but_unreadable() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs().with("system/umcfWorkflow", "not { a dictionary")), "mem", OpenOptions::default(), false)
        .unwrap();
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "unreadable");
    assert!(out.message.contains("doesn't load (not a dictionary"), "{}", out.message);
    for p in [Value::Null, json!({ "replace": true })] {
        let e = s.execute("workflow.import", &p).unwrap_err();
        assert!(e.to_string().contains("mend or remove it first"), "{e}");
    }
}

#[test]
fn workflow_show_after_an_undo_past_a_save() {
    let mut s = session();
    s.execute("workflow.import", &Value::Null).unwrap();
    s.execute("case.save", &Value::Null).unwrap();
    s.execute("edit.undo", &Value::Null).unwrap();
    // The saved file is still on disk, a good dictionary the open case doesn't hold.
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "disk");
    assert!(out.message.contains("written or undone since the case was opened"), "{}", out.message);
    let e = s.execute("workflow.import", &json!({ "replace": true })).unwrap_err();
    assert!(e.to_string().contains("edit.redo or open the case again first"), "{e}");
}

#[test]
fn workflow_show_counts_a_description_over_the_dictionary_limit_as_unreadable() {
    // A good description, too large for the limit the case was opened with: it didn't load,
    // and a reread with a larger limit mustn't call it a file the open case could hold.
    let big = format!(
        "FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class dictionary;\n    object umcfWorkflow;\n}}\n// {}\nregions {{ }}\n",
        "x".repeat(4096)
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let options = OpenOptions { dict_bytes: 2048, ..OpenOptions::default() };
    s.open(Box::new(fs().with("system/umcfWorkflow", big)), "mem", options, false).unwrap();
    let out = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(out.data["source"], "unreadable");
}

/// A case's files, counting how often each mesh's `points` and each surface is read.
struct CountingFs {
    inner: MemFs,
    reads: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl umcf_core::CaseFs for CountingFs {
    fn size(&self, rel: &str) -> Option<u64> {
        self.inner.size(rel)
    }
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, umcf_core::FsError> {
        if rel.ends_with("/points") || rel.ends_with(".stl") {
            *self.reads.lock().unwrap().entry(rel.to_string()).or_default() += 1;
        }
        self.inner.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.inner.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.inner.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, umcf_core::FsError> {
        self.inner.list(rel)
    }
}

impl umcf_core::CaseWrite for CountingFs {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), umcf_core::FsError> {
        self.inner.write(rel, bytes)
    }
}

/// Two regions, air and ground, each a unit cube, ground one metre lower.
fn two_region_fs() -> MemFs {
    let mut fs = fs();
    fs.insert("system/controlDict", control().replace("fluid (air);", "fluid (air);\n    solid (ground);"));
    cube_mesh(&mut fs, "constant/air/polyMesh");
    cube_mesh(&mut fs, "constant/ground/polyMesh");
    let ground = umcf_core::CaseFs::read(&fs, "constant/ground/polyMesh/points", 1 << 20).unwrap().unwrap();
    let lowered = String::from_utf8(ground).unwrap().replace(" 0)\n", " -1)\n").replace(" 1)\n", " 0)\n");
    fs.insert("constant/ground/polyMesh/points", lowered);
    fs
}

/// A session of `fs`, and the reads of each mesh's points.
type Reads = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>;

fn counting_session(fs: MemFs) -> (Session, Reads) {
    let reads: Reads = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(CountingFs { inner: fs, reads: reads.clone() }), "mem", OpenOptions::default(), false).unwrap();
    (s, reads)
}

fn two_region_session() -> (Session, Reads) {
    counting_session(two_region_fs())
}

/// Building the scene counts a step for each object read and each built, and expects the bytes
/// of the files it reads.
#[test]
fn the_scene_counts_its_progress() {
    let (mut s, _) = two_region_session();
    s.scene(&json!({})).unwrap();
    let now = s.canceller().progress().get();
    assert_eq!((now.done, now.total), (4, 4), "{now:?}");
    assert!(now.expected > 0, "{now:?}");
}

/// Showing fewer regions and then all again reads no mesh twice; `reload` reads them again.
#[test]
fn the_scene_reads_each_mesh_once() {
    let (mut s, reads) = two_region_session();
    let count = |r: &str| reads.lock().unwrap().get(&format!("constant/{r}/polyMesh/points")).copied().unwrap_or(0);
    let snap = |s: &mut Session, p: Value| s.execute("scene.snapshot", &p).unwrap().data["camera"].clone();
    let all = snap(&mut s, json!({ "width": 8, "height": 8 }));
    assert_eq!((count("air"), count("ground")), (1, 1));
    snap(&mut s, json!({ "regions": ["ground"], "width": 8, "height": 8 }));
    snap(&mut s, json!({ "regions": ["air"], "width": 8, "height": 8 }));
    // The same scene as the first time: the same objects at the same origin.
    assert_eq!(snap(&mut s, json!({ "width": 8, "height": 8 })), all);
    assert_eq!((count("air"), count("ground")), (1, 1));
    snap(&mut s, json!({ "regions": ["air"], "reload": true, "width": 8, "height": 8 }));
    assert_eq!((count("air"), count("ground")), (2, 1));
    // After a reload the other region is read again too when it is shown (its objects went).
    snap(&mut s, json!({ "width": 8, "height": 8 }));
    assert_eq!((count("air"), count("ground")), (2, 2));
}

/// Hidden objects are kept within the budget for them, the ones shown least recently dropped
/// first: a region dropped is read again when it is shown, one kept is not.
#[test]
fn hidden_regions_are_kept_within_the_budget() {
    let mut fs = two_region_fs();
    fs.insert("system/controlDict", control().replace("fluid (air);", "fluid (air);\n    solid (ground roofs);"));
    cube_mesh(&mut fs, "constant/roofs/polyMesh");
    let (mut s, reads) = counting_session(fs);
    let count = |r: &str| reads.lock().unwrap().get(&format!("constant/{r}/polyMesh/points")).copied().unwrap_or(0);
    let one = s.scene(&json!({ "regions": ["air"] })).unwrap().objects[0].bytes();
    assert!(one > 0);
    // Room for one hidden region.
    s.set_budget(Budget { read: u64::MAX, spare: one });
    s.scene(&json!({})).unwrap();
    assert_eq!((count("air"), count("ground"), count("roofs")), (1, 1, 1));
    // Ground, then roofs, are hidden: roofs, hidden last, is kept.
    s.scene(&json!({ "regions": ["air"] })).unwrap();
    s.scene(&json!({})).unwrap();
    assert_eq!((count("air"), count("ground"), count("roofs")), (1, 2, 1));
    // Roofs is hidden first, then ground: ground, hidden more recently, is kept, roofs goes.
    s.scene(&json!({ "regions": ["air", "ground"] })).unwrap();
    s.scene(&json!({ "regions": ["air"] })).unwrap();
    s.scene(&json!({})).unwrap();
    assert_eq!((count("air"), count("ground"), count("roofs")), (1, 2, 2));
    // No room: every hidden region goes.
    s.set_budget(Budget { read: u64::MAX, spare: 0 });
    s.scene(&json!({ "regions": ["air"] })).unwrap();
    s.scene(&json!({})).unwrap();
    assert_eq!((count("air"), count("ground"), count("roofs")), (1, 3, 3));
}

/// A read budget smaller than any read (the regions read one at a time: see the scheduler's
/// own tests) draws the same scene.
#[test]
fn a_small_read_budget_draws_the_same_scene() {
    let (mut s, _) = two_region_session();
    let all = s.execute("scene.snapshot", &json!({ "width": 8, "height": 8 })).unwrap().data["camera"].clone();
    let (mut t, _) = two_region_session();
    t.set_budget(Budget { read: 1, spare: 0 });
    let one = t.execute("scene.snapshot", &json!({ "width": 8, "height": 8 })).unwrap().data["camera"].clone();
    assert_eq!(one, all);
}

/// A patch renamed by a command is shown by its new name, also for a region shown before.
#[test]
fn a_spare_region_follows_a_renamed_patch() {
    let (mut s, _) = two_region_session();
    let names = |s: &mut Session, r: &str| -> Vec<String> {
        let scene = s.scene(&json!({ "regions": [r] })).unwrap();
        scene.objects.iter().flat_map(|o| o.classes.clone()).collect()
    };
    assert_eq!(names(&mut s, "air"), ["inlet"]);
    assert_eq!(names(&mut s, "ground"), ["inlet"]);
    s.execute("select.set", &json!({ "items": [{ "region": "air", "patch": "inlet" }] })).unwrap();
    s.execute("patch.set", &json!({ "name": "sides" })).unwrap();
    assert_eq!(names(&mut s, "air"), ["sides"]);
    assert_eq!(names(&mut s, "ground"), ["inlet"]);
}

/// A region in projected coordinates shown after one near the world's origin: the scene
/// moves its origin to what it shows, so the far mesh's points stay precise in single precision,
/// and the region kept from before moves with it.
#[test]
fn a_far_region_gets_an_origin_of_its_own() {
    let mut fs = two_region_fs();
    // Ground at (2 600 000, 1 200 000), as LV95 coordinates put it.
    let h = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class vectorField;\n}\n";
    let corners: String = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]
        .iter()
        .map(|(x, y, z)| format!("({} {} {z})\n", 2_600_000 + x, 1_200_000 + y))
        .collect();
    fs.insert("constant/ground/polyMesh/points", format!("{h}8\n(\n{corners})\n"));
    let (mut s, _) = counting_session(fs);
    assert!(s.scene(&json!({ "regions": ["air"] })).unwrap().origin.length() < 10.0);
    let scene = s.scene(&json!({ "regions": ["ground"] })).unwrap();
    assert!((scene.origin - umcf_geom::Vec3::new(2.6e6, 1.2e6, 0.0)).length() < 10.0, "{:?}", scene.origin);
    // Exact: the points are within a metre of the origin.
    let b = scene.bbox().unwrap();
    assert_eq!((b.min.x, b.max.x, b.min.y, b.max.y), (2.6e6, 2_600_001.0, 1.2e6, 1_200_001.0));
    // Both: air is where it was.
    let scene = s.scene(&json!({ "regions": ["air", "ground"] })).unwrap();
    let air = scene.objects.iter().find(|o| o.label() == "mesh air").unwrap().bbox(scene.origin).unwrap();
    assert!(air.min.x.abs() < 0.5 && (air.max.x - 1.0).abs() < 0.5, "{air:?}");
}

/// Shown, hidden while the origin is far away, and shown again: a region's points are as exact
/// as the first time. It is read again, never moved away and back in single precision.
#[test]
fn a_region_back_from_far_away_is_exact() {
    let mut fs = two_region_fs();
    let h = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class vectorField;\n}\n";
    let corners: String = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]
        .iter()
        .map(|(x, y, z)| format!("({} {} {z})\n", 2_600_000 + x, 1_200_000 + y))
        .collect();
    fs.insert("constant/ground/polyMesh/points", format!("{h}8\n(\n{corners})\n"));
    // Air's points a fraction of a millimetre from whole numbers: lost at 2.6e6 m in f32.
    let fine = umcf_core::CaseFs::read(&fs, "constant/air/polyMesh/points", 1 << 20).unwrap().unwrap();
    fs.insert("constant/air/polyMesh/points", String::from_utf8(fine).unwrap().replace("(1 ", "(1.0003 "));
    let (mut s, reads) = counting_session(fs);
    let air_points = |s: &mut Session| s.scene(&json!({ "regions": ["air"] })).unwrap().objects[0].points.clone();
    let first = air_points(&mut s);
    // Relative to the origin (the box's middle): 1.0003 is 0.50015 from it.
    assert!(first.iter().any(|p| p[0] == 0.50015_f32), "{first:?}");
    s.scene(&json!({ "regions": ["ground"] })).unwrap();
    assert_eq!(air_points(&mut s), first);
    assert_eq!(reads.lock().unwrap().get("constant/air/polyMesh/points"), Some(&2));
}

/// Undo and redo of a rename in a region that is hidden in between show the right names.
#[test]
fn undo_and_redo_reach_hidden_regions() {
    let (mut s, _) = two_region_session();
    let names = |s: &mut Session, r: &str| -> Vec<String> {
        s.scene(&json!({ "regions": [r] })).unwrap().objects.iter().flat_map(|o| o.classes.clone()).collect()
    };
    assert_eq!(names(&mut s, "air"), ["inlet"]);
    s.execute("select.set", &json!({ "items": [{ "region": "air", "patch": "inlet" }] })).unwrap();
    s.execute("patch.set", &json!({ "name": "sides" })).unwrap();
    assert_eq!(names(&mut s, "air"), ["sides"]);
    assert_eq!(names(&mut s, "ground"), ["inlet"]);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(names(&mut s, "ground"), ["inlet"]);
    assert_eq!(names(&mut s, "air"), ["inlet"]);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(names(&mut s, "ground"), ["inlet"]);
    assert_eq!(names(&mut s, "air"), ["sides"]);
}

/// A surface shown, hidden and shown again is read once; a surface a command changed since is
/// read again from what the command put, not from disk.
#[test]
fn the_scene_reads_each_surface_once() {
    let mut fs = two_region_fs();
    fs.insert("constant/triSurface/box.stl", cube_stl(umcf_geom::stl::Format::Ascii, false));
    let (mut s, reads) = counting_session(fs);
    let count = || reads.lock().unwrap().get("constant/triSurface/box.stl").copied().unwrap_or(0);
    let surface = json!({ "surfaces": ["box.stl"] });
    s.scene(&surface).unwrap();
    s.scene(&json!({ "regions": ["air"] })).unwrap();
    let triangles = s.scene(&surface).unwrap().triangles();
    assert_eq!(count(), 1);
    // Scaled by a command: the scene shows the new surface (put, not yet saved).
    s.execute("geometry.scale", &json!({ "file": "box.stl", "factor": 2 })).unwrap();
    let scene = s.scene(&surface).unwrap();
    assert_eq!(scene.triangles(), triangles);
    let b = scene.bbox().unwrap();
    assert!((b.max.x - b.min.x - 2.0).abs() < 1e-6, "{b:?}");
}

/// A session can move to another thread (a job), and its cancel flag stops what it reads.
#[test]
fn a_cancelled_command_leaves_the_case_as_it_was() {
    fn send<T: Send>() {}
    send::<Session>();
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let cancel = s.canceller();
    s.open(
        Box::new(umcf_core::Cancellable::new(two_region_fs(), cancel.clone())),
        "mem",
        OpenOptions::default(),
        false,
    )
    .unwrap();
    // The scene, cancelled before it is read: an error that says so, nothing cached.
    cancel.cancel();
    let e = s.scene(&json!({ "regions": ["air"] })).unwrap_err();
    assert!(matches!(e, Error::Cancelled { ref id } if id == "scene"), "{e}");
    // A command: cancelled, and the case is as it was.
    let before = s.case().cloned();
    let e = s.execute("scene.snapshot", &json!({ "regions": ["air"] })).unwrap_err();
    assert!(matches!(e, Error::Cancelled { .. }), "{e}");
    assert!(e.to_string().contains("as it was"), "{e}");
    assert_eq!(s.case().map(|c| c.regions.len()), before.map(|c| c.regions.len()));
    // Reset, it reads again.
    cancel.reset();
    assert!(s.scene(&json!({ "regions": ["air"] })).is_ok());
}

/// `case.open` cancelled keeps the case that was open.
#[test]
fn a_cancelled_open_keeps_the_open_case() {
    // A case of each OpenFOAM version.
    for example in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of8/streetCanyon_CFD"),
    ] {
        let mut s = Session::with_backup(".umcfgui/backup/test");
        s.open(Box::new(two_region_fs()), "mem", OpenOptions::default(), false).unwrap();
        s.canceller().cancel();
        let e = s.execute("case.open", &json!({ "path": example })).unwrap_err();
        assert!(matches!(e, Error::Cancelled { ref id } if id == "case.open"), "{example}: {e}");
        assert_eq!(s.location(), Some("mem"));
        // Not cancelled, it opens.
        s.canceller().reset();
        s.execute("case.open", &json!({ "path": example })).unwrap();
        assert_eq!(s.location(), Some(example));
    }
}

/// A case that sets the cancel flag when one file is read, as a click on *Cancel* would while
/// a command reads.
struct Trip {
    inner: MemFs,
    at: &'static str,
    cancel: umcf_core::Cancel,
}

impl umcf_core::CaseFs for Trip {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, umcf_core::FsError> {
        if rel == self.at {
            self.cancel.cancel();
        }
        self.inner.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.inner.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.inner.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, umcf_core::FsError> {
        self.inner.list(rel)
    }
}

impl umcf_core::CaseWrite for Trip {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), umcf_core::FsError> {
        self.inner.write(rel, bytes)
    }
}

/// Cancelled in the middle of reading, where the reads that fail would only be notes: still
/// cancelled, and the session as it was.
#[test]
fn a_command_cancelled_while_it_reads_is_cancelled() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(two_region_fs()), "first", OpenOptions::default(), false).unwrap();
    let cancel = s.canceller();
    // Opening another case: the one open stays.
    let trip = Trip { inner: two_region_fs(), at: "constant/air/polyMesh/boundary", cancel: cancel.clone() };
    let e =
        s.open(Box::new(umcf_core::Cancellable::new(trip, cancel.clone())), "second", OpenOptions::default(), false);
    assert!(matches!(e, Err(Error::Cancelled { .. })), "{e:?}");
    assert_eq!(s.location(), Some("first"));
    // A check whose unreadable meshes would be diagnostics.
    cancel.reset();
    let trip = Trip { inner: two_region_fs(), at: "constant/air/polyMesh/points", cancel: cancel.clone() };
    s.open(Box::new(umcf_core::Cancellable::new(trip, cancel.clone())), "third", OpenOptions::default(), false)
        .unwrap();
    let e = s.execute("mesh.check", &json!({})).unwrap_err();
    assert!(matches!(e, Error::Cancelled { ref id } if id == "mesh.check"), "{e}");
    assert_eq!(s.location(), Some("third"));
    // A snapshot whose mesh read was cancelled leaves no scene: the next one reads it again.
    cancel.reset();
    let reads = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let counted = CountingFs { inner: two_region_fs(), reads: reads.clone() };
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let cancel = s.canceller();
    s.open(Box::new(umcf_core::Cancellable::new(counted, cancel.clone())), "fourth", OpenOptions::default(), false)
        .unwrap();
    assert!(s.scene(&json!({ "regions": ["air"] })).is_ok());
    cancel.cancel();
    let r = s.execute("scene.snapshot", &json!({ "regions": ["air"] }));
    assert!(matches!(r, Err(Error::Cancelled { .. })), "{r:?}");
    cancel.reset();
    let before = reads.lock().unwrap().values().sum::<usize>();
    assert!(s.scene(&json!({ "regions": ["air"] })).is_ok());
    assert!(reads.lock().unwrap().values().sum::<usize>() > before, "the scene was kept");
}

/// A command that only reads writes nothing to the journal or the undo history.
#[test]
fn commands_that_only_read_change_nothing() {
    for c in COMMANDS {
        if c.reads_only {
            assert!(!c.journal && !c.undoable, "{} reads only but is journaled or undoable", c.id);
        }
    }
    assert!(COMMANDS.iter().filter(|c| c.reads_only).count() >= 5);
}

/// A case whose `points` stream sets the cancel flag after its first read, as a click on
/// *Cancel* would partway through a big file; whole-file reads of `points` are counted.
struct MidStream {
    inner: MemFs,
    cancel: umcf_core::Cancel,
    whole: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

struct Tripping {
    inner: std::io::Cursor<Vec<u8>>,
    cancel: umcf_core::Cancel,
}

impl std::io::Read for Tripping {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.cancel.cancel();
        Ok(n)
    }
}

impl umcf_core::CaseFs for MidStream {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, umcf_core::FsError> {
        if rel.ends_with("/points") {
            self.whole.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.read(rel, limit)
    }
    fn reader(&self, rel: &str, limit: u64) -> Result<Option<Box<dyn std::io::Read + Send>>, umcf_core::FsError> {
        let cancel = self.cancel.clone();
        Ok(self.inner.read(rel, limit)?.map(|b| {
            if rel.ends_with("/points") {
                Box::new(Tripping { inner: std::io::Cursor::new(b), cancel }) as Box<dyn std::io::Read + Send>
            } else {
                Box::new(std::io::Cursor::new(b)) as Box<dyn std::io::Read + Send>
            }
        }))
    }
    fn is_file(&self, rel: &str) -> bool {
        self.inner.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.inner.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, umcf_core::FsError> {
        self.inner.list(rel)
    }
}

impl umcf_core::CaseWrite for MidStream {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), umcf_core::FsError> {
        self.inner.write(rel, bytes)
    }
}

/// Cancelled partway through a streamed `points` file of several megabytes: the scene is
/// cancelled, and the file is not read again whole.
#[test]
fn a_cancel_stops_a_streamed_file_partway() {
    let mut fs = two_region_fs();
    // The cube's eight points, then 300 000 more nothing uses: some 7 MB of text.
    let mut points = String::from("FoamFile\n{\n    format ascii;\n    class vectorField;\n}\n300008\n(\n");
    points.push_str("(0 0 0)\n(1 0 0)\n(1 1 0)\n(0 1 0)\n(0 0 1)\n(1 0 1)\n(1 1 1)\n(0 1 1)\n");
    for i in 0..300_000 {
        points.push_str(&format!("({i}.5 0.25 0.125)\n"));
    }
    points.push_str(")\n");
    fs.insert("constant/air/polyMesh/points", points);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let cancel = s.canceller();
    let whole = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fs = MidStream { inner: fs, cancel: cancel.clone(), whole: whole.clone() };
    s.open(Box::new(umcf_core::Cancellable::new(fs, cancel.clone())), "mem", OpenOptions::default(), false).unwrap();
    whole.store(0, std::sync::atomic::Ordering::SeqCst);
    let e = s.scene(&json!({ "regions": ["air"] })).unwrap_err();
    assert!(matches!(e, Error::Cancelled { .. }), "{e}");
    assert_eq!(whole.load(std::sync::atomic::Ordering::SeqCst), 0, "read again whole");
}

#[test]
fn fields_opened_later_are_read_by_case_fields_with_progress() {
    // A case of each OpenFOAM version.
    for example in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of8/streetCanyon_CFD"),
    ] {
        let mut s = Session::with_backup(".umcfgui/backup/test");
        s.execute("case.open", &json!({ "path": example, "fields": "later" })).unwrap();
        assert!(s.case().unwrap().fields_pending, "{example}");
        // Cancelled: the case is as it was, its fields still to read.
        s.canceller().cancel();
        let e = s.execute("case.fields", &json!({})).unwrap_err();
        assert!(matches!(e, Error::Cancelled { .. }), "{example}: {e}");
        assert!(s.case().unwrap().fields_pending);
        s.canceller().reset();
        // Read, each file counted.
        let o = s.execute("case.fields", &json!({})).unwrap();
        assert_eq!(o.data["read"], json!(true));
        let umcf_core::ProgressNow { done, total, bytes, .. } = s.canceller().progress().get();
        assert!(total > 0 && done == total && bytes > 0, "{example}: {done}/{total}, {bytes} bytes");
        let mut whole = Session::with_backup(".umcfgui/backup/test");
        whole.execute("case.open", &json!({ "path": example })).unwrap();
        assert_eq!(s.case().unwrap().regions, whole.case().unwrap().regions, "{example}");
        // Again: nothing to do.
        assert_eq!(s.execute("case.fields", &json!({})).unwrap().data["read"], json!(false));
        // An edit made before they were read, undone: they stay read.
        let mut s = Session::with_backup(".umcfgui/backup/test");
        s.execute("case.open", &json!({ "path": example, "fields": "later" })).unwrap();
        s.execute("control.set", &json!({ "endTime": 7200 })).unwrap();
        s.execute("case.fields", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.case().unwrap().fields_pending, "{example}");
        assert_eq!(s.case().unwrap().regions, whole.case().unwrap().regions, "{example}");
    }
}

#[test]
fn a_stand_in_shows_the_session_and_reads_nothing() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(two_region_fs()), "mem", OpenOptions::default(), false).unwrap();
    s.execute("control.set", &json!({ "endTime": 7200 })).unwrap();
    s.scene(&json!({ "regions": ["air"] })).unwrap();
    let copy = s.stand_in(true);
    assert_eq!(copy.location(), Some("mem"));
    assert_eq!(copy.case().map(|c| c.controls.end_time.clone()), s.case().map(|c| c.controls.end_time.clone()));
    assert_eq!(copy.history().0.len(), 1);
    // Its scene is the session's, shared: drawn from without reading.
    let shared = |a: &Session, b: &Session| match (&a.scene, &b.scene) {
        (Some(x), Some(y)) => std::sync::Arc::ptr_eq(x, y),
        _ => false,
    };
    assert!(shared(&copy, &s));
    let mut copy = copy;
    assert!(copy.scene(&json!({ "regions": ["air"] })).is_ok(), "from the cache");
    // What needs a file fails, and changes nothing of the session's.
    assert!(copy.scene(&json!({ "regions": ["air"], "reload": true })).is_err());
    assert!(s.stand_in(false).scene.is_none());
    // The session builds on, its scene not copied while the copy is gone.
    drop(copy);
    s.scene(&json!({ "regions": ["air", "ground"] })).unwrap();
}

#[test]
fn patch_edits_wait_for_the_fields() {
    // Renaming or adding a patch changes the field files with the mesh: not while they are
    // listed only (a case opened with fields "later").
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let options = OpenOptions { defer_fields: true, ..OpenOptions::default() };
    s.open(Box::new(two_region_fs()), "mem", options, false).unwrap();
    let first = s.case().unwrap().region("air").unwrap().patches.as_ref().unwrap()[0].name.clone();
    let rename = json!({ "patches": [{ "region": "air", "patch": first }], "name": "renamed" });
    let e = s.execute("patch.set", &rename).unwrap_err().to_string();
    assert!(e.contains("aren't read yet"), "{e}");
    let e = s.execute("patch.create", &json!({ "name": "x", "region": "air", "faces": [3] })).unwrap_err().to_string();
    assert!(e.contains("aren't read yet"), "{e}");
    s.execute("case.fields", &json!({})).unwrap();
    s.execute("patch.set", &rename).unwrap();
}

fn canyon_params(path: &std::path::Path, version: u32) -> Value {
    json!({
        "path": path.to_str().unwrap(),
        "version": version,
        "regions": [
            {"name": "air", "kind": "fluid"},
            {"name": "street", "kind": "solid", "materials": [{"name": "soil", "model": "Soil"}]},
            {"name": "vegetation", "kind": "vegetation"},
        ],
        "radiation": "vegetation",
        "grass": ["air_to_street"],
        "temperatureLimit": true,
    })
}

#[test]
fn case_new_writes_a_case_and_opens_it() {
    for version in [8, 12] {
        let dir = std::env::temp_dir().join(format!("umcf-engine-{}-new-{version}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::with_backup(".umcfgui/backup/test");
        let out = s.execute("case.new", &canyon_params(&dir, version)).unwrap();
        assert!(
            out.message.starts_with(&format!("made {}: OpenFOAM-{version}, 3 regions;", dir.display())),
            "{}",
            out.message
        );
        let written = out.data["written"].as_array().unwrap();
        assert!(written.iter().all(|w| dir.join(w.as_str().unwrap()).is_file()));
        assert!(
            dir.join("system/air/fvSchemes").is_file() && dir.join("constant/street/transportProperties").is_file()
        );
        assert_eq!(s.location(), dir.to_str());
        let case = s.case().unwrap();
        assert_eq!(case.regions.len(), 3);
        assert!(case.changed().is_empty());
        let show = s.execute("workflow.show", &Value::Null).unwrap();
        assert!(show.data["description"].get("profile").is_none());
        assert_eq!(show.data["description"]["components"], json!(["grass", "temperatureLimit"]));
        // What the check finds is what case.new leaves to later steps: the tables the radiation
        // reads, the meshes, the start time's fields.
        let check = s.execute("case.check", &Value::Null).unwrap();
        for d in check.data["diagnostics"].as_array().unwrap() {
            let code = d["code"].as_str().unwrap();
            assert!(
                ["case.table", "region.no-mesh", "case.note", "fv.limit-pressure"].contains(&code),
                "{version}: {d}"
            );
        }
        // Not empty now: a second one is refused, and nothing is written.
        let e = s.execute("case.new", &canyon_params(&dir, version)).unwrap_err();
        assert!(e.to_string().contains("is not empty"), "{e}");
        // Nothing is left of the staging.
        let stray: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".umcfgui-new-"))
            .collect();
        assert!(stray.is_empty(), "{stray:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn case_new_refuses_before_it_writes() {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-new-refused", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let with = |k: &str, v: Value| {
        let mut p = canyon_params(&dir, 12);
        p[k] = v;
        p
    };
    // An empty directory is fine: the case takes its place.
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = canyon_params(&dir, 12);
    p["regions"] = json!([{"name": "air", "kind": "fluid"}]);
    p["radiation"] = Value::Null;
    p.as_object_mut().unwrap().remove("radiation");
    p.as_object_mut().unwrap().remove("grass");
    s.execute("case.new", &p).unwrap();
    assert!(dir.join("system/controlDict").is_file());
    std::fs::remove_dir_all(&dir).unwrap();
    s = Session::with_backup(".umcfgui/backup/test");
    for (params, want) in [
        (with("version", json!(10)), "version is 8 or 12"),
        (with("profile", json!("hk")), "unknown parameter `profile`"),
        (with("regions", json!([{"name": "air", "kind": "gas"}])), "kind is fluid, solid or vegetation"),
        (with("regions", json!([{"name": "street", "kind": "solid"}])), "needs a fluid region"),
        (
            {
                let mut p = with("radiation", json!("street"));
                p["regions"] = json!([{"name": "air", "kind": "fluid"}, {"name": "street", "kind": "solid"}]);
                p
            },
            "a solid has no radiation",
        ),
        (with("up", json!("w")), "up is x, y or z"),
        (with("grass", json!("lawn")), "grass is true, or a list"),
        (with("colour", json!("red")), "colour"),
        (with("path", json!(" ")), "path is the directory"),
    ] {
        let e = s.execute("case.new", &params).unwrap_err().to_string();
        assert!(e.contains(want), "{e} (wanted {want})");
        assert!(!dir.exists(), "{want}: something was written");
    }
    // An open case with unsaved changes stays, unless given up.
    let a = scratch("new-open");
    s.execute("case.open", &json!({ "path": a.to_str().unwrap() })).unwrap();
    s.execute("control.set", &json!({ "endTime": 9 })).unwrap();
    let e = s.execute("case.new", &canyon_params(&dir, 8)).unwrap_err().to_string();
    assert!(e.contains("unsaved changes"), "{e}");
    assert!(!dir.exists());
    assert_eq!(s.location(), a.to_str());
    let mut p = canyon_params(&dir, 8);
    p["discard"] = json!(true);
    let out = s.execute("case.new", &p).unwrap();
    assert!(out.message.starts_with("made "), "{}", out.message);
    assert_eq!(s.location(), dir.to_str());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&a);
}

#[test]
fn case_new_leaves_things_as_they_were_when_it_fails() {
    let base = std::env::temp_dir().join(format!("umcf-engine-{}-new-fail", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let params = |path: &std::path::Path| json!({ "path": path.to_str().unwrap(), "version": 12, "regions": [{"name": "air", "kind": "fluid"}] });
    // `..` is refused.
    let e = s.execute("case.new", &params(&base.join("a/../b"))).unwrap_err().to_string();
    assert!(e.contains("without `..`"), "{e}");
    assert!(!base.join("a").exists() && !base.join("b").exists());
    // A parent made on the way goes again when the case's own directory can't be made.
    let long = base.join("q").join("n".repeat(300));
    let e = s.execute("case.new", &params(&long)).unwrap_err().to_string();
    assert!(e.contains("nothing was made"), "{e}");
    assert!(!base.join("q").exists());
    // `.` components are no directories of their own.
    s.execute("case.new", &params(&base.join("c/d/."))).unwrap();
    assert!(base.join("c/d/system/controlDict").is_file());
    s = Session::with_backup(".umcfgui/backup/test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The directories made on the way are taken away again: here the case's own can't be
        // made inside a directory that can't be written.
        let locked = base.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::File::create(locked.join("probe")).is_ok();
        if !writable {
            let e = s.execute("case.new", &params(&locked.join("x/y"))).unwrap_err().to_string();
            assert!(e.contains("nothing was made"), "{e}");
            assert_eq!(std::fs::read_dir(&locked).unwrap().count(), 0);
            // An empty directory of the user's stays, with its mode, whatever happens: one that
            // can't be written into is left as it was.
            let mine = base.join("mine");
            std::fs::create_dir(&mine).unwrap();
            std::fs::set_permissions(&mine, std::fs::Permissions::from_mode(0o555)).unwrap();
            let e = s.execute("case.new", &params(&mine)).unwrap_err().to_string();
            assert!(e.contains("nothing was made"), "{e}");
            assert_eq!(std::fs::read_dir(&mine).unwrap().count(), 0);
            assert_eq!(std::fs::metadata(&mine).unwrap().permissions().mode() & 0o777, 0o555);
            std::fs::set_permissions(&mine, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A user's directory made beforehand keeps its mode once the case is in it.
        let kept = base.join("kept");
        std::fs::create_dir(&kept).unwrap();
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o750)).unwrap();
        s.execute("case.new", &params(&kept)).unwrap();
        assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o750);
        assert!(kept.join("system/controlDict").is_file());
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn region_add_lists_the_region_and_is_undone_whole() {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-region-add", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute(
        "case.new",
        &json!({ "path": dir.to_str().unwrap(), "version": 12, "regions": [{"name": "air", "kind": "fluid"}] }),
    )
    .unwrap();
    let out = s
        .execute(
            "region.add",
            &json!({ "region": "street", "kind": "solid", "materials": [{"name": "soil", "model": "Soil"}] }),
        )
        .unwrap();
    assert!(
        out.message.starts_with("region street added, solid: listed in system/controlDict, 4 files written, described"),
        "{}",
        out.message
    );
    assert_eq!(s.case().unwrap().regions.len(), 2);
    let show = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(show.data["description"]["regions"][1]["materials"], json!([{ "name": "soil", "model": "Soil" }]));
    // One step in the history: undone, the case is as it was.
    s.execute("edit.undo", &Value::Null).unwrap();
    assert_eq!(s.case().unwrap().regions.len(), 1);
    assert!(s.case().unwrap().changed().is_empty(), "{:?}", s.case().unwrap().changed());
    s.execute("edit.redo", &Value::Null).unwrap();
    s.execute("case.save", &Value::Null).unwrap();
    assert!(dir.join("constant/street/transportProperties").is_file());
    s.execute("case.open", &json!({ "path": dir.to_str().unwrap() })).unwrap();
    assert_eq!(s.case().unwrap().layout.regions.solid, ["street"]);
    // Refused: nothing changes.
    let e = s.execute("region.add", &json!({ "region": "street", "kind": "solid" })).unwrap_err().to_string();
    assert!(e.contains("there already"), "{e}");
    let e = s.execute("region.add", &json!({ "region": "x", "kind": "rock" })).unwrap_err().to_string();
    assert!(e.contains("kind is fluid, solid or vegetation"), "{e}");
    assert!(s.case().unwrap().changed().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn region_add_to_an_example_without_a_description() {
    let ex = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of8/streetCanyon_CFDHAM");
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute("case.open", &json!({ "path": ex, "fields": false })).unwrap();
    let out = s.execute("region.add", &json!({ "region": "roofs", "kind": "solid" })).unwrap();
    assert!(out.message.contains("listed in constant/regionProperties"), "{}", out.message);
    assert_eq!(out.data["described"], json!(false));
    let rp = s.case().unwrap().dict("constant/regionProperties").unwrap().to_string();
    assert!(rp.contains("solid   (street windward leeward roofs)"), "{rp}");
    // Only in memory: the example isn't written.
}

#[test]
fn workflow_settings_go_into_the_files_and_the_description_and_are_undone_whole() {
    for version in [8, 12] {
        let dir = std::env::temp_dir().join(format!("umcf-engine-{}-settings-{version}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::with_backup(".umcfgui/backup/test");
        let mut p = canyon_params(&dir, version);
        p["settings"] = json!({ "deltaT": 1800, "turbulence": "realizableKE" });
        s.execute("case.new", &p).unwrap();
        let show = s.execute("workflow.show", &Value::Null).unwrap();
        assert_eq!(show.data["description"]["settings"], json!({ "deltaT": "1800", "turbulence": "realizableKE" }));
        let case = s.case().unwrap();
        assert_eq!(case.dict("system/controlDict").unwrap().value(&["deltaT"]).as_deref(), Some("1800"));
        let out =
            s.execute("workflow.settings", &json!({ "endTime": 86400, "temperatureLimit": [283.15, 370.15] })).unwrap();
        assert!(out.message.starts_with("settings chosen: endTime, temperatureLimit; written into"), "{}", out.message);
        assert_eq!(out.data["described"], json!(true));
        let case = s.case().unwrap();
        assert_eq!(case.dict("system/controlDict").unwrap().value(&["endTime"]).as_deref(), Some("86400"));
        let (d, problems) = case.description().unwrap();
        assert!(problems.is_empty());
        assert_eq!(d.settings.temperature_limit, Some(("283.15".into(), "370.15".into())));
        assert_eq!(d.settings.delta_t.as_deref(), Some("1800"));
        s.execute("edit.undo", &Value::Null).unwrap();
        let case = s.case().unwrap();
        assert_eq!(case.dict("system/controlDict").unwrap().value(&["endTime"]).as_deref(), Some("7200"));
        assert!(case.description().unwrap().0.settings.temperature_limit.is_none());
        for (params, want) in [
            (json!({}), "give a setting"),
            (json!({ "deltaT": -1 }), "deltaT is a number above 0"),
            (json!({ "deltaT": "$dt" }), "deltaT is a number or a name"),
            (json!({ "temperatureLimit": [370, 280] }), "temperatureLimit is ( min max )"),
            (json!({ "blending": { "dampingThickness": 60 } }), "blending gives dampingThickness"),
            (json!({ "colour": "red" }), "colour is not a setting"),
            (json!({ "turbulence": "k e" }), "turbulence is the name of a RAS model"),
        ] {
            let e = s.execute("workflow.settings", &params).unwrap_err().to_string();
            assert!(e.contains(want), "{params}: {e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn workflow_settings_take_another_case_s_settings_and_material_library() {
    let a = std::env::temp_dir().join(format!("umcf-engine-{}-settings-from-a", std::process::id()));
    let b = std::env::temp_dir().join(format!("umcf-engine-{}-settings-from-b", std::process::id()));
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let concrete = json!({ "HamstadConcrete": { "rho": 1950, "cap": 850, "lambda1": 1.3, "lambda2": 0 } });
    // case.new takes no `from`: refused, not left out.
    let e = s
        .execute("case.new", &json!({ "path": b.to_str().unwrap(), "version": 8, "regions": [{"name": "air", "kind": "fluid"}], "settings": { "from": "x" } }))
        .unwrap_err()
        .to_string();
    assert!(e.contains("from is workflow.settings' own"), "{e}");
    s.execute(
        "case.new",
        &json!({
            "path": a.to_str().unwrap(), "version": 8,
            "regions": [{"name": "air", "kind": "fluid"}, {"name": "ground", "kind": "solid", "materials": [{"name": "pavement", "model": "HamstadConcrete"}]}],
            "settings": { "turbulence": "realizableKE", "materials": concrete },
        }),
    )
    .unwrap();
    s.execute(
        "case.new",
        &json!({ "path": b.to_str().unwrap(), "version": 8, "regions": [{"name": "air", "kind": "fluid"}] }),
    )
    .unwrap();
    // Read only: the other case is as it was, byte for byte.
    let snapshot = |dir: &std::path::Path| {
        let mut all = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() { stack.push(p) } else { all.push((p.clone(), std::fs::read(&p).unwrap())) }
            }
        }
        all.sort();
        all
    };
    let before = snapshot(&a);
    // Relative to the open case's directory.
    let relative = format!("../{}", a.file_name().unwrap().to_str().unwrap());
    let out = s.execute("workflow.settings", &json!({ "from": relative, "endTime": 600 })).unwrap();
    assert_eq!(snapshot(&a), before);
    // `from` alone: the other case's settings, nothing else given.
    let only = s.execute("workflow.settings", &json!({ "from": relative })).unwrap();
    assert_eq!(only.data["settings"]["turbulence"], json!("realizableKE"), "{}", only.data);
    assert!(out.message.contains("materials"), "{}", out.message);
    assert!(out.message.contains("the material library is kept in system/umcfWorkflow"), "{}", out.message);
    let chosen = &out.data["settings"];
    assert_eq!(chosen["turbulence"], json!("realizableKE"), "{chosen}");
    assert_eq!(chosen["endTime"], json!("600"));
    assert_eq!(chosen["materials"]["HamstadConcrete"]["rho"], json!("1950"), "{chosen}");
    let added = s
        .execute(
            "region.add",
            &json!({ "region": "roofs", "kind": "solid", "materials": [{"name": "slab", "model": "HamstadConcrete"}] }),
        )
        .unwrap();
    assert!(!added.message.contains("coefficients"), "{}", added.message);
    let tp = s.case().unwrap().dict("constant/roofs/transportProperties").unwrap().to_string();
    assert!(tp.contains("rho     1950;"), "{tp}");
    // What the call gives itself goes over the other case's: the whole entry of that model.
    let own = json!({ "HamstadConcrete": { "rho": 2100, "cap": 900, "lambda1": 1.6, "lambda2": 0 } });
    let out = s.execute("workflow.settings", &json!({ "from": relative, "materials": own })).unwrap();
    let chosen = &out.data["settings"];
    assert_eq!(chosen["materials"]["HamstadConcrete"]["rho"], json!("2100"), "{chosen}");
    assert_eq!(chosen["materials"]["HamstadConcrete"]["lambda1"], json!("1.6"), "{chosen}");
    assert_eq!(chosen["materials"]["HamstadConcrete"]["cap"], json!("900"), "{chosen}");
    let d = s.case().unwrap().description().unwrap().0;
    let rho = d
        .settings
        .materials
        .iter()
        .find(|(m, _)| m == "HamstadConcrete")
        .and_then(|(_, c)| c.iter().find(|(k, _)| k == "rho"));
    assert_eq!(rho.map(|(_, v)| v.as_str()), Some("2100"), "{:?}", d.settings.materials);
    for (params, want) in [
        (json!({ "from": "" }), "from is the path of a case"),
        (json!({ "materials": { "HamstadConcrete": { "rho": 1950 } } }), "materials/HamstadConcrete has no cap"),
        (json!({ "materials": { "Soil": 3 } }), "materials/Soil is an object of coefficients"),
        (json!({ "materials": { "Soil": { "rho": "$r" } } }), "materials/Soil/rho is a number"),
        (json!({ "materials": { "Marble": { "rho": 1 } } }), "materials/Marble is not a material model"),
    ] {
        let e = s.execute("workflow.settings", &params).unwrap_err().to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn workflow_settings_choose_the_solver_numerics_of_a_region_kind() {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-numerics", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    // Two cases to take numerics from: one of the same version, one of OpenFOAM-8.
    let other = |v: u8| std::env::temp_dir().join(format!("umcf-engine-{}-numerics-of{v}", std::process::id()));
    for v in [8, 12] {
        let _ = std::fs::remove_dir_all(other(v));
        let numerics = json!({ "fluid": { "SIMPLE": { "nNonOrthogonalCorrectors": 3 }, "relaxationFactors": { "fields": { "p_rgh": 0.2 } } } });
        s.execute(
            "case.new",
            &json!({ "path": other(v).to_str().unwrap(), "version": v, "regions": [{"name": "air", "kind": "fluid"}], "settings": { "numerics": numerics } }),
        )
        .unwrap();
    }
    s.execute(
        "case.new",
        &json!({ "path": dir.to_str().unwrap(), "version": 12, "regions": [{"name": "air", "kind": "fluid"}] }),
    )
    .unwrap();
    // Those given go over the other case's, entry by entry.
    let from = other(12).to_str().unwrap().to_string();
    let out = s
        .execute(
            "workflow.settings",
            &json!({ "from": from, "numerics": { "fluid": { "SIMPLE": { "nNonOrthogonalCorrectors": 5 } } } }),
        )
        .unwrap();
    assert!(out.message.contains("numerics"), "{}", out.message);
    let fluid = &out.data["settings"]["numerics"]["fluid"];
    assert_eq!(fluid["SIMPLE"]["nNonOrthogonalCorrectors"], json!("5"), "{fluid}");
    assert_eq!(fluid["relaxationFactors"]["fields"]["p_rgh"], json!("0.2"), "{fluid}");
    // Another version's: left out, with a note.
    let out = s.execute("workflow.settings", &json!({ "from": other(8).to_str().unwrap(), "endTime": 100 })).unwrap();
    assert!(out.message.contains("its numerics are left out"), "{}", out.message);
    assert!(out.data["settings"].get("numerics").is_none(), "{}", out.data);
    let numerics = json!({ "fluid": { "SIMPLE": { "nNonOrthogonalCorrectors": 2 }, "relaxationFactors": { "equations": { "(h|e)": 0.4 } } } });
    let out = s.execute("workflow.settings", &json!({ "numerics": numerics })).unwrap();
    assert!(out.message.starts_with("settings chosen: numerics"), "{}", out.message);
    assert_eq!(
        out.data["settings"]["numerics"]["fluid"]["relaxationFactors"]["equations"]["(h|e)"],
        json!("0.4"),
        "{}",
        out.data
    );
    let sol = s.case().unwrap().dict("system/air/fvSolution").unwrap();
    assert_eq!(sol.value(&["SIMPLE", "nNonOrthogonalCorrectors"]).as_deref(), Some("2"));
    assert_eq!(sol.value(&["relaxationFactors", "equations", "(h|e)"]).as_deref(), Some("0.4"));
    let show = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(
        show.data["description"]["settings"]["numerics"]["fluid"]["SIMPLE"]["nNonOrthogonalCorrectors"],
        json!("2")
    );
    for (params, want) in [
        (json!({ "numerics": { "fluid": { "SIMPLE": { "a b": 1 } } } }), "is not a field's name or pattern"),
        (json!({ "numerics": { "gas": { "SIMPLE": { "x": 1 } } } }), "numerics/gas is not a region kind"),
        (json!({ "numerics": { "fluid": { "fvSchemes": { "x": 1 } } } }), "numerics/fluid/fvSchemes is not one of"),
        (json!({ "numerics": { "fluid": { "SIMPLE": { "x": "$y" } } } }), "is a number or a plain word"),
        // Unbalanced brackets: written unquoted, the file wouldn't read.
        (
            json!({ "numerics": { "fluid": { "relaxationFactors": { "equations": { "a)": 0.3 } } } } }),
            "equations/a) is not a field's name or pattern",
        ),
    ] {
        let e = s.execute("workflow.settings", &params).unwrap_err().to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    for v in [8, 12] {
        let _ = std::fs::remove_dir_all(other(v));
    }
}

#[test]
fn workflow_component_switches_a_component_in_the_files_and_the_description() {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-component", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute(
        "case.new",
        &json!({ "path": dir.to_str().unwrap(), "version": 12, "regions": [{"name": "air", "kind": "fluid"}], "radiation": "air" }),
    )
    .unwrap();
    let out = s.execute("workflow.component", &json!({ "component": "temperatureLimit", "on": true })).unwrap();
    assert!(
        out.message.starts_with("temperatureLimit switched on: written into system/air/fvConstraints"),
        "{}",
        out.message
    );
    assert_eq!(out.data["described"], json!(true));
    let show = s.execute("workflow.show", &Value::Null).unwrap();
    assert_eq!(show.data["description"]["components"], json!(["temperatureLimit"]), "{}", show.data);
    s.execute("edit.undo", &Value::Null).unwrap();
    let fvc = s.case().unwrap().dict("system/air/fvConstraints").unwrap().to_string();
    assert!(!fvc.contains("limitTemperature"), "{fvc}");
    let out =
        s.execute("workflow.component", &json!({ "component": "grass", "on": true, "patches": ["ground"] })).unwrap();
    assert!(out.message.contains("constant/air/grassProperties"), "{}", out.message);
    let out = s.execute("workflow.component", &json!({ "component": "grass", "on": false })).unwrap();
    assert!(out.message.contains("grassModel none"), "{}", out.message);
    for (params, want) in [
        (json!({ "component": "grass" }), "on is true or false"),
        (json!({ "component": "grass", "on": true, "patches": "ground" }), "patches is a list of names"),
        (json!({ "component": "trees", "on": true }), "trees is not a component"),
        (json!({ "component": "blending", "on": false, "patches": ["a"] }), "patches are the grass's"),
    ] {
        let e = s.execute("workflow.component", &params).unwrap_err().to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn material_set_changes_one_material_s_coefficients() {
    let dir = std::env::temp_dir().join(format!("umcf-engine-{}-material", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute(
        "case.new",
        &json!({
            "path": dir.to_str().unwrap(), "version": 12,
            "regions": [{"name": "air", "kind": "fluid"}, {"name": "street", "kind": "solid", "materials": [{"name": "brick", "model": "Hamstad5Brick"}]}],
        }),
    )
    .unwrap();
    let out = s
        .execute(
            "material.set",
            &json!({ "region": "street", "material": "brick", "coefficients": { "rho": 1700, "wcap": 2 } }),
        )
        .unwrap();
    assert!(
        out.message.starts_with(
            "material brick of region street: coefficients changed in constant/street/transportProperties; wcap: not a coefficient the solver's catalog lists for Hamstad5Brick"
        ),
        "{}",
        out.message
    );
    assert_eq!(out.data["model"], json!("Hamstad5Brick"));
    let tp = s.case().unwrap().dict("constant/street/transportProperties").unwrap().to_string();
    assert!(tp.contains("1700") && tp.contains("wcap"), "{tp}");
    s.execute("edit.undo", &Value::Null).unwrap();
    let tp = s.case().unwrap().dict("constant/street/transportProperties").unwrap().to_string();
    assert!(!tp.contains("1700"), "{tp}");
    for (params, want) in [
        (json!({ "region": "street", "material": "brick" }), "coefficients is an object"),
        (
            json!({ "region": "street", "material": "brick", "coefficients": { "rho": "x" } }),
            "rho is a number, or null",
        ),
        (
            json!({ "region": "street", "material": "brick", "coefficients": { "V": { "n": 2 } } }),
            "V is a number, or null",
        ),
        (
            json!({ "region": "street", "material": "brick", "coefficients": { "cap": null } }),
            "cap: the solver reads it",
        ),
        (json!({ "region": "street", "material": "glass", "coefficients": { "rho": 1 } }), "lists no material glass"),
    ] {
        let e = s.execute("material.set", &params).unwrap_err().to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
