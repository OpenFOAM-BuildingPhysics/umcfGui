//! The 3D scene and the selection: `scene.snapshot` draws the case's meshes or surfaces into a
//! PNG image with the CPU rasteriser; `select.pick` selects the patch or surface region under a
//! pixel (or along a ray), `select.set` and `select.clear` set the selection directly.
//!
//! The selection is plain data in the session ([`Sel`]): what a command run without an explicit
//! target acts on. The scene is built from the files the first time it is needed and kept until
//! what it shows changes (another case, another set of objects, a surface a command changed); a
//! mesh rewritten on disk by OpenFOAM is read again with `"reload": true`.

use std::sync::Arc;

use serde_json::{Value, json};
use umcf_foam_mesh::{Limits, PolyMesh, mesh_dir};
use umcf_geom::BBox;
use umcf_geom::Vec3;
use umcf_geom::bvh::Ray;
use umcf_geom::stl;
use umcf_render::camera::{Camera, View};
use umcf_render::raster::{ColourBy, MAX_SIDE, Style, draw};
use umcf_render::{Object, Pick, Scene, Source, png};

use super::case::count;
use super::geometry::{SURFACE_DIR, limits, surface_files, surface_path};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

/// One selected thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sel {
    /// A patch of a region's mesh (`region` is `""` in a single-region case).
    Patch { region: String, patch: String },
    /// A region of an STL surface (`file` case-relative), as OpenFOAM names it.
    Solid { file: String, solid: String },
}

impl Sel {
    pub fn to_json(&self) -> Value {
        match self {
            Sel::Patch { region, patch } => json!({ "region": region, "patch": patch }),
            Sel::Solid { file, solid } => json!({ "surface": file, "solid": solid }),
        }
    }

    /// The selection of class `class` of `object`.
    fn of(object: &Object, class: u32) -> Option<Sel> {
        let name = object.classes.get(class as usize)?.clone();
        Some(match &object.source {
            Source::Mesh { region } => Sel::Patch { region: region.clone(), patch: name },
            Source::Surface { file } => Sel::Solid { file: file.clone(), solid: name },
        })
    }
}

/// The scene last built and what it was built from.
pub(crate) struct Cached {
    opens: u64,
    regions: Vec<String>,
    /// Each surface, with the bytes a command put there (compared by identity: the case keeps
    /// the same `Arc` until the file is put again), or `None` when it was read from disk.
    surfaces: Vec<(String, Option<Arc<Vec<u8>>>)>,
    scene: Scene,
}

/// The parameters that say what the scene shows.
const WHICH: [&str; 3] = ["regions", "surfaces", "reload"];
/// The parameters that place the camera.
const CAMERA: [&str; 6] = ["view", "azimuth", "elevation", "zoom", "width", "height"];
const DEFAULT_SIZE: (u32, u32) = (1280, 800);
/// Most hits a pick lists.
const MAX_HITS: usize = 64;

fn strings<'a>(id: &str, key: &str, v: &'a Value) -> Result<Vec<&'a str>, Error> {
    let bad = || Error::Params(format!("{id}: `{key}` is a list of names"));
    v.as_array().ok_or_else(bad)?.iter().map(|x| x.as_str().ok_or_else(bad)).collect()
}

fn optional<'a>(p: &Params<'a>, key: &str) -> Result<Option<&'a Value>, Error> {
    match p.required(key) {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}

