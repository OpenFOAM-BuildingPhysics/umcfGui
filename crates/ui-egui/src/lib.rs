//! The umcfGui desktop interface (egui). It is thin: panels read the engine's state and change
//! it only by running commands ([`App::run`]), the same ones the CLI and scripts run.
//!
//! The layout follows SolidCraft's (MIT OR Apache-2.0, Copyright (c) 2026 SolidCraft
//! contributors): a dark application bar; a workspace card beside a ribbon of commands in
//! groups, with the command search; the 3D view filling the window with floating cards over it
//! (the browser on the left, properties and results on the right, the navigation bar); the edit
//! history along the bottom. The code is umcfGui's own.
//!
//! - The ribbon ([`ribbon`]) and the search are built from the command registry
//!   (`umcf_engine::COMMANDS`): a command that can't run is shown disabled, with the reason. A
//!   command with parameters opens its card (its parameter text and a JSON editor); one without
//!   runs at once. Shortcuts come from the registry too.
//! - The browser lists the case's regions with their patches and its STL surfaces with their
//!   regions; a click selects (`select.set`), the eye says what the 3D view shows.
//! - The 3D view ([`viewport`]) draws the scene with the CPU renderer; a click picks
//!   (`select.pick`).
//! - Colours and sizes come from [`theme::Tokens`]; icons are drawn in code ([`icons`]).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::BTreeSet;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Id, Key, KeyboardShortcut, Layout, Modifiers, Rect, Response, RichText,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use serde_json::json;
use umcf_engine::{COMMANDS, CommandSpec, Outcome, Sel, Session};
use umcf_render::Source;
use umcf_render::camera::{Camera, View};

pub mod bc;
pub mod cut;
pub mod display;
mod gpu;
pub mod icons;
mod jobs;
pub mod newpatch;
pub mod patch;
pub mod results;
pub mod ribbon;
pub mod theme;
pub mod viewport;

#[cfg(test)]
mod tests;

use icons::{Icon, Ink};
use ribbon::{Item, Workspace};
use theme::Tokens;
use umcf_engine::Layer;
use umcf_render::raster::Display;
pub use viewport::Renderer;
use viewport::{Action, Drag, Viewport};

pub use serde_json::Value;

/// One command that ran, for the results card and the status line.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: String,
    pub ok: bool,
    pub message: String,
    pub data: Value,
}

/// A command waiting for its parameters, in the properties card.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub id: &'static str,
    pub params: String,
    pub error: Option<String>,
}

/// The interface's state around a [`Session`].
pub struct App {
    session: Session,
    pub tokens: Tokens,
    pub viewport: Viewport,
    /// Mesh regions and surfaces the 3D view shows.
    pub shown_regions: Vec<String>,
    pub shown_surfaces: Vec<String>,
    /// How the 3D view draws each layer, as `scene.snapshot`'s `display` takes it.
    pub display: Vec<umcf_engine::DisplayEntry>,
    pub workspace: Workspace,
    pub tab: usize,
    pub workspace_menu: bool,
    /// The command search's text; the results show while it has the focus.
    pub search: String,
    focus_search: bool,
    /// The search's results are shown: from when the field gets the focus until a command is
    /// chosen, Escape, or a click elsewhere.
    pub search_open: bool,
    pub dialog: Option<Dialog>,
    /// The folder dialog *Open* uses (see [`Options::pick_folder`]).
    pub pick_folder: Option<PickFolder>,
    /// *Open* asked for the folder dialog: the window shows it after this frame, over itself
    /// (see [`App::folder_picked`]).
    pub folder_requested: bool,
    /// The boundary-condition card, while it is open.
    pub bc: Option<bc::BcForm>,
    /// The patch card, while it is open.
    pub patch_form: Option<patch::PatchForm>,
    /// The cut card, while a cut is shown.
    pub cut: Option<cut::CutForm>,
    pub new_patch: Option<newpatch::NewPatchForm>,
    /// Commands that ran, newest last (at most [`LOG`]).
    pub log: Vec<Entry>,
    /// The results card is open (it opens when a command reports diagnostics).
    pub results_open: bool,
    /// Browser rows that are open.
    pub expanded: BTreeSet<String>,
    /// Goes up whenever a command ran: the 3D view draws again.
    revision: u64,
    /// Goes up whenever how the layers are drawn changes (`display`): the 3D view draws again,
    /// from the scene it has (nothing in the case changed).
    pub(crate) look: u64,
    /// The case the shown lists were made for.
    shown_for: Option<String>,
    /// The open case's STL surfaces, listed again after each command (not on every frame).
    surfaces: Vec<String>,
    /// They couldn't be listed when the case opened: the first listing that works shows them
    /// as opening would have.
    unlisted: bool,
    /// Why the last listing of the surfaces failed; `None` when it worked.
    listing_error: Option<String>,
    /// Names [`App::show_only`] was asked for while the surfaces couldn't be listed: the first
    /// listing that works shows those that are surfaces.
    waiting: Vec<String>,
    /// The scene of this revision failed to build, with why: it isn't tried again until the
    /// revision changes.
    scene_error: Option<(u64, String)>,
    /// The revision and parameters the session's scene was last built for: drawing it again
    /// takes it from the session's cache, on this thread; anything else is built as a job.
    scene_built: Option<(u64, Value)>,
    /// The work running on a thread of its own, with the session (see [`jobs`]).
    job: Option<jobs::Job>,
    /// Work queued to start at the top of the next frame.
    /// Work asked for and not started yet, in order (see [`jobs`]).
    queue: std::collections::VecDeque<jobs::Work>,
    /// The regions of the surfaces shown, as the browser listed them last.
    classes: Vec<(String, Vec<String>)>,
    /// *Fit* waits for the scene being read: the view it asked for.
    fit_pending: Option<Option<View>>,
    /// The next fit frames what *Check mesh* found, shown in the cut card, not the whole scene.
    pub(crate) fit_found: bool,
    /// How long a job is waited for before the window shows it running (a quick one looks as
    /// if it ran in place).
    pub job_wait: std::time::Duration,
    styled: bool,
}

/// The most log entries kept.
pub const LOG: usize = 200;

const SEARCH_ID: &str = "command-search";

/// The *Add a region* card's parameters to start with: a solid, its name and materials to fill
/// in.
const ADD_REGION: &str = "{\n  \"region\": \"\",\n  \"kind\": \"solid\",\n  \"materials\": []\n}";

/// The *Set a material's coefficients* card's parameters to start with.
const SET_MATERIAL: &str = "{\n  \"region\": \"\",\n  \"material\": \"\",\n  \"coefficients\": { \"rho\": 1600 }\n}";

/// The *Switch a component* card's parameters to start with (`patches` are the grass's).
const SWITCH_COMPONENT: &str = "{\n  \"component\": \"temperatureLimit\",\n  \"on\": true\n}";

/// The *New case* card's parameters to start with: the air alone, for OpenFOAM-12, from the
/// tutorials' settings.
const NEW_CASE: &str = "{\n  \"path\": \"\",\n  \"version\": 12,\n  \"regions\": [\n    { \"name\": \"air\", \"kind\": \"fluid\" }\n  ]\n}";

/// A registry shortcut (`Ctrl+Shift+Z`, `F7`) as egui's.
pub fn shortcut(text: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+') {
        match part {
            "Ctrl" | "Cmd" => mods |= Modifiers::COMMAND,
            "Shift" => mods |= Modifiers::SHIFT,
            "Alt" => mods |= Modifiers::ALT,
            k => key = Key::from_name(k),
        }
    }
    key.map(|k| KeyboardShortcut::new(mods, k))
}

/// Commands whose parameters are all optional and whose defaults are what a button means:
/// they run at once, from the ribbon, the search and `--open`.
const DIRECT: [&str; 5] = ["case.check", "case.fields", "case.info", "geometry.check", "mesh.check"];

/// Whether a command runs without asking for parameters.
fn takes_none(spec: &CommandSpec) -> bool {
    if DIRECT.contains(&spec.id) {
        return true;
    }
    let p = spec.params.trim().trim_end_matches('.');
    p == "none" || p.is_empty()
}

/// Command parameters from JSON text.
pub fn parse_params(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// No parameters.
pub fn empty_params() -> Value {
    json!({})
}

/// A square icon button; `label` names it for tooltips and accessibility. `on` draws it
/// pressed (a mode that is active).
#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    size: f32,
    enabled: bool,
    on: bool,
    label: &str,
    ink: Ink,
    t: &Tokens,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    if on {
        ui.painter().rect_filled(rect, 5.0, t.accent_soft);
    } else if r.hovered() && enabled {
        ui.painter().rect_filled(rect, 5.0, t.hover);
    }
    let ink =
        if enabled { ink } else { Ink { line: ink.line.gamma_multiply(0.4), fill: ink.fill.gamma_multiply(0.4) } };
    icons::paint(ui.painter(), rect.shrink(size * 0.2), icon, ink);
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, on, label));
    r
}

fn tip(spec: &CommandSpec, enabled: &Result<(), String>) -> String {
    match (enabled, spec.shortcut) {
        (Err(reason), _) => format!("{}: {reason}", spec.label),
        (Ok(()), Some(s)) => format!("{} ({s})\n{}", spec.label, spec.id),
        (Ok(()), None) => format!("{}\n{}", spec.label, spec.id),
    }
}

impl App {
    pub fn new(session: Session) -> App {
        let mut app = App {
            session,
            tokens: Tokens::light(),
            viewport: Viewport::default(),
            shown_regions: Vec::new(),
            shown_surfaces: Vec::new(),
            display: Vec::new(),
            workspace: Workspace::Setup,
            tab: 0,
            workspace_menu: false,
            search: String::new(),
            focus_search: false,
            search_open: false,
            dialog: None,
            pick_folder: None,
            folder_requested: false,
            bc: None,
            patch_form: None,
            cut: None,
            new_patch: None,
            log: Vec::new(),
            results_open: false,
            expanded: BTreeSet::new(),
            revision: 0,
            look: 0,
            shown_for: None,
            surfaces: Vec::new(),
            unlisted: false,
            listing_error: None,
            waiting: Vec::new(),
            scene_error: None,
            scene_built: None,
            job: None,
            queue: std::collections::VecDeque::new(),
            fit_pending: None,
            fit_found: false,
            classes: Vec::new(),
            job_wait: std::time::Duration::from_millis(150),
            styled: false,
        };
        app.reset_shown();
        app
    }

    /// The open case's folder name, for the bar.
    fn case_name(&self) -> Option<String> {
        self.session.location().map(|l| l.trim_end_matches('/').rsplit('/').next().unwrap_or(l).to_string())
    }

    /// The session; while a job runs ([`App::busy`]) an empty one stands in for it.
    pub fn session(&self) -> &Session {
        &self.session
    }

    fn ink(&self) -> Ink {
        Ink { line: self.tokens.text, fill: self.tokens.accent }
    }

    /// What the 3D view shows when a case opens: every region with a mesh, or every surface
    /// when there is none (as `scene.snapshot` does).
    fn reset_shown(&mut self) {
        self.shown_for = self.session.location().map(String::from);
        let listed = self.list_surfaces();
        self.unlisted = listed.is_none() && self.session.case().is_some();
        self.waiting.clear();
        self.surfaces = listed.unwrap_or_default();
        self.viewport.camera = None;
        self.expanded.clear();
        self.display.clear();
        // A cut is of the case that was open (its entry went with the display's).
        self.cut = None;
        let Some(case) = self.session.case() else {
            self.shown_regions.clear();
            self.shown_surfaces.clear();
            return;
        };
        self.shown_regions = case.regions.iter().filter(|r| r.patches.is_some()).map(|r| r.name.clone()).collect();
        self.expanded.insert("regions".into());
        if case.regions.len() == 1 {
            self.expanded.extend(case.regions.iter().map(|r| format!("region:{}", r.name)));
        }
        self.shown_surfaces = if self.shown_regions.is_empty() { self.surfaces.clone() } else { Vec::new() };
        if !self.shown_surfaces.is_empty() {
            self.expanded.insert("surfaces".into());
        }
    }

    /// The case's surfaces; a failure to list them goes in the log and the results card.
    fn list_surfaces(&mut self) -> Option<Vec<String>> {
        let listed = self.session.surfaces();
        self.listing_error = listed.as_ref().err().map(ToString::to_string);
        match listed {
            Ok(s) => Some(s),
            Err(e) => {
                self.log_entry(Entry {
                    command: "surfaces".to_string(),
                    ok: false,
                    message: e.to_string(),
                    data: Value::Null,
                });
                None
            }
        }
    }

