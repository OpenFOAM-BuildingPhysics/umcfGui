//! Which points lie near a surface: within a distance of one of its triangles.

use crate::Vec3;

/// Triangles per leaf of the tree.
const LEAF: usize = 8;

/// For each of `points`, whether it is within `distance` of one of `triangles`. Triangles and
/// points that are not finite are never near; a negative or non-finite distance finds nothing.
/// A bounding volume tree over the triangles answers each point in about logarithmic time,
/// whatever the mix of triangle sizes.
pub fn near(triangles: &[[Vec3; 3]], points: &[Vec3], distance: f64) -> Vec<bool> {
    let mut out = vec![false; points.len()];
    let tris: Vec<[Vec3; 3]> = triangles.iter().filter(|t| t.iter().all(|p| p.is_finite())).copied().collect();
    if tris.is_empty() || !distance.is_finite() || distance < 0.0 {
        return out;
    }
    let tree = Tree::build(&tris);
    let d2 = distance * distance;
    let mut stack = Vec::new();
    for (p, hit) in points.iter().zip(out.iter_mut()) {
        if p.is_finite() {
            *hit = tree.any_within(&tris, *p, d2, &mut stack);
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Bounds {
    fn of(t: &[Vec3; 3]) -> Bounds {
        let c = |k: usize| t.iter().map(move |p| [p.x, p.y, p.z][k]);
        Bounds {
            lo: [0, 1, 2].map(|k| c(k).fold(f64::INFINITY, f64::min)),
            hi: [0, 1, 2].map(|k| c(k).fold(f64::NEG_INFINITY, f64::max)),
        }
    }

    fn join(self, o: Bounds) -> Bounds {
        Bounds { lo: [0, 1, 2].map(|k| self.lo[k].min(o.lo[k])), hi: [0, 1, 2].map(|k| self.hi[k].max(o.hi[k])) }
    }

    /// The squared distance from `p` to the box (0 inside).
    fn distance_squared(&self, p: Vec3) -> f64 {
        [p.x, p.y, p.z]
            .iter()
            .enumerate()
            .map(|(k, x)| {
                let d = (self.lo[k] - x).max(x - self.hi[k]).max(0.0);
                d * d
            })
            .sum()
    }
}

/// A node: its box, and either two children or a run of `order`.
#[derive(Debug, Clone, Copy)]
struct Node {
    bounds: Bounds,
    /// Children (`left`, `left + 1`) when `count == 0`; else `order[first..first + count]`.
    first: u32,
    count: u32,
}

struct Tree {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl Tree {
    fn build(tris: &[[Vec3; 3]]) -> Tree {
        let boxes: Vec<Bounds> = tris.iter().map(Bounds::of).collect();
        let centre = |i: u32, k: usize| boxes.get(i as usize).map_or(0.0, |b| (b.lo[k] + b.hi[k]) * 0.5);
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let span = |items: &[u32]| {
            items
                .iter()
                .filter_map(|&i| boxes.get(i as usize).copied())
                .reduce(Bounds::join)
                .unwrap_or(Bounds { lo: [0.0; 3], hi: [0.0; 3] })
        };
        let mut nodes = vec![Node { bounds: span(&order), first: 0, count: order.len() as u32 }];
        // Split nodes until their runs are short, widest axis at the median centre.
        let mut todo = vec![0usize];
        while let Some(n) = todo.pop() {
            let Some(node) = nodes.get(n).copied() else { continue };
            let (first, count) = (node.first as usize, node.count as usize);
            if count <= LEAF {
                continue;
            }
            let Some(items) = order.get_mut(first..first + count) else { continue };
            let b = node.bounds;
            let axis = (0..3).max_by(|&a, &c| (b.hi[a] - b.lo[a]).total_cmp(&(b.hi[c] - b.lo[c]))).unwrap_or(0);
            let half = count / 2;
            items.select_nth_unstable_by(half, |&a, &c| centre(a, axis).total_cmp(&centre(c, axis)));
            let (left, right) = items.split_at(half);
            let left = Node { bounds: span(left), first: first as u32, count: half as u32 };
            let right = Node { bounds: span(right), first: (first + half) as u32, count: (count - half) as u32 };
            let at = nodes.len();
            nodes.push(left);
            nodes.push(right);
            if let Some(node) = nodes.get_mut(n) {
                *node = Node { bounds: b, first: at as u32, count: 0 };
            }
            todo.push(at);
            todo.push(at + 1);
        }
        Tree { nodes, order }
    }

    fn any_within(&self, tris: &[[Vec3; 3]], p: Vec3, d2: f64, stack: &mut Vec<u32>) -> bool {
        stack.clear();
        stack.push(0);
        while let Some(n) = stack.pop() {
            let Some(node) = self.nodes.get(n as usize) else { continue };
            if node.bounds.distance_squared(p) > d2 {
                continue;
            }
            if node.count == 0 {
                stack.push(node.first);
                stack.push(node.first + 1);
                continue;
            }
            let run = self.order.get(node.first as usize..(node.first + node.count) as usize).unwrap_or(&[]);
            if run.iter().filter_map(|&t| tris.get(t as usize)).any(|t| closest_squared(p, t) <= d2) {
                return true;
            }
        }
        false
    }
}

/// The squared distance from `p` to triangle `t` (Ericson, Real-Time Collision Detection, 5.1.5);
/// a degenerate triangle counts as its edges.
fn closest_squared(p: Vec3, t: &[Vec3; 3]) -> f64 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    // Degenerate (two corners the same, or all in a line): the nearest of its edges. The
    // formulas below divide by zero there.
    let scale = ab.length_squared().max(ac.length_squared());
    if ab.cross(ac).length_squared() <= 1e-20 * scale * scale {
        return [(a, b), (b, c), (c, a)].iter().map(|&(u, v)| segment_squared(p, u, v)).fold(f64::INFINITY, f64::min);
    }
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
        // A distance tiny next to the surface.
        let t = [
            [v(0.0, 0.0, 0.0), v(1e5, 0.0, 0.0), v(0.0, 1e5, 0.0)],
            [v(0.0, 0.0, 5.0), v(1.0, 0.0, 5.0), v(0.0, 1.0, 5.0)],
        ];
        let pts = [v(5e4, 1.0, 1e-4), v(5e4, 1.0, 2e-3), v(0.2, 0.2, 5.0005)];
        assert_eq!(near(&t, &pts, 1e-3), [true, false, true]);
        // A degenerate triangle counts as its edges.
        let flat = [[v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(2.0, 0.0, 0.0)]];
        assert_eq!(near(&flat, &[v(1.5, 0.1, 0.0), v(1.5, 0.3, 0.0)], 0.2), [true, false]);
        // Two corners the same, in each order; all three the same.
        let p = [v(0.5, 0.1, 0.0), v(0.5, 0.3, 0.0)];
        for t in [
            [v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0)],
            [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(1.0, 0.0, 0.0)],
            [v(1.0, 0.0, 0.0), v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0)],
        ] {
            assert_eq!(near(&[t], &p, 0.2), [true, false], "{t:?}");
        }
        let dot = [[v(1.0, 1.0, 1.0); 3]];
        assert_eq!(near(&dot, &[v(1.0, 1.0, 1.1), v(1.0, 1.0, 1.3)], 0.2), [true, false]);
        // Distance 0: a corner is on it, a point just off it is not.
        assert_eq!(near(&t, &[v(0.0, 0.0, 0.0), v(0.2, 0.2, 1e-9)], 0.0), [true, false]);
    }

    #[test]
    fn many_small_triangles_beside_a_huge_one_are_quick() {
        // A building of 100,000 small triangles in a 10 m block beside a 2 km terrain triangle:
        // each point looks at a few tree nodes, not at every triangle near it (a grid took minutes).
        let mut t = vec![[v(-1000.0, -1000.0, 0.0), v(1000.0, -1000.0, 0.0), v(0.0, 1000.0, 0.0)]];
        for i in 0..100_000 {
            let (x, y, z) = ((i % 100) as f64 * 0.1, ((i / 100) % 100) as f64 * 0.1, 1.0 + (i / 10_000) as f64);
            t.push([v(x, y, z), v(x + 0.1, y, z), v(x, y + 0.1, z)]);
        }
        let pts: Vec<Vec3> = (0..20_000).map(|i| v((i % 200) as f64 * 0.05, (i / 200) as f64 * 0.1, 0.5)).collect();
        let hits = near(&t, &pts, 1e-3);
        assert!(hits.iter().all(|h| !h));
        assert_eq!(near(&t, &[v(5.0, 5.0, 1e-4), v(0.05, 0.02, 1.0005)], 1e-3), [true, true]);
    }
}
