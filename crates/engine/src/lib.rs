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
//!   a case is not recorded and starts a new journal, so the journal holds the edits of one
//!   case and runs again on whichever case is open: another copy gets the same edits.
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
use umcf_core::{CaseFs, CaseWrite, FsError};
use umcf_model::{Case, OpenOptions};

mod budget;
mod cmd;
mod params;

pub use budget::Budget;
pub use cmd::COMMANDS;
pub use cmd::display::{DisplayEntry, Layer, parse as parse_display, style as display_style};
pub use cmd::scene::{Sel, camera_json, selected_faces};
pub use cmd::workflow::settings_json;
pub use umcf_core::Cancel;

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
    /// It only reads the case: nothing is written, nothing in the session changes but what it
    /// shows. Such a command can be cancelled while it reads ([`Session::canceller`]); one that
    /// writes runs to its end.
    pub reads_only: bool,
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
    #[error("{id} was cancelled; the case is as it was before it ran")]
    Cancelled { id: String },
}

/// The opened case and where it is.
struct Opened {
    case: Case,
    fs: Box<dyn CaseWrite + Send + Sync>,
    location: String,
}

/// One edit in the undo history: the case as it was on one side of it, and what it was.
#[derive(Clone)]
pub(crate) struct Step {
    pub(crate) case: Case,
    pub(crate) edit: Edit,
}

/// An edit in the undo history, for people: the command and what it acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub command: &'static str,
    pub label: &'static str,
    /// The parameters, short (`endTime 7200`, `walls.stl`).
    pub detail: String,
}

fn detail(params: &Value) -> String {
    let Some(map) = params.as_object() else { return String::new() };
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in map {
        let text = match v {
            Value::String(s) => s.rsplit('/').next().unwrap_or(s).to_string(),
            Value::Number(_) | Value::Bool(_) => format!("{k} {v}"),
            _ => continue,
        };
        parts.push(if matches!(v, Value::String(_)) && !["file", "path", "name"].contains(&k.as_str()) {
            format!("{k} {text}")
        } else {
            text
        });
    }
    let mut d = parts.join(", ");
    if d.chars().count() > 48 {
        d = d.chars().take(47).collect::<String>() + "…";
    }
    d
}

