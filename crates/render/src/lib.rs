//! The 3D scene of a case: its meshes' boundary patches and its STL surfaces, as triangles that
//! each know where they came from (a mesh face of a patch, an STL triangle of a region), so what
//! is drawn or picked is a patch or a region, not a pixel.
//!
//! - [`Scene`] holds the [`Object`]s, built from `foam-mesh` and `geom` data. It doesn't depend
//!   on any renderer: the GUI's viewport and the CPU rasteriser here draw the same scene.
//! - [`camera`] places the eye (standard views, framing a box) and gives the ray through a pixel.
//! - [`guides`] makes the grid on the ground and the origin's planes and axes both renderers
//!   draw beside the scene.
//! - [`raster`] draws a scene into an image in software, for headless snapshots and machines
//!   without a GPU; [`png`] encodes it.
//! - [`Scene::pick`] casts a ray through the scene with a triangle BVH per object (built the
//!   first time it is needed) and returns everything the ray passes through, nearest first.
//!
//! Points are kept in single precision relative to the scene's origin (the middle of what it
//! shows), so projected coordinates hundreds of kilometres from the world origin keep their
//! precision.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::sync::OnceLock;

use umcf_foam_mesh::Boundary;
use umcf_geom::bvh::{Bvh, Ray};
use umcf_geom::stl::Stl;
use umcf_geom::{BBox, Vec3};

pub mod camera;
pub mod guides;
pub mod png;
pub mod raster;

#[cfg(test)]
mod tests;

/// Where an object came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The boundary of a region's mesh (`""` for a single-region case); its classes are the
    /// patches, a triangle's source index is its mesh face.
    Mesh { region: String },
    /// An STL surface (case-relative path); its classes are the regions OpenFOAM makes of it, a
    /// triangle's source index is its index in the file.
    Surface { file: String },
    /// What is shown of a region's cells ([`Cells`]): those a plane cuts ([`Object::from_cut`];
    /// its classes are the cell zones, `cells` for those in none) or the faces and cells a mesh
    /// check named ([`Object::from_found`]; its classes are what was found). A triangle's source
    /// index is its mesh face.
    Cut { region: String },
}

/// What a cut ([`Source::Cut`]) shows of a region's cells.
#[derive(Debug, Clone)]
pub enum Cells {
    /// The cells a plane crosses, whole ([`Object::from_cut`]).
    Plane(Plane),
    /// The faces and cells a mesh check named ([`Object::from_found`]).
    Found(std::sync::Arc<Found>),
}

