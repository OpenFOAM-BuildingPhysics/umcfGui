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

/// A window of this width, 800 high.
fn harness_wide(mut app: App, width: f32) -> Harness<'static, App> {
    // Jobs are waited for, as in `harness`.
    app.job_wait = std::time::Duration::from_secs(120);
    let mut h = Harness::builder()
        .with_size(vec2(width, 800.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run();
    h
}

#[test]
fn the_window_starts_dark() {
    assert!(app().tokens.dark);
    let mut h = harness(app());
    h.get_by_role_and_label(egui::Role::Tab, "View").click();
    h.run();
    h.get_by_label("Light").click();
    h.run();
    assert!(!h.state().tokens.dark);
}

#[test]
fn a_narrow_ribbon_gives_way_and_drops_nothing() {
    // Wide: every button with its label.
    let h = harness_wide(app(), 2600.0);
    assert!(h.query_by_label("More tabs").is_none() && h.query_by_label("Scroll ribbon right").is_none());
    assert!(h.get_by_label("Read fields").rect().width() > 50.0);
    drop(h);
    // Narrower: buttons with an icon of their own go compact (icon only, the name in the
    // tooltip); those with only the generic command icon never lose their label: their group
    // folds into its drop-down instead.
    for width in [700.0, 1000.0, 1300.0] {
        let h = harness_wide(app(), width);
        // On the ribbon with its label, or in the More drop-down: never an icon alone.
        match h.query_by_label("Read fields") {
            Some(n) => assert!(n.rect().width() > 50.0, "at {width} Read fields is an icon alone"),
            None => {
                h.get_by_role_and_label(egui::Role::Button, "More");
            }
        };
    }
    // At 700 the More group, of generic icons only, is its drop-down rather than icons.
    let h = harness_wide(app(), 700.0);
    assert!(h.query_by_label("Read fields").is_none());
    h.get_by_role_and_label(egui::Role::Button, "More");
    drop(h);
    let mut a = app();
    a.tab = 4;
    let h = harness_wide(a, 700.0);
    assert!(h.get_by_label("Snapshot").rect().width() < 40.0, "the View tab's buttons go compact");
    drop(h);
    // Narrow: groups fold into drop-down buttons, tabs into », and the ribbon scrolls.
    let mut h = harness_wide(app(), 380.0);
    assert!(h.query_by_label("Read fields").is_none());
    h.get_by_role_and_label(egui::Role::Button, "Inspect").click();
    h.run();
    h.get_by_label("Check").click();
    h.run();
    assert_eq!(h.state().log.last().unwrap().command, "case.check");
    assert!(h.query_by_label("Summary").is_none(), "running an item closes the drop-down");
    h.get_by_label("Scroll ribbon right").click();
    h.run();
    h.run();
    h.get_by_label("Scroll ribbon left");
    h.get_by_role_and_label(egui::Role::Button, "More").click();
    h.run();
    h.get_by_label("Read fields").click();
    h.run();
    assert_eq!(h.state().log.last().unwrap().command, "case.fields");
    // The tabs that don't fit are in the » menu.
    h.get_by_label("More tabs").click();
    h.run();
    h.get_by_label("View").click();
    h.run();
    assert_eq!(h.state().tab, 4);
    h.get_by_role_and_label(egui::Role::Tab, "View");
}

/// A wide chosen tab on a narrow window leaves no room for a usable search field: it shows as
/// its icon, which opens a field of usable width.
#[test]
fn the_search_folds_to_its_icon_rather_than_squeeze() {
    let narrow = |width: f32| {
        let mut a = app();
        a.tab = 3;
        harness_wide(a, width)
    };
    // Wide enough: the field, at its full width.
    let h = narrow(1000.0);
    assert!(h.query_by_role_and_label(egui::Role::Button, "Search commands").is_none());
    assert!(h.get_by_role_and_label(egui::Role::TextInput, "Search commands").rect().width() >= 139.0);
    drop(h);
    // Somewhat narrower, the chosen "Boundary conditions" leaves no room for the field: the icon.
    let width = (380..=700)
        .step_by(10)
        .map(|w| w as f32)
        .find(|&w| narrow(w).query_by_role_and_label(egui::Role::Button, "Search commands").is_some())
        .expect("a width with the search folded to its icon");
    let mut h = narrow(width);
    assert!(h.query_by_role_and_label(egui::Role::TextInput, "Search commands").is_none());
    h.get_by_role_and_label(egui::Role::Button, "Search commands").click();
    h.run();
    // Open, the field has its width, clear of the tabs (all in »), and it has the focus.
    let field = h.get_by_role_and_label(egui::Role::TextInput, "Search commands").rect();
    assert!(field.width() >= 139.0 && field.right() <= width + 0.5, "{field:?}");
    assert!(h.query_all_by_role(egui::Role::Tab).all(|t| !t.rect().intersects(field)));
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").type_text("mesh");
    h.run();
    assert_eq!(h.state().search, "mesh");
    // Escape closes it, back to the icon and the tabs.
    h.key_press(egui::Key::Escape);
    h.run();
    h.run();
    h.get_by_role_and_label(egui::Role::Button, "Search commands");
    h.get_by_role_and_label(egui::Role::Tab, "Boundary conditions");
}

/// The narrowest window (200 points): tabs stay reachable through », the open search takes
/// the tab row, and its results stay inside the window.
#[test]
fn the_narrowest_window_keeps_tabs_and_search_usable() {
    let mut a = app();
    a.tab = 3;
    let mut h = harness_wide(a, 200.0);
    let more = h.get_by_label("More tabs").rect();
    assert!(more.right() <= 200.5, "» at {more:?}");
    h.get_by_label("More tabs").click();
    h.run();
    h.get_by_label("Mesh").click();
    h.run();
    assert_eq!(h.state().tab, 2);
    h.get_by_role_and_label(egui::Role::Button, "Search commands").click();
    h.run();
    let field = h.get_by_role_and_label(egui::Role::TextInput, "Search commands").rect();
    // The ribbon is about 80 points wide there: the field takes most of it.
    assert!(field.right() <= 200.5 && field.width() >= 40.0, "{field:?}");
    assert!(h.query_by_label("More tabs").is_none(), "» gives the field its room");
    // The results, under the field, inside the window.
    // (The Mesh tab, now chosen, has a Check mesh button too: the result is the one under
    // the field.)
    let row = h
        .query_all_by_label_contains("Check mesh")
        .map(|n| n.rect())
        .find(|r| r.top() > field.bottom())
        .expect("Check mesh in the results");
    assert!(row.left() >= -0.5 && row.right() <= 200.5, "{row:?}");
}

/// On a short window the search results stay wholly under the field and inside the window:
/// the popup is never moved up over what is typed.
#[test]
fn the_search_results_stay_under_the_field_on_a_short_window() {
    for height in [150.0, 220.0, 400.0] {
        let mut h = Harness::builder()
            .with_size(vec2(900.0, height))
            .with_max_steps(16)
            .build_ui_state(|ui, app: &mut App| app.show(ui), app());
        h.run();
        h.get_by_role_and_label(egui::Role::TextInput, "Search commands").click();
        h.run();
        let field = h.get_by_role_and_label(egui::Role::TextInput, "Search commands").rect();
        let popup = h.ctx.memory(|m| m.area_rect(egui::Id::unique("search-results"))).expect("the results are open");
        assert!(popup.top() >= field.bottom() - 0.5, "at {height}: {popup:?} over the field {field:?}");
        assert!(popup.bottom() <= height + 0.5, "at {height}: {popup:?} past the window");
    }
    // Even at the shortest a result is in sight and runs: filter to Summary and click its row
    // (the first, just under the card's top margin).
    let mut a = app();
    a.job_wait = std::time::Duration::from_secs(120);
    let mut h = Harness::builder()
        .with_size(vec2(900.0, 150.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), a);
    h.run();
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").click();
    h.run();
    h.get_by_role_and_label(egui::Role::TextInput, "Search commands").type_text("summary");
    h.run();
    let popup = h.ctx.memory(|m| m.area_rect(egui::Id::unique("search-results"))).expect("the results are open");
    assert!(popup.height() >= 9.0 * 2.0 + 36.0 - 0.5, "a row's room at 150: {popup:?}");
    click_at(&mut h, popup.left_top() + vec2(60.0, 9.0 + 18.0), Modifiers::NONE);
    assert_eq!(h.state().log.last().map(|e| e.command.as_str()), Some("case.info"));
}

/// At every width the ribbon's buttons in sight never sit on one another, and the tabs stay in
/// the window.
#[test]
fn the_ribbon_never_overlaps_itself() {
    for width in [380.0, 560.0, 760.0, 1000.0, 1300.0, 1800.0] {
        for tab in 0..5 {
            let mut a = app();
            a.tab = tab;
            let h = harness_wide(a, width);
            let tabs: Vec<egui::Rect> = h.query_all_by_role(egui::Role::Tab).map(|n| n.rect()).collect();
            assert!(tabs.iter().all(|r| r.right() <= width + 0.5), "tab past the window at {width}");
            let buttons: Vec<(String, egui::Rect)> = h
                .query_all_by_role(egui::Role::Button)
                .filter_map(|n| Some((n.accesskit_node().label()?.to_string(), n.rect())))
                .filter(|(l, r)| {
                    r.top() > 70.0
                        && r.top() < 160.0
                        && r.right() > 110.0
                        && r.left() < width
                        && !l.starts_with("Scroll ribbon")
                })
                .collect();
            assert!(!buttons.is_empty());
            for (i, (a, ra)) in buttons.iter().enumerate() {
                for (b, rb) in buttons.iter().skip(i + 1) {
                    let o = ra.intersect(*rb);
                    assert!(o.width() <= 0.5 || o.height() <= 0.5, "{a} and {b} overlap at {width}, tab {tab}");
                }
            }
        }
    }
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
    // A form, filled from the case's controlDict.
    assert_eq!(
        h.state().dialog.as_ref().unwrap().form.as_ref().unwrap().values.get("endTime").map(String::as_str),
        Some("3600")
    );
    // As JSON (Edit as JSON), not JSON: the dialog stays and says why.
    h.get_by_label("Edit as JSON").click();
    h.run();
    assert_eq!(h.state().dialog.as_ref().unwrap().params.trim(), "{}", "nothing changed yet");
    h.state_mut().dialog.as_mut().unwrap().params = "{endTime: 1}".into();
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().dialog.as_ref().unwrap().error.as_deref().unwrap().starts_with("not JSON"));
    // Back to the form, the end time typed there.
    h.state_mut().dialog.as_mut().unwrap().params = "{}".into();
    // The card settles (its scroll area grows to its height over frames) before its link is
    // clicked.
    let mut last = None;
    for _ in 0..200 {
        h.run();
        let now = h.get_by_label("Back to the form").rect();
        if last == Some(now) {
            break;
        }
        last = Some(now);
    }
    h.get_by_label("Back to the form").click();
    h.run();
    assert!(!h.state().dialog.as_ref().unwrap().form.as_ref().unwrap().json);
    // The end time typed in its field.
    h.state_mut().dialog.as_mut().unwrap().form.as_mut().unwrap().values.insert("endTime", "7200".into());
    h.run();
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().dialog.is_none(), "{:?}", h.state().dialog.as_ref().map(|d| d.error.clone()));
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
    // In the results too: with the name only.
    let results = crate::patch::PatchForm { results: true, ..renamed };
    assert_eq!(results.params(true).unwrap(), json!({ "name": "roof", "results": true }));
    let typed = crate::patch::PatchForm { results: true, ..typed };
    assert!(typed.params(true).unwrap().get("results").is_none());
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

