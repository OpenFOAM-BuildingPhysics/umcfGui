//! The interface driven without a window (egui_kittest): clicks and keys go through the same
//! code as on the desktop, and every change is a command in the session.

use eframe::egui::{self, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use umcf_core::{CaseFs, CaseWrite, FsError, MemFs};
use umcf_engine::{Sel, Session};
use umcf_model::OpenOptions;
use umcf_render::camera::View;

use crate::App;

const HEADER: &str = "FoamFile\n{\n    version 2.0;\n    format ascii;\n    class dictionary;\n}\n";

fn h(class: &str) -> String {
    format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n")
}

/// A single-region case whose mesh is a unit cube: patches ground (bottom), sky (top), walls.
fn case() -> MemFs {
    let mut fs = MemFs::new();
    fs.insert(
        "system/controlDict",
        format!(
            "{HEADER}application urbanMicroclimateFoam;\nstartTime 0;\nendTime 3600;\ndeltaT 60;\nwriteInterval 600;\n"
        ),
    );
    fs.insert("system/fvSolution", format!("{HEADER}PIMPLE\n{{\n    nOuterCorrectors 1;\n}}\n"));
    let pts: String = (0..8)
        .map(|i| {
            let b = |k: usize| if (i >> k) & 1 == 1 { 1 } else { 0 };
            format!("({} {} {})\n", b(0), b(1), b(2))
        })
        .collect();
    let d = "constant/polyMesh";
    fs.insert(&format!("{d}/points"), format!("{}8\n(\n{pts})\n", h("vectorField")));
    fs.insert(
        &format!("{d}/faces"),
        format!("{}6\n(\n4(0 2 3 1)\n4(4 5 7 6)\n4(0 1 5 4)\n4(2 6 7 3)\n4(0 4 6 2)\n4(1 3 7 5)\n)\n", h("faceList")),
    );
    fs.insert(&format!("{d}/owner"), format!("{}6\n(\n0 0 0 0 0 0\n)\n", h("labelList")));
    fs.insert(&format!("{d}/neighbour"), format!("{}0\n(\n)\n", h("labelList")));
    let patch = |n: &str, count: u32, start: u32| {
        format!("    {n}\n    {{\n        type wall;\n        nFaces {count};\n        startFace {start};\n    }}\n")
    };
    fs.insert(
        &format!("{d}/boundary"),
        format!(
            "{}3\n(\n{}{}{})\n",
            h("polyBoundaryMesh"),
            patch("ground", 1, 0),
            patch("sky", 1, 1),
            patch("walls", 4, 2)
        ),
    );
    fs.insert(
        "0/T",
        format!(
            "{}internalField uniform 300;\nboundaryField\n{{\n    sky {{ type fixedValue; value uniform 290; }}\n    \"(ground|walls)\" {{ type zeroGradient; }}\n}}\n",
            h("volScalarField")
        ),
    );
    fs
}

fn app() -> App {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(case()), "mem", OpenOptions::default(), false).unwrap();
    App::new(s)
}

fn harness(app: App) -> Harness<'static, App> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run();
    h
}

fn click_at(h: &mut Harness<'_, App>, pos: Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
    }
    h.run();
}

fn tree_item<'a>(h: &'a Harness<'_, App>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(egui::Role::TreeItem, label)
}

#[test]
fn the_ribbon_comes_from_the_registry() {
    let mut h = harness(app());
    for tab in ["Case", "Geometry", "Mesh", "Boundary conditions", "View"] {
        h.get_by_role_and_label(egui::Role::Tab, tab);
    }
    // A command without parameters runs from its button.
    h.get_by_label("Check").click();
    h.run();
    assert_eq!(h.state().log.last().unwrap().command, "case.check");
    // Another tab, another ribbon.
    h.get_by_role_and_label(egui::Role::Tab, "Mesh").click();
    h.run();
    h.get_by_label("Check mesh").click();
    h.run();
    assert_eq!(h.state().log.last().unwrap().command, "mesh.check");
    // Undo is disabled until there is something to undo.
    assert!(h.get_by_label("Undo").accesskit_node().is_disabled());
}

