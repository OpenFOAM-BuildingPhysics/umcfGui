//! `case.*`: make, open, save, describe and check the case.

use serde_json::{Value, json};
use umcf_core::{Cancellable, CaseWrite, DirFs, OfVersion};
use umcf_model::description::{Components, MaterialDesc};
use umcf_model::new_case::{self, NewCase, NewRegion, Up};
use umcf_model::{Case, OpenOptions, RegionKind};
use umcf_schema::{CaseKind, Severity};
use umcf_validate::{Diagnostic, Level};

use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

fn always(_: &Session) -> Result<(), String> {
    Ok(())
}

pub(super) const OPEN: CommandSpec = CommandSpec {
    id: "case.open",
    label: "Open case…",
    menu: "Case/Open…",
    shortcut: Some("Ctrl+O"),
    params: "path: the case directory (required). fields: false to skip reading field files (faster; the boundary conditions are then not checked), \"later\" to list them only and read them with case.fields (the desktop app's way: the case opens first). discard: true to open even though the open case has unsaved changes.",
    enabled: always,
    run: open,
    // Not journaled: a journal runs again on whichever case is open.
    journal: false,
    undoable: false,
    reads_only: true,
};

fn open(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(OPEN.id, p, &["path", "fields", "discard"])?;
    let path = p.required_str("path")?;
    let (fields, later) = match p.value("fields") {
        Some(Value::String(s)) if s == "later" => (true, true),
        _ => (
            p.bool("fields")
                .map_err(|_| Error::Params("case.open: `fields` is true, false or \"later\"".into()))?
                .unwrap_or(true),
            false,
        ),
    };
    let discard = p.bool("discard")?.unwrap_or(false);
    if !std::path::Path::new(path).is_dir() {
        return Err(Error::Failed(format!("{path} is not a directory")));
    }
    let options = match (fields, later) {
        (_, true) => OpenOptions { defer_fields: true, ..Default::default() },
        (true, false) => OpenOptions::default(),
        (false, false) => OpenOptions { field_bytes: 0, ..Default::default() },
    };
    // Its reads stop when the command running is cancelled (`Session::canceller`).
    s.open(Box::new(Cancellable::new(DirFs::new(path), s.canceller())), path, options, discard)?;
    let case = s.case().ok_or_else(|| Error::Failed("the case did not open".to_string()))?;
    let version = case.layout.version.map_or("unknown OpenFOAM version", |v| v.label());
    let message = format!("opened {path}: {version}, {} regions", case.regions.len());
    Ok(Outcome::new(message, info_data(s)))
}

pub(super) const NEW: CommandSpec = CommandSpec {
    id: "case.new",
    label: "New case…",
    menu: "Case/New…",
    shortcut: None,
    params: "path: a directory that isn't there or is empty (required). version: 8 or 12 (required). regions: [{\"name\": \"air\", \"kind\": \"fluid\"}, {\"name\": \"street\", \"kind\": \"solid\", \"materials\": [{\"name\": \"soil\", \"model\": \"Soil\"}]}, {\"name\": \"vegetation\", \"kind\": \"vegetation\"}, …] (required; at least one fluid; plain names that don't differ only by case and aren't those of the files in constant/ or system/; the vegetation region is named vegetation, as the solver looks it up, and needs the radiation on it). The components: radiation: the region it is computed on (a fluid or the vegetation; it needs a fluid named air); grass: true, or the patches the grass grows on (it needs the radiation); temperatureLimit: true; blending: true (the blending layer: it needs z up, and the air's patches west, east, north and south once meshed). up: the axis that points up, x, y or z (default z): gravity points down it. discard: true to replace an open case with unsaved changes. settings: the settings chosen, as workflow.settings takes them but for from ({\"deltaT\": 1800, \"turbulence\": \"realizableKE\", \"temperatureLimit\": [283.15, 370.15], …}); the others are the configuration's. The settings are one configuration, the urbanMicroclimateFoam tutorials'. Writes system/umcfWorkflow (the description, with the components), the region list (OpenFOAM-12's controlDict, OpenFOAM-8's constant/regionProperties), controlDict's run controls (the tutorials', without their function objects) and each region's settings by its kind and the components' (docs/user-guide/new-case.md lists them), then opens the case. The files are written into a directory of their own inside path and moved up once all are: a failure leaves things as they were (a directory there stays, with its owner and mode; one made for the case goes again). A path with `..` is refused. A solid's material gets the coefficients of the tutorials' first material of its model (Soil, Hamstad5Brick); one of another model gets none (a note says so). Not written: meshes, the fields of 0/, decomposeParDict, the weather and sun tables, the view factor dictionary.",
    enabled: always,
    run: new_case,
    // Not journaled: a journal runs again on whichever case is open.
    journal: false,
    undoable: false,
    reads_only: false,
};

