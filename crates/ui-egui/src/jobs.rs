//! Work that may take long (opening a case, a check, building the 3D scene) runs on a thread of
//! its own, with the session moved there: the window keeps drawing, says what runs and for how
//! long, and can cancel it ([`Session::canceller`]). The window waits a moment first
//! ([`App::job_wait`]): work done by then looks as if it ran in place, with no busy card.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::Value;
use umcf_engine::{Error, Outcome, Session};

use crate::{App, Dialog};

/// What a job does.
pub(crate) enum Work {
    /// A command, and what the window does with its outcome.
    Command { id: String, params: Value, then: Then },
    /// The 3D scene for these parameters (as [`App::shown`] gives them), built into the session's
    /// cache.
    Scene { params: Value },
}

/// What the window does once a command ran.
pub(crate) enum Then {
    Nothing,
    /// `case.open` of a folder: the *Open* card again, with why, when it didn't open.
    Open {
        path: String,
    },
    /// The command card it ran from: open again, with the error, when the command failed.
    Dialog(Dialog),
}

/// What came back.
enum Done {
    Command(Result<Outcome, Error>),
    Scene(Result<(), Error>),
}

/// A job running.
pub(crate) struct Job {
    /// What runs, for people (*Opening case…*).
    pub label: String,
    /// The case open when it started (its folder's name), for the window's bar.
    pub case: Option<String>,
    pub started: Instant,
    pub cancel: umcf_engine::Cancel,
    /// Whether it may be cancelled: work that only reads (a save is never cut short).
    pub cancellable: bool,
    work: Work,
    done: Receiver<(Session, Done)>,
}

impl Job {
    /// Whether *Cancel* was pressed.
    pub fn cancelling(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// What `work` does, run on `session` (on the job's thread, or here when no thread starts).
fn perform(session: &mut Session, work: &Work) -> Done {
    match work {
        Work::Command { id, params, .. } => Done::Command(session.execute(id, params)),
        Work::Scene { params } => Done::Scene(session.scene(params).map(|_| ())),
    }
}

/// Whether `work` may be cancelled: commands that only read the case
/// ([`umcf_engine::CommandSpec::reads_only`]) and reading the scene. Whatever writes runs to its
/// end.
pub(crate) fn cancellable(work: &Work) -> bool {
    match work {
        Work::Command { id, .. } => Session::command(id).is_some_and(|c| c.reads_only),
        Work::Scene { .. } => true,
    }
}

fn label(work: &Work) -> String {
    match work {
        Work::Command { id, params, .. } => match id.as_str() {
            "case.open" => match params.get("path").and_then(Value::as_str) {
                Some(p) => format!("Opening {}", p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)),
                None => "Opening a case".to_string(),
            },
            _ => umcf_engine::Session::command(id).map_or(id.clone(), |s| s.label.to_string()),
        },
        Work::Scene { .. } => "Reading the meshes and surfaces to show".to_string(),
    }
}

impl App {
    /// Queues `work`: it starts at the top of the next frame ([`App::launch`]), before anything
    /// draws, so no part of a frame draws from the stand-in session. While a job runs or waits,
    /// another is refused, and the log says so.
    pub(crate) fn start(&mut self, work: Work) {
        if let Some(label) = self.job.as_ref().map(|j| j.label.clone()).or_else(|| self.pending.as_ref().map(label)) {
            self.log_entry(crate::Entry {
                command: match &work {
                    Work::Command { id, .. } => id.clone(),
                    Work::Scene { .. } => "scene".to_string(),
                },
                ok: false,
                message: format!("not run: {} first", label.to_lowercase()),
                data: Value::Null,
            });
            // What the work came from stays, to run again.
            if let Work::Command { then, id, .. } = work {
                match then {
                    Then::Dialog(mut d) => {
                        d.error = Some(format!("{id} can't run yet: {} first", label.to_lowercase()));
                        self.dialog = Some(d);
                    }
                    Then::Open { path } => {
                        let params =
                            serde_json::to_string_pretty(&serde_json::json!({ "path": path })).unwrap_or_default();
                        let error = Some(format!("not opened yet: {} first", label.to_lowercase()));
                        self.dialog = Some(Dialog { id: "case.open", params, error });
                    }
                    Then::Nothing => {}
                }
            }
            return;
        }
        self.pending = Some(work);
    }