    /// Drops from what the 3D view shows the regions and surfaces the case no longer has (a
    /// surface an undo took away): the scene would not build without them.
    fn prune_shown(&mut self) {
        // A listing that failed keeps the last one, and what is shown with it.
        let Some(surfaces) = self.list_surfaces() else { return };
        self.surfaces = surfaces;
        if std::mem::take(&mut self.unlisted) && self.shown_regions.is_empty() && self.shown_surfaces.is_empty() {
            self.shown_surfaces = self.surfaces.clone();
            if !self.shown_surfaces.is_empty() {
                self.expanded.insert("surfaces".into());
            }
            self.revision += 1;
        }
        let waiting = std::mem::take(&mut self.waiting);
        for f in &self.surfaces {
            let named = waiting.iter().any(|n| f == n || f.rsplit('/').next() == Some(n.as_str()));
            if named && !self.shown_surfaces.contains(f) {
                self.shown_surfaces.push(f.clone());
                self.expanded.insert("surfaces".into());
                self.revision += 1;
            }
        }
        let surfaces = &self.surfaces;
        let regions: Vec<String> = self
            .session
            .case()
            .map_or(Vec::new(), |c| c.regions.iter().filter(|r| r.patches.is_some()).map(|r| r.name.clone()).collect());
        let before = (self.shown_regions.len(), self.shown_surfaces.len());
        self.shown_regions.retain(|r| regions.contains(r));
        self.shown_surfaces.retain(|f| surfaces.contains(f));
        if before != (self.shown_regions.len(), self.shown_surfaces.len()) {
            self.revision += 1;
        }
    }

    /// Shows only these mesh regions and surfaces (by region name or surface file name) in the
    /// 3D view; returns the names that are neither. While the surfaces can't be listed, those
    /// names wait for the first listing that works.
    pub fn show_only(&mut self, names: &[String]) -> Vec<String> {
        // What is shown is chosen now: a listing that works later doesn't change it.
        self.unlisted = false;
        let regions: Vec<String> = self.session.case().map_or(Vec::new(), |c| {
            c.regions
                .iter()
                .filter(|r| r.patches.is_some() && names.contains(&r.name))
                .map(|r| r.name.clone())
                .collect()
        });
        let surfaces: Vec<String> = self
            .surfaces
            .iter()
            .filter(|f| names.iter().any(|n| *f == n || f.rsplit('/').next() == Some(n.as_str())))
            .cloned()
            .collect();
        if !surfaces.is_empty() {
            self.expanded.insert("surfaces".into());
            self.expanded.extend(surfaces.iter().map(|f| format!("surface:{f}")));
        }
        let unknown = names
            .iter()
            .filter(|n| {
                !regions.contains(n) && !surfaces.iter().any(|f| f == *n || f.rsplit('/').next() == Some(n.as_str()))
            })
            .cloned()
            .collect::<Vec<String>>();
        self.waiting = if self.listing_error.is_some() { unknown.clone() } else { Vec::new() };
        self.shown_regions = regions;
        self.shown_surfaces = surfaces;
        self.viewport.camera = None;
        self.revision += 1;
        unknown
    }