/// `case.new`: makes a case in a new directory, then opens it.
fn new_case(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(
        NEW.id,
        p,
        &[
            "path",
            "version",
            "regions",
            "radiation",
            "grass",
            "temperatureLimit",
            "blending",
            "up",
            "discard",
            "settings",
        ],
    )?;
    let bad = |m: &str| Error::Params(format!("{}: {m}", NEW.id));
    let path = p.required_str("path")?;
    if path.trim().is_empty() {
        return Err(bad("path is the directory to make the case in"));
    }
    let version = match p.required("version")? {
        Value::Number(n) if n.as_u64() == Some(8) => OfVersion::V8,
        Value::Number(n) if n.as_u64() == Some(12) => OfVersion::V12,
        Value::String(v) if v == "8" => OfVersion::V8,
        Value::String(v) if v == "12" => OfVersion::V12,
        _ => return Err(bad("version is 8 or 12")),
    };
    let Some(Value::Array(list)) = p.value("regions") else {
        return Err(bad("regions is a list: [{\"name\": \"air\", \"kind\": \"fluid\"}, …]"));
    };
    let mut regions = Vec::new();
    for r in list {
        let r = Params::new(NEW.id, r, &["name", "kind", "materials"])?;
        let name = r.required_str("name")?;
        let kind = match r.required_str("kind")? {
            "fluid" => RegionKind::Fluid,
            "solid" => RegionKind::Solid,
            "vegetation" => RegionKind::Vegetation,
            _ => return Err(bad(&format!("region {name}: kind is fluid, solid or vegetation"))),
        };
        let mut materials = Vec::new();
        match r.value("materials") {
            None => {}
            Some(Value::Array(ms)) => {
                for m in ms {
                    let m = Params::new(NEW.id, m, &["name", "model"])?;
                    materials.push(MaterialDesc {
                        name: m.required_str("name")?.to_string(),
                        model: Some(m.required_str("model")?.to_string()),
                    });
                }
            }
            Some(_) => return Err(bad(&format!("region {name}: materials is a list of {{\"name\", \"model\"}}"))),
        }
        regions.push(NewRegion { name: name.to_string(), kind, materials });
    }
    let (grass, grass_patches) = match p.value("grass") {
        None | Some(Value::Bool(false)) => (false, Vec::new()),
        Some(Value::Bool(true)) => (true, Vec::new()),
        Some(Value::Array(ps)) => {
            let names: Option<Vec<String>> = ps.iter().map(|v| v.as_str().map(str::to_string)).collect();
            (true, names.ok_or_else(|| bad("grass is true, or a list of the patches' names"))?)
        }
        Some(_) => return Err(bad("grass is true, or a list of the patches' names")),
    };
    let up = match p.str("up")? {
        None => Up::Z,
        Some(a) => Up::parse(a).ok_or_else(|| bad("up is x, y or z"))?,
    };
    let spec = NewCase {
        version,
        regions,
        radiation: p.str("radiation")?.map(str::to_string),
        components: Components {
            grass,
            temperature_limit: p.bool("temperatureLimit")?.unwrap_or(false),
            blending: p.bool("blending")?.unwrap_or(false),
        },
        grass_patches,
        up,
        settings: match p.value("settings") {
            None => Default::default(),
            Some(v) => super::workflow::settings_params(NEW.id, v)?,
        },
    };
    let discard = p.bool("discard")?.unwrap_or(false);
    // Everything is checked before anything is written.
    let made = new_case::make(&spec)?;
    if let Some(case) = s.case() {
        let changed = case.changed();
        if !changed.is_empty() && !discard {
            return Err(Error::Failed(format!(
                "the open case has unsaved changes ({}): save it first, or pass \"discard\": true",
                changed.join(", ")
            )));
        }
    }
    let (written, left) = write_new(path, &made.files)?;
    // Its unsaved changes were refused or given up above.
    if let Err(e) =
        s.open(Box::new(Cancellable::new(DirFs::new(path), s.canceller())), path, OpenOptions::default(), true)
    {
        return Err(Error::Failed(format!("made {path}, but it did not open: {e} (case.open opens it)")));
    }
    let message = format!(
        "made {path}: {}, {} regions; {} files{}",
        version.label(),
        spec.regions.len(),
        written.len(),
        if made.notes.is_empty() { String::new() } else { format!("; {}", made.notes.join("; ")) }
    );
    let notes: Vec<String> = made.notes.iter().cloned().chain(left.clone()).collect();
    let message = match &left {
        Some(l) => format!("{message}; {l}"),
        None => message,
    };
    let mut data = info_data(s);
    if let Value::Object(o) = &mut data {
        o.insert("written".into(), json!(written));
        o.insert("notes".into(), json!(notes));
    }
    Ok(Outcome::new(message, data))
}

