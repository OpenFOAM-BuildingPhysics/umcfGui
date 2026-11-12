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
    /// An axis of a grid of blocks, as a table: a row per station, with the cells and their
    /// grading to the next (`[station, [cells, grading], station, …]`). Sent only when changed
    /// from what the form was filled with.
    Axis,
    /// Blocks of a grid left out, `[[i, j, k], …]`: ticked on a layer of the grid the axes'
    /// fields (these keys) make, or listed. Sent only when changed.
    Missing(&'static [&'static str; 3]),
    /// A patch name for each of these sides (`{"west": "inlet", …}`): only those changed are
    /// sent, one emptied as `null` (taken away).
    Sides(&'static [&'static str]),
    /// A name typed, or picked from those the card offers ([`FormState::options`] under the
    /// field's key): a region, a patch, a material of the description.
    Pick(&'static str),
    /// A checkbox sent either way (`"on": true` or `false`), on to start with.
    Switch,
    /// Several names the card offers (the patches of a region): a list, none ticked left out.
    Names,
    /// One of these words, sent only when changed from what the form was filled with; emptied:
    /// `null` (taken away). A patch's role.
    Role(&'static [&'static str]),
    /// A name typed or picked (as [`Kind::Pick`]), sent as [`Kind::Role`] is: a material's model.
    Model,
    /// The region and patch a patch is coupled to (`{"region", "patch"}`), each typed or picked:
    /// sent only when changed, both emptied: `null`.
    Partner,
}

/// The parameters that name what a command acts on, kept with `remove: true`.
const NAMES_WHAT: [&str; 3] = ["region", "patch", "material"];

/// Where a [`Kind::Partner`] field's patches are offered, in [`FormState::options`] (its
/// regions under the field's key).
pub(crate) const PARTNER_PATCHES: &str = "partner/patch";

/// The rows of a [`Kind::Axis`] or [`Kind::Missing`] field's text.
const ROWS: char = '\n';

/// Whether a field is sent only when changed from what the form was filled with.
fn sent_when_changed(kind: Kind) -> bool {
    matches!(kind, Kind::Axis | Kind::Missing(_) | Kind::Sides(_) | Kind::Role(_) | Kind::Model | Kind::Partner)
}

/// A grading as JSON: a number, or sections `a b c; a b c` as `[[a, b, c], …]`; blank: 1.
fn grading_json(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        return Some(json!(1));
    }
    if !text.contains(|c: char| c == ';' || c == ',' || c.is_whitespace()) {
        return serde_json::from_str::<Value>(text).ok().filter(Value::is_number);
    }
    let mut sections = Vec::new();
    for s in text.split(';').filter(|s| !s.trim().is_empty()) {
        let three: Option<Vec<Value>> = s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|x| !x.is_empty())
            .map(|x| serde_json::from_str::<Value>(x).ok().filter(Value::is_number))
            .collect();
        match three {
            Some(t) if t.len() == 3 => sections.push(Value::Array(t)),
            _ => return None,
        }
    }
    (!sections.is_empty()).then_some(Value::Array(sections))
}

/// A grading's text: a number, or sections `a b c; a b c`.
fn grading_text(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::Array(sections) if !sections.is_empty() => {
            let parts: Option<Vec<String>> = sections
                .iter()
                .map(|s| {
                    let t = s.as_array().filter(|t| t.len() == 3 && t.iter().all(Value::is_number))?;
                    Some(t.iter().map(as_text).collect::<Vec<_>>().join(" "))
                })
                .collect();
            parts.map(|p| p.join("; "))
        }
        _ => None,
    }
}

/// A row of an axis' table: its station, and the cells and grading to the next.
fn axis_row(line: &str) -> [String; 3] {
    let mut p = line.split(PARTS).map(str::to_string);
    [p.next().unwrap_or_default(), p.next().unwrap_or_default(), p.next().unwrap_or_default()]
}

