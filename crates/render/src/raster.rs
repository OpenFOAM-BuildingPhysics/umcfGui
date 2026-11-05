//! The CPU rasteriser: a scene into an RGBA image, flat-shaded with a light at the eye, each
//! patch or surface region in its own colour, outlines where they meet, and optionally the
//! edges of the faces (the mesh's cells at the boundary).

use umcf_geom::Vec3;

use crate::camera::Camera;
use crate::{Object, Scene};

/// The largest image side drawn.
pub const MAX_SIDE: u32 = 8192;
/// The most pixels drawn (4096 × 4096): a larger image is drawn this size, keeping its
/// width and taking height off. Drawing takes 16 bytes per pixel.
pub const MAX_PIXELS: u32 = 4096 * 4096;

/// How classes get their colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColourBy {
    /// Each patch or surface region its own colour.
    #[default]
    Class,
    /// Each object (mesh region, surface) its own colour.
    Object,
}

/// How a class (a patch, a surface region) is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Filled, shaded faces.
    #[default]
    Surface,
    /// The faces' edges only.
    Wireframe,
    /// The faces' points only.
    Points,
    /// Not drawn, and not seen by picking.
    Hidden,
}

impl Mode {
    pub const ALL: [(&'static str, Mode); 4] = [
        ("surface", Mode::Surface),
        ("wireframe", Mode::Wireframe),
        ("points", Mode::Points),
        ("hidden", Mode::Hidden),
    ];

    pub fn parse(name: &str) -> Option<Mode> {
        Mode::ALL.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
    }

    pub fn name(self) -> &'static str {
        Mode::ALL.iter().find(|(_, m)| *m == self).map_or("surface", |(n, _)| n)
    }
}

/// How a class is drawn: its mode, and how opaque (1, the default: opaque; 0: invisible but
/// still seen by picking).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub mode: Mode,
    pub opacity: f32,
}

impl Default for Display {
    fn default() -> Self {
        Display { mode: Mode::Surface, opacity: 1.0 }
    }
}

impl Display {
    /// Drawn opaque, as a surface: what fills the depth buffer and is seen in front.
    pub fn solid(&self) -> bool {
        self.mode == Mode::Surface && self.opacity >= 1.0
    }
}

/// What to draw and how.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    pub width: u32,
    pub height: u32,
    /// Draw the edges of the faces the triangles come from.
    pub edges: bool,
    /// Classes drawn in the selection colour: (object index, class index).
    pub selected: Vec<(usize, u32)>,
    /// Mesh faces drawn in the selection colour: (object index, the face label each triangle of
    /// the object comes from, [`crate::Object::origin_index`]).
    pub selected_faces: std::collections::HashSet<(usize, u32)>,
    pub colour_by: ColourBy,
    /// How each class is drawn, by (object index, class index); a class not in it is an opaque
    /// surface.
    pub display: std::collections::HashMap<(usize, u32), Display>,
    /// The background's colours at the top and the bottom (a vertical gradient); `None` for
    /// the default light grey.
    pub background: Option<[[f32; 3]; 2]>,
}

/// An image, rows from the top, 4 bytes (RGBA) per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Style {
    /// How class `class` of object `object` is drawn.
    pub fn display_of(&self, object: usize, class: u32) -> Display {
        self.display.get(&(object, class)).copied().unwrap_or_default()
    }
}

impl Image {
    /// The pixel at (`x`, `y`), if inside.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.rgba.get(i..i + 4)?;
        Some([p.first().copied()?, p.get(1).copied()?, p.get(2).copied()?, p.get(3).copied()?])
    }
}

/// The selection colour.
pub const SELECTED: [f32; 3] = [1.0, 0.55, 0.1];
const TOP: [f32; 3] = [0.94, 0.95, 0.96];
const BOTTOM: [f32; 3] = [0.78, 0.81, 0.85];
/// How much outlines and edges darken.
const OUTLINE: f32 = 0.55;
const EDGE: f32 = 0.6;

