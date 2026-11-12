//! What the view draws beside the scene to place it: a grid on the ground (the plane z = 0) and
//! the origin, its three coordinate planes and its axes. Both renderers (the CPU rasteriser and
//! the desktop app's GPU view) draw the lines and squares made here, so that the two show the same.
//!
//! - The grid lies under everything: what the scene shows hides it, and it never hides the
//!   scene. Its spacing follows the zoom in steps of 1, 2 and 5 (a major line every fifth), and
//!   it fades out with the distance from the middle of the view.
//! - The origin's planes are small translucent squares (xy, xz, yz, on the positive side of each
//!   axis), sized with the view, with their edges and the axes (x red, y green, z blue): what
//!   the scene shows in front of them hides them.
//!
//! - A cut's plane, while it is moved (the desktop app's cut card): a translucent rectangle
//!   over the region's extent, a little past it so that its frame shows outside the cut cells,
//!   its frame, and an arrow along its normal from its middle, the handles a drag moves it by
//!   ([`CutPlane::handles`]); brighter while it is held or the pointer is on it.
//! - A grid of blocks, while it is set (the desktop app's box card): the box's edges and its
//!   stations as strong lines, its cells' lines fainter, on the six sides of the box
//!   ([`BlockPreview`]); hidden by what the scene shows in front of them.
//!
//! The spacing, the extent, the fade and the colours are ported from SolidCraft's viewport
//! (<https://github.com/tangency-labs/solidcraft>, MIT or Apache-2.0, at commit 9325e3a:
//! `crates/engine/src/view.rs` `grid_step`, `grid_ext`, `GridFade`, and
//! `crates/ui-egui/src/selection.rs` `origin_size`). Those parts:
//! Copyright (c) 2026 SolidCraft contributors, under the MIT licence
//! (`assets/licenses/MIT-SolidCraft.txt`; `ATTRIBUTION.md`).

use umcf_geom::{BBox, Vec3};

use crate::Scene;
use crate::camera::Camera;

/// Which guides are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guides {
    pub grid: bool,
    pub origin: bool,
}

impl Guides {
    /// Both, as the window and the snapshots draw them unless told otherwise.
    pub const ALL: Guides = Guides { grid: true, origin: true };
}

/// A line, from `a` to `b` (relative to the scene's origin), in `colour` with `alpha`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub colour: [f32; 3],
    pub alpha: f32,
}

/// A filled square, its corners in order (relative to the scene's origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Square {
    pub corners: [[f32; 3]; 4],
    pub colour: [f32; 3],
    pub alpha: f32,
}

/// A cut's plane, drawn to be dragged along its normal (world coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutPlane {
    /// A point on the plane.
    pub point: Vec3,
    /// Its normal (any length but zero).
    pub normal: Vec3,
    /// The region's extent: the rectangle covers it, seen along the normal.
    pub extent: BBox,
    /// Held, or the pointer on a handle: drawn brighter.
    pub active: bool,
}

/// A grid of blocks to preview (world coordinates): along each axis, where its cells' lines
/// are (the first and the last the box's sides) and where its blocks' stations are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockPreview {
    pub cells: [Vec<f64>; 3],
    pub stations: [Vec<f64>; 3],
}

/// The most lines of cells drawn along an axis: past it, every n-th (the stations always).
pub const PREVIEW_LINES: usize = 120;

/// The grid of blocks' colours: light over a dark background, dark over a light one.
const BLOCKS_DARK: [u8; 3] = [120, 200, 255];
const BLOCKS_LIGHT: [u8; 3] = [20, 90, 190];

/// How far the rectangle reaches past the extent, of its half sides.
const CUT_MARGIN: f64 = 0.15;
/// The arrow's length, of the rectangle's longer half side.
const ARROW: f64 = 0.6;

