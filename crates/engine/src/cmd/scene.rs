//! The 3D scene and the selection: `scene.snapshot` draws the case's meshes or surfaces into a
//! PNG image with the CPU rasteriser; `select.pick` selects the patch or surface region under a
//! pixel (or along a ray), `select.set` and `select.clear` set the selection directly.
//!
//! The selection is plain data in the session ([`Sel`]): what a command run without an explicit
//! target acts on. The scene is built from the files the first time it is needed, each region's
//! mesh (only its boundary) and each surface read in parallel. Each object is kept with the
//! versions of the files it came from, also while it isn't shown: showing other regions or
//! surfaces reads only those not read yet, and a surface or patch list a command changed is read
//! again. A mesh rewritten on disk by OpenFOAM is read again with `"reload": true`, which drops
//! what was kept of hidden ones too. Points are single precision, relative to the scene's origin
//! (the middle of what is shown); an object is never moved to another origin: when the middle
//! moves more than 10 km, what is shown is read again around it and the kept ones are dropped.

use std::sync::Arc;

use serde_json::{Value, json};
use umcf_foam_mesh::{BoundaryMesh, Limits, mesh_dir};
use umcf_geom::BBox;
use umcf_geom::bvh::Ray;
use umcf_geom::stl;
use umcf_render::camera::{Camera, View};
use umcf_render::raster::{self, ColourBy, MAX_PIXELS, MAX_SIDE, Style, draw};
use umcf_render::{Object, Pick, Scene, Source, png};

use super::case::count;
use super::display;
use super::geometry::{SURFACE_DIR, limits, surface_files, surface_path};
use crate::params::Params;
use crate::{CommandSpec, Error, Outcome, Session, needs_case};

/// One selected thing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sel {
    /// A patch of a region's mesh (`region` is `""` in a single-region case).
    Patch { region: String, patch: String },
    /// A region of an STL surface (`file` case-relative), as OpenFOAM names it.
    Solid { file: String, solid: String },
    /// A boundary face of a region's mesh, by its label.
    Face { region: String, face: u32 },
}

impl Sel {
    /// The selected thing as the commands take and return it.
    pub fn to_json(&self) -> Value {
        match self {
            Sel::Patch { region, patch } => json!({ "region": region, "patch": patch }),
            Sel::Solid { file, solid } => json!({ "surface": file, "solid": solid }),
            Sel::Face { region, face } => json!({ "region": region, "face": face }),
        }
    }

