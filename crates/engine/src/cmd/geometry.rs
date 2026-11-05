//! `geometry.*`: the STL surfaces in `constant/triSurface` that `snappyHexMesh` and
//! `surfaceFeatures` read: importing one, checking them, and the safe repairs.
//!
//! A surface a command changes is held by the case model until `case.save` writes it (with a
//! backup of the file it replaces), and undo takes the change back.

use serde_json::{Value, json};
use umcf_core::{CaseFs, DirFs};
use umcf_geom::check::{Listed, Report, analyse};
use umcf_geom::repair;
use umcf_geom::stl::{self, Limits, Stl};
use umcf_geom::trees as trees_geom;
use umcf_model::{Case, MAX_FILE_BYTES};
use umcf_validate::surface::{diagnostics, unreadable};
use umcf_validate::{Diagnostic, Level};

use super::case::{count, diagnostic};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

/// Where a case's surfaces are.
pub const SURFACE_DIR: &str = "constant/triSurface";

/// The extensions OpenFOAM reads as STL (it picks the reader by extension, case-sensitively:
/// `.stlb` is always read as binary).
const EXTENSIONS: [&str; 4] = [".stl", ".stlb", ".stl.gz", ".stlb.gz"];

fn always(_: &Session) -> Result<(), String> {
    Ok(())
}

fn is_surface_name(name: &str) -> bool {
    EXTENSIONS.iter().any(|e| name.len() > e.len() && name.ends_with(e))
}

/// An STL name whose extension OpenFOAM doesn't recognise only for its case (`.STL`).
fn is_miscased(name: &str) -> bool {
    !is_surface_name(name) && is_surface_name(&name.to_ascii_lowercase())
}

/// `name` with an STL extension in lower case (`Building.STL` → `Building.stl`).
fn lower_extension(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match EXTENSIONS.iter().rev().find(|e| lower.ends_with(*e)) {
        Some(e) => format!("{}{e}", name.get(..name.len() - e.len()).unwrap_or_default()),
        None => name.to_string(),
    }
}

fn is_stlb(rel: &str) -> bool {
    rel.ends_with(".stlb") || rel.ends_with(".stlb.gz")
}

/// The case-relative path of surface `name` (`buildings.stl` or
/// `constant/triSurface/buildings.stl`).
/// `allow_miscased` takes `.STL` too (to check such a file, not to make one).
pub(super) fn surface_path(id: &str, name: &str, allow_miscased: bool) -> Result<String, Error> {
    let file = name.strip_prefix("constant/triSurface/").unwrap_or(name);
    if file.is_empty() || file.starts_with('.') || file.contains(['/', '\\']) || file.chars().any(char::is_control) {
        return Err(Error::Params(format!(
            "{id}: `{name}` is not a surface of the case (a file name in {SURFACE_DIR})"
        )));
    }
    if !is_surface_name(file) && !(allow_miscased && is_miscased(file)) {
        return Err(Error::Params(format!(
            "{id}: `{name}` is not an STL file name (OpenFOAM reads {}, in lower case, as STL)",
            EXTENSIONS.join(", ")
        )));
    }
    Ok(format!("{SURFACE_DIR}/{file}"))
}

pub(super) fn limits() -> Limits {
    Limits { max_bytes: MAX_FILE_BYTES, ..Limits::default() }
}

fn listed(l: &Listed) -> Value {
    json!({ "count": l.count, "triangles": l.triangles })
}

fn report_json(file: &str, stl: &Stl, r: &Report) -> Value {
    let v = |p: umcf_geom::Vec3| json!([p.x, p.y, p.z]);
    json!({
        "file": file,
        "format": stl.format.label(),
        "openfoam": stl.openfoam,
        "notes": stl.notes,
        "triangles": r.triangles,
        "points": r.points,
        "regions": r.regions,
        "region_count": r.region_count,
        "bbox": r.bbox.map(|b| json!({ "min": v(b.min), "max": v(b.max) })),
        "parts": r.parts,
        "closed_parts": r.closed_parts,
        "open_edges": r.open_edges,
        "non_manifold_edges": r.non_manifold_edges,
        "inconsistent_edges": r.inconsistent_edges,
        "inward_parts": r.inward_parts,
        "open": listed(&r.open),
        "non_manifold": listed(&r.non_manifold),
        "inconsistent": listed(&r.inconsistent),
        "inward": listed(&r.inward),
        "degenerate": listed(&r.degenerate),
        "duplicate": listed(&r.duplicate),
        "non_finite": listed(&r.non_finite),
        "stored_normals": listed(&r.stored_normals),
    })
}