    /// Runs a command here (on this thread), logs it and redraws; returns its outcome. While a
    /// job runs it is refused.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Outcome, String> {
        if let Some(job) = &self.job {
            return Err(format!("{id} can't run: {} is running", job.label.to_lowercase()));
        }
        // A flag left set by a job cancelled as it ended stops nothing here.
        self.session.canceller().reset();
        let result = self.session.execute(id, &params);
        self.after(id, result)
    }

    /// The GPU switch's tooltip: what it does, or why it can't.
    fn renderer_tip(&self) -> String {
        match &self.viewport.no_gpu {
            Some(why) => format!("The 3D view is drawn on the CPU: {why}"),
            None => "Draw the 3D view on the GPU".to_string(),
        }
    }

    /// Switches the 3D view between the GPU and the CPU, where it can be drawn on the GPU.
    fn toggle_renderer(&mut self) {
        self.viewport.renderer = match self.viewport.renderer {
            Renderer::Cpu if self.viewport.has_gpu() => Renderer::Gpu,
            _ => Renderer::Cpu,
        };
    }

    /// Runs a quick command here, or, while a job has the session, after it (in the queue).
    pub(crate) fn act(&mut self, id: &str, params: Value) {
        if self.working() {
            self.start(jobs::Work::Command { id: id.to_string(), params, then: jobs::Then::Nothing });
        } else {
            let _ = self.run(id, params);
        }
    }

    /// What follows a command, here or as a job: it is logged, the view draws again, and what
    /// is shown follows the case.
    fn after(&mut self, id: &str, result: Result<Outcome, umcf_engine::Error>) -> Result<Outcome, String> {
        let opens = matches!(id, "case.open" | "case.new");
        self.revision += 1;
        let entry = match &result {
            Ok(o) => Entry { command: id.to_string(), ok: true, message: o.message.clone(), data: o.data.clone() },
            Err(e) => Entry { command: id.to_string(), ok: false, message: e.to_string(), data: Value::Null },
        };
        self.log_entry(entry);
        if (opens && result.is_ok()) || self.session.location().map(String::from) != self.shown_for {
            self.reset_shown();
        } else {
            self.prune_shown();
            self.prune_display();
        }
        result.map_err(|e| e.to_string())
    }

    /// Logs an entry, opening the results card when it failed, has diagnostics, or returned a
    /// workflow description (with its problems and notes).
    fn log_entry(&mut self, entry: Entry) {
        if entry.data.get("diagnostics").and_then(Value::as_array).is_some_and(|d| !d.is_empty())
            || !entry.ok
            || !results::lines(&entry.data).is_empty()
        {
            self.results_open = true;
        }
        self.log.push(entry);
        if self.log.len() > LOG {
            self.log.remove(0);
        }
    }

    /// What the ribbon and the search do with a command: run one without parameters, open the
    /// card for one with.
    pub fn invoke(&mut self, id: &'static str) {
        let Some(spec) = Session::command(id) else { return };
        // Opening a case asks for its folder in the system's dialog, when there is one.
        if id == "case.open" && self.pick_folder.is_some() {
            self.folder_requested = true;
            return;
        }
        // Setting a condition and editing patches have forms of their own.
        if matches!(id, "bc.set" | "patch.set" | "patch.create") {
            self.dialog = None;
            self.close_idle_cards();
            if self.bc.is_none() && id == "bc.set" {
                self.bc = Some(bc::BcForm::default());
            }
            if self.patch_form.is_none() && id == "patch.set" {
                self.patch_form = Some(patch::PatchForm::default());
            }
            if self.new_patch.is_none() && id == "patch.create" {
                self.new_patch = Some(newpatch::NewPatchForm::default());
            }
            return;
        }
        if takes_none(spec) {
            self.start(jobs::Work::Command { id: id.to_string(), params: json!({}), then: jobs::Then::Nothing });
        } else {
            self.close_idle_cards();
            let params = match id {
                "scene.snapshot" => self.snapshot_params(),
                "case.new" => NEW_CASE.to_string(),
                "region.add" => ADD_REGION.to_string(),
                "workflow.component" => SWITCH_COMPONENT.to_string(),
                "material.set" => SET_MATERIAL.to_string(),
                "workflow.settings" => self.settings_params(),
                _ => "{}".to_string(),
            };
            self.dialog = Some(Dialog { id, params, error: None });
        }
    }

    /// The *Choose settings* card's parameters to start with: the case's settings, as its files
    /// have them.
    fn settings_params(&self) -> String {
        let Some(case) = self.session.case() else { return "{}".to_string() };
        let (s, _) = umcf_model::settings::Settings::from_case(case);
        serde_json::to_string_pretty(&umcf_engine::settings_json(&s)).unwrap_or_else(|_| "{}".to_string())
    }

    /// The *Snapshot* card's parameters to start with: the grid, the origin and the background
    /// as the 3D view has them switched (only those that differ from the snapshot's defaults).
    fn snapshot_params(&self) -> String {
        let mut p = serde_json::Map::new();
        let v = &self.viewport;
        for (key, on) in [("grid", v.guides.grid), ("origin", v.guides.origin), ("dark", v.dark)] {
            if !on {
                p.insert(key.to_string(), json!(false));
            }
        }
        serde_json::to_string_pretty(&p).ok().filter(|_| !p.is_empty()).unwrap_or_else(|| "{}".to_string())
    }

    /// The 3D view's background, middle colour (behind the view, and dimmed while it is busy).
    fn view_middle(&self) -> Color32 {
        let [top, bottom] = self.viewport.background();
        let c = |v: [f32; 3]| Color32::from_rgb((v[0] * 255.0) as u8, (v[1] * 255.0) as u8, (v[2] * 255.0) as u8);
        c(top).lerp_to_gamma(c(bottom), 0.5)
    }

    /// Closes the edit cards, apart from one whose command waits or runs: it stays till it is
    /// done (its outcome closes it, or shows its error).
    fn close_idle_cards(&mut self) {
        if !self.card_busy(jobs::Card::Bc) {
            self.bc = None;
        }
        if !self.card_busy(jobs::Card::Patch) {
            self.patch_form = None;
        }
        if !self.card_busy(jobs::Card::NewPatch) {
            self.new_patch = None;
        }
    }

    /// What the folder dialog gave: the case opens, nothing happens, or (no dialog) the *Open*
    /// card asks for the path.
    pub fn folder_picked(&mut self, picked: Picked) {
        match picked {
            Picked::Folder(path) => self.open_folder(&path),
            Picked::Cancelled => {}
            Picked::Unavailable => {
                self.close_idle_cards();
                let error = "No folder dialog on this system (a desktop portal or zenity): type the case's path.";
                self.dialog = Some(Dialog {
                    id: "case.open",
                    params: "{\n  \"path\": \"\"\n}".to_string(),
                    error: Some(error.to_string()),
                });
            }
        }
    }

    /// Opens the case in `path`. When that doesn't work (unsaved changes, not a case), the
    /// *Open* card shows the path and why, to change or confirm (`discard`).
    pub fn open_folder(&mut self, path: &str) {
        let work = jobs::Work::Command {
            id: "case.open".to_string(),
            // Its fields are read after it opens, as a job of their own.
            params: json!({ "path": path, "fields": "later" }),
            then: jobs::Then::Open { path: path.to_string() },
        };
        self.start(work);
    }

    /// What follows opening the folder `path` (see [`App::open_folder`]).
    fn opened_folder(&mut self, path: &str, result: Result<Outcome, String>) {
        // An Open card from an earlier folder that didn't open goes once one does.
        if result.is_ok() && self.dialog.as_ref().is_some_and(|d| d.id == "case.open") {
            self.dialog = None;
        }
        if let Err(e) = result {
            self.close_idle_cards();
            let params = serde_json::to_string_pretty(&json!({ "path": path })).unwrap_or_default();
            self.dialog = Some(Dialog { id: "case.open", params, error: Some(e) });
        }
    }

    /// Frames the scene the 3D view shows from `view`; `None` keeps the direction. A findings
    /// fit asked for before and still waiting gives way to it.
    pub fn fit_view(&mut self, view: Option<View>) {
        self.fit_found = false;
        self.fit(view);
    }

    /// Frames what the cut card shows other than a plane's cells: what *Check mesh* found, the
    /// layers at a wall, the cells near a surface (the whole scene where nothing is).
    pub(crate) fn fit_findings(&mut self) {
        self.fit_found = true;
        self.fit(None);
    }

    /// The fit [`App::fit_view`] or [`App::fit_findings`] asked for: now, or, while work runs or
    /// the scene isn't read, once it is (`fit_pending`, with `fit_found` kept).
    pub(crate) fn fit(&mut self, view: Option<View>) {
        // While work runs or waits, the fit follows the next scene read.
        if self.working() {
            self.fit_pending = Some(view);
            return;
        }
        let params = self.shown();
        // A scene not read yet (or whose reading failed or was cancelled) is read as a job; the
        // fit follows when it is there.
        let built = self.scene_built.as_ref().is_some_and(|(r, p)| *r == self.revision && *p == params);
        if !built {
            self.scene_error = None;
            self.fit_pending = Some(view);
            self.start(jobs::Work::Scene { params });
            return;
        }
        let aspect = self.viewport.aspect();
        let (az, el) = match view {
            Some(v) => v.angles(),
            None => self.viewport.camera.map_or(View::Iso.angles(), |c| (c.azimuth, c.elevation)),
        };
        // What the cut card shows (what the check found, the layers at a wall or the cells near a
        // surface), when the card asked for it (else, or when it is empty, the scene).
        let found = std::mem::take(&mut self.fit_found)
            .then(|| self.cut.as_ref().filter(|f| f.show != crate::cut::Show::Plane).map(|f| f.region.clone()))
            .flatten();
        match self.session.scene(&params) {
            Ok(scene) => {
                let of_found = found.and_then(|r| {
                    let o = scene.objects.iter().find(|o| o.source == Source::Cut { region: r.clone() })?;
                    o.bbox(scene.origin)
                });
                if let Some(b) = of_found.or_else(|| scene.bbox()) {
                    self.viewport.camera = Some(Camera::fit_angles(az, el, &b, aspect));
                }
                // It builds now (a file changed outside the window): the view draws it.
                self.scene_error = None;
            }
            Err(e) => self.scene_error = Some((self.revision, e.to_string())),
        }
    }

    /// The parameters of the scene the 3D view shows.
    fn shown(&self) -> Value {
        let mut p = json!({ "regions": self.shown_regions, "surfaces": self.shown_surfaces });
        if let Some(c) = self.cut_shown() {
            p["cuts"] = json!([c]);
        }
        p
    }

    /// Undo or redo until `done` edits are done, as a job.
    pub fn go_to(&mut self, done: usize) {
        self.start(jobs::Work::History { done });
    }

    /// Runs card command `id` as a job, the card staying open until it ends.
    pub(crate) fn start_card(&mut self, id: &str, params: Value, card: jobs::Card) {
        self.start(jobs::Work::Command { id: id.to_string(), params, then: jobs::Then::Card(card) });
    }

    /// The whole interface.
    pub fn show(&mut self, ui: &mut Ui) {
        if !self.styled {
            ui.ctx().set_fonts(theme::fonts());
            self.tokens.apply(ui.ctx());
            self.styled = true;
        }
        self.poll(std::time::Duration::ZERO);
        self.launch();
        if self.modal_busy() {
            self.busy_screen(ui);
            return;
        }
        self.shortcuts(ui.ctx());
        let t = self.tokens.clone();
        let gap = t.gap as i8;
        egui::Panel::top("bar")
            .exact_size(t.bar_height)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(t.bar).inner_margin(egui::Margin::symmetric(8, 0)))
            .show(ui, |ui| self.app_bar(ui));
        egui::Panel::top("ribbon")
            .exact_size(t.ribbon_height + 2.0 * t.gap)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(egui::Margin::same(gap)))
            .show(ui, |ui| self.ribbon_row(ui));
        egui::Panel::bottom("history")
            .exact_size(42.0 + t.gap)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(egui::Margin {
                left: gap,
                right: gap,
                top: 0,
                bottom: gap,
            }))
            .show(ui, |ui| self.history(ui));
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(egui::Margin {
                left: gap,
                right: gap,
                top: 0,
                bottom: gap,
            }))
            .show(ui, |ui| self.canvas(ui));
        self.workspace_popup(ui.ctx());
        // Queued work starts next frame, input or not.
        if !self.queue.is_empty() {
            ui.ctx().request_repaint();
        }
    }

    /// The window while a job runs: the last image of the 3D view, dimmed, and a card with what
    /// runs, for how long, and *Cancel*.
    fn busy_screen(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        // An open or save waits for work to start (the next frame): ask for it.
        let Some(job) = &self.job else {
            ui.ctx().request_repaint();
            return;
        };
        let rect = ui.max_rect();
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, t.chrome);
        // The bar, with the case.
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), t.bar_height));
        p.rect_filled(bar, CornerRadius::ZERO, t.bar);
        let logo = Rect::from_center_size(pos2(bar.min.x + 19.0, bar.center().y), vec2(22.0, 22.0));
        icons::paint(p, logo, Icon::Logo, Ink { line: t.bar_text, fill: t.bar_accent });
        let name = job.case.as_deref().unwrap_or("umcfGui");
        let at = pos2(logo.max.x + 10.0, bar.center().y);
        p.text(at, egui::Align2::LEFT_CENTER, name, theme::strong(14.0), t.bar_text);
        // The 3D view as it was, dimmed.
        let view = self.viewport.rect.unwrap_or_else(|| rect.with_min_y(bar.max.y).shrink(t.gap));
        p.rect_filled(view, CornerRadius::same(t.radius_card), self.view_middle());
        self.viewport.paint_image(p, view);
        p.rect_filled(
            view,
            CornerRadius::same(t.radius_card),
            Color32::from_black_alpha(if t.dark { 110 } else { 50 }),
        );
        let (label, cancelling, secs) = (job.label.clone(), job.cancelling(), job.started.elapsed().as_secs());
        let next = if job.modal { None } else { self.modal_waiting() };
        let cancellable = job.cancellable;
        let mut cancel = false;
        egui::Area::new(Id::unique("job")).pivot(Align2::CENTER_CENTER).fixed_pos(view.center()).fade_in(false).show(
            ui.ctx(),
            |ui| {
                egui::Frame::new()
                    .fill(t.card)
                    .stroke(egui::Stroke::new(1.0, t.card_border))
                    .corner_radius(CornerRadius::same(t.radius_card))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.label(RichText::new(format!("{label}…")).font(theme::strong(14.0)).color(t.text));
                        let status = if cancelling { "Cancelling…".to_string() } else { format!("{secs} s") };
                        ui.label(RichText::new(status).color(t.text_muted));
                        if let Some(next) = &next {
                            ui.label(RichText::new(format!("Then: {next}")).color(t.text_muted));
                        }
                        ui.add_space(8.0);
                        if !cancellable {
                            ui.label(RichText::new("It changes the case: it runs to its end.").color(t.text_muted));
                        } else if ui.add_enabled(!cancelling, egui::Button::new("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
            },
        );
        if cancel && let Some(job) = &self.job {
            job.cancel.cancel();
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.dialog = None;
            // A card whose command waits or runs stays till it is done.
            self.close_idle_cards();
            self.results_open = false;
            self.workspace_menu = false;
            self.search.clear();
            self.search_open = false;
            ctx.memory_mut(|m| m.surrender_focus(Id::unique(SEARCH_ID)));
        }
        if ctx.input_mut(|i| {
            i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::P))
                || i.consume_key(Modifiers::NONE, Key::F1)
        }) {
            self.focus_search = true;
        }
        if typing {
            return;
        }
        // Longer shortcuts first, so Ctrl+Shift+Z is not taken for Ctrl+Z.
        let mut keyed: Vec<(&'static str, KeyboardShortcut)> =
            COMMANDS.iter().filter_map(|c| c.shortcut.and_then(shortcut).map(|s| (c.id, s))).collect();
        keyed.sort_by_key(|(_, s)| std::cmp::Reverse(u8::from(s.modifiers.shift) + u8::from(s.modifiers.alt)));
        for (id, s) in keyed {
            if ctx.input_mut(|i| i.consume_shortcut(&s)) && self.session.enabled(id).is_ok() {
                self.invoke(id);
            }
        }
    }

    /// The dark bar: the name, save, undo and redo, the open case, the theme.
    fn app_bar(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let ink = Ink { line: t.bar_text, fill: t.bar_accent };
        let bar = Tokens { hover: t.bar_raised, accent_soft: t.bar_raised, ..t.clone() };
        ui.horizontal_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            icons::paint(ui.painter(), rect, Icon::Logo, ink);
            ui.label(RichText::new("umcfGui").font(theme::strong(14.0)).color(t.bar_text));
            ui.add_space(12.0);
            for (id, icon) in [
                ("case.open", Icon::Open),
                ("case.save", Icon::Save),
                ("edit.undo", Icon::Undo),
                ("edit.redo", Icon::Redo),
            ] {
                let Some(spec) = Session::command(id) else { continue };
                let enabled = self.session.enabled(id);
                let r = icon_button(ui, icon, 26.0, enabled.is_ok(), false, spec.label, ink, &bar);
                if r.on_hover_text(tip(spec, &enabled)).clicked() {
                    self.invoke(id);
                }
            }
            ui.add_space(12.0);
            // The open case, as a tab.
            let name = match self.session.location() {
                Some(l) => l.trim_end_matches('/').rsplit('/').next().unwrap_or(l).to_string(),
                None => "No case open".to_string(),
            };
            let unsaved = self.session.case().map_or(0, |c| c.changed().len());
            let text = if unsaved > 0 { format!("{name} •") } else { name };
            let galley = ui.painter().layout_no_wrap(text, theme::strong(13.0), t.bar_text);
            let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, 26.0), Sense::hover());
            ui.painter().rect_filled(rect, 5.0, t.bar_raised);
            ui.painter().galley(rect.left_center() + vec2(12.0, -galley.size().y / 2.0), galley, t.bar_text);
            if let Some(l) = self.session.location() {
                let text = if unsaved > 0 { format!("{l}\n{unsaved} unsaved") } else { l.to_string() };
                r.on_hover_text(text);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(t.bar_muted).size(11.5));
                let label = if t.dark { "Light theme" } else { "Dark theme" };
                if icon_button(ui, Icon::Theme, 26.0, true, false, label, ink, &bar).on_hover_text(label).clicked() {
                    self.toggle_theme(ui.ctx());
                }
            });
        });
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.tokens = if self.tokens.dark { Tokens::light() } else { Tokens::dark() };
        self.tokens.apply(ctx);
    }

    /// The workspace card and the ribbon card.
    fn ribbon_row(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let full = ui.available_rect_before_wrap();
        let ws = Rect::from_min_size(full.min, vec2(94.0, full.height()));
        let rib = Rect::from_min_max(pos2(ws.right() + t.gap, full.top()), full.max);
        for r in [ws, rib] {
            ui.painter().rect(r, t.radius_card, t.card, Stroke::new(1.0, t.card_border), StrokeKind::Inside);
        }
        // The workspace card: a button that opens the switcher.
        let r = ui.interact(ws, Id::unique("workspace-card"), Sense::click());
        if r.hovered() || self.workspace_menu {
            ui.painter().rect_filled(ws.shrink(1.0), t.radius_card, t.accent_soft);
        }
        ui.painter().rect_filled(
            Rect::from_min_size(ws.min + vec2(14.0, 0.0), vec2(ws.width() - 28.0, 3.0)),
            1.5,
            t.accent,
        );
        let icon_rect = Rect::from_center_size(pos2(ws.center().x, ws.top() + 34.0), vec2(30.0, 30.0));
        icons::paint(ui.painter(), icon_rect, self.workspace.icon(), self.ink());
        ui.painter().text(
            pos2(ws.center().x, ws.top() + 62.0),
            Align2::CENTER_CENTER,
            self.workspace.name(),
            theme::strong(14.0),
            t.accent,
        );
        let label = ui.painter().text(
            pos2(ws.center().x - 5.0, ws.top() + 82.0),
            Align2::CENTER_CENTER,
            "Workspace",
            egui::FontId::proportional(11.5),
            t.text_muted,
        );
        let c = pos2(label.right() + 7.0, label.center().y);
        let open = self.workspace_menu;
        let pts = if open {
            [c + vec2(-3.0, 1.5), c + vec2(0.0, -1.5), c + vec2(3.0, 1.5)]
        } else {
            [c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)]
        };
        ui.painter().add(egui::Shape::line(pts.to_vec(), Stroke::new(1.2, t.text_muted)));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Workspace"));
        if r.clicked() {
            self.workspace_menu = !self.workspace_menu;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(rib.shrink2(vec2(10.0, 5.0))).layout(Layout::top_down(Align::Min)),
        );
        self.ribbon(&mut child);
    }

    fn ribbon(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let tabs = self.workspace.tabs();
        ui.horizontal(|ui| {
            ui.set_height(26.0);
            for (k, tab) in tabs.iter().enumerate() {
                let on = k == self.tab;
                let font = if on { theme::strong(13.5) } else { egui::FontId::proportional(13.5) };
                let galley =
                    ui.painter().layout_no_wrap(tab.name.to_string(), font, if on { t.accent } else { t.text });
                let (rect, r) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 25.0), Sense::click());
                if on {
                    ui.painter().rect_filled(rect, t.radius, t.accent_soft);
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, t.radius, t.hover);
                }
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, tab.name));
                if r.clicked() {
                    self.tab = k;
                }
            }
            if tabs.is_empty() {
                ui.label(RichText::new(self.workspace.name()).font(theme::strong(13.5)).color(t.accent));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.search_field(ui));
        });
        let line = ui.cursor().top() + 2.0;
        ui.painter().hline(ui.max_rect().x_range(), line, Stroke::new(1.0, t.card_border));
        ui.add_space(7.0);
        let Some(tab) = tabs.get(self.tab).or_else(|| tabs.first()) else {
            ui.add_space(16.0);
            ui.label(
                RichText::new(format!("{}: planned. {}.", self.workspace.name(), self.workspace.about()))
                    .color(t.text_muted),
            );
            return;
        };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (name, items) in ribbon::groups(tab) {
                ui.vertical(|ui| {
                    ui.horizontal_top(|ui| {
                        for item in &items {
                            self.ribbon_item(ui, item);
                        }
                    });
                    let w = ui.min_rect().width();
                    ui.allocate_ui(vec2(w, 14.0), |ui| {
                        ui.centered_and_justified(|ui| ui.label(RichText::new(name).size(11.5).color(t.text_muted)));
                    });
                });
                ui.add_space(6.0);
                let x = ui.cursor().left();
                let top = ui.min_rect().top() + 2.0;
                ui.painter().vline(x, top..=top + 64.0, Stroke::new(1.0, t.card_border));
                ui.add_space(8.0);
            }
            if let Some(note) = tab.note {
                ui.vertical(|ui| {
                    ui.add_space(20.0);
                    ui.label(RichText::new(note).italics().size(12.5).color(t.text_muted));
                });
            }
        });
    }

    /// A big ribbon button: the icon over its label.
    fn ribbon_button(&self, ui: &mut Ui, icon: Icon, label: &str, enabled: bool, on: bool) -> Response {
        let t = &self.tokens;
        let font = egui::FontId::proportional(12.5);
        let width = ui.painter().layout_no_wrap(label.to_string(), font.clone(), t.text).size().x.max(36.0) + 16.0;
        let (rect, r) =
            ui.allocate_exact_size(vec2(width, 52.0), if enabled { Sense::click() } else { Sense::hover() });
        if on {
            ui.painter().rect(
                rect,
                t.radius,
                t.accent_soft,
                Stroke::new(1.0, t.accent.gamma_multiply(0.45)),
                StrokeKind::Inside,
            );
        } else if r.hovered() && enabled {
            ui.painter().rect_filled(rect, t.radius, t.hover);
        }
        let ink = if enabled {
            self.ink()
        } else {
            Ink { line: t.text_muted.gamma_multiply(0.55), fill: t.text_muted.gamma_multiply(0.45) }
        };
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(rect.center().x, rect.top() + 17.0), vec2(26.0, 26.0)),
            icon,
            ink,
        );
        let colour = if enabled { t.text } else { t.text_muted.gamma_multiply(0.8) };
        ui.painter().text(pos2(rect.center().x, rect.top() + 42.0), Align2::CENTER_CENTER, label, font, colour);
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, on, label));
        r
    }

    fn ribbon_item(&mut self, ui: &mut Ui, item: &Item) {
        match *item {
            Item::Command(id, label, icon) => {
                let Some(spec) = Session::command(id) else { return };
                let enabled = self.session.enabled(id);
                let on = self.dialog.as_ref().is_some_and(|d| d.id == id);
                let r = self.ribbon_button(ui, icon, label, enabled.is_ok(), on);
                if r.on_hover_text(tip(spec, &enabled)).clicked() && enabled.is_ok() {
                    self.invoke(id);
                }
            }
            Item::View(view, label, icon) => {
                let has = self.session.case().is_some();
                if self
                    .ribbon_button(ui, icon, label, has, false)
                    .on_hover_text(format!("{label} view of the whole scene"))
                    .clicked()
                {
                    self.fit_view(Some(view));
                }
            }
            Item::Edges => {
                let on = self.viewport.edges;
                if self
                    .ribbon_button(ui, Icon::Edges, "Edges", true, on)
                    .on_hover_text("Draw the faces' edges")
                    .clicked()
                {
                    self.viewport.edges = !on;
                }
            }
            Item::Cut => {
                let on = self.cut.is_some();
                let can = on || self.can_cut();
                let tip = if can { CUT_TIP } else { "Cut: no region of the case has a mesh" };
                if self.ribbon_button(ui, Icon::Cut, "Cut", can, on).on_hover_text(tip).clicked() {
                    self.toggle_cut();
                }
            }
            Item::Grid => {
                let on = self.viewport.guides.grid;
                if self.ribbon_button(ui, Icon::Grid, "Grid", true, on).on_hover_text(GRID_TIP).clicked() {
                    self.viewport.guides.grid = !on;
                }
            }
            Item::Origin => {
                let on = self.viewport.guides.origin;
                if self.ribbon_button(ui, Icon::Origin, "Origin", true, on).on_hover_text(ORIGIN_TIP).clicked() {
                    self.viewport.guides.origin = !on;
                }
            }
            Item::DarkView => {
                let on = self.viewport.dark;
                if self.ribbon_button(ui, Icon::DarkView, "Dark view", true, on).on_hover_text(DARK_TIP).clicked() {
                    self.viewport.dark = !on;
                }
            }
            Item::Renderer => {
                let on = self.viewport.renderer == Renderer::Gpu;
                let can = self.viewport.has_gpu();
                if self.ribbon_button(ui, Icon::Gpu, "GPU", can, on).on_hover_text(self.renderer_tip()).clicked() {
                    self.toggle_renderer();
                }
            }
            Item::Theme => {
                let label = if self.tokens.dark { "Light" } else { "Dark" };
                if self
                    .ribbon_button(ui, Icon::Theme, label, true, false)
                    .on_hover_text("Light or dark theme")
                    .clicked()
                {
                    self.toggle_theme(ui.ctx());
                }
            }
        }
    }

    /// The command search: every command of the registry, filtered as you type (Ctrl+P).
    fn search_field(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let edit = egui::TextEdit::singleline(&mut self.search)
            .id(Id::unique(SEARCH_ID))
            .hint_text("Search commands      Ctrl+P")
            .desired_width(240.0)
            .margin(egui::Margin { left: 26, right: 6, top: 4, bottom: 4 });
        let r = ui.add(edit);
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search commands"));
        let icon = Rect::from_center_size(r.rect.left_center() + vec2(13.0, 0.0), vec2(14.0, 14.0));
        icons::paint(ui.painter(), icon, Icon::Search, Ink { line: t.text_muted, fill: t.accent });
        if self.focus_search {
            r.request_focus();
            self.focus_search = false;
        }
        let entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if r.has_focus() {
            self.search_open = true;
        }
        if !self.search_open {
            return;
        }
        let q = self.search.to_lowercase();
        let matches: Vec<&'static CommandSpec> = COMMANDS
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.id.contains(&q)
                    || c.label.to_lowercase().contains(&q)
                    || c.menu.to_lowercase().contains(&q)
            })
            .collect();
        if entered {
            self.search_open = false;
            if let Some(c) = matches.iter().find(|c| self.session.enabled(c.id).is_ok()) {
                self.search.clear();
                self.invoke(c.id);
            }
            return;
        }
        let mut chosen = None;
        let popup = egui::Area::new(Id::unique("search-results"))
            .order(egui::Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(r.rect.right_bottom() + vec2(0.0, 6.0))
            .show(ui.ctx(), |ui| {
                t.card_frame().show(ui, |ui| {
                    ui.set_width(420.0);
                    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        for c in matches {
                            let enabled = self.session.enabled(c.id);
                            let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                            if row.hovered() && enabled.is_ok() {
                                ui.painter().rect_filled(rect, t.radius, t.hover);
                            }
                            let ink = if enabled.is_ok() {
                                self.ink()
                            } else {
                                Ink { line: t.text_muted, fill: t.text_muted }
                            };
                            let ir = Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(20.0, 20.0));
                            icons::paint(ui.painter(), ir, ribbon::icon_of(c.id), ink);
                            let colour = if enabled.is_ok() { t.text } else { t.text_muted };
                            let p = ui.painter();
                            p.text(
                                rect.left_center() + vec2(36.0, -7.0),
                                Align2::LEFT_CENTER,
                                c.label,
                                egui::FontId::proportional(13.0),
                                colour,
                            );
                            let sub = match &enabled {
                                Err(reason) => reason.clone(),
                                Ok(()) => c.id.to_string(),
                            };
                            p.text(
                                rect.left_center() + vec2(36.0, 9.0),
                                Align2::LEFT_CENTER,
                                sub,
                                egui::FontId::proportional(11.0),
                                t.text_muted,
                            );
                            if let Some(s) = c.shortcut {
                                p.text(
                                    rect.right_center() - vec2(8.0, 0.0),
                                    Align2::RIGHT_CENTER,
                                    s,
                                    egui::FontId::proportional(11.5),
                                    t.text_muted,
                                );
                            }
                            row.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled.is_ok(), c.label));
                            if row.clicked() && enabled.is_ok() {
                                chosen = Some(c.id);
                            }
                        }
                    });
                });
            });
        let pressed_elsewhere =
            ui.input(|i| i.pointer.any_pressed()) && !popup.response.contains_pointer() && !r.contains_pointer();
        if let Some(id) = chosen {
            self.search.clear();
            self.search_open = false;
            ui.ctx().memory_mut(|m| m.surrender_focus(Id::unique(SEARCH_ID)));
            self.invoke(id);
        } else if pressed_elsewhere {
            self.search_open = false;
        }
    }

    fn workspace_popup(&mut self, ctx: &egui::Context) {
        if !self.workspace_menu {
            return;
        }
        let t = self.tokens.clone();
        let top = t.bar_height + t.gap + t.ribbon_height + 6.0;
        let mut chosen = None;
        let area = egui::Area::new(Id::unique("workspaces")).order(egui::Order::Foreground).fixed_pos(pos2(t.gap, top));
        let r = area.show(ctx, |ui| {
            t.card_frame().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new("WORKSPACE").font(theme::strong(11.5)).color(t.text_muted));
                for w in Workspace::ALL {
                    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::click());
                    let current = w == self.workspace;
                    if current {
                        ui.painter().rect_filled(rect, t.radius, t.accent_soft);
                    } else if row.hovered() && w.available() {
                        ui.painter().rect_filled(rect, t.radius, t.hover);
                    }
                    let bar = Rect::from_min_size(rect.min + vec2(2.0, 8.0), vec2(3.0, rect.height() - 16.0));
                    ui.painter().rect_filled(bar, 1.5, if w.available() { t.accent } else { t.card_border });
                    let ink = if w.available() { self.ink() } else { Ink { line: t.text_muted, fill: t.text_muted } };
                    let ir = Rect::from_center_size(rect.left_center() + vec2(32.0, 0.0), vec2(30.0, 30.0));
                    icons::paint(ui.painter(), ir, w.icon(), ink);
                    let x = rect.left() + 62.0;
                    let p = ui.painter();
                    let name =
                        p.text(pos2(x, rect.top() + 19.0), Align2::LEFT_CENTER, w.name(), theme::strong(14.0), t.text);
                    let (badge, colour) = if current {
                        ("Current", t.accent)
                    } else if w.available() {
                        ("Available", t.info)
                    } else {
                        ("Planned", t.text_muted)
                    };
                    let g = p.layout_no_wrap(badge.to_string(), egui::FontId::proportional(11.0), colour);
                    let b =
                        Rect::from_min_size(pos2(name.right() + 8.0, rect.top() + 10.0), g.size() + vec2(12.0, 4.0));
                    p.rect(
                        b,
                        8.0,
                        colour.gamma_multiply(0.12),
                        Stroke::new(1.0, colour.gamma_multiply(0.5)),
                        StrokeKind::Inside,
                    );
                    p.galley(b.min + vec2(6.0, 2.0), g, colour);
                    p.text(
                        pos2(x, rect.top() + 41.0),
                        Align2::LEFT_CENTER,
                        w.about(),
                        egui::FontId::proportional(12.5),
                        t.text_muted,
                    );
                    row.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, w.available(), current, w.name())
                    });
                    if row.clicked() && w.available() {
                        chosen = Some(w);
                    }
                }
                ui.separator();
                ui.label(
                    RichText::new("Every workspace works on the same case and selection; only the ribbon changes.")
                        .size(12.0)
                        .color(t.text_muted),
                );
            });
        });
        if let Some(w) = chosen {
            self.workspace = w;
            self.tab = 0;
            self.workspace_menu = false;
        } else if ctx.input(|i| i.pointer.any_pressed()) && !r.response.contains_pointer() {
            let over_card = ctx.pointer_interact_pos().is_some_and(|p| p.x < t.gap + 94.0 && p.y < top);
            if !over_card {
                self.workspace_menu = false;
            }
        }
    }

    /// The 3D view with the floating cards over it.
    fn canvas(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, CornerRadius::same(t.radius_card), self.view_middle());
        // Text on the view in the muted colour of the view's own shade, whatever the theme's.
        let muted = if self.viewport.dark { Tokens::dark() } else { Tokens::light() }.text_muted;
        let mut action = None;
        if self.session.case().is_none() {
            let text = RichText::new("Open a case (Ctrl+O) to see it here.").size(15.0).color(muted);
            ui.put(rect, egui::Label::new(text));
        } else if self.shown_regions.is_empty() && self.shown_surfaces.is_empty() && self.cut_shown().is_none() {
            let text = RichText::new("Nothing shown: turn on a region or a surface in the browser.").color(muted);
            ui.put(rect, egui::Label::new(text));
        } else {
            let params = self.shown();
            let selection = self.session.selection().to_vec();
            let revision = self.revision;
            let entries = self.display.clone();
            let failed = self.scene_error.as_ref().filter(|(r, _)| *r == revision).map(|(_, e)| e.clone());
            // A scene not built for this revision is built as a job; one that is taken from the
            // cache here.
            let built = self.scene_built.as_ref().is_some_and(|(r, p)| *r == revision && *p == params);
            let waiting = failed.is_none() && !built;
            if waiting {
                // Read as a job from the top of the next frame; till then the image as it was.
                if !self.working() {
                    self.start(jobs::Work::Scene { params: params.clone() });
                }
                self.viewport.paint_last(ui, rect, &t);
                ui.ctx().request_repaint();
            }
            let error = match failed {
                Some(e) => Some(e),
                None if waiting => None,
                None => match self.session.scene(&params) {
                    Ok(scene) => {
                        let display = umcf_engine::display_style(scene, &entries);
                        let look = self.look;
                        self.viewport.cut_plane = self.cut.as_ref().and_then(cut::CutForm::plane);
                        action = self.viewport.show(ui, rect, scene, &selection, &display, [revision, look], &t);
                        None
                    }
                    Err(e) => {
                        self.scene_error = Some((revision, e.to_string()));
                        Some(e.to_string())
                    }
                },
            };
            if let Some(e) = error {
                ui.put(rect, egui::Label::new(RichText::new(e).color(t.error)));
            }
        }
        match action {
            Some(Action::Pick { ray, mode, faces }) => {
                let display = self.display_json();
                let mut p = self.shown();
                if let Some(o) = p.as_object_mut() {
                    o.insert("ray".into(), json!({ "origin": ray.origin, "direction": ray.direction }));
                    if !display.as_array().is_none_or(Vec::is_empty) {
                        o.insert("display".into(), display.clone());
                    }
                    o.insert("mode".into(), json!(mode));
                    if faces {
                        o.insert("faces".into(), json!(true));
                    }
                }
                // Not as a job: the view takes clicks only once its scene is built, so the pick
                // reads nothing. While a job runs it waits for it.
                self.act("select.pick", p);
            }
            Some(Action::Rect { camera, size, rect, mode, faces, through }) => {
                let display = self.display_json();
                let mut p = self.shown();
                if let Some(o) = p.as_object_mut() {
                    o.insert("camera".into(), umcf_engine::camera_json(&camera));
                    o.insert("width".into(), json!(size[0]));
                    o.insert("height".into(), json!(size[1]));
                    o.insert("rect".into(), json!(rect));
                    if !display.as_array().is_none_or(Vec::is_empty) {
                        o.insert("display".into(), display.clone());
                    }
                    o.insert("mode".into(), json!(mode));
                    if faces {
                        o.insert("faces".into(), json!(true));
                    }
                    if through {
                        o.insert("through".into(), json!(true));
                    }
                }
                self.act("select.rect", p);
            }
            Some(Action::Brush { camera, size, stroke, radius, mode, faces, through }) => {
                let display = self.display_json();
                let mut p = self.shown();
                if let Some(o) = p.as_object_mut() {
                    o.insert("camera".into(), umcf_engine::camera_json(&camera));
                    o.insert("width".into(), json!(size[0]));
                    o.insert("height".into(), json!(size[1]));
                    o.insert("stroke".into(), json!(stroke));
                    o.insert("radius".into(), json!(radius));
                    if !display.as_array().is_none_or(Vec::is_empty) {
                        o.insert("display".into(), display.clone());
                    }
                    o.insert("mode".into(), json!(mode));
                    o.insert("faces".into(), json!(faces));
                    if through {
                        o.insert("through".into(), json!(true));
                    }
                }
                self.act("select.brush", p);
            }
            Some(Action::Fit(view)) => self.fit_view(Some(view)),
            Some(Action::MoveCut { point }) => self.move_cut(point),
            None => {}
        }
        self.browser(ui.ctx(), rect);
        self.right_cards(ui.ctx(), rect);
        self.navigation_bar(ui.ctx(), rect);
    }

    /// A row of the browser tree. Returns (row clicked, chevron clicked, eye clicked, the row's
    /// response: for its context menu).
    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &self,
        ui: &mut Ui,
        depth: usize,
        open: Option<bool>,
        eye: Option<bool>,
        icon: Icon,
        label: &str,
        detail: &str,
        selected: bool,
    ) -> (bool, bool, bool, egui::Response) {
        let t = &self.tokens;
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 25.0), Sense::click());
        if selected {
            ui.painter().rect_filled(rect, t.radius, t.selection.gamma_multiply(0.25));
        } else if r.hovered() {
            ui.painter().rect_filled(rect, t.radius, t.hover);
        }
        let mut x = rect.left() + 2.0 + 14.0 * depth as f32;
        let mut chevron = false;
        if let Some(o) = open {
            let c = pos2(x + 6.0, rect.center().y);
            let pts = if o {
                vec![c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)]
            } else {
                vec![c + vec2(-1.5, -3.5), c + vec2(2.0, 0.0), c + vec2(-1.5, 3.5)]
            };
            ui.painter().add(egui::Shape::line(pts, Stroke::new(1.3, t.text_muted)));
            let hit = Rect::from_center_size(c, vec2(16.0, 22.0));
            chevron = r.clicked() && r.interact_pointer_pos().is_some_and(|p| hit.contains(p));
        }
        x += 14.0;
        let mut eye_hit = false;
        if let Some(on) = eye {
            let er = Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(17.0, 17.0));
            let ink = Ink { line: if on { t.text } else { t.text_muted }, fill: t.accent };
            icons::paint(ui.painter(), er, if on { Icon::Eye } else { Icon::EyeOff }, ink);
            eye_hit = r.clicked() && r.interact_pointer_pos().is_some_and(|p| er.expand(3.0).contains(p));
            x += 20.0;
        }
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(16.0, 16.0)),
            icon,
            self.ink(),
        );
        x += 22.0;
        let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(13.0), t.text);
        let font = egui::FontId::proportional(11.5);
        let right =
            ui.painter().text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, detail, font, t.text_muted);
        let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(right.left() - 4.0, rect.bottom()));
        ui.painter().with_clip_rect(clip).galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, t.text);
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, label));
        (r.clicked() && !chevron && !eye_hit, chevron, eye_hit, r)
    }

    /// The browser card: the case's regions and patches, its surfaces and their regions.
    fn browser(&mut self, ctx: &egui::Context, canvas: Rect) {
        let t = self.tokens.clone();
        // The regions of the surfaces shown, from the scene the view draws.
        let params = self.shown();
        let failed = self.scene_error.as_ref().is_some_and(|(r, _)| *r == self.revision);
        // Only from a scene read already (taken from the cache): one still to read is a job's,
        // and the regions listed last stay till it is there.
        let built = self.scene_built.as_ref().is_some_and(|(r, p)| *r == self.revision && *p == params);
        if self.shown_surfaces.is_empty() || failed {
            self.classes.clear();
        } else if built {
            self.classes = self.session.scene(&params).map_or(Vec::new(), |scene| {
                scene
                    .objects
                    .iter()
                    .filter_map(|o| match &o.source {
                        Source::Surface { file } => Some((file.clone(), o.classes.clone())),
                        Source::Mesh { .. } | Source::Cut { .. } => None,
                    })
                    .collect()
            });
        }
        let classes = self.classes.clone();
        let mut select: Option<(Sel, bool)> = None;
        let mut change: Option<(Layer, display::Change)> = None;
        let mut toggle: Option<String> = None;
        let mut show_region: Option<String> = None;
        let mut show_surface: Option<String> = None;
        let mut open_case = false;
        let height = (canvas.height() - 2.0 * t.gap - 90.0).max(120.0);
        let area = egui::Area::new(Id::unique("browser"))
            .order(egui::Order::Middle)
            .fixed_pos(canvas.min + vec2(t.gap, t.gap));
        area.show(ctx, |ui| {
            t.card_frame().show(ui, |ui| {
                ui.set_width(272.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Browser").font(theme::strong(13.5)));
                    if let Some(case) = self.session.case() {
                        let n = case.regions.len();
                        let text = if n == 1 { "1 region".to_string() } else { format!("{n} regions") };
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(text).size(11.5).color(t.text_muted));
                        });
                    }
                });
                ui.separator();
                let Some(case) = self.session.case() else {
                    ui.label(RichText::new("No case open.").color(t.text_muted));
                    open_case = ui.button("Open a case…").clicked();
                    return;
                };
                let selection = self.session.selection();
                let surfaces = &self.surfaces;
                // The rows shown, so the card is as tall as they are (up to the view's height).
                let mut rows = 3 + if self.expanded.contains("regions") { case.regions.len() } else { 0 };
                for r in &case.regions {
                    if self.expanded.contains("regions") && self.expanded.contains(&format!("region:{}", r.name)) {
                        rows += r.patches.as_ref().map_or(0, Vec::len);
                    }
                }
                if self.expanded.contains("surfaces") {
                    rows += surfaces.len();
                    for (f, c) in &classes {
                        if self.expanded.contains(&format!("surface:{f}")) {
                            rows += c.len();
                        }
                    }
                }
                let content = rows as f32 * 26.0 + 4.0;
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .min_scrolled_height(content.min(height))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let name = self
                            .session
                            .location()
                            .map_or("case", |l| l.trim_end_matches('/').rsplit('/').next().unwrap_or(l));
                        let version = case.layout.version.map_or("", |v| v.label());
                        let _ = self.tree_row(ui, 0, None, None, Icon::Case, name, version, false);
                        let open = self.expanded.contains("regions");
                        let label = format!("Regions ({})", case.regions.len());
                        let (row, c, _, _) = self.tree_row(ui, 0, Some(open), None, Icon::Folder, &label, "", false);
                        if c || row {
                            toggle = Some("regions".into());
                        }
                        if open {
                            for r in &case.regions {
                                let key = format!("region:{}", r.name);
                                let ropen = self.expanded.contains(&key);
                                let shown = r.patches.as_ref().map(|_| self.shown_regions.contains(&r.name));
                                let label = if r.name.is_empty() { "(single region)" } else { r.name.as_str() };
                                let layer = Layer::Region(r.name.clone());
                                let entry = self.layer_entry(&layer);
                                let detail = match (&r.patches, entry.map(display::note).filter(|n| !n.is_empty())) {
                                    (Some(p), None) => format!("{} patches", p.len()),
                                    (Some(p), Some(n)) => format!("{} patches · {n}", p.len()),
                                    (None, _) => "no mesh".to_string(),
                                };
                                let chevron = r.patches.as_ref().map(|_| ropen);
                                let (row, c, e, resp) =
                                    self.tree_row(ui, 1, chevron, shown, Icon::Region, label, &detail, false);
                                if r.patches.is_some() {
                                    let current = self.layer_display(&layer, &[]);
                                    let set = entry.is_some();
                                    resp.context_menu(|ui| {
                                        let title = format!("Region {label}");
                                        if let Some(c) = display::menu(ui, &title, current, set) {
                                            change = Some((layer.clone(), c));
                                        }
                                    });
                                }
                                if c || row {
                                    toggle = Some(key.clone());
                                }
                                if e {
                                    show_region = Some(r.name.clone());
                                }
                                if ropen && let Some(patches) = &r.patches {
                                    for p in patches {
                                        let sel = Sel::Patch { region: r.name.clone(), patch: p.name.clone() };
                                        let on = selection.contains(&sel);
                                        let layer = Layer::Patch { region: r.name.clone(), patch: p.name.clone() };
                                        let entry = self.layer_entry(&layer);
                                        let note = entry.map(display::note).filter(|n| !n.is_empty());
                                        let detail = match note {
                                            None => format!("{} · {}", p.kind, p.n_faces),
                                            Some(n) => format!("{} · {} · {n}", p.kind, p.n_faces),
                                        };
                                        let (row, _, _, resp) =
                                            self.tree_row(ui, 2, None, None, Icon::Patch, &p.name, &detail, on);
                                        let current = self.layer_display(&layer, &p.groups);
                                        let set = entry.is_some();
                                        let groups: Vec<(Layer, Display, bool)> = p
                                            .groups
                                            .iter()
                                            .map(|g| {
                                                let l = Layer::Group { region: r.name.clone(), group: g.clone() };
                                                let d = self.layer_display(&l, &[]);
                                                let set = self.layer_entry(&l).is_some();
                                                (l, d, set)
                                            })
                                            .collect();
                                        resp.context_menu(|ui| {
                                            let title = format!("Patch {}", p.name);
                                            if let Some(c) = display::menu(ui, &title, current, set) {
                                                change = Some((layer.clone(), c));
                                            }
                                            for (l, d, set) in &groups {
                                                let Layer::Group { group, .. } = l else { continue };
                                                ui.separator();
                                                ui.menu_button(format!("Group {group}"), |ui| {
                                                    let title = format!("The patches of group {group}");
                                                    if let Some(c) = display::menu(ui, &title, *d, *set) {
                                                        change = Some((l.clone(), c));
                                                    }
                                                });
                                            }
                                        });
                                        if row {
                                            select = Some((sel, ui.input(|i| i.modifiers.command)));
                                        }
                                    }
                                }
                            }
                        }
                        let open = self.expanded.contains("surfaces");
                        let label = format!("Surfaces ({})", surfaces.len());
                        let (row, c, _, _) = self.tree_row(ui, 0, Some(open), None, Icon::Folder, &label, "", false);
                        if c || row {
                            toggle = Some("surfaces".into());
                        }
                        if open {
                            for f in surfaces {
                                let key = format!("surface:{f}");
                                let sopen = self.expanded.contains(&key);
                                let shown = self.shown_surfaces.contains(f);
                                let short = f.rsplit('/').next().unwrap_or(f);
                                let regions = classes.iter().find(|(g, _)| g == f).map(|(_, c)| c.clone());
                                let layer = Layer::Surface(f.clone());
                                let entry = self.layer_entry(&layer);
                                let mut detail =
                                    regions.as_ref().map_or(String::new(), |c| format!("{} regions", c.len()));
                                if let Some(n) = entry.map(display::note).filter(|n| !n.is_empty()) {
                                    detail = if detail.is_empty() { n } else { format!("{detail} · {n}") };
                                }
                                let chevron = regions.as_ref().map(|_| sopen);
                                let (row, c, e, resp) =
                                    self.tree_row(ui, 1, chevron, Some(shown), Icon::Surface, short, &detail, false);
                                let current = self.layer_display(&layer, &[]);
                                let set = entry.is_some();
                                resp.context_menu(|ui| {
                                    if let Some(c) = display::menu(ui, &format!("Surface {short}"), current, set) {
                                        change = Some((layer.clone(), c));
                                    }
                                });
                                if c || (row && regions.is_some()) {
                                    toggle = Some(key.clone());
                                }
                                if e || (row && regions.is_none()) {
                                    show_surface = Some(f.clone());
                                }
                                if sopen && let Some(regions) = regions {
                                    for name in regions {
                                        let sel = Sel::Solid { file: f.clone(), solid: name.clone() };
                                        let on = selection.contains(&sel);
                                        let layer = Layer::Solid { file: f.clone(), solid: name.clone() };
                                        let entry = self.layer_entry(&layer);
                                        let note = entry.map(display::note).unwrap_or_default();
                                        let (row, _, _, resp) =
                                            self.tree_row(ui, 2, None, None, Icon::Patch, &name, &note, on);
                                        let current = self.layer_display(&layer, &[]);
                                        let set = entry.is_some();
                                        resp.context_menu(|ui| {
                                            if let Some(c) = display::menu(ui, &format!("Region {name}"), current, set)
                                            {
                                                change = Some((layer.clone(), c));
                                            }
                                        });
                                        if row {
                                            select = Some((sel, ui.input(|i| i.modifiers.command)));
                                        }
                                    }
                                }
                            }
                        }
                    });
            });
        });
        if open_case {
            self.invoke("case.open");
        }
        if let Some(k) = toggle
            && !self.expanded.remove(&k)
        {
            self.expanded.insert(k);
        }
        for (name, regions) in [(show_region, true), (show_surface, false)] {
            let Some(name) = name else { continue };
            self.unlisted = false;
            self.waiting.clear();
            let v = if regions { &mut self.shown_regions } else { &mut self.shown_surfaces };
            match v.iter().position(|x| *x == name) {
                Some(k) => {
                    v.remove(k);
                }
                None => v.push(name),
            }
            self.revision += 1;
        }
        if let Some((layer, c)) = change {
            self.set_display(layer, c);
        }
        if let Some((sel, add)) = select {
            let selected = self.session.selection().contains(&sel);
            let mode = match (add, selected) {
                (true, true) => "remove",
                (true, false) => "add",
                (false, _) => "replace",
            };
            self.act("select.set", json!({ "items": [sel.to_json()], "mode": mode }));
        }
    }

    /// The cards on the right: a command's parameters or the selection's properties, then the
    /// results of the last command.
    fn right_cards(&mut self, ctx: &egui::Context, canvas: Rect) {
        let t = self.tokens.clone();
        let width = 330.0;
        let mut run = false;
        let mut close_dialog = false;
        let mut clear = false;
        let mut close_results = false;
        let muted = Ink { line: t.text_muted, fill: t.accent };
        // The height a card has below the triad, down to the bottom of the view.
        let room = canvas.bottom() - (canvas.top() + t.gap + 112.0) - t.gap;
        let area = egui::Area::new(Id::unique("right-cards"))
            .order(egui::Order::Middle)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(pos2(canvas.right() - t.gap, canvas.top() + t.gap + 112.0));
        let mut set_bc = false;
        let mut close_bc = false;
        let (mut set_patch, mut close_patch) = (false, false);
        let (mut make_patch, mut close_new) = (false, false);
        let mut close_cut = false;
        area.show(ctx, |ui| {
            ui.set_width(width + 16.0);
            if self.cut.is_some() {
                close_cut = self.cut_card(ui, width);
                ui.add_space(t.gap);
            }
            if self.bc.is_some() {
                (set_bc, close_bc) = self.bc_card(ui, width, room);
                ui.add_space(t.gap);
            } else if self.patch_form.is_some() {
                (set_patch, close_patch) = self.patch_card(ui, width, room);
                ui.add_space(t.gap);
            } else if self.new_patch.is_some() {
                (make_patch, close_new) = self.new_patch_card(ui, width, room);
                ui.add_space(t.gap);
            } else if let Some(d) = &mut self.dialog
                && let Some(spec) = Session::command(d.id)
            {
                t.card_frame().show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        icons::paint(ui.painter(), r, ribbon::icon_of(d.id), Ink { line: t.text, fill: t.accent });
                        ui.label(RichText::new(spec.label.trim_end_matches('…')).font(theme::strong(14.0)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close_dialog =
                                icon_button(ui, Icon::Close, 22.0, true, false, "Close", muted, &t).clicked();
                        });
                    });
                    ui.separator();
                    ui.label(RichText::new(spec.id).monospace().size(11.5).color(t.text_muted));
                    egui::ScrollArea::vertical().id_salt("params-doc").max_height(170.0).show(ui, |ui| {
                        ui.label(RichText::new(spec.params).size(12.5));
                    });
                    ui.add_space(4.0);
                    let label = ui.label(RichText::new("Parameters (JSON)").size(12.0).color(t.text_muted));
                    let edit = egui::TextEdit::multiline(&mut d.params)
                        .code_editor()
                        .desired_rows(4)
                        .desired_width(f32::INFINITY);
                    ui.add(edit).labelled_by(label.id);
                    if let Some(e) = &d.error {
                        ui.label(RichText::new(e).color(t.error));
                    }
                    ui.add_space(4.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ok = egui::Button::new(RichText::new("OK").color(Color32::WHITE).strong())
                            .fill(t.accent)
                            .min_size(vec2(64.0, 26.0));
                        run = ui.add(ok).clicked();
                        close_dialog |= ui.add(egui::Button::new("Cancel").min_size(vec2(64.0, 26.0))).clicked();
                    });
                });
                ui.add_space(t.gap);
            } else if !self.session.selection().is_empty() {
                t.card_frame().show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Properties").font(theme::strong(14.0)));
                        let n = self.session.selection().len();
                        ui.label(RichText::new(format!("{n} selected")).size(11.5).color(t.text_muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            clear = ui.small_button("Select none").clicked();
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt("properties").max_height(320.0).show(ui, |ui| {
                        // Faces in one line per region.
                        let mut faces: Vec<(&str, usize)> = Vec::new();
                        for sel in self.session.selection() {
                            match sel {
                                Sel::Patch { region, patch } => self.patch_details(ui, region, patch),
                                Sel::Solid { file, solid } => {
                                    ui.label(RichText::new(solid).font(theme::strong(13.5)));
                                    ui.label(RichText::new(format!("surface region of {file}")).color(t.text_muted));
                                }
                                Sel::Face { region, .. } => {
                                    match faces.iter_mut().find(|(r, _)| r == region) {
                                        Some((_, n)) => *n += 1,
                                        None => faces.push((region.as_str(), 1)),
                                    }
                                    continue;
                                }
                            }
                            ui.add_space(6.0);
                        }
                        for (region, n) in faces {
                            let what = if n == 1 { "1 face".to_string() } else { format!("{n} faces") };
                            ui.label(RichText::new(what).font(theme::strong(13.5)));
                            let of = if region.is_empty() {
                                "of the mesh".to_string()
                            } else {
                                format!("of region {region}")
                            };
                            ui.label(
                                RichText::new(format!("{of}: New patch makes a patch of them")).color(t.text_muted),
                            );
                            ui.add_space(6.0);
                        }
                    });
                });
                ui.add_space(t.gap);
            }
            if self.results_open
                && let Some(e) = self.log.last()
            {
                t.card_frame().show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Results").font(theme::strong(14.0)));
                        ui.label(RichText::new(&e.command).monospace().size(11.5).color(t.text_muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close_results =
                                icon_button(ui, Icon::Close, 22.0, true, false, "Close results", muted, &t).clicked();
                        });
                    });
                    ui.separator();
                    ui.label(RichText::new(&e.message).color(if e.ok { t.text } else { t.error }));
                    let diagnostics = e.data.get("diagnostics").and_then(Value::as_array).cloned().unwrap_or_default();
                    egui::ScrollArea::vertical().id_salt("results").max_height(260.0).show(ui, |ui| {
                        for d in diagnostics.iter().take(500) {
                            let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
                            let colour = match s("level") {
                                "error" => t.error,
                                "warning" => t.warning,
                                _ => t.info,
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(s("code")).monospace().size(11.5).color(colour));
                                ui.label(RichText::new(format!("{} {}", s("path"), s("message"))).size(12.5));
                            });
                        }
                        if diagnostics.len() > 500 {
                            ui.label(
                                RichText::new(format!("… and {} more", diagnostics.len() - 500)).color(t.text_muted),
                            );
                        }
                        // Limited part by part already (results::MESSAGES, results::DESCRIPTION).
                        for (kind, line) in results::lines(&e.data) {
                            let colour = match kind {
                                results::Kind::Text => t.text,
                                results::Kind::Warning => t.warning,
                                results::Kind::Info => t.info,
                            };
                            ui.label(RichText::new(line).size(12.5).color(colour));
                        }
                    });
                });
            }
        });
        if close_dialog {
            self.dialog = None;
        }
        if close_bc {
            self.bc = None;
        } else if set_bc {
            self.set_bc();
        }
        if close_patch {
            self.patch_form = None;
        } else if set_patch {
            self.set_patch();
        }
        if close_new {
            self.new_patch = None;
        } else if make_patch {
            self.create_patch();
        }
        if close_cut {
            self.close_cut();
        }
        if close_results {
            self.results_open = false;
        }
        if clear {
            self.act("select.clear", json!({}));
        }
        if run && let Some(mut d) = self.dialog.take() {
            match serde_json::from_str::<Value>(&d.params) {
                Ok(params) => {
                    let id = d.id.to_string();
                    self.start(jobs::Work::Command { id, params, then: jobs::Then::Dialog(d) });
                    return;
                }
                Err(e) => d.error = Some(format!("not JSON: {e}")),
            }
            self.dialog = Some(d);
        }
    }

    fn patch_details(&self, ui: &mut Ui, region: &str, patch: &str) {
        let t = &self.tokens;
        ui.label(RichText::new(patch).font(theme::strong(13.5)));
        let Some(r) = self.session.case().and_then(|c| c.region(region)) else { return };
        let where_ = if region.is_empty() { "patch".to_string() } else { format!("patch of {region}") };
        let Some(p) = r.patches.as_ref().and_then(|ps| ps.iter().find(|p| p.name == patch)) else {
            ui.label(RichText::new(where_).color(t.text_muted));
            return;
        };
        ui.label(RichText::new(format!("{where_} · {} · {} faces", p.kind, p.n_faces)).color(t.text_muted));
        if !p.groups.is_empty() {
            ui.label(RichText::new(format!("groups: {}", p.groups.join(", "))).color(t.text_muted));
        }
        egui::Grid::new(("bcs", region, patch)).num_columns(2).striped(true).show(ui, |ui| {
            for f in &r.fields {
                let Some(bcs) = &f.bcs else { continue };
                // The entry for the patch by its name, or else one of its groups (patterns are
                // matched by the case check, not here).
                let bc = bcs
                    .iter()
                    .find(|b| !b.pattern && b.patch == patch)
                    .or_else(|| bcs.iter().find(|b| !b.pattern && p.groups.contains(&b.patch)));
                ui.label(RichText::new(&f.name).monospace());
                match bc {
                    Some(b) => {
                        let mut text = b.kind.clone().unwrap_or_else(|| "(no type)".to_string());
                        if let Some(v) = &b.value {
                            text.push_str(&format!("  {v}"));
                        }
                        ui.label(text);
                    }
                    None => {
                        ui.label(RichText::new("by pattern or none").color(t.text_muted));
                    }
                }
                ui.end_row();
            }
        });
    }

    /// The navigation bar at the bottom of the view.
    fn navigation_bar(&mut self, ctx: &egui::Context, canvas: Rect) {
        if self.session.case().is_none() {
            return;
        }
        let t = self.tokens.clone();
        let mut fit = None;
        let mut zoom = None;
        let area = egui::Area::new(Id::unique("navigation"))
            .order(egui::Order::Middle)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(canvas.center().x, canvas.bottom() - t.gap));
        area.show(ctx, |ui| {
            t.card_frame().inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let ink = self.ink();
                    let drag = self.viewport.drag;
                    let b = |ui: &mut Ui, icon, on, label: &str, tip: &str| {
                        icon_button(ui, icon, 30.0, true, on, label, ink, &t).on_hover_text(tip).clicked()
                    };
                    if b(ui, Icon::Orbit, drag == Drag::Orbit, "Orbit", "Drag turns the view") {
                        self.viewport.drag = Drag::Orbit;
                    }
                    if b(
                        ui,
                        Icon::Pan,
                        drag == Drag::Pan,
                        "Pan",
                        "Drag moves the view (a right or middle drag always does)",
                    ) {
                        self.viewport.drag = Drag::Pan;
                    }
                    if b(
                        ui,
                        Icon::BoxSelect,
                        drag == Drag::Select,
                        "Box select",
                        "Drag a rectangle to select what is in it (Shift adds, Ctrl takes away); a right or middle drag moves the view",
                    ) {
                        self.viewport.drag = Drag::Select;
                    }
                    if b(ui, Icon::Brush, drag == Drag::Brush, "Brush", BRUSH_TIP) {
                        self.viewport.drag = Drag::Brush;
                    }
                    if b(ui, Icon::ZoomIn, false, "Zoom in", "Zoom in (wheel)") {
                        zoom = Some(0.8);
                    }
                    if b(ui, Icon::ZoomOut, false, "Zoom out", "Zoom out (wheel)") {
                        zoom = Some(1.25);
                    }
                    ui.separator();
                    if b(ui, Icon::Fit, false, "Fit", "Fit the whole scene") {
                        fit = Some(None);
                    }
                    if b(ui, Icon::Home, false, "Home", "Iso view of the whole scene") {
                        fit = Some(Some(View::Iso));
                    }
                    for (v, icon, name) in [
                        (View::Top, Icon::ViewTop, "Top"),
                        (View::Front, Icon::ViewFront, "Front"),
                        (View::Right, Icon::ViewRight, "Right"),
                    ] {
                        if b(ui, icon, false, name, &format!("{name} view")) {
                            fit = Some(Some(v));
                        }
                    }
                    ui.separator();
                    let edges = self.viewport.edges;
                    if b(ui, Icon::Edges, edges, "Edges", "Draw the faces' edges") {
                        self.viewport.edges = !edges;
                    }
                    let grid = self.viewport.guides.grid;
                    if b(ui, Icon::Grid, grid, "Grid", GRID_TIP) {
                        self.viewport.guides.grid = !grid;
                    }
                    let origin = self.viewport.guides.origin;
                    if b(ui, Icon::Origin, origin, "Origin", ORIGIN_TIP) {
                        self.viewport.guides.origin = !origin;
                    }
                    let dark = self.viewport.dark;
                    if b(ui, Icon::DarkView, dark, "Dark view", DARK_TIP) {
                        self.viewport.dark = !dark;
                    }
                    let gpu = self.viewport.renderer == Renderer::Gpu;
                    if self.viewport.has_gpu() && b(ui, Icon::Gpu, gpu, "GPU", &self.renderer_tip()) {
                        self.toggle_renderer();
                    }
                    let faces = self.viewport.faces;
                    if b(
                        ui,
                        Icon::Faces,
                        faces,
                        "Pick faces",
                        "A click picks the mesh face under it, not its patch (Ctrl+click adds or takes away one)",
                    ) {
                        self.viewport.faces = !faces;
                    }
                    let through = self.viewport.through;
                    if b(
                        ui,
                        Icon::Through,
                        through,
                        "Select through",
                        "A rectangle or a brush stroke takes everything in it, hidden too (the faces whose middle is in it), not only what is seen",
                    ) {
                        self.viewport.through = !through;
                    }
                });
            });
        });
        if let Some(z) = zoom
            && let Some(c) = &mut self.viewport.camera
        {
            c.distance *= z;
        }
        if let Some(v) = fit {
            self.fit_view(v);
        }
    }

    /// The edit history along the bottom: one chip per edit, the marker between done and
    /// undone; a click goes there.
    fn history(&mut self, ui: &mut Ui) {
        let t = self.tokens.clone();
        let full = ui.available_rect_before_wrap();
        ui.painter().rect(full, t.radius_card, t.card, Stroke::new(1.0, t.card_border), StrokeKind::Inside);
        let mut target = None;
        let builder =
            egui::UiBuilder::new().max_rect(full.shrink2(vec2(12.0, 0.0))).layout(Layout::left_to_right(Align::Center));
        let mut child = ui.new_child(builder);
        let ui = &mut child;
        ui.label(RichText::new("History").font(theme::strong(13.5)));
        ui.add_space(4.0);
        let ink = self.ink();
        let (nd, nu) = {
            let (done, undone) = self.session.history();
            (done.len(), undone.len())
        };
        for (icon, label, to, ok) in [
            (Icon::First, "Undo all", 0, nd > 0),
            (Icon::Previous, "Undo one", nd.saturating_sub(1), nd > 0),
            (Icon::Next, "Redo one", nd + 1, nu > 0),
            (Icon::Last, "Redo all", nd + nu, nu > 0),
        ] {
            if icon_button(ui, icon, 24.0, ok, false, label, ink, &t).on_hover_text(label).clicked() {
                target = Some(to);
            }
        }
        ui.add_space(8.0);
        let chip = |ui: &mut Ui, command: &str, tip: String, faded: bool, k: usize| -> bool {
            let (rect, r) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
            let fill = if faded { t.hover } else { t.accent_soft };
            let border = if r.hovered() { t.accent } else { t.card_border };
            ui.painter().rect(rect, 5.0, fill, Stroke::new(1.0, border), StrokeKind::Inside);
            let ink = if faded { Ink { line: t.text_muted, fill: t.text_muted } } else { ink };
            icons::paint(ui.painter(), rect.shrink(5.0), ribbon::icon_of(command), ink);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Edit {}", k + 1)));
            r.on_hover_text(tip).clicked()
        };
        let progress = self.progress();
        if nd + nu == 0 {
            let text = if progress.is_some() {
                "No edits yet."
            } else {
                "No edits yet: each change shows here, and a click goes back to it."
            };
            ui.label(RichText::new(text).size(12.0).color(t.text_muted));
        }
        let (done, undone): (Vec<_>, Vec<_>) = {
            let (d, u) = self.session.history();
            (d.into_iter().cloned().collect(), u.into_iter().cloned().collect())
        };
        // As many chips as fit beside the status text, around the marker: the newest done
        // edits, then the next ones to redo; "+k" says how many are left out on each side.
        // A job's progress takes room of its own on the right.
        let reserved = if progress.is_some() { 1000.0 } else { 640.0 };
        let room = ((ui.available_width() - reserved) / 30.0).floor().max(4.0) as usize;
        let show_undone = nu.min(room / 3);
        let show_done = nd.min(room.saturating_sub(show_undone));
        let show_undone = nu.min(room.saturating_sub(show_done));
        let more = |ui: &mut Ui, n: usize| {
            if n > 0 {
                ui.label(RichText::new(format!("+{n}")).size(12.0).color(t.text_muted))
                    .on_hover_text(format!("{n} more edits"));
            }
        };
        more(ui, nd - show_done);
        for (k, e) in done.iter().enumerate().skip(nd - show_done) {
            let tip = if e.detail.is_empty() { e.label.to_string() } else { format!("{}: {}", e.label, e.detail) };
            if chip(ui, e.command, tip, false, k) {
                target = Some(k + 1);
            }
        }
        if nd + nu > 0 {
            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 32.0), Sense::hover());
            ui.painter().vline(rect.center().x, rect.top()..=rect.bottom() - 6.0, Stroke::new(2.5, t.accent));
            ui.painter().circle(pos2(rect.center().x, rect.bottom() - 4.0), 4.0, t.card, Stroke::new(2.0, t.accent));
        }
        for (k, e) in undone.iter().enumerate().take(show_undone) {
            let detail = if e.detail.is_empty() { String::new() } else { format!(": {}", e.detail) };
            if chip(ui, e.command, format!("{} (undone){detail}", e.label), true, nd + k) {
                target = Some(nd + k + 1);
            }
        }
        more(ui, nu - show_undone);
        let mut cancel = false;
        let mut switch = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(p) = &progress {
                if p.cancellable {
                    let text = if p.cancelling { "Cancelling…" } else { "Cancel" };
                    cancel = ui.add_enabled(!p.cancelling, egui::Button::new(text).small()).clicked();
                }
                progress_bar(ui, p.fraction, &t);
                let waiting = if p.queued > 0 { format!(" · {} waiting", p.queued) } else { String::new() };
                let text = format!("{}… {}{waiting}", p.label, p.detail);
                ui.label(RichText::new(text).size(12.0).color(t.text));
            }
            // The last command's message, in the job's place once it is done.
            if let Some(e) = self.log.last().filter(|_| progress.is_none()) {
                let colour = if e.ok { t.text_muted } else { t.error };
                // Short, so the chips keep their room; the whole message is in the tooltip and
                // the results card.
                let full = format!("{}: {}", e.command, e.message);
                let short = if full.chars().count() > 56 {
                    full.chars().take(55).collect::<String>() + "…"
                } else {
                    full.clone()
                };
                ui.label(RichText::new(short).size(12.0).color(colour)).on_hover_text(full);
            }
            if let Some(h) = &self.viewport.hover {
                ui.separator();
                let short =
                    if h.chars().count() > 28 { h.chars().take(27).collect::<String>() + "…" } else { h.clone() };
                ui.label(RichText::new(short).size(12.0));
            }
            // What drew the 3D view, and why the CPU did when the GPU was asked for; a click
            // switches between the two where there is a GPU view (as *GPU* in the ribbon).
            if self.session.case().is_some() {
                let (by, why) = self.viewport.drawn_with();
                ui.separator();
                let colour = if why.is_some() { t.warning } else { t.text_muted };
                let tip = match (why, by, &self.viewport.no_gpu) {
                    (Some(w), _, _) => format!("Drawn on the CPU: {w}"),
                    (None, Renderer::Cpu, Some(no)) => format!("The 3D view is drawn on the CPU: {no}"),
                    (None, _, _) => format!("The 3D view is drawn on the {}", by.name()),
                };
                let can = self.viewport.has_gpu();
                let label = egui::Label::new(RichText::new(by.name()).size(12.0).color(colour)).sense(if can {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                });
                // The click switches the setting (as *GPU* in the ribbon), not what drew this
                // frame: with the GPU on, the CPU may draw a style the GPU doesn't.
                let hint = match self.viewport.renderer {
                    Renderer::Cpu => "Click to draw the view on the GPU.",
                    Renderer::Gpu => "Click to turn the GPU view off (the CPU draws the view).",
                };
                let r = ui.add(label);
                if can {
                    // A switch: a button to screen readers, not a label.
                    let name = by.name();
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                }
                let r = if can {
                    r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("{tip}. {hint}"))
                } else {
                    r.on_hover_text(tip)
                };
                switch = can && r.clicked();
            }
            if let Some(case) = self.session.case() {
                let n = case.changed().len();
                if n > 0 {
                    ui.separator();
                    ui.label(RichText::new(format!("{n} unsaved")).size(12.0).color(t.warning));
                }
            }
        });
        if cancel && let Some(job) = &self.job {
            job.cancel.cancel();
        }
        if switch {
            self.toggle_renderer();
        }
        if let Some(k) = target {
            self.go_to(k);
        }
    }
}