/// The handles of a cut's plane (world coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handles {
    /// The rectangle's corners, in order.
    pub corners: [Vec3; 4],
    /// The arrow, from the rectangle's middle (on the plane) to its tip.
    pub base: Vec3,
    pub tip: Vec3,
    /// The unit normal and two unit axes in the plane.
    pub normal: Vec3,
    pub u: Vec3,
    pub v: Vec3,
}

impl CutPlane {
    /// The rectangle over the extent seen along the normal (its middle the extent's middle
    /// moved onto the plane), a little past it, and the arrow; `None` for a normal of zero
    /// length or anything not finite.
    pub fn handles(&self) -> Option<Handles> {
        let length = self.normal.length();
        if !(length.is_finite() && length > 0.0) {
            return None;
        }
        let n = self.normal * (1.0 / length);
        let other = if n.z.abs() < 0.9 { Vec3::new(0.0, 0.0, 1.0) } else { Vec3::new(1.0, 0.0, 0.0) };
        let u = n.cross(other);
        let u = u * (1.0 / u.length());
        let v = n.cross(u);
        let middle = self.extent.centre();
        let c = middle - n * n.dot(middle - self.point);
        let half = self.extent.size() * 0.5;
        let reach = |a: Vec3| a.x.abs() * half.x + a.y.abs() * half.y + a.z.abs() * half.z;
        let (hu, hv) = (reach(u) * (1.0 + CUT_MARGIN), reach(v) * (1.0 + CUT_MARGIN));
        let corners = [c - u * hu - v * hv, c + u * hu - v * hv, c + u * hu + v * hv, c - u * hu + v * hv];
        let tip = c + n * (hu.max(hv) * ARROW);
        let ok = corners.iter().chain([&c, &tip]).all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite());
        ok.then_some(Handles { corners, base: c, tip, normal: n, u, v })
    }
}

/// The cut's plane: its rectangle, frame and arrow. Light over a dark background, dark over a
/// light one; brighter when active.
const CUT_DARK: [u8; 3] = [255, 214, 102];
const CUT_LIGHT: [u8; 3] = [196, 120, 0];

/// What is drawn: the grid's lines (under everything), the origin's squares and lines (hidden
/// by what is in front of them), each in the order it is drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawn {
    pub grid: Vec<Line>,
    pub squares: Vec<Square>,
    pub lines: Vec<Line>,
}

fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r, g, b].map(|c| f32::from(c) / 255.0)
}

/// The grid's minor and major lines' colours over a dark background, and over a light one.
const GRID_DARK: ([u8; 3], [u8; 3]) = ([59, 65, 76], [73, 79, 90]);
const GRID_LIGHT: ([u8; 3], [u8; 3]) = ([196, 202, 212], [170, 178, 190]);
/// A minor line's strength (it is thinner: drawn one pixel wide, fainter).
const MINOR: f32 = 0.7;
/// The most lines on each side of the middle.
const MOST: i64 = 300;
/// The steps of the fade at each end of a line.
const FADE_STEPS: usize = 12;
/// The origin's squares and their edges.
pub const PLANE: ([u8; 3], u8) = ([249, 184, 134], 94);
const PLANE_EDGE: ([u8; 3], u8) = ([214, 140, 84], 200);
/// The axes, x, y and z.
pub const AXES: [[u8; 3]; 3] = [[220, 60, 60], [60, 170, 60], [60, 100, 230]];
/// How long an axis is (it is dashed past the squares' side), in squares' sides.
const AXIS_LENGTH: f64 = 1.8;

/// The grid's minor and major spacing for a view `half_height` (world units) high from its
/// middle to its top: 10 to 25 minor cells over that, a step of 1, 2 or 5 times a power of ten.
pub fn grid_step(half_height: f64) -> (f64, f64) {
    let raw = (half_height / 10.0).max(1e-3);
    let p = 10f64.powf(raw.log10().floor());
    let m = raw / p;
    let minor = if m < 2.0 {
        p
    } else if m < 5.0 {
        2.0 * p
    } else {
        5.0 * p
    };
    (minor, minor * 5.0)
}