/// While a scene job runs, the window draws from a copy of the session without the scene: the
/// status bar still says which regions are read whole, as they were.
#[test]
fn the_regions_read_whole_are_still_said_while_a_job_runs() {
    let (app, hold, waiting) = held();
    hold.store(false, Ordering::SeqCst);
    let mut h = Harness::builder()
        .with_size(vec2(1200.0, 800.0))
        .with_max_steps(16)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    h.run_steps(2);
    h.state_mut().wait_for_job();
    h.state_mut().toggle_cut();
    h.run_steps(2);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    let said = |h: &Harness<'static, App>| h.query_all_by_label_contains(" read whole · ").count();
    assert_eq!(said(&h), 1);
    // The meshes read again, held at their read: busy, the label stays.
    hold.store(true, Ordering::SeqCst);
    let mut params = h.state().shown();
    params["reload"] = serde_json::json!(true);
    h.state_mut().start(crate::jobs::Work::Scene { params });
    h.run_steps(1);
    wait_until(&waiting);
    h.run_steps(2);
    assert!(h.state().busy());
    assert!(h.state().session().whole_meshes().is_empty(), "the copy holds no scene");
    assert_eq!(said(&h), 1);
    hold.store(false, Ordering::SeqCst);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    assert!(!h.state().busy());
    assert_eq!(said(&h), 1);
    // The cut closed and the meshes read again: nothing held. Then a check run here (no job)
    // hands over the mesh it read, and a job starts before the bar is drawn: still said.
    h.state_mut().toggle_cut();
    h.run_steps(2);
    h.state_mut().wait_for_job();
    let mut params = h.state().shown();
    params["reload"] = serde_json::json!(true);
    h.state_mut().session.scene(&params).unwrap();
    h.run_steps(2);
    assert_eq!(said(&h), 0);
    h.state_mut().session.execute("mesh.check", &serde_json::json!({ "region": "" })).unwrap();
    hold.store(true, Ordering::SeqCst);
    waiting.store(false, Ordering::SeqCst);
    let mut params = h.state().shown();
    params["reload"] = serde_json::json!(true);
    h.state_mut().start(crate::jobs::Work::Scene { params });
    h.run_steps(1);
    wait_until(&waiting);
    h.run_steps(1);
    assert!(h.state().busy());
    assert_eq!(said(&h), 1);
    hold.store(false, Ordering::SeqCst);
    h.state_mut().wait_for_job();
}

