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

fn harness(mut app: App) -> Harness<'static, App> {
    // Jobs are waited for: a frame shows what they did, however slow the machine.
    app.job_wait = std::time::Duration::from_secs(120);
    let mut h = Harness::builder()
        .with_size(vec2(1200.0, 800.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run();
    h
}

#[test]
fn a_stroke_painted_with_the_brush_selects_the_faces_under_it() {
    let mut h = harness(app());
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    h.get_by_label("Brush").click();
    h.run();
    assert_eq!(h.state().viewport.drag, crate::viewport::Drag::Brush);
    let camera = h.state().viewport.camera.unwrap();
    let c = h.state().viewport.rect.unwrap().center();
    // Over the middle of the cube, picking faces: the sky's face; the view didn't turn.
    h.get_by_label("Pick faces").click();
    h.run();
    drag(&mut h, c - vec2(20.0, 0.0), c + vec2(20.0, 0.0), Modifiers::NONE);
    let last = h.state().log.last().unwrap();
    assert_eq!(last.command, "select.brush", "{}", last.message);
    assert_eq!(h.state().session().selection(), [Sel::Face { region: String::new(), face: 1 }]);
    assert_eq!(h.state().viewport.camera.unwrap(), camera);
    // `]` makes the brush larger, `[` smaller, within its bounds.
    let r = h.state().viewport.brush;
    h.hover_at(c);
    h.key_press(egui::Key::CloseBracket);
    h.run();
    assert!(h.state().viewport.brush > r);
    for _ in 0..40 {
        h.key_press(egui::Key::OpenBracket);
        h.run();
    }
    assert_eq!(h.state().viewport.brush, crate::viewport::BRUSH.0);
    // Not while a text field has the keys, the cursor over the view.
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").click();
    h.run();
    h.hover_at(c);
    h.key_press(egui::Key::CloseBracket);
    h.run();
    assert_eq!(h.state().viewport.brush, crate::viewport::BRUSH.0);
    h.key_press(egui::Key::Escape);
    h.run();
    // Ctrl takes away; a click is a dab, with the same modes.
    drag(&mut h, c - vec2(20.0, 0.0), c + vec2(20.0, 0.0), Modifiers::COMMAND);
    assert!(h.state().session().selection().is_empty());
    click_at(&mut h, c, Modifiers::SHIFT);
    assert_eq!(h.state().log.last().unwrap().command, "select.brush");
    assert_eq!(h.state().session().selection(), [Sel::Face { region: String::new(), face: 1 }]);
}

fn click_at(h: &mut Harness<'_, App>, pos: Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
    }
    h.run();
}