#[test]
fn the_workspace_switcher_offers_what_exists() {
    let mut h = harness(app());
    h.get_by_label("Workspace").click();
    h.run();
    assert!(h.state().workspace_menu);
    // Run and Results are planned: they can't be chosen yet.
    assert!(h.get_by_role_and_label(egui::Role::Button, "Run").accesskit_node().is_disabled());
    h.get_by_role_and_label(egui::Role::Button, "Setup").click();
    h.run();
    assert!(!h.state().workspace_menu);
}

#[test]
fn the_browser_selects_patches() {
    let mut h = harness(app());
    // The single region is open: its patches are listed.
    tree_item(&h, "(single region)");
    tree_item(&h, "sky").click();
    h.run();
    assert_eq!(h.state().session().selection(), [Sel::Patch { region: String::new(), patch: "sky".into() }]);
    // Ctrl+click adds, Ctrl+click on a selected one takes it out.
    tree_item(&h, "walls").click_modifiers(Modifiers::COMMAND);
    h.run();
    assert_eq!(h.state().session().selection().len(), 2);
    tree_item(&h, "sky").click_modifiers(Modifiers::COMMAND);
    h.run();
    assert_eq!(h.state().session().selection(), [Sel::Patch { region: String::new(), patch: "walls".into() }]);
    // The properties card shows the selection.
    h.get_by_label("Properties");
    h.get_by_label("patch · wall · 4 faces");
    // Each change was a command.
    let commands: Vec<&str> = h.state().log.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["select.set", "select.set", "select.set"]);
}