/// Another case opened: the bar no longer says what the case before held, even while the new
/// case's scene and fields are read right after (the window busy all along).
#[test]
fn another_case_opened_drops_the_regions_read_whole_of_the_one_before() {
    let dir = std::env::temp_dir().join(format!("umcf-gui-{}-held", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("system")).unwrap();
    std::fs::write(
        dir.join("system/controlDict"),
        format!(
            "{HEADER}application urbanMicroclimateFoam;\nstartTime 0;\nendTime 3600;\ndeltaT 60;\nwriteInterval 600;\n"
        ),
    )
    .unwrap();
    let mut h = harness(app());
    h.state_mut().toggle_cut();
    h.run();
    assert_eq!(h.state().held.len(), 1);
    // Opened as a job, no frame drawn after it: the session's own list already.
    let app = h.state_mut();
    app.folder_picked(crate::Picked::Folder(dir.to_string_lossy().into_owned()));
    app.wait_for_job();
    assert_eq!(app.session().location(), dir.to_str());
    assert!(app.held.is_empty(), "{:?}", app.held);
    let _ = std::fs::remove_dir_all(&dir);
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
    let d = crate::Dialog { id: "control.set", params: "{\"endTime\": 7200}".into(), error: None, form: None };
    app.start(crate::jobs::Work::Command {
        id: "control.set".into(),
        params: json!({ "endTime": 7200 }),
        then: crate::jobs::Then::Dialog(Box::new(d)),
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
            crate::jobs::Work::Bounds { .. } => "bounds".to_string(),
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
        then: crate::jobs::Then::Dialog(Box::new(d.clone())),
    });
    app.wait_for_job();
    assert!(app.dialog.as_ref().and_then(|d| d.error.as_ref()).is_some());
    params["path"] = json!(dir.to_str().unwrap());
    app.start(crate::jobs::Work::Command {
        id: "case.new".into(),
        params,
        then: crate::jobs::Then::Dialog(Box::new(d)),
    });
    app.wait_for_job();
    assert_eq!(app.session().location(), dir.to_str());
    assert_eq!(app.log.last().unwrap().command, "case.new");
    assert!(app.log.last().unwrap().ok);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_job_names_the_folder_it_makes_or_opens() {
    use crate::jobs::folder_name;
    assert_eq!(folder_name("/runs/canyon/"), Some("canyon"));
    assert_eq!(folder_name("C:\\runs\\canyon\\"), Some("canyon"));
    assert_eq!(folder_name("canyon"), Some("canyon"));
    for none in ["", "  ", "/", "C:\\", "C:"] {
        assert_eq!(folder_name(none), None, "{none:?}");
    }
}

/// The status bar says which regions are read whole and what they take: from the cut on, kept
/// aside once the cut is closed, and gone once dropped.
#[test]
fn the_status_bar_says_which_regions_are_read_whole() {
    let mut h = harness(app());
    let read_whole = |h: &Harness<'static, App>| h.query_all_by_label_contains(" read whole · ").count();
    assert_eq!(read_whole(&h), 0);
    h.state_mut().toggle_cut();
    h.run();
    assert_eq!(read_whole(&h), 1);
    let held = h.state().session().whole_meshes();
    assert_eq!(held.len(), 1);
    assert!(held[0].cut);
    h.get_by_label(&format!("mesh read whole · {}", crate::jobs::size(held[0].bytes)));
    // Closed: kept aside, still said.
    h.state_mut().toggle_cut();
    h.run();
    assert_eq!(read_whole(&h), 1);
    assert!(!h.state().session().whole_meshes()[0].cut);
    // Dropped (read again from disk): no longer said.
    let mut reload = h.state().shown();
    reload["reload"] = serde_json::json!(true);
    h.state_mut().session.scene(&reload).unwrap();
    h.run();
    assert_eq!(read_whole(&h), 0);
}

#[test]
fn the_cut_card_hides_the_boundary_and_leaves_the_display_as_it_was() {
    use umcf_engine::{DisplayEntry, Layer};
    use umcf_render::raster::Mode;
    let mut app = app();
    let before = vec![
        DisplayEntry {
            layer: Layer::Patch { region: String::new(), patch: "sky".into() },
            mode: Some(Mode::Wireframe),
            opacity: None,
        },
        DisplayEntry { layer: Layer::Region(String::new()), mode: None, opacity: Some(0.5) },
    ];
    app.display = before.clone();
    app.toggle_cut();
    let cut = app.cut.clone().unwrap();
    assert_eq!(cut.region, "");
    assert_eq!(app.shown()["cuts"][0]["region"], "");
    // Hidden by an entry of its own, last; the others as they were.
    assert_eq!(app.display.len(), 3);
    assert_eq!(app.display[..2], before[..]);
    assert_eq!(app.display[2].mode, Some(Mode::Hidden));
    app.toggle_cut();
    assert!(app.cut.is_none());
    assert_eq!(app.display, before);
    assert!(app.shown().get("cuts").is_none());
    // The region given an opacity while the card shows a cut: the cut's while it shows, and
    // the region's, which it keeps once the card closes.
    app.display.clear();
    app.toggle_cut();
    app.set_display(Layer::Region(String::new()), crate::display::Change::Opacity(0.5));
    assert_eq!(app.shared_cut_entry().and_then(|e| e.opacity), Some(0.5));
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: Layer::Region(String::new()), mode: None, opacity: Some(0.5) }]);
    // A region turned off in the browser: the slider's extent from its whole mesh, read for
    // the cut, the plane then in its middle.
    app.display.clear();
    app.shown_regions.clear();
    app.toggle_cut();
    // Not read yet: no range (not one kept from another region or axis).
    let mut form = app.cut.clone().unwrap();
    form.range = (5.0, 10.0);
    app.fit_cut(&mut form, true);
    assert_eq!(form.range, (0.0, 0.0));
    let params = app.shown();
    app.session.scene(&params).unwrap();
    let mut form = app.cut.clone().unwrap();
    app.fit_cut(&mut form, true);
    assert!(form.range.1 > form.range.0, "{:?}", form.range);
    assert!(form.at > form.range.0 && form.at < form.range.1);
    app.toggle_cut();
    // A moved plane: the view is drawn again.
    app.toggle_cut();
    let look = app.look;
    let mut form = app.cut.clone().unwrap();
    form.at += 0.1;
    app.set_cut(form);
    assert!(app.look > look);
    // Closed after the browser showed the region again (no hiding left): drawn again too.
    app.set_display(Layer::Region(String::new()), crate::display::Change::Mode(Mode::Surface));
    let look = app.look;
    app.toggle_cut();
    assert!(app.look > look);
    // A patch with a mode of its own, then a change in the browser while the card is open: the
    // card's hiding stays last, over the patch.
    app.display = before.clone();
    app.toggle_cut();
    app.set_display(
        Layer::Patch { region: String::new(), patch: "ground".into() },
        crate::display::Change::Opacity(0.3),
    );
    let last = app.display.last().unwrap();
    assert_eq!((&last.layer, last.mode), (&Layer::Region(String::new()), Some(Mode::Hidden)));
    app.toggle_cut();
    assert!(app.display.iter().all(|e| e.mode != Some(Mode::Hidden)));
    // The region given a mode in the browser while the card shows a cut, with or without an
    // entry of its own from before: the cut's cells take that mode, the region's boundary stays
    // hidden (the plane moved after too); closing leaves the region as it was.
    let region = Layer::Region(String::new());
    let cut_layer = Layer::Cut(String::new());
    // The region shown again (turned off above), so that its boundary is in the scene.
    app.shown_regions.push(String::new());
    for start in [Vec::new(), before.clone()] {
        app.display = start.clone();
        app.toggle_cut();
        for m in [Mode::Wireframe, Mode::Points] {
            app.set_display(region.clone(), crate::display::Change::Mode(m));
            let mut form = app.cut.clone().unwrap();
            form.at += 0.1;
            app.set_cut(form);
            let last = app.display.iter().rfind(|e| e.layer == region).unwrap();
            assert_eq!(last.mode, Some(Mode::Hidden), "{:?}", app.display);
            assert_eq!(app.layer_entry(&cut_layer).and_then(|e| e.mode), Some(m), "{:?}", app.display);
            // As the view draws it: the boundary hidden, the cut in that mode.
            let entries = app.view_display();
            let scene = app.session.scene(&app.shown()).unwrap();
            let style = umcf_engine::display_style(scene, &entries);
            assert!(scene.objects.iter().any(|o| matches!(o.source, umcf_render::Source::Cut { .. })));
            assert!(scene.objects.iter().any(|o| matches!(o.source, umcf_render::Source::Mesh { .. })));
            for (k, o) in scene.objects.iter().enumerate() {
                let want = match o.source {
                    umcf_render::Source::Cut { .. } => m,
                    _ => Mode::Hidden,
                };
                for c in 0..o.classes.len() {
                    let got = style.get(&(k, c as u32)).map(|d| d.mode);
                    assert_eq!(got, Some(want), "{:?} class {c}", o.source);
                }
            }
            // The browser's menu for the region shows the cut's mode.
            assert_eq!(app.cut_takes(&region), Some(cut_layer.clone()));
            assert_eq!(app.layer_display(&cut_layer, &[]).mode, m);
        }
        app.toggle_cut();
        assert_eq!(app.display, start, "closed: as before");
    }
    // An opacity goes to the region, and to the cut so that it shows now; a reset takes both
    // out; the hiding stays until the card closes.
    let hidden = |app: &App| app.display.iter().any(|e| e.mode == Some(Mode::Hidden));
    app.display.clear();
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Opacity(0.4));
    assert!(hidden(&app));
    assert_eq!(app.shared_cut_entry().and_then(|e| e.opacity), Some(0.4));
    app.set_display(region.clone(), crate::display::Change::Reset);
    assert!(hidden(&app) && app.layer_entry(&cut_layer).is_none(), "{:?}", app.display);
    app.toggle_cut();
    assert!(app.display.is_empty(), "{:?}", app.display);
    // Closed, the region keeps the opacity set while the cut showed; the cut's goes.
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Opacity(0.4));
    assert_eq!(app.layer_display(&region, &[]).opacity, 0.4);
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: None, opacity: Some(0.4) }]);
    // An opacity the region had before the card opened: the cut's menu shows it and can
    // reset it; a mode of the region's own from before, and the card's hiding, stay.
    app.toggle_cut();
    let cut_now = app.cut_takes(&region).unwrap();
    let (d, set) = app.menu_display(&region, Some(&cut_now));
    assert_eq!((d.opacity, set), (0.4, true));
    // Drawn so too, and noted.
    assert_eq!(app.region_note(""), "cut 40%");
    let (entries, shown) = (app.view_display(), app.shown());
    let scene = app.session.scene(&shown).unwrap();
    let style = umcf_engine::display_style(scene, &entries);
    for (k, o) in scene.objects.iter().enumerate() {
        if matches!(o.source, umcf_render::Source::Cut { .. }) {
            assert!((0..o.classes.len()).all(|c| style.get(&(k, c as u32)).map(|d| d.opacity) == Some(0.4)));
        }
    }
    app.set_display(region.clone(), crate::display::Change::Reset);
    let (d, set) = app.menu_display(&region, Some(&cut_now));
    assert_eq!((d.opacity, set), (1.0, false));
    app.toggle_cut();
    app.display = vec![DisplayEntry { layer: region.clone(), mode: Some(Mode::Wireframe), opacity: Some(0.3) }];
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Reset);
    assert!(hidden(&app), "{:?}", app.display);
    assert_eq!(
        app.display.first(),
        Some(&DisplayEntry { layer: region.clone(), mode: Some(Mode::Wireframe), opacity: None })
    );
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: Some(Mode::Wireframe), opacity: None }]);
    app.display = vec![DisplayEntry { layer: region.clone(), mode: None, opacity: Some(0.4) }];
    // One value: an opacity set while the card shows nothing (the region's own) is the cut's
    // once it shows one again.
    app.display.clear();
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Opacity(0.4));
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    app.set_cut(form);
    assert!(app.cut_takes(&region).is_none());
    app.set_display(region.clone(), crate::display::Change::Opacity(0.7));
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Plane;
    app.set_cut(form);
    assert_eq!(app.shared_cut_entry().and_then(|e| e.opacity), Some(0.7));
    assert_eq!(app.region_note(""), "cut 70%");
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: None, opacity: Some(0.7) }]);
    // A patch's opacity set after the region's, while the card shows the cut: the patch keeps
    // it once the card closes (the region's is in an entry of its own, not the card's hiding).
    app.display.clear();
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Opacity(0.4));
    let patch = Layer::Patch { region: String::new(), patch: "walls".into() };
    app.set_display(patch.clone(), crate::display::Change::Opacity(0.7));
    app.toggle_cut();
    assert_eq!(app.layer_display(&patch, &[]).opacity, 0.7, "{:?}", app.display);
    assert_eq!(app.layer_display(&region, &[]).opacity, 0.4, "{:?}", app.display);
    app.display = vec![DisplayEntry { layer: region.clone(), mode: None, opacity: Some(0.4) }];
    // The region hidden by its own entry before the card opened, then an opacity: the card's
    // hiding stays the card's, so closing leaves the region's own, and a reset takes it all.
    app.display = vec![DisplayEntry { layer: region.clone(), mode: Some(Mode::Hidden), opacity: None }];
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Opacity(0.4));
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: Some(Mode::Hidden), opacity: Some(0.4) }]);
    app.set_display(region.clone(), crate::display::Change::Reset);
    assert!(app.display.is_empty(), "{:?}", app.display);
    app.display = vec![DisplayEntry { layer: region.clone(), mode: None, opacity: Some(0.4) }];
    // The view draws the region so once the card is closed.
    let (entries, shown) = (app.view_display(), app.shown());
    let style = umcf_engine::display_style(app.session.scene(&shown).unwrap(), &entries);
    assert!(style.values().all(|d| d.opacity == 0.4 && d.mode != Mode::Hidden), "{style:?}");
    app.display.clear();
    // While the card shows nothing (no check to show), a mode is the region's own: the hiding
    // goes, and the region keeps the mode once the card closes.
    app.toggle_cut();
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    app.set_cut(form);
    assert!(app.cut_takes(&region).is_none());
    app.set_display(region.clone(), crate::display::Change::Mode(Mode::Wireframe));
    assert!(!hidden(&app) && app.cut.as_ref().unwrap().released);
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: Some(Mode::Wireframe), opacity: None }]);
    // A cut mode, then *Show* switched: it stays with the cut; then the card showing nothing and
    // the region set: the hiding and the cut's mode both go (no menu would reach it).
    app.display.clear();
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Mode(Mode::Points));
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Layers;
    app.set_cut(form);
    assert_eq!(app.layer_entry(&cut_layer).and_then(|e| e.mode), Some(Mode::Points));
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    app.set_cut(form);
    app.set_display(region.clone(), crate::display::Change::Mode(Mode::Surface));
    assert!(!hidden(&app) && app.layer_entry(&cut_layer).is_none(), "{:?}", app.display);
    // Back to a plane: hidden again, the region's menu the cut's; closing leaves the region's
    // own mode, set while the card showed nothing.
    let mut form = app.cut.clone().unwrap();
    form.show = crate::cut::Show::Plane;
    app.set_cut(form);
    assert!(hidden(&app) && !app.cut.as_ref().unwrap().released, "{:?}", app.display);
    assert_eq!(app.cut_takes(&region), Some(cut_layer.clone()));
    app.set_display(region.clone(), crate::display::Change::Mode(Mode::Wireframe));
    assert_eq!(app.layer_entry(&cut_layer).and_then(|e| e.mode), Some(Mode::Wireframe));
    app.toggle_cut();
    assert_eq!(app.display, [DisplayEntry { layer: region.clone(), mode: Some(Mode::Surface), opacity: None }]);
    app.display.clear();
    // A cut mode, then the card moved to another region: the mode goes with the cut before.
    app.toggle_cut();
    app.set_display(region.clone(), crate::display::Change::Mode(Mode::Wireframe));
    assert!(app.layer_entry(&cut_layer).is_some());
    let mut form = app.cut.clone().unwrap();
    form.region = "other".into();
    app.set_cut(form);
    assert_eq!(app.cut.as_ref().unwrap().hid.as_deref(), Some("other"));
    assert!(app.display.iter().all(|e| !matches!(e.layer, Layer::Cut(_))), "{:?}", app.display);
    app.toggle_cut();
}