/// A job's progress bar: filled by `fraction` where the job knows its steps, else a block going
/// back and forth (the job works, how far it is isn't known).
fn progress_bar(ui: &mut Ui, fraction: Option<f32>, t: &Tokens) {
    let (rect, response) = ui.allocate_exact_size(vec2(160.0, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, t.hover);
    let fill = match fraction {
        Some(f) => Rect::from_min_size(rect.min, vec2(rect.width() * f, rect.height())),
        None => {
            let phase = (ui.input(|i| i.time) * 0.8).fract() as f32;
            let travel = 1.0 - 2.0 * (phase - 0.5).abs();
            let w = rect.width() * 0.3;
            Rect::from_min_size(pos2(rect.min.x + (rect.width() - w) * travel, rect.min.y), vec2(w, rect.height()))
        }
    };
    p.rect_filled(fill, 4.0, t.accent);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, "Job progress"));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

/// What the folder dialog gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Folder(String),
    Cancelled,
    /// There is no dialog to show (Linux without a desktop portal or `zenity`).
    Unavailable,
}

/// The window the desktop app runs in: the folder dialog's parent.
pub use eframe::Frame;

/// Asks for a folder in the system's own dialog, over `parent`, starting in `start`.
pub type PickFolder = fn(start: Option<&str>, parent: &Frame) -> Picked;

