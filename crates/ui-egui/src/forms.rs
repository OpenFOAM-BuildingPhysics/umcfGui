//! Forms for the commands whose card used to take JSON: each command's fields (`Spec`), the
//! state of a card's form ([`FormState`]), the parameters built from it, and the drawing.
//!
//! A form fills the same parameters the JSON did: each field is one key, left out while it is
//! empty (the command's default). *Edit as JSON* shows those parameters as text, for what a form
//! doesn't cover; back on the form, the keys it has go into its fields and the rest are kept and
//! sent as they are. A refusal that names a parameter to pass (`pass "replace": true`) offers it
//! as a checkbox under the error.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, RichText, Ui, vec2};
use serde_json::{Map, Value, json};

/// What a field takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    /// Text, with a hint shown while it is empty.
    Text(&'static str),
    /// A number (its unit or default as the hint).
    Number(&'static str),
    /// A whole number of 0 or more.
    Count(&'static str),
    /// A checkbox, off by default: sent as `true` when ticked.
    Flag,
    /// A checkbox, on by default: sent as `false` when unticked.
    FlagOn,
    /// One of these words; the first, when empty, is the command's default (left out).
    Choice(&'static [&'static str]),
    /// One STL surface of the case (`constant/triSurface`), by file name.
    Surface,
    /// Several STL surfaces of the case: a list (none ticked: left out).
    Surfaces,
    /// Several mesh regions of the case: a list (none ticked: left out).
    Regions,
    /// A file anywhere, its path typed or chosen in the system's dialog (*Browse…*), with
    /// these extensions offered.
    File(&'static [&'static str]),
    /// A region of the workflow description (`system/umcfWorkflow`).
    WorkflowRegion,
    /// Numbers, one per label (a box's six margins): a list, sent when every one is given.
    Numbers(&'static [&'static str]),
    /// Whole numbers, one per label (the cells along x, y and z): a list, as [`Kind::Numbers`].
    Counts(&'static [&'static str]),
}

/// The text of a [`Kind::Numbers`] or [`Kind::Counts`] field: its parts, one per label.
const PARTS: char = '|';

fn parts(text: &str, n: usize) -> Vec<String> {
    let mut v: Vec<String> = text.split(PARTS).map(str::to_string).collect();
    v.resize(n, String::new());
    v
}

/// One field: the parameter it sets, its label, what it takes, and a tooltip.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub tip: &'static str,
}

/// A command's form.
#[derive(Debug)]
pub(crate) struct Spec {
    pub id: &'static str,
    /// A line above the fields.
    pub intro: &'static str,
    pub fields: &'static [Field],
    /// Only the fields changed from what the form was filled with are sent (`control.set`:
    /// the entries as the case has them, set again only where changed).
    pub changed_only: bool,
}

const fn field(key: &'static str, label: &'static str, kind: Kind, tip: &'static str) -> Field {
    Field { key, label, kind, tip }
}

// Both cases: the dialogs on Linux match the pattern as written (OpenFOAM takes only the
// lower-case ones; geometry.import names the copy so).
const STL: &[&str] = &["stl", "stlb", "gz", "STL", "STLB", "GZ"];