/// Writes a new case's `files` into the directory `path`, which isn't there or is empty: into a
/// directory of their own inside it first (`.umcfgui-new-<process>`), each top-level entry moved
/// up once all are written. The directory itself stays the user's (its owner, mode and ACLs); one
/// made here, and the parents made with it, are removed again when it fails, as is everything
/// written, so that a failure leaves things as they were. The paths written.
fn write_new(path: &str, files: &[(String, String)]) -> Result<(Vec<String>, Option<String>), Error> {
    use std::path::{Component, Path, PathBuf};
    let failed = |e: &dyn std::fmt::Display| Error::Failed(format!("{path}: {e} (nothing was made)"));
    if Path::new(path).components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::Failed(format!("{path}: give the directory without `..`")));
    }
    // Without `.` components: `a/b/.` is `a/b`, whose parents are `a` and none.
    let dir: PathBuf = Path::new(path).components().filter(|c| !matches!(c, Component::CurDir)).collect();
    if dir.as_os_str().is_empty() {
        return Err(Error::Failed(format!("{path}: give the directory to make the case in")));
    }
    // The directories this call makes, outermost first: removed again on a failure (only
    // while empty, so never what someone else put in them meanwhile).
    let mut made: Vec<PathBuf> = Vec::new();
    let undo_dirs = |made: &[PathBuf]| {
        for d in made.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
    };
    if dir.exists() {
        match std::fs::read_dir(&dir).map(|mut d| d.next().is_none()) {
            Ok(true) => {}
            Ok(false) => {
                return Err(Error::Failed(format!("{path} is not empty: a new case goes in a new or empty directory")));
            }
            Err(e) => return Err(failed(&e)),
        }
    } else {
        let mut missing: Vec<&Path> =
            dir.ancestors().take_while(|a| !a.as_os_str().is_empty() && !a.exists()).collect();
        missing.reverse();
        for d in missing {
            match std::fs::create_dir(d) {
                Ok(()) => made.push(d.to_path_buf()),
                // Made by someone else meanwhile: theirs, not undone.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && d.is_dir() => {}
                Err(e) => {
                    undo_dirs(&made);
                    return Err(failed(&format!("{}: {e}", d.display())));
                }
            }
        }
    }
    // Made new, never one that is there (another case.new's, say).
    let staging = dir.join(format!(".umcfgui-new-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir(&staging) {
        undo_dirs(&made);
        return Err(failed(&e));
    }
    let mut fs = DirFs::new(&staging);
    let mut written = Vec::new();
    let mut result = Ok(());
    for (rel, text) in files {
        if let Err(e) = fs.write(rel, text.as_bytes()) {
            // The error names the file.
            result = Err(failed(&e));
            break;
        }
        written.push(rel.clone());
    }
    // The top-level entries (`system`, `constant`), moved up; undone if one can't be.
    let mut moved: Vec<PathBuf> = Vec::new();
    if result.is_ok() {
        let mut tops: Vec<String> =
            files.iter().filter_map(|(rel, _)| rel.split('/').next().map(str::to_string)).collect();
        tops.sort();
        tops.dedup();
        for top in tops {
            let to = dir.join(&top);
            // `rename` would replace a file, or an empty directory, made there meanwhile.
            let moving = if to.exists() {
                Err(format!("{top} is there now"))
            } else {
                std::fs::rename(staging.join(&top), &to).map_err(|e| format!("{top}: {e}"))
            };
            if let Err(e) = moving {
                result = Err(failed(&format!("moving {e}")));
                break;
            }
            moved.push(to);
        }
    }
    let mut left = Vec::new();
    if result.is_err() {
        for m in &moved {
            if std::fs::remove_dir_all(m).is_err() {
                left.push(m.display().to_string());
            }
        }
    }
    // Only the directory made above, with what was written into it.
    if std::fs::remove_dir_all(&staging).is_err() && staging.exists() {
        left.push(staging.display().to_string());
    }
    if result.is_err() {
        undo_dirs(&made);
    }
    match result {
        Err(Error::Failed(m)) if !left.is_empty() => Err(Error::Failed(format!(
            "{}: the case was not made, but this could not be removed, remove it by hand: {}",
            m.trim_end_matches(" (nothing was made)"),
            left.join(", ")
        ))),
        // Made: a staging directory that couldn't be removed is all that is left over.
        Ok(()) if !left.is_empty() => {
            Ok((written, Some(format!("{} could not be removed: remove it by hand", left.join(", ")))))
        }
        r => r.map(|()| (written, None)),
    }
}