/// Where the desktop app starts.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The folder dialog *Open* uses; without one, *Open* asks for the path in its card.
    pub pick_folder: Option<PickFolder>,
    /// A case directory to open.
    pub case: Option<String>,
    /// Commands to run after opening it: `(id, params)`.
    pub commands: Vec<(String, Value)>,
    /// Write a PNG of the window to this file once it has drawn, then quit.
    pub screenshot: Option<String>,
    /// The window's inner size, points.
    pub size: Option<(f32, f32)>,
    pub dark: bool,
    /// What the 3D view shows first: mesh regions and surface files (`walls.stl`) by name;
    /// empty for the default.
    pub show: Vec<String>,
    /// A command whose card opens at the start, as its ribbon button opens it (`bc.set`), or
    /// `cut`: the cut card.
    pub open: Option<String>,
    /// What draws the 3D view at the start.
    pub renderer: Start,
}

/// What draws the 3D view at the start (`--renderer`). The window runs on a GPU device (wgpu)
/// where there is a hardware one, and the view can then be switched in it; else, and with
/// `--renderer cpu`, it runs on OpenGL (glow). A software device (lavapipe, llvmpipe, WARP) is
/// no faster than the CPU view and a virtual machine's may not show a window: both are passed
/// over unless `--renderer gpu` asks for them. A GPU window that doesn't open is opened again
/// on OpenGL; one that fails after it opened (`ssh -X` to a machine with a GPU, say) can stop the
/// app, whose error names `--renderer cpu`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Start {
    /// The CPU, with the window on a hardware GPU where there is one, so that the view can be
    /// switched to it (no `--renderer`).
    #[default]
    Ready,
    /// The CPU rasteriser, the window on OpenGL: no switch.
    Cpu,
    /// The GPU; the CPU, with a warning, where there is none.
    Gpu,
    /// The GPU where there is a hardware one, else the CPU (a software GPU is no faster).
    Auto,
}

