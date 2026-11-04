//! The editing session: everything the GUI, the CLI and MCP do to a case goes through here.
//!
//! - A **command** ([`CommandSpec`]) has an id (`case.open`, `control.set`, …), a label and menu
//!   path for the UI, a description of its parameters, an `enabled` test that says why it can't
//!   run, and a `run` function. [`COMMANDS`] is the registry: the UI builds its menus and
//!   palette from it, and the CLI and MCP dispatch by id.
//! - Parameters are JSON. A command called with no parameters (`null` or `{}`) uses its
//!   defaults or returns [`Error::Params`]; nothing here opens a dialog. Unknown parameter names
//!   are an error, so a typo is not silently ignored.
//! - [`Session::execute`] runs a command **all or nothing**: when it fails, or panics, the case
//!   model and the undo history are put back as they were. A panic is caught
//!   ([`Error::Panicked`]); it is a safety net for bugs, not a way of reporting errors.
//! - **Undo**: before an undoable command runs, the case model is snapshotted (a cheap clone:
//!   documents are shared until edited). `edit.undo` and `edit.redo` step through them; up to
//!   [`UNDO_DEPTH`] steps are kept. Opening a case starts a new history.
//! - **Journal**: every command that changes the case is recorded as a JSON line
//!   `{"command": "...", "params": {...}}`, in the order it ran ([`Session::journal`]). Opening
//!   a case is not recorded, so the lines can be run again on whichever case is open: another
//!   copy of the same case gets the same edits.
//!
//! Saving writes only the files that changed (see `umcf_model::Case::save`) and keeps a backup
//! of each under `.umcfgui/backup/<session>/` the first time it is saved in the session.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};
use umcf_core::CaseWrite;
use umcf_model::{Case, OpenOptions};

mod cmd;
mod params;

pub use cmd::COMMANDS;

/// The most undo steps kept; the oldest go first.
pub const UNDO_DEPTH: usize = 100;

/// One command of the registry.
pub struct CommandSpec {
    /// `family.verb`, stable: scripts, journals and MCP tools use it.
    pub id: &'static str,
    pub label: &'static str,
    /// Where the UI puts it (`Case/Open…`); empty when it is in no menu.
    pub menu: &'static str,
    pub shortcut: Option<&'static str>,
    /// The parameters, for help texts and MCP tool descriptions.
    pub params: &'static str,
    /// `Err(reason)` when the command can't run now; the UI shows the reason.
    pub enabled: fn(&Session) -> Result<(), String>,
    pub run: fn(&mut Session, &Value) -> Result<Outcome, Error>,
    /// Recorded in the journal when it succeeds (it changes the case or the session).
    pub journal: bool,
    /// Snapshotted for undo before it runs.
    pub undoable: bool,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec").field("id", &self.id).finish_non_exhaustive()
    }
}

/// What a command did: a line for people, and data for programs.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub message: String,
    pub data: Value,
}