static SPECS: &[Spec] = &[
    Spec {
        id: "geometry.import",
        intro: "Copies an STL file into constant/triSurface (Save writes it).",
        fields: &[
            field("path", "File", Kind::File(STL), "The STL file (ASCII or binary, gzip-compressed too)"),
            field(
                "name",
                "Name",
                Kind::Text("the file's own"),
                "Its name in constant/triSurface (.stl, .stlb, .stl.gz or .stlb.gz)",
            ),
            field("scale", "Scale", Kind::Number("1 (0.001 for mm)"), "A factor for the coordinates"),
            field("replace", "Replace a surface of that name", Kind::Flag, ""),
        ],
        changed_only: false,
    },
    Spec {
        id: "geometry.trees",
        intro: "Tree crowns from a table, one tree a line: x y z CD CH (the crown's base, diameter and height).",
        fields: &[
            field("table", "Table in the case", Kind::Text("a file of the case"), "Give this or a table file"),
            field("path", "Table file", Kind::File(&["txt", "csv", "dat"]), "A file outside the case, instead"),
            field("name", "Surface", Kind::Text("trees.stl"), "Its name in constant/triSurface"),
            field("solid", "Solid", Kind::Text("trees"), "The surface's one region"),
            field("replace", "Replace a surface of that name", Kind::Flag, ""),
        ],
        changed_only: false,
    },
    Spec {
        id: "geometry.repair",
        intro: "The safe repairs of a surface; with none ticked, degenerate triangles and orientation.",
        fields: &[
            field("file", "Surface", Kind::Surface, "A surface of the case"),
            field(
                "weld",
                "Weld within",
                Kind::Number("m"),
                "Each vertex within this distance of one before it is moved onto it",
            ),
            field("degenerate", "Remove degenerate and duplicate triangles", Kind::Flag, ""),
            field("orient", "Orient the normals (closed parts outward)", Kind::Flag, ""),
        ],
        changed_only: false,
    },
    Spec {
        id: "geometry.scale",
        intro: "Multiplies a surface's coordinates.",
        fields: &[
            field("file", "Surface", Kind::Surface, "A surface of the case"),
            field("factor", "Factor", Kind::Number("0.001 for mm to m"), "A positive number"),
        ],
        changed_only: false,
    },
    Spec {
        id: "control.set",
        intro: "The run controls of system/controlDict; only those changed are set.",
        fields: &[
            field("startFrom", "Start from", Kind::Choice(&["", "startTime", "firstTime", "latestTime"]), ""),
            field("startTime", "Start time", Kind::Number("s"), ""),
            field("stopAt", "Stop at", Kind::Choice(&["", "endTime", "writeNow", "noWriteNow", "nextWrite"]), ""),
            field("endTime", "End time", Kind::Number("s"), ""),
            field("deltaT", "Time step", Kind::Number("s"), ""),
            field(
                "writeControl",
                "Write control",
                Kind::Choice(&["", "timeStep", "runTime", "adjustableRunTime", "cpuTime", "clockTime"]),
                "",
            ),
            field(
                "writeInterval",
                "Write interval",
                Kind::Number(""),
                "In steps or seconds, as the write control says",
            ),
            field("purgeWrite", "Keep times", Kind::Count("0: all"), "How many written times are kept"),
            field("writeFormat", "Write format", Kind::Choice(&["", "ascii", "binary"]), ""),
            field("writePrecision", "Write precision", Kind::Count("6"), ""),
            field("writeCompression", "Compression", Kind::Choice(&["", "off", "on"]), ""),
            field("timeFormat", "Time format", Kind::Choice(&["", "general", "fixed", "scientific"]), ""),
            field("timePrecision", "Time precision", Kind::Count("6"), ""),
            field("runTimeModifiable", "Read changes while running", Kind::Choice(&["", "true", "false"]), ""),
        ],
        changed_only: true,
    },
    Spec {
        id: "mesh.box",
        intro: "A box around the case's surfaces: the region's blocks, and its blockMeshDict.",
        fields: &[
            field("region", "Region", Kind::WorkflowRegion, "A region of system/umcfWorkflow"),
            field("surfaces", "Surfaces", Kind::Surfaces, "None ticked: the box's, else every surface"),
            field(
                "margin",
                "Margins",
                Kind::Numbers(&["West", "East", "South", "North", "Ground", "Top"]),
                "How far the box reaches past the surfaces on each side (m): west/east along x, south/north along y, ground/top along z",
            ),
            field("cellSize", "Cell size", Kind::Number("m"), "Cubes; or the cells along each axis below"),
            field("cells", "Cells", Kind::Counts(&["x", "y", "z"]), "Instead of a cell size"),
            field(
                "reshape",
                "Replace blocks not made from the box",
                Kind::Flag,
                "Blocks given by Block mesh, read by Write workflow description, or edited since the box made them",
            ),
            field(
                "replace",
                "Write over a blockMeshDict edited by hand",
                Kind::Flag,
                "Save keeps the one there in the backup",
            ),
        ],
        changed_only: false,
    },
    Spec {
        id: "scene.snapshot",
        intro: "A PNG of the case in 3D, drawn by the CPU renderer.",
        fields: &[
            field("regions", "Regions", Kind::Regions, "None ticked: every region with a mesh"),
            field("surfaces", "Surfaces", Kind::Surfaces, "None ticked: every surface, when no region is drawn"),
            field("view", "View", Kind::Choice(&["", "iso", "top", "bottom", "front", "back", "left", "right"]), ""),
            field("zoom", "Zoom", Kind::Number("1: the whole scene"), ""),
            field("width", "Width", Kind::Count("1280 px"), "At most 8192, and 4096 × 4096 together"),
            field("height", "Height", Kind::Count("800 px"), "At most 8192, and 4096 × 4096 together"),
            field("colour_by", "Colour by", Kind::Choice(&["", "patch", "object"]), ""),
            field("edges", "Draw the faces' edges", Kind::Flag, ""),
            field("grid", "Grid", Kind::FlagOn, "A grid on the ground"),
            field("origin", "Origin", Kind::FlagOn, "The origin's planes and axes"),
            field("dark", "Dark background", Kind::FlagOn, ""),
        ],
        changed_only: false,
    },
];

