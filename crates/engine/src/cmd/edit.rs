//! Editing dictionaries (`control.set`, `dict.set`, `dict.remove`) and the history
//! (`edit.undo`, `edit.redo`).

use serde_json::{Value, json};
use umcf_model::{CONTROL_DICT, Case};

use crate::params::{Params, foam_text, key};
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

pub(super) const CONTROL_SET: CommandSpec = CommandSpec {
    id: "control.set",
    label: "Set run controls",
    menu: "Edit/Run controls…",
    shortcut: None,
    params: "the system/controlDict entries to set, by name: {\"endTime\": 3600, \"writeInterval\": 600}. A number or true/false as is, a string as OpenFOAM text, an array as a list.",
    enabled: needs_case,
    run: control_set,
    journal: true,
    undoable: true,
};

fn control_set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::any(CONTROL_SET.id, p)?;
    if p.is_empty() {
        return Err(Error::Params("control.set: give the entries to set, e.g. {\"endTime\": 3600}".to_string()));
    }
    let o = s.opened_mut()?;
    let mut set = Vec::new();
    for (k, v) in p.entries() {
        let k = key(CONTROL_SET.id, k)?;
        let text = foam_text(CONTROL_SET.id, v)?;
        o.case.set(CONTROL_DICT, &[k], &text)?;
        set.push(format!("{k} {}", stored(&o.case, CONTROL_DICT, &[k])));
    }
    Ok(Outcome::new(format!("{CONTROL_DICT}: {}", set.join("; ")), json!({ "file": CONTROL_DICT })))
}

/// The entry as it now reads (comments in the given text are not part of it).
fn stored(case: &Case, file: &str, path: &[&str]) -> String {
    case.dict(file).and_then(|d| d.value(path)).unwrap_or_else(|| "{ … }".to_string())
}

pub(super) const DICT_SET: CommandSpec = CommandSpec {
    id: "dict.set",
    label: "Set dictionary entry",
    menu: "",
    shortcut: None,
    params: "file: a loaded dictionary (system/air/fvSolution). path: \"PIMPLE/nOuterCorrectors\" or [\"PIMPLE\", \"nOuterCorrectors\"]; missing parent dictionaries are added. value: a number, true/false, OpenFOAM text, or an array (a list). Only that entry changes.",
    enabled: needs_case,
    run: dict_set,
    journal: true,
    undoable: true,
};

fn dict_set(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(DICT_SET.id, p, &["file", "path", "value"])?;
    let file = p.required_str("file")?;
    let path = p.path("path")?;
    let text = foam_text(DICT_SET.id, p.required("value")?)?;
    let case = &mut s.opened_mut()?.case;
    case.set(file, &path, &text)?;
    let message = format!("{file}: {} {}", path.join("/"), stored(case, file, &path));
    Ok(Outcome::new(message, json!({ "file": file })))
}

pub(super) const DICT_REMOVE: CommandSpec = CommandSpec {
    id: "dict.remove",
    label: "Remove dictionary entry",
    menu: "",
    shortcut: None,
    params: "file: a loaded dictionary. path: the entry, as for dict.set. Removing an entry that isn't there changes nothing.",
    enabled: needs_case,
    run: dict_remove,
    journal: true,
    undoable: true,
};

fn dict_remove(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(DICT_REMOVE.id, p, &["file", "path"])?;
    let file = p.required_str("file")?;
    let path = p.path("path")?;
    let removed = s.opened_mut()?.case.remove(file, &path)?;
    let message =
        if removed { format!("{file}: removed {}", path.join("/")) } else { format!("{file}: no {}", path.join("/")) };
    Ok(Outcome::new(message, json!({ "file": file, "removed": removed })))
}

pub(super) const UNDO: CommandSpec = CommandSpec {
    id: "edit.undo",
    label: "Undo",
    menu: "Edit/Undo",
    shortcut: Some("Ctrl+Z"),
    params: "none.",
    enabled: can_undo,
    run: undo,
    journal: true,
    undoable: false,
};

fn can_undo(s: &Session) -> Result<(), String> {
    needs_case(s)?;
    if s.can_undo() { Ok(()) } else { Err("nothing to undo".to_string()) }
}

fn undo(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(UNDO.id, p, &[])?;
    let previous = s.undo.pop().ok_or_else(|| Error::Failed("nothing to undo".to_string()))?;
    let o = s.opened_mut()?;
    let current = std::mem::replace(&mut o.case, previous);
    s.redo.push(current);
    Ok(Outcome::new("undone", json!({ "undo": s.undo.len(), "redo": s.redo.len() })))
}

pub(super) const REDO: CommandSpec = CommandSpec {
    id: "edit.redo",
    label: "Redo",
    menu: "Edit/Redo",
    shortcut: Some("Ctrl+Shift+Z"),
    params: "none.",
    enabled: can_redo,
    run: redo,
    journal: true,
    undoable: false,
};

fn can_redo(s: &Session) -> Result<(), String> {
    needs_case(s)?;
    if s.can_redo() { Ok(()) } else { Err("nothing to redo".to_string()) }
}

fn redo(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(REDO.id, p, &[])?;
    let next = s.redo.pop().ok_or_else(|| Error::Failed("nothing to redo".to_string()))?;
    let o = s.opened_mut()?;
    let current = std::mem::replace(&mut o.case, next);
    s.undo.push(current);
    Ok(Outcome::new("redone", json!({ "undo": s.undo.len(), "redo": s.redo.len() })))
}
