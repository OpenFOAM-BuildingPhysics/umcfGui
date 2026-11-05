//! Which points lie near a surface: within a distance of one of its triangles.

use crate::Vec3;

/// The most grid cells, and the most (cell, triangle) entries: a grid that would need more is
/// made coarser (the answer is the same, only slower to find).
const MAX_CELLS: usize = 1 << 21;
const MAX_ENTRIES: usize = 1 << 25;

/// For each of `points`, whether it is within `distance` of one of `triangles`. Triangles and
/// points that are not finite are never near; a negative or non-finite distance finds nothing.
pub fn near(triangles: &[[Vec3; 3]], points: &[Vec3], distance: f64) -> Vec<bool> {
    let mut out = vec![false; points.len()];
    let tris: Vec<&[Vec3; 3]> = triangles.iter().filter(|t| t.iter().all(|p| p.is_finite())).collect();
    if tris.is_empty() || !distance.is_finite() || distance < 0.0 {
        return out;
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in tris.iter().flat_map(|t| t.iter()) {
        for (k, x) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[k] = lo[k].min(x);
            hi[k] = hi[k].max(x);
        }
    }
    for k in 0..3 {
        lo[k] -= distance;
        hi[k] += distance;
    }
    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    // Cells at least `distance` wide, so a point's neighbours are the 27 cells around it.
    let mut h = distance.max(extent / (MAX_CELLS as f64).cbrt()).max(f64::MIN_POSITIVE);
    if !h.is_finite() {
        return out;
    }
    let grid = loop {
        if let Some(g) = Grid::build(&tris, lo, hi, h) {
            break g;
        }
        h *= 2.0;
        if !h.is_finite() {
            return out;
        }
    };
    let d2 = distance * distance;
    for (p, hit) in points.iter().zip(out.iter_mut()) {
        if !p.is_finite() {
            continue;
        }
        let c = [p.x, p.y, p.z];
        if (0..3).any(|k| c[k] < lo[k] || c[k] > hi[k]) {
            continue;
        }
        let cell = grid.cell(c);
        'cells: for i in cell[0].saturating_sub(1)..=(cell[0] + 1).min(grid.n[0] - 1) {
            for j in cell[1].saturating_sub(1)..=(cell[1] + 1).min(grid.n[1] - 1) {
                for k in cell[2].saturating_sub(1)..=(cell[2] + 1).min(grid.n[2] - 1) {
                    for &t in grid.triangles(i, j, k) {
                        if let Some(t) = tris.get(t as usize)
                            && closest_squared(*p, t) <= d2
                        {
                            *hit = true;
                            break 'cells;
                        }
                    }
                }
            }
        }
    }
    out
}

/// Triangles by grid cell (each in every cell its box touches), as offsets into one list.
struct Grid {
    lo: [f64; 3],
    h: f64,
    n: [usize; 3],
    starts: Vec<u32>,
    entries: Vec<u32>,
}

impl Grid {
    /// `None` when it would hold more than [`MAX_CELLS`] cells or [`MAX_ENTRIES`] entries.
    fn build(tris: &[&[Vec3; 3]], lo: [f64; 3], hi: [f64; 3], h: f64) -> Option<Grid> {
        let n = [0, 1, 2].map(|k| (((hi[k] - lo[k]) / h).floor() as usize).saturating_add(1));
        let cells = n[0].checked_mul(n[1])?.checked_mul(n[2])?;
        if cells > MAX_CELLS || tris.len() > u32::MAX as usize {
            return None;
        }
        let mut g = Grid { lo, h, n, starts: vec![0; cells + 1], entries: Vec::new() };
        let span = |t: &[Vec3; 3], g: &Grid| {
            let a = g.cell([0, 1, 2].map(|k| t.iter().map(|p| [p.x, p.y, p.z][k]).fold(f64::INFINITY, f64::min)));
            let b = g.cell([0, 1, 2].map(|k| t.iter().map(|p| [p.x, p.y, p.z][k]).fold(f64::NEG_INFINITY, f64::max)));
            (a, b)
        };
        let mut total = 0usize;
        for t in tris {
            let (a, b) = span(t, &g);
            total = total.saturating_add((0..3).map(|k| b[k] - a[k] + 1).product::<usize>());
            if total > MAX_ENTRIES {
                return None;
            }
        }
        let mut counts = vec![0u32; cells];
        for t in tris {
            let (a, b) = span(t, &g);
            for i in a[0]..=b[0] {
                for j in a[1]..=b[1] {
                    for k in a[2]..=b[2] {
                        if let Some(c) = counts.get_mut(g.index(i, j, k)) {
                            *c += 1;
                        }
                    }
                }
            }
        }
        let mut acc = 0u32;
        for (s, c) in g.starts.iter_mut().zip(counts.iter().chain(std::iter::once(&0))) {
            *s = acc;
            acc += c;
        }
        let mut fill: Vec<u32> = g.starts.clone();
        g.entries = vec![0; acc as usize];
        for (ti, t) in tris.iter().enumerate() {
            let (a, b) = span(t, &g);
            for i in a[0]..=b[0] {
                for j in a[1]..=b[1] {
                    for k in a[2]..=b[2] {
                        let c = g.index(i, j, k);
                        if let Some(f) = fill.get_mut(c) {
                            if let Some(e) = g.entries.get_mut(*f as usize) {
                                *e = ti as u32;
                            }
                            *f += 1;
                        }
                    }
                }
            }
        }
        Some(g)
    }