pub(super) const FIELDS: CommandSpec = CommandSpec {
    id: "case.fields",
    label: "Read fields",
    menu: "Case/Read fields",
    shortcut: None,
    params: "none. Reads the start-time field files of a case opened with fields: \"later\" (their boundary conditions and sizes, as case.open would have), in parallel; nothing to do when they were read already. Cancelled, the case stays as it was: listed, not read.",
    enabled: needs_case,
    run: fields,
    journal: false,
    undoable: false,
    // It reads files only: cancelled, the case is put back as it was.
    reads_only: true,
};

fn fields(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(FIELDS.id, p, &[])?;
    let progress = s.canceller().progress().clone();
    let o = s.opened_mut()?;
    let read = o.case.read_start_fields(o.fs.as_ref(), Some(&progress))?;
    // The undo history's snapshots from before have the fields listed only: they take them, so
    // an undo doesn't make them unread again.
    if read {
        let case = o.case.clone();
        for step in s.undo.iter_mut().chain(s.redo.iter_mut()) {
            step.case.adopt_fields(&case);
        }
    }
    let case = s.case().ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let n: usize = case.regions.iter().map(|r| r.fields.len()).sum();
    let message = if read { format!("read {n} field files") } else { "the field files were read already".to_string() };
    Ok(Outcome::new(message, json!({ "read": read, "fields": n })))
}

pub(super) const SAVE: CommandSpec = CommandSpec {
    id: "case.save",
    label: "Save",
    menu: "Case/Save",
    shortcut: Some("Ctrl+S"),
    params: "none. Writes the changed files (dictionaries, fields, surfaces, face sets, the workflow description), after a backup of each under the session's backup directory. A file a command wrote whole is refused when it is a symbolic link: checked once, before any file is written, so a link made while the save runs is not caught. Then the patch renames patch.set was asked to follow into the results (results: true; undone ones back): the other time directories' and processor directories' fields and meshes, one file at a time, and boundaryData directories; not backed up; what was skipped is named (results).",
    enabled: needs_case,
    run: save,
    journal: true,
    undoable: false,
    reads_only: false,
};

