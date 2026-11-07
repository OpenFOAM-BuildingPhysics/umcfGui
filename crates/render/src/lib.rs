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

    /// A name for people: `mesh air`, `mesh` (single region), `surface constant/triSurface/x.stl`.
    pub fn label(&self) -> String {
        match &self.source {
            Source::Mesh { region } if region.is_empty() => "mesh".to_string(),
            Source::Mesh { region } => format!("mesh {region}"),
            Source::Surface { file } => format!("surface {file}"),
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