/// While the cut card shows a cut, the browser's note beside its region is the cut's: `cut`, then
/// `cut wireframe` once the region's menu set that mode; the region's own once the card closes.
#[test]
fn the_browser_notes_the_cut_beside_its_region() {
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    let mut app = app();
    assert_eq!(app.region_note(""), "");
    app.toggle_cut();
    assert_eq!(app.region_note(""), "cut");
    app.set_display(Layer::Region(String::new()), crate::display::Change::Mode(Mode::Wireframe));
    assert_eq!(app.region_note(""), "cut wireframe");
    app.set_display(Layer::Region(String::new()), crate::display::Change::Opacity(0.4));
    assert_eq!(app.region_note(""), "cut wireframe 40%");
    app.toggle_cut();
    // The opacity is the region's too, and stays.
    assert_eq!(app.region_note(""), "40%");
    app.set_display(Layer::Region(String::new()), crate::display::Change::Reset);
    assert_eq!(app.region_note(""), "");
    // The region's own mode, without the card.
    app.set_display(Layer::Region(String::new()), crate::display::Change::Mode(Mode::Points));
    assert_eq!(app.region_note(""), "points");
}

/// The browser ended the cut card's hiding while it showed nothing (no check yet); then the check
/// runs and its findings show, with no change of the card: the region is hidden again and its
/// menu is the cut's.
#[test]
fn findings_that_come_hide_the_region_again() {
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    // A checkMesh set of one face, for the check to find.
    let mut fs = case();
    fs.insert("constant/polyMesh/sets/skewFaces", format!("{}1\n(\n2\n)\n", h("faceSet")));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut h = harness(App::new(s));
    h.state_mut().toggle_cut();
    h.run();
    let mut form = h.state().cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    h.state_mut().set_cut(form);
    h.run();
    let region = Layer::Region(String::new());
    h.state_mut().set_display(region.clone(), crate::display::Change::Mode(Mode::Surface));
    h.run();
    let hidden = |app: &App| app.display.iter().any(|e| e.mode == Some(Mode::Hidden));
    assert!(!hidden(h.state()) && h.state().cut.as_ref().unwrap().released);
    // The check's findings come (here straight through the session: no change of the card).
    h.state_mut().session.execute("mesh.check", &serde_json::json!({ "region": "" })).unwrap();
    assert!(h.state().session().found("").is_some_and(|f| !f.classes.is_empty()));
    assert!(h.state().cut_shown().is_some());
    h.run();
    assert!(hidden(h.state()) && !h.state().cut.as_ref().unwrap().released, "{:?}", h.state().display);
    assert_eq!(h.state().cut_takes(&region), Some(Layer::Cut(String::new())));
}

