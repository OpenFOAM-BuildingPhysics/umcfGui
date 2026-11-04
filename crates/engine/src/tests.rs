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
    // air: only its boundary file (unreadable); wall: a whole cube.
    cube_mesh(&mut fs, "constant/wall/polyMesh");
    fs.insert(
        "0/wall/Ts",
        "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class volScalarField;\n}\ninternalField uniform 300;\nboundaryField\n{\n    inlet { type zeroGradient; }\n}\n",
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let out = s.execute("mesh.check", &Value::Null).unwrap();
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
    assert_eq!(s.journal().iter().filter(|j| j["command"] == "select.pick").count(), 6);
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