/// A surface's report and diagnostics, from its bytes.
fn check_bytes(file: &str, bytes: &[u8]) -> (Value, Vec<Diagnostic>) {
    match stl::read(bytes, &limits()) {
        Ok(stl) => {
            let r = analyse(&stl);
            let mut d = diagnostics(file, &stl, &r);
            if is_miscased(file.rsplit('/').next().unwrap_or(file)) {
                let message = format!(
                    "OpenFOAM doesn't know the extension (it reads only {}, in lower case): rename the file",
                    EXTENSIONS.join(", ")
                );
                let code = "surface.extension";
                let path = file.to_string();
                d.insert(
                    0,
                    Diagnostic { level: Level::Error, code, path, region: None, patch: None, field: None, message },
                );
            }
            (report_json(file, &stl, &r), d)
        }
        Err(e) => (json!({ "file": file, "error": e.to_string() }), vec![unreadable(file, &e.to_string())]),
    }
}

/// The numbers of errors, warnings and notes, and a line saying so.
fn tally(all: &[Diagnostic]) -> (usize, usize, usize, String) {
    let n = |l: Level| all.iter().filter(|d| d.level == l).count();
    let (e, w, i) = (n(Level::Error), n(Level::Warning), n(Level::Info));
    (e, w, i, format!("{}, {}, {}", count(e, "error"), count(w, "warning"), count(i, "note")))
}

/// Reads a file anywhere on this machine (not in the case), up to [`MAX_FILE_BYTES`].
fn read_outside(id: &str, path: &str) -> Result<Vec<u8>, Error> {
    let p = std::path::Path::new(path);
    let name =
        p.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::Params(format!("{id}: `{path}` is not a file")))?;
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    DirFs::new(dir)
        .read(name, MAX_FILE_BYTES)
        .map_err(|e| Error::Failed(format!("{path}: {e}")))?
        .ok_or_else(|| Error::Failed(format!("{path}: no such file")))
}

/// The bytes a surface is stored as under `rel`: in its own format (binary for a `.stlb`,
/// which OpenFOAM reads as binary), compressed when the name ends in `.gz`; and how many
/// triangles with coordinates that are not numbers were left out.
fn encode(stl: &Stl, rel: &str) -> (Vec<u8>, usize) {
    let dropped = stl.triangles.iter().filter(|t| !t.is_finite()).count();
    let bytes = if is_stlb(rel) && stl.format != stl::Format::Binary {
        let mut b = stl.clone();
        b.format = stl::Format::Binary;
        b.solids.clear();
        b.to_bytes()
    } else {
        stl.to_bytes()
    };
    (if rel.ends_with(".gz") { stl::gzip(&bytes) } else { bytes }, dropped)
}

fn left_out(n: usize) -> String {
    format!("{} with coordinates that are not numbers left out", count(n, "triangle"))
}

/// Puts `bytes` into the case as surface `rel` and checks them. `notes` replaces what reading
/// them notes (a file written again keeps the notes of the one it came from).
fn store(
    s: &mut Session,
    rel: &str,
    bytes: Vec<u8>,
    notes: Option<Vec<String>>,
) -> Result<(Value, Vec<Diagnostic>), Error> {
    let mut stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{rel} doesn't read back: {e}")))?;
    if let Some(n) = notes {
        stl.notes = n;
    }
    let r = analyse(&stl);
    let out = (report_json(rel, &stl, &r), diagnostics(rel, &stl, &r));
    let o = s.opened_mut()?;
    o.case.put_file(o.fs.as_ref(), rel, bytes)?;
    Ok(out)
}