impl PartialEq for Cells {
    /// Planes by value; what was found by identity (a check run again is another).
    fn eq(&self, other: &Cells) -> bool {
        match (self, other) {
            (Cells::Plane(a), Cells::Plane(b)) => a == b,
            (Cells::Found(a), Cells::Found(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// The faces and cells a mesh check named, by what it found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub classes: Vec<FoundClass>,
}

/// One thing a mesh check found: the faces or cells where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundClass {
    /// What was found, for people (`skew faces`).
    pub name: String,
    /// Cells (else faces).
    pub cells: bool,
    /// Their labels in the mesh (the first, where there are many).
    pub labels: Vec<u32>,
    /// How many faces or cells it was found at, listed or not.
    pub count: usize,
}

/// A plane: the points `x` with `normal · (x - point) = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub point: Vec3,
    /// Not zero; its length doesn't matter.
    pub normal: Vec3,
}

/// One thing in the scene: triangles with a class (a patch, a surface region) each.
#[derive(Debug, Clone)]
pub struct Object {
    pub source: Source,
    /// Points relative to the scene's origin.
    pub points: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Each triangle's class: an index into `classes`.
    pub class: Vec<u32>,
    /// Each triangle's mesh face or STL triangle.
    pub origin_index: Vec<u32>,
    /// Which of each triangle's edges (bit 0: vertex 0 to 1, bit 1: 1 to 2, bit 2: 2 to 0) are
    /// edges of the face it came from, not a diagonal added to split the face.
    pub edges: Vec<u8>,
    /// The classes' names: patch or surface-region names.
    pub classes: Vec<String>,
    /// What each class is: a patch's type (`wall`, `patch`, `empty`, …), `region` for a
    /// surface region.
    pub kinds: Vec<String>,
    /// Each class's patch groups (`inGroups`); none for a surface region.
    pub groups: Vec<Vec<String>>,
    /// Faces or triangles left out: a coordinate that is not finite, or so far from the
    /// scene's origin that it doesn't fit in single precision.
    pub dropped: usize,
    /// Tells this object from every other built in this process (a copy keeps it): a renderer
    /// that keeps what it uploaded of an object finds it again by it. An object isn't changed
    /// once built; what changes is built again, with an id of its own.
    id: u64,
    bvh: OnceLock<Bvh>,
    wire: OnceLock<Wire>,
    /// The box around the points, relative to the scene's origin, found once.
    extent: OnceLock<Option<BBox>>,
}

/// An object's edges and points as wireframes and points draw them: each edge of a face once
/// per class (where two patches meet, each has it), each point once per class, an STL's corners
/// welded by position; each with a triangle it belongs to.
#[derive(Debug, Clone, Default)]
pub struct Wire {
    /// Two points and a triangle.
    pub edges: Vec<[u32; 3]>,
    /// A point and a triangle.
    pub points: Vec<[u32; 2]>,
}

/// Something a ray passes through.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    /// The object's index in the scene.
    pub object: usize,
    /// The class (patch, surface region) index in the object.
    pub class: u32,
    /// The mesh face or STL triangle.
    pub index: u32,
    /// Distance along the ray, in the ray direction's units.
    pub distance: f64,
    /// Where the ray hits, in world coordinates.
    pub point: Vec3,
}

/// The scene; see the crate documentation.
#[derive(Debug, Clone)]
pub struct Scene {
    /// World coordinates of the point the objects' points are relative to.
    pub origin: Vec3,
    pub objects: Vec<Object>,
}

fn relative(p: Vec3, origin: Vec3) -> [f32; 3] {
    let r = p - origin;
    [r.x as f32, r.y as f32, r.z as f32]
}

impl Object {
    fn new(source: Source) -> Object {
        Object {
            source,
            points: Vec::new(),
            triangles: Vec::new(),
            class: Vec::new(),
            origin_index: Vec::new(),
            edges: Vec::new(),
            classes: Vec::new(),
            kinds: Vec::new(),
            groups: Vec::new(),
            dropped: 0,
            id: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            bvh: OnceLock::new(),
            wire: OnceLock::new(),
            extent: OnceLock::new(),
        }
    }

    /// The boundary faces of `mesh`, by patch, each split into a fan of triangles from its first
    /// point. Faces with a point that is not finite are left out.
    pub fn from_mesh<M: Boundary + ?Sized>(region: &str, mesh: &M, origin: Vec3) -> Object {
        let mut o = Object::new(Source::Mesh { region: region.to_string() });
        let points = mesh.points();
        // Mesh point → object point, filled as boundary faces use them.
        let mut map: Vec<u32> = vec![u32::MAX; points.len()];
        for (k, patch) in mesh.patches().iter().enumerate() {
            let Ok(class) = u32::try_from(k) else { break };
            o.classes.push(patch.name.clone());
            o.kinds.push(patch.kind.clone());
            o.groups.push(patch.groups.clone());
            for f in patch.faces() {
                let (Some(face), Ok(f32_index)) = (mesh.face(f), u32::try_from(f)) else { continue };
                let mut ids: Vec<u32> = Vec::with_capacity(face.len());
                for p in face {
                    let Some(slot) = map.get_mut(*p as usize) else { break };
                    if *slot == u32::MAX {
                        let Some(w) = points.get(*p as usize) else { break };
                        let r = relative(Vec3::new(w[0], w[1], w[2]), origin);
                        if !r.iter().all(|c| c.is_finite()) {
                            break;
                        }
                        let Ok(id) = u32::try_from(o.points.len()) else { break };
                        o.points.push(r);
                        *slot = id;
                    }
                    ids.push(*slot);
                }
                if ids.len() != face.len() || ids.len() < 3 {
                    o.dropped += 1;
                    continue;
                }
                let n = ids.len();
                for i in 1..n - 1 {
                    let (Some(a), Some(b), Some(c)) = (ids.first(), ids.get(i), ids.get(i + 1)) else { continue };
                    o.triangles.push([*a, *b, *c]);
                    o.class.push(class);
                    o.origin_index.push(f32_index);
                    // Edge 1→2 is always a face edge; 0→1 only for the first triangle, 2→0 only
                    // for the last.
                    o.edges.push(0b010 | u8::from(i == 1) | (u8::from(i == n - 2) << 2));
                }
            }
        }
        o
    }