/// A check that found nothing shows nothing: the region's boundary stays as the browser set it
/// (not hidden), its menu the region's; the empty findings are still asked for.
#[test]
fn a_check_that_found_nothing_leaves_the_region_shown() {
    use umcf_engine::Layer;
    use umcf_render::raster::Mode;
    let mut h = harness(app());
    h.state_mut().toggle_cut();
    h.run();
    let mut form = h.state().cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    h.state_mut().set_cut(form);
    h.run();
    let region = Layer::Region(String::new());
    h.state_mut().set_display(region.clone(), crate::display::Change::Mode(Mode::Wireframe));
    // The cube has nothing to find.
    h.state_mut().session.execute("mesh.check", &serde_json::json!({ "region": "" })).unwrap();
    assert!(h.state().session().found("").is_some_and(|f| f.classes.is_empty()));
    h.run();
    let app = h.state();
    assert!(app.cut_shown().is_some() && !app.cut_visible());
    assert!(app.cut.as_ref().unwrap().released, "{:?}", app.display);
    assert!(app.display.iter().all(|e| e.mode != Some(Mode::Hidden)), "{:?}", app.display);
    assert!(app.cut_takes(&region).is_none());
    assert_eq!(app.layer_display(&region, &[]).mode, Mode::Wireframe);
    // Without a mode of the browser's, the card's hiding is left out of the view while it shows
    // nothing, as before a check.
    h.state_mut().toggle_cut();
    h.state_mut().display.clear();
    h.state_mut().toggle_cut();
    let mut form = h.state().cut.clone().unwrap();
    form.show = crate::cut::Show::Found;
    h.state_mut().set_cut(form);
    h.run();
    assert!(h.state().idle_hiding().is_some());
    assert!(h.state().view_display().iter().all(|e| e.mode != Some(Mode::Hidden)));
    // Nothing turned on in the browser: the view says so, rather than an empty scene.
    h.state_mut().shown_regions.clear();
    h.run();
    let _ = h.get_by_label_contains("Nothing shown");
}