/// The STL surfaces of the case (case-relative paths, sorted): the files in [`SURFACE_DIR`]
/// with an STL name (and, with `miscased`, those whose extension is in the wrong case), and
/// those commands put there. The error says why the directory couldn't be listed.
pub(super) fn surface_files(case: &Case, fs: &dyn CaseFs, miscased: bool) -> (Vec<String>, Result<(), String>) {
    let (mut names, listed): (Vec<String>, _) = match fs.list(SURFACE_DIR) {
        Ok(n) => (n.into_iter().map(|n| format!("{SURFACE_DIR}/{n}")).collect(), Ok(())),
        Err(e) => (Vec::new(), Err(e.to_string())),
    };
    names.retain(|n| fs.is_file(n));
    names.extend(case.put_paths().filter(|p| p.starts_with("constant/triSurface/")).map(String::from));
    names.retain(|n| n.rsplit('/').next().is_some_and(|f| is_surface_name(f) || (miscased && is_miscased(f))));
    names.sort();
    names.dedup();
    (names, listed)
}

/// The surface `rel` of the open case, as it now is.
fn load(s: &Session, rel: &str) -> Result<(std::sync::Arc<Vec<u8>>, Stl), Error> {
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let bytes = case.file(fs, rel)?.ok_or_else(|| Error::Failed(format!("there is no surface {rel}")))?;
    let stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{rel}: {e}")))?;
    Ok((bytes, stl))
}

fn result(message: String, mut data: Value, diags: &[Diagnostic]) -> Outcome {
    let (e, w, i, line) = tally(diags);
    if let Some(m) = data.as_object_mut() {
        m.insert("errors".to_string(), json!(e));
        m.insert("warnings".to_string(), json!(w));
        m.insert("infos".to_string(), json!(i));
        m.insert("diagnostics".to_string(), diags.iter().map(diagnostic).collect());
    }
    Outcome::new(format!("{message}; {line}"), data)
}

pub(super) const CHECK: CommandSpec = CommandSpec {
    id: "geometry.check",
    label: "Check surfaces",
    menu: "Geometry/Check surfaces",
    shortcut: None,
    params: "file: one surface of the case (`buildings.stl` or `constant/triSurface/buildings.stl`). path: an STL file anywhere instead (no case needed). Neither: every STL surface in constant/triSurface (.stl, .stlb, and .gz-compressed ones). Reads each without trusting its header and reports what breaks meshing: a file OpenFOAM can't read, open and non-manifold edges, inconsistent and inward normals, degenerate and duplicate triangles, coordinates that look like millimetres or are far from the origin; with the triangles involved (the first 1000 of each).",
    enabled: always,
    run: check,
    journal: false,
    undoable: false,
};

fn check(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(CHECK.id, p, &["file", "path"])?;
    let mut surfaces = Vec::new();
    let mut all = Vec::new();
    match (p.str("path")?, p.str("file")?) {
        (Some(_), Some(_)) => return Err(Error::Params("geometry.check: give `file` or `path`, not both".to_string())),
        (Some(path), None) => {
            let bytes = read_outside(CHECK.id, path)?;
            let (j, d) = check_bytes(path, &bytes);
            surfaces.push(j);
            all.extend(d);
        }
        (None, file) => {
            let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| {
                Error::Failed("no case is open (give `path` to check a file outside a case)".to_string())
            })?;
            let files = match file {
                Some(f) => vec![surface_path(CHECK.id, f, true)?],
                None => {
                    let (names, listed) = surface_files(case, fs, true);
                    if let Err(e) = listed {
                        all.push(unreadable(SURFACE_DIR, &format!("the directory can't be listed: {e}")));
                    }
                    names
                }
            };
            for rel in files {
                match case.file(fs, &rel) {
                    Ok(Some(bytes)) => {
                        let (j, d) = check_bytes(&rel, &bytes);
                        surfaces.push(j);
                        all.extend(d);
                    }
                    Ok(None) => return Err(Error::Failed(format!("there is no surface {rel}"))),
                    Err(e) => all.push(unreadable(&rel, &e.to_string())),
                }
            }
        }
    }
    if surfaces.is_empty() && all.is_empty() {
        let data = json!({ "surfaces": [], "errors": 0, "warnings": 0, "infos": 0, "diagnostics": [] });
        return Ok(Outcome::new(format!("no STL surface in {SURFACE_DIR}"), data));
    }
    let message = count(surfaces.len(), "surface");
    Ok(result(message, json!({ "surfaces": surfaces }), &all))
}