#[test]
fn a_click_in_the_view_picks_through_the_engine() {
    let mut h = harness(app());
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    let rect = h.state().viewport.rect.unwrap();
    click_at(&mut h, rect.center(), Modifiers::NONE);
    assert_eq!(h.state().session().selection(), [Sel::Patch { region: String::new(), patch: "sky".into() }]);
    assert_eq!(h.state().log.last().unwrap().command, "select.pick");
    // The status bar says what is under the cursor.
    assert_eq!(h.state().viewport.hover.as_deref(), Some("patch sky"));
    // A click beside the cube clears the selection.
    click_at(&mut h, rect.min + vec2(5.0, rect.height() - 5.0), Modifiers::NONE);
    assert!(h.state().session().selection().is_empty());
    // Dragging turns the view.
    let before = h.state().viewport.camera.unwrap().azimuth;
    let c = rect.center();
    h.event(egui::Event::PointerButton {
        pos: c,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    for k in 1..=5 {
        h.event(egui::Event::PointerMoved(c + vec2(10.0 * k as f32, 0.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton {
        pos: c + vec2(50.0, 0.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    assert!(h.state().viewport.camera.unwrap().azimuth < before - 5.0);
    // A drag is not a click: nothing was picked.
    assert!(h.state().session().selection().is_empty());
}

#[test]
fn the_search_lists_commands_and_runs_them() {
    let mut h = harness(app());
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::P);
    h.run();
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").type_text("mesh");
    h.run();
    assert!(h.query_by_label("Set run controls").is_none(), "filtered out");
    h.get_by_label("Check mesh").click();
    h.run();
    assert_eq!(h.state().log.last().unwrap().command, "mesh.check");
    assert!(h.state().search.is_empty() && !h.state().search_open);
}

#[test]
fn the_axis_triad_turns_the_view() {
    let mut h = harness(app());
    h.get_by_label("+Z view").click();
    h.run();
    assert_eq!(h.state().viewport.camera.unwrap().elevation, 90.0);
    h.get_by_label("-Y view").click();
    h.run();
    let c = h.state().viewport.camera.unwrap();
    assert_eq!((c.azimuth, c.elevation), (0.0, 0.0));
}

#[test]
fn commands_with_parameters_open_a_dialog_and_undo_has_its_shortcut() {
    let mut h = harness(app());
    h.state_mut().invoke("control.set");
    h.run();
    assert!(h.state().dialog.is_some());
    // Not JSON: the dialog stays and says why.
    h.state_mut().dialog.as_mut().unwrap().params = "{endTime: 1}".into();
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().dialog.as_ref().unwrap().error.as_deref().unwrap().starts_with("not JSON"));
    h.state_mut().dialog.as_mut().unwrap().params = r#"{"endTime": 7200}"#.into();
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().dialog.is_none());
    let end = |h: &Harness<'_, App>| {
        h.state().session().case().unwrap().dict(umcf_model::CONTROL_DICT).unwrap().value(&["endTime"])
    };
    assert_eq!(end(&h).as_deref(), Some("7200"));
    assert_eq!(h.state().session().case().unwrap().changed(), ["system/controlDict"]);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert_eq!(end(&h).as_deref(), Some("3600"));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z);
    h.run();
    assert_eq!(end(&h).as_deref(), Some("7200"));
    // The history strip: one chip per edit; a click on it goes back there.
    h.get_by_label("Undo one").click();
    h.run();
    assert_eq!(end(&h).as_deref(), Some("3600"));
    h.get_by_label("Edit 1").click();
    h.run();
    assert_eq!(end(&h).as_deref(), Some("7200"));
    let (done, undone) = h.state().session().history();
    assert_eq!((done.len(), undone.len()), (1, 0));
    assert_eq!((done[0].command, done[0].detail.as_str()), ("control.set", "endTime 7200"));
}

#[test]
fn without_a_case_the_shell_says_what_to_do() {
    let mut h = harness(App::new(Session::with_backup(".umcfgui/backup/test")));
    h.get_by_label("No case open.");
    for save in h.get_all_by_role_and_label(egui::Role::Button, "Save") {
        assert!(save.accesskit_node().is_disabled());
    }
    h.get_by_label("Open a case…").click();
    h.run();
    assert_eq!(h.state().dialog.as_ref().map(|d| d.id), Some("case.open"));
}

#[test]
fn registry_shortcuts_parse() {
    use crate::shortcut;
    let s = shortcut("Ctrl+Shift+Z").unwrap();
    assert_eq!((s.modifiers, s.logical_key), (Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z));
    assert_eq!(shortcut("F7").unwrap().logical_key, egui::Key::F7);
    assert!(shortcut("Ctrl+Nope").is_none());
}

/// A surface an undo takes away leaves the view, which still draws what is left.
#[test]
fn undoing_an_import_takes_its_surface_out_of_the_view() {
    let dir = std::env::temp_dir().join(format!("umcf-ui-{}-import", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tri.stl");
    std::fs::write(
        &file,
        "solid lid\nfacet normal 0 0 1\nouter loop\nvertex 0 0 2\nvertex 1 0 2\nvertex 0 1 2\nendloop\nendfacet\nendsolid lid\n",
    )
    .unwrap();
    let mut h = harness(app());
    h.state_mut().run("geometry.import", json!({ "path": file.to_str().unwrap() })).unwrap();
    h.state_mut().show_only(&[String::new(), "tri.stl".to_string()]);
    h.run();
    assert_eq!(h.state().shown_surfaces, ["constant/triSurface/tri.stl"]);
    tree_item(&h, "lid");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert!(h.state().shown_surfaces.is_empty());
    assert_eq!(h.state().shown_regions, [String::new()]);
    // The mesh is still drawn and picked.
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    let rect = h.state().viewport.rect.unwrap();
    click_at(&mut h, rect.center(), Modifiers::NONE);
    assert_eq!(h.state().session().selection().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Many edits: the strip shows the ones next to the marker and counts the others.
#[test]
fn the_history_strip_shows_what_fits() {
    let mut h = harness(app());
    for k in 0..60 {
        h.state_mut().run("control.set", json!({ "endTime": 3600 + k })).unwrap();
    }
    h.state_mut().go_to(40);
    h.run();
    assert_eq!(h.state().session().history().0.len(), 40);
    let shown = h.get_all_by_label_contains("Edit ").count();
    assert!((4..60).contains(&shown), "{shown}");
    let count = |l: &str| l.strip_prefix('+').is_some_and(|d| d.parse::<usize>().is_ok());
    let shown_counts = h
        .root()
        .children_recursive()
        .filter(|n| {
            let a = n.accesskit_node();
            a.label().is_some_and(|l| count(&l)) || a.value().is_some_and(|v| count(&v))
        })
        .count();
    assert!(shown_counts >= 1, "a +k count is shown");
    // The chip next to the marker on the redo side is there and goes one step on.
    h.get_by_label("Edit 41").click();
    h.run();
    assert_eq!(h.state().session().history().0.len(), 41);
}

#[test]
fn every_ribbon_command_is_in_the_registry() {
    for w in crate::ribbon::Workspace::ALL {
        for tab in w.tabs() {
            for g in tab.groups {
                for item in g.items {
                    if let crate::ribbon::Item::Command(id, _, _) = item {
                        assert!(Session::command(id).is_some(), "the ribbon places `{id}`, which isn't a command");
                    }
                }
            }
        }
    }
}

/// A case on [`MemFs`] that counts the reads of its surfaces and can be made to panic on them.
struct Watched {
    fs: MemFs,
    reads: Arc<AtomicUsize>,
    panics: Arc<AtomicBool>,
    /// Listing `constant/triSurface` fails (an unreadable directory).
    unreadable: Arc<AtomicBool>,
}

impl Watched {
    fn surface(&self, rel: &str) {
        if rel.starts_with("constant/triSurface") {
            assert!(!self.panics.load(Ordering::SeqCst), "a bug while reading {rel}");
        }
    }
}

impl CaseFs for Watched {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        self.surface(rel);
        if rel.ends_with(".stl") {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        self.fs.read(rel, limit)
    }
    fn is_file(&self, rel: &str) -> bool {
        self.fs.is_file(rel)
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.fs.is_dir(rel)
    }
    fn list(&self, rel: &str) -> Result<Vec<String>, FsError> {
        self.surface(rel);
        if rel.starts_with("constant/triSurface") && self.unreadable.load(Ordering::SeqCst) {
            return Err(FsError::BadPath(rel.to_string()));
        }
        self.fs.list(rel)
    }
}

impl CaseWrite for Watched {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.fs.write(rel, bytes)
    }
}

/// The case with a surface `bad.stl` that says it has 1000 triangles and has none.
fn watched() -> (App, Arc<AtomicUsize>, Arc<AtomicBool>) {
    let mut fs = case();
    let mut bad = vec![0u8; 80];
    bad.extend(1000u32.to_le_bytes());
    fs.insert("constant/triSurface/bad.stl", bad);
    let (reads, panics) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let fs = Watched { fs, reads: reads.clone(), panics: panics.clone(), unreadable: Default::default() };
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    (App::new(s), reads, panics)
}

#[test]
fn a_scene_that_fails_is_tried_once_per_change() {
    let (mut app, reads, _) = watched();
    assert_eq!(app.surfaces, ["constant/triSurface/bad.stl"]);
    assert!(app.show_only(&["bad.stl".to_string()]).is_empty());
    let mut h = harness(app);
    let revision = h.state().revision;
    let error = h.state().scene_error.clone();
    assert!(matches!(&error, Some((r, e)) if *r == revision && e.contains("bad.stl")), "{error:?}");
    let tried = reads.load(Ordering::SeqCst);
    assert!(tried > 0);
    for _ in 0..3 {
        h.run();
    }
    assert_eq!(reads.load(Ordering::SeqCst), tried, "the failed scene was built again");
    assert_eq!(h.state().scene_error, error);
    // A change to what is shown tries again.
    h.state_mut().show_only(&["walls".to_string()]);
    h.run();
    assert!(h.state().scene_error.as_ref().is_none_or(|(r, _)| *r != h.state().revision));
}

#[test]
fn a_bug_while_building_the_scene_is_an_error_not_a_crash() {
    let (app, _, panics) = watched();
    let mut s = app.session;
    panics.store(true, Ordering::SeqCst);
    let err = s.scene(&json!({ "surfaces": ["constant/triSurface/bad.stl"] })).err();
    assert!(matches!(err, Some(umcf_engine::Error::Panicked { .. })), "{err:?}");
    assert!(matches!(s.surfaces(), Err(umcf_engine::Error::Panicked { .. })));
    panics.store(false, Ordering::SeqCst);
    assert_eq!(s.surfaces().unwrap(), ["constant/triSurface/bad.stl"]);
}

#[test]
fn a_surface_directory_that_cant_be_listed_is_an_error() {
    let unreadable = Arc::new(AtomicBool::new(false));
    let mut fs = case();
    fs.insert("constant/triSurface/a.stl", vec![0u8; 84]);
    let fs = Watched { fs, reads: Default::default(), panics: Default::default(), unreadable: unreadable.clone() };
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    unreadable.store(true, Ordering::SeqCst);
    let err = s.surfaces().err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("can't be listed"), "{err}");
    assert!(s.scene(&json!({ "regions": [""] })).is_ok());
}

#[test]
fn a_bug_while_listing_surfaces_keeps_what_is_shown() {
    let (mut app, _, panics) = watched();
    assert!(app.show_only(&["bad.stl".to_string()]).is_empty());
    panics.store(true, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    assert_eq!(app.surfaces, ["constant/triSurface/bad.stl"]);
    assert_eq!(app.shown_surfaces, ["constant/triSurface/bad.stl"]);
    assert!(app.log.last().is_some_and(|e| !e.ok && e.command == "surfaces"));
    assert!(app.results_open);
}

#[test]
fn surfaces_that_could_not_be_listed_at_opening_show_once_they_can() {
    // No mesh: opening shows every surface.
    let mut fs = MemFs::new();
    fs.insert("system/controlDict", format!("{HEADER}application urbanMicroclimateFoam;\nendTime 3600;\n"));
    let mut bad = vec![0u8; 80];
    bad.extend(0u32.to_le_bytes());
    fs.insert("constant/triSurface/a.stl", bad);
    let (reads, panics) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(
        Box::new(Watched { fs, reads, panics: panics.clone(), unreadable: Default::default() }),
        "mem",
        OpenOptions::default(),
        false,
    )
    .unwrap();
    panics.store(true, Ordering::SeqCst);
    let mut app = App::new(s);
    assert!(app.shown_regions.is_empty() && app.shown_surfaces.is_empty());
    assert!(app.unlisted);
    panics.store(false, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    assert!(!app.unlisted);
    assert_eq!(app.shown_surfaces, ["constant/triSurface/a.stl"]);
}

#[test]
fn an_empty_view_chosen_before_the_surfaces_list_stays_empty() {
    let mut fs = MemFs::new();
    fs.insert("system/controlDict", format!("{HEADER}application urbanMicroclimateFoam;\nendTime 3600;\n"));
    fs.insert("constant/triSurface/a.stl", vec![0u8; 84]);
    let unreadable = Arc::new(AtomicBool::new(false));
    let fs = Watched { fs, reads: Default::default(), panics: Default::default(), unreadable: unreadable.clone() };
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    unreadable.store(true, Ordering::SeqCst);
    let mut app = App::new(s);
    assert!(app.unlisted && app.listing_error.is_some());
    assert!(app.show_only(&[]).is_empty());
    unreadable.store(false, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    assert!(app.listing_error.is_none());
    assert_eq!(app.surfaces, ["constant/triSurface/a.stl"]);
    assert!(app.shown_surfaces.is_empty());
    // A listing that fails after one that worked is the last word too.
    unreadable.store(true, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 3600 })).unwrap();
    assert!(!app.unlisted && app.listing_error.is_some());
    assert_eq!(app.surfaces, ["constant/triSurface/a.stl"]);
}

#[test]
fn a_surface_asked_for_before_the_surfaces_list_shows_once_they_do() {
    let mut fs = case();
    fs.insert("constant/triSurface/a.stl", vec![0u8; 84]);
    let unreadable = Arc::new(AtomicBool::new(false));
    let fs = Watched { fs, reads: Default::default(), panics: Default::default(), unreadable: unreadable.clone() };
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    unreadable.store(true, Ordering::SeqCst);
    let mut app = App::new(s);
    // The case's one region is named "".
    assert_eq!(app.show_only(&["".to_string(), "a.stl".to_string()]), ["a.stl"]);
    unreadable.store(false, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    assert_eq!(app.shown_surfaces, ["constant/triSurface/a.stl"]);
    assert_eq!(app.shown_regions, [""]);
}

fn type_into(h: &mut Harness<'_, App>, role: egui::Role, label: &str, text: &str) {
    h.get_by_role_and_label(role, label).click();
    h.run();
    h.get_by_role_and_label(role, label).type_text(text);
    h.run();
}

#[test]
fn a_condition_is_set_on_the_picked_patches() {
    let mut app = app();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "ground" }, { "region": "", "patch": "sky" }] }))
        .unwrap();
    let mut h = harness(app);
    h.get_by_role_and_label(egui::Role::Tab, "Boundary conditions").click();
    h.run();
    h.get_by_label("Set condition").click();
    h.run();
    // The card starts from the first patch's condition: ground's comes from a pattern.
    assert_eq!(h.state().bc.as_ref().map(|f| (f.field.as_str(), f.kind.as_str())), Some(("T", "")));
    h.get_by_label("2 patches: ground, sky");
    type_into(&mut h, egui::Role::TextInput, "Type", "inletOut");
    h.run();
    // The suggestions follow what is typed.
    h.get_by_label("inletOutlet").click();
    h.run();
    type_into(&mut h, egui::Role::TextInput, "Value", "uniform 280");
    type_into(
        &mut h,
        egui::Role::MultilineTextInput,
        "Other entries, one per line: key value",
        "inletValue uniform 281",
    );
    h.run();
    h.get_by_label("Set").click();
    h.run();
    assert!(h.state().bc.is_none(), "{:?}", h.state().bc);
    let case = h.state().session().case().unwrap();
    let t = case.region("").unwrap().field("T").unwrap();
    let bcs: Vec<_> =
        t.bcs.as_ref().unwrap().iter().map(|b| (b.patch.as_str(), b.kind.as_deref(), b.value.as_deref())).collect();
    assert!(bcs.contains(&("ground", Some("inletOutlet"), Some("uniform 280"))), "{bcs:?}");
    assert!(bcs.contains(&("sky", Some("inletOutlet"), Some("uniform 280"))), "{bcs:?}");
    assert_eq!(h.state().log.last().unwrap().command, "bc.set");
    // An error keeps the card open and says why.
    h.get_by_label("Set condition").click();
    h.run();
    h.state_mut().bc.as_mut().unwrap().kind = "bad type".to_string();
    h.run();
    h.get_by_label("Set").click();
    h.run();
    assert!(
        h.state()
            .bc
            .as_ref()
            .and_then(|f| f.error.as_ref())
            .is_some_and(|e| e.contains("not a boundary condition type"))
    );
}

#[test]
fn the_condition_card_reads_its_entries() {
    let form = crate::bc::BcForm {
        field: "U".into(),
        kind: " fixedValue ".into(),
        value: "uniform (1 0 0)".into(),
        more: "inletValue uniform (0 0 0);\n\n  refValue   uniform (1 1 1)\n".into(),
        ..Default::default()
    };
    assert_eq!(
        form.params().unwrap(),
        json!({ "field": "U", "type": "fixedValue", "value": "uniform (1 0 0)", "entries": { "inletValue": "uniform (0 0 0)", "refValue": "uniform (1 1 1)" } })
    );
    // A value as filled in from the patch is a default: a new entry gets it, a patch keeping its
    // entry keeps its own.
    let kept = crate::bc::BcForm { filled_value: "uniform (1 0 0)".into(), ..form.clone() };
    assert!(kept.params().unwrap().get("value").is_none());
    assert_eq!(kept.params().unwrap()["defaults"], json!({ "value": "uniform (1 0 0)" }));
    let twice = crate::bc::BcForm { more: "value uniform (0 0 1)".into(), ..form.clone() };
    assert!(twice.params().unwrap_err().contains("its own field"));
    let bad = crate::bc::BcForm { more: "lonely".into(), ..form.clone() };
    assert!(bad.params().unwrap_err().contains("key value"));
    assert!(crate::bc::BcForm { kind: " ".into(), ..form }.params().is_err());
}

#[test]
fn a_patch_is_edited_from_its_card() {
    let mut app = app();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    let mut h = harness(app);
    h.get_by_role_and_label(egui::Role::Tab, "Mesh").click();
    h.run();
    h.get_by_label("Edit patch").click();
    h.run();
    // Filled from the patch.
    let f = h.state().patch_form.clone().unwrap();
    assert_eq!((f.name.as_str(), f.kind.as_str()), ("sky", "wall"));
    h.get_by_label("symmetry").click();
    h.run();
    type_into(&mut h, egui::Role::TextInput, "Name", "Top");
    h.get_by_label("Set").click();
    h.run();
    assert!(h.state().patch_form.is_none(), "{:?}", h.state().patch_form);
    let case = h.state().session().case().unwrap();
    let p = case.region("").unwrap().patches.as_ref().unwrap().iter().find(|p| p.name == "skyTop").cloned().unwrap();
    assert_eq!(p.kind, "symmetry");
    assert_eq!(h.state().log.last().unwrap().command, "patch.set");
}

#[test]
fn the_patch_card_sends_only_what_changed() {
    let form = crate::patch::PatchForm {
        name: "top".into(),
        kind: "wall".into(),
        groups: "wall  sky".into(),
        filled: ("top".into(), "wall".into(), "wall sky".into()),
        ..Default::default()
    };
    assert!(form.params(true).unwrap_err().contains("Nothing changed"));
    let typed = crate::patch::PatchForm { kind: "patch".into(), more: "neighbourRegion air".into(), ..form.clone() };
    assert_eq!(typed.params(true).unwrap(), json!({ "type": "patch", "entries": { "neighbourRegion": "air" } }));
    // A name is sent for one patch only.
    let renamed = crate::patch::PatchForm { name: "roof".into(), ..form.clone() };
    assert_eq!(renamed.params(true).unwrap(), json!({ "name": "roof" }));
    assert!(renamed.params(false).is_err());
    assert!(crate::patch::PatchForm { kind: " ".into(), ..form }.params(true).is_err());
}

#[test]
fn a_new_patch_is_prepared_from_its_card() {
    let mut app = app();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "walls" }] })).unwrap();
    let mut h = harness(app);
    h.get_by_role_and_label(egui::Role::Tab, "Mesh").click();
    h.run();
    h.get_by_label("New patch").click();
    h.run();
    // The card is laid out in its first frame and shown from the next.
    h.run();
    assert!(h.state().new_patch.is_some());
    // Without a name it says so, and stays open.
    h.get_by_label("Prepare").click();
    h.run();
    let f = h.state().new_patch.clone();
    assert!(
        f.as_ref().and_then(|f| f.error.as_deref()).is_some_and(|e| e.contains("name")),
        "{f:?} {:?}",
        h.state().log.last().map(|l| &l.command)
    );
    type_into(&mut h, egui::Role::TextInput, "Name", "west");
    if let Some(f) = &mut h.state_mut().new_patch {
        f.min = ["-1".into(), "-1".into(), "-1".into()];
        f.max = ["0.1".into(), "2".into(), "2".into()];
    }
    h.run();
    h.get_by_label("Prepare").click();
    h.run();
    assert!(h.state().new_patch.is_none(), "{:?}", h.state().new_patch);
    assert_eq!(h.state().log.last().unwrap().command, "patch.create");
    let case = h.state().session().case().unwrap();
    let set = String::from_utf8(case.put_bytes("constant/polyMesh/sets/west").unwrap().to_vec()).unwrap();
    assert!(set.ends_with("\n1\n(\n4\n)\n"), "{set}");
    assert!(case.dict("system/createPatchDict.umcf").is_some());
}