/// The form of command `id`, if it has one.
pub(crate) fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

/// A card's form: the text of each field, the lists ticked, what it was filled with, the
/// parameters a refusal offered and those ticked, and the keys it doesn't show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormState {
    pub(crate) values: BTreeMap<&'static str, String>,
    pub(crate) lists: BTreeMap<&'static str, BTreeSet<String>>,
    pub(crate) initial: BTreeMap<&'static str, String>,
    /// Parameters a refusal named (`replace`, `reshape`, …), and those ticked.
    pub(crate) offered: Vec<String>,
    pub(crate) passed: BTreeSet<String>,
    /// The labels of the form's fields a refusal names to go on (`replace`: *Replace a surface
    /// of that name*).
    pub(crate) pointed: Vec<&'static str>,
    /// Keys of the JSON the form has no field for: sent as they are.
    pub(crate) extra: Map<String, Value>,
    /// Shown as JSON text (the card's `params`) instead of the fields.
    pub(crate) json: bool,
    /// What the surfaces and regions were when the card opened.
    pub(crate) surfaces: Vec<String>,
    pub(crate) regions: Vec<String>,
    /// The regions of the workflow description, for a [`Kind::WorkflowRegion`].
    pub(crate) workflow_regions: Vec<String>,
    /// The region the form was last filled for (the box card fills its fields from the
    /// region's box when the region changes).
    pub(crate) filled_for: Option<String>,
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

impl FormState {
    /// A form with `values` as its fields' text, and these as what it was filled with.
    pub(crate) fn filled(values: BTreeMap<&'static str, String>) -> Self {
        FormState { initial: values.clone(), values, ..FormState::default() }
    }