pub(super) const IMPORT: CommandSpec = CommandSpec {
    id: "geometry.import",
    label: "Import surface",
    menu: "Geometry/Import STL…",
    shortcut: None,
    params: "path: the STL file (ASCII or binary, gzip-compressed too; required). name: its name in constant/triSurface (default: the file's own name; .stl, .stlb, .stl.gz or .stlb.gz). replace: true to replace a surface of that name. scale: a factor for the coordinates (0.001 for millimetres). The file is copied as it is, unless it is scaled or OpenFOAM can't read it as it is: then it is written again. case.save writes it. Reports what geometry.check reports.",
    enabled: needs_case,
    run: import,
    journal: true,
    undoable: true,
};

fn import(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(IMPORT.id, p, &["path", "name", "replace", "scale"])?;
    let path = p.required_str("path")?;
    let own = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or_default();
    // `Building.STL` comes in as `Building.stl`: OpenFOAM knows only the lower-case extension.
    let rel = surface_path(IMPORT.id, &p.str("name")?.map_or_else(|| lower_extension(own), String::from), false)?;
    let replace = p.bool("replace")?.unwrap_or(false);
    let scale = p.number("scale")?;
    if scale.is_some_and(|f| f <= 0.0) {
        return Err(Error::Params("geometry.import: `scale` is a positive number".to_string()));
    }
    {
        let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
        if !replace && (case.put_paths().any(|p| p == rel) || fs.is_file(&rel)) {
            return Err(Error::Failed(format!("{rel} is already there: pass \"replace\": true to replace it")));
        }
    }
    let bytes = read_outside(IMPORT.id, path)?;
    let mut stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{path}: {e}")))?;
    let mut why = Vec::new();
    if let Some(f) = scale {
        if !repair::scale(&mut stl, f) {
            return Err(Error::Params(format!(
                "geometry.import: scaling by {f} takes coordinates beyond what STL holds"
            )));
        }
        why.push(format!("scaled by {f}"));
    }
    let stlb = is_stlb(&rel);
    if stl.openfoam.is_some() || (stl.header_solid && !stlb) || (stlb && stl.format != stl::Format::Binary) {
        why.push("written again, as OpenFOAM can't read it as it was".to_string());
    }
    // A compressed file under a plain name (or the other way round) is written again too.
    let gz = bytes.starts_with(&[0x1f, 0x8b]);
    if gz != rel.ends_with(".gz") {
        why.push(if gz { "decompressed".to_string() } else { "compressed".to_string() });
    }
    let (out, notes) = if why.is_empty() {
        (bytes, None)
    } else {
        let notes = std::mem::take(&mut stl.notes);
        let (out, dropped) = encode(&stl, &rel);
        if dropped > 0 {
            why.push(left_out(dropped));
        }
        (out, Some(notes))
    };
    let (surface, diags) = store(s, &rel, out, notes)?;
    let how = if why.is_empty() { String::new() } else { format!(" ({})", why.join(", ")) };
    let message = format!("{rel}: imported from {path}{how}");
    Ok(result(message, json!({ "file": rel, "rewritten": why, "surface": surface }), &diags))
}

pub(super) const TREES: CommandSpec = CommandSpec {
    id: "geometry.trees",
    label: "Tree crowns",
    menu: "Geometry/Tree crowns…",
    shortcut: None,
    params: "The trees, one of: table: a file of the case, one tree a line (x y z CD CH: the crown's base, diameter and height; a first line of these column names in any order, or none; tabs, spaces or commas); path: such a file outside the case; trees: [{\"x\": …, \"y\": …, \"z\": …, \"diameter\": …, \"height\": …}]. name: the surface in constant/triSurface (default trees.stl). solid: the surface's one region (default trees; a binary .stlb has none, and refuses it). replace: true to replace a surface of that name. Each crown is a rhombicuboctahedron of the tree's crown diameter and height standing on its z, closed and wound outward (44 triangles). case.save writes it. Reports what geometry.check reports.",
    enabled: needs_case,
    run: trees,
    journal: true,
    undoable: true,
};