    /// Adds face `face` (labelled `index`) of a mesh with `points`, split into a fan from its
    /// first point, in class `class`; `map` takes each mesh point to the object's, filled as
    /// faces use them. A face with a point that is not finite (or out of range) is left out and
    /// counted.
    fn push_face(&mut self, points: &[[f64; 3]], map: &mut [u32], face: &[u32], index: u32, class: u32, origin: Vec3) {
        let mut ids: Vec<u32> = Vec::with_capacity(face.len());
        for p in face {
            let Some(slot) = map.get_mut(*p as usize) else { break };
            if *slot == u32::MAX {
                let Some(w) = points.get(*p as usize) else { break };
                let r = relative(Vec3::new(w[0], w[1], w[2]), origin);
                if !r.iter().all(|c| c.is_finite()) {
                    break;
                }
                let Ok(id) = u32::try_from(self.points.len()) else { break };
                self.points.push(r);
                *slot = id;
            }
            ids.push(*slot);
        }
        if ids.len() != face.len() || ids.len() < 3 {
            self.dropped += 1;
            return;
        }
        let k = ids.len();
        for i in 1..k - 1 {
            let (Some(a), Some(b), Some(c)) = (ids.first(), ids.get(i), ids.get(i + 1)) else { continue };
            self.triangles.push([*a, *b, *c]);
            self.class.push(class);
            self.origin_index.push(index);
            self.edges.push(0b010 | u8::from(i == 1) | (u8::from(i == k - 2) << 2));
        }
    }

    /// The faces and cells of `mesh` that `found` names, each in the class of what was found: a
    /// face as it is, a cell by all its faces. A label beyond the mesh is left out; a face of
    /// two cells named for one thing is drawn once.
    pub fn from_found(region: &str, mesh: &umcf_foam_mesh::PolyMesh, found: &Found, origin: Vec3) -> Object {
        let mut o = Object::new(Source::Cut { region: region.to_string() });
        let mut map: Vec<u32> = vec![u32::MAX; mesh.points.len()];
        // Each named cell's classes.
        let mut cells: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        for (k, c) in found.classes.iter().enumerate() {
            let Ok(class) = u32::try_from(k) else { break };
            o.classes.push(c.name.clone());
            o.kinds.push(if c.cells { "cells" } else { "faces" }.to_string());
            o.groups.push(Vec::new());
            if c.cells {
                for l in &c.labels {
                    let of = cells.entry(*l).or_default();
                    if !of.contains(&class) {
                        of.push(class);
                    }
                }
            } else {
                let mut seen = std::collections::HashSet::new();
                for f in &c.labels {
                    if !seen.insert(*f) {
                        continue;
                    }
                    if let Some(face) = mesh.face(*f as usize) {
                        o.push_face(&mesh.points, &mut map, face, *f, class, origin);
                    }
                }
            }
        }
        if !cells.is_empty() {
            let mut classes: Vec<u32> = Vec::new();
            for f in 0..mesh.n_faces() {
                classes.clear();
                for c in [mesh.owner.get(f), mesh.neighbour.get(f)].into_iter().flatten() {
                    for k in cells.get(c).into_iter().flatten() {
                        if !classes.contains(k) {
                            classes.push(*k);
                        }
                    }
                }
                if classes.is_empty() {
                    continue;
                }
                let (Some(face), Ok(index)) = (mesh.face(f), u32::try_from(f)) else { continue };
                for k in &classes {
                    o.push_face(&mesh.points, &mut map, face, index, *k, origin);
                }
            }
        }
        o
    }