    /// The parameters the form makes; an error naming a field whose text isn't what it takes.
    pub(crate) fn params(&self, spec: &Spec) -> Result<Value, String> {
        let mut out = self.extra.clone();
        for f in spec.fields {
            if let Kind::Surfaces | Kind::Regions = f.kind {
                let list: Vec<&String> = self.lists.get(f.key).map(|s| s.iter().collect()).unwrap_or_default();
                // None ticked: left out (or what the JSON had that a list can't hold, kept).
                if !list.is_empty() {
                    out.insert(f.key.to_string(), json!(list));
                }
                continue;
            }
            if let Kind::Numbers(labels) | Kind::Counts(labels) = f.kind {
                let v = parts(self.values.get(f.key).map(String::as_str).unwrap_or(""), labels.len());
                if v.iter().all(|p| p.trim().is_empty()) {
                    continue;
                }
                let mut list = Vec::with_capacity(v.len());
                for (p, label) in v.iter().zip(labels.iter()) {
                    let p = p.trim();
                    let n = match f.kind {
                        Kind::Counts(_) => p.parse::<u64>().ok().map(|n| json!(n)),
                        _ => serde_json::from_str::<Value>(p).ok().filter(Value::is_number),
                    };
                    match n {
                        Some(n) => list.push(n),
                        None if p.is_empty() => {
                            return Err(format!("{}: give all {} (none for the default)", f.label, labels.len()));
                        }
                        None => return Err(format!("{} ({label}): `{p}` isn't a number", f.label)),
                    }
                }
                out.insert(f.key.to_string(), Value::Array(list));
                continue;
            }
            let text = self.values.get(f.key).map(|s| s.trim()).unwrap_or("");
            // A value from the JSON the field can't hold, kept aside (`extra`) and the field
            // left empty: sent as it is, for the command to say what is wrong with it.
            if text.is_empty() && self.extra.contains_key(f.key) {
                continue;
            }
            let initial = self.initial.get(f.key).map(|s| s.trim()).unwrap_or("");
            if spec.changed_only && initial == text {
                continue;
            }
            // An entry the case has, emptied (or set to default): nothing would change it.
            if spec.changed_only && text.is_empty() {
                return Err(format!(
                    "{}: the case has it ({initial}); an empty field doesn't take it out of the file (dict.remove does)",
                    f.label
                ));
            }
            // OpenFOAM text where a number goes (controlDict): a macro or a directive only.
            let foam_text = spec.changed_only && text.starts_with(['$', '#']);
            let value = match f.kind {
                Kind::Flag => (text == "true").then_some(Value::Bool(true)),
                Kind::FlagOn => (text == "false").then_some(Value::Bool(false)),
                _ if text.is_empty() => None,
                // As typed: `7200` stays a whole number, `1e3` a float.
                Kind::Number(_) => match serde_json::from_str::<Value>(text).ok().filter(Value::is_number) {
                    Some(x) => Some(x),
                    None if foam_text => Some(json!(text)),
                    None => return Err(format!("{}: `{text}` isn't a number", f.label)),
                },
                Kind::Count(_) => match text.parse::<u64>() {
                    Ok(n) => Some(json!(n)),
                    Err(_) if foam_text => Some(json!(text)),
                    Err(_) => return Err(format!("{}: `{text}` isn't a whole number", f.label)),
                },
                _ => Some(json!(text)),
            };
            // Nothing in the field: left out, or what the JSON had that the field can't hold
            // (in `extra`), kept.
            if let Some(v) = value {
                out.insert(f.key.to_string(), v);
            }
        }
        for p in &self.passed {
            out.insert(p.clone(), Value::Bool(true));
        }
        Ok(Value::Object(out))
    }

