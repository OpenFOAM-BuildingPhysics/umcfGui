//! Where the scene is seen from: an orbit camera around a target point, z up.

use umcf_geom::bvh::Ray;
use umcf_geom::{BBox, Vec3};

/// A standard view direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// From the front, the right and above (azimuth 45°, elevation 35.26°: the three axes at
    /// equal angles).
    Iso,
    /// From above, looking down (−z), x to the right.
    Top,
    /// From below, looking up (+z).
    Bottom,
    /// From −y, looking along +y.
    Front,
    /// From +y, looking along −y.
    Back,
    /// From −x, looking along +x.
    Left,
    /// From +x, looking along −x.
    Right,
}

impl View {
    pub const ALL: [(&'static str, View); 7] = [
        ("iso", View::Iso),
        ("top", View::Top),
        ("bottom", View::Bottom),
        ("front", View::Front),
        ("back", View::Back),
        ("left", View::Left),
        ("right", View::Right),
    ];

    pub fn parse(name: &str) -> Option<View> {
        View::ALL.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }

    /// Azimuth and elevation, in degrees.
    pub fn angles(self) -> (f64, f64) {
        match self {
            View::Iso => (45.0, 35.264_389_682_754_654),
            View::Top => (0.0, 90.0),
            View::Bottom => (0.0, -90.0),
            View::Front => (0.0, 0.0),
            View::Back => (180.0, 0.0),
            View::Left => (-90.0, 0.0),
            View::Right => (90.0, 0.0),
        }
    }
}

/// An orbit camera in world coordinates. The eye is `distance` from `target` in the direction
/// given by `azimuth` (degrees, 0 from −y, 90 from +x) and `elevation` (degrees above the xy
/// plane, ±90 straight down or up). The image's x axis is always horizontal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f64,
    pub azimuth: f64,
    pub elevation: f64,
    /// Vertical field of view, degrees.
    pub fov: f64,
}

/// The camera's frame: the eye and three unit axes.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub eye: Vec3,
    /// To the right of the image.
    pub right: Vec3,
    /// Up in the image.
    pub up: Vec3,
    /// Into the image.
    pub forward: Vec3,
    /// `tan(fov / 2)`.
    pub tan_half: f64,
}

/// The default vertical field of view, degrees.
pub const FOV: f64 = 30.0;

impl Camera {
    /// A camera looking from `view` at the whole of `bbox`, for an image of `aspect` (width
    /// over height).
    pub fn fit(view: View, bbox: &BBox, aspect: f64) -> Camera {
        let (azimuth, elevation) = view.angles();
        Camera::fit_angles(azimuth, elevation, bbox, aspect)
    }

    /// As [`Camera::fit`], from any azimuth and elevation.
    pub fn fit_angles(azimuth: f64, elevation: f64, bbox: &BBox, aspect: f64) -> Camera {
        let radius = (bbox.size().length() * 0.5).max(1e-9);
        let half = (FOV * 0.5).to_radians();
        // The narrower of the two fields of view decides.
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let narrow = if aspect < 1.0 { (half.tan() * aspect).atan() } else { half };
        let distance = radius / narrow.sin() * 1.02;
        Camera { target: bbox.centre(), distance, azimuth, elevation, fov: FOV }
    }

    pub fn frame(&self) -> Frame {
        let (az, el) = (self.azimuth.to_radians(), self.elevation.clamp(-90.0, 90.0).to_radians());
        let back = Vec3::new(el.cos() * az.sin(), -el.cos() * az.cos(), el.sin());
        let forward = back * -1.0;
        let right = Vec3::new(az.cos(), az.sin(), 0.0);
        let up = right.cross(forward);
        let fov = if self.fov.is_finite() && self.fov > 0.1 && self.fov < 170.0 { self.fov } else { FOV };
        Frame { eye: self.target + back * self.distance, right, up, forward, tan_half: (fov * 0.5).to_radians().tan() }
    }

    /// The ray from the eye through the middle of pixel (`x`, `y`) of a `width` × `height`
    /// image (`x` to the right, `y` down, from the top left corner), with a unit direction.
    pub fn ray(&self, x: f64, y: f64, width: u32, height: u32) -> Ray {
        let f = self.frame();
        let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
        let sx = ((x + 0.5) / w * 2.0 - 1.0) * f.tan_half * w / h;
        let sy = (1.0 - (y + 0.5) / h * 2.0) * f.tan_half;
        let d = f.forward + f.right * sx + f.up * sy;
        let d = d * (1.0 / d.length());
        Ray { origin: [f.eye.x, f.eye.y, f.eye.z], direction: [d.x, d.y, d.z] }
    }
}