    /// The cells of `mesh` that `plane` cuts, as a "crinkle" cut: each such cell whole, so the
    /// mesh's cells are seen as they are (no face is cut). A cell is cut when it has points on
    /// the plane's negative side and points on it or past it: of two layers of cells that only
    /// touch the plane (a plane on the faces between them), the one behind it. The layer is drawn
    /// by its skin: each face between a cut cell and one not cut, or of the boundary, once, split
    /// into a fan from its first point, in the class of the cell zone of its cut cell; `cells`
    /// for one in no zone. The faces between two cut cells are inside it, not drawn. A point that
    /// is not finite tells nothing of where its cells are, and the faces with one are left out.
    /// `None` when the normal is zero or not finite.
    pub fn from_cut(region: &str, mesh: &umcf_foam_mesh::PolyMesh, plane: Plane, origin: Vec3) -> Option<Object> {
        let n = plane.normal;
        let length = n.length();
        if !(length.is_finite() && length > 0.0) {
            return None;
        }
        let n = n * (1.0 / length);
        // Each point's signed distance, once (a point is on several faces).
        // A point not finite tells nothing: NaN, which min and max skip.
        let distance: Vec<f64> = mesh
            .points
            .iter()
            .map(|p| {
                if p.iter().all(|c| c.is_finite()) {
                    n.dot(Vec3::new(p[0], p[1], p[2]) - plane.point)
                } else {
                    f64::NAN
                }
            })
            .collect();
        // Each cell's nearest and farthest point, from its faces' points.
        let cells = mesh.n_cells();
        let mut low = vec![f64::INFINITY; cells];
        let mut high = vec![f64::NEG_INFINITY; cells];
        let mut reach = |cell: u32, d: f64| {
            if let (Some(lo), Some(hi)) = (low.get_mut(cell as usize), high.get_mut(cell as usize)) {
                *lo = lo.min(d);
                *hi = hi.max(d);
            }
        };
        for f in 0..mesh.n_faces() {
            let Some(face) = mesh.face(f) else { continue };
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in face {
                if let Some(d) = distance.get(*p as usize) {
                    lo = lo.min(*d);
                    hi = hi.max(*d);
                }
            }
            if lo > hi {
                // No finite point: the face says nothing about its cells.
                continue;
            }
            for c in [mesh.owner.get(f), mesh.neighbour.get(f)].into_iter().flatten() {
                reach(*c, lo);
                reach(*c, hi);
            }
        }
        let cut: Vec<bool> = low.iter().zip(&high).map(|(lo, hi)| *lo < 0.0 && *hi >= 0.0).collect();
        // Each cell's zone: the first that lists it.
        let mut zone: Vec<u32> = vec![u32::MAX; cells];
        for (z, cz) in mesh.cell_zones.iter().enumerate() {
            let Ok(z) = u32::try_from(z) else { break };
            for c in &cz.cells {
                if let Some(slot) = zone.get_mut(*c as usize)
                    && *slot == u32::MAX
                {
                    *slot = z;
                }
            }
        }
        let mut o = Object::new(Source::Cut { region: region.to_string() });
        // Zone (or u32::MAX, no zone) → class, as they are met.
        let mut class_of: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut map: Vec<u32> = vec![u32::MAX; mesh.points.len()];
        for f in 0..mesh.n_faces() {
            // The layer's skin: a face between a cut cell and one not cut, or the boundary. A face
            // between two cut cells is inside the layer: not seen, and drawn it would show
            // through where it meets the skin.
            let is_cut = |c: Option<&u32>| c.is_some_and(|c| cut.get(*c as usize).copied().unwrap_or(false));
            let (own, nb) = (mesh.owner.get(f), mesh.neighbour.get(f));
            let cell = match (is_cut(own), is_cut(nb)) {
                (true, false) => own,
                (false, true) => nb,
                _ => None,
            };
            let (Some(cell), Some(face), Ok(index)) = (cell, mesh.face(f), u32::try_from(f)) else { continue };
            let z = zone.get(*cell as usize).copied().unwrap_or(u32::MAX);
            let next = u32::try_from(o.classes.len()).unwrap_or(u32::MAX);
            let class = *class_of.entry(z).or_insert_with(|| {
                // The cells in no zone: `cells`, unless a zone is called so.
                let unzoned =
                    if mesh.cell_zones.iter().any(|cz| cz.name == "cells") { "cells (no zone)" } else { "cells" };
                let name = mesh.cell_zones.get(z as usize).map_or_else(|| unzoned.to_string(), |cz| cz.name.clone());
                o.classes.push(name);
                o.kinds.push(if z == u32::MAX { "cells" } else { "cellZone" }.to_string());
                o.groups.push(Vec::new());
                next
            });
            o.push_face(&mesh.points, &mut map, face, index, class, origin);
        }
        Some(o)
    }