impl Start {
    /// `cpu`, `gpu` or `auto`.
    pub fn parse(name: &str) -> Option<Start> {
        match name {
            "cpu" => Some(Start::Cpu),
            "gpu" => Some(Start::Gpu),
            "auto" => Some(Start::Auto),
            _ => None,
        }
    }
}

/// What makes the window's app: takes the app from `slot` (once), attaches the GPU view where
/// the window runs on a device it can use, and sets what draws the 3D view at the start.
fn creator(
    slot: std::sync::Arc<std::sync::Mutex<Option<Desktop>>>,
    start: Start,
    probe: Result<(), String>,
) -> eframe::AppCreator<'static> {
    Box::new(move |cc: &eframe::CreationContext<'_>| {
        let Some(mut desktop) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
            return Err("the app was already started".into());
        };
        let viewport = &mut desktop.app.viewport;
        let attached = match (&cc.wgpu_render_state, &probe) {
            // `auto` and the default take a hardware device only: the window's own may differ from
            // the probe's.
            (Some(state), _)
                if matches!(start, Start::Auto | Start::Ready) && passed_over(state.adapter.get_info().device_type) =>
            {
                Err(PASSED_OVER.to_string())
            }
            (Some(state), _) => viewport.attach_gpu(state, &cc.egui_ctx),
            (None, Err(e)) => Err(e.clone()),
            (None, Ok(())) => Err("the window doesn't run on a GPU device".to_string()),
        };
        if let Err(e) = &attached {
            viewport.no_gpu = Some(e.clone());
        }
        viewport.renderer = match start {
            Start::Gpu | Start::Auto if viewport.has_gpu() => Renderer::Gpu,
            _ => Renderer::Cpu,
        };
        if let (Start::Gpu, Err(e)) = (start, &attached) {
            eprintln!("umcf-gui: --renderer gpu: the 3D view is drawn on the CPU: {e}");
        }
        Ok(Box::new(desktop) as Box<dyn eframe::App>)
    })
}