fn save(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(SAVE.id, p, &[])?;
    let backup = s.backup.clone();
    // A result file is edited whole in memory: read (compressed), decompressed, written and
    // compressed again, about four times its size at once: a quarter of what may be read.
    let cap = s.budget().read / 4;
    let o = s.opened_mut()?;
    let saved = o.case.save(o.fs.as_mut(), Some(&backup))?;
    // The patch renames followed into the results, after the case's own files.
    // Stopped there (cancelled): the case's own files are saved, and some results: listed.
    let results = match o.case.save_results(o.fs.as_mut(), cap) {
        Ok(r) => r,
        Err(umcf_model::Error::Partial { saved: done, source }) => {
            return Err(Error::Model(umcf_model::Error::Partial {
                saved: saved.into_iter().chain(done).collect(),
                source,
            }));
        }
        Err(e) if !saved.is_empty() => {
            return Err(Error::Model(umcf_model::Error::Partial { saved, source: Box::new(e) }));
        }
        Err(e) => return Err(e.into()),
    };
    let mut message = match saved.len() {
        0 if results.renames.is_empty() => "nothing to save".to_string(),
        0 => "saved no file".to_string(),
        1 => "saved 1 file".to_string(),
        n => format!("saved {n} files"),
    };
    if !results.renames.is_empty() {
        let n = results.files.len();
        message += &format!(
            "; the patch names followed into the results: {n} file{} rewritten{}",
            if n == 1 { "" } else { "s" },
            results.dirs.iter().map(|d| format!(", {d}")).collect::<String>()
        );
        // The first few: a decomposed case can skip thousands (all in results.skipped).
        const SHOWN: usize = 10;
        if !results.skipped.is_empty() {
            message += &format!(
                "; not changed: {}",
                results
                    .skipped
                    .iter()
                    .take(SHOWN)
                    .map(|(f, why)| format!("{f} ({why})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if results.skipped.len() > SHOWN {
                message += &format!(" and {} more (results.skipped)", results.skipped.len() - SHOWN);
            }
        }
    }
    let renames: Vec<Value> =
        results.renames.iter().map(|r| json!({ "region": r.region, "from": r.from, "to": r.to })).collect();
    Ok(Outcome::new(
        message,
        json!({
            "saved": saved,
            "backup": backup,
            "results": {
                "renames": renames,
                "files": results.files,
                "dirs": results.dirs,
                "skipped": results.skipped.iter().map(|(f, why)| json!({ "path": f, "why": why })).collect::<Vec<_>>(),
            },
        }),
    ))
}

pub(super) const INFO: CommandSpec = CommandSpec {
    id: "case.info",
    label: "Case summary",
    menu: "Case/Summary",
    shortcut: None,
    params: "none. Returns the location, OpenFOAM version, kind, regions, decomposition, unsaved files, what didn't read, and the undo depth.",
    enabled: needs_case,
    run: info,
    journal: false,
    undoable: false,
    reads_only: true,
};

fn info(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(INFO.id, p, &[])?;
    let data = info_data(s);
    let changed = data.get("changed").and_then(Value::as_array).map_or(0, Vec::len);
    let message = format!("{} ({changed} unsaved)", s.location().unwrap_or(""));
    Ok(Outcome::new(message, data))
}

fn region_kind(k: RegionKind) -> &'static str {
    match k {
        RegionKind::Fluid => "fluid",
        RegionKind::Solid => "solid",
        RegionKind::Vegetation => "vegetation",
        RegionKind::Single => "single",
    }
}

fn info_data(s: &Session) -> Value {
    let Some(case) = s.case() else { return Value::Null };
    json!({
        "path": s.location(),
        "version": case.layout.version.map(|v| v.label()),
        "kind": match case.layout.kind { CaseKind::Solver => "solver", CaseKind::Meshing => "meshing" },
        "regions": case.regions.iter().map(|r| json!({
            "name": r.name,
            "kind": region_kind(r.kind),
            "patches": r.patches.as_ref().map(Vec::len),
            "fields": r.fields.len(),
        })).collect::<Vec<_>>(),
        "processors": case.processors,
        "changed": case.changed(),
        "notes": case.notes.iter().map(|n| json!({
            "severity": match n.severity { Severity::Info => "info", Severity::Warning => "warning" },
            "path": n.path,
            "message": n.message,
        })).collect::<Vec<_>>(),
        "undo": s.undo.len(),
        "redo": s.redo.len(),
    })
}

pub(super) const CHECK: CommandSpec = CommandSpec {
    id: "case.check",
    label: "Check case",
    menu: "Case/Check",
    shortcut: Some("F7"),
    params: "none. Returns the diagnostics, errors first: level, code, path, region, patch, field, message.",
    enabled: needs_case,
    run: check,
    journal: false,
    undoable: false,
    reads_only: true,
};

pub(crate) fn level(l: Level) -> &'static str {
    match l {
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Info => "info",
    }
}

pub(crate) fn diagnostic(d: &Diagnostic) -> Value {
    json!({
        "level": level(d.level),
        "code": d.code,
        "path": d.path,
        "region": d.region,
        "patch": d.patch,
        "field": d.field,
        "message": d.message,
    })
}

pub(crate) fn count(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

fn check(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(CHECK.id, p, &[])?;
    let case: &Case = s.case().ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let all = umcf_validate::check(case);
    let n = |l: Level| all.iter().filter(|d| d.level == l).count();
    let (errors, warnings, infos) = (n(Level::Error), n(Level::Warning), n(Level::Info));
    let message = format!("{}, {}, {}", count(errors, "error"), count(warnings, "warning"), count(infos, "note"));
    let data = json!({
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "diagnostics": all.iter().map(diagnostic).collect::<Vec<_>>(),
    });
    Ok(Outcome::new(message, data))
}
