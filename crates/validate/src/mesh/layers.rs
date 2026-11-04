//! Layer coverage, from the summary `snappyHexMesh` prints at the end of its layer phase
//! (OpenFOAM-8 and 12 print the same tables, `snappyLayerDriver.C`): for each patch that asked
//! for layers, how many it wanted (first table) and how many it got on average, with the
//! thickness reached as a percentage of the one wanted (last table).
//!
//! ```text
//! patch                       faces    layers avg thickness[m]
//!                                             near-wall overall
//! -----                       -----    ------ --------- -------
//! twoFridgeFreezers_seal_0    800      3      0.0497    0.149
//! …
//! patch                       faces    layers   overall thickness
//!                                               [m]       [%]
//! -----                       -----    ------   ---       ---
//! twoFridgeFreezers_seal_0    800      1.96     0.0766    49
//! ```

use crate::{Diagnostic, Level};

/// Below this fraction of the layers wanted (on average over a patch's faces), a patch's
/// layers are reported.
pub const LAYER_COVERAGE_WARN: f64 = 0.9;

/// The most patches read from one table.
pub const MAX_LAYER_PATCHES: usize = 10_000;

/// The most characters kept of a text taken from a log (a patch name, the command line, a
/// fatal error's message): a log is hostile input, and a line can be megabytes long.
pub const MAX_LOG_TEXT: usize = 256;

/// `s`, cut to [`MAX_LOG_TEXT`] characters (on a character boundary), with `…` when cut.
fn clip(s: &str) -> String {
    match s.char_indices().nth(MAX_LOG_TEXT) {
        Some((at, _)) => format!("{}…", s.get(..at).unwrap_or_default()),
        None => s.to_string(),
    }
}

/// One patch of the layer summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerPatch {
    pub name: String,
    pub faces: u64,
    /// Layers wanted (the first table); `None` when the log doesn't say.
    pub wanted: Option<f64>,
    /// Overall thickness wanted, m.
    pub wanted_thickness: Option<f64>,
    /// Layers grown, the average over the patch's faces.
    pub layers: f64,
    /// Overall thickness reached, m, and as a percentage of the one wanted.
    pub thickness: f64,
    pub percent: f64,
}

impl LayerPatch {
    /// Layers grown over layers wanted.
    pub fn coverage(&self) -> Option<f64> {
        self.wanted.filter(|w| *w > 0.0).map(|w| self.layers / w)
    }
}

/// What a `snappyHexMesh` log says about layers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnappyLog {
    /// The log file (case-relative).
    pub file: String,
    /// `-region`'s value, or `None` when the command line has none.
    pub region: Option<String>,
    /// `Build : 12`.
    pub version: Option<String>,
    pub command: String,
    /// The layer phase ran (`Shrinking and layer addition phase`).
    pub layer_phase: bool,
    /// The last layer summary (empty without a layer phase).
    pub patches: Vec<LayerPatch>,
    /// The mesh it wrote last (`Layer mesh : cells:… faces:… points:…`, or the snapped or
    /// refined mesh without layers).
    pub cells: Option<u64>,
    pub faces: Option<u64>,
    pub points: Option<u64>,
    /// `Finished meshing without any errors`.
    pub finished: bool,
    /// `FOAM FATAL ERROR`: the message's first line.
    pub fatal: Option<String>,
    /// `No layers to generate`: the layer phase ran, but no patch asked for layers (or the
    /// names in `layers` match none).
    pub no_layers: bool,
    /// A table had more than [`MAX_LAYER_PATCHES`] patches: the rest are not listed.
    pub truncated: bool,
}

/// `Layer mesh : cells:11274  faces:37893  points:15906` → the three counts.
fn mesh_size(t: &str) -> Option<[u64; 3]> {
    let (_, rest) = t.split_once(" mesh : ")?;
    let mut out = [0u64; 3];
    for (slot, key) in out.iter_mut().zip(["cells:", "faces:", "points:"]) {
        let at = rest.find(key)?;
        let num: String = rest.get(at + key.len()..)?.chars().take_while(char::is_ascii_digit).collect();
        *slot = num.parse().ok()?;
    }
    Some(out)
}

