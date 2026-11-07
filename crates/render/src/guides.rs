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
//! The spacing, the extent, the fade and the colours are ported from SolidCraft's viewport
//! (<https://github.com/tangency-labs/solidcraft>, MIT or Apache-2.0, at commit 9325e3a:
//! `crates/engine/src/view.rs` `grid_step`, `grid_ext`, `GridFade`, and
//! `crates/ui-egui/src/selection.rs` `origin_size`). Those parts:
//! Copyright (c) 2026 SolidCraft contributors, under the MIT licence
//! (`assets/licenses/MIT-SolidCraft.txt`; `ATTRIBUTION.md`).

use umcf_geom::Vec3;

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
pub fn build(scene: &Scene, camera: &Camera, which: Guides, dark: bool) -> Drawn {
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
    out
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
        let d = build(&scene, &camera, Guides::ALL, true);
        assert!(!d.grid.is_empty() && d.squares.len() == 3 && d.lines.len() > 12);
        // The grid on z = 0 (relative to the scene's origin, also 0 here), faded towards its ends.
        assert!(d.grid.iter().all(|l| l.a[2] == 0.0 && l.b[2] == 0.0));
        assert!(d.grid.iter().any(|l| l.alpha < 0.3) && d.grid.iter().any(|l| l.alpha >= 1.0));
        // The origin's xy square from the world's origin: (-100, -200) relative to the scene's.
        assert_eq!(d.squares[0].corners[0], [-100.0, -200.0, 0.0]);
        // Nothing asked, nothing drawn.
        assert_eq!(build(&scene, &camera, Guides::default(), true), Drawn::default());
    }
}
