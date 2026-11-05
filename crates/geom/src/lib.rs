//! Geometry basics for umcfGui: vectors ([`Vec3`]), bounding boxes ([`BBox`]) and STL surfaces.
//!
//! - [`stl`] reads ASCII and binary STL files without trusting the header (the size of the file
//!   decides), with named `solid`s, and writes them back.
//! - [`check`] finds what breaks meshing: open and non-manifold edges, inconsistent and inward
//!   normals, degenerate and duplicate triangles, coordinates that are not finite. Each finding
//!   lists the triangles involved.
//! - [`bvh`] finds the triangles a ray hits, for picking.
//! - [`repair`] holds the safe repairs: welding vertices within a tolerance, dropping degenerate
//!   and duplicate triangles, making normals consistent and outward, scaling.
//!
//! STL files are hostile input: every count is bounded ([`stl::Limits`]) and nothing here
//! panics on a malformed file.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod bvh;
pub mod check;
pub mod near;
pub mod repair;
pub mod stl;
pub mod trees;

/// A point or direction in 3D, in double precision.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    pub fn from_f32(p: [f32; 3]) -> Self {
        let [x, y, z] = p;
        Vec3::new(f64::from(x), f64::from(y), f64::from(z))
    }

    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// The largest absolute component.
    pub fn max_abs(self) -> f64 {
        self.x.abs().max(self.y.abs()).max(self.z.abs())
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl std::ops::Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

/// An axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min: Vec3,
    pub max: Vec3,
}

impl BBox {
    /// The box of one point.
    pub fn at(p: Vec3) -> Self {
        BBox { min: p, max: p }
    }

    /// Grows the box to hold `p`.
    pub fn add(&mut self, p: Vec3) {
        self.min = Vec3::new(self.min.x.min(p.x), self.min.y.min(p.y), self.min.z.min(p.z));
        self.max = Vec3::new(self.max.x.max(p.x), self.max.y.max(p.y), self.max.z.max(p.z));
    }

    /// The box of the finite points among `points`; `None` if there are none.
    pub fn of(points: impl IntoIterator<Item = Vec3>) -> Option<BBox> {
        let mut b: Option<BBox> = None;
        for p in points.into_iter().filter(|p| p.is_finite()) {
            match &mut b {
                Some(b) => b.add(p),
                None => b = Some(BBox::at(p)),
            }
        }
        b
    }

    /// Edge lengths along x, y and z.
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn centre(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

#[cfg(test)]
mod tests;