    /// The triangles of `stl`, by region (see [`Stl::regions`]). Triangles with a coordinate
    /// that is not finite are left out.
    pub fn from_stl(file: &str, stl: &Stl, origin: Vec3) -> Object {
        let mut o = Object::new(Source::Surface { file: file.to_string() });
        let (names, region) = stl.regions();
        o.kinds = vec!["region".to_string(); names.len()];
        o.groups = vec![Vec::new(); names.len()];
        o.classes = names;
        for (k, (t, r)) in stl.triangles.iter().zip(&region).enumerate() {
            let (Ok(k), Ok(base)) = (u32::try_from(k), u32::try_from(o.points.len())) else { break };
            let p = t.points().map(|p| relative(p, origin));
            if !p.iter().flatten().all(|c| c.is_finite()) {
                o.dropped += 1;
                continue;
            }
            o.points.extend(p);
            o.triangles.push([base, base + 1, base + 2]);
            o.class.push(*r);
            o.origin_index.push(k);
            o.edges.push(0b111);
        }
        o
    }

    /// The box around the points `mesh`'s boundary faces use (points no boundary face uses
    /// don't move the scene's origin); `None` when there are none that are finite.
    pub fn boundary_bounds<M: Boundary + ?Sized>(mesh: &M) -> Option<BBox> {
        let points = mesh.points();
        let mut used = vec![false; points.len()];
        for patch in mesh.patches() {
            for f in patch.faces() {
                for p in mesh.face(f).unwrap_or_default() {
                    if let Some(u) = used.get_mut(*p as usize) {
                        *u = true;
                    }
                }
            }
        }
        BBox::of(points.iter().zip(&used).filter(|(_, u)| **u).map(|(p, _)| Vec3::new(p[0], p[1], p[2])))
    }

    /// A name for people: `mesh air`, `mesh` (single region), `surface constant/triSurface/x.stl`,
    /// `cut air`.
    pub fn label(&self) -> String {
        match &self.source {
            Source::Mesh { region } if region.is_empty() => "mesh".to_string(),
            Source::Mesh { region } => format!("mesh {region}"),
            Source::Surface { file } => format!("surface {file}"),
            Source::Cut { region } if region.is_empty() => "cut".to_string(),
            Source::Cut { region } => format!("cut {region}"),
        }
    }

    /// The box around the object's points, in world coordinates.
    pub fn bbox(&self, origin: Vec3) -> Option<BBox> {
        let extent = self.extent.get_or_init(|| BBox::of(self.points.iter().map(|p| Vec3::from_f32(*p))));
        extent.map(|b| BBox { min: origin + b.min, max: origin + b.max })
    }

    /// The object's edges and points (see [`Wire`]), found the first time they are asked for.
    pub fn wire(&self) -> &Wire {
        self.wire.get_or_init(|| {
            use std::collections::{HashMap, HashSet};
            // An STL's triangles have points of their own: the same corner is one point.
            let weld = matches!(self.source, Source::Surface { .. });
            let mut at: HashMap<[u32; 3], u32> = HashMap::new();
            let canonical: Vec<u32> = (0..self.points.len())
                .map(|i| {
                    let i = u32::try_from(i).unwrap_or(u32::MAX);
                    match self.points.get(i as usize) {
                        // -0 and 0 are the same place.
                        Some(p) if weld => *at.entry(p.map(|c| (c + 0.0).to_bits())).or_insert(i),
                        _ => i,
                    }
                })
                .collect();
            let mut edges = HashSet::new();
            let mut points = HashSet::new();
            let mut wire = Wire::default();
            for (n, tri) in self.triangles.iter().enumerate() {
                let (Ok(t), Some(class)) = (u32::try_from(n), self.class.get(n)) else { continue };
                let c = tri.map(|v| canonical.get(v as usize).copied().unwrap_or(v));
                let bits = self.edges.get(n).copied().unwrap_or(0);
                for (bit, (i, j)) in [(0b001u8, (0, 1)), (0b010, (1, 2)), (0b100, (2, 0))] {
                    let (Some(p), Some(q)) = (c.get(i), c.get(j)) else { continue };
                    let (a, b) = ((*p).min(*q), (*p).max(*q));
                    if bits & bit != 0 && a != b && edges.insert((a, b, *class)) {
                        wire.edges.push([a, b, t]);
                    }
                }
                for p in c {
                    if points.insert((p, *class)) {
                        wire.points.push([p, t]);
                    }
                }
            }
            wire
        })
    }