impl Outcome {
    pub fn new(message: impl Into<String>, data: Value) -> Self {
        Outcome { message: message.into(), data }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there is no command `{0}`")]
    Unknown(String),
    #[error("{id} can't run: {reason}")]
    Disabled { id: String, reason: String },
    #[error("{0}")]
    Params(String),
    #[error(transparent)]
    Model(#[from] umcf_model::Error),
    #[error("{0}")]
    Failed(String),
    #[error("{id} stopped on an internal error ({message}); the case is as it was before it ran")]
    Panicked { id: String, message: String },
}

/// The opened case and where it is.
struct Opened {
    case: Case,
    fs: Box<dyn CaseWrite>,
    location: String,
}

/// One editing session: at most one open case, its undo history and the journal.
pub struct Session {
    opened: Option<Opened>,
    undo: Vec<Case>,
    redo: Vec<Case>,
    journal: Vec<Value>,
    backup: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("location", &self.location())
            .field("undo", &self.undo.len())
            .field("redo", &self.redo.len())
            .field("journal", &self.journal.len())
            .finish_non_exhaustive()
    }
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

/// Everything a failed or panicking command could have changed.
struct Saved {
    case: Option<Case>,
    location: Option<String>,
    undo: Vec<Case>,
    redo: Vec<Case>,
}

impl Session {
    /// A session whose backups go to `.umcfgui/backup/<seconds since 1970>/`.
    pub fn new() -> Self {
        // The engine is native only (L5); the clock just names the backup directory.
        #[allow(clippy::disallowed_methods)]
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Session::with_backup(format!(".umcfgui/backup/{now}"))
    }

    /// A session with its own backup directory (under `.umcfgui/`).
    pub fn with_backup(backup: impl Into<String>) -> Self {
        Session { opened: None, undo: Vec::new(), redo: Vec::new(), journal: Vec::new(), backup: backup.into() }
    }

    pub fn case(&self) -> Option<&Case> {
        self.opened.as_ref().map(|o| &o.case)
    }

    /// Where the open case is (the directory given to `case.open`).
    pub fn location(&self) -> Option<&str> {
        self.opened.as_ref().map(|o| o.location.as_str())
    }

    /// The open case's files.
    pub fn fs(&self) -> Option<&dyn CaseWrite> {
        self.opened.as_ref().map(|o| o.fs.as_ref())
    }

    pub fn backup_dir(&self) -> &str {
        &self.backup
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The commands that changed something, in order, as `{"command", "params"}` objects.
    pub fn journal(&self) -> &[Value] {
        &self.journal
    }

    /// The registry entry for `id`.
    pub fn command(id: &str) -> Option<&'static CommandSpec> {
        COMMANDS.iter().find(|c| c.id == id)
    }

    /// Whether `id` can run now, and if not, why.
    pub fn enabled(&self, id: &str) -> Result<(), String> {
        let spec = Session::command(id).ok_or_else(|| format!("there is no command `{id}`"))?;
        (spec.enabled)(self)
    }

    /// Runs command `id` with `params` (see the crate documentation).
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Outcome, Error> {
        let spec = Session::command(id).ok_or_else(|| Error::Unknown(id.to_string()))?;
        self.run_spec(spec, params)
    }

    /// Opens the case in `fs` (what `case.open` does once it has the directory), replacing the
    /// open one and its history. A tool that has its own file access (an in-memory case, a
    /// sample) opens it here. Unsaved changes in the open case are an error unless `discard`.
    pub fn open(
        &mut self,
        fs: Box<dyn CaseWrite>,
        location: impl Into<String>,
        options: OpenOptions,
        discard: bool,
    ) -> Result<(), Error> {
        if let Some(case) = self.case() {
            let changed = case.changed();
            if !changed.is_empty() && !discard {
                return Err(Error::Failed(format!(
                    "the open case has unsaved changes ({}): save it first, or pass \"discard\": true",
                    changed.join(", ")
                )));
            }
        }
        let case = Case::open_with(fs.as_ref(), options)?;
        self.opened = Some(Opened { case, fs, location: location.into() });
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    fn saved(&self) -> Saved {
        Saved {
            case: self.case().cloned(),
            location: self.location().map(str::to_string),
            undo: self.undo.clone(),
            redo: self.redo.clone(),
        }
    }

    /// Puts back what [`Session::saved`] kept. A case opened by the failed command (another
    /// location) is closed again only if there was none before; otherwise the old model stays
    /// with the files it was read from.
    fn restore(&mut self, saved: Saved) {
        match (saved.case, &mut self.opened) {
            (Some(case), Some(o)) if Some(o.location.as_str()) == saved.location.as_deref() => o.case = case,
            // The command replaced the case (and lost the old files) or closed it: keep what is
            // there now rather than pair a model, or its history, with the wrong files.
            (Some(_), _) => {
                self.undo.clear();
                self.redo.clear();
                return;
            }
            (None, _) => self.opened = None,
        }
        self.undo = saved.undo;
        self.redo = saved.redo;
    }

    fn run_spec(&mut self, spec: &CommandSpec, params: &Value) -> Result<Outcome, Error> {
        (spec.enabled)(self).map_err(|reason| Error::Disabled { id: spec.id.to_string(), reason })?;
        let saved = self.saved();
        let before = if spec.undoable { self.case().cloned() } else { None };
        let result = catch_unwind(AssertUnwindSafe(|| (spec.run)(self, params)));
        match result {
            Ok(Ok(outcome)) => {
                if let Some(case) = before {
                    self.undo.push(case);
                    if self.undo.len() > UNDO_DEPTH {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                if spec.journal {
                    let params = if params.is_null() { json!({}) } else { params.clone() };
                    self.journal.push(json!({ "command": spec.id, "params": params }));
                }
                Ok(outcome)
            }
            Ok(Err(e)) => {
                self.restore(saved);
                Err(e)
            }
            Err(payload) => {
                self.restore(saved);
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "no message".to_string());
                Err(Error::Panicked { id: spec.id.to_string(), message })
            }
        }
    }

    fn opened_mut(&mut self) -> Result<&mut Opened, Error> {
        self.opened.as_mut().ok_or_else(|| Error::Failed("no case is open".to_string()))
    }
}

/// `enabled` for commands that need an open case.
fn needs_case(s: &Session) -> Result<(), String> {
    if s.opened.is_some() { Ok(()) } else { Err("no case is open".to_string()) }
}

#[cfg(test)]
mod tests;