/// The side of the origin's squares for a view `half_height` high: 7.5 % of the view's height.
pub fn origin_size(half_height: f64) -> f64 {
    (half_height * 2.0 * 0.075).max(1e-9)
}

/// The guides `which` for `scene` seen by `camera`; `dark` for the colours over a dark
/// background.
pub fn build(
    scene: &Scene,
    camera: &Camera,
    which: Guides,
    dark: bool,
    cut: Option<&CutPlane>,
    blocks: Option<&BlockPreview>,
) -> Drawn {
    let mut out = Drawn::default();
    let f = camera.frame();
    let half_height = camera.distance * f.tan_half;
    if !(half_height.is_finite() && half_height > 0.0) {
        return out;
    }
    let rel = |p: Vec3| {
        let r = p - scene.origin;
        [r.x as f32, r.y as f32, r.z as f32]
    };
    if which.grid {
        grid(&mut out, camera, half_height, dark, &rel);
    }
    if which.origin {
        let s = origin_size(half_height);
        let o = Vec3::ZERO;
        let (x, y, z) = (Vec3::new(s, 0.0, 0.0), Vec3::new(0.0, s, 0.0), Vec3::new(0.0, 0.0, s));
        for (u, v) in [(x, y), (x, z), (y, z)] {
            let corners = [o, o + u, o + u + v, o + v];
            let (c, a) = PLANE;
            out.squares.push(Square {
                corners: corners.map(&rel),
                colour: rgb(c[0], c[1], c[2]),
                alpha: f32::from(a) / 255.0,
            });
            let (c, a) = PLANE_EDGE;
            for k in 0..4 {
                let (Some(p), Some(q)) = (corners.get(k), corners.get((k + 1) % 4)) else { continue };
                out.lines.push(Line {
                    a: rel(*p),
                    b: rel(*q),
                    colour: rgb(c[0], c[1], c[2]),
                    alpha: f32::from(a) / 255.0,
                });
            }
        }
        // Each axis solid to the squares' side, then dashed.
        for (dir, c) in [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)].iter().zip(AXES)
        {
            let colour = rgb(c[0], c[1], c[2]);
            out.lines.push(Line { a: rel(o), b: rel(*dir * s), colour, alpha: 1.0 });
            let dash = s * 0.08;
            let mut at = s + dash;
            while at < s * AXIS_LENGTH {
                out.lines.push(Line { a: rel(*dir * at), b: rel(*dir * (at + dash)), colour, alpha: 1.0 });
                at += dash * 2.0;
            }
        }
    }
    if let Some(h) = cut.and_then(|c| c.handles().map(|h| (h, c.active))) {
        plane(&mut out, h.0, h.1, dark, f.eye, &rel);
    }
    if let Some(b) = blocks {
        block_lines(&mut out, b, dark, &rel);
    }
    out
}