/// Colour number `i` of a palette whose neighbours differ (golden-ratio hues).
pub fn palette(i: usize) -> [f32; 3] {
    let h = (i as f64 * 0.618_033_988_749_895 + 0.08).fract();
    hsl(h, 0.5, 0.62)
}

fn hsl(h: f64, s: f64, l: f64) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [(r + m) as f32, (g + m) as f32, (b + m) as f32]
}

/// A vertex on the screen: pixel x, y (from the top left) and 1/depth.
#[derive(Debug, Clone, Copy)]
struct Screen {
    x: f64,
    y: f64,
    inv_z: f64,
}

/// The buffers being drawn into.
struct Target {
    w: usize,
    /// The pixels drawn: columns `x.0..=x.1`, rows `y.0..=y.1`.
    x: (usize, usize),
    y: (usize, usize),
    depth: Vec<f32>,
    id: Vec<u32>,
    colour: Vec<[u8; 3]>,
    /// When there: the ids of the blended triangles' pixels, as they are drawn.
    hits: Option<std::collections::HashSet<u32>>,
}

fn bytes(c: [f32; 3]) -> [u8; 3] {
    c.map(|ch| (ch * 255.0).round().clamp(0.0, 255.0) as u8)
}

const NONE: u32 = u32::MAX;

impl Target {
    /// Fills triangle `v` with `colour` where it is nearer than what is there; darkens the
    /// pixels within half a pixel of the edges set in `edges` (bit k: edge from v[k] to v[k+1]).
    /// A `front` triangle wins over one in the same place (a selected patch over the patch of
    /// another region it lies on).
    /// With `alpha` below 1 it is blended over what is there, behind nothing nearer, and leaves
    /// the depth and the ids as they were.
    #[allow(clippy::too_many_arguments)]
    fn triangle(&mut self, v: &[Screen; 3], colour: [f32; 3], id: u32, edges: u8, front: bool, alpha: f32) {
        let [a, b, c] = *v;
        let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if !area.is_finite() || area.abs() <= 1e-12 {
            return;
        }
        let x0 = a.x.min(b.x).min(c.x).floor().max(self.x.0 as f64);
        let x1 = a.x.max(b.x).max(c.x).ceil().min(self.x.1 as f64);
        let y0 = a.y.min(b.y).min(c.y).floor().max(self.y.0 as f64);
        let y1 = a.y.max(b.y).max(c.y).ceil().min(self.y.1 as f64);
        if x0.is_nan() || y0.is_nan() || x1 < x0 || y1 < y0 {
            return;
        }
        let len = |p: Screen, q: Screen| ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
        // Pixels from each edge per unit of the barycentric opposite it.
        let scale = [area.abs() / len(b, c), area.abs() / len(c, a), area.abs() / len(a, b)];
        let edge = |p: Screen, q: Screen, x: f64, y: f64| (q.x - p.x) * (y - p.y) - (q.y - p.y) * (x - p.x);
        for py in (y0 as usize)..=(y1 as usize) {
            let y = py as f64 + 0.5;
            for px in (x0 as usize)..=(x1 as usize) {
                let x = px as f64 + 0.5;
                let w0 = edge(b, c, x, y) / area;
                let w1 = edge(c, a, x, y) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let mut z = (w0 * a.inv_z + w1 * b.inv_z + w2 * c.inv_z) as f32;
                if front {
                    z *= 1.0 + 1e-5;
                }
                let i = py * self.w + px;
                let Some(d) = self.depth.get_mut(i) else { continue };
                if z <= *d {
                    continue;
                }
                // Edge k (v[k] → v[k+1]) is opposite vertex k+2.
                let near_edge = [(0b001, w2 * scale[2]), (0b010, w0 * scale[0]), (0b100, w1 * scale[1])]
                    .iter()
                    .any(|(bit, dist)| edges & bit != 0 && *dist < 0.6);
                if alpha < 1.0 {
                    let k = if near_edge { EDGE } else { 1.0 };
                    self.blend(i, colour.map(|ch| ch * k), alpha);
                    if let Some(h) = &mut self.hits {
                        h.insert(id);
                    }
                    continue;
                }
                *d = z;
                let k = if near_edge { EDGE } else { 1.0 };
                if let Some(col) = self.colour.get_mut(i) {
                    *col = bytes(colour.map(|ch| ch * k));
                }
                if let Some(slot) = self.id.get_mut(i) {
                    *slot = id;
                }
            }
        }
    }
}