/// A primary drag from `a` to `b`, with `modifiers` held when it is let go.
fn drag(h: &mut Harness<'_, App>, a: Pos2, b: Pos2, modifiers: Modifiers) {
    h.hover_at(a);
    h.run();
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers });
    for k in 1..=5 {
        h.event(egui::Event::PointerMoved(a + (b - a) * (k as f32 / 5.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers });
    h.run();
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
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
    fn is_link(&self, rel: &str) -> bool {
        self.fs.is_link(rel)
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
    app.wait_for_job();
    let d = app.dialog.take().unwrap();
    assert_eq!(d.id, "case.open");
    assert!(d.params.contains("/nonexistent/umcf-case") && d.error.is_some(), "{d:?}");
    // No dialog on this system: the card asks for the path.
    app.folder_picked(Picked::Unavailable);
    let d = app.dialog.take().unwrap();
    assert!(d.error.unwrap().contains("type the case's path"));
    // A case: opened, and the card from the folder before goes.
    app.folder_picked(Picked::Folder("/nonexistent/umcf-case".to_string()));
    app.wait_for_job();
    assert!(app.dialog.is_some());
    app.folder_picked(Picked::Folder(path.clone()));
    app.wait_for_job();
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

#[test]
fn faces_picked_in_the_view_become_a_new_patch() {
    let mut h = harness(app());
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    h.get_by_label("Pick faces").click();
    h.run();
    assert!(h.state().viewport.faces);
    let rect = h.state().viewport.rect.unwrap();
    click_at(&mut h, rect.center(), Modifiers::NONE);
    assert_eq!(h.state().session().selection(), [Sel::Face { region: String::new(), face: 1 }]);
    assert_eq!(h.state().viewport.hover.as_deref(), Some("face 1 of patch sky"));
    // The card takes the picked faces.
    h.get_by_role_and_label(egui::Role::Tab, "Mesh").click();
    h.run();
    h.get_by_label("New patch").click();
    h.run();
    h.run();
    type_into(&mut h, egui::Role::TextInput, "Name", "lid");
    h.run();
    assert_eq!(h.state().new_patch.as_ref().unwrap().source, crate::newpatch::Source::Picked);
    h.get_by_label("Prepare").click();
    h.run();
    assert!(h.state().new_patch.is_none(), "{:?}", h.state().new_patch);
    let case = h.state().session().case().unwrap();
    let set = String::from_utf8(case.put_bytes("constant/polyMesh/sets/lid").unwrap().to_vec()).unwrap();
    assert!(set.ends_with("\n1\n(\n1\n)\n"), "{set}");
    // The faces are in the patch now, out of the selection: the card opens on a box again.
    assert!(h.state().session().selection().is_empty());
    h.get_by_label("New patch").click();
    h.run();
    h.run();
    assert_eq!(h.state().new_patch.as_ref().unwrap().source, crate::newpatch::Source::Box);
}

#[test]
fn a_rectangle_dragged_in_box_select_selects_what_is_in_it() {
    let mut h = harness(app());
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    h.get_by_label("Box select").click();
    h.run();
    assert_eq!(h.state().viewport.drag, crate::viewport::Drag::Select);
    let rect = h.state().viewport.rect.unwrap();
    let camera = h.state().viewport.camera.unwrap();
    let c = rect.center();
    // Around the middle of the cube: the sky, seen; the view didn't turn.
    drag(&mut h, c - vec2(20.0, 20.0), c + vec2(20.0, 20.0), Modifiers::NONE);
    assert_eq!(h.state().log.last().unwrap().command, "select.rect");
    assert_eq!(h.state().session().selection(), [Sel::Patch { region: String::new(), patch: "sky".into() }]);
    assert_eq!(h.state().viewport.camera.unwrap(), camera);
    // Faces through it, added with Shift: the sky's face and the ground's under it.
    h.get_by_label("Pick faces").click();
    h.get_by_label("Select through").click();
    h.run();
    drag(&mut h, c - vec2(20.0, 20.0), c + vec2(20.0, 20.0), Modifiers::SHIFT);
    let faces: Vec<&Sel> = h.state().session().selection().iter().filter(|s| matches!(s, Sel::Face { .. })).collect();
    assert_eq!(faces, [&Sel::Face { region: String::new(), face: 0 }, &Sel::Face { region: String::new(), face: 1 }]);
    let log: Vec<String> = h.state().log.iter().map(|l| format!("{} {}", l.command, l.message)).collect();
    assert_eq!(h.state().session().selection().len(), 3, "{log:#?}");
    // Ctrl takes them away again.
    drag(&mut h, c - vec2(20.0, 20.0), c + vec2(20.0, 20.0), Modifiers::COMMAND);
    assert_eq!(h.state().session().selection().len(), 1);
    // A click still picks.
    click_at(&mut h, c, Modifiers::NONE);
    assert_eq!(h.state().session().selection(), [Sel::Face { region: String::new(), face: 1 }]);
    let n = h.state().log.len();
    // Back to orbit: a drag turns the view and selects nothing.
    h.get_by_label("Orbit").click();
    h.run();
    drag(&mut h, c, c + vec2(50.0, 0.0), Modifiers::NONE);
    assert!(h.state().viewport.camera.unwrap().azimuth < camera.azimuth - 5.0);
    assert_eq!(h.state().log.len(), n);
}

#[test]
fn a_layer_is_shown_as_its_context_menu_says() {
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    let mut h = harness(app());
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    let rect = h.state().viewport.rect.unwrap();
    // The sky hidden from its row's menu: the row says so, and a click picks the ground.
    tree_item(&h, "sky").click_secondary();
    h.run();
    h.get_by_label("Hidden").click();
    h.run();
    let sky = Layer::Patch { region: String::new(), patch: "sky".into() };
    assert_eq!(h.state().layer_entry(&sky).and_then(|e| e.mode), Some(Mode::Hidden));
    h.key_press(egui::Key::Escape);
    h.run();
    click_at(&mut h, rect.center(), Modifiers::NONE);
    assert_eq!(h.state().session().selection(), [Sel::Patch { region: String::new(), patch: "ground".into() }]);
    assert!(h.state().log.last().unwrap().command == "select.pick");
    assert_eq!(h.state().viewport.hover.as_deref(), Some("patch ground"));
    // The sky half transparent too; then the region as a wireframe: that governs all its
    // patches, the sky with them (its mode goes, its opacity stays), and draws nothing new.
    h.state_mut().set_display(sky.clone(), crate::display::Change::Opacity(0.5));
    let (revision, built) = (h.state().revision, h.state().scene_built.clone());
    tree_item(&h, "(single region)").click_secondary();
    h.run();
    h.get_by_label("Wireframe").click();
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    let region = Layer::Region(String::new());
    assert_eq!(h.state().layer_entry(&sky).map(|e| (e.mode, e.opacity)), Some((None, Some(0.5))));
    assert_eq!(h.state().layer_display(&sky, &[]).mode, Mode::Wireframe);
    assert_eq!((h.state().revision, &h.state().scene_built), (revision, &built), "the scene isn't built again");
    // The region's opacity: the sky's entry has nothing left to set, and goes.
    h.state_mut().set_display(region.clone(), crate::display::Change::Opacity(0.8));
    assert!(h.state().layer_entry(&sky).is_none());
    assert_eq!(h.state().layer_display(&sky, &[]).opacity, 0.8);
    // A patch set after its region wins over it, in its place after it; Reset takes it away.
    h.state_mut().set_display(sky.clone(), crate::display::Change::Mode(Mode::Surface));
    let order: Vec<&Layer> = h.state().display.iter().map(|e| &e.layer).collect();
    assert_eq!(order, [&region, &sky]);
    assert_eq!(h.state().layer_display(&sky, &[]).mode, Mode::Surface);
    h.run();
    tree_item(&h, "sky").click_secondary();
    h.run();
    h.get_by_label("Reset").click();
    h.run();
    assert!(h.state().layer_entry(&sky).is_none());
    // A patch renamed: its entry goes (a pick with it would be refused), the region's stays.
    h.state_mut().set_display(sky.clone(), crate::display::Change::Mode(Mode::Points));
    h.state_mut().run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    h.state_mut().run("patch.set", json!({ "name": "lid" })).unwrap();
    h.run();
    let order: Vec<&Layer> = h.state().display.iter().map(|e| &e.layer).collect();
    assert_eq!(order, [&region]);
    click_at(&mut h, rect.center(), Modifiers::NONE);
    assert_eq!(h.state().log.last().unwrap().command, "select.pick");
}

#[test]
fn what_a_region_group_or_surface_sets_governs_what_it_holds() {
    use crate::display::Change;
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    // The sky and the walls in a group `outer`, the ground not.
    let mut fs = case();
    let patch = |n: &str, count: u32, start: u32, group: &str| {
        format!(
            "    {n}\n    {{\n        type wall;\n{group}        nFaces {count};\n        startFace {start};\n    }}\n"
        )
    };
    let outer = "        inGroups List<word> 1(outer);\n";
    fs.insert(
        "constant/polyMesh/boundary",
        format!(
            "{}3\n(\n{}{}{})\n",
            h("polyBoundaryMesh"),
            patch("ground", 1, 0, ""),
            patch("sky", 1, 1, outer),
            patch("walls", 4, 2, outer)
        ),
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut app = App::new(s);
    let p = |n: &str| Layer::Patch { region: String::new(), patch: n.into() };
    let group = Layer::Group { region: String::new(), group: "outer".into() };
    for n in ["ground", "sky", "walls"] {
        app.set_display(p(n), Change::Mode(Mode::Points));
    }
    let look = app.look;
    // The group's mode: its patches', not the ground's.
    app.set_display(group.clone(), Change::Mode(Mode::Wireframe));
    let set: Vec<&Layer> = app.display.iter().map(|e| &e.layer).collect();
    assert_eq!(set, [&group, &p("ground")]);
    assert!(app.look > look);
    // An opacity takes only opacities: the group's mode stays.
    app.set_display(p("sky"), Change::Opacity(0.3));
    app.set_display(Layer::Region(String::new()), Change::Opacity(0.6));
    assert!(app.layer_entry(&p("sky")).is_none());
    assert_eq!(app.layer_entry(&group).map(|e| (e.mode, e.opacity)), Some((Some(Mode::Wireframe), None)));
    // A surface's: its regions', not another surface's.
    let solid = |f: &str| Layer::Solid { file: f.into(), solid: "roof".into() };
    app.display.push(umcf_engine::DisplayEntry { layer: solid("a.stl"), mode: Some(Mode::Points), opacity: None });
    app.display.push(umcf_engine::DisplayEntry { layer: solid("b.stl"), mode: Some(Mode::Points), opacity: None });
    app.set_display(Layer::Surface("a.stl".into()), Change::Mode(Mode::Hidden));
    assert!(app.layer_entry(&solid("a.stl")).is_none());
    assert!(app.layer_entry(&solid("b.stl")).is_some());
    // A patch whose entry kept its opacity, set again after its region: it wins (its entry goes
    // after the region's).
    let region = Layer::Region(String::new());
    app.set_display(region.clone(), Change::Reset);
    app.set_display(p("walls"), Change::Opacity(0.5));
    app.set_display(region.clone(), Change::Mode(Mode::Wireframe));
    app.set_display(p("walls"), Change::Mode(Mode::Surface));
    assert_eq!(app.layer_display(&p("walls"), &["outer".to_string()]).mode, Mode::Surface);
    assert_eq!(app.display.last().map(|e| &e.layer), Some(&p("walls")));
    // Reset takes away only the layer's own entry.
    app.set_display(group.clone(), Change::Reset);
    assert!(app.layer_entry(&group).is_none());
    assert!(app.layer_entry(&p("walls")).is_some());
}

/// The case of `case()` with its patches in the groups named (`ground` in `g1` and `g2`, say).
fn grouped(groups: &[(&str, &[&str])]) -> App {
    let mut fs = case();
    let names = ["ground", "sky", "walls"];
    let counts = [(1, 0), (1, 1), (4, 2)];
    let mut body = String::new();
    for (n, (count, start)) in names.iter().zip(counts) {
        let g = groups.iter().find(|(p, _)| p == n).map_or(&[][..], |(_, g)| *g);
        let in_groups = if g.is_empty() {
            String::new()
        } else {
            format!("        inGroups List<word> {}({});\n", g.len(), g.join(" "))
        };
        body += &format!(
            "    {n}\n    {{\n        type wall;\n{in_groups}        nFaces {count};\n        startFace {start};\n    }}\n"
        );
    }
    fs.insert("constant/polyMesh/boundary", format!("{}3\n(\n{body})\n", h("polyBoundaryMesh")));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    App::new(s)
}

/// What the browser says of each patch (`layer_display`) is what the view draws (the engine's
/// `display_style`, class by class); returns each patch's (mode, opacity).
fn drawn(app: &mut App) -> std::collections::HashMap<String, (umcf_render::raster::Mode, f32)> {
    let scene = app.session.scene(&json!({})).unwrap().clone();
    let style = umcf_engine::display_style(&scene, &app.display);
    let mut out = std::collections::HashMap::new();
    for (k, o) in scene.objects.iter().enumerate() {
        let umcf_render::Source::Mesh { region } = &o.source else { continue };
        for (c, name) in o.classes.iter().enumerate() {
            let groups = o.groups.get(c).cloned().unwrap_or_default();
            let d = style.get(&(k, c as u32)).copied().unwrap_or_default();
            let layer = umcf_engine::Layer::Patch { region: region.clone(), patch: name.clone() };
            let said = app.layer_display(&layer, &groups);
            assert_eq!((d.mode, d.opacity), (said.mode, said.opacity), "{name}");
            out.insert(name.clone(), (d.mode, d.opacity));
        }
    }
    out
}

#[test]
fn what_was_set_last_wins_until_its_holder_sets_the_same() {
    use crate::display::Change;
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    let region = || Layer::Region(String::new());
    let patch = |n: &str| Layer::Patch { region: String::new(), patch: n.into() };
    let group = |g: &str| Layer::Group { region: String::new(), group: g.into() };
    let both: &[&str] = &["g1", "g2"];
    // Each: the changes in order, then what the ground is drawn as.
    type Case = (Vec<(Layer, Change)>, (Mode, f32));
    let cases: Vec<Case> = vec![
        // A region's opacity after a patch's mode set after the region's.
        (
            vec![
                (region(), Change::Mode(Mode::Wireframe)),
                (patch("ground"), Change::Mode(Mode::Points)),
                (region(), Change::Opacity(0.5)),
            ],
            (Mode::Points, 0.5),
        ),
        // A patch's opacity after its region's.
        (
            vec![
                (patch("ground"), Change::Mode(Mode::Points)),
                (region(), Change::Opacity(0.5)),
                (patch("ground"), Change::Opacity(0.8)),
            ],
            (Mode::Points, 0.8),
        ),
        // A region's mode after a patch's opacity set after the region's.
        (
            vec![
                (region(), Change::Opacity(0.5)),
                (patch("ground"), Change::Opacity(0.8)),
                (region(), Change::Mode(Mode::Wireframe)),
            ],
            (Mode::Wireframe, 0.8),
        ),
        // A group's, between its region's and its patch's.
        (
            vec![
                (patch("ground"), Change::Opacity(0.8)),
                (group("g1"), Change::Mode(Mode::Points)),
                (region(), Change::Opacity(0.4)),
                (group("g1"), Change::Opacity(0.6)),
            ],
            (Mode::Points, 0.6),
        ),
        // Two groups of one patch: the one set last.
        (
            vec![
                (group("g1"), Change::Opacity(0.3)),
                (group("g2"), Change::Opacity(0.7)),
                (group("g1"), Change::Opacity(0.5)),
            ],
            (Mode::Surface, 0.5),
        ),
    ];
    for (k, (changes, want)) in cases.into_iter().enumerate() {
        let mut app = grouped(&[("ground", both)]);
        for (layer, change) in changes {
            app.set_display(layer, change);
        }
        let got = drawn(&mut app);
        assert_eq!(got.get("ground"), Some(&want), "case {k}: {:?}", app.display);
        // The sky, in no group, is drawn as its region only.
        let r = app.layer_display(&region(), &[]);
        assert_eq!(got.get("sky"), Some(&(r.mode, r.opacity)), "case {k}");
    }
}

/// A case whose mesh `boundary` reads wait, once `hold` is set, until it is cleared again.
struct Held {
    fs: MemFs,
    hold: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
}

impl CaseFs for Held {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        if rel.ends_with("polyMesh/boundary") {
            while self.hold.load(Ordering::SeqCst) {
                self.waiting.store(true, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
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
        self.fs.list(rel)
    }
}

impl CaseWrite for Held {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.fs.write(rel, bytes)
    }
}

/// An app whose case can be held at its mesh reads, its reads stopped by the session's cancel.
fn held() -> (App, Arc<AtomicBool>, Arc<AtomicBool>) {
    let (hold, waiting) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let fs = Held { fs: case(), hold: hold.clone(), waiting: waiting.clone() };
    let fs = umcf_core::Cancellable::new(fs, s.canceller());
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut app = App::new(s);
    app.job_wait = std::time::Duration::ZERO;
    hold.store(true, Ordering::SeqCst);
    (app, hold, waiting)
}

#[test]
fn a_slow_scene_is_a_job_the_window_shows_and_can_cancel() {
    let (app, hold, waiting) = held();
    let mut h = Harness::builder()
        .with_size(vec2(1200.0, 800.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run_steps(2);
    // The mesh is being read: the window says so, the session is with the job.
    wait_until(&waiting);
    assert!(h.state().busy());
    let _ = h.get_by_label_contains("Reading the meshes");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    let _ = h.get_by_label_contains("Cancelling");
    // The read it waited in ends; the next one stops: the scene is cancelled, the case is kept.
    hold.store(false, Ordering::SeqCst);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    assert!(!h.state().busy());
    assert!(h.state().session().case().is_some());
    let _ = h.get_by_label_contains("Reading cancelled");
    // The flag went with the job: what runs next reads.
    assert!(h.state_mut().run("mesh.check", json!({})).is_ok());
}

#[test]
fn a_command_waits_while_a_job_runs() {
    let (mut app, hold, waiting) = held();
    app.start_scene_for_test();
    wait_until(&waiting);
    let e = app.run("case.info", json!({})).unwrap_err();
    assert!(e.contains("is running"), "{e}");
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    assert!(app.run("case.info", json!({})).is_ok());
    // After a command the scene is taken again as a job, from the cache: done within the wait.
    app.job_wait = std::time::Duration::from_secs(120);
    app.start_scene_for_test();
    assert!(!app.busy());
    assert!(app.session().case().is_some());
}

#[test]
fn only_work_that_reads_can_be_cancelled() {
    let read =
        crate::jobs::Work::Command { id: "mesh.check".into(), params: json!({}), then: crate::jobs::Then::Nothing };
    let write =
        crate::jobs::Work::Command { id: "case.save".into(), params: json!({}), then: crate::jobs::Then::Nothing };
    assert!(crate::jobs::cancellable(&read));
    assert!(!crate::jobs::cancellable(&write));
    assert!(crate::jobs::cancellable(&crate::jobs::Work::Scene { params: json!({}) }));
}

#[test]
fn work_asked_while_a_job_runs_waits_and_runs_after_it() {
    let (mut app, hold, waiting) = held();
    app.start_scene_for_test();
    wait_until(&waiting);
    // Not refused: queued, in order, and the window still draws from a copy of the session.
    let logged = app.log.len();
    app.invoke("case.check");
    let d = crate::Dialog { id: "control.set", params: "{\"endTime\": 7200}".into(), error: None };
    app.start(crate::jobs::Work::Command {
        id: "control.set".into(),
        params: json!({ "endTime": 7200 }),
        then: crate::jobs::Then::Dialog(d),
    });
    assert_eq!(app.log.len(), logged, "nothing refused");
    assert!(app.busy() && !app.modal_busy() && app.session().case().is_some());
    assert_eq!(app.progress().map(|p| p.queued), Some(2));
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    let ran: Vec<_> = app.log.iter().skip(logged).map(|e| (e.command.as_str(), e.ok)).collect();
    assert_eq!(ran, [("case.check", true), ("control.set", true)]);
    assert_eq!(app.session().case().unwrap().controls.end_time.as_deref(), Some("7200"));
}

/// Waits for `flag`, at most 30 s: a test fails rather than hangs.
fn wait_until(flag: &AtomicBool) {
    let mut waited = 0;
    while !flag.load(Ordering::SeqCst) {
        assert!(waited < 15_000, "the job never got to its read");
        std::thread::sleep(std::time::Duration::from_millis(2));
        waited += 1;
    }
}

/// A case that counts the surface reads made on the thread the window runs on (the test's).
struct OffJob {
    fs: MemFs,
    reads: Arc<AtomicUsize>,
    window: std::thread::ThreadId,
}

impl CaseFs for OffJob {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        if rel.ends_with(".stl") && std::thread::current().id() == self.window {
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
        self.fs.list(rel)
    }
}

impl CaseWrite for OffJob {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.fs.write(rel, bytes)
    }
}

#[test]
fn the_scene_is_read_only_by_jobs() {
    let mut fs = case();
    let stl = "solid s\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s\n";
    fs.insert("constant/triSurface/tri.stl", stl);
    let reads = Arc::new(AtomicUsize::new(0));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let window = std::thread::current().id();
    s.open(Box::new(OffJob { fs, reads: reads.clone(), window }), "mem", OpenOptions::default(), false).unwrap();
    let mut h = harness(App::new(s));
    h.state_mut().show_only(&[String::new(), "tri.stl".to_string()]);
    reads.store(0, Ordering::SeqCst);
    // The view, the browser's surface regions and Fit: none reads the surface on the window's thread.
    h.run();
    h.state_mut().fit_view(Some(View::Top));
    h.run();
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(!h.state().classes.is_empty(), "the browser lists the surface's regions");
}

#[test]
fn a_cards_command_is_a_job_and_its_card_waits_for_it() {
    let (mut app, hold, waiting) = held();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    hold.store(true, Ordering::SeqCst);
    // `patch.set` reads the mesh's `boundary`, held here: the command runs as a job and the
    // card stays open meanwhile, behind the busy screen.
    let form = crate::patch::PatchForm {
        name: "sky".into(),
        kind: "symmetry".into(),
        filled: ("sky".into(), "wall".into(), String::new()),
        ..Default::default()
    };
    app.patch_form = Some(form.clone());
    app.set_patch();
    app.launch();
    wait_until(&waiting);
    assert!(app.busy() && app.writing(), "a card's command changes the case: not cancellable");
    assert!(app.patch_form.is_some());
    // Another card's *Set* meanwhile: it waits, its card with it.
    app.bc = Some(crate::bc::BcForm { field: "T".into(), kind: "zeroGradient".into(), ..Default::default() });
    app.set_bc();
    assert!(app.card_busy(crate::jobs::Card::Bc) && app.bc.as_ref().is_some_and(|f| f.error.is_none()));
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    assert!(app.patch_form.is_none(), "it worked: the card closed");
    let case = app.session().case().unwrap();
    assert!(case.region("").unwrap().patches.as_ref().unwrap().iter().any(|p| p.name == "sky" && p.kind == "symmetry"));
    // Both ran, the card's own first, the one asked meanwhile after it.
    let ran: Vec<_> = app.log.iter().rev().take(2).map(|e| (e.command.as_str(), e.ok)).collect();
    assert_eq!(ran, [("bc.set", true), ("patch.set", true)]);
    assert!(app.bc.is_none());
    // A failure keeps the card, with the error.
    app.patch_form = Some(crate::patch::PatchForm { kind: "no such type".into(), ..form });
    app.set_patch();
    app.wait_for_job();
    assert!(app.patch_form.as_ref().and_then(|f| f.error.as_ref()).is_some(), "{:?}", app.patch_form);
}

#[test]
fn the_history_bar_goes_back_as_a_job() {
    let (mut app, hold, waiting) = held();
    hold.store(false, Ordering::SeqCst);
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    app.run("control.set", json!({ "endTime": 9000 })).unwrap();
    assert_eq!(app.session().history().0.len(), 2);
    app.go_to(0);
    assert!(app.working(), "queued for the next frame");
    app.wait_for_job();
    assert_eq!(app.session().history().0.len(), 0);
    let steps: Vec<_> = app.log.iter().rev().take(2).map(|e| (e.command.as_str(), e.ok)).collect();
    assert_eq!(steps, [("edit.undo", true), ("edit.undo", true)]);
    let _ = waiting;
}

#[test]
fn a_cards_set_takes_the_place_of_a_scene_only_queued() {
    // The view queues its scene as it draws, and the cards draw after it: their *Set* in that
    // frame runs (the view asks for its scene again), rather than being refused.
    let mut app = app();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    app.start(crate::jobs::Work::Scene { params: app.shown() });
    app.bc = Some(crate::bc::BcForm { field: "T".into(), kind: "zeroGradient".into(), ..Default::default() });
    app.set_bc();
    app.wait_for_job();
    assert!(app.bc.is_none(), "{:?}", app.bc);
    assert_eq!(app.log.last().map(|e| (e.command.as_str(), e.ok)), Some(("bc.set", true)), "{:?}", app.log.last());
    // The scene that gave way is read on the view's next frames, for the case as it is now.
    assert!(app.scene_built.is_none());
    let mut h = harness(app);
    h.run();
    let (revision, shown) = (h.state().revision, h.state().shown());
    assert_eq!(h.state().scene_built.as_ref(), Some(&(revision, shown)));
}

#[test]
fn a_history_walk_that_fails_partway_logs_each_step_and_opens_the_results() {
    let mut app = app();
    app.results_open = false;
    let undone = umcf_engine::Outcome::new("undone", json!({}));
    let failed = umcf_engine::Error::Failed("the case changed on disk".into());
    app.finish_history_for_test(vec![("edit.undo", Ok(undone)), ("edit.undo", Err(failed))]);
    let last: Vec<_> = app.log.iter().rev().take(2).map(|e| (e.command.as_str(), e.ok)).collect();
    assert_eq!(last, [("edit.undo", false), ("edit.undo", true)]);
    assert!(app.log.last().is_some_and(|e| e.message.contains("changed on disk")));
    assert!(app.results_open, "a failure opens the results card");
}

#[test]
fn a_patch_card_set_while_a_job_runs_waits_with_its_card() {
    // The card reads the selection from the copy of the session the window draws from, and
    // its command waits for the job: the card shows it as sent, then closes.
    let (mut app, hold, waiting) = held();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    hold.store(true, Ordering::SeqCst);
    app.start_scene_for_test();
    wait_until(&waiting);
    let form = crate::patch::PatchForm {
        name: "sky".into(),
        kind: "symmetry".into(),
        filled: ("sky".into(), "wall".into(), String::new()),
        ..Default::default()
    };
    app.patch_form = Some(form);
    app.set_patch();
    assert!(app.card_busy(crate::jobs::Card::Patch) && app.patch_form.is_some());
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    assert!(app.patch_form.is_none(), "{:?}", app.patch_form);
    let case = app.session().case().unwrap();
    assert!(case.region("").unwrap().patches.as_ref().unwrap().iter().any(|p| p.name == "sky" && p.kind == "symmetry"));
}

#[test]
fn the_history_bar_goes_forward_as_a_job_too() {
    let mut app = app();
    app.run("control.set", json!({ "endTime": 7200 })).unwrap();
    app.run("control.set", json!({ "endTime": 9000 })).unwrap();
    app.go_to(0);
    app.wait_for_job();
    app.go_to(2);
    app.wait_for_job();
    assert_eq!(app.session().history().0.len(), 2);
    let steps: Vec<_> = app.log.iter().rev().take(2).map(|e| (e.command.as_str(), e.ok)).collect();
    assert_eq!(steps, [("edit.redo", true), ("edit.redo", true)]);
    // Past the end: nothing to do, nothing logged.
    let logged = app.log.len();
    app.go_to(5);
    app.wait_for_job();
    assert_eq!((app.session().history().0.len(), app.log.len()), (2, logged));
}

#[test]
fn a_folder_opens_first_and_its_fields_are_read_after_as_a_job() {
    // A case of each OpenFOAM version.
    for example in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of8/streetCanyon_CFD"),
    ] {
        let mut app = App::new(Session::with_backup(".umcfgui/backup/test"));
        app.open_folder(example);
        // Opening takes the whole window; reading the fields after it doesn't.
        app.job_wait = std::time::Duration::ZERO;
        app.launch();
        assert!(app.modal_busy(), "{example}");
        while app.busy() {
            app.poll(std::time::Duration::from_millis(20));
        }
        assert!(app.session().case().is_some_and(|c| c.fields_pending), "{example}");
        // The view's scene first, so the case shows at once; then the fields.
        app.launch();
        assert!(app.busy() && !app.modal_busy(), "{example}: the scene is read as a job");
        assert!(app.progress().is_some_and(|p| p.label.contains("meshes") && p.queued == 1));
        while app.busy() {
            app.poll(std::time::Duration::from_millis(20));
        }
        app.launch();
        assert!(app.busy() && !app.modal_busy(), "{example}: the fields are read as a job");
        assert!(app.progress().is_some_and(|p| p.label.contains("field") && p.cancellable));
        app.wait_for_job();
        let case = app.session().case().unwrap();
        assert!(!case.fields_pending, "{example}");
        assert!(case.regions.iter().flat_map(|r| &r.fields).any(|f| f.bcs.is_some()), "{example}");
        assert_eq!(app.log.last().map(|e| (e.command.as_str(), e.ok)), Some(("case.fields", true)));
    }
}

#[test]
fn the_window_stays_in_use_while_a_job_runs() {
    let (app, hold, waiting) = held();
    let mut h = Harness::builder()
        .with_size(vec2(1600.0, 900.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.state_mut().start_scene_for_test();
    wait_until(&waiting);
    h.run_steps(2);
    // The ribbon, the browser and the status bar, with the job's progress and its Cancel.
    assert!(h.state().busy());
    let _ = h.get_by_role_and_label(egui::Role::Tab, "Mesh");
    let _ = h.get_by_label("Browser");
    let _ = h.get_by_label_contains("Reading the meshes");
    let _ = h.get_by_role_and_label(egui::Role::ProgressIndicator, "Job progress");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().progress().is_some_and(|p| p.cancelling));
    hold.store(false, Ordering::SeqCst);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    assert!(!h.state().busy());
}

#[test]
fn a_condition_card_opened_before_the_fields_fills_when_they_are_read() {
    // Review finding: it filled once, from fields not read yet, and stayed blank.
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let options = OpenOptions { defer_fields: true, ..OpenOptions::default() };
    s.open(Box::new(case()), "mem", options, false).unwrap();
    let mut app = App::new(s);
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    app.bc = Some(crate::bc::BcForm::default());
    let mut h = harness(app);
    h.run();
    assert_eq!(h.state().bc.as_ref().map(|f| f.kind.as_str()), Some(""));
    h.state_mut().after_open();
    h.state_mut().wait_for_job();
    h.run();
    let f = h.state().bc.clone().unwrap();
    assert_eq!((f.field.as_str(), f.kind.as_str(), f.value.as_str()), ("T", "fixedValue", "uniform 290"));
}

#[test]
fn a_cards_set_names_the_patches_it_shows() {
    // Review finding: sent without them, a selection changed while it waited was set instead.
    let (mut app, hold, waiting) = held();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    hold.store(true, Ordering::SeqCst);
    app.start_scene_for_test();
    wait_until(&waiting);
    // A click on another patch waits; the card still shows `sky`, and sets that.
    app.act("select.set", json!({ "items": [{ "region": "", "patch": "ground" }] }));
    app.bc = Some(crate::bc::BcForm { field: "T".into(), kind: "zeroGradient".into(), ..Default::default() });
    app.set_bc();
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    let case = app.session().case().unwrap();
    let bcs = case.region("").unwrap().field("T").unwrap().bcs.clone().unwrap();
    let kind = |p: &str| bcs.iter().find(|b| b.patch == p).and_then(|b| b.kind.clone());
    assert_eq!(kind("sky").as_deref(), Some("zeroGradient"));
    assert_eq!(kind("ground"), None, "ground keeps its condition from the pattern");
    assert_eq!(app.session().selection().len(), 1);
}

#[test]
fn a_condition_card_keeps_what_is_typed_while_the_fields_are_read() {
    // Review finding: it filled again on every frame till the fields were read.
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let options = OpenOptions { defer_fields: true, ..OpenOptions::default() };
    s.open(Box::new(case()), "mem", options, false).unwrap();
    let mut app = App::new(s);
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    app.bc = Some(crate::bc::BcForm::default());
    let mut h = harness(app);
    h.run();
    if let Some(f) = h.state_mut().bc.as_mut() {
        f.kind = "inletOutlet".into();
    }
    h.run();
    assert_eq!(h.state().bc.as_ref().map(|f| f.kind.as_str()), Some("inletOutlet"));
    // Read now: what was typed stays.
    h.state_mut().after_open();
    h.state_mut().wait_for_job();
    h.run();
    assert_eq!(h.state().bc.as_ref().map(|f| f.kind.as_str()), Some("inletOutlet"));
}

#[test]
fn a_case_opening_behind_a_job_takes_the_window_and_goes_first_after_it() {
    // Review finding: work asked while an open waited ran on the case it opened.
    let (mut app, hold, waiting) = held();
    app.start_scene_for_test();
    wait_until(&waiting);
    assert!(!app.modal_busy());
    app.open_folder(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD"));
    assert!(app.modal_busy(), "taken at once: nothing more can be asked of the case shown");
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    let ran: Vec<_> = app.log.iter().rev().take(2).map(|e| e.command.as_str()).collect();
    assert_eq!(ran, ["case.fields", "case.open"]);
}

#[test]
fn the_ribbon_leaves_a_card_whose_command_waits() {
    let (mut app, hold, waiting) = held();
    app.run("select.set", json!({ "items": [{ "region": "", "patch": "sky" }] })).unwrap();
    hold.store(true, Ordering::SeqCst);
    app.start_scene_for_test();
    wait_until(&waiting);
    app.bc = Some(crate::bc::BcForm { field: "T".into(), kind: "zeroGradient".into(), ..Default::default() });
    app.set_bc();
    // Another card from the ribbon, and Escape: the waiting card stays.
    app.invoke("patch.set");
    assert!(app.bc.is_some() && app.patch_form.is_some());
    app.invoke("control.set");
    assert!(app.bc.is_some() && app.patch_form.is_none());
    hold.store(false, Ordering::SeqCst);
    app.wait_for_job();
    assert!(app.bc.is_none(), "closed by its own command's outcome");
}

#[test]
fn every_open_from_the_window_reads_the_fields_after() {
    // Review finding: the Open card (no folder dialog, or a retry) read them at open.
    let mut app = App::new(Session::with_backup(".umcfgui/backup/test"));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD");
    app.start(crate::jobs::Work::Command {
        id: "case.open".into(),
        params: json!({ "path": path }),
        then: crate::jobs::Then::Nothing,
    });
    app.launch();
    while app.busy() {
        app.poll(std::time::Duration::from_millis(20));
    }
    assert!(app.session().case().is_some_and(|c| c.fields_pending));
    app.wait_for_job();
    assert!(app.session().case().is_some_and(|c| !c.fields_pending));
}

#[test]
fn a_newer_scene_request_takes_the_place_of_one_waiting() {
    let mut app = App::new(Session::with_backup(".umcfgui/backup/test"));
    app.start(crate::jobs::Work::Scene { params: json!({ "regions": ["air"] }) });
    app.start(crate::jobs::Work::Scene { params: json!({ "regions": ["air", "ground"] }) });
    assert_eq!(app.queue.len(), 1);
    assert!(
        matches!(&app.queue[0], crate::jobs::Work::Scene { params } if params["regions"] == json!(["air", "ground"]))
    );
}

#[test]
fn read_fields_runs_without_a_card() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD");
    s.execute("case.open", &json!({ "path": path, "fields": "later" })).unwrap();
    let mut h = harness(App::new(s));
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::P);
    h.run();
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").type_text("Read fields");
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.state().dialog.is_none(), "no card");
    h.state_mut().wait_for_job();
    assert!(h.state().session().case().is_some_and(|c| !c.fields_pending));
    assert_eq!(h.state().log.last().map(|e| (e.command.as_str(), e.ok)), Some(("case.fields", true)));
}

#[test]
fn a_command_asked_for_at_start_up_leaves_the_first_scene_waiting() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD");
    s.execute("case.open", &json!({ "path": path, "fields": "later" })).unwrap();
    let mut app = App::new(s);
    app.start_up(Some("case.check")).unwrap();
    let ids: Vec<String> = app
        .queue
        .iter()
        .map(|w| match w {
            crate::jobs::Work::Command { id, .. } => id.clone(),
            crate::jobs::Work::Scene { .. } => "scene".to_string(),
            crate::jobs::Work::History { .. } => "history".to_string(),
        })
        .collect();
    assert_eq!(ids, ["scene", "case.fields", "case.check"]);
}

#[test]
fn read_fields_asked_for_at_start_up_reads_them_once() {
    let mut s = Session::with_backup(".umcfgui/backup/test");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/of12/streetCanyon_CFD");
    s.execute("case.open", &json!({ "path": path, "fields": "later" })).unwrap();
    let mut app = App::new(s);
    app.start_up(Some("case.fields")).unwrap();
    assert_eq!(app.queue.len(), 2);
    assert!(matches!(app.queue.back(), Some(crate::jobs::Work::Command { id, .. }) if id == "case.fields"));
    app.wait_for_job();
    assert!(app.session().case().is_some_and(|c| !c.fields_pending));
    let reads: Vec<bool> = app.log.iter().filter(|e| e.command == "case.fields").map(|e| e.ok).collect();
    assert_eq!(reads, [true]);
}

#[test]
fn without_a_gpu_view_the_status_bar_label_is_no_switch() {
    // No GPU device here (the window runs on OpenGL): the label says CPU, and a click leaves it.
    // (The switch itself is `toggle_renderer`, as *GPU* in the ribbon; the harness has no GPU.)
    let mut h = harness(app());
    h.run();
    h.get_by_label("CPU").click();
    h.run();
    assert_eq!(h.state().viewport.renderer, crate::viewport::Renderer::Cpu);
    assert!(!h.state().viewport.has_gpu());
    h.get_by_label("CPU");
}

#[test]
fn the_snapshot_card_starts_with_the_views_grid_origin_and_background() {
    let mut a = app();
    a.invoke("scene.snapshot");
    assert_eq!(a.dialog.as_ref().unwrap().params, "{}");
    a.dialog = None;
    a.viewport.guides.grid = false;
    a.viewport.dark = false;
    a.invoke("scene.snapshot");
    let params: serde_json::Value = serde_json::from_str(&a.dialog.as_ref().unwrap().params).unwrap();
    assert_eq!(params, serde_json::json!({ "grid": false, "dark": false }));
}

#[test]
fn the_triad_stays_while_the_scene_is_built() {
    // The frames that wait for a scene show the last image and the triad, which would blink
    // otherwise.
    let h = harness(app());
    let mut viewport = crate::viewport::Viewport::default();
    viewport.camera = h.state().viewport.camera;
    assert!(viewport.camera.is_some());
    let tokens = crate::theme::Tokens::light();
    let rect = egui::Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
    let mut waiting = Harness::new_ui(move |ui| {
        ui.ctx().set_fonts(crate::theme::fonts());
        viewport.paint_last(ui, rect, &tokens);
    });
    waiting.run();
    for axis in ["+X view", "-Y view", "+Z view"] {
        assert!(waiting.query_by_label(axis).is_some(), "{axis}");
    }
}

#[test]
fn new_case_starts_from_the_air_alone_and_opens_what_it_makes() {
    let dir = std::env::temp_dir().join(format!("umcf-ui-{}-new", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::new(Session::new());
    app.invoke("case.new");
    let d = app.dialog.take().unwrap();
    assert_eq!(d.id, "case.new");
    let mut params: serde_json::Value = serde_json::from_str(&d.params).unwrap();
    assert_eq!(params["regions"], json!([{ "name": "air", "kind": "fluid" }]));
    // Without a path, the card stays with why.
    app.start(crate::jobs::Work::Command {
        id: "case.new".into(),
        params: params.clone(),
        then: crate::jobs::Then::Dialog(d.clone()),
    });
    app.wait_for_job();
    assert!(app.dialog.as_ref().and_then(|d| d.error.as_ref()).is_some());
    params["path"] = json!(dir.to_str().unwrap());
    app.start(crate::jobs::Work::Command { id: "case.new".into(), params, then: crate::jobs::Then::Dialog(d) });
    app.wait_for_job();
    assert_eq!(app.session().location(), dir.to_str());
    assert_eq!(app.log.last().unwrap().command, "case.new");
    assert!(app.log.last().unwrap().ok);
    let _ = std::fs::remove_dir_all(&dir);
}