#[test]
fn the_cut_card_shows_what_check_mesh_found() {
    let mut h = harness(app());
    h.state_mut().toggle_cut();
    h.run();
    h.get_by_label("Check findings").click();
    h.run();
    // Not checked yet: nothing to cut, no plane to drag; the card says so.
    assert_eq!(h.state().cut.as_ref().unwrap().show, crate::cut::Show::Found);
    assert!(h.state().shown().get("cuts").is_none());
    assert!(h.state().cut.as_ref().unwrap().plane().is_none());
    let _ = h.get_by_label_contains("No check of this region to show");
    h.get_by_label("Check mesh").click();
    h.run();
    h.state_mut().wait_for_job();
    h.run_steps(3);
    // Checked: the region's findings are the cut (the canyon's mesh is fine: none listed).
    assert_eq!(h.state().shown()["cuts"], json!([{ "region": "", "found": true }]));
    assert!(h.state().session().found("").is_some());
    let _ = h.get_by_label_contains("found nothing here");
    let params = h.state().shown();
    let scene = h.state_mut().session.scene(&params).unwrap();
    assert!(scene.objects.iter().any(|o| o.source == umcf_render::Source::Cut { region: String::new() }));
    // Back to the plane.
    h.get_by_label("Plane").click();
    h.run();
    assert!(h.state().shown()["cuts"][0].get("point").is_some());
}

#[test]
fn the_cut_card_shows_the_layers_at_a_wall() {
    let mut h = harness(app());
    h.state_mut().toggle_cut();
    h.run();
    // The plane's cut built first: its class (`cells`) is no layer.
    h.state_mut().wait_for_job();
    h.run_steps(3);
    assert!(h.state().session().last_cut("").is_some());
    h.get_by_label("Wall layers").click();
    h.run();
    assert!(h.query_by_label("cells").is_none());
    // At the region's first wall, five layers; no plane to drag.
    let patches = h.state().patches("");
    let form = h.state().cut.clone().unwrap();
    assert_eq!((form.show, form.patch.as_str(), form.layers), (crate::cut::Show::Layers, patches[0].as_str(), 5));
    assert!(form.plane().is_none());
    assert_eq!(h.state().shown()["cuts"], json!([{ "region": "", "patch": patches[0], "layers": 5 }]));
    h.state_mut().wait_for_job();
    h.run_steps(3);
    // Each layer listed with its count.
    let _ = h.get_by_label_contains("layer 1: ");
    // A patch the region doesn't have shows nothing (the card takes its first again).
    let mut form = h.state().cut.clone().unwrap();
    form.patch = "nowhere".into();
    h.state_mut().cut = Some(form);
    assert!(h.state().shown().get("cuts").is_none());
    h.run();
    assert_eq!(h.state().cut.as_ref().unwrap().patch, patches[0]);
    // `--open layers`.
    let mut app = self::app();
    app.start_up(Some("layers")).unwrap();
    assert_eq!(app.cut.as_ref().unwrap().show, crate::cut::Show::Layers);
}

#[test]
fn the_cut_card_shows_the_cells_near_a_surface() {
    let mut fs = case();
    fs.insert(
        "constant/triSurface/floor.stl",
        "solid floor\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid floor\n",
    );
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs.clone()), "mem", OpenOptions::default(), false).unwrap();
    let mut h = harness(App::new(s));
    h.state_mut().toggle_cut();
    h.run();
    h.get_by_label("Near a surface").click();
    h.run();
    // The case's surface, within 1 m; no plane to drag.
    let form = h.state().cut.clone().unwrap();
    assert_eq!(
        (form.show, form.surface.as_str(), form.distance),
        (crate::cut::Show::Near, "constant/triSurface/floor.stl", 1.0)
    );
    assert!(form.plane().is_none());
    assert_eq!(
        h.state().shown()["cuts"],
        json!([{ "region": "", "surface": "constant/triSurface/floor.stl", "distance": 1.0 }])
    );
    h.state_mut().wait_for_job();
    h.run_steps(3);
    let _ = h.get_by_label("cells about 1 m across: 1");
    // Fit cells frames them: the one cell.
    h.get_by_label("Fit cells").click();
    h.run();
    let target = h.state().viewport.camera.unwrap().target;
    assert!((target.x - 0.5).abs() < 1e-9 && (target.z - 0.5).abs() < 1e-9, "{target:?}");
    // `--open near`.
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut app = App::new(s);
    app.start_up(Some("near")).unwrap();
    assert_eq!(app.cut.as_ref().unwrap().show, crate::cut::Show::Near);
    // A case without a surface: nothing to show, and the card says so.
    let mut h = harness(self::app());
    h.state_mut().toggle_cut();
    h.run();
    h.get_by_label("Near a surface").click();
    h.run();
    assert!(h.state().shown().get("cuts").is_none());
    let _ = h.get_by_label_contains("no STL surface");
    // Nothing to show: the region's boundary isn't hidden from the view (the card keeps its
    // hiding, which comes back once there is something to show).
    let hidden = |d: &[umcf_engine::DisplayEntry]| d.iter().any(|e| e.mode == Some(umcf_render::raster::Mode::Hidden));
    assert!(hidden(&h.state().display));
    assert!(!hidden(&h.state().view_display()));
    // The browser says as the view draws it.
    let region = umcf_engine::Layer::Region(String::new());
    assert_ne!(h.state().layer_display(&region, &[]).mode, umcf_render::raster::Mode::Hidden);
    // The region's row paints its note (`layer_note`) after its patches: not hidden.
    assert!(!h.state().layer_note(&region).contains("hidden"));
    // A mode of the region's own, then the card showing something: its row says what the view
    // draws, the card's hiding.
    h.state_mut().display.insert(
        0,
        umcf_engine::DisplayEntry {
            layer: region.clone(),
            mode: Some(umcf_render::raster::Mode::Wireframe),
            opacity: None,
        },
    );
    // Idle: the region's own mode.
    assert_eq!(h.state().layer_note(&region), "wireframe");
    h.get_by_label("Plane").click();
    h.run();
    assert_eq!(h.state().layer_note(&region), "hidden");
    h.get_by_label("Plane").click();
    h.run();
    assert!(hidden(&h.state().view_display()));
}