impl Target {
    /// Blends `colour` over pixel `i`.
    fn blend(&mut self, i: usize, colour: [f32; 3], alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        if let Some(col) = self.colour.get_mut(i) {
            let mixed = [0, 1, 2].map(|k| {
                let under = f32::from(col.get(k).copied().unwrap_or(0)) / 255.0;
                colour.get(k).copied().unwrap_or(0.0) * a + under * (1.0 - a)
            });
            *col = bytes(mixed);
        }
    }

    /// Whether something at `z` (1/depth) is in front of what pixel `i` holds; a little in front
    /// counts, so an edge or a point on a surface is seen on it.
    fn in_front(&self, i: usize, z: f64) -> bool {
        self.depth.get(i).is_some_and(|d| (z as f32) * (1.0 + 1e-4) > *d)
    }

    fn inside(&self, x: f64, y: f64) -> Option<usize> {
        let (px, py) = (x.floor(), y.floor());
        let ok = px >= self.x.0 as f64 && px <= self.x.1 as f64 && py >= self.y.0 as f64 && py <= self.y.1 as f64;
        ok.then(|| py as usize * self.w + px as usize)
    }

    /// A line one pixel wide from `a` to `b`, blended with `alpha`, behind nothing nearer.
    fn line(&mut self, a: Screen, b: Screen, colour: [f32; 3], alpha: f32) {
        // Only the part over the pixels drawn (Liang–Barsky), 1/depth cut with it.
        let (lo, hi) = ((self.x.0 as f64, self.y.0 as f64), (self.x.1 as f64 + 1.0, self.y.1 as f64 + 1.0));
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (mut s0, mut s1) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, a.x - lo.0), (dx, hi.0 - a.x), (-dy, a.y - lo.1), (dy, hi.1 - a.y)] {
            if p == 0.0 {
                if q < 0.0 {
                    return;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    s0 = s0.max(r);
                } else {
                    s1 = s1.min(r);
                }
            }
        }
        if s0.is_nan() || s1.is_nan() || s0 > s1 {
            return;
        }
        let at = |s: f64| Screen { x: a.x + dx * s, y: a.y + dy * s, inv_z: a.inv_z + (b.inv_z - a.inv_z) * s };
        let (a, b) = (at(s0), at(s1));
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let steps = dx.abs().max(dy.abs()).ceil();
        // Within the image now: at most its diagonal.
        if !steps.is_finite() || steps > 2e4 {
            return;
        }
        let n = steps.max(1.0) as usize;
        for k in 0..=n {
            let s = k as f64 / n as f64;
            let (x, y) = (a.x + dx * s, a.y + dy * s);
            // 1/depth is linear on the screen.
            let z = a.inv_z + (b.inv_z - a.inv_z) * s;
            if let Some(i) = self.inside(x, y)
                && self.in_front(i, z)
            {
                self.blend(i, colour, alpha);
            }
        }
    }

    /// A square point `size` pixels wide at `p`, blended with `alpha`, behind nothing nearer.
    fn dot(&mut self, p: Screen, size: f64, colour: [f32; 3], alpha: f32) {
        let r = size * 0.5;
        if !(p.x.is_finite() && p.y.is_finite()) {
            return;
        }
        let (x0, y0) = ((p.x - r).round(), (p.y - r).round());
        let n = size.round().max(1.0) as usize;
        for j in 0..n {
            for i in 0..n {
                if let Some(at) = self.inside(x0 + i as f64 + 0.5, y0 + j as f64 + 0.5)
                    && self.in_front(at, p.inv_z)
                {
                    self.blend(at, colour, alpha);
                }
            }
        }
    }
}