/// The scene the parameters ask for, built or as cached.
fn scene<'s>(s: &'s mut Session, id: &str, p: &Params) -> Result<&'s Scene, Error> {
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let with_mesh: Vec<&str> = case.regions.iter().filter(|r| r.patches.is_some()).map(|r| r.name.as_str()).collect();
    let regions: Vec<String> = match optional(p, "regions")? {
        Some(v) => {
            let mut out = Vec::new();
            for r in strings(id, "regions", v)? {
                if !case.regions.iter().any(|x| x.name == r) {
                    let all: Vec<&str> = case.regions.iter().map(|x| x.name.as_str()).collect();
                    return Err(Error::Params(format!("{id}: no region `{r}` (the case has: {})", all.join(", "))));
                }
                if !with_mesh.contains(&r) {
                    return Err(Error::Failed(format!("{id}: region `{r}` has no mesh yet")));
                }
                if !out.iter().any(|x: &String| x == r) {
                    out.push(r.to_string());
                }
            }
            out
        }
        None if optional(p, "surfaces")?.is_some() => Vec::new(),
        None => with_mesh.iter().map(|r| r.to_string()).collect(),
    };
    let surfaces: Vec<String> = match optional(p, "surfaces")? {
        Some(v) => {
            let mut out = Vec::new();
            for f in strings(id, "surfaces", v)? {
                let rel = surface_path(id, f, false)?;
                if !out.contains(&rel) {
                    out.push(rel);
                }
            }
            out
        }
        None if regions.is_empty() && optional(p, "regions")?.is_none() => surface_files(case, fs, false).0,
        None => Vec::new(),
    };
    if regions.is_empty() && surfaces.is_empty() {
        return Err(Error::Failed(format!(
            "{id}: nothing to show: the case has no mesh and no STL surface in {SURFACE_DIR}"
        )));
    }
    let versions: Vec<(String, Option<Arc<Vec<u8>>>)> =
        surfaces.iter().map(|f| (f.clone(), case.put_bytes(f).cloned())).collect();
    let reload = p.bool("reload")?.unwrap_or(false);
    let same = |c: &Cached| {
        c.opens == s.opens
            && c.regions == regions
            && c.surfaces.len() == versions.len()
            && c.surfaces.iter().zip(&versions).all(|((f, a), (g, b))| {
                f == g
                    && match (a, b) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    }
            })
    };
    if reload || !s.scene.as_ref().is_some_and(same) {
        let mut meshes = Vec::new();
        for r in &regions {
            let mesh = PolyMesh::read(fs, &mesh_dir(r), &Limits::default()).map_err(|e| {
                let which = if r.is_empty() { String::new() } else { format!(" of region {r}") };
                Error::Failed(format!("{id}: the mesh{which} can't be read: {e}"))
            })?;
            meshes.push((r, mesh));
        }
        let mut stls = Vec::new();
        for f in &surfaces {
            let bytes = case.file(fs, f)?.ok_or_else(|| Error::Failed(format!("{id}: there is no surface {f}")))?;
            let stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{id}: {f}: {e}")))?;
            stls.push((f, stl));
        }
        let boxes = meshes
            .iter()
            .filter_map(|(_, m)| {
                m.bounds()
                    .map(|(lo, hi)| BBox { min: Vec3::new(lo[0], lo[1], lo[2]), max: Vec3::new(hi[0], hi[1], hi[2]) })
            })
            .chain(stls.iter().filter_map(|(_, s)| s.bbox()));
        let mut scene = Scene::new(Scene::origin_for(boxes));
        for (r, m) in &meshes {
            scene.objects.push(Object::from_mesh(r, m, scene.origin));
        }
        for (f, stl) in &stls {
            scene.objects.push(Object::from_stl(f, stl, scene.origin));
        }
        s.scene = Some(Cached { opens: s.opens, regions, surfaces: versions, scene });
    }
    s.scene.as_ref().map(|c| &c.scene).ok_or_else(|| Error::Failed(format!("{id}: no scene")))
}

/// A pixel count from 1 to [`MAX_SIDE`].
fn side(id: &str, p: &Params, key: &str, default: u32) -> Result<u32, Error> {
    match p.number(key)? {
        None => Ok(default),
        Some(x) if x.fract() == 0.0 && x >= 1.0 && x <= f64::from(MAX_SIDE) => Ok(x as u32),
        Some(_) => Err(Error::Params(format!("{id}: `{key}` is a whole number of pixels from 1 to {MAX_SIDE}"))),
    }
}