fn trees(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(TREES.id, p, &["table", "path", "trees", "name", "solid", "replace"])?;
    let rel = surface_path(TREES.id, p.str("name")?.unwrap_or("trees.stl"), false)?;
    let solid = stl::foam_name(p.str("solid")?.unwrap_or("trees"));
    // The ASCII writer keeps printable ASCII only: a name it would change is refused, so the
    // region is the name reported.
    if !solid.chars().all(|c| c.is_ascii_graphic()) {
        return Err(Error::Params(format!(
            "{}: `{solid}` as a region name: printable ASCII only (letters, digits, `_` …), as STL writes it",
            TREES.id
        )));
    }
    // Binary STL has no region names: the region would be OpenFOAM's `patch0`.
    if is_stlb(&rel) && p.value("solid").is_some_and(|v| !v.is_null()) {
        return Err(Error::Params(format!(
            "{}: a binary surface ({rel}) has no region names: give it an .stl name to keep `solid`",
            TREES.id
        )));
    }
    let replace = p.bool("replace")?.unwrap_or(false);
    let given = |k: &str| p.value(k).is_some_and(|v| !v.is_null());
    if ["table", "path", "trees"].iter().filter(|k| given(k)).count() != 1 {
        return Err(Error::Params(format!("{}: give the trees one way: table, path or trees", TREES.id)));
    }
    {
        let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
        if !replace && (case.put_paths().any(|p| p == rel) || fs.is_file(&rel)) {
            return Err(Error::Failed(format!("{rel} is already there: pass \"replace\": true to replace it")));
        }
    }
    let list: Vec<trees_geom::Tree> = if given("trees") {
        let items = p
            .value("trees")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Params(format!("{}: `trees` is a list", TREES.id)))?;
        if items.len() > trees_geom::MAX_TREES {
            return Err(Error::Params(format!("{}: more than {} trees", TREES.id, trees_geom::MAX_TREES)));
        }
        let mut out = Vec::with_capacity(items.len());
        for (k, t) in items.iter().enumerate() {
            let num = |key: &str| {
                t.get(key)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| Error::Params(format!("{}: tree {k}: `{key}` is a number", TREES.id)))
            };
            let tree = trees_geom::Tree {
                x: num("x")?,
                y: num("y")?,
                z: num("z")?,
                diameter: num("diameter")?,
                height: num("height")?,
            };
            if let Some(why) = tree.problem() {
                return Err(Error::Params(format!("{}: tree {k}: {why}", TREES.id)));
            }
            out.push(tree);
        }
        out
    } else {
        let (what, bytes) = if given("path") {
            let path = p.required_str("path")?;
            (path.to_string(), read_outside(TREES.id, path)?)
        } else {
            let table = p.required_str("table")?;
            let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
            let bytes =
                case.file(fs, table)?.ok_or_else(|| Error::Failed(format!("{table}: no such file in the case")))?;
            (table.to_string(), bytes.to_vec())
        };
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Failed(format!("{what}: not text")))?;
        trees_geom::read_table(text).map_err(|e| Error::Failed(format!("{what}: {e}")))?
    };
    if list.is_empty() {
        return Err(Error::Params(format!("{}: no trees", TREES.id)));
    }
    let stl = trees_geom::crowns(&list, &solid);
    let (out, dropped) = encode(&stl, &rel);
    if dropped > 0 {
        return Err(Error::Failed(format!("{}: {}", TREES.id, left_out(dropped))));
    }
    let (surface, diags) = store(s, &rel, out, None)?;
    let (message, solid) = if is_stlb(&rel) {
        (format!("{rel}: {} as crowns (binary: no region name)", count(list.len(), "tree")), Value::Null)
    } else {
        (format!("{rel}: {} as crowns ({solid})", count(list.len(), "tree")), json!(solid))
    };
    Ok(result(message, json!({ "file": rel, "trees": list.len(), "solid": solid, "surface": surface }), &diags))
}

pub(super) const REPAIR: CommandSpec = CommandSpec {
    id: "geometry.repair",
    label: "Repair surface",
    menu: "Geometry/Repair…",
    shortcut: None,
    params: "file: a surface of the case (required). weld: a distance in metres: each vertex within it of one before it is moved onto that one, which closes gaps narrower than that (triangles that lose their area are removed). degenerate: true to remove triangles without area, copies of others and ones with coordinates that are not numbers. orient: true to make the normals of each part consistent and point those of closed parts outward. Without weld, degenerate or orient: degenerate and orient. They run in that order. The surface is written again in its own format (which also fixes a file OpenFOAM can't read as it is); case.save writes it.",
    enabled: needs_case,
    run: repair_cmd,
    journal: true,
    undoable: true,
};