/// A transparent triangle: its points in view coordinates, colour, opacity, depth and the
/// edges drawn on it.
type Glass = ([[f64; 3]; 3], [f32; 3], f32, f64, u8);

/// The camera as the rasteriser uses it: points into view coordinates (right, up, into the
/// image, relative to the eye) and those onto the image.
struct Lens {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    focal: f64,
    near: f64,
    w: f64,
    h: f64,
}

impl Lens {
    fn new(scene: &Scene, camera: &Camera, w: usize, h: usize) -> Lens {
        let f = camera.frame();
        Lens {
            eye: f.eye - scene.origin,
            right: f.right,
            up: f.up,
            forward: f.forward,
            focal: h as f64 * 0.5 / f.tan_half,
            near: (camera.distance * 1e-4).max(1e-9),
            w: w as f64,
            h: h as f64,
        }
    }

    /// `obj`'s points in view coordinates.
    fn view(&self, obj: &Object) -> Vec<[f64; 3]> {
        obj.points.iter().map(|p| self.point(Vec3::from_f32(*p))).collect()
    }

    /// The segment `a`–`b` (view coordinates) on the screen, cut at the near plane; `None` when
    /// it is all behind it.
    fn segment(&self, a: [f64; 3], b: [f64; 3]) -> Option<(Screen, Screen)> {
        let near = self.near;
        let cut = |p: [f64; 3], q: [f64; 3]| {
            let s = (near - p[2]) / (q[2] - p[2]);
            [p[0] + s * (q[0] - p[0]), p[1] + s * (q[1] - p[1]), near]
        };
        match (a[2] >= near, b[2] >= near) {
            (true, true) => Some((self.project(a), self.project(b))),
            (true, false) => Some((self.project(a), self.project(cut(a, b)))),
            (false, true) => Some((self.project(cut(b, a)), self.project(b))),
            (false, false) => None,
        }
    }

    /// A point relative to the scene's origin, in view coordinates.
    fn point(&self, p: Vec3) -> [f64; 3] {
        let d = p - self.eye;
        [d.dot(self.right), d.dot(self.up), d.dot(self.forward)]
    }

    fn project(&self, v: [f64; 3]) -> Screen {
        Screen {
            x: self.w * 0.5 + v[0] / v[2] * self.focal,
            y: self.h * 0.5 - v[1] / v[2] * self.focal,
            inv_z: 1.0 / v[2],
        }
    }

    /// Fills the triangle `verts` (view coordinates) into `t`, clipped against the near plane
    /// (the pieces of a clipped one get no edges).
    #[allow(clippy::too_many_arguments)]
    fn fill(
        &self,
        t: &mut Target,
        verts: [[f64; 3]; 3],
        colour: [f32; 3],
        id: u32,
        edges: u8,
        front: bool,
        alpha: f32,
    ) {
        let near = self.near;
        if verts.iter().all(|v| v[2] >= near) {
            t.triangle(&verts.map(|v| self.project(v)), colour, id, edges, front, alpha);
            return;
        }
        let mut poly: Vec<[f64; 3]> = Vec::with_capacity(4);
        for i in 0..3 {
            let (Some(p), Some(q)) = (verts.get(i), verts.get((i + 1) % 3)) else { continue };
            if p[2] >= near {
                poly.push(*p);
            }
            if (p[2] >= near) != (q[2] >= near) {
                let s = (near - p[2]) / (q[2] - p[2]);
                poly.push([p[0] + s * (q[0] - p[0]), p[1] + s * (q[1] - p[1]), near]);
            }
        }
        for i in 1..poly.len().saturating_sub(1) {
            if let (Some(a), Some(b), Some(c)) = (poly.first(), poly.get(i), poly.get(i + 1)) {
                t.triangle(&[self.project(*a), self.project(*b), self.project(*c)], colour, id, 0, front, alpha);
            }
        }
    }
}