/// The camera and image size the parameters ask for, framing `scene`.
fn camera(id: &str, p: &Params, scene: &Scene) -> Result<(Camera, u32, u32), Error> {
    let (w, h) = (side(id, p, "width", DEFAULT_SIZE.0)?, side(id, p, "height", DEFAULT_SIZE.1)?);
    let view = match p.str("view")? {
        None => View::Iso,
        Some(v) => View::parse(v).ok_or_else(|| {
            let all: Vec<&str> = View::ALL.iter().map(|(n, _)| *n).collect();
            Error::Params(format!("{id}: no view `{v}` (views: {})", all.join(", ")))
        })?,
    };
    let (az, el) = view.angles();
    let az = p.number("azimuth")?.unwrap_or(az);
    let el = p.number("elevation")?.unwrap_or(el);
    if !(-90.0..=90.0).contains(&el) {
        return Err(Error::Params(format!("{id}: `elevation` is from -90 to 90 degrees")));
    }
    let zoom = p.number("zoom")?.unwrap_or(1.0);
    if zoom <= 0.0 {
        return Err(Error::Params(format!("{id}: `zoom` is a positive number (2 is twice as close)")));
    }
    let bbox = scene.bbox().unwrap_or(BBox::at(scene.origin));
    let mut cam = Camera::fit_angles(az, el, &bbox, f64::from(w) / f64::from(h));
    cam.distance /= zoom;
    Ok((cam, w, h))
}

fn camera_json(c: &Camera) -> Value {
    json!({
        "target": [c.target.x, c.target.y, c.target.z],
        "distance": c.distance,
        "azimuth": c.azimuth,
        "elevation": c.elevation,
        "fov": c.fov,
    })
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sym = |v: u32| char::from(ABC.get((v & 63) as usize).copied().unwrap_or(b'A'));
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = |i: usize| u32::from(c.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if c.len() > 1 { sym(n >> 6) } else { '=' });
        out.push(if c.len() > 2 { sym(n) } else { '=' });
    }
    out
}

fn objects_json(scene: &Scene) -> Value {
    scene
        .objects
        .iter()
        .map(|o| {
            let (key, value) = match &o.source {
                Source::Mesh { region } => ("region", region),
                Source::Surface { file } => ("surface", file),
            };
            json!({ key: value, "triangles": o.triangles.len(), "classes": o.classes })
        })
        .collect()
}

pub(super) const SNAPSHOT: CommandSpec = CommandSpec {
    id: "scene.snapshot",
    label: "Snapshot",
    menu: "View/Snapshot",
    shortcut: None,
    params: "Draws the case in 3D with the CPU renderer and returns a PNG image (`png`, base64). What: regions (mesh regions, their boundary patches; \"\" in a single-region case) and surfaces (STL files in constant/triSurface); by default every region with a mesh, or every surface when there is none. reload: true reads the files again (after OpenFOAM rewrote a mesh). Camera: view (iso, top, bottom, front, back, left, right; default iso), azimuth and elevation (degrees, override the view's), zoom (default 1: the whole scene), width and height (pixels, default 1280 × 800, at most 8192). Style: edges (true: the faces' edges), colour_by (patch, default, or object). The selection is drawn in orange.",
    enabled: needs_case,
    run: snapshot,
    journal: false,
    undoable: false,
};