    fn cell(&self, p: [f64; 3]) -> [usize; 3] {
        [0, 1, 2].map(|k| (((p[k] - self.lo[k]) / self.h).floor().max(0.0) as usize).min(self.n[k] - 1))
    }

    fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (i * self.n[1] + j) * self.n[2] + k
    }

    fn triangles(&self, i: usize, j: usize, k: usize) -> &[u32] {
        let c = self.index(i, j, k);
        match (self.starts.get(c), self.starts.get(c + 1)) {
            (Some(&a), Some(&b)) => self.entries.get(a as usize..b as usize).unwrap_or(&[]),
            _ => &[],
        }
    }
}

/// The squared distance from `p` to triangle `t` (Ericson, Real-Time Collision Detection, 5.1.5);
/// a degenerate triangle counts as its edges.
fn closest_squared(p: Vec3, t: &[Vec3; 3]) -> f64 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length_squared();
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length_squared();
    }
    let sum = va + vb + vc;
    if sum.abs() <= f64::MIN_POSITIVE {
        // Degenerate: the nearest of its edges.
        return [(a, b), (b, c), (c, a)].iter().map(|&(u, v)| segment_squared(p, u, v)).fold(f64::INFINITY, f64::min);
    }
    let (v, w) = (vb / sum, vc / sum);
    (p - (a + ab * v + ac * w)).length_squared()
}

fn segment_squared(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = b - a;
    let l = ab.length_squared();
    let t = if l > 0.0 { ((p - a).dot(ab) / l).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn points_within_the_distance_of_a_triangle_are_near() {
        let t = [[v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0), v(0.0, 10.0, 0.0)]];
        let pts = [
            v(1.0, 1.0, 0.4),   // above the inside
            v(1.0, 1.0, 0.6),   // too high
            v(-0.3, -0.3, 0.0), // off the corner: 0.42
            v(6.0, 6.0, 0.0),   // off the long edge: 1.41
            v(5.0, 5.0, 0.3),   // on the long edge, above
            v(f64::NAN, 0.0, 0.0),
        ];
        assert_eq!(near(&t, &pts, 0.5), [true, false, true, false, true, false]);
        assert_eq!(near(&t, &pts, -1.0), [false; 6]);
        assert_eq!(near(&[], &pts, 1.0), [false; 6]);
    }

    #[test]
    fn a_fine_distance_on_a_large_surface_still_finds_points() {
        // A distance tiny next to the surface: the grid is coarser than the distance.
        let t = [
            [v(0.0, 0.0, 0.0), v(1e5, 0.0, 0.0), v(0.0, 1e5, 0.0)],
            [v(0.0, 0.0, 5.0), v(1.0, 0.0, 5.0), v(0.0, 1.0, 5.0)],
        ];
        let pts = [v(5e4, 1.0, 1e-4), v(5e4, 1.0, 2e-3), v(0.2, 0.2, 5.0005)];
        assert_eq!(near(&t, &pts, 1e-3), [true, false, true]);
        // A degenerate triangle counts as its edges.
        let flat = [[v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(2.0, 0.0, 0.0)]];
        assert_eq!(near(&flat, &[v(1.5, 0.1, 0.0), v(1.5, 0.3, 0.0)], 0.2), [true, false]);
    }
}