/// One editing session: at most one open case, its undo history and the journal.
pub struct Session {
    opened: Option<Opened>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    journal: Vec<Value>,
    backup: String,
    /// How many times a case was opened: a failed command that opened one is told apart from
    /// one that didn't, even when it opened the same location again.
    opens: u64,
    /// What is selected (patches, surface regions): what a command run without an explicit
    /// target acts on. Set by the `select.*` commands; opening a case clears it.
    selection: Vec<Sel>,
    /// The 3D scene last built, for picking and snapshots.
    scene: Option<std::sync::Arc<cmd::scene::Cached>>,
    /// What `mesh.check` found last in each region it checked (faces and cells), for the 3D
    /// view's cuts; opening a case or reading its meshes again (`reload`) drops it.
    found: Vec<(String, std::sync::Arc<umcf_render::Found>)>,
    /// The whole meshes `mesh.check` read, handed to the next scene built (within the budget for
    /// hidden objects), so that showing what it found reads nothing again.
    handed: Vec<cmd::scene::Whole>,
    /// Regions `mesh.check` checked without handing their mesh over (no room for it, or it
    /// couldn't be read): the whole meshes the scene kept of them from before are not used again
    /// (the files may have changed since).
    outdated: Vec<String>,
    /// Stops what a command reads from a case `case.open` opened (see [`Session::canceller`]).
    cancel: umcf_core::Cancel,
    /// The memory the session plans with (see [`Budget`]).
    budget: Budget,
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
    opens: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The journal only grows during a command (or is cleared by an open): its length is enough.
    journal: usize,
    /// What `mesh.check` found and handed over (a `reload` drops it; a command that fails after
    /// gives it back).
    found: Vec<(String, std::sync::Arc<umcf_render::Found>)>,
    handed: Vec<cmd::scene::Whole>,
    outdated: Vec<String>,
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
        Session {
            opened: None,
            undo: Vec::new(),
            redo: Vec::new(),
            journal: Vec::new(),
            backup: backup.into(),
            opens: 0,
            selection: Vec::new(),
            scene: None,
            found: Vec::new(),
            handed: Vec::new(),
            outdated: Vec::new(),
            cancel: umcf_core::Cancel::new(),
            budget: Budget::system(),
        }
    }

    /// The memory the session plans with.
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// Plans with `budget` from now on (the next scene built keeps to it).
    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    /// The flag that stops the command running now, from another thread (a *Cancel* button):
    /// the files of a case `case.open` opened are read no more, and the command fails as
    /// [`Error::Cancelled`] with the case as it was. It stays set until reset, so whoever sets it
    /// resets it before the next command. Its progress adds up over commands until reset too
    /// (the desktop app resets it for each job; the CLI shows none).
    pub fn canceller(&self) -> umcf_core::Cancel {
        self.cancel.clone()
    }

    /// `e`, or [`Error::Cancelled`] when the command was cancelled (whatever the read that stopped
    /// made of it). A save that wrote some files before it stopped keeps its own error, which
    /// lists them: the case is not as it was.
    fn or_cancelled(&self, id: &str, e: Error) -> Error {
        let partial = matches!(e, Error::Model(umcf_model::Error::Partial { .. }));
        if self.cancel.is_cancelled() && !partial { Error::Cancelled { id: id.to_string() } } else { e }
    }

    pub fn case(&self) -> Option<&Case> {
        self.opened.as_ref().map(|o| &o.case)
    }

    /// Where the open case is (the directory given to `case.open`).
    pub fn location(&self) -> Option<&str> {
        self.opened.as_ref().map(|o| o.location.as_str())
    }

    /// The open case's files (`Sync`: the mesh reader reads its files in parallel).
    pub fn fs(&self) -> Option<&(dyn CaseWrite + Send + Sync)> {
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

    /// The undo history: the edits that can be undone, oldest first, and those that can be
    /// redone, next first.
    pub fn history(&self) -> (Vec<&Edit>, Vec<&Edit>) {
        (self.undo.iter().map(|s| &s.edit).collect(), self.redo.iter().rev().map(|s| &s.edit).collect())
    }

    /// What is selected, in the order it was selected.
    pub fn selection(&self) -> &[Sel] {
        &self.selection
    }

    /// The edits made to the open case, in order, as `{"command", "params"}` objects.
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
        fs: Box<dyn CaseWrite + Send + Sync>,
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
        let case = Case::open_with(fs.as_ref(), options).map_err(|e| self.or_cancelled("case.open", e.into()))?;
        // Cancelled while it read: a file it couldn't read is a note in the case, not an error,
        // so the case would open half read. The case that was open stays.
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled { id: "case.open".to_string() });
        }
        self.opened = Some(Opened { case, fs, location: location.into() });
        self.undo.clear();
        self.redo.clear();
        self.journal.clear();
        self.selection.clear();
        self.scene = None;
        self.found.clear();
        self.handed.clear();
        self.outdated.clear();
        self.opens += 1;
        Ok(())
    }

    fn saved(&self) -> Saved {
        Saved {
            case: self.case().cloned(),
            opens: self.opens,
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            journal: self.journal.len(),
            found: self.found.clone(),
            handed: self.handed.clone(),
            outdated: self.outdated.clone(),
        }
    }

    /// Puts back what [`Session::saved`] kept. A case opened by the failed command is closed
    /// again if there was none before; if there was one, the newly opened case stays, with an
    /// empty history and journal.
    fn restore(&mut self, saved: Saved) {
        // Of the case that was open; a case opened since starts without them.
        if self.opens == saved.opens {
            (self.found, self.handed, self.outdated) = (saved.found, saved.handed, saved.outdated);
        }
        match (saved.case, &mut self.opened) {
            (Some(case), Some(o)) if self.opens == saved.opens => o.case = case,
            // The command opened a case (the same one again, or another) and then failed: keep
            // the case it opened, with a new history and journal, rather than pair a model, its
            // history or its journal with the wrong files.
            (Some(_), _) => {
                self.undo.clear();
                self.redo.clear();
                self.journal.clear();
                return;
            }
            (None, _) => self.opened = None,
        }
        self.undo = saved.undo;
        self.redo = saved.redo;
        self.journal.truncate(saved.journal);
    }

    fn run_spec(&mut self, spec: &CommandSpec, params: &Value) -> Result<Outcome, Error> {
        (spec.enabled)(self).map_err(|reason| Error::Disabled { id: spec.id.to_string(), reason })?;
        let saved = self.saved();
        let before = if spec.undoable { self.case().cloned() } else { None };
        let result = catch_unwind(AssertUnwindSafe(|| (spec.run)(self, params)));
        match result {
            // A command that only reads ([`CommandSpec::reads_only`]), cancelled while it read: what
            // it couldn't read became notes or diagnostics, not an error. It is cancelled all the
            // same, and the session is as it was. One that writes keeps what it did.
            // A case it opened already (the cancel came after) stays open, and so does the result.
            Ok(Ok(_)) if self.cancel.is_cancelled() && spec.reads_only && self.opens == saved.opens => {
                self.restore(saved);
                // A scene it built may lack what it couldn't read.
                if spec.id == "scene.snapshot" {
                    self.scene = None;
                }
                Err(Error::Cancelled { id: spec.id.to_string() })
            }
            Ok(Ok(outcome)) => {
                self.prune_selection();
                if let Some(case) = before {
                    let edit = Edit { command: spec.id, label: spec.label, detail: detail(params) };
                    self.undo.push(Step { case, edit });
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
                Err(self.or_cancelled(spec.id, e))
            }
            Err(payload) => {
                self.restore(saved);
                Err(Error::Panicked { id: spec.id.to_string(), message: panic_message(payload.as_ref()) })
            }
        }
    }

    /// Drops from the selection the patches the case no longer has (a rename undone, say).
    fn prune_selection(&mut self) {
        let Some(case) = self.opened.as_ref().map(|o| &o.case) else { return };
        self.selection.retain(|sel| match sel {
            // A region whose patch list the model couldn't read keeps its selection.
            Sel::Patch { region, patch } => case
                .region(region)
                .is_some_and(|r| r.patches.as_ref().is_none_or(|ps| ps.iter().any(|p| p.name == *patch))),
            Sel::Solid { .. } => true,
            Sel::Face { region, face } => case.region(region).is_some_and(|r| {
                r.patches.as_ref().is_none_or(|ps| ps.iter().any(|p| p.faces().contains(&(*face as usize))))
            }),
        });
    }

    fn opened_mut(&mut self) -> Result<&mut Opened, Error> {
        self.opened.as_mut().ok_or_else(|| Error::Failed("no case is open".to_string()))
    }

    /// A copy to look at while this session is away, running a job on another thread: the
    /// case, its location, the history, the selection and (`with_scene`) the 3D scene as they
    /// are now, its cache shared, not copied. It reads no file: a command that needs one fails,
    /// and a scene not in its cache can't be built.
    pub fn stand_in(&self, with_scene: bool) -> Session {
        let mut s = Session::with_backup(self.backup.clone());
        s.opened = self.opened.as_ref().map(|o| Opened {
            case: o.case.clone(),
            fs: Box::new(Away),
            location: o.location.clone(),
        });
        s.undo = self.undo.clone();
        s.redo = self.redo.clone();
        s.opens = self.opens;
        s.selection = self.selection.clone();
        s.scene = if with_scene { self.scene.clone() } else { None };
        s.found = self.found.clone();
        // What a check handed over, or made outdated, is taken into the scene by this session's
        // next build: the copy, which reads nothing, draws the scene as it is (a pending change
        // would have it build, copying the scene whole).
        s.budget = self.budget;
        s
    }
}

/// The files of a [`Session::stand_in`]: they are with the job.
struct Away;

impl Away {
    fn away(rel: &str) -> FsError {
        FsError::Io {
            path: rel.to_string(),
            source: std::io::Error::other("the case's files are in use by a job; if none runs, open the case again"),
        }
    }
}

impl CaseFs for Away {
    fn read(&self, rel: &str, _limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        Err(Away::away(rel))
    }
    fn is_file(&self, _rel: &str) -> bool {
        false
    }
    fn is_dir(&self, _rel: &str) -> bool {
        false
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, FsError> {
        Err(Away::away(rel))
    }
}

impl CaseWrite for Away {
    fn write(&mut self, rel: &str, _bytes: &[u8]) -> Result<(), FsError> {
        Err(Away::away(rel))
    }
    fn rename_dir(&mut self, from: &str, _to: &str) -> Result<(), FsError> {
        Err(Away::away(from))
    }
}

/// `enabled` for commands that need an open case.
fn needs_case(s: &Session) -> Result<(), String> {
    if s.opened.is_some() { Ok(()) } else { Err("no case is open".to_string()) }
}

#[cfg(test)]
mod tests;

/// What a caught panic said.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}