/// A grid of blocks: on each of the box's six sides, the lines across it at each axis'
/// stations (strong) and cells (faint, at most [`PREVIEW_LINES`] along an axis). Nothing for a
/// grid without two finite stations along each axis.
fn block_lines(out: &mut Drawn, b: &BlockPreview, dark: bool, rel: &dyn Fn(Vec3) -> [f32; 3]) {
    let ends: Vec<(f64, f64)> = b
        .stations
        .iter()
        .filter_map(|s| match (s.first(), s.last()) {
            (Some(lo), Some(hi)) if lo.is_finite() && hi.is_finite() && lo < hi => Some((*lo, *hi)),
            _ => None,
        })
        .collect();
    let [(x0, x1), (y0, y1), (z0, z1)] = ends.as_slice() else { return };
    let (lo, hi) = ([*x0, *y0, *z0], [*x1, *y1, *z1]);
    let c = if dark { BLOCKS_DARK } else { BLOCKS_LIGHT };
    let colour = rgb(c[0], c[1], c[2]);
    let point = |v: [f64; 3]| Vec3::new(v[0], v[1], v[2]);
    for a in 0..3 {
        let (b1, b2) = ((a + 1) % 3, (a + 2) % 3);
        // At each position along axis a: across the sides at b1's ends (along b2) and at b2's
        // ends (along b1).
        let mut lines = |at: f64, alpha: f32| {
            if !(at.is_finite() && at >= lo[a] && at <= hi[a]) {
                return;
            }
            for (fixed, along) in [(b1, b2), (b2, b1)] {
                for end in [lo[fixed], hi[fixed]] {
                    let mut p = [0.0; 3];
                    p[a] = at;
                    p[fixed] = end;
                    let mut q = p;
                    p[along] = lo[along];
                    q[along] = hi[along];
                    out.lines.push(Line { a: rel(point(p)), b: rel(point(q)), colour, alpha });
                }
            }
        };
        let cells = b.cells.get(a).map(Vec::as_slice).unwrap_or_default();
        let every = cells.len().div_ceil(PREVIEW_LINES).max(1);
        for (k, at) in cells.iter().enumerate() {
            if k % every == 0 {
                lines(*at, 0.35);
            }
        }
        for at in b.stations.get(a).map(Vec::as_slice).unwrap_or_default() {
            lines(*at, 1.0);
        }
    }
}

/// The sides of the arrow's shaft and head.
const ARROW_SIDES: usize = 16;

/// A cut's plane: the rectangle, its frame, and the arrow, solid: a shaft and a cone, each facet
/// shaded by how squarely it faces the eye at `eye`. Those facing away are left out, and the
/// parts go far to near (the squares are drawn in order, not sorted by depth).
fn plane(out: &mut Drawn, h: Handles, active: bool, dark: bool, eye: Vec3, rel: &dyn Fn(Vec3) -> [f32; 3]) {
    let c = if dark { CUT_DARK } else { CUT_LIGHT };
    let colour = rgb(c[0], c[1], c[2]);
    let (fill, line) = if active { (0.22, 1.0) } else { (0.12, 0.8) };
    out.squares.push(Square { corners: h.corners.map(rel), colour, alpha: fill });
    for k in 0..4 {
        let (Some(p), Some(q)) = (h.corners.get(k), h.corners.get((k + 1) % 4)) else { continue };
        out.lines.push(Line { a: rel(*p), b: rel(*q), colour, alpha: line });
    }
    let length = (h.tip - h.base).length();
    let (shaft_r, head_r, head) = (length * 0.025, length * 0.075, length * 0.25);
    let neck = h.tip - h.normal * head;
    let ring = |centre: Vec3, r: f64, k: usize| {
        let a = std::f64::consts::TAU * k as f64 / ARROW_SIDES as f64;
        centre + h.u * (r * a.cos()) + h.v * (r * a.sin())
    };
    // Lit as it faces the eye; brighter held.
    let lift = if active { 1.0 } else { 0.85 };
    let mut facet = |out: &mut Drawn, corners: [Vec3; 4], outward: Vec3| {
        let [a, b, c, d] = corners;
        let middle = (a + b + c + d) * 0.25;
        let to_eye = eye - middle;
        let facing = outward.dot(to_eye) / (outward.length() * to_eye.length()).max(1e-300);
        if facing.is_nan() || facing <= 0.0 {
            return;
        }
        let shade = ((0.45 + 0.55 * facing) * lift) as f32;
        out.squares.push(Square {
            corners: corners.map(rel),
            colour: colour.map(|x| (x * shade).min(1.0)),
            alpha: 1.0,
        });
    };
    let shaft = |out: &mut Drawn, facet: &mut dyn FnMut(&mut Drawn, [Vec3; 4], Vec3)| {
        for k in 0..ARROW_SIDES {
            let (a, b) = (ring(h.base, shaft_r, k), ring(h.base, shaft_r, k + 1));
            let (c, d) = (ring(neck, shaft_r, k + 1), ring(neck, shaft_r, k));
            facet(out, [a, b, c, d], (a + b) * 0.5 - h.base);
        }
    };
    let cone = |out: &mut Drawn, facet: &mut dyn FnMut(&mut Drawn, [Vec3; 4], Vec3)| {
        for k in 0..ARROW_SIDES {
            let (a, b) = (ring(neck, head_r, k), ring(neck, head_r, k + 1));
            // Outward: across the side, tilted toward the tip by the cone's slope.
            let side = (a + b) * 0.5 - neck;
            let outward = side * (1.0 / side.length().max(1e-300)) + h.normal * (head_r / head);
            facet(out, [a, b, h.tip, h.tip], outward);
        }
        // Its base, seen from behind.
        for k in 0..ARROW_SIDES {
            let (a, b) = (ring(neck, head_r, k + 1), ring(neck, head_r, k));
            facet(out, [neck, a, b, b], h.normal * -1.0);
        }
    };
    // Seen from past the head's base the shaft is behind the head; from short of it (the head's
    // base facing the eye), in front of it.
    if h.normal.dot(eye - neck) >= 0.0 {
        shaft(out, &mut facet);
        cone(out, &mut facet);
    } else {
        cone(out, &mut facet);
        shaft(out, &mut facet);
    }
}