/// The text of a [`Kind::Axis`], [`Kind::Missing`] or [`Kind::Sides`] field from its JSON;
/// `None` for a value it can't hold. Sides: over `start`'s parts (their keys merge).
pub(crate) fn grid_text(kind: Kind, v: &Value, start: &str) -> Option<String> {
    match kind {
        Kind::Axis => {
            let items = v.as_array()?;
            if items.len() % 2 == 0 {
                return None;
            }
            let mut rows = Vec::with_capacity(items.len() / 2 + 1);
            for pair in items.chunks(2) {
                let station = pair.first().filter(|x| x.is_number()).map(as_text)?;
                match pair.get(1) {
                    Some(span) => {
                        let two = span.as_array().filter(|t| t.len() == 2)?;
                        let cells = two.first().and_then(Value::as_u64)?;
                        let grading = grading_text(two.get(1)?)?;
                        rows.push(format!("{station}{PARTS}{cells}{PARTS}{grading}"));
                    }
                    None => rows.push(station),
                }
            }
            Some(rows.join(&ROWS.to_string()))
        }
        Kind::Missing(_) => {
            let rows: Option<Vec<String>> = v
                .as_array()?
                .iter()
                .map(|b| {
                    let b = b.as_array().filter(|b| b.len() == 3)?;
                    let n: Option<Vec<String>> = b.iter().map(|x| x.as_u64().map(|x| x.to_string())).collect();
                    n.map(|n| n.join(" "))
                })
                .collect();
            rows.map(|r| r.join(&ROWS.to_string()))
        }
        Kind::Sides(names) => {
            let m = v.as_object()?;
            let mut p = parts(start, names.len());
            for (k, n) in m {
                let at = names.iter().position(|x| x == k)?;
                let slot = p.get_mut(at)?;
                match n {
                    Value::Null => slot.clear(),
                    Value::String(n) if !n.contains(PARTS) => *slot = n.clone(),
                    _ => return None,
                }
            }
            Some(p.join(&PARTS.to_string()))
        }
        Kind::Role(_) | Kind::Model => match v {
            Value::Null => Some(String::new()),
            Value::String(t) => Some(t.clone()),
            _ => None,
        },
        Kind::Partner => match v {
            Value::Null => Some(String::new()),
            Value::Object(m) => {
                let (r, p) = (m.get("region")?.as_str()?, m.get("patch")?.as_str()?);
                (m.len() == 2 && !r.contains(PARTS) && !p.contains(PARTS)).then(|| format!("{r}{PARTS}{p}"))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A [`Kind::Sides`] text typed in (`value`), filled from `was`, put over the grid as it is
/// `now`: each side not typed in (as it was filled) takes the grid's, the others stay.
pub(crate) fn merge_sides(value: &str, was: &str, now: &str, n: usize) -> String {
    let (v, w, m) = (parts(value, n), parts(was, n), parts(now, n));
    let out: Vec<String> =
        v.iter().zip(&w).zip(&m).map(|((v, w), m)| if v.trim() == w.trim() { m.clone() } else { v.clone() }).collect();
    out.join(&PARTS.to_string())
}

/// The JSON of a [`Kind::Axis`], [`Kind::Missing`] or [`Kind::Sides`] field's text (`was`:
/// what the form was filled with); `None` to leave it out; an error naming the field.
fn grid_json(f: &Field, text: &str, was: &str) -> Result<Option<Value>, String> {
    let blank = |t: &str| t.chars().all(|c| c == PARTS || c == ROWS || c.is_whitespace());
    let rows = |t: &str| -> Vec<String> {
        t.split(ROWS).filter(|r| !blank(r)).map(|r| r.trim().to_string()).collect::<Vec<_>>()
    };
    match f.kind {
        Kind::Axis => {
            let (now, then) = (rows(text), rows(was));
            if now == then {
                return Ok(None);
            }
            if now.is_empty() {
                return Err(format!("{}: give its stations (an axis isn't taken away)", f.label));
            }
            let mut out = Vec::with_capacity(2 * now.len());
            for (i, r) in now.iter().enumerate() {
                let [station, cells, grading] = axis_row(r);
                let row = i + 1;
                if r.matches(PARTS).count() > 2 {
                    return Err(format!("{} (row {row}): `{r}` has more than station|cells|grading", f.label));
                }
                let station =
                    serde_json::from_str::<Value>(station.trim()).ok().filter(Value::is_number).ok_or_else(|| {
                        format!("{} (row {row}): the station `{}` isn't a number", f.label, station.trim())
                    })?;
                out.push(station);
                if i + 1 < now.len() {
                    let cells = cells.trim().parse::<u64>().map_err(|_| {
                        format!(
                            "{} (row {row}): the cells to the next station `{}` aren't a whole number",
                            f.label,
                            cells.trim()
                        )
                    })?;
                    let grading = grading_json(&grading).ok_or_else(|| {
                        format!(
                            "{} (row {row}): the grading `{}` is a number, or sections `fraction cells-fraction expansion; …`",
                            f.label,
                            grading.trim()
                        )
                    })?;
                    out.push(json!([cells, grading]));
                }
            }
            Ok(Some(Value::Array(out)))
        }
        Kind::Missing(_) => {
            let (now, then) = (rows(text), rows(was));
            if now == then {
                return Ok(None);
            }
            let mut out = Vec::with_capacity(now.len());
            for r in &now {
                let n: Option<Vec<u64>> = block_parts(r).into_iter().map(|x| x.parse().ok()).collect();
                match n.as_deref() {
                    Some([i, j, k]) => out.push(json!([i, j, k])),
                    _ => return Err(format!("{}: `{r}` isn't a block `i j k` (whole numbers from 0)", f.label)),
                }
            }
            Ok(Some(Value::Array(out)))
        }
        Kind::Sides(names) => {
            let (now, then) = (parts(text, names.len()), parts(was, names.len()));
            let mut m = Map::new();
            for ((name, n), w) in names.iter().zip(&now).zip(&then) {
                let (n, w) = (n.trim(), w.trim());
                if n == w {
                    continue;
                }
                m.insert(name.to_string(), if n.is_empty() { Value::Null } else { json!(n) });
            }
            Ok((!m.is_empty()).then_some(Value::Object(m)))
        }
        Kind::Role(_) | Kind::Model => {
            let (now, then) = (text.trim(), was.trim());
            if now == then {
                return Ok(None);
            }
            Ok(Some(if now.is_empty() { Value::Null } else { json!(now) }))
        }
        Kind::Partner => {
            let (now, then) = (parts(text, 2), parts(was, 2));
            let trim = |v: &[String]| v.iter().map(|x| x.trim().to_string()).collect::<Vec<_>>();
            let (now, then) = (trim(&now), trim(&then));
            if now == then {
                return Ok(None);
            }
            match (now.first().map(String::as_str), now.get(1).map(String::as_str)) {
                (Some(""), Some("")) => Ok(Some(Value::Null)),
                (Some(r), Some(p)) if !r.is_empty() && !p.is_empty() => Ok(Some(json!({"region": r, "patch": p}))),
                _ => Err(format!("{}: give the region and the patch (or neither, to take it away)", f.label)),
            }
        }
        _ => Ok(None),
    }
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
        id: "mesh.block",
        intro: "A region's grid of blocks: its stations and cells along each axis, the blocks left out (the buildings) and the sides' patches; its blockMeshDict written from them.",
        fields: &[
            field("region", "Region", Kind::WorkflowRegion, "A region of system/umcfWorkflow"),
            field("x", "x", Kind::Axis, "The stations along x, west to east"),
            field("y", "y", Kind::Axis, "The stations along y, south to north"),
            field("z", "z", Kind::Axis, "The stations along z"),
            field(
                "missing",
                "Missing blocks",
                Kind::Missing(&["x", "y", "z"]),
                "Blocks left out (the buildings), counted from 0 along x, y and z",
            ),
            field(
                "patches",
                "Patches",
                Kind::Sides(&["west", "east", "south", "north", "ground", "top", "missing"]),
                "Each side's patch; empty: its faces left to blockMesh's own (defaultFaces)",
            ),
            field(
                "replace",
                "Write over a blockMeshDict edited by hand",
                Kind::Flag,
                "Save keeps the one there in the backup",
            ),
            field(
                "remove",
                "Take the blocks out of the description",
                Kind::Flag,
                "With the box they were made from; the blockMeshDict stays",
            ),
        ],
        changed_only: false,
    },
    Spec {
        id: "workflow.import",
        intro: "Writes system/umcfWorkflow from the case: its scripts, regions, patches, materials, components, settings and grids of blocks (Save writes it).",
        fields: &[field(
            "replace",
            "Write over the case's system/umcfWorkflow",
            Kind::Flag,
            "Its comments and hand edits are not kept",
        )],
        changed_only: false,
    },
    Spec {
        id: "workflow.region",
        intro: "A region of system/umcfWorkflow: added with its kind, given another kind, or taken out.",
        fields: &[
            field(
                "region",
                "Region",
                Kind::Pick("a name"),
                "A new region, or one of the description; one plain OpenFOAM word",
            ),
            field("kind", "Kind", Kind::Choice(&["fluid", "solid", "vegetation"]), "A solid with materials stays one"),
            field(
                "remove",
                "Take the region out",
                Kind::Flag,
                "What still names it (coupled patches, the radiation) is listed",
            ),
        ],
        changed_only: false,
    },
    Spec {
        id: "workflow.patch",
        intro: "A patch of a region of system/umcfWorkflow: its role, its partner, or a new name (a patch the region doesn't list is added).",
        fields: &[
            field("region", "Region", Kind::WorkflowRegion, "A region of system/umcfWorkflow"),
            field("patch", "Patch", Kind::Pick("a patch, or a new one"), "A patch of the region, or a new one"),
            field(
                "role",
                "Role",
                Kind::Role(&[
                    "inlet", "outlet", "side", "top", "ground", "coupled", "canopy", "interior", "bottom", "sky",
                ]),
                "none: taken away",
            ),
            field("partner", "Coupled to", Kind::Partner, "The region and patch it is coupled to; neither: taken away"),
            field(
                "name",
                "New name",
                Kind::Text("as it is"),
                "The partners and the region's blocks follow; patch.set renames it in the case",
            ),
        ],
        changed_only: false,
    },
    Spec {
        id: "workflow.material",
        intro: "A material of a solid region of system/umcfWorkflow: the cell zone built of it and its model.",
        fields: &[
            field("region", "Region", Kind::Pick("a solid region"), "A solid region of system/umcfWorkflow"),
            field("material", "Material", Kind::Pick("its cell zone"), "Added when the region doesn't list it"),
            field("model", "Model", Kind::Model, "Its buildingMaterialModel; empty: taken away"),
            field("remove", "Take the material out", Kind::Flag, ""),
        ],
        changed_only: false,
    },
    Spec {
        id: "workflow.component",
        intro: "Switches a component in the case's files and in system/umcfWorkflow.",
        fields: &[
            field("component", "Component", Kind::Choice(&["grass", "temperatureLimit", "blending"]), ""),
            field("on", "On", Kind::Switch, "Off keeps the coefficients"),
            field("patches", "Grass on", Kind::Names, "The patches of air the grass grows on (grass, on only)"),
        ],
        changed_only: false,
    },
    Spec {
        id: "workflow.scripts",
        intro: "Writes Allprepare, Allrun and Allclean from system/umcfWorkflow (Save writes them, executable).",
        fields: &[field(
            "replace",
            "Write over scripts that are there and differ",
            Kind::Flag,
            "Save keeps a backup of each",
        )],
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
    /// The lists as the form was filled (to tell what was ticked since).
    pub(crate) initial_lists: BTreeMap<&'static str, BTreeSet<String>>,
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
    /// Parameters from *Edit as JSON* naming another region than the form was filled for:
    /// laid over that region's grid once it is filled from it (the block card).
    pub(crate) refill: Option<Value>,
    /// The names a card offers its [`Kind::Pick`], [`Kind::Model`], [`Kind::Partner`] and
    /// [`Kind::Names`] fields, by key (set each frame from the case).
    pub(crate) options: BTreeMap<&'static str, Vec<String>>,
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
    /// Whether something was typed or ticked since the form was filled: a field's text other
    /// than it was (empty parts counted as empty), a list other than it was.
    pub(crate) fn typed(&self) -> bool {
        let empty = |t: &str| t.chars().all(|c| c == PARTS || c == ROWS || c.is_whitespace());
        let differs = |k: &&'static str, v: &String| {
            let was = self.initial.get(k).map(String::as_str).unwrap_or("");
            v != was && !(empty(v) && empty(was))
        };
        let lists = |m: &BTreeMap<&'static str, BTreeSet<String>>| -> BTreeMap<&'static str, BTreeSet<String>> {
            m.iter().filter(|(_, s)| !s.is_empty()).map(|(k, s)| (*k, s.clone())).collect()
        };
        self.values.iter().any(|(k, v)| differs(k, v)) || lists(&self.lists) != lists(&self.initial_lists)
    }

    /// A form with `values` as its fields' text, and these as what it was filled with.
    pub(crate) fn filled(values: BTreeMap<&'static str, String>) -> Self {
        FormState { initial: values.clone(), values, ..FormState::default() }
    }

    /// The parameters the form makes; an error naming a field whose text isn't what it takes.
    pub(crate) fn params(&self, spec: &Spec) -> Result<Value, String> {
        let mut out = self.extra.clone();
        for f in spec.fields {
            if let Kind::Surfaces | Kind::Regions | Kind::Names = f.kind {
                let list: Vec<&String> = self.lists.get(f.key).map(|s| s.iter().collect()).unwrap_or_default();
                // None ticked: left out (or what the JSON had that a list can't hold, kept).
                if !list.is_empty() {
                    out.insert(f.key.to_string(), json!(list));
                }
                continue;
            }
            if sent_when_changed(f.kind) {
                let text = self.values.get(f.key).map(String::as_str).unwrap_or("");
                // What the JSON had that the field can't hold: kept, sent as it is.
                if text.trim().is_empty() && self.extra.contains_key(f.key) {
                    continue;
                }
                let was = self.initial.get(f.key).map(String::as_str).unwrap_or("");
                if let Some(v) = grid_json(f, text, was)? {
                    out.insert(f.key.to_string(), v);
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
                Kind::Switch => Some(Value::Bool(text != "false")),
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
        // *Take … out* ticked: only what names what is taken out goes with it (the commands
        // refuse anything else with remove), whatever the other fields were filled with.
        if out.get("remove") == Some(&Value::Bool(true)) {
            out.retain(|k, _| NAMES_WHAT.contains(&k.as_str()) || k == "remove");
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
                Kind::Surfaces | Kind::Regions | Kind::Names => {
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
                k if sent_when_changed(k) => {
                    // Left out: as the form was filled (unchanged).
                    let was = self.initial.get(f.key).cloned().unwrap_or_default();
                    let text = match &v {
                        None => Some(was.clone()),
                        Some(v) => grid_text(f.kind, v, &was),
                    };
                    match (text, &v) {
                        (Some(t), _) => {
                            self.values.insert(f.key, t);
                        }
                        (None, Some(v)) => {
                            keep(&mut extra, v);
                            self.values.insert(f.key, String::new());
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
                Kind::Switch => {
                    let on = match &v {
                        None => true,
                        Some(Value::Bool(b)) => *b,
                        Some(other) => {
                            // Kept and sent as it is (the field left empty), for the command
                            // to refuse by name.
                            keep(&mut extra, other);
                            self.values.insert(f.key, String::new());
                            continue;
                        }
                    };
                    self.values.insert(f.key, on.to_string());
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
                Kind::Switch => {
                    ui.label("");
                    let mut on = form.values.get(f.key).is_none_or(|v| v != "false");
                    let r = ui.checkbox(&mut on, f.label);
                    // Set only when clicked: left alone, a value from the JSON it can't hold
                    // (kept aside, the field empty) is sent as it is.
                    if r.changed() {
                        form.values.insert(f.key, on.to_string());
                    }
                    if !f.tip.is_empty() {
                        r.on_hover_text(f.tip);
                    }
                }
                Kind::Pick(_) | Kind::Model | Kind::Role(_) | Kind::Partner | Kind::Names => {
                    let label = ui.label(f.label);
                    if !f.tip.is_empty() {
                        label.clone().on_hover_text(f.tip);
                    }
                    let offered = form.options.get(f.key).cloned().unwrap_or_default();
                    match f.kind {
                        Kind::Pick(hint) => {
                            let text = form.values.entry(f.key).or_default();
                            pick(ui, (spec.id, f.key), text, &offered, hint, wide, label.id);
                        }
                        Kind::Model => {
                            let text = form.values.entry(f.key).or_default();
                            pick(ui, (spec.id, f.key), text, &offered, "none", wide, label.id);
                        }
                        Kind::Role(options) => {
                            let text = form.values.entry(f.key).or_default();
                            let shown = if text.is_empty() { "none".to_string() } else { text.clone() };
                            egui::ComboBox::from_id_salt(("role", spec.id, f.key))
                                .selected_text(shown)
                                .width(wide)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(text, String::new(), "none");
                                    for o in options {
                                        ui.selectable_value(text, o.to_string(), *o);
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                        }
                        Kind::Partner => {
                            let patches = form.options.get(PARTNER_PATCHES).cloned().unwrap_or_default();
                            let text = form.values.entry(f.key).or_default();
                            let mut v = parts(text, 2);
                            ui.vertical(|ui| {
                                if let [region, patch] = v.as_mut_slice() {
                                    let half = (wide - 4.0).max(80.0);
                                    pick(ui, (spec.id, "partner-region"), region, &offered, "region", half, label.id);
                                    pick(ui, (spec.id, "partner-patch"), patch, &patches, "patch", half, label.id);
                                }
                            });
                            for p in &mut v {
                                if p.contains(PARTS) {
                                    *p = p.replace(PARTS, "");
                                }
                            }
                            *text =
                                if v.iter().all(String::is_empty) { String::new() } else { v.join(&PARTS.to_string()) };
                        }
                        _ => {
                            // Names: those the card offers, and those ticked it doesn't (from the
                            // JSON), to untick.
                            let set = form.lists.entry(f.key).or_default();
                            let mut all = offered;
                            for n in set.iter() {
                                if !all.contains(n) {
                                    all.push(n.clone());
                                }
                            }
                            ui.vertical(|ui| {
                                if all.is_empty() {
                                    ui.label(RichText::new("none").weak());
                                }
                                egui::ScrollArea::vertical().id_salt(("names", f.key)).max_height(110.0).show(
                                    ui,
                                    |ui| {
                                        for name in &all {
                                            let mut on = set.contains(name);
                                            if ui.checkbox(&mut on, name.as_str()).changed() {
                                                if on {
                                                    set.insert(name.clone());
                                                } else {
                                                    set.remove(name);
                                                }
                                            }
                                        }
                                    },
                                );
                            });
                        }
                    }
                }
                Kind::Axis | Kind::Missing(_) | Kind::Sides(_) => {
                    let label = ui.label(f.label);
                    if !f.tip.is_empty() {
                        label.on_hover_text(f.tip);
                    }
                    let counts = match f.kind {
                        Kind::Missing(axes) => {
                            Some(axes.map(|k| stations(form.values.get(k).map_or("", String::as_str))))
                        }
                        _ => None,
                    };
                    let text = form.values.entry(f.key).or_default();
                    ui.vertical(|ui| match f.kind {
                        Kind::Axis => axis_table(ui, (spec.id, f.key), text),
                        Kind::Missing(_) => missing_blocks(ui, (spec.id, f.key), text, counts.unwrap_or_default()),
                        Kind::Sides(names) => sides(ui, (spec.id, f.key), text, names),
                        _ => {}
                    });
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
                            // The first empty: the command's default; else one must be chosen.
                            let empty = if options.first() == Some(&"") { "default" } else { "choose…" };
                            let shown = if text.is_empty() { empty.to_string() } else { text.clone() };
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
                        Kind::Flag
                        | Kind::FlagOn
                        | Kind::Surfaces
                        | Kind::Regions
                        | Kind::Axis
                        | Kind::Missing(_)
                        | Kind::Sides(_)
                        | Kind::Pick(_)
                        | Kind::Switch
                        | Kind::Names
                        | Kind::Role(_)
                        | Kind::Model
                        | Kind::Partner => {}
                    }
                }
            }
            ui.end_row();
        }
    });
    ask
}

/// A name typed, or picked from `offered` in the menu beside it (*▾*).
fn pick(ui: &mut Ui, id: (&str, &str), text: &mut String, offered: &[String], hint: &str, width: f32, label: egui::Id) {
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(text).desired_width((width - 30.0).max(60.0)).hint_text(hint);
        ui.add(edit).labelled_by(label);
        ui.add_enabled_ui(!offered.is_empty(), |ui| {
            egui::ComboBox::from_id_salt(("pick", id.0, id.1))
                .selected_text("")
                .width(16.0)
                .show_ui(ui, |ui| {
                    for o in offered {
                        if ui.selectable_label(o == text, o).clicked() {
                            text.clone_from(o);
                        }
                    }
                })
                .response
                .on_hover_text("Choose one the case has")
                .labelled_by(label);
        });
    });
}

/// The parts of a missing block's row (`1 1 0`, `1,1,0`).
fn block_parts(row: &str) -> Vec<&str> {
    row.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).collect()
}

/// The number of stations an axis' table gives (its rows not blank).
fn stations(text: &str) -> usize {
    text.split(ROWS).filter(|r| !r.chars().all(|c| c == PARTS || c.is_whitespace())).count()
}

/// The rows of a table shown at most, past which it is edited as text.
const TABLE_ROWS: usize = 200;

/// An axis as a table: a row per station, with the cells and grading to the next; rows added
/// and taken away. A longer one as text, a row a line (`station|cells|grading`).
fn axis_table(ui: &mut Ui, id: (&str, &str), text: &mut String) {
    // Counted before any row is made: a long axis is edited as text.
    let lines = if text.is_empty() { 0 } else { text.matches(ROWS).count() + 1 };
    if lines > TABLE_ROWS {
        let l = ui.label(RichText::new(format!("{lines} stations, a row a line: station|cells|grading")).size(12.0));
        egui::ScrollArea::vertical().id_salt(("axis-text", id)).max_height(140.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(text).code_editor().desired_rows(6).desired_width(f32::INFINITY))
                .labelled_by(l.id);
        });
        return;
    }
    let mut rows: Vec<[String; 3]> =
        if text.is_empty() { Vec::new() } else { text.split(ROWS).map(axis_row).collect() };
    let mut remove = None;
    let last = rows.len().saturating_sub(1);
    egui::ScrollArea::vertical().id_salt(("axis", id)).max_height(150.0).show(ui, |ui| {
        egui::Grid::new(("axis-grid", id)).num_columns(4).spacing(vec2(6.0, 3.0)).show(ui, |ui| {
            let hs = ui.label(RichText::new("Station (m)").size(11.5)).id;
            let hc = ui.label(RichText::new("Cells to next").size(11.5)).id;
            let hg = ui
                .label(RichText::new("Grading").size(11.5))
                .on_hover_text("Last cell over first; or sections: fraction cells-fraction expansion; …")
                .id;
            ui.label("");
            ui.end_row();
            for (i, [station, cells, grading]) in rows.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(station).desired_width(64.0)).labelled_by(hs);
                if i < last {
                    ui.add(egui::TextEdit::singleline(cells).desired_width(52.0)).labelled_by(hc);
                    ui.add(egui::TextEdit::singleline(grading).desired_width(84.0).hint_text("1")).labelled_by(hg);
                } else {
                    ui.label("");
                    ui.label("");
                }
                if ui.small_button("−").on_hover_text("Take this station away").clicked() {
                    remove = Some(i);
                }
                for t in [&mut *station, &mut *cells, &mut *grading] {
                    if t.contains([PARTS, ROWS]) {
                        *t = t.replace([PARTS, ROWS], "");
                    }
                }
                ui.end_row();
            }
        });
    });
    if let Some(i) = remove {
        rows.remove(i);
    }
    if ui.small_button("+ Station").on_hover_text("A station after the last").clicked() {
        rows.push(Default::default());
    }
    let joined: Vec<String> =
        rows.iter()
            // A row only of its station (the last) as the station; a blank one kept as its parts,
            // to be typed in (blank rows are sent as none).
            .map(|[a, b, c]| {
                if b.is_empty() && c.is_empty() && !a.is_empty() {
                    a.clone()
                } else {
                    format!("{a}{PARTS}{b}{PARTS}{c}")
                }
            })
            .collect();
    *text = joined.join(&ROWS.to_string());
}

/// The most blocks along x or y shown to tick on a layer; past it, listed only.
const TICK_SIDE: usize = 40;

/// Missing blocks: ticked on a layer of the grid (along z; x across, y up), and listed, a block
/// a line (`i j k`).
fn missing_blocks(ui: &mut Ui, id: (&str, &str), text: &mut String, stations: [usize; 3]) {
    let [nx, ny, nz] = stations.map(|s| s.saturating_sub(1));
    // Each row as `i j k`, however typed.
    let mut list: Vec<String> = text.split(ROWS).map(|r| block_parts(r).join(" ")).filter(|r| !r.is_empty()).collect();
    let ticked: std::collections::HashSet<String> = list.iter().cloned().collect();
    let mut toggle = None;
    if nx > 0 && ny > 0 && nz > 0 && nx <= TICK_SIDE && ny <= TICK_SIDE {
        let layer_id = egui::Id::unique(("missing-layer", id));
        let mut k: usize = ui.data(|d| d.get_temp(layer_id)).unwrap_or(0).min(nz - 1);
        ui.horizontal(|ui| {
            let l = ui.label(RichText::new("Layer k (along z)").size(12.0));
            ui.add(egui::DragValue::new(&mut k).range(0..=nz - 1)).labelled_by(l.id);
            ui.label(RichText::new(format!("of {nz}; x across, y up")).size(12.0).weak());
        });
        ui.data_mut(|d| d.insert_temp(layer_id, k));
        let side = (220.0 / nx.max(ny) as f32).clamp(5.0, 18.0);
        let (rect, response) = ui.allocate_exact_size(vec2(side * nx as f32, side * ny as f32), egui::Sense::click());
        let painter = ui.painter_at(rect);
        let v = ui.visuals();
        let (fill, empty, line) = (v.selection.bg_fill, v.extreme_bg_color, v.widgets.noninteractive.bg_stroke);
        for i in 0..nx {
            for j in 0..ny {
                let at = egui::pos2(rect.min.x + i as f32 * side, rect.max.y - (j + 1) as f32 * side);
                let cell = egui::Rect::from_min_size(at, vec2(side, side));
                let on = ticked.contains(&format!("{i} {j} {k}"));
                painter.rect_filled(cell.shrink(0.5), 0.0, if on { fill } else { empty });
                painter.rect_stroke(cell, 0.0, line, egui::StrokeKind::Inside);
            }
        }
        let block = |p: egui::Pos2| {
            let i = ((p.x - rect.min.x) / side).floor();
            let j = ((rect.max.y - p.y) / side).floor();
            (i >= 0.0 && j >= 0.0 && (i as usize) < nx && (j as usize) < ny).then_some((i as usize, j as usize))
        };
        if let Some((i, j)) = response.hover_pos().and_then(block) {
            response.clone().on_hover_text(format!("block {i} {j} {k}"));
        }
        if response.clicked()
            && let Some((i, j)) = response.interact_pointer_pos().and_then(block)
        {
            toggle = Some(format!("{i} {j} {k}"));
        }
    } else if nx > TICK_SIDE || ny > TICK_SIDE {
        ui.label(RichText::new(format!("{nx} × {ny} blocks a layer: listed below")).size(12.0).weak());
    }
    if let Some(b) = toggle {
        // Every row of the block taken away (however typed), or the block added.
        let before = list.len();
        list.retain(|x| *x != b);
        if list.len() == before {
            list.push(b);
        }
        *text = list.join(&ROWS.to_string());
    }
    egui::CollapsingHeader::new(RichText::new(format!("{} missing, as a list", list.len())).size(12.0))
        .id_salt(("missing-list", id))
        .show(ui, |ui| {
            let l = ui.label(RichText::new("A block a line: i j k").size(11.5));
            egui::ScrollArea::vertical().id_salt(("missing-text", id)).max_height(120.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("i j k, a block a line"),
                )
                .labelled_by(l.id);
            });
        });
}