#[test]
fn the_new_patch_card_reads_its_fields() {
    use crate::newpatch::{NewPatchForm, Source};
    let form = NewPatchForm {
        name: " roof ".into(),
        kind: "wall".into(),
        groups: "buildings  roofs".into(),
        min: ["0".into(), "0".into(), "10".into()],
        max: ["5".into(), " 5 ".into(), "20".into()],
        ..Default::default()
    };
    assert_eq!(
        form.params(Some("air")).unwrap(),
        json!({ "name": "roof", "type": "wall", "groups": ["buildings", "roofs"], "region": "air", "box": { "min": [0.0, 0.0, 10.0], "max": [5.0, 5.0, 20.0] } })
    );
    assert!(
        NewPatchForm { min: ["a".into(), "0".into(), "0".into()], ..form.clone() }
            .params(None)
            .unwrap_err()
            .contains("From x")
    );
    let near =
        NewPatchForm { source: Source::Surface, file: "buildings.stl".into(), distance: "0.5".into(), ..form.clone() };
    assert_eq!(near.params(None).unwrap()["surface"], json!({ "file": "buildings.stl", "distance": 0.5 }));
    assert!(NewPatchForm { distance: String::new(), ..near.clone() }.params(None).is_err());
    assert!(NewPatchForm { file: " ".into(), ..near }.params(None).is_err());
}