    /// The selection of face `index` of `object`: a mesh face (an STL triangle is not one).
    fn face_of(object: &Object, index: u32) -> Option<Sel> {
        match &object.source {
            Source::Mesh { region } => Some(Sel::Face { region: region.clone(), face: index }),
            Source::Surface { .. } => None,
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

/// The scene last built and what it was built from. Shared with the copy of the session the
/// desktop app draws from while a job runs ([`Session::stand_in`]).
#[derive(Clone)]
pub(crate) struct Cached {
    opens: u64,
    regions: Vec<String>,
    /// Each surface, with the bytes a command put there (compared by identity: the case keeps
    /// the same `Arc` until the file is put again), or `None` when it was read from disk.
    surfaces: Vec<(String, Option<Arc<Vec<u8>>>)>,
    scene: Scene,
    /// What each of the scene's objects was built from, in their order.
    keys: Vec<Key>,
    /// Objects built for this case and not shown now: shown again without reading them again.
    spare: Vec<(Key, Object)>,
}

/// What an object was built from: a region's mesh or a surface, with the versions of the files
/// commands may have put (compared by identity; `None`: as on disk).
#[derive(Debug, Clone)]
pub(crate) struct Key {
    source: Source,
    files: Vec<(String, Option<Arc<Vec<u8>>>)>,
}

impl Key {
    /// Whether `other` is the same source with the same file versions.
    fn same(&self, other: &Key) -> bool {
        self.source == other.source && same_files(&self.files, &other.files)
    }
}

/// Whether two lists name the same files with the same versions (put bytes by identity).
fn same_files(a: &[(String, Option<Arc<Vec<u8>>>)], b: &[(String, Option<Arc<Vec<u8>>>)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((f, x), (g, y))| {
            f == g
                && match (x, y) {
                    (Some(x), Some(y)) => Arc::ptr_eq(x, y),
                    (None, None) => true,
                    _ => false,
                }
        })
}

/// Whether the case still has `version` put at `file`.
fn case_put(s: &Session, file: &str, version: &Option<Arc<Vec<u8>>>) -> bool {
    match (s.case().and_then(|c| c.put_bytes(file)), version) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A job of [`parallel`].
type Job<'a, T> = Box<dyn FnOnce() -> Result<T, Error> + Send + 'a>;

/// Runs `jobs` on up to one worker thread per core and on the calling thread, which takes
/// jobs too (so they all run, one after the other, where no thread starts: wasm, or a thread
/// limit); their results in order, or the first error. A job that panics is a `Panicked` error.
///
/// Each job comes with an estimate of the memory it holds while it runs. A job starts only while
/// the estimates of the jobs running and its own fit in `budget`; the biggest start first, and a
/// job runs alone when nothing else runs, whatever its estimate.
fn parallel<T: Send>(id: &str, jobs: Vec<(u64, Job<'_, T>)>, budget: u64) -> Result<Vec<T>, Error> {
    use std::sync::{Condvar, Mutex};
    struct State<'a, T> {
        /// The jobs not started, the biggest first.
        queue: Vec<(usize, u64, Job<'a, T>)>,
        /// The estimates of the jobs running, together.
        held: u64,
        running: usize,
    }
    let n = jobs.len();
    let mut queue: Vec<(usize, u64, Job<'_, T>)> = jobs.into_iter().enumerate().map(|(k, (c, j))| (k, c, j)).collect();
    queue.sort_by_key(|(k, c, _)| (std::cmp::Reverse(*c), *k));
    let state = Mutex::new(State { queue, held: 0, running: 0 });
    let changed = Condvar::new();
    let done: Mutex<Vec<Option<Result<T, Error>>>> = Mutex::new((0..n).map(|_| None).collect());
    let work = || {
        loop {
            // The next job that fits, waiting for a running one to end while none does.
            let next = {
                let Ok(mut st) = state.lock() else { return };
                loop {
                    if st.queue.is_empty() {
                        break None;
                    }
                    let fits = if st.running == 0 {
                        Some(0)
                    } else {
                        let held = st.held;
                        st.queue.iter().position(|(_, c, _)| held.saturating_add(*c) <= budget)
                    };
                    if let Some(i) = fits {
                        let job = st.queue.remove(i);
                        st.held = st.held.saturating_add(job.1);
                        st.running += 1;
                        break Some(job);
                    }
                    st = match changed.wait(st) {
                        Ok(st) => st,
                        Err(_) => return,
                    };
                }
            };
            let Some((k, cost, job)) = next else { return };
            // A panic is the engine's `Panicked`, as the guard around every command makes it.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|payload| {
                Err(Error::Panicked { id: id.to_string(), message: crate::panic_message(payload.as_ref()) })
            });
            if let Ok(mut d) = done.lock()
                && let Some(slot) = d.get_mut(k)
            {
                *slot = Some(r);
            }
            if let Ok(mut st) = state.lock() {
                st.held = st.held.saturating_sub(cost);
                st.running = st.running.saturating_sub(1);
            }
            changed.notify_all();
        }
    };
    let workers = std::thread::available_parallelism().map_or(1, |c| c.get()).min(n).saturating_sub(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            if std::thread::Builder::new().spawn_scoped(scope, work).is_err() {
                break;
            }
        }
        work();
    });
    let done = done.into_inner().map_err(|_| Error::Failed(format!("{id}: a reader thread failed")))?;
    done.into_iter()
        .map(|r| r.unwrap_or_else(|| Err(Error::Failed(format!("{id}: a reader job did not run")))))
        .collect()
}

/// About how much memory reading region `region`'s boundary holds while it runs: every point
/// of the mesh (read whole, then only the boundary's kept) and the boundary faces, from the
/// counts the `owner` header notes; without them, from the size of the points file.
fn mesh_need(fs: &dyn umcf_core::CaseFs, region: &str) -> u64 {
    let dir = mesh_dir(region);
    let counts = umcf_foam_mesh::counts(fs, &dir, &Limits::default()).unwrap_or_default();
    if let Some(points) = counts.points {
        let boundary = counts.faces.zip(counts.internal_faces).map_or(0, |(f, i)| f.saturating_sub(i));
        return points.saturating_mul(32).saturating_add(boundary.saturating_mul(32));
    }
    let plain = fs.size(&format!("{dir}/points")).map(|n| n.saturating_mul(2));
    let gz = || fs.size(&format!("{dir}/points.gz")).map(|n| n.saturating_mul(3));
    plain.or_else(gz).unwrap_or(0)
}

/// About how much memory reading surface `file` holds while it runs: its bytes, what is read of
/// them and the object built.
fn surface_need(case: &umcf_model::Case, fs: &dyn umcf_core::CaseFs, file: &str) -> u64 {
    let size = case.put_bytes(file).map(|b| b.len() as u64).or_else(|| fs.size(file)).unwrap_or(0);
    size.saturating_mul(3)
}

/// Drops the first of `spare` (the ones shown least recently) while they hold more than `budget`.
fn trim_spare(spare: &mut Vec<(Key, Object)>, budget: u64) {
    let mut held: u64 = spare.iter().map(|(_, o)| o.bytes()).sum();
    let mut drop = 0;
    while held > budget && drop < spare.len() {
        held = held.saturating_sub(spare.get(drop).map_or(0, |(_, o)| o.bytes()));
        drop += 1;
    }
    spare.drain(..drop);
}

/// The parameters that say what the scene shows.
const WHICH: [&str; 3] = ["regions", "surfaces", "reload"];
/// The parameters that place the camera.
const CAMERA: [&str; 7] = ["view", "azimuth", "elevation", "zoom", "camera", "width", "height"];
const DEFAULT_SIZE: (u32, u32) = (1280, 800);
/// The most items one `select.set` takes.
const MAX_ITEMS: usize = 50_000_000;
/// Most hits a pick lists.
const MAX_HITS: usize = 64;

/// `v` as a list of names, or a parameter error naming `key`.
fn strings<'a>(id: &str, key: &str, v: &'a Value) -> Result<Vec<&'a str>, Error> {
    let bad = || Error::Params(format!("{id}: `{key}` is a list of names"));
    v.as_array().ok_or_else(bad)?.iter().map(|x| x.as_str().ok_or_else(bad)).collect()
}

/// The parameter `key`, when it was given.
fn optional<'a>(p: &Params<'a>, key: &str) -> Result<Option<&'a Value>, Error> {
    match p.required(key) {
        Ok(v) => Ok(Some(v)),
        Err(_) => Ok(None),
    }
}

/// The scene last built.
fn cached<'s>(s: &'s Session, id: &str) -> Result<&'s Scene, Error> {
    s.scene.as_ref().map(|c| &c.scene).ok_or_else(|| Error::Failed(format!("{id}: no scene")))
}

