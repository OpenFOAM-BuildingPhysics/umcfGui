//! The 3D scene of a case: its meshes' boundary patches and its STL surfaces, as triangles that
//! each know where they came from (a mesh face of a patch, an STL triangle of a region), so what
//! is drawn or picked is a patch or a region, not a pixel.
//!
//! - [`Scene`] holds the [`Object`]s, built from `foam-mesh` and `geom` data. It doesn't depend
//!   on any renderer: the GUI's viewport and the CPU rasteriser here draw the same scene.
//! - [`camera`] places the eye (standard views, framing a box) and gives the ray through a pixel.
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

use umcf_foam_mesh::PolyMesh;
use umcf_geom::bvh::{Bvh, Ray};
use umcf_geom::stl::Stl;
use umcf_geom::{BBox, Vec3};

pub mod camera;
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
    /// Faces or triangles left out: a coordinate that is not finite, or so far from the
    /// scene's origin that it doesn't fit in single precision.
    pub dropped: usize,
    bvh: OnceLock<Bvh>,
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
            dropped: 0,
            bvh: OnceLock::new(),
        }
    }

    /// The boundary faces of `mesh`, by patch, each split into a fan of triangles from its first
    /// point. Faces with a point that is not finite are left out.
    pub fn from_mesh(region: &str, mesh: &PolyMesh, origin: Vec3) -> Object {
        let mut o = Object::new(Source::Mesh { region: region.to_string() });
        // Mesh point → object point, filled as boundary faces use them.
        let mut map: Vec<u32> = vec![u32::MAX; mesh.n_points()];
        for (k, patch) in mesh.boundary.iter().enumerate() {
            let Ok(class) = u32::try_from(k) else { break };
            o.classes.push(patch.name.clone());
            o.kinds.push(patch.kind.clone());
            for f in patch.faces() {
                let (Some(face), Ok(f32_index)) = (mesh.face(f), u32::try_from(f)) else { continue };
                let mut ids: Vec<u32> = Vec::with_capacity(face.len());
                for p in face {
                    let Some(slot) = map.get_mut(*p as usize) else { break };
                    if *slot == u32::MAX {
                        let Some(w) = mesh.points.get(*p as usize) else { break };
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
    pub fn boundary_bounds(mesh: &PolyMesh) -> Option<BBox> {
        let mut used = vec![false; mesh.n_points()];
        for patch in &mesh.boundary {
            for f in patch.faces() {
                for p in mesh.face(f).unwrap_or_default() {
                    if let Some(u) = used.get_mut(*p as usize) {
                        *u = true;
                    }
                }
            }
        }
        BBox::of(mesh.points.iter().zip(&used).filter(|(_, u)| **u).map(|(p, _)| Vec3::new(p[0], p[1], p[2])))
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
        BBox::of(self.points.iter().map(|p| origin + Vec3::from_f32(*p)))
    }

    /// The object's BVH, built the first time it is asked for.
    pub fn bvh(&self) -> &Bvh {
        self.bvh.get_or_init(|| Bvh::new(&self.points, &self.triangles))
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