/// The grid on z = 0 around where the view's axis meets it, its lines fading out with the
/// distance from there.
fn grid(out: &mut Drawn, camera: &Camera, half_height: f64, dark: bool, rel: &dyn Fn(Vec3) -> [f32; 3]) {
    let f = camera.frame();
    let (minor, major) = grid_step(half_height);
    // How squarely the ground is seen (1 from above, 0 edge on).
    let slant = f.forward.z.abs().max(0.25);
    // The middle: where the view's axis meets the ground, else (seen edge on, or looking up
    // away from it) the point under the target; on a major line.
    let along = if f.forward.z.abs() > 1e-6 { -f.eye.z / f.forward.z } else { -1.0 };
    let hit = f.eye + f.forward * along;
    let near = along > 0.0 && (hit - camera.target).length() < camera.distance * 20.0;
    let (cx, cy) = if near { (hit.x, hit.y) } else { (camera.target.x, camera.target.y) };
    let (cx, cy) = ((cx / major).round() * major, (cy / major).round() * major);
    // How far it reaches, and where it fades out.
    let reach = half_height / slant;
    let ext = ((half_height * 3.0 / major).ceil() * major / slant / major).ceil() * major;
    let end = (2.2 * reach).min(ext);
    let start = 0.6 * reach.min(end);
    let n = ((ext / minor).ceil() as i64).min(MOST);
    let (minor_c, major_c) = if dark { GRID_DARK } else { GRID_LIGHT };
    for axis in 0..2 {
        for k in -n..=n {
            let off = k as f64 * minor;
            let is_major = k.rem_euclid(5) == 0;
            let c = if is_major { major_c } else { minor_c };
            let colour = rgb(c[0], c[1], c[2]);
            let strength = if is_major { 1.0 } else { MINOR };
            // The line's points at `t` along it, from -ext to ext.
            let at = |t: f64| {
                if axis == 0 { Vec3::new(cx + off, cy + t, 0.0) } else { Vec3::new(cx + t, cy + off, 0.0) }
            };
            fade(out, &at, off.abs(), ext, start, end, colour, strength, rel);
        }
    }
}