/// A rectangle of an image, in pixels from its top left corner: any two opposite corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub a: [f64; 2],
    pub b: [f64; 2],
}

impl Rect {
    /// The pixels whose middle is inside, as column and row ranges (inclusive) within a
    /// `w` × `h` image; `None` when there are none.
    fn pixels(&self, w: usize, h: usize) -> Option<((usize, usize), (usize, usize))> {
        let range = |a: f64, b: f64, n: usize| {
            let (lo, hi) = (a.min(b), a.max(b));
            // Pixel i's middle is i + 0.5.
            let first = (lo - 0.5).ceil().max(0.0);
            let last = (hi - 0.5).floor().min(n as f64 - 1.0);
            (lo.is_finite() && hi.is_finite() && first <= last).then_some((first as usize, last as usize))
        };
        Some((range(self.a[0], self.b[0], w)?, range(self.a[1], self.b[1], h)?))
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let inside = |v: f64, a: f64, b: f64| a.min(b) <= v && v <= a.max(b);
        inside(x, self.a[0], self.b[0]) && inside(y, self.a[1], self.b[1])
    }
}

/// What `rect` of the image [`draw`] makes of `scene` with `camera` and `style` covers, as
/// (object index, triangle index) in order, each once. Of the style only the size and the
/// selection count: selected classes and faces are in front where they lie on others, as drawn.
/// Only the classes `keep` takes, (object index, class index), and not hidden by the style, are
/// looked at: the others neither count nor hide anything. Only opaque surfaces hide what is
/// behind them: transparent ones, wireframes and points are seen where nothing opaque is in
/// front of them.
///
/// - `through` false: the triangles seen in it, in front of everything else, at the middle of
///   one of its pixels or more (a triangle smaller than a pixel may be missed).
/// - `through` true: the first triangle of each face (mesh face, STL triangle) whose middle (its
///   area-weighted centre) is inside it, on the image and in front of the camera, seen or
///   hidden; a rectangle narrower than a pixel still takes the middles inside it.
pub fn covered(
    scene: &Scene,
    camera: &Camera,
    style: &Style,
    rect: Rect,
    through: bool,
    keep: &dyn Fn(usize, u32) -> bool,
) -> Vec<(usize, u32)> {
    let (w, h) = size(style.width, style.height);
    let (wu, hu) = (w as usize, h as usize);
    let lens = Lens::new(scene, camera, wu, hu);
    let mut out = Vec::new();
    if through {
        if !rect.a.iter().chain(&rect.b).all(|c| c.is_finite()) {
            return out;
        }
        // In the rectangle and on the image (a middle on its far edge is off it).
        let on_image =
            |x: f64, y: f64| rect.contains(x, y) && (0.0..wu as f64).contains(&x) && (0.0..hu as f64).contains(&y);
        for (k, obj) in scene.objects.iter().enumerate() {
            let view = lens.view(obj);
            // The triangles of one face follow each other.
            let mut n = 0;
            while let Some(face) = obj.origin_index.get(n) {
                let mut end = n + 1;
                while obj.origin_index.get(end) == Some(face) {
                    end += 1;
                }
                let class = obj.class.get(n).copied().unwrap_or(0);
                if !keep(k, class) || style.display_of(k, class).mode == Mode::Hidden {
                    n = end;
                    continue;
                }
                let (mut sum, mut area) = ([0.0f64; 3], 0.0f64);
                let (mut first, mut count) = ([0.0f64; 3], 0usize);
                for tri in obj.triangles.get(n..end).unwrap_or(&[]) {
                    let [Some(a), Some(b), Some(c)] = tri.map(|i| view.get(i as usize).copied()) else { continue };
                    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let x =
                        [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                    let s = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt();
                    for i in 0..3 {
                        let m = (a[i] + b[i] + c[i]) / 3.0;
                        sum[i] += s * m;
                        if count == 0 {
                            first[i] = a[i];
                        }
                    }
                    area += s;
                    count += 1;
                }
                // A face of no area: its first point.
                let centre = if area > 0.0 { sum.map(|v| v / area) } else { first };
                if count > 0 && centre[2] >= lens.near {
                    let p = lens.project(centre);
                    if let (true, Ok(t)) = (on_image(p.x, p.y), u32::try_from(n)) {
                        out.push((k, t));
                    }
                }
                n = end;
            }
        }
        return out;
    }
    let Some((xs, ys)) = rect.pixels(wu, hu) else { return out };
    let mut t = Target {
        w: wu,
        x: xs,
        y: ys,
        depth: vec![0.0; wu * hu],
        id: vec![NONE; wu * hu],
        colour: Vec::new(),
        hits: Some(std::collections::HashSet::new()),
    };
    // Each triangle's id: its object's first id plus its index.
    let mut first: Vec<u32> = Vec::with_capacity(scene.objects.len());
    let mut see_through: Vec<([[f64; 3]; 3], u32)> = Vec::new();
    let mut next: u32 = 0;
    for (k, obj) in scene.objects.iter().enumerate() {
        first.push(next);
        let view = lens.view(obj);
        let selected: Vec<bool> = (0..obj.classes.len())
            .map(|c| style.selected.contains(&(k, u32::try_from(c).unwrap_or(u32::MAX))))
            .collect();
        let mut picked: Vec<u32> = style.selected_faces.iter().filter(|(o, _)| *o == k).map(|(_, f)| *f).collect();
        picked.sort_unstable();
        for (n, tri) in obj.triangles.iter().enumerate() {
            let [Some(a), Some(b), Some(c)] = tri.map(|i| view.get(i as usize).copied()) else { continue };
            let Some(id) = u32::try_from(n).ok().and_then(|n| next.checked_add(n)).filter(|id| *id != NONE) else {
                break;
            };
            // In front as `draw` puts it.
            let class = obj.class.get(n).copied().unwrap_or(0);
            if !keep(k, class) {
                continue;
            }
            let front = selected.get(class as usize).copied().unwrap_or(false)
                || (!picked.is_empty() && obj.origin_index.get(n).is_some_and(|f| picked.binary_search(f).is_ok()));
            // Only opaque surfaces hide what is behind them; the others are seen where nothing
            // opaque is in front of them.
            let d = style.display_of(k, class);
            match d.mode {
                Mode::Hidden => {}
                _ if d.solid() => lens.fill(&mut t, [a, b, c], [0.0; 3], id, 0, front, 1.0),
                _ => see_through.push(([a, b, c], id)),
            }
        }
        next = next.saturating_add(u32::try_from(obj.triangles.len()).unwrap_or(u32::MAX));
    }
    for (verts, id) in see_through {
        lens.fill(&mut t, verts, [0.0; 3], id, 0, false, 0.0);
    }
    // Each triangle seen once, as the pixels are read: as many as the rectangle has pixels at
    // most, whatever the size of the scene.
    let mut seen: std::collections::HashSet<u32> = t.hits.take().unwrap_or_default();
    for y in ys.0..=ys.1 {
        seen.extend(t.id.get(y * wu + xs.0..=y * wu + xs.1).unwrap_or(&[]).iter().filter(|i| **i != NONE));
    }
    let mut ids: Vec<u32> = seen.into_iter().collect();
    ids.sort_unstable();
    for id in ids {
        // The last object starting at or before it.
        let k = first.partition_point(|f| *f <= id).saturating_sub(1);
        if let Some(f) = first.get(k) {
            out.push((k, id - f));
        }
    }
    out
}

/// The size an image of `width` × `height` is drawn at: each side from 1 to [`MAX_SIDE`],
/// and at most [`MAX_PIXELS`] (the width kept, the height cut).
pub fn size(width: u32, height: u32) -> (u32, u32) {
    let w = width.clamp(1, MAX_SIDE);
    (w, height.clamp(1, MAX_SIDE).min(MAX_PIXELS / w))
}

/// Draws `scene` as `camera` sees it.
pub fn draw(scene: &Scene, camera: &Camera, style: &Style) -> Image {
    let (w, h) = size(style.width, style.height);
    let (wu, hu) = (w as usize, h as usize);
    let mut t = Target {
        w: wu,
        x: (0, wu - 1),
        y: (0, hu - 1),
        depth: vec![0.0; wu * hu],
        id: vec![NONE; wu * hu],
        colour: (0..hu)
            .flat_map(|y| {
                let s = y as f32 / (hu.max(2) - 1) as f32;
                let [top, bottom] = style.background.unwrap_or([TOP, BOTTOM]);
                std::iter::repeat_n(bytes([0, 1, 2].map(|k| top[k] * (1.0 - s) + bottom[k] * s)), wu)
            })
            .collect(),
        hits: None,
    };
    let lens = Lens::new(scene, camera, wu, hu);
    let mut next_id: u32 = 0;
    // What is drawn after the opaque surfaces: (view points, colour, opacity[, depth]).
    let mut glass: Vec<Glass> = Vec::new();
    let mut lines: Vec<([[f64; 3]; 2], [f32; 3], f32)> = Vec::new();
    let mut dots: Vec<([f64; 3], [f32; 3], f32)> = Vec::new();
    for (k, obj) in scene.objects.iter().enumerate() {
        let view = lens.view(obj);
        let selected: Vec<bool> = (0..obj.classes.len())
            .map(|c| style.selected.contains(&(k, u32::try_from(c).unwrap_or(u32::MAX))))
            .collect();
        let colours: Vec<[f32; 3]> = (0..obj.classes.len())
            .map(|c| {
                if selected.get(c).copied().unwrap_or(false) {
                    SELECTED
                } else {
                    match style.colour_by {
                        ColourBy::Class => palette(c),
                        ColourBy::Object => palette(k),
                    }
                }
            })
            .collect();
        // The selected faces of this object, by label, in order.
        let mut picked: Vec<u32> = style.selected_faces.iter().filter(|(o, _)| *o == k).map(|(_, f)| *f).collect();
        picked.sort_unstable();
        let base = next_id;
        next_id = next_id.saturating_add(u32::try_from(obj.classes.len()).unwrap_or(u32::MAX));
        for (n, tri) in obj.triangles.iter().enumerate() {
            let p = tri.map(|i| view.get(i as usize).copied());
            let [Some(a), Some(b), Some(c)] = p else { continue };
            let class = obj.class.get(n).copied().unwrap_or(0);
            let face_selected =
                !picked.is_empty() && obj.origin_index.get(n).is_some_and(|f| picked.binary_search(f).is_ok());
            let Some(colour) = (if face_selected { Some(&SELECTED) } else { colours.get(class as usize) }) else {
                continue;
            };
            // Lambert with the light at the eye, both sides lit alike.
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let nrm = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let g = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
            let dot = nrm[0] * g[0] + nrm[1] * g[1] + nrm[2] * g[2];
            let mag = (nrm.iter().map(|x| x * x).sum::<f64>() * g.iter().map(|x| x * x).sum::<f64>()).sqrt();
            let cos = if mag > 0.0 { (dot / mag).abs() } else { 1.0 };
            let shade = (0.3 + 0.7 * cos) as f32;
            let colour = colour.map(|ch| ch * shade);
            let id = base.saturating_add(class);
            let front = face_selected || selected.get(class as usize).copied().unwrap_or(false);
            let face_edges = obj.edges.get(n).copied().unwrap_or(0);
            let d = style.display_of(k, class);
            match d.mode {
                Mode::Hidden => {}
                Mode::Surface if d.opacity >= 1.0 => {
                    let edges = if style.edges { face_edges } else { 0 };
                    lens.fill(&mut t, [a, b, c], colour, id, edges, front, 1.0);
                }
                Mode::Surface => {
                    let depth = (a[2] + b[2] + c[2]) / 3.0;
                    let edges = if style.edges { face_edges } else { 0 };
                    glass.push(([a, b, c], colour, d.opacity, depth, edges));
                }
                // Drawn from the object's edges and points, below.
                Mode::Wireframe | Mode::Points => {}
            }
        }
        // Wireframes and points, from the object's edges and points (found once per object),
        // in the class's colour or the selection's.
        let shows = |m: Mode| {
            (0..obj.classes.len()).any(|c| {
                let d = style.display_of(k, u32::try_from(c).unwrap_or(u32::MAX));
                d.mode == m && d.opacity > 0.0
            })
        };
        let (wire, points) = (shows(Mode::Wireframe), shows(Mode::Points));
        if wire || points {
            let w = obj.wire();
            // The colour and display of what triangle `t` belongs to.
            let of = |t: u32| -> Option<([f32; 3], Display)> {
                let class = *obj.class.get(t as usize)?;
                let face_selected = !picked.is_empty()
                    && obj.origin_index.get(t as usize).is_some_and(|f| picked.binary_search(f).is_ok());
                let colour = if face_selected { SELECTED } else { *colours.get(class as usize)? };
                Some((colour, style.display_of(k, class)))
            };
            let at = |p: u32| view.get(p as usize).copied();
            for [a, b, tri] in w.edges.iter().filter(|_| wire) {
                if let (Some((colour, d)), Some(a), Some(b)) = (of(*tri), at(*a), at(*b))
                    && d.mode == Mode::Wireframe
                    && d.opacity > 0.0
                {
                    lines.push(([a, b], colour, d.opacity));
                }
            }
            for [p, tri] in w.points.iter().filter(|_| points) {
                if let (Some((colour, d)), Some(p)) = (of(*tri), at(*p))
                    && d.mode == Mode::Points
                    && d.opacity > 0.0
                {
                    dots.push((p, colour, d.opacity));
                }
            }
        }
    }
    // Edges and points over the opaque surfaces, behind nothing nearer.
    for ([a, b], colour, alpha) in lines {
        if let Some((p, q)) = lens.segment(a, b) {
            t.line(p, q, colour.map(|ch| ch * 0.85), alpha);
        }
    }
    let dot_size = ((hu as f64) / 400.0).clamp(2.0, 4.0);
    for (p, colour, alpha) in dots {
        if p[2] >= lens.near {
            t.dot(lens.project(p), dot_size, colour.map(|ch| ch * 0.85), alpha);
        }
    }
    // Transparent surfaces last, the farthest first.
    glass.sort_by(|a, b| b.3.total_cmp(&a.3));
    for (verts, colour, alpha, _, edges) in glass {
        lens.fill(&mut t, verts, colour, NONE, edges, false, alpha);
    }
    // Outlines where one class meets another or the background.
    let mut rgba = Vec::with_capacity(wu * hu * 4);
    for y in 0..hu {
        for x in 0..wu {
            let i = y * wu + x;
            let id = t.id.get(i).copied().unwrap_or(NONE);
            let differs = |j: Option<usize>| j.and_then(|j| t.id.get(j)).is_some_and(|o| *o != id);
            let outline = id != NONE
                && (differs((x + 1 < wu).then_some(i + 1))
                    || differs(x.checked_sub(1).map(|_| i - 1))
                    || differs((y + 1 < hu).then_some(i + wu))
                    || differs(y.checked_sub(1).map(|_| i - wu)));
            let c = t.colour.get(i).copied().unwrap_or([255; 3]);
            let k = if outline { OUTLINE } else { 1.0 };
            rgba.extend(c.map(|ch| (f32::from(ch) * k).round().clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    Image { width: w, height: h, rgba }
}