/// Why the window doesn't run on a GPU device it found: a software or virtual one.
const PASSED_OVER: &str = "the GPU device is a software or virtual one";

/// Whether the window runs on a GPU device, for `start`, with `probe` (as [`gpu_probe`]: whether
/// the device found is passed over, or why there is none); why not. The probe runs only when it
/// is needed.
fn window_on_gpu(start: Start, probe: impl FnOnce() -> Result<bool, String>) -> Result<(), String> {
    match start {
        Start::Cpu => Err("started with --renderer cpu: the window runs on OpenGL".to_string()),
        Start::Gpu => probe().map(|_| ()),
        Start::Auto | Start::Ready => match probe() {
            Ok(false) => Ok(()),
            Ok(true) => Err(PASSED_OVER.to_string()),
            Err(e) => Err(e),
        },
    }
}

/// Whether a device of `kind` is passed over unless the GPU is asked for: a software one (no
/// faster than the CPU view) or a virtual machine's (whose window may not show).
fn passed_over(kind: eframe::egui_wgpu::wgpu::DeviceType) -> bool {
    use eframe::egui_wgpu::wgpu::DeviceType;
    matches!(kind, DeviceType::Cpu | DeviceType::VirtualGpu)
}

/// Whether this machine has a GPU device the 3D view can be drawn on (wgpu with storage buffers
/// in vertex shaders), and whether it is passed over (software: lavapipe, llvmpipe, WARP; or a
/// virtual machine's); why not.
fn gpu_probe() -> Result<bool, String> {
    use eframe::egui_wgpu::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let options =
        wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() };
    let adapter = pollster::block_on(instance.request_adapter(&options)).map_err(|e| format!("no GPU device: {e}"))?;
    gpu::usable(&adapter)?;
    Ok(passed_over(adapter.get_info().device_type))
}