#[test]
fn open_asks_for_the_folder_in_the_systems_dialog() {
    use crate::Picked;
    // A case on disk.
    let dir = std::env::temp_dir().join(format!("umcf-gui-{}-open", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("system")).unwrap();
    std::fs::write(
        dir.join("system/controlDict"),
        format!(
            "{HEADER}application urbanMicroclimateFoam;\nstartTime 0;\nendTime 3600;\ndeltaT 60;\nwriteInterval 600;\n"
        ),
    )
    .unwrap();
    let path = dir.to_string_lossy().into_owned();
    // With a dialog, Open (here from the ribbon) asks the window for it.
    let mut app = App::new(Session::new());
    app.pick_folder = Some(|_, _| Picked::Cancelled);
    let mut h = harness(app);
    h.get_by_label("Open").click();
    h.run();
    assert!(h.state().folder_requested && h.state().dialog.is_none());
    let mut app = App::new(Session::new());
    // Cancelled: nothing happens.
    app.folder_picked(Picked::Cancelled);
    assert!(app.session().location().is_none() && app.dialog.is_none());
    // Not a case: the card shows the path and why.
    app.folder_picked(Picked::Folder("/nonexistent/umcf-case".to_string()));
    let d = app.dialog.take().unwrap();
    assert_eq!(d.id, "case.open");
    assert!(d.params.contains("/nonexistent/umcf-case") && d.error.is_some(), "{d:?}");
    // No dialog on this system: the card asks for the path.
    app.folder_picked(Picked::Unavailable);
    let d = app.dialog.take().unwrap();
    assert!(d.error.unwrap().contains("type the case's path"));
    // A case: opened, and the card from the folder before goes.
    app.folder_picked(Picked::Folder("/nonexistent/umcf-case".to_string()));
    assert!(app.dialog.is_some());
    app.folder_picked(Picked::Folder(path.clone()));
    assert_eq!(app.session().location(), Some(path.as_str()));
    assert!(app.dialog.is_none());
    assert_eq!(app.log.last().unwrap().command, "case.open");
    // Without a dialog at all, Open is the card.
    let mut app = App::new(Session::new());
    app.invoke("case.open");
    assert_eq!(app.dialog.as_ref().map(|d| d.id), Some("case.open"));
    assert!(!app.folder_requested);
    let _ = std::fs::remove_dir_all(&dir);
}