fn snapshot(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SNAPSHOT.id;
    let mut allowed = WHICH.to_vec();
    allowed.extend(CAMERA);
    allowed.extend(["edges", "colour_by"]);
    let p = Params::new(id, p, &allowed)?;
    let edges = p.bool("edges")?.unwrap_or(false);
    let colour_by = match p.str("colour_by")? {
        None | Some("patch") => ColourBy::Class,
        Some("object") => ColourBy::Object,
        Some(o) => return Err(Error::Params(format!("{id}: `colour_by` is patch or object, not `{o}`"))),
    };
    let selection = s.selection.clone();
    let scene = scene(s, id, &p)?;
    let (cam, width, height) = camera(id, &p, scene)?;
    let mut selected = Vec::new();
    for (k, o) in scene.objects.iter().enumerate() {
        for c in 0..o.classes.len() {
            let Ok(c) = u32::try_from(c) else { break };
            if Sel::of(o, c).is_some_and(|x| selection.contains(&x)) {
                selected.push((k, c));
            }
        }
    }
    let n_selected = selected.len();
    let image = draw(scene, &cam, &Style { width, height, edges, selected, colour_by });
    let file = png::encode(&image);
    let labels: Vec<String> = scene.objects.iter().map(Object::label).collect();
    let mut message = format!("{width} × {height} of {} ({})", labels.join(", "), count(scene.triangles(), "triangle"));
    if n_selected > 0 {
        message.push_str(&format!(", {n_selected} selected"));
    }
    let data = json!({
        "width": width,
        "height": height,
        "png": base64(&file),
        "bytes": file.len(),
        "camera": camera_json(&cam),
        "objects": objects_json(scene),
    });
    Ok(Outcome::new(message, data))
}

/// How a pick or `select.set` changes the selection.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Replace,
    Add,
    Remove,
    Toggle,
    /// Only report what is there.
    None,
}

fn mode(id: &str, p: &Params, allowed: &[(&str, Mode)]) -> Result<Mode, Error> {
    match p.str("mode")? {
        None => Ok(Mode::Replace),
        Some(m) => allowed.iter().find(|(n, _)| *n == m).map(|(_, x)| *x).ok_or_else(|| {
            let names: Vec<&str> = allowed.iter().map(|(n, _)| *n).collect();
            Error::Params(format!("{id}: `mode` is {}, not `{m}`", names.join(", ")))
        }),
    }
}

fn apply(selection: &mut Vec<Sel>, mode: Mode, items: Vec<Sel>) {
    match mode {
        Mode::Replace => {
            selection.clear();
            for i in items {
                if !selection.contains(&i) {
                    selection.push(i);
                }
            }
        }
        Mode::Add => {
            for i in items {
                if !selection.contains(&i) {
                    selection.push(i);
                }
            }
        }
        Mode::Remove => selection.retain(|x| !items.contains(x)),
        Mode::Toggle => {
            for i in items {
                match selection.iter().position(|x| *x == i) {
                    Some(k) => {
                        selection.remove(k);
                    }
                    None => selection.push(i),
                }
            }
        }
        Mode::None => {}
    }
}

fn selection_json(s: &[Sel]) -> Value {
    s.iter().map(Sel::to_json).collect()
}

fn vector(id: &str, key: &str, v: Option<&Value>) -> Result<[f64; 3], Error> {
    let bad = || Error::Params(format!("{id}: `{key}` is three finite numbers"));
    let a = v.and_then(Value::as_array).filter(|a| a.len() == 3).ok_or_else(bad)?;
    let x = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite()).ok_or_else(bad);
    Ok([x(0)?, x(1)?, x(2)?])
}

pub(super) const PICK: CommandSpec = CommandSpec {
    id: "select.pick",
    label: "Pick",
    menu: "",
    shortcut: None,
    params: "Selects the patch or surface region under a pixel of the view scene.snapshot draws with the same parameters (pixel: [x, y], from the top left), or along a ray in world coordinates (ray: {\"origin\": [x, y, z], \"direction\": [x, y, z]}). Lists everything under it, nearest first, one entry per patch or region (hits). depth: which of them to take (default 0, the nearest; 1 the one behind it: \"select other\"). mode: replace (default; nothing under it clears the selection), add, toggle, or none (only list). What and camera: as scene.snapshot (regions, surfaces, reload, view, azimuth, elevation, zoom, width, height).",
    enabled: needs_case,
    run: pick,
    journal: true,
    undoable: false,
};