/// What each side is, for its label.
fn side_hint(name: &str) -> &'static str {
    match name {
        "west" => "x min",
        "east" => "x max",
        "south" => "y min",
        "north" => "y max",
        "ground" => "z min",
        "top" => "z max",
        "missing" => "around the missing blocks",
        _ => "",
    }
}

/// A patch for each side: a name each; empty: none (its faces to blockMesh's own).
fn sides(ui: &mut Ui, id: (&str, &str), text: &mut String, names: &[&str]) {
    let mut v = parts(text, names.len());
    egui::Grid::new(("sides", id)).num_columns(2).spacing(vec2(6.0, 3.0)).show(ui, |ui| {
        for (p, name) in v.iter_mut().zip(names) {
            let l = ui.label(RichText::new(*name).size(12.0)).on_hover_text(side_hint(name));
            ui.add(egui::TextEdit::singleline(p).desired_width(120.0).hint_text(side_hint(name))).labelled_by(l.id);
            if p.contains([PARTS, ROWS]) {
                *p = p.replace([PARTS, ROWS], "");
            }
            ui.end_row();
        }
    });
    *text = v.join(&PARTS.to_string());
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

    #[test]
    fn a_grids_axes_missing_blocks_and_sides_are_sent_when_changed() {
        let spec = spec("mesh.block").unwrap();
        // Filled from a region's grid: as JSON, the same back; unchanged, left out.
        let had = json!({
            "x": [0, [7, 0.5], 200, [10, [[0.5, 0.5, 13.9], [0.5, 0.5, 0.072]]], 590],
            "missing": [[1, 1, 0], [2, 0, 3]],
            "patches": {"west": "inlet", "ground": "ground"},
        });
        let mut f = FormState::default();
        for k in ["x", "missing", "patches"] {
            let field = spec.fields.iter().find(|x| x.key == k).unwrap();
            f.values.insert(field.key, grid_text(field.kind, &had[k], "").unwrap());
        }
        assert_eq!(f.values["x"], "0|7|0.5\n200|10|0.5 0.5 13.9; 0.5 0.5 0.072\n590");
        assert_eq!(f.values["patches"], "inlet||||ground||");
        f.values.insert("region", "air".into());
        f.initial = f.values.clone();
        assert_eq!(f.params(spec).unwrap(), json!({"region": "air"}));
        assert!(!f.typed());
        // A station changed: the whole axis sent; a grading left blank is 1.
        f.values.insert("x", "0|7|0.5\n200|10|\n600".into());
        assert_eq!(f.params(spec).unwrap()["x"], json!([0, [7, 0.5], 200, [10, 1], 600]));
        assert!(f.typed());
        // A side renamed and one emptied: only those, the emptied one as null.
        f.values.insert("patches", "inflow||||||buildings".into());
        assert_eq!(
            f.params(spec).unwrap()["patches"],
            json!({"west": "inflow", "ground": null, "missing": "buildings"})
        );
        // Every missing block taken away: an empty list.
        f.values.insert("missing", " \n".into());
        assert_eq!(f.params(spec).unwrap()["missing"], json!([]));
        // What isn't what the field takes: said by row.
        f.values.insert("x", "0|7|0.5\nfar|10|1\n600".into());
        assert!(f.params(spec).unwrap_err().contains("x (row 2): the station `far`"));
        f.values.insert("x", "0|seven|1\n600".into());
        assert!(f.params(spec).unwrap_err().contains("aren't a whole number"));
        f.values.insert("x", "0|7|1 2\n600".into());
        assert!(f.params(spec).unwrap_err().contains("the grading `1 2`"));
        f.values.insert("x", String::new());
        assert!(f.params(spec).unwrap_err().contains("isn't taken away"));
        f.values.insert("x", "0|7|1|9\n600".into());
        assert!(f.params(spec).unwrap_err().contains("more than station|cells|grading"));
        // Sections typed with tabs or commas; a block with commas.
        f.values.insert("x", "0|7|0.5\t0.5\t2;0.5,0.5,1\n600".into());
        assert_eq!(f.params(spec).unwrap()["x"], json!([0, [7, [[0.5, 0.5, 2], [0.5, 0.5, 1]]], 600]));
        f.values.insert("missing", "1,1,0".into());
        assert_eq!(f.params(spec).unwrap()["missing"], json!([[1, 1, 0]]));
        f.values.insert("x", f.initial["x"].clone());
        f.values.insert("missing", "1 1".into());
        assert!(f.params(spec).unwrap_err().contains("`1 1` isn't a block"));
        // Back from JSON: a key left out stays as filled; patches merge over it; what the
        // fields can't hold is kept and sent as it is.
        f.fill(spec, &json!({"region": "air", "patches": {"top": "sky"}, "y": "bad", "types": {"sky": "patch"}}))
            .unwrap();
        assert_eq!(f.values["missing"], f.initial["missing"]);
        assert_eq!(f.values["patches"], "inlet||||ground|sky|");
        assert_eq!(
            f.params(spec).unwrap(),
            json!({"region": "air", "patches": {"top": "sky"}, "y": "bad", "types": {"sky": "patch"}})
        );
    }

    #[test]
    fn a_role_a_model_and_a_partner_are_sent_when_changed_and_taken_away_when_emptied() {
        let spec = spec("workflow.patch").unwrap();
        let mut f = FormState::default();
        for (k, v) in [("region", "air"), ("patch", "ground"), ("role", "ground"), ("partner", "")] {
            f.values.insert(k, v.to_string());
        }
        f.initial = f.values.clone();
        assert_eq!(f.params(spec).unwrap(), json!({"region": "air", "patch": "ground"}));
        f.values.insert("role", "coupled".into());
        f.values.insert("partner", "street|street_to_air".into());
        assert_eq!(
            f.params(spec).unwrap(),
            json!({"region": "air", "patch": "ground", "role": "coupled", "partner": {"region": "street", "patch": "street_to_air"}})
        );
        // Emptied: taken away; half a partner: said.
        f.initial = f.values.clone();
        f.values.insert("role", String::new());
        f.values.insert("partner", String::new());
        assert_eq!(f.params(spec).unwrap(), json!({"region": "air", "patch": "ground", "role": null, "partner": null}));
        f.values.insert("partner", "street|".into());
        assert!(f.params(spec).unwrap_err().contains("give the region and the patch"));
        // Back from JSON: null and an object go in; a partner of another shape is kept as it is.
        f.fill(spec, &json!({"region": "air", "patch": "ground", "partner": null, "role": "top"})).unwrap();
        assert_eq!(f.values["partner"], "");
        assert_eq!(f.values["role"], "top");
        f.fill(spec, &json!({"region": "air", "patch": "ground", "partner": ["street"]})).unwrap();
        assert_eq!(f.params(spec).unwrap()["partner"], json!(["street"]));
        // A switch is sent either way; names ticked as a list.
        let spec = super::spec("workflow.component").unwrap();
        let mut c = FormState::default();
        c.fill(spec, &json!({"component": "grass"})).unwrap();
        assert_eq!(c.params(spec).unwrap(), json!({"component": "grass", "on": true}));
        c.values.insert("on", "false".into());
        c.lists.entry("patches").or_default().insert("ground".into());
        assert_eq!(c.params(spec).unwrap(), json!({"component": "grass", "on": false, "patches": ["ground"]}));
        // A bad `on` from the JSON is sent as it is, for the command to refuse; ticked, the box wins.
        c.fill(spec, &json!({"component": "grass", "on": "off"})).unwrap();
        assert_eq!(c.params(spec).unwrap()["on"], json!("off"));
        c.values.insert("on", "false".into());
        assert_eq!(c.params(spec).unwrap()["on"], json!(false));
        // Taking out sends only what names it, whatever else the form was filled with.
        let spec = super::spec("workflow.region").unwrap();
        let mut r = FormState::default();
        r.fill(spec, &json!({"region": "street", "kind": "solid"})).unwrap();
        r.values.insert("remove", "true".into());
        assert_eq!(r.params(spec).unwrap(), json!({"region": "street", "remove": true}));
        let spec = super::spec("workflow.material").unwrap();
        let mut m = FormState::default();
        m.fill(spec, &json!({"region": "street", "material": "brick", "model": "HamstadBrick", "remove": true}))
            .unwrap();
        assert_eq!(m.params(spec).unwrap(), json!({"region": "street", "material": "brick", "remove": true}));
    }
}