    /// Fills the fields from `params` (back from the JSON): the keys of fields go in them, the
    /// rest are kept as they are; the parameters passed follow those set `true`.
    /// A value the field can't hold (a flag that isn't `true` or `false`, a list that isn't of
    /// names, a list or a map for a text) is kept as it is with the keys the form has no field
    /// for, and sent so: the command says what is wrong with it. Not a JSON object: an error.
    pub(crate) fn fill(&mut self, spec: &Spec, params: &Value) -> Result<(), String> {
        let Some(map) = params.as_object() else {
            return Err("the parameters are a JSON object: {\"key\": value, …}".to_string());
        };
        let mut extra = map.clone();
        for f in spec.fields {
            let v = extra.remove(f.key);
            let keep = |extra: &mut Map<String, Value>, v: &Value| {
                extra.insert(f.key.to_string(), v.clone());
            };
            match f.kind {
                Kind::Surfaces | Kind::Regions => {
                    let names: Option<BTreeSet<String>> = match &v {
                        None => Some(BTreeSet::new()),
                        Some(Value::Array(a)) => a
                            .iter()
                            .map(|x| {
                                x.as_str().map(|n| n.strip_prefix("constant/triSurface/").unwrap_or(n).to_string())
                            })
                            .collect(),
                        Some(_) => None,
                    };
                    match (names, &v) {
                        (Some(set), _) => {
                            self.lists.insert(f.key, set);
                        }
                        (None, Some(v)) => {
                            self.lists.insert(f.key, BTreeSet::new());
                            keep(&mut extra, v);
                        }
                        (None, None) => {}
                    }
                }
                Kind::Numbers(labels) | Kind::Counts(labels) => {
                    let list: Option<Vec<String>> = match &v {
                        None => Some(Vec::new()),
                        Some(Value::Array(a)) if a.len() == labels.len() && a.iter().all(Value::is_number) => {
                            Some(a.iter().map(as_text).collect())
                        }
                        Some(_) => None,
                    };
                    match list {
                        Some(l) => {
                            self.values.insert(f.key, l.join(&PARTS.to_string()));
                        }
                        None => {
                            if let Some(v) = &v {
                                keep(&mut extra, v);
                            }
                            self.values.insert(f.key, String::new());
                        }
                    }
                }
                Kind::FlagOn | Kind::Flag => {
                    let on = match &v {
                        None => f.kind == Kind::FlagOn,
                        Some(Value::Bool(b)) => *b,
                        Some(other) => {
                            keep(&mut extra, other);
                            f.kind == Kind::FlagOn
                        }
                    };
                    let text = match (f.kind, on) {
                        (_, true) => "true",
                        (Kind::FlagOn, false) => "false",
                        _ => "",
                    };
                    self.values.insert(f.key, text.to_string());
                }
                _ => {
                    let text = match &v {
                        Some(v @ (Value::Array(_) | Value::Object(_) | Value::Null)) => {
                            keep(&mut extra, v);
                            String::new()
                        }
                        Some(v) => as_text(v),
                        // Left out: as the form had it when changed_only (unchanged), else empty.
                        None if spec.changed_only => self.initial.get(f.key).cloned().unwrap_or_default(),
                        None => String::new(),
                    };
                    self.values.insert(f.key, text);
                }
            }
        }
        self.passed = self.offered.iter().filter(|k| extra.get(*k) == Some(&Value::Bool(true))).cloned().collect();
        for k in &self.passed {
            extra.remove(k);
        }
        self.extra = extra;
        Ok(())
    }

    /// Takes the parameters a refusal names (`pass "replace": true`) that the form has no
    /// field for, to offer them as checkboxes.
    /// One the form has a field for is pointed to instead (`pointed`: its label).
    pub(crate) fn offer(&mut self, spec: &Spec, error: &str) {
        self.pointed.clear();
        for k in offered(error) {
            match spec.fields.iter().find(|f| f.key == k) {
                Some(f) => self.pointed.push(f.label),
                None if !self.offered.contains(&k) => self.offered.push(k),
                None => {}
            }
        }
    }
}

/// The parameters an error tells to pass as `true`: each `"name": true` in it.
pub(crate) fn offered(error: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = error;
    let mut seen = 0usize;
    while let Some(at) = rest.find("\": true") {
        let before = rest.get(..at).unwrap_or("");
        let all_before = error.get(..seen + at).unwrap_or("");
        // Only what follows a `pass`, and not a key of an example's object (`{"found": true}`).
        let told = all_before.to_ascii_lowercase().contains("pass ");
        let braces = all_before.matches('{').count() > all_before.matches('}').count();
        if let Some(open) = before.rfind('"')
            && told
            && !braces
        {
            let name = before.get(open + 1..).unwrap_or("");
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !out.iter().any(|o| o == name)
            {
                out.push(name.to_string());
            }
        }
        seen += at + 7;
        rest = rest.get(at + 7..).unwrap_or("");
    }
    out
}

/// What a field asks of the window: a file chosen in the system's dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct FileAsk {
    pub key: &'static str,
    pub extensions: &'static [&'static str],
}