/// Builds the scene the parameters ask for into the cache, unless it is there already.
fn build(s: &mut Session, id: &str, p: &Params) -> Result<(), Error> {
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
        None if regions.is_empty() && optional(p, "regions")?.is_none() => {
            let (names, listed) = surface_files(case, fs, false);
            if let (true, Err(e)) = (names.is_empty(), &listed) {
                return Err(Error::Failed(format!("{id}: {SURFACE_DIR} can't be listed: {e}")));
            }
            names
        }
        None => Vec::new(),
    };
    if regions.is_empty() && surfaces.is_empty() {
        return Err(Error::Failed(format!(
            "{id}: nothing to show: the case has no mesh and no STL surface in {SURFACE_DIR}"
        )));
    }
    // The files the scene is drawn from that commands may have put: the surfaces, and the
    // meshes' patch lists (a patch renamed or retyped).
    let boundaries: Vec<String> = regions
        .iter()
        .flat_map(|r| {
            let b = format!("{}/boundary", mesh_dir(r));
            [format!("{b}.gz"), b]
        })
        .collect();
    let versions: Vec<(String, Option<Arc<Vec<u8>>>)> =
        surfaces.iter().chain(&boundaries).map(|f| (f.clone(), case.put_bytes(f).cloned())).collect();
    let reload = p.bool("reload")?.unwrap_or(false);
    let same = |c: &Cached| c.opens == s.opens && c.regions == regions && same_files(&c.surfaces, &versions);
    if reload || !s.scene.as_deref().is_some_and(same) {
        // What each object is built from. Objects of this case built before (shown or spare)
        // are taken as they are, unless `reload`; the others are read, in parallel.
        let mesh_key = |r: &str| {
            let b = format!("{}/boundary", mesh_dir(r));
            Key {
                source: Source::Mesh { region: r.to_string() },
                files: [format!("{b}.gz"), b].into_iter().map(|f| (f.clone(), case.put_bytes(&f).cloned())).collect(),
            }
        };
        let surface_key = |f: &str| Key {
            source: Source::Surface { file: f.to_string() },
            files: vec![(f.to_string(), case.put_bytes(f).cloned())],
        };
        let wanted: Vec<Key> =
            regions.iter().map(|r| mesh_key(r)).chain(surfaces.iter().map(|f| surface_key(f))).collect();
        let previous = s.scene.as_ref().filter(|c| c.opens == s.opens && !reload);
        let have =
            |k: &Key| previous.is_some_and(|c| c.keys.iter().chain(c.spare.iter().map(|(k, _)| k)).any(|x| x.same(k)));
        let missing: Vec<&Key> = wanted.iter().filter(|k| !have(k)).collect();

        // Read what is missing: meshes (only their boundary: drawing and picking need no
        // cells) and surfaces.
        enum Read {
            Mesh(BoundaryMesh),
            Surface(stl::Stl),
        }
        let budget = s.budget;
        let jobs_for = |keys: &[&Key]| -> Vec<(u64, Job<'_, Read>)> {
            let mut jobs: Vec<(u64, Job<'_, Read>)> = Vec::new();
            for k in keys {
                match &k.source {
                    Source::Mesh { region } => {
                        let r = region.clone();
                        jobs.push((
                            mesh_need(fs, &r),
                            Box::new(move || {
                                let mut mesh =
                                    BoundaryMesh::read(fs, &mesh_dir(&r), &Limits::default()).map_err(|e| {
                                        let which =
                                            if r.is_empty() { String::new() } else { format!(" of region {r}") };
                                        Error::Failed(format!("{id}: the mesh{which} can't be read: {e}"))
                                    })?;
                                // The patches as the case has them (renamed or retyped and not saved
                                // yet), when they are the same faces.
                                if let Some(patches) = case.region(&r).and_then(|x| x.patches.as_ref())
                                    && patches.len() == mesh.boundary.len()
                                    && patches.iter().zip(&mesh.boundary).all(|(a, b)| a.faces() == b.faces())
                                {
                                    mesh.boundary = patches.clone();
                                }
                                Ok(Read::Mesh(mesh))
                            }),
                        ));
                    }
                    Source::Surface { file } => {
                        let f = file.clone();
                        jobs.push((
                            surface_need(case, fs, &f),
                            Box::new(move || {
                                let bytes = case
                                    .file(fs, &f)?
                                    .ok_or_else(|| Error::Failed(format!("{id}: there is no surface {f}")))?;
                                let stl = stl::read(&bytes, &limits())
                                    .map_err(|e| Error::Failed(format!("{id}: {f}: {e}")))?;
                                Ok(Read::Surface(stl))
                            }),
                        ));
                    }
                }
            }
            jobs
        };
        let read = parallel(id, jobs_for(&missing), budget.read)?;

        // The origin: the middle of what is shown, so that points far from the world's origin
        // (projected coordinates) stay precise in single precision. The previous scene's is kept
        // while it is near that middle, so that the objects built before fit as they are.
        // Otherwise they are not moved (in single precision that would lose what moving back
        // can't restore): the shown ones are read again at the new origin, the hidden ones go.
        const NEAR: f64 = 1.0e4;
        let kept_box = |k: &Key| -> Option<BBox> {
            let c = previous?;
            let mut all = c.keys.iter().zip(&c.scene.objects).chain(c.spare.iter().map(|(k, o)| (k, o)));
            all.find(|(x, _)| x.same(k)).and_then(|(_, o)| o.bbox(c.scene.origin))
        };
        let boxes: Vec<BBox> = read
            .iter()
            .filter_map(|x| match x {
                Read::Mesh(m) => Object::boundary_bounds(m),
                Read::Surface(stl) => stl.bbox(),
            })
            .chain(wanted.iter().filter(|k| !missing.iter().any(|m| m.same(k))).filter_map(kept_box))
            .collect();
        let middle = Scene::origin_for(boxes);
        let origin = match previous {
            Some(c) if (c.scene.origin - middle).length() <= NEAR => c.scene.origin,
            _ => middle,
        };
        let moved = previous.is_some_and(|c| c.scene.origin != origin);
        let mut missing = missing;
        let mut read = read;
        if moved {
            let again: Vec<&Key> = wanted.iter().filter(|k| !missing.iter().any(|m| m.same(k))).collect();
            read.extend(parallel(id, jobs_for(&again), budget.read)?);
            missing.extend(again);
        }
        // Building an object reads nothing more: what it holds is the object's, kept anyway.
        let built: Vec<(u64, Job<'_, Object>)> = missing
            .iter()
            .zip(read)
            .map(|(k, x)| -> (u64, Job<'_, Object>) {
                let source = k.source.clone();
                (
                    0,
                    Box::new(move || {
                        Ok(match (source, x) {
                            (Source::Mesh { region }, Read::Mesh(m)) => Object::from_mesh(&region, &m, origin),
                            (Source::Surface { file }, Read::Surface(stl)) => Object::from_stl(&file, &stl, origin),
                            _ => return Err(Error::Failed(format!("{id}: a scene object was read as the wrong kind"))),
                        })
                    }),
                )
            })
            .collect();
        let mut fresh: Vec<(Key, Object)> =
            missing.into_iter().cloned().zip(parallel(id, built, budget.read)?).collect();

        // The new scene: each object taken from what was read, else from before; what was
        // there and isn't shown now is kept aside, the ones shown least recently first (those
        // hidden before, then those hidden now), within the budget for them.
        let mut pool: Vec<(Key, Object)> = match s.scene.take() {
            Some(c) if c.opens == s.opens && !reload && !moved => {
                // Copied only while a copy of the session still holds it.
                let c = std::sync::Arc::try_unwrap(c).unwrap_or_else(|shared| (*shared).clone());
                c.spare.into_iter().chain(c.keys.into_iter().zip(c.scene.objects)).collect()
            }
            _ => Vec::new(),
        };
        let mut scene = Scene::new(origin);
        let mut keys = Vec::new();
        for k in wanted {
            let found = fresh
                .iter()
                .position(|(x, _)| x.same(&k))
                .map(|i| fresh.swap_remove(i))
                .or_else(|| pool.iter().position(|(x, _)| x.same(&k)).map(|i| pool.remove(i)));
            let Some((k, o)) = found else {
                return Err(Error::Failed(format!("{id}: an object of the scene was neither read nor kept")));
            };
            scene.objects.push(o);
            keys.push(k);
        }
        // An object of files a command has put since (another version) can't be shown again.
        pool.retain(|(k, _)| k.files.iter().all(|(f, v)| case_put(s, f, v)));
        trim_spare(&mut pool, budget.spare);
        // A mesh read again from disk (rewritten by OpenFOAM) may number its faces otherwise:
        // the faces picked on it go.
        if reload {
            s.selection.retain(|x| !matches!(x, Sel::Face { region, .. } if regions.contains(region)));
        }
        s.scene =
            Some(std::sync::Arc::new(Cached { opens: s.opens, regions, surfaces: versions, scene, keys, spare: pool }));
    }
    Ok(())
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
    if u64::from(w) * u64::from(h) > u64::from(MAX_PIXELS) {
        return Err(Error::Params(format!("{id}: {w} × {h} is more than {MAX_PIXELS} pixels (4096 × 4096)")));
    }
    if let Some(c) = optional(p, "camera")? {
        if let Some(k) =
            ["view", "azimuth", "elevation", "zoom"].iter().find(|k| optional(p, k).ok().flatten().is_some())
        {
            return Err(Error::Params(format!("{id}: `camera` places the camera; `{k}` is not given with it")));
        }
        return Ok((given_camera(id, c)?, w, h));
    }
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

/// The `display` entries in `p` and the scene they apply to, built (or as cached), with what the
/// entries name checked against the case and the scene.
fn displayed<'s>(s: &'s mut Session, id: &str, p: &Params) -> Result<(Vec<display::DisplayEntry>, &'s Scene), Error> {
    let entries = match optional(p, "display")? {
        Some(v) => display::parse(id, v)?,
        None => Vec::new(),
    };
    build(s, id, p)?;
    let s: &'s Session = s;
    display::check(s, id, &entries, cached(s, id)?)?;
    Ok((entries, cached(s, id)?))
}

/// A camera as `camera_json` writes it (`fov` may be left out).
fn given_camera(id: &str, c: &Value) -> Result<Camera, Error> {
    const KEYS: [&str; 5] = ["target", "distance", "azimuth", "elevation", "fov"];
    let bad = || {
        Error::Params(format!(
            "{id}: `camera` is {{\"target\": [x, y, z], \"distance\", \"azimuth\", \"elevation\", \"fov\"}}, as scene.snapshot returns it"
        ))
    };
    let o = c.as_object().filter(|o| o.keys().all(|k| KEYS.contains(&k.as_str()))).ok_or_else(bad)?;
    let number = |k: &str| o.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    let t = vector(id, "camera.target", o.get("target"))?;
    let distance = number("distance").filter(|d| *d > 0.0).ok_or_else(bad)?;
    let elevation = number("elevation").filter(|e| (-90.0..=90.0).contains(e)).ok_or_else(bad)?;
    let fov = match o.get("fov") {
        None => umcf_render::camera::FOV,
        Some(_) => number("fov").filter(|f| *f > 0.1 && *f < 170.0).ok_or_else(bad)?,
    };
    Ok(Camera {
        target: umcf_geom::Vec3::new(t[0], t[1], t[2]),
        distance,
        azimuth: number("azimuth").ok_or_else(bad)?,
        elevation,
        fov,
    })
}

/// A camera as `scene.snapshot` returns it and `camera` parameters take it.
pub fn camera_json(c: &Camera) -> Value {
    json!({
        "target": [c.target.x, c.target.y, c.target.z],
        "distance": c.distance,
        "azimuth": c.azimuth,
        "elevation": c.elevation,
        "fov": c.fov,
    })
}

/// `bytes` in base64 (standard alphabet, padded).
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

/// What the scene holds, one entry per object: its source, patches or regions, triangles.
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
    params: "Draws the case in 3D with the CPU renderer and returns a PNG image (`png`, base64). What: regions (mesh regions, their boundary patches; \"\" in a single-region case) and surfaces (STL files in constant/triSurface); by default every region with a mesh, or every surface when there is none. Each mesh (only its boundary) and surface is read once and kept, also while hidden; when what is shown is more than 10 km from the scene's middle so far, it is read again around its own middle and what was kept of hidden ones is dropped (single precision). reload: true reads the files again (after OpenFOAM rewrote a mesh), and drops what was kept of hidden ones; faces picked on the meshes read again are taken out of the selection, as their labels may have changed. Camera: view (iso, top, bottom, front, back, left, right; default iso), azimuth and elevation (degrees, override the view's), zoom (default 1: the whole scene), or camera ({\"target\": [x, y, z], \"distance\", \"azimuth\", \"elevation\", \"fov\"} as this returns it; fov may be left out) in place of those four, width and height (pixels, default 1280 × 800; each at most 8192, together at most 4096 × 4096). Style: edges (true: the faces' edges), colour_by (patch, default, or object), display (a list of {\"region\"[, \"patch\" or \"group\"]} or {\"surface\"[, \"solid\"]} entries with mode: surface, wireframe, points or hidden, and/or opacity from 0 to 1; later entries win). The selection is drawn in orange.",
    enabled: needs_case,
    run: snapshot,
    journal: false,
    undoable: false,
    reads_only: true,
};