    /// The object's BVH, built the first time it is asked for.
    pub fn bvh(&self) -> &Bvh {
        self.bvh.get_or_init(|| Bvh::new(&self.points, &self.triangles))
    }

    /// The object's id: see the field.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// About how many bytes the object holds: its lists, and its BVH and edges once they were
    /// built.
    pub fn bytes(&self) -> u64 {
        use std::mem::size_of;
        fn of<T>(v: &Vec<T>) -> u64 {
            (v.capacity() * size_of::<T>()) as u64
        }
        let names = |v: &[String]| v.iter().map(|x| (x.capacity() + size_of::<String>()) as u64).sum::<u64>();
        let groups = self.groups.iter().map(|g| names(g) + size_of::<Vec<String>>() as u64).sum::<u64>();
        let wire = self.wire.get().map_or(0, |w| of(&w.edges) + of(&w.points));
        of(&self.points)
            + of(&self.triangles)
            + of(&self.class)
            + of(&self.origin_index)
            + of(&self.edges)
            + names(&self.classes)
            + names(&self.kinds)
            + groups
            + self.bvh.get().map_or(0, Bvh::bytes)
            + wire
    }
}

impl Scene {
    /// An empty scene whose points will be relative to `origin` (best: the middle of what it
    /// shows; see [`Scene::origin_for`]).
    pub fn new(origin: Vec3) -> Scene {
        Scene { origin, objects: Vec::new() }
    }

    /// The middle of `boxes` together (the world origin when there are none): the origin to
    /// build a scene of them with.
    pub fn origin_for(boxes: impl IntoIterator<Item = BBox>) -> Vec3 {
        BBox::of(boxes.into_iter().flat_map(|b| [b.min, b.max])).map_or(Vec3::ZERO, |b| b.centre())
    }

    /// The box around every object, in world coordinates.
    pub fn bbox(&self) -> Option<BBox> {
        BBox::of(self.objects.iter().filter_map(|o| o.bbox(self.origin)).flat_map(|b| [b.min, b.max]))
    }

    /// How many triangles the scene has.
    pub fn triangles(&self) -> usize {
        self.objects.iter().map(|o| o.triangles.len()).sum()
    }

    /// Each class (patch, surface region) `ray` passes through, at its nearest hit, nearest
    /// first: what lies under the cursor, however many times the ray crosses one class.
    pub fn pick_classes(&self, ray: &Ray) -> Vec<Pick> {
        let mut seen = std::collections::HashSet::new();
        self.pick(ray, usize::MAX).into_iter().filter(|p| seen.insert((p.object, p.class))).collect()
    }

    /// Everything `ray` (in world coordinates) passes through, nearest first, up to `limit`.
    /// Each object's BVH is built the first time it is needed.
    pub fn pick(&self, ray: &Ray, limit: usize) -> Vec<Pick> {
        let o = self.origin;
        let local =
            Ray { origin: [ray.origin[0] - o.x, ray.origin[1] - o.y, ray.origin[2] - o.z], direction: ray.direction };
        let mut all: Vec<Pick> = Vec::new();
        for (k, obj) in self.objects.iter().enumerate() {
            for h in obj.bvh().cast_all(&obj.points, &obj.triangles, &local, limit) {
                let (Some(class), Some(index)) =
                    (obj.class.get(h.triangle as usize), obj.origin_index.get(h.triangle as usize))
                else {
                    continue;
                };
                let d = ray.direction;
                let point =
                    Vec3::new(ray.origin[0] + h.t * d[0], ray.origin[1] + h.t * d[1], ray.origin[2] + h.t * d[2]);
                all.push(Pick { object: k, class: *class, index: *index, distance: h.t, point });
            }
        }
        all.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        all.truncate(limit);
        all
    }
}