fn repair_cmd(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(REPAIR.id, p, &["file", "weld", "degenerate", "orient"])?;
    let rel = surface_path(REPAIR.id, p.required_str("file")?, false)?;
    let weld = p.number("weld")?;
    let (mut degenerate, mut orient) = (p.bool("degenerate")?, p.bool("orient")?);
    if weld.is_none() && degenerate.is_none() && orient.is_none() {
        (degenerate, orient) = (Some(true), Some(true));
    }
    if weld.is_some_and(|w| w <= 0.0) {
        return Err(Error::Params("geometry.repair: `weld` is a positive distance in metres".to_string()));
    }
    let (bytes, mut stl) = load(s, &rel)?;
    let mut done = Vec::new();
    let mut data = serde_json::Map::new();
    if let Some(t) = weld {
        let w = repair::weld(&mut stl, t);
        done.push(format!("{} moved, {} removed by welding", count(w.moved, "vertex"), count(w.removed, "triangle")));
        data.insert("weld".to_string(), json!({ "tolerance": t, "moved": w.moved, "removed": w.removed }));
    }
    if degenerate == Some(true) {
        let d = repair::drop_degenerate(&mut stl);
        let mut line = format!(
            "{} removed ({} without area, {} copies, {} not finite)",
            count(d.removed(), "triangle"),
            d.degenerate + d.split,
            d.duplicate,
            d.non_finite
        );
        if d.split > 0 {
            line.push_str(&format!(", {} of them by splitting the triangle across their long edge", d.split));
        }
        if d.left > 0 {
            line.push_str(&format!(
                ", {} left: removing them would open the surface (see the user guide)",
                count(d.left, "triangle without area")
            ));
        }
        done.push(line);
        data.insert(
            "degenerate".to_string(),
            json!({ "degenerate": d.degenerate, "split": d.split, "left": d.left, "duplicate": d.duplicate, "non_finite": d.non_finite }),
        );
    }
    if orient == Some(true) {
        let o = repair::orient(&mut stl);
        let mut line = format!("{} turned over", count(o.flipped, "triangle"));
        if o.outward > 0 {
            line.push_str(&format!(" ({} turned outward)", count(o.outward, "closed part")));
        }
        if o.conflicts > 0 {
            line.push_str(&format!(", {} with no consistent side left as they were", count(o.conflicts, "edge")));
        }
        done.push(line);
        data.insert(
            "orient".to_string(),
            json!({ "flipped": o.flipped, "outward": o.outward, "conflicts": o.conflicts }),
        );
    }
    let (out, dropped) = encode(&stl, &rel);
    if dropped > 0 {
        done.push(left_out(dropped));
    }
    let unchanged = out.as_slice() == bytes.as_slice();
    let (surface, diags) = store(s, &rel, out, Some(Vec::new()))?;
    data.insert("file".to_string(), json!(rel));
    data.insert("surface".to_string(), surface);
    let message = if unchanged { format!("{rel}: nothing to change") } else { format!("{rel}: {}", done.join("; ")) };
    Ok(result(message, Value::Object(data), &diags))
}

pub(super) const SCALE: CommandSpec = CommandSpec {
    id: "geometry.scale",
    label: "Scale surface",
    menu: "Geometry/Scale…",
    shortcut: None,
    params: "file: a surface of the case (required). factor: a positive number the coordinates are multiplied by (0.001 turns millimetres into metres; required). The surface is written again in its own format; case.save writes it.",
    enabled: needs_case,
    run: scale,
    journal: true,
    undoable: true,
};

fn scale(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let p = Params::new(SCALE.id, p, &["file", "factor"])?;
    let rel = surface_path(SCALE.id, p.required_str("file")?, false)?;
    let factor =
        p.number("factor")?.ok_or_else(|| Error::Params("geometry.scale: `factor` is required".to_string()))?;
    let (_, mut stl) = load(s, &rel)?;
    if !repair::scale(&mut stl, factor) {
        return Err(Error::Params(
            "geometry.scale: `factor` is a positive number that keeps the coordinates within what STL holds"
                .to_string(),
        ));
    }
    let (out, dropped) = encode(&stl, &rel);
    let (surface, diags) = store(s, &rel, out, Some(Vec::new()))?;
    let mut message = format!("{rel}: scaled by {factor}");
    if dropped > 0 {
        message.push_str(&format!("; {}", left_out(dropped)));
    }
    Ok(result(message, json!({ "file": rel, "factor": factor, "surface": surface }), &diags))
}