/// Reads a `snappyHexMesh` log; `None` when it is not one (its `Exec` line runs something
/// else, or there is none).
pub fn parse_snappy_log(file: &str, text: &str) -> Option<SnappyLog> {
    let mut log = SnappyLog { file: file.to_string(), ..SnappyLog::default() };
    let mut is_snappy = false;
    // The table being read: the wanted one (`near-wall`) or the result (`[%]`), after its
    // `-----` line.
    #[derive(PartialEq)]
    enum Table {
        None,
        WantedHeader,
        Wanted,
        ResultHeader,
        Result,
    }
    let mut table = Table::None;
    let mut wanted: Vec<(String, f64, f64)> = Vec::new();
    let mut result: Vec<LayerPatch> = Vec::new();
    let mut fatal_next = false;
    for line in text.lines() {
        let t = line.trim();
        // A parallel run prefixes each line with its processor: `[0] `.
        let t = match t.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((n, rest)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => rest.trim(),
            _ => t,
        };
        if fatal_next && !t.is_empty() {
            log.fatal = Some(clip(t));
            fatal_next = false;
            continue;
        }
        if let Some(cmd) = t.strip_prefix("Exec").map(str::trim_start).and_then(|r| r.strip_prefix(':')) {
            // A run appended to the log starts afresh: the last one made the mesh.
            let version = log.version.take();
            log = SnappyLog { file: file.to_string(), version, ..SnappyLog::default() };
            (table, fatal_next) = (Table::None, false);
            wanted.clear();
            result.clear();
            let words: Vec<&str> = cmd.split_whitespace().collect();
            is_snappy = words.first().is_some_and(|w| w.rsplit('/').next() == Some("snappyHexMesh"));
            log.region = words.iter().position(|w| *w == "-region").and_then(|k| words.get(k + 1)).map(|s| clip(s));
            log.command = clip(&words.join(" "));
            continue;
        }
        if let Some(v) = t.strip_prefix("Build").map(str::trim_start).and_then(|r| r.strip_prefix(':')) {
            // The banner, before `Exec`.
            log.version = Some(clip(v.trim()));
            continue;
        }
        if t.starts_with("--> FOAM FATAL") {
            log.fatal = Some(String::new());
            fatal_next = true;
            continue;
        }
        if t == "Shrinking and layer addition phase" {
            log.layer_phase = true;
        } else if t.starts_with("No layers to generate") {
            log.no_layers = true;
        } else if t.starts_with("Finished meshing without any errors") {
            log.finished = true;
        } else if let Some([c, f, p]) = mesh_size(t) {
            (log.cells, log.faces, log.points) = (Some(c), Some(f), Some(p));
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        // A number of a row: finite (Rust reads `nan` and `inf`, which OpenFOAM doesn't print).
        let num = |w: &str| w.parse::<f64>().ok().filter(|x| x.is_finite());
        // A row of the table being read, if it parses; anything else ends the table.
        let row = match (&table, words.as_slice()) {
            (Table::Wanted, [name, _faces, layers, _near, overall]) => match (num(layers), num(overall)) {
                (Some(l), Some(o)) if wanted.len() < MAX_LAYER_PATCHES => {
                    wanted.push((clip(name), l, o));
                    true
                }
                (Some(_), Some(_)) => {
                    log.truncated = true;
                    true
                }
                _ => false,
            },
            (Table::Result, [name, faces, layers, thickness, percent]) => {
                match (faces.parse::<u64>().ok(), num(layers), num(thickness), num(percent)) {
                    (Some(f), Some(l), Some(th), Some(pc)) if result.len() < MAX_LAYER_PATCHES => {
                        result.push(LayerPatch {
                            name: clip(name),
                            faces: f,
                            layers: l,
                            thickness: th,
                            percent: pc,
                            ..LayerPatch::default()
                        });
                        true
                    }
                    (Some(_), Some(_), Some(_), Some(_)) => {
                        log.truncated = true;
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if row {
            continue;
        }
        match (&table, words.as_slice()) {
            (_, ["patch", "faces", "layers", "avg", ..]) => {
                table = Table::WantedHeader;
                wanted.clear();
            }
            (_, ["patch", "faces", "layers", "overall", ..]) => {
                table = Table::ResultHeader;
                result.clear();
            }
            (Table::WantedHeader, [first, ..]) if first.starts_with("-----") => table = Table::Wanted,
            (Table::ResultHeader, [first, ..]) if first.starts_with("-----") => table = Table::Result,
            (Table::WantedHeader | Table::ResultHeader, _) => {}
            // A blank line or anything else ends a table.
            (Table::Wanted | Table::Result, _) => {
                if table == Table::Result {
                    log.patches = std::mem::take(&mut result);
                }
                table = Table::None;
            }
            (Table::None, _) => {}
        }
    }
    if table == Table::Result {
        log.patches = std::mem::take(&mut result);
    }
    if !is_snappy {
        return None;
    }
    for p in &mut log.patches {
        if let Some((_, l, o)) = wanted.iter().find(|(n, _, _)| *n == p.name) {
            (p.wanted, p.wanted_thickness) = (Some(*l), Some(*o));
        }
    }
    Some(log)
}

/// The findings of a `snappyHexMesh` log's layer summary, for `region` (`""` for a
/// single-region case): a warning per patch with less than [`LAYER_COVERAGE_WARN`] of its
/// layers (or none), one note for the others; a layer phase without layers to add, or without a
/// summary, says so.
pub fn layer_diagnostics(log: &SnappyLog, region: &str) -> Vec<Diagnostic> {
    let version = log.version.as_deref().map_or(String::new(), |v| format!(", OpenFOAM-{v}"));
    let source = format!("snappyHexMesh ({}{version})", log.file);
    let mut out = Vec::new();
    let mut push = |level, code, patch: Option<&str>, message: String| {
        let mut d = Diagnostic::new(level, code, &log.file, message);
        if !region.is_empty() {
            d = d.region(region);
        }
        if let Some(p) = patch {
            d = d.patch(p);
        }
        out.push(d);
    };
    let (mut good, mut unknown): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for p in &log.patches {
        let wanted = p.wanted.map_or(String::new(), |w| format!(" of {w}"));
        let detail = format!(
            "{} layers{wanted} on average over {} faces, {:.3} m thick ({}% of the thickness wanted)",
            p.layers, p.faces, p.thickness, p.percent
        );
        // The result table lists only patches that asked for layers.
        let asked = p.wanted.map_or("the number wanted isn't in the log".to_string(), |w| format!("{w} wanted"));
        match p.coverage() {
            _ if p.layers <= 0.0 && p.wanted != Some(0.0) => push(
                Level::Warning,
                "mesh.layers",
                Some(&p.name),
                format!("{source}: no layers grew on `{}` ({asked}): its wall cells are not layer cells", p.name),
            ),
            // Not known to be enough: listed apart, not among the patches that got their layers.
            None => unknown.push(format!("`{}` {}", p.name, p.layers)),
            Some(c) if c < LAYER_COVERAGE_WARN => push(
                Level::Warning,
                "mesh.layers",
                Some(&p.name),
                format!(
                    "{source}: `{}` got {:.0}% of its layers: {detail}; relax the layer controls (featureAngle, minThickness, the mesh quality controls) or refine near the wall",
                    p.name,
                    100.0 * c
                ),
            ),
            _ => good.push(format!("`{}` {}{wanted}", p.name, p.layers)),
        }
    }
    // The patches that got their layers: one note (an urban case can have hundreds).
    if !good.is_empty() {
        push(
            Level::Info,
            "mesh.layers",
            None,
            format!(
                "{source}: {} patches got at least {:.0}% of their layers: {}",
                good.len(),
                100.0 * LAYER_COVERAGE_WARN,
                good.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                    + if good.len() > 20 { ", …" } else { "" }
            ),
        );
    }
    if !unknown.is_empty() {
        push(
            Level::Info,
            "mesh.layers",
            None,
            format!(
                "{source}: {} patches grew layers, but the log doesn't say how many they wanted: {}",
                unknown.len(),
                unknown.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                    + if unknown.len() > 20 { ", …" } else { "" }
            ),
        );
    }
    if log.truncated {
        push(
            Level::Info,
            "mesh.layers",
            None,
            format!("{source}: the layer tables list more than {MAX_LAYER_PATCHES} patches; the rest are not read"),
        );
    }
    if log.no_layers {
        push(
            Level::Warning,
            "mesh.layers",
            None,
            format!(
                "{source}: addLayers is on, but no patch asked for layers: check the patch names under `layers` in addLayersControls"
            ),
        );
    } else if log.layer_phase && log.patches.is_empty() {
        let why = if log.fatal.is_some() || !log.finished {
            "the log stops before the layer summary"
        } else {
            "the log has no layer summary"
        };
        push(Level::Warning, "mesh.layers", None, format!("{source}: layers were added, but {why}"));
    }
    if let Some(f) = &log.fatal {
        push(
            Level::Warning,
            "mesh.snappyhexmesh",
            None,
            format!("{source}: snappyHexMesh stopped with a fatal error, so the mesh it wrote may be unfinished: {f}"),
        );
    }
    out
}