fn pick(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = PICK.id;
    let mut allowed = WHICH.to_vec();
    allowed.extend(CAMERA);
    allowed.extend(["pixel", "ray", "depth", "mode"]);
    let p = Params::new(id, p, &allowed)?;
    let mode = mode(
        id,
        &p,
        &[("replace", Mode::Replace), ("add", Mode::Add), ("toggle", Mode::Toggle), ("none", Mode::None)],
    )?;
    let depth = match p.number("depth")? {
        None => 0,
        Some(d) if d.fract() == 0.0 && (0.0..=MAX_HITS as f64).contains(&d) => d as usize,
        Some(_) => return Err(Error::Params(format!("{id}: `depth` is a whole number from 0 to {MAX_HITS}"))),
    };
    let (pixel, ray) = (optional(&p, "pixel")?, optional(&p, "ray")?);
    let scene = scene(s, id, &p)?;
    let ray = match (pixel, ray) {
        (Some(px), None) => {
            let bad = || Error::Params(format!("{id}: `pixel` is [x, y], two finite numbers"));
            let a = px.as_array().filter(|a| a.len() == 2).ok_or_else(bad)?;
            let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite()).ok_or_else(bad);
            let (cam, w, h) = camera(id, &p, scene)?;
            // The pixel's middle, as the rasteriser samples it.
            cam.ray(c(0)?.floor(), c(1)?.floor(), w, h)
        }
        (None, Some(r)) => {
            if !r.is_object() || r.as_object().is_some_and(|o| o.keys().any(|k| k != "origin" && k != "direction")) {
                return Err(Error::Params(format!(
                    "{id}: `ray` is {{\"origin\": [x, y, z], \"direction\": [x, y, z]}}"
                )));
            }
            let direction = vector(id, "ray.direction", r.get("direction"))?;
            if direction.iter().all(|d| *d == 0.0) {
                return Err(Error::Params(format!("{id}: `ray.direction` is not zero")));
            }
            Ray { origin: vector(id, "ray.origin", r.get("origin"))?, direction }
        }
        _ => return Err(Error::Params(format!("{id}: give `pixel` or `ray` (one of them)"))),
    };
    // One entry per patch or region, at its nearest hit.
    let hits: Vec<Pick> = scene.pick(&ray, MAX_HITS * 4);
    let mut under: Vec<(Sel, &Pick)> = Vec::new();
    for h in &hits {
        let Some(sel) = scene.objects.get(h.object).and_then(|o| Sel::of(o, h.class)) else { continue };
        if !under.iter().any(|(x, _)| *x == sel) && under.len() < MAX_HITS {
            under.push((sel, h));
        }
    }
    let listed: Vec<Value> = under
        .iter()
        .map(|(sel, h)| {
            let mut j = sel.to_json();
            let index = if matches!(sel, Sel::Patch { .. }) { "face" } else { "triangle" };
            if let Some(o) = j.as_object_mut() {
                o.insert(index.to_string(), json!(h.index));
                o.insert("distance".to_string(), json!(h.distance));
                o.insert("point".to_string(), json!([h.point.x, h.point.y, h.point.z]));
            }
            j
        })
        .collect();
    let picked = under.get(depth).map(|(sel, _)| sel.clone());
    let n_under = under.len();
    let items: Vec<Sel> = picked.iter().cloned().collect();
    if !(items.is_empty() && mode != Mode::Replace) {
        apply(&mut s.selection, mode, items);
    }
    let what = |x: &Sel| match x {
        Sel::Patch { region, patch } if region.is_empty() => format!("patch {patch}"),
        Sel::Patch { region, patch } => format!("patch {patch} of region {region}"),
        Sel::Solid { file, solid } => format!("region {solid} of {file}"),
    };
    let message = match &picked {
        Some(x) => format!("{} ({} under the cursor); {} selected", what(x), n_under, s.selection.len()),
        None if n_under > 0 => format!("only {} under the cursor; {} selected", n_under, s.selection.len()),
        None => format!("nothing under the cursor; {} selected", s.selection.len()),
    };
    let data = json!({
        "hits": listed,
        "picked": picked.as_ref().map(Sel::to_json),
        "selection": selection_json(&s.selection),
        "ray": { "origin": ray.origin, "direction": ray.direction },
    });
    Ok(Outcome::new(message, data))
}