/// `scene.snapshot`.
fn snapshot(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SNAPSHOT.id;
    let mut allowed = WHICH.to_vec();
    allowed.extend(CAMERA);
    allowed.extend(["edges", "colour_by", "display"]);
    let p = Params::new(id, p, &allowed)?;
    let edges = p.bool("edges")?.unwrap_or(false);
    let colour_by = match p.str("colour_by")? {
        None | Some("patch") => ColourBy::Class,
        Some("object") => ColourBy::Object,
        Some(o) => return Err(Error::Params(format!("{id}: `colour_by` is patch or object, not `{o}`"))),
    };
    let selection = s.selection.clone();
    let (entries, scene) = displayed(s, id, &p)?;
    let (cam, width, height) = camera(id, &p, scene)?;
    let selected = selected_classes(scene, &selection);
    let selected_faces = selected_faces(scene, &selection);
    let n_selected = selected.len() + selected_faces.len();
    let display = display::style(scene, &entries);
    let image = draw(
        scene,
        &cam,
        &Style { width, height, edges, selected, selected_faces, colour_by, display, ..Style::default() },
    );
    let file = png::encode(&image);
    let labels: Vec<String> = scene.objects.iter().map(Object::label).collect();
    let mut message = format!("{width} × {height} of {} ({})", labels.join(", "), count(scene.triangles(), "triangle"));
    let dropped: usize = scene.objects.iter().map(|o| o.dropped).sum();
    if dropped > 0 {
        message.push_str(&format!(
            ", {} left out (coordinates not finite, or too far from the rest to draw)",
            if dropped == 1 { "1 face or triangle".to_string() } else { format!("{dropped} faces or triangles") }
        ));
    }
    if n_selected > 0 {
        message.push_str(&format!(", {n_selected} selected"));
    }
    let data = json!({
        "width": width,
        "height": height,
        "png": base64(&file),
        "bytes": file.len(),
        "camera": camera_json(&cam),
        "left_out": dropped,
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

/// The `mode` parameter, one of `allowed`.
fn mode(id: &str, p: &Params, allowed: &[(&str, Mode)]) -> Result<Mode, Error> {
    match p.str("mode")? {
        None => Ok(Mode::Replace),
        Some(m) => allowed.iter().find(|(n, _)| *n == m).map(|(_, x)| *x).ok_or_else(|| {
            let names: Vec<&str> = allowed.iter().map(|(n, _)| *n).collect();
            Error::Params(format!("{id}: `mode` is {}, not `{m}`", names.join(", ")))
        }),
    }
}

/// Changes `selection` by `items` as `mode` says.
fn apply(selection: &mut Vec<Sel>, mode: Mode, items: Vec<Sel>) {
    use std::collections::HashSet;
    if mode == Mode::Replace {
        selection.clear();
    }
    let mut have: HashSet<Sel> = selection.iter().cloned().collect();
    match mode {
        Mode::Replace | Mode::Add => {
            for i in items {
                if have.insert(i.clone()) {
                    selection.push(i);
                }
            }
        }
        Mode::Remove => {
            let gone: HashSet<Sel> = items.into_iter().collect();
            selection.retain(|x| !gone.contains(x));
        }
        Mode::Toggle => {
            // Toggling twice in one list puts an item back as it was.
            let mut gone: HashSet<Sel> = HashSet::new();
            for i in items {
                if have.remove(&i) {
                    gone.insert(i);
                } else if gone.remove(&i) {
                    have.insert(i);
                } else {
                    have.insert(i.clone());
                    selection.push(i);
                }
            }
            selection.retain(|x| !gone.contains(x));
        }
        Mode::None => {}
    }
}

/// The selected patches and surface regions as the renderer takes them: (object index, class).
fn selected_classes(scene: &Scene, selection: &[Sel]) -> Vec<(usize, u32)> {
    let chosen: std::collections::HashSet<&Sel> = selection.iter().filter(|s| !matches!(s, Sel::Face { .. })).collect();
    let mut out = Vec::new();
    for (k, o) in scene.objects.iter().enumerate() {
        for c in 0..o.classes.len() {
            let Ok(c) = u32::try_from(c) else { break };
            if !chosen.is_empty() && Sel::of(o, c).is_some_and(|x| chosen.contains(&x)) {
                out.push((k, c));
            }
        }
    }
    out
}

/// The selected faces as the renderer takes them: (object index, face label), for mesh objects.
pub fn selected_faces(scene: &Scene, selection: &[Sel]) -> std::collections::HashSet<(usize, u32)> {
    let mut out = std::collections::HashSet::new();
    for sel in selection {
        let Sel::Face { region, face } = sel else { continue };
        for (k, o) in scene.objects.iter().enumerate() {
            if matches!(&o.source, Source::Mesh { region: r } if r == region) {
                out.insert((k, *face));
            }
        }
    }
    out
}

/// The selection as the commands return it.
fn selection_json(s: &[Sel]) -> Value {
    s.iter().map(Sel::to_json).collect()
}

pub(super) fn vector(id: &str, key: &str, v: Option<&Value>) -> Result<[f64; 3], Error> {
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
    params: "Selects the patch or surface region under a pixel of the view scene.snapshot draws with the same parameters (pixel: [x, y], from the top left), or along a ray in world coordinates (ray: {\"origin\": [x, y, z], \"direction\": [x, y, z]}). Lists everything under it, nearest first, one entry per patch or region at its nearest hit, up to 64 (hits). faces: true selects the mesh face under it instead ({\"region\", \"face\"}), and lists one entry per face. depth: which of them to take (default 0, the nearest; 1 the one behind it: \"select other\"); past the last, nothing changes. mode: replace (default; nothing under it clears the selection), add, toggle, or none (only list). What and camera: as scene.snapshot (regions, surfaces, reload, view, azimuth, elevation, zoom, or camera as scene.snapshot returns it; width, height); display as scene.snapshot's: hidden layers are passed over.",
    enabled: needs_case,
    run: pick,
    journal: true,
    undoable: false,
    reads_only: false,
};

/// `select.pick`.
fn pick(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = PICK.id;
    let mut allowed = WHICH.to_vec();
    allowed.extend(CAMERA);
    allowed.extend(["pixel", "ray", "depth", "mode", "faces", "display"]);
    let p = Params::new(id, p, &allowed)?;
    let faces = p.bool("faces")?.unwrap_or(false);
    let mode = mode(
        id,
        &p,
        &[("replace", Mode::Replace), ("add", Mode::Add), ("toggle", Mode::Toggle), ("none", Mode::None)],
    )?;
    let depth = match p.number("depth")? {
        None => 0,
        Some(d) if d.fract() == 0.0 && (0.0..MAX_HITS as f64).contains(&d) => d as usize,
        Some(_) => return Err(Error::Params(format!("{id}: `depth` is a whole number from 0 to {}", MAX_HITS - 1))),
    };
    let (pixel, ray) = (optional(&p, "pixel")?, optional(&p, "ray")?);
    let (entries, scene) = displayed(s, id, &p)?;
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
    // One entry per patch or region at its nearest hit; with `faces`, per mesh face.
    // Surfaces have no faces to pick: their triangles are passed over before the limit.
    // Hidden layers are passed over; one entry per patch or region at its nearest hit, or with
    // `faces` per mesh face.
    let display = display::style(scene, &entries);
    let shown = |h: &Pick| display.get(&(h.object, h.class)).is_none_or(|d| d.mode != raster::Mode::Hidden);
    let mut classes = std::collections::HashSet::new();
    let hits: Vec<Pick> = scene
        .pick(&ray, usize::MAX)
        .into_iter()
        .filter(shown)
        .filter(|h| {
            if faces {
                scene.objects.get(h.object).is_some_and(|o| matches!(o.source, Source::Mesh { .. }))
            } else {
                classes.insert((h.object, h.class))
            }
        })
        .collect();
    let mut under: Vec<(Sel, &Pick)> = Vec::new();
    for h in &hits {
        let sel = scene
            .objects
            .get(h.object)
            .and_then(|o| if faces { Sel::face_of(o, h.index) } else { Sel::of(o, h.class) });
        let Some(sel) = sel else { continue };
        if !under.iter().any(|(x, _)| *x == sel) && under.len() < MAX_HITS {
            under.push((sel, h));
        }
    }
    let listed: Vec<Value> = under
        .iter()
        .map(|(sel, h)| {
            let mut j = sel.to_json();
            let index = if matches!(sel, Sel::Solid { .. }) { "triangle" } else { "face" };
            if let Some(o) = j.as_object_mut() {
                o.insert(index.to_string(), json!(h.index));
                if faces && let Some(name) = scene.objects.get(h.object).and_then(|o| o.classes.get(h.class as usize)) {
                    o.insert("patch".to_string(), json!(name));
                }
                o.insert("distance".to_string(), json!(h.distance));
                o.insert("point".to_string(), json!([h.point.x, h.point.y, h.point.z]));
            }
            j
        })
        .collect();
    let picked = under.get(depth).map(|(sel, _)| sel.clone());
    let n_under = under.len();
    let items: Vec<Sel> = picked.iter().cloned().collect();
    // Nothing under the cursor clears the selection (replace); a depth past what is there
    // changes nothing.
    if !items.is_empty() || (mode == Mode::Replace && n_under == 0) {
        apply(&mut s.selection, mode, items);
    }
    let what = |x: &Sel| match x {
        Sel::Patch { region, patch } if region.is_empty() => format!("patch {patch}"),
        Sel::Patch { region, patch } => format!("patch {patch} of region {region}"),
        Sel::Solid { file, solid } => format!("region {solid} of {file}"),
        Sel::Face { region, face } if region.is_empty() => format!("face {face}"),
        Sel::Face { region, face } => format!("face {face} of region {region}"),
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

pub(super) const RECT: CommandSpec = CommandSpec {
    id: "select.rect",
    label: "Select in a rectangle",
    menu: "",
    shortcut: None,
    params: "Selects the patches and surface regions seen in a rectangle of the view scene.snapshot draws with the same parameters (rect: [x0, y0, x1, y1], two opposite corners in pixels from the top left; a pixel is in it when its middle is). faces: true selects the mesh faces instead ({\"region\", \"face\"}; surfaces are passed over and hide nothing). through: true takes everything in it, seen or behind something: the faces whose middle (area-weighted centre) is inside it and in front of the camera, and the patches and regions of those; otherwise only what is seen at one of its pixels (a face smaller than a pixel may be missed: zoom in, or go through). mode: replace (default), add, remove, or none (only count). What and camera: as scene.snapshot (regions, surfaces, reload, view, azimuth, elevation, zoom, or camera as scene.snapshot returns it; width, height); display as scene.snapshot's: hidden layers are passed over. Returns how many were in it and up to 1000 of them (items).",
    enabled: needs_case,
    run: rect,
    journal: true,
    undoable: false,
    reads_only: false,
};

/// Most items `select.rect` lists.
const MAX_LISTED: usize = 1000;

/// `select.rect`.
fn rect(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = RECT.id;
    let mut allowed = WHICH.to_vec();
    allowed.extend(CAMERA);
    allowed.extend(["rect", "through", "mode", "faces", "display"]);
    let p = Params::new(id, p, &allowed)?;
    let faces = p.bool("faces")?.unwrap_or(false);
    let through = p.bool("through")?.unwrap_or(false);
    let mode = mode(
        id,
        &p,
        &[("replace", Mode::Replace), ("add", Mode::Add), ("remove", Mode::Remove), ("none", Mode::None)],
    )?;
    let bad = || Error::Params(format!("{id}: `rect` is [x0, y0, x1, y1], four finite numbers"));
    let r = p.required("rect")?.as_array().filter(|a| a.len() == 4).ok_or_else(bad)?;
    let c = |i: usize| r.get(i).and_then(Value::as_f64).filter(|x| x.is_finite()).ok_or_else(bad);
    let area = umcf_render::raster::Rect { a: [c(0)?, c(1)?], b: [c(2)?, c(3)?] };
    let selection = s.selection.clone();
    let (entries, scene) = displayed(s, id, &p)?;
    let (cam, width, height) = camera(id, &p, scene)?;
    let style = Style {
        width,
        height,
        selected: selected_classes(scene, &selection),
        selected_faces: selected_faces(scene, &selection),
        display: display::style(scene, &entries),
        ..Style::default()
    };
    // (object, class or face) once each, then what they select.
    let mut keys: Vec<(usize, u32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Picking faces, surfaces are passed over: they don't hide the mesh behind them.
    let keep =
        |k: usize, _: u32| !faces || scene.objects.get(k).is_some_and(|o| matches!(o.source, Source::Mesh { .. }));
    for (k, t) in umcf_render::raster::covered(scene, &cam, &style, area, through, &keep) {
        let Some(o) = scene.objects.get(k) else { continue };
        let key = if faces { o.origin_index.get(t as usize) } else { o.class.get(t as usize) };
        if let Some(key) = key
            && seen.insert((k, *key))
        {
            keys.push((k, *key));
        }
    }
    if keys.len() > MAX_ITEMS {
        return Err(Error::Params(format!("{id}: more than {MAX_ITEMS} items in the rectangle")));
    }
    let items: Vec<Sel> = keys
        .into_iter()
        .filter_map(|(k, x)| {
            let o = scene.objects.get(k)?;
            if faces { Sel::face_of(o, x) } else { Sel::of(o, x) }
        })
        .collect();
    let n = items.len();
    let listed: Vec<Value> = items.iter().take(MAX_LISTED).map(Sel::to_json).collect();
    let before = s.selection.len();
    apply(&mut s.selection, mode, items);
    let what = if faces { "face" } else { "item" };
    let message = match mode {
        Mode::Remove => format!(
            "{} in the rectangle, {} taken off; {} selected",
            count(n, what),
            before.saturating_sub(s.selection.len()),
            s.selection.len()
        ),
        _ => format!("{} in the rectangle; {} selected", count(n, what), s.selection.len()),
    };
    Ok(Outcome::new(message, json!({ "in": n, "items": listed, "selected": s.selection.len() })))
}

pub(super) const SELECT: CommandSpec = CommandSpec {
    id: "select.set",
    label: "Select",
    menu: "",
    shortcut: None,
    params: "items: a list of patches ({\"region\": \"air\", \"patch\": \"inlet\"}; region \"\" in a single-region case) and surface regions ({\"surface\": \"walls.stl\", \"solid\": \"roofs\"}, the region as OpenFOAM names it) and boundary faces of a mesh ({\"region\": \"air\", \"face\": 75548}, by label). mode: replace (default), add or remove. Each must exist.",
    enabled: needs_case,
    run: select,
    journal: true,
    undoable: false,
    reads_only: false,
};

/// `select.set`.
fn select(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    let id = SELECT.id;
    let p = Params::new(id, p, &["items", "mode"])?;
    let mode = mode(id, &p, &[("replace", Mode::Replace), ("add", Mode::Add), ("remove", Mode::Remove)])?;
    let list = p.required("items")?.as_array().ok_or_else(|| Error::Params(format!("{id}: `items` is a list")))?;
    if list.len() > MAX_ITEMS {
        return Err(Error::Params(format!("{id}: more than {MAX_ITEMS} items")));
    }
    let (case, fs) = s.case().zip(s.fs()).ok_or_else(|| Error::Failed("no case is open".to_string()))?;
    let mut items = Vec::new();
    // Each surface's regions, read once.
    let mut regions: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
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
            (Some(region), None, None, None) if keys.len() == 2 && o.contains_key("face") => {
                let face =
                    o.get("face").and_then(Value::as_u64).and_then(|f| u32::try_from(f).ok()).ok_or_else(|| {
                        Error::Params(format!(
                            "{id}: `face` is a face label, not {}",
                            o.get("face").unwrap_or(&Value::Null)
                        ))
                    })?;
                let r = case
                    .regions
                    .iter()
                    .find(|r| r.name == region)
                    .ok_or_else(|| Error::Params(format!("{id}: no region `{region}`")))?;
                let patches =
                    r.patches.as_ref().ok_or_else(|| Error::Params(format!("{id}: region `{region}` has no mesh")))?;
                if !patches.iter().any(|x| x.faces().contains(&(face as usize))) {
                    return Err(Error::Params(format!(
                        "{id}: face {face} is not a boundary face of region `{region}`"
                    )));
                }
                Sel::Face { region: region.to_string(), face }
            }
            (None, None, Some(file), Some(solid)) if keys.len() == 2 => {
                let rel = surface_path(id, file, false)?;
                if !regions.contains_key(&rel) {
                    let bytes = case
                        .file(fs, &rel)?
                        .ok_or_else(|| Error::Params(format!("{id}: there is no surface {rel}")))?;
                    let stl = stl::read(&bytes, &limits()).map_err(|e| Error::Failed(format!("{id}: {rel}: {e}")))?;
                    regions.insert(rel.clone(), stl.regions().0);
                }
                if !regions.get(&rel).is_some_and(|names| names.iter().any(|r| r == solid)) {
                    return Err(Error::Params(format!("{id}: {rel} has no region `{solid}`")));
                }
                Sel::Solid { file: rel, solid: solid.to_string() }
            }
            _ => {
                return Err(Error::Params(format!(
                    "{id}: an item is {{\"region\", \"patch\"}}, {{\"region\", \"face\"}} or {{\"surface\", \"solid\"}}, not {v}"
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
    reads_only: false,
};

/// `select.clear`.
fn clear(s: &mut Session, p: &Value) -> Result<Outcome, Error> {
    Params::new(CLEAR.id, p, &[])?;
    s.selection.clear();
    Ok(Outcome::new("nothing selected", json!({ "selection": [] })))
}

impl Session {
    /// The scene `regions`, `surfaces` and `reload` in `params` ask for (as `scene.snapshot`
    /// takes them), built or as cached: for a viewport that draws it itself. Reading it changes
    /// nothing in the case.
    ///
    /// A panic while building it (a bug) comes back as [`Error::Panicked`], as a command's
    /// would.
    pub fn scene(&mut self, params: &Value) -> Result<&Scene, Error> {
        let p = Params::new("scene", params, &WHICH)?;
        // Built into the cache under the guard, then borrowed from it.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(self, "scene", &p))) {
            // Cancelled as it ended: it may have read less than it shows.
            Ok(Ok(())) if self.cancel.is_cancelled() => {
                self.scene = None;
                Err(Error::Cancelled { id: "scene".to_string() })
            }
            Ok(Ok(())) => cached(self, "scene"),
            Ok(Err(e)) => Err(self.or_cancelled("scene", e)),
            Err(payload) => {
                self.scene = None;
                Err(Error::Panicked { id: "scene".to_string(), message: crate::panic_message(payload.as_ref()) })
            }
        }
    }

    /// The STL surfaces of the open case (case-relative paths, sorted), including those a
    /// command put there and the case has not saved yet. A directory that can't be listed is an
    /// error, unsaved surfaces or not (a missing one is no surfaces), and so is a panic while
    /// listing (a bug, [`Error::Panicked`]).
    pub fn surfaces(&self) -> Result<Vec<String>, Error> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match self.case().zip(self.fs()) {
            Some((case, fs)) => {
                let (names, listed) = surface_files(case, fs, false);
                listed.map(|()| names).map_err(|e| Error::Failed(format!("the surfaces can't be listed: {e}")))
            }
            None => Ok(Vec::new()),
        }))
        .map_err(|payload| Error::Panicked {
            id: "surfaces".to_string(),
            message: crate::panic_message(payload.as_ref()),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Jobs that note the estimates running together; the most seen.
    fn run(costs: &[u64], budget: u64) -> (Vec<usize>, u64) {
        let held = AtomicU64::new(0);
        let most = AtomicU64::new(0);
        let jobs: Vec<(u64, Job<'_, usize>)> = costs
            .iter()
            .enumerate()
            .map(|(k, &c)| -> (u64, Job<'_, usize>) {
                let (held, most) = (&held, &most);
                (
                    c,
                    Box::new(move || {
                        let now = held.fetch_add(c, Ordering::SeqCst) + c;
                        most.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        held.fetch_sub(c, Ordering::SeqCst);
                        Ok(k)
                    }),
                )
            })
            .collect();
        let out = parallel("test", jobs, budget).unwrap();
        (out, most.load(Ordering::SeqCst))
    }

    #[test]
    fn jobs_start_only_while_they_fit_the_budget() {
        let (out, most) = run(&[60, 50, 40, 10, 30], 100);
        assert_eq!(out, [0, 1, 2, 3, 4], "results in the jobs' order");
        assert!(most <= 100, "{most}");
        // A job bigger than the budget runs, alone.
        let (out, most) = run(&[500, 20, 20], 100);
        assert_eq!(out, [0, 1, 2]);
        assert_eq!(most, 500);
        // A big budget runs them together (where threads start).
        if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
            assert!(run(&[10, 10, 10], u64::MAX).1 > 10);
        }
    }

    #[test]
    fn a_failing_or_panicking_job_fails_the_whole() {
        let jobs: Vec<(u64, Job<'_, u32>)> = vec![
            (5, Box::new(|| Ok(1))),
            (5, Box::new(|| Err(Error::Failed("no".into())))),
            (5, Box::new(|| panic!("boom"))),
        ];
        assert!(parallel("test", jobs, 5).is_err());
    }
}