/// Draws the form's fields; a *Browse…* pressed asks for a file (when the window has a
/// dialog: `browse`).
pub(crate) fn show(ui: &mut Ui, spec: &Spec, form: &mut FormState, width: f32, browse: bool) -> Option<FileAsk> {
    let mut ask = None;
    let wide = (width - 120.0).max(120.0);
    egui::Grid::new(("form", spec.id)).num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        for f in spec.fields {
            match f.kind {
                Kind::Flag | Kind::FlagOn => {
                    ui.label("");
                    let mut on = match form.values.get(f.key) {
                        Some(v) => v == "true",
                        None => f.kind == Kind::FlagOn,
                    };
                    let r = ui.checkbox(&mut on, f.label);
                    if !f.tip.is_empty() {
                        r.on_hover_text(f.tip);
                    }
                    form.values.insert(f.key, if on { "true" } else { "false" }.to_string());
                    if f.kind == Kind::Flag && !on {
                        form.values.insert(f.key, String::new());
                    }
                }
                Kind::Surfaces | Kind::Regions => {
                    let label = ui.label(f.label);
                    if !f.tip.is_empty() {
                        label.on_hover_text(f.tip);
                    }
                    let mut all = if f.kind == Kind::Surfaces { form.surfaces.clone() } else { form.regions.clone() };
                    // Names ticked that the case doesn't list (from the JSON): shown, to untick.
                    for n in form.lists.get(f.key).into_iter().flatten() {
                        if !all.contains(n) {
                            all.push(n.clone());
                        }
                    }
                    ui.vertical(|ui| {
                        if all.is_empty() {
                            ui.label(RichText::new("none in the case").weak());
                        }
                        let set = form.lists.entry(f.key).or_default();
                        egui::ScrollArea::vertical().id_salt(("list", f.key)).max_height(110.0).show(ui, |ui| {
                            for name in &all {
                                let mut on = set.contains(name);
                                let shown = if name.is_empty() { "(single region)" } else { name.as_str() };
                                if ui.checkbox(&mut on, shown).changed() {
                                    if on {
                                        set.insert(name.clone());
                                    } else {
                                        set.remove(name);
                                    }
                                }
                            }
                        });
                    });
                }
                _ => {
                    let label = ui.label(f.label);
                    if !f.tip.is_empty() {
                        label.clone().on_hover_text(f.tip);
                    }
                    let text = form.values.entry(f.key).or_default();
                    match f.kind {
                        Kind::Choice(options) => {
                            let shown = if text.is_empty() { "default".to_string() } else { text.clone() };
                            egui::ComboBox::from_id_salt(("choice", spec.id, f.key))
                                .selected_text(shown)
                                .width(wide)
                                .show_ui(ui, |ui| {
                                    for o in options {
                                        let name = if o.is_empty() { "default" } else { o };
                                        ui.selectable_value(text, o.to_string(), name);
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                        }
                        Kind::WorkflowRegion => {
                            let shown = if text.is_empty() { "choose…".to_string() } else { text.clone() };
                            egui::ComboBox::from_id_salt(("wregion", spec.id, f.key))
                                .selected_text(shown)
                                .width(wide)
                                .show_ui(ui, |ui| {
                                    for s in &form.workflow_regions {
                                        ui.selectable_value(text, s.clone(), s);
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                        }
                        Kind::Numbers(labels) | Kind::Counts(labels) => {
                            let mut v = parts(text, labels.len());
                            egui::Grid::new(("parts", spec.id, f.key)).num_columns(2).spacing(vec2(6.0, 4.0)).show(
                                ui,
                                |ui| {
                                    for (k, (p, name)) in v.iter_mut().zip(labels.iter()).enumerate() {
                                        let l = ui.label(RichText::new(*name).size(12.0));
                                        ui.add(egui::TextEdit::singleline(p).desired_width(56.0)).labelled_by(l.id);
                                        // The parts' separator isn't part of a number.
                                        if p.contains(PARTS) {
                                            *p = p.replace(PARTS, "");
                                        }
                                        if k % 2 == 1 {
                                            ui.end_row();
                                        }
                                    }
                                },
                            );
                            *text = v.join(&PARTS.to_string());
                        }
                        Kind::Surface => {
                            let shown = if text.is_empty() { "choose…".to_string() } else { text.clone() };
                            egui::ComboBox::from_id_salt(("surface", spec.id, f.key))
                                .selected_text(shown)
                                .width(wide)
                                .show_ui(ui, |ui| {
                                    for s in &form.surfaces {
                                        ui.selectable_value(text, s.clone(), s);
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                        }
                        Kind::File(ext) => {
                            ui.horizontal(|ui| {
                                let edit =
                                    egui::TextEdit::singleline(text).desired_width(wide - 74.0).hint_text("path");
                                ui.add(edit).labelled_by(label.id);
                                if ui.add_enabled(browse, egui::Button::new("Browse…")).clicked() {
                                    ask = Some(FileAsk { key: f.key, extensions: ext });
                                }
                            });
                        }
                        Kind::Text(hint) | Kind::Number(hint) | Kind::Count(hint) => {
                            let edit = egui::TextEdit::singleline(text).desired_width(wide).hint_text(hint);
                            ui.add(edit).labelled_by(label.id);
                        }
                        Kind::Flag | Kind::FlagOn | Kind::Surfaces | Kind::Regions => {}
                    }
                }
            }
            ui.end_row();
        }
    });
    ask
}

/// Draws the parameters a refusal offered, as checkboxes.
pub(crate) fn show_offered(ui: &mut Ui, form: &mut FormState) {
    for label in &form.pointed {
        ui.label(format!("To go on: tick “{label}” above."));
    }
    let offered = form.offered.clone();
    for k in offered {
        let mut on = form.passed.contains(&k);
        if ui.checkbox(&mut on, format!("Pass {k}, as the message says")).changed() {
            if on {
                form.passed.insert(k);
            } else {
                form.passed.remove(&k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_sends_what_is_filled_in() {
        let spec = spec("geometry.import").unwrap();
        let mut f = FormState::default();
        f.values.insert("path", " /tmp/a.stl ".to_string());
        f.values.insert("scale", "0.001".to_string());
        f.values.insert("replace", String::new());
        assert_eq!(f.params(spec).unwrap(), json!({"path": "/tmp/a.stl", "scale": 0.001}));
        f.values.insert("replace", "true".to_string());
        assert_eq!(f.params(spec).unwrap()["replace"], json!(true));
        f.values.insert("scale", "a lot".to_string());
        assert!(f.params(spec).unwrap_err().contains("Scale: `a lot` isn't a number"));
    }

    #[test]
    fn run_controls_send_only_what_changed() {
        let spec = spec("control.set").unwrap();
        let mut start = BTreeMap::new();
        start.insert("endTime", "3600".to_string());
        start.insert("deltaT", "$dt".to_string());
        let mut f = FormState::filled(start);
        assert_eq!(f.params(spec).unwrap(), json!({}));
        f.values.insert("endTime", "7200".to_string());
        f.values.insert("writeFormat", "binary".to_string());
        assert_eq!(f.params(spec).unwrap(), json!({"endTime": 7200, "writeFormat": "binary"}));
        // OpenFOAM text where a number goes: a macro or a directive, sent as text; else refused.
        f.values.insert("deltaT", "#calc \"1/2\"".to_string());
        assert_eq!(f.params(spec).unwrap()["deltaT"], json!("#calc \"1/2\""));
        for bad in ["1,5", "3600 s", "72OO"] {
            f.values.insert("deltaT", bad.to_string());
            assert!(f.params(spec).unwrap_err().contains("isn't a number"), "{bad}");
        }
        f.values.insert("deltaT", "$dt".to_string());
        f.values.insert("writePrecision", "6.5".to_string());
        assert!(f.params(spec).unwrap_err().contains("isn't a whole number"));
        f.values.insert("writePrecision", String::new());
        // JSON a number field can't hold: sent as it is (the command refuses it by name).
        let mut g = FormState::filled(BTreeMap::from([("deltaT", "60".to_string())]));
        g.fill(spec, &json!({"deltaT": null, "endTime": [1]})).unwrap();
        assert_eq!(g.params(spec).unwrap(), json!({"deltaT": null, "endTime": [1]}));
        // An entry the case has, emptied: said, not sent as nothing.
        f.values.insert("endTime", String::new());
        assert!(f.params(spec).unwrap_err().contains("dict.remove"));
    }

    #[test]
    fn defaults_on_are_sent_when_turned_off_and_lists_when_ticked() {
        let spec = spec("scene.snapshot").unwrap();
        let mut f = FormState::default();
        f.fill(spec, &json!({})).unwrap();
        assert_eq!(f.params(spec).unwrap(), json!({}));
        f.values.insert("grid", "false".to_string());
        f.lists.entry("regions").or_default().insert("air".to_string());
        assert_eq!(f.params(spec).unwrap(), json!({"grid": false, "regions": ["air"]}));
    }

    #[test]
    fn json_and_form_go_both_ways_keeping_what_the_form_lacks() {
        let spec = spec("scene.snapshot").unwrap();
        let mut f = FormState::default();
        let p = json!({"view": "top", "dark": false, "cuts": [{"region": "air", "found": true}], "regions": ["air"]});
        f.fill(spec, &p).unwrap();
        assert_eq!(f.values.get("view").map(String::as_str), Some("top"));
        assert!(f.extra.contains_key("cuts"));
        assert_eq!(f.params(spec).unwrap(), p);
        // What a field can't hold is kept, and sent as it is; not an object: refused.
        let odd = json!({"edges": "yes", "regions": "air", "zoom": [1, 2]});
        f.fill(spec, &odd).unwrap();
        assert_eq!(f.params(spec).unwrap(), odd);
        assert!(f.fill(spec, &json!([])).is_err());
        // A surface named with its directory goes into the list by its file name.
        f.fill(spec, &json!({"surfaces": ["constant/triSurface/b.stl"]})).unwrap();
        assert_eq!(f.params(spec).unwrap()["surfaces"], json!(["b.stl"]));
    }

    #[test]
    fn a_refusal_offers_the_parameters_it_names() {
        let e = "mesh.box: region air has blocks …: pass \"reshape\": true to replace …; or \"replace\": true, \"reshape\": true";
        assert_eq!(offered(e), ["reshape", "replace"]);
        // Not a key of an example, nor without a pass.
        assert!(offered("cuts: [{\"region\", \"found\": true}, …]: pass one").is_empty());
        assert!(offered("{\"found\": true} is what it takes").is_empty());
        let spec = spec("geometry.import").unwrap();
        let mut f = FormState::default();
        // replace has a field of its own: pointed to, not offered again.
        f.offer(spec, "pass \"replace\": true or \"dropSettings\": true");
        assert_eq!(f.offered, ["dropSettings"]);
        assert_eq!(f.pointed, ["Replace a surface of that name"]);
        f.passed.insert("dropSettings".to_string());
        assert_eq!(f.params(spec).unwrap()["dropSettings"], json!(true));
        // Back from JSON: the offered one ticked as the JSON has it.
        f.fill(spec, &json!({"dropSettings": false})).unwrap();
        assert!(f.passed.is_empty());
        assert_eq!(f.extra.get("dropSettings"), Some(&json!(false)));
    }

    #[test]
    fn a_boxs_margins_and_cells_are_lists() {
        let spec = spec("mesh.box").unwrap();
        let mut f = FormState::default();
        f.fill(spec, &json!({"region": "air", "margin": [1, 2, 3, 4, 0, 6], "cellSize": 2})).unwrap();
        assert_eq!(f.params(spec).unwrap(), json!({"region": "air", "margin": [1, 2, 3, 4, 0, 6], "cellSize": 2}));
        f.values.insert("cells", "10|20|".to_string());
        assert!(f.params(spec).unwrap_err().contains("Cells: give all 3"));
        f.values.insert("cells", "10|20|x".to_string());
        assert!(f.params(spec).unwrap_err().contains("(z): `x`"));
        f.values.insert("cells", "10|20|30".to_string());
        assert_eq!(f.params(spec).unwrap()["cells"], json!([10, 20, 30]));
        // One margin for all (JSON only): kept and sent as it is.
        f.fill(spec, &json!({"margin": 50})).unwrap();
        assert_eq!(f.params(spec).unwrap()["margin"], json!(50));
    }
}