pub(super) const SELECT: CommandSpec = CommandSpec {
    id: "select.set",
    label: "Select",
    menu: "",
    shortcut: None,
    params: "items: a list of patches ({\"region\": \"air\", \"patch\": \"inlet\"}; region \"\" in a single-region case) and surface regions ({\"surface\": \"walls.stl\", \"solid\": \"roofs\"}, the region as OpenFOAM names it). mode: replace (default), add or remove. Each must exist.",
    enabled: needs_case,
    run: select,
    journal: true,
    undoable: false,
};

fn select(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SELECT.id;
    let p = Params::new(id, p, &["items", "mode"])?;
    let mode = mode(id, &p, &[("replace", Mode::Replace), ("add", Mode::Add), ("remove", Mode::Remove)])?;
    let list = p.required("items")?.as_array().ok_or_else(|| Error::Params(format!("{id}: `items` is a list")))?;
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let mut items = Vec::new();
    for v in list {
        let o = v.as_object().ok_or_else(|| Error::Params(format!("{id}: each item is an object")))?;
        let get = |k: &str| o.get(k).and_then(Value::as_str);
        let keys: Vec<&str> = o.keys().map(String::as_str).collect();
        let sel = match (get("region"), get("patch"), get("surface"), get("solid")) {
            (Some(region), Some(patch), None, None) if keys.len() == 2 => {
                let r = case
                    .regions
                    .iter()
                    .find(|r| r.name == region)
                    .ok_or_else(|| Error::Params(format!("{id}: no region `{region}`")))?;
                let patches =
                    r.patches.as_ref().ok_or_else(|| Error::Params(format!("{id}: region `{region}` has no mesh")))?;
                if !patches.iter().any(|x| x.name == patch) {
                    return Err(Error::Params(format!("{id}: region `{region}` has no patch `{patch}`")));
                }
                Sel::Patch { region: region.to_string(), patch: patch.to_string() }
            }
            (None, None, Some(file), Some(solid)) if keys.len() == 2 => {
                let rel = surface_path(id, file, false)?;
                let bytes =
                    case.file(fs, &rel)?.ok_or_else(|| Error::Params(format!("{id}: there is no surface {rel}")))?;
                let stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{id}: {rel}: {e}")))?;
                if !stl.regions().0.iter().any(|r| r == solid) {
                    return Err(Error::Params(format!("{id}: {rel} has no region `{solid}`")));
                }
                Sel::Solid { file: rel, solid: solid.to_string() }
            }
            _ => {
                return Err(Error::Params(format!(
                    "{id}: an item is {{\"region\", \"patch\"}} or {{\"surface\", \"solid\"}}, not {v}"
                )));
            }
        };
        items.push(sel);
    }
    apply(&mut s.selection, mode, items);
    let message = format!("{} selected", count(s.selection.len(), "item"));
    Ok(Outcome::new(message, json!({ "selection": selection_json(&s.selection) })))
}

pub(super) const CLEAR: CommandSpec = CommandSpec {
    id: "select.clear",
    label: "Select none",
    menu: "Edit/Select none",
    shortcut: None,
    params: "none",
    enabled: needs_case,
    run: clear,
    journal: true,
    undoable: false,
};

fn clear(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(CLEAR.id, p, &[])?;
    s.selection.clear();
    Ok(Outcome::new("nothing selected", json!({ "selection": [] })))
}
