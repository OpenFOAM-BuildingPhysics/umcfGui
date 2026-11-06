//! `case.*`: open, save, describe and check the case.

use serde_json::{Value, json};
use umcf_core::DirFs;
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
    params: "path: the case directory (required). fields: false to skip reading field files (faster; the boundary conditions are then not checked). discard: true to open even though the open case has unsaved changes.",
    enabled: always,
    run: open,
    // Not journaled: a journal runs again on whichever case is open.
    journal: false,
    undoable: false,
};

fn open(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(OPEN.id, p, &["path", "fields", "discard"])?;
    let path = p.required_str("path")?;
    let fields = p.bool("fields")?.unwrap_or(true);
    let discard = p.bool("discard")?.unwrap_or(false);
    if !std::path::Path::new(path).is_dir() {
        return Err(Error::Failed(format!("{path} is not a directory")));
    }
    let options = if fields { OpenOptions::default() } else { OpenOptions { field_bytes: 0, ..Default::default() } };
    s.open(Box::new(DirFs::new(path)), path, options, discard)?;
    let case = s.case().ok_or_else(|| Error::Failed("the case did not open".to_string()))?;
    let version = case.layout.version.map_or("unknown OpenFOAM version", |v| v.label());
    let message = format!("opened {path}: {version}, {} regions", case.regions.len());
    Ok(Outcome::new(message, info_data(s)))
}

pub(super) const SAVE: CommandSpec = CommandSpec {
    id: "case.save",
    label: "Save",
    menu: "Case/Save",
    shortcut: Some("Ctrl+S"),
    params: "none. Writes the changed files (dictionaries, fields, surfaces, face sets, the workflow description), after a backup of each under the session's backup directory. A file a command wrote whole is not written through a symbolic link.",
    enabled: needs_case,
    run: save,
    journal: true,
    undoable: false,
};

fn save(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(SAVE.id, p, &[])?;
    let backup = s.backup.clone();
    let o = s.opened_mut()?;
    let saved = o.case.save(o.fs.as_mut(), Some(&backup))?;
    let message = match saved.len() {
        0 => "nothing to save".to_string(),
        1 => "saved 1 file".to_string(),
        n => format!("saved {n} files"),
    };
    Ok(Outcome::new(message, json!({ "saved": saved, "backup": backup })))
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