#[test]
fn the_view_frames_what_was_found() {
    // checkMesh's set: the bottom face (z = 0) of the one cell.
    let mut fs = case();
    fs.insert("constant/polyMesh/sets/skewFaces", format!("{}1\n(\n0\n)\n", h("faceSet")));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut app = App::new(s);
    app.run("mesh.check", json!({})).unwrap();
    app.start_up(Some("found")).unwrap();
    let mut h = harness(app);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    // On the face, not the cell's middle.
    let target = h.state().viewport.camera.unwrap().target;
    assert!(target.z.abs() < 1e-9 && (target.x - 0.5).abs() < 1e-9, "{target:?}");
    let _ = h.get_by_label_contains("faces with skewness above 4: 1 face");
    // Fit (F) frames the whole scene again.
    h.state_mut().fit_view(None);
    assert!((h.state().viewport.camera.unwrap().target.z - 0.5).abs() < 1e-9);
    // The card's Fit: on what was found again.
    h.get_by_label("Fit findings").click();
    h.run();
    assert!(h.state().viewport.camera.unwrap().target.z.abs() < 1e-9);
}

#[test]
fn a_whole_scene_fit_asked_after_a_waiting_findings_fit_frames_the_scene() {
    let mut fs = case();
    fs.insert("constant/polyMesh/sets/skewFaces", format!("{}1\n(\n0\n)\n", h("faceSet")));
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.open(Box::new(fs), "mem", OpenOptions::default(), false).unwrap();
    let mut app = App::new(s);
    app.run("mesh.check", json!({})).unwrap();
    // The findings fit waits for the scene, read as a job; Fit (F) is asked meanwhile.
    app.start_up(Some("found")).unwrap();
    assert!(app.fit_found);
    app.fit_view(None);
    let mut h = harness(app);
    h.state_mut().wait_for_job();
    h.run_steps(2);
    // The whole cell, not the face found.
    let target = h.state().viewport.camera.unwrap().target;
    assert!((target.z - 0.5).abs() < 1e-9, "{target:?}");
    assert!(!h.state().fit_found);
}

#[test]
fn no_cut_opens_where_no_case_is_open() {
    let mut app = App::new(Session::with_backup(".umcfgui/backup/test"));
    assert!(!app.can_cut());
    let e = app.start_up(Some("cut")).unwrap_err();
    assert!(e.contains("--open cut: no case is open"), "{e}");
    assert!(app.cut.is_none());
    let e = app.start_up(Some("found")).unwrap_err();
    assert!(e.contains("--open found: no case is open"), "{e}");
    let e = app.start_up(Some("layers")).unwrap_err();
    assert!(e.contains("--open layers: no case is open"), "{e}");
    assert!(self::app().can_cut());
    // `found`: the card showing what Check mesh found.
    let mut app = self::app();
    app.run("mesh.check", json!({})).unwrap();
    app.start_up(Some("found")).unwrap();
    assert_eq!(app.cut.as_ref().unwrap().show, crate::cut::Show::Found);
    assert_eq!(app.shown()["cuts"], json!([{ "region": "", "found": true }]));
}

#[test]
fn the_cut_s_plane_is_dragged_by_its_arrow() {
    let mut h = harness(app());
    h.state_mut().toggle_cut();
    h.run();
    h.run();
    let form = h.state().cut.clone().unwrap();
    let plane = form.plane().expect("the region's extent is known once its scene is drawn");
    let handles = plane.handles().unwrap();
    let camera = h.state().viewport.camera.unwrap();
    let rect = h.state().viewport.rect.unwrap();
    // Taken by the arrow, half way along it, and pulled on along it on the screen: the plane
    // moves along its normal, the camera stays.
    let screen = |p| crate::viewport::to_screen(&camera, rect, p).unwrap();
    let (base, tip) = (screen(handles.base), screen(handles.tip));
    let start = base + (tip - base) * 0.5;
    assert!(crate::viewport::on_handle(&camera, rect, &handles, start));
    drag(&mut h, start, start + (tip - base) * 0.3, Modifiers::NONE);
    let moved = h.state().cut.clone().unwrap();
    assert!(moved.at > form.at, "{} → {}", form.at, moved.at);
    assert!(moved.placed);
    assert!(moved.at <= moved.range.1);
    assert_eq!(h.state().viewport.camera, Some(camera));
    // It followed the pointer: the arrow's middle is where it was let go (within a few points).
    let now = moved.plane().unwrap().handles().unwrap();
    let middle = screen(now.base) + (screen(now.tip) - screen(now.base)) * 0.5;
    let end = start + (tip - base) * 0.3;
    assert!(middle.distance(end) < 3.0, "{middle:?} {end:?}");
    // Away from the handles a drag turns the view, the plane staying.
    let away = rect.left_top() + vec2(30.0, rect.height() - 30.0);
    assert!(!crate::viewport::on_handle(&camera, rect, &handles, away));
    drag(&mut h, away, away + vec2(40.0, 0.0), Modifiers::NONE);
    assert_eq!(h.state().cut.as_ref().unwrap().at, moved.at);
    assert_ne!(h.state().viewport.camera, Some(camera));
}

/// The open card's form follows the case: the run controls left alone show what the case has
/// after a command changed it; one typed in stays.
#[test]
fn a_form_follows_the_case() {
    let mut h = harness(app());
    h.state_mut().invoke("control.set");
    h.run();
    let form = |h: &Harness<'_, App>, k: &str| {
        h.state().dialog.as_ref().unwrap().form.as_ref().unwrap().values.get(k).cloned().unwrap_or_default()
    };
    assert_eq!(form(&h, "endTime"), "3600");
    h.state_mut().dialog.as_mut().unwrap().form.as_mut().unwrap().values.insert("deltaT", "5".into());
    h.state_mut().act("control.set", json!({"endTime": 9000, "deltaT": 7}));
    for _ in 0..20 {
        h.run();
        if !h.state().working() {
            break;
        }
    }
    assert_eq!(form(&h, "endTime"), "9000");
    assert_eq!(form(&h, "deltaT"), "5", "typed: kept");
    let p =
        h.state().dialog.as_ref().unwrap().form.as_ref().unwrap().params(crate::forms::spec("control.set").unwrap());
    assert_eq!(p.unwrap(), json!({"deltaT": 5}));
}