    /// Starts the queued work with the session moved to a thread of its own, and waits for it a
    /// moment ([`App::job_wait`]). Where no thread starts, it runs here.
    pub(crate) fn launch(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(work) = self.pending.take() else { return };
        let cancel = self.session.canceller();
        cancel.reset();
        let (give, take) = std::sync::mpsc::channel::<(Session, Work)>();
        let (send, done) = std::sync::mpsc::channel::<(Session, Done)>();
        // The thread waits for the session: if it doesn't start, the session is still here.
        let started = std::thread::Builder::new().name("umcf-job".into()).spawn(move || {
            let Ok((mut session, work)) = take.recv() else { return };
            // Session::execute and Session::scene guard against panics themselves; this guard
            // is for the rest, so that the session (the user's edits) always comes back.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| perform(&mut session, &work)))
                .unwrap_or_else(|payload| {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "a panic".to_string());
                    let id = match &work {
                        Work::Command { id, .. } => id.clone(),
                        Work::Scene { .. } => "scene".to_string(),
                    };
                    let e = Error::Panicked { id, message };
                    match &work {
                        Work::Command { .. } => Done::Command(Err(e)),
                        Work::Scene { .. } => Done::Scene(Err(e)),
                    }
                });
            let _ = send.send((session, result));
        });
        let label = label(&work);
        if started.is_err() {
            let result = perform(&mut self.session, &work);
            self.finish(work, result);
            return;
        }
        let case = self.case_name();
        let stand_in = Session::with_backup(self.session.backup_dir());
        let session = std::mem::replace(&mut self.session, stand_in);
        let work_for_thread = match &work {
            Work::Command { id, params, .. } => {
                Work::Command { id: id.clone(), params: params.clone(), then: Then::Nothing }
            }
            Work::Scene { params } => Work::Scene { params: params.clone() },
        };
        if let Err(std::sync::mpsc::SendError((session, _))) = give.send((session, work_for_thread)) {
            // The thread is gone already: run it here.
            self.session = session;
            let result = perform(&mut self.session, &work);
            self.finish(work, result);
            return;
        }
        #[allow(clippy::disallowed_methods)] // The desktop app is native only.
        let now = Instant::now();
        let cancellable = cancellable(&work);
        self.job = Some(Job { label, case, started: now, cancel, cancellable, work, done });
        self.poll(self.job_wait);
    }

    /// Takes the job's result when it is there (waiting up to `wait`), puts the session back and
    /// does what the work's outcome asks for. A job whose thread died without sending the session
    /// back (it can't panic: the engine guards it) leaves an empty session, and says so.
    pub(crate) fn poll(&mut self, wait: Duration) {
        let Some(job) = &self.job else { return };
        match job.done.recv_timeout(wait) {
            Ok((session, result)) => {
                self.session = session;
                // The job's own flag: a click on Cancel as it ended stops nothing after it.
                self.session.canceller().reset();
                if let Some(job) = self.job.take() {
                    self.finish(job.work, result);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let label = self.job.take().map_or(String::new(), |j| j.label);
                self.revision += 1;
                self.log_entry(crate::Entry {
                    command: "job".to_string(),
                    ok: false,
                    message: format!("{label}: its thread stopped; the case is closed"),
                    data: Value::Null,
                });
                self.reset_shown();
            }
        }
    }

    /// Whether a job runs (the window shows it, and nothing else can start).
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Whether work that can't be cancelled (it writes) runs.
    pub fn writing(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.cancellable) || self.pending.as_ref().is_some_and(|w| !cancellable(w))
    }

    /// Whether work is queued or running.
    pub fn working(&self) -> bool {
        self.job.is_some() || self.pending.is_some()
    }

    /// Starts building the scene shown, as the 3D view would (tests).
    #[cfg(test)]
    pub(crate) fn start_scene_for_test(&mut self) {
        let params = self.shown();
        let built = self.scene_built.as_ref().is_some_and(|(r, p)| *r == self.revision && *p == params);
        if !built {
            self.start(Work::Scene { params });
            self.launch();
        }
    }

    /// Starts what is queued and waits for it to finish (tests, screenshots).
    pub fn wait_for_job(&mut self) {
        self.launch();
        while self.job.is_some() {
            self.poll(Duration::from_millis(50));
        }
    }

    fn finish(&mut self, work: Work, result: Done) {
        match (work, result) {
            (Work::Command { id, then, .. }, Done::Command(result)) => {
                let cancelled = matches!(result, Err(Error::Cancelled { .. }));
                // The engine may have dropped its scene: the next one is read as a job.
                if cancelled {
                    self.scene_built = None;
                }
                let result = self.after(&id, result);
                match (then, result) {
                    (Then::Nothing, _) => {}
                    // Cancelled on purpose: no card to say so again.
                    (Then::Open { .. }, Err(_)) if cancelled => {}
                    (Then::Open { path }, result) => self.opened_folder(&path, result),
                    (Then::Dialog(_), Ok(_)) => {}
                    (Then::Dialog(mut d), Err(e)) => {
                        d.error = Some(e);
                        self.dialog = Some(d);
                    }
                }
                // A fit asked for while it ran.
                if let Some(view) = self.fit_pending.take() {
                    self.fit_view(view);
                }
            }
            (Work::Scene { params }, Done::Scene(result)) => match result {
                Ok(()) => {
                    self.scene_built = Some((self.revision, params));
                    if let Some(view) = self.fit_pending.take() {
                        self.fit_view(view);
                    }
                }
                Err(Error::Cancelled { .. }) => {
                    self.fit_pending = None;
                    let message = "Reading cancelled: Fit (F), or a change to what is shown, reads it again.";
                    self.scene_error = Some((self.revision, message.to_string()));
                }
                Err(e) => {
                    self.fit_pending = None;
                    self.scene_error = Some((self.revision, e.to_string()));
                }
            },
            // A job gives back what its work makes.
            (Work::Command { .. }, Done::Scene(_)) | (Work::Scene { .. }, Done::Command(_)) => {}
        }
    }
}