/// The wgpu set-up of the window: egui's, with the device given all the adapter allows (the 3D
/// view's buffers can be large).
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, wgpu};
    let setup = WgpuSetupCreateNew {
        device_descriptor: std::sync::Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
            label: Some("umcfGui device"),
            required_limits: adapter.limits(),
            ..Default::default()
        }),
        // The adapter the probe looked at (two GPUs: the faster).
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..WgpuSetupCreateNew::without_display_handle()
    };
    WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() }
}

/// The view's guides' and background's tooltips.
const BRUSH_TIP: &str = "Paint over what to select: what is seen under the stroke (Shift adds, Ctrl takes away; [ and ] change its size); a right or middle drag moves the view";
const CUT_TIP: &str = "Cut a region's mesh with a plane: the cells it crosses, whole (its whole mesh is read once)";
const GRID_TIP: &str = "The grid on the ground (z = 0): its spacing follows the zoom; the scene hides it";
const ORIGIN_TIP: &str = "The origin: its xy, xz and yz planes and its x, y and z axes, sized with the view";
const DARK_TIP: &str = "A dark 3D view (else a light one), whatever the window's theme";

/// The desktop window around an [`App`].
struct Desktop {
    app: App,
    screenshot: Option<String>,
    frames: u32,
    /// When the screenshot was asked of the window.
    asked: Option<std::time::Instant>,
    /// Set once the screenshot is written (or failed): the window closes.
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Why the screenshot was not written, for [`run`] to return.
    failed: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.app.show(ui);
        // The folder dialog, over this window (modal to it), once the frame is drawn.
        // Not while work runs or waits: the folder opens once it is done.
        if !self.app.working()
            && std::mem::take(&mut self.app.folder_requested)
            && let Some(pick) = self.app.pick_folder
        {
            let start = self.app.session.location().map(String::from);
            let picked = pick(start.as_deref(), frame);
            self.app.folder_picked(picked);
            ui.ctx().request_repaint();
        }
        // A save (work that can't be cancelled) runs to its end: the window stays till then.
        if self.app.writing() && ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.done.load(std::sync::atomic::Ordering::SeqCst) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let Some(path) = &self.screenshot else { return };
        // The view as it is once what it shows (and the case's fields) has been read.
        if self.app.working() {
            ui.ctx().request_repaint();
            return;
        }
        self.frames += 1;
        ui.ctx().request_repaint();
        // A screenshot asked for and never written (a window format the capture doesn't take):
        // fail after half a minute, not wait.
        if self.asked.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(30)) {
            let mut f = self.failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            f.get_or_insert_with(|| format!("the screenshot {path} wasn't taken: the window's capture gave nothing"));
            self.done.store(true, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        // A few frames for the panels to settle and the view to draw.
        if self.frames == 5 {
            #[allow(clippy::disallowed_methods)]
            let now = std::time::Instant::now();
            self.asked = Some(now);
            let (path, done, failed, ctx) = (path.clone(), self.done.clone(), self.failed.clone(), ui.ctx().clone());
            let callback = egui::ScreenshotCallback::new(move |image| {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                let png = umcf_render::png::encode(&umcf_render::raster::Image {
                    width: u32::try_from(w).unwrap_or(0),
                    height: u32::try_from(h).unwrap_or(0),
                    rgba,
                });
                if let Err(e) = std::fs::write(&path, png) {
                    let mut f = failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    *f = Some(format!("writing the screenshot {path}: {e}"));
                }
                done.store(true, std::sync::atomic::Ordering::SeqCst);
                ctx.request_repaint();
            });
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
        }
    }
}

impl App {
    /// What the window does once the case named on the command line is open: the card or
    /// command of `--open` (`open`), then, ahead of it, the view's scene (of what `--show` asked
    /// for) and the field files when they were listed only. The command is asked for first, as
    /// other work would drop a scene already waiting.
    pub(crate) fn start_up(&mut self, open: Option<&str>) -> Result<(), String> {
        // The cut card, which is no command's: with `found`, showing what Check mesh found; with
        // `layers`, the layers at a patch; with `near`, the cells near a surface.
        if let Some(which @ ("cut" | "found" | "layers" | "near")) = open {
            if let Some(tab) = self
                .workspace
                .tabs()
                .iter()
                .position(|t| t.groups.iter().flat_map(|g| g.items).any(|i| matches!(i, ribbon::Item::Cut)))
            {
                self.tab = tab;
            }
            self.toggle_cut();
            if self.cut.is_none() {
                return Err(if self.session.case().is_none() {
                    format!("--open {which}: no case is open")
                } else {
                    format!("--open {which}: no region of the case has a mesh")
                });
            }
            if which == "found" {
                self.show_found();
                self.fit_findings();
            }
            if which == "layers" {
                self.show_layers();
            }
            if which == "near" {
                self.show_near();
                self.fit_findings();
            }
        } else if let Some(id) = open {
            let spec = Session::command(id).ok_or_else(|| format!("--open: there is no command `{id}`"))?;
            if let Some(tab) = ribbon::tab_of(self.workspace, spec.id) {
                self.tab = tab;
            }
            self.invoke(spec.id);
        }
        self.after_open();
        Ok(())
    }
}

/// Opens the window and runs until it is closed.
pub fn run(options: Options) -> Result<(), String> {
    let mut startup = Vec::new();
    if let Some(case) = &options.case {
        startup.push(("case.open".to_string(), json!({ "path": case })));
    }
    startup.extend(options.commands.iter().cloned());
    let mut app = App::new(Session::new());
    app.pick_folder = options.pick_folder;
    if options.dark {
        app.tokens = Tokens::dark();
    }
    // A case named on the command line opens first, its fields read as the window shows; with
    // commands to run, they are read at open, for the commands to act on.
    let later = options.commands.is_empty();
    for (id, mut params) in startup {
        if later
            && id == "case.open"
            && let Some(o) = params.as_object_mut()
            && !o.contains_key("fields")
        {
            o.insert("fields".into(), json!("later"));
        }
        if let Err(e) = app.run(&id, params) {
            // A screenshot of a window whose case or commands failed would not show what was
            // asked for: stop with the error instead.
            if options.screenshot.is_some() {
                return Err(format!("{id}: {e}"));
            }
            eprintln!("umcf-gui: {id}: {e}");
        }
    }
    // Surfaces that couldn't be listed would be missing from the picture.
    if options.screenshot.is_some()
        && let Some(e) = &app.listing_error
    {
        return Err(e.clone());
    }
    if !options.show.is_empty() {
        let unknown = app.show_only(&options.show);
        if options.screenshot.is_some()
            && let Some(n) = unknown.first()
        {
            return Err(format!("--show: no region with a mesh or surface named `{n}`"));
        }
        for n in unknown {
            eprintln!("umcf-gui: --show: no region with a mesh or surface named `{n}`");
        }
    }
    app.start_up(options.open.as_deref())?;
    let (w, h) = options.size.unwrap_or((1440.0, 900.0));
    // The window on a GPU device where there is one (see [`Start`]); without --renderer and with
    // `auto`, a software or virtual one is passed over.
    let probe = window_on_gpu(options.renderer, gpu_probe);
    let native = |renderer| eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([w, h]).with_title("umcfGui"),
        renderer,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    let failed: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
    let wanted = options.screenshot.clone();
    let done: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
    // The app waits here until the window is made: a GPU window that doesn't open leaves it,
    // and it is started again on OpenGL.
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(Desktop {
        app,
        screenshot: options.screenshot,
        frames: 0,
        asked: None,
        done: done.clone(),
        failed: failed.clone(),
    })));
    let start = options.renderer;
    let on_gpu = probe.is_ok();
    let renderer = if on_gpu { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow };
    let mut result = eframe::run_native("umcfGui", native(renderer), creator(slot.clone(), start, probe));
    // Whether the last run was on the GPU (the hint below is for that one only).
    let mut last_on_gpu = on_gpu;
    if on_gpu && let Err(e) = &result {
        let waiting = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some();
        if waiting {
            let why = format!("the window didn't open on the GPU device ({e}): it runs on OpenGL");
            eprintln!("umcf-gui: {why}");
            last_on_gpu = false;
            result = eframe::run_native("umcfGui", native(eframe::Renderer::Glow), creator(slot, start, Err(why)));
        }
    }
    result.map_err(|e| {
        if last_on_gpu {
            format!("{e} (the window ran on the GPU device: start with --renderer cpu to run it on OpenGL)")
        } else {
            e.to_string()
        }
    })?;
    if let Some(e) = failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
        return Err(e);
    }
    // Closed before the screenshot was taken (the window was closed by hand).
    match wanted {
        Some(path) if !done.load(std::sync::atomic::Ordering::SeqCst) => {
            Err(format!("the window closed before the screenshot was written to {path}"))
        }
        _ => Ok(()),
    }
}