/// The box card: filled from the region's box, the box previewed in the view (both renderers
/// draw it from `viewport.blocks`) with its size and cells said, then made by OK.
#[test]
fn the_box_card_previews_the_box_and_makes_it() {
    let dir = std::env::temp_dir().join(format!("umcf-ui-{}-box", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::with_backup(".umcfgui/backup/test");
    s.execute(
        "case.new",
        &json!({"path": dir.to_str().unwrap(), "version": 12, "regions": [{"name": "air", "kind": "fluid"}]}),
    )
    .unwrap();
    s.execute("geometry.trees", &json!({"trees": [{"x": 10, "y": 20, "z": 0, "diameter": 4, "height": 6}]})).unwrap();
    let mut h = harness(App::new(s));
    h.state_mut().invoke("mesh.box");
    h.run();
    let form = |h: &Harness<'_, App>| h.state().dialog.as_ref().unwrap().form.clone().unwrap();
    assert_eq!(form(&h).values.get("region").map(String::as_str), Some("air"));
    // Without margins: no preview, and the card says what it needs.
    assert!(h.state().viewport.blocks.is_none());
    assert!(matches!(&h.state().box_summary, Some(Err(e)) if e.contains("six margins")));
    {
        let f = h.state_mut().dialog.as_mut().unwrap().form.as_mut().unwrap();
        f.values.insert("margin", "10|10|10|10|0|10".into());
        f.values.insert("cellSize", "2".into());
    }
    h.run();
    // Another command (a selection) keeps what was typed.
    h.state_mut().act("select.clear", json!({}));
    h.run();
    assert_eq!(form(&h).values.get("margin").map(String::as_str), Some("10|10|10|10|0|10"));
    assert_eq!(form(&h).values.get("cellSize").map(String::as_str), Some("2"));
    let blocks = h.state().viewport.blocks.clone().unwrap_or_else(|| panic!("no preview: {:?}", h.state().box_summary));
    // 8 .. 12 in x with 10 m around: -2 .. 22, 12 cells of 2.
    assert_eq!(blocks.stations[0].len(), 2);
    assert_eq!(blocks.cells[0].len(), 13, "{:?}", blocks.cells[0]);
    let summary = h.state().box_summary.clone().unwrap().unwrap();
    assert!(summary.contains("cells; the ground: ground"), "{summary}");
    // OK: the box made, the card closed, the preview gone.
    h.get_by_label("OK").click();
    for _ in 0..20 {
        h.run();
        if !h.state().working() {
            break;
        }
    }
    assert!(h.state().dialog.is_none(), "{:?}", h.state().dialog.as_ref().map(|d| d.error.clone()));
    assert!(h.state().viewport.blocks.is_none());
    let d = h.state().session().case().unwrap().description().unwrap().0;
    assert!(d.region("air").unwrap().mesh_box.is_some());
    // Edited as JSON, a region named there keeps the JSON's margins and cells.
    h.state_mut().invoke("mesh.box");
    h.run();
    h.get_by_label("Edit as JSON").click();
    h.run();
    h.state_mut().dialog.as_mut().unwrap().params =
        r#"{"region": "street", "margin": [1, 1, 1, 1, 0, 1], "cellSize": 3}"#.into();
    let mut last = None;
    for _ in 0..200 {
        h.run();
        let now = h.get_by_label("Back to the form").rect();
        if last == Some(now) {
            break;
        }
        last = Some(now);
    }
    h.get_by_label("Back to the form").click();
    h.run();
    h.run();
    assert_eq!(form(&h).values.get("region").map(String::as_str), Some("street"));
    assert_eq!(form(&h).values.get("margin").map(String::as_str), Some("1|1|1|1|0|1"));
    assert_eq!(form(&h).values.get("cellSize").map(String::as_str), Some("3"));
    // The preview is of what OK sends: one margin from the JSON previews; a cell size that
    // isn't a number, or a region the description doesn't have, doesn't.
    let set_json = |h: &mut Harness<'_, App>, p: serde_json::Value| {
        let d = h.state_mut().dialog.as_mut().unwrap();
        let spec = crate::forms::spec("mesh.box").unwrap();
        let f = d.form.as_mut().unwrap();
        f.fill(spec, &p).unwrap();
        f.filled_for = f.values.get("region").cloned();
    };
    set_json(&mut h, json!({"region": "air", "margin": 5, "cellSize": 1}));
    h.run();
    assert!(h.state().viewport.blocks.is_some(), "{:?}", h.state().box_summary);
    set_json(&mut h, json!({"region": "air", "margin": 5, "cellSize": "two", "cells": null}));
    h.run();
    assert!(h.state().viewport.blocks.is_none());
    set_json(&mut h, json!({"region": "air", "margin": [1, 1, 1, 1, 0, 1, "bad"], "cellSize": 1}));
    h.run();
    assert!(matches!(&h.state().box_summary, Some(Err(e)) if e.contains("Margins")), "{:?}", h.state().box_summary);
    set_json(&mut h, json!({"region": "air", "margin": -1, "cellSize": 1}));
    h.run();
    assert!(matches!(&h.state().box_summary, Some(Err(e)) if e.contains("margin")), "{:?}", h.state().box_summary);
    set_json(&mut h, json!({"region": "nowhere", "margin": 5, "cellSize": 1}));
    h.run();
    assert!(matches!(&h.state().box_summary, Some(Err(e)) if e.contains("isn't in system/umcfWorkflow")));
    h.state_mut().dialog = None;
    h.run();
    // A read cancelled is read again when the card opens again.
    let key = h.state().box_bounds.as_ref().map(|(s, r, _)| (s.clone(), *r)).unwrap();
    h.state_mut().box_bounds = Some((key.0, key.1, Err("Reading the surfaces cancelled".into())));
    h.run();
    assert!(h.state().box_bounds.is_none(), "dropped when the card is closed");
    // Opened again: filled from the region's box; untouched, it follows the case (the box
    // undone: emptied), ticked surfaces count as typed.
    h.state_mut().invoke("mesh.box");
    h.run();
    assert_eq!(form(&h).values.get("margin").map(String::as_str), Some("10|10|10|10|0|10"));
    assert_eq!(form(&h).values.get("cellSize").map(String::as_str), Some("2"));
    assert!(h.state().viewport.blocks.is_some());
    assert!(!form(&h).typed());
    // Blocks given otherwise since (the box taken out): OK needs reshape, and the card says so.
    h.state_mut().act("mesh.block", json!({"region": "air", "x": [0, [3, 1], 30]}));
    h.run();
    h.run();
    assert!(
        matches!(&h.state().box_summary, Some(Err(e)) if e.contains("Replace blocks not made from the box")),
        "{:?}",
        h.state().box_summary
    );
    h.state_mut().act("edit.undo", json!({}));
    h.run();
    // And the box itself undone.
    h.state_mut().act("edit.undo", json!({}));
    h.run();
    h.run();
    assert_eq!(form(&h).values.get("cellSize").map(String::as_str), Some(""), "refilled: the box undone");
    assert!(h.state().viewport.blocks.is_none());
    h.state_mut()
        .dialog
        .as_mut()
        .unwrap()
        .form
        .as_mut()
        .unwrap()
        .lists
        .entry("surfaces")
        .or_default()
        .insert("trees.stl".into());
    assert!(form(&h).typed());
    let _ = std::fs::remove_dir_all(&dir);
}