/// The line `at(-ext)`–`at(ext)`, `off` from the middle, as pieces of constant alpha: whole
/// within `start` of the middle, fading to nothing at `end`, in steps.
#[allow(clippy::too_many_arguments)]
fn fade(
    out: &mut Drawn,
    at: &dyn Fn(f64) -> Vec3,
    off: f64,
    ext: f64,
    start: f64,
    end: f64,
    colour: [f32; 3],
    strength: f32,
    rel: &dyn Fn(Vec3) -> [f32; 3],
) {
    if off >= end {
        return;
    }
    // Where along the line the distance from the middle is `r`.
    let along = |r: f64| (r * r - off * off).max(0.0).sqrt().min(ext);
    let alpha = |r: f64| {
        if r <= start { 1.0 } else { (1.0 - (r - start) / (end - start).max(1e-12)).clamp(0.0, 1.0) }
    };
    let solid = if off < start { along(start) } else { 0.0 };
    if solid > 0.0 {
        out.grid.push(Line { a: rel(at(-solid)), b: rel(at(solid)), colour, alpha: strength });
    }
    // From `solid` to where the line leaves `end`, in steps, on both sides.
    let last = along(end);
    if last <= solid {
        return;
    }
    let step = (last - solid) / FADE_STEPS as f64;
    for i in 0..FADE_STEPS {
        let (t0, t1) = (solid + step * i as f64, solid + step * (i + 1) as f64);
        let mid = (t0 + t1) * 0.5;
        let a = alpha((mid * mid + off * off).sqrt()) as f32 * strength;
        if a <= 0.0 {
            continue;
        }
        for sign in [1.0, -1.0] {
            out.grid.push(Line { a: rel(at(sign * t0)), b: rel(at(sign * t1)), colour, alpha: a });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::View;
    use umcf_geom::BBox;

    #[test]
    fn the_grid_steps_one_two_five_and_a_major_every_fifth() {
        assert_eq!(grid_step(10.0), (1.0, 5.0));
        assert_eq!(grid_step(25.0), (2.0, 10.0));
        assert_eq!(grid_step(60.0), (5.0, 25.0));
        assert_eq!(grid_step(100.0), (10.0, 50.0));
        assert_eq!(grid_step(1.0).0, 0.1);
    }

    #[test]
    fn the_guides_are_where_they_should_be() {
        let scene = Scene::new(Vec3::new(100.0, 200.0, 0.0));
        let camera =
            Camera::fit(View::Iso, &BBox { min: Vec3::new(90.0, 190.0, 0.0), max: Vec3::new(110.0, 210.0, 10.0) }, 1.5);
        let d = build(&scene, &camera, Guides::ALL, true, None, None);
        assert!(!d.grid.is_empty() && d.squares.len() == 3 && d.lines.len() > 12);
        // The grid on z = 0 (relative to the scene's origin, also 0 here), faded towards its ends.
        assert!(d.grid.iter().all(|l| l.a[2] == 0.0 && l.b[2] == 0.0));
        assert!(d.grid.iter().any(|l| l.alpha < 0.3) && d.grid.iter().any(|l| l.alpha >= 1.0));
        // The origin's xy square from the world's origin: (-100, -200) relative to the scene's.
        assert_eq!(d.squares[0].corners[0], [-100.0, -200.0, 0.0]);
        // Nothing asked, nothing drawn.
        assert_eq!(build(&scene, &camera, Guides::default(), true, None, None), Drawn::default());
    }

    #[test]
    fn a_cut_s_plane_covers_the_extent_and_has_its_arrow() {
        let extent = BBox { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(10.0, 20.0, 4.0) };
        let plane =
            CutPlane { point: Vec3::new(3.0, 0.0, 0.0), normal: Vec3::new(2.0, 0.0, 0.0), extent, active: false };
        let h = plane.handles().unwrap();
        // Across x at 3, over y and z a little past the extent.
        assert!(h.corners.iter().all(|p| p.x == 3.0));
        let ys: Vec<f64> = h.corners.iter().map(|p| p.y).collect();
        assert!(ys.iter().any(|y| *y < 0.0) && ys.iter().any(|y| *y > 20.0) && ys.iter().all(|y| y.abs() < 22.0));
        assert_eq!(h.base, Vec3::new(3.0, 10.0, 2.0));
        assert!(h.tip.x > 3.0 && h.tip.y == 10.0);
        // Drawn: the rectangle, its frame, and the arrow solid: the facets facing the eye, of
        // the shaft's and the head's sides (and the head's base, seen from behind), shaded.
        let scene = Scene::new(Vec3::ZERO);
        let camera = Camera::fit(View::Iso, &extent, 1.5);
        let d = build(&scene, &camera, Guides::default(), true, Some(&plane), None);
        assert_eq!(d.lines.len(), 4);
        let solid: Vec<&Square> = d.squares.iter().filter(|q| q.alpha == 1.0).collect();
        assert!(solid.len() >= ARROW_SIDES && solid.len() < 3 * ARROW_SIDES, "{}", solid.len());
        assert!(solid.iter().any(|q| q.colour != solid[0].colour), "shaded");
        let held = build(&scene, &camera, Guides::default(), true, Some(&CutPlane { active: true, ..plane }), None);
        assert!(held.squares[0].alpha > d.squares[0].alpha);
        // The head over the shaft seen from past the head's base; the shaft over the head's base
        // seen from short of it, even from the tip's side of the plane (the last square drawn:
        // a side of the head has the tip twice, one of the shaft four corners).
        let from = |x: f64| {
            let camera =
                Camera { target: Vec3::new(x, 10.0, 2.0), distance: 50.0, azimuth: 0.0, elevation: 0.0, fov: 30.0 };
            let d = build(&scene, &camera, Guides::default(), true, Some(&plane), None);
            let last = *d.squares.last().unwrap();
            last.corners[2] == last.corners[3]
        };
        assert!(from(30.0), "the head last, seen from past it");
        assert!(!from(4.0), "the shaft last, seen from short of the head's base");
        // A normal of zero: nothing.
        assert!(CutPlane { normal: Vec3::ZERO, ..plane }.handles().is_none());
    }

    #[test]
    fn a_grid_of_blocks_is_drawn_on_the_sides_of_its_box() {
        let scene = Scene::new(Vec3::ZERO);
        let camera =
            Camera::fit(crate::camera::View::Iso, &BBox { min: Vec3::ZERO, max: Vec3::new(10.0, 10.0, 10.0) }, 1.6);
        let b = BlockPreview {
            cells: [vec![0.0, 5.0, 10.0], vec![0.0, 10.0], vec![0.0, 10.0]],
            stations: [vec![0.0, 10.0], vec![0.0, 10.0], vec![0.0, 10.0]],
        };
        let d = build(&scene, &camera, Guides::default(), true, None, Some(&b));
        // Per axis and position: 4 lines; x has 3 cells' lines and 2 stations, y and z 2 + 2.
        assert_eq!(d.lines.len(), 4 * (3 + 2) + 4 * (2 + 2) * 2);
        assert!(d.lines.iter().all(|l| l.a.iter().chain(&l.b).all(|v| (0.0..=10.0).contains(v))));
        // Many cells: thinned to PREVIEW_LINES along an axis.
        let many: Vec<f64> = (0..=1000).map(|k| f64::from(k) / 100.0).collect();
        let b = BlockPreview { cells: [many, vec![], vec![]], ..b };
        let d = build(&scene, &camera, Guides::default(), true, None, Some(&b));
        assert!(d.lines.len() <= 4 * (PREVIEW_LINES + 1 + 2) + 4 * 2 * 2, "{}", d.lines.len());
        // A grid without a box: nothing.
        let flat = BlockPreview { stations: [vec![0.0], vec![0.0, 1.0], vec![0.0, 1.0]], ..BlockPreview::default() };
        assert!(build(&scene, &camera, Guides::default(), true, None, Some(&flat)).lines.is_empty());
    }
}
