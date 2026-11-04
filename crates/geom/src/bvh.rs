//! A bounding volume hierarchy over triangles, for picking: which triangle a ray through the
//! cursor hits first, and everything else it passes through ("select other").
//!
//! The hierarchy holds only the triangles' order and boxes; the points and triangles stay with
//! their owner and are passed to every query, so a triangle costs about 20 bytes here. Coordinates are single precision, relative to an origin the owner
//! chooses (the middle of the scene), so far-away projected coordinates keep their precision;
//! the intersection itself is computed in double precision.
//!
//! Built with binned surface-area heuristics, iteratively (no recursion, whatever the input).
//! Triangles with a vertex that is not finite, or a vertex index out of range, are left out.

/// A ray: `origin + t·direction`, `t ≥ 0`. The direction need not be of unit length; distances
/// ([`Hit::t`]) are in its units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

/// A triangle a ray hits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The triangle's index in the list the hierarchy was built from.
    pub triangle: u32,
    /// Where along the ray: `origin + t·direction`.
    pub t: f64,
}

#[derive(Debug, Clone, Copy)]
struct Node {
    lo: [f32; 3],
    hi: [f32; 3],
    /// A leaf's first entry in `order`, or an inner node's first child (the second follows it).
    first: u32,
    /// Triangles in a leaf; 0 for an inner node.
    count: u32,
}

/// The hierarchy. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

/// Most triangles in a leaf.
const LEAF: usize = 4;
/// Bins for the surface-area heuristic.
const BINS: usize = 12;

fn corners(points: &[[f32; 3]], t: &[u32; 3]) -> Option<[[f32; 3]; 3]> {
    let p = |i: u32| points.get(i as usize).copied().filter(|p| p.iter().all(|c| c.is_finite()));
    Some([p(t[0])?, p(t[1])?, p(t[2])?])
}

fn area(lo: &[f32; 3], hi: &[f32; 3]) -> f64 {
    let d = [(hi[0] - lo[0]) as f64, (hi[1] - lo[1]) as f64, (hi[2] - lo[2]) as f64];
    if d.iter().any(|c| *c < 0.0) {
        return 0.0;
    }
    d[0] * d[1] + d[1] * d[2] + d[2] * d[0]
}

#[derive(Clone, Copy)]
struct Bounds {
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Bounds {
    const EMPTY: Bounds = Bounds { lo: [f32::INFINITY; 3], hi: [f32::NEG_INFINITY; 3] };

    fn add(&mut self, p: &[f32; 3]) {
        for ((lo, hi), c) in self.lo.iter_mut().zip(self.hi.iter_mut()).zip(p) {
            *lo = lo.min(*c);
            *hi = hi.max(*c);
        }
    }

    fn join(&mut self, b: &Bounds) {
        self.add(&b.lo);
        self.add(&b.hi);
    }
}

impl Bvh {
    /// Builds the hierarchy over `triangles` (indices into `points`).
    pub fn new(points: &[[f32; 3]], triangles: &[[u32; 3]]) -> Bvh {
        // Each triangle's box and centroid, for the triangles that can be hit.
        let mut boxes: Vec<Bounds> = Vec::new();
        let mut centres: Vec<[f32; 3]> = Vec::new();
        let mut order: Vec<u32> = Vec::new();
        for (i, t) in triangles.iter().enumerate() {
            let (Some(c), Ok(i)) = (corners(points, t), u32::try_from(i)) else { continue };
            let mut b = Bounds::EMPTY;
            c.iter().for_each(|p| b.add(p));
            // Padded by more than the edge tolerance of the triangle test (and a rounding
            // step), so a ray the triangle test accepts on an edge or corner never misses the
            // box.
            let size = (0..3).fold(0.0f32, |m, k| m.max(b.hi[k] - b.lo[k]));
            let pad = size * 1e-7;
            b.lo = b.lo.map(|x| (x - pad).next_down());
            b.hi = b.hi.map(|x| (x + pad).next_up());
            boxes.push(b);
            centres.push([0, 1, 2].map(|k| (b.lo[k] + b.hi[k]) * 0.5));
            order.push(i);
        }
        if order.is_empty() {
            return Bvh::default();
        }
        // `slot[j]` is the entry of `boxes`/`centres` for `order[j]`; both are permuted together.
        let mut slot: Vec<u32> = (0..order.len() as u32).collect();
        let mut nodes = Vec::with_capacity(2 * order.len() / LEAF + 1);
        nodes.push(Node { lo: [0.0; 3], hi: [0.0; 3], first: 0, count: 0 });
        let mut stack: Vec<(usize, usize, usize)> = vec![(0, 0, order.len())];
        while let Some((node, start, end)) = stack.pop() {
            let range = slot.get(start..end).unwrap_or_default();
            let mut b = Bounds::EMPTY;
            let mut cb = Bounds::EMPTY;
            for s in range {
                if let (Some(bx), Some(c)) = (boxes.get(*s as usize), centres.get(*s as usize)) {
                    b.join(bx);
                    cb.add(c);
                }
            }
            let leaf = |nodes: &mut Vec<Node>| {
                if let Some(n) = nodes.get_mut(node) {
                    *n = Node { lo: b.lo, hi: b.hi, first: start as u32, count: (end - start) as u32 };
                }
            };
            let n = end - start;
            if n <= LEAF {
                leaf(&mut nodes);
                continue;
            }
            let split = best_split(range, &boxes, &centres, &cb, area(&b.lo, &b.hi));
            let mid = match split {
                Some((axis, cut)) => {
                    let at = |s: &u32| centres.get(*s as usize).map_or(0.0, |c| c[axis]);
                    let r = slot.get_mut(start..end).unwrap_or_default();
                    start + partition(r, |s| at(s) < cut)
                }
                None => start,
            };
            // A split that leaves one side empty (all centroids together, or no split worth its
            // cost): halve by position instead when the leaf would be too big to scan cheaply.
            let mid = if mid == start || mid == end {
                if n <= 4 * LEAF {
                    leaf(&mut nodes);
                    continue;
                }
                start + n / 2
            } else {
                mid
            };
            let first = nodes.len();
            nodes.push(Node { lo: [0.0; 3], hi: [0.0; 3], first: 0, count: 0 });
            nodes.push(Node { lo: [0.0; 3], hi: [0.0; 3], first: 0, count: 0 });
            if let Some(nd) = nodes.get_mut(node) {
                *nd = Node { lo: b.lo, hi: b.hi, first: first as u32, count: 0 };
            }
            stack.push((first, start, mid));
            stack.push((first + 1, mid, end));
        }
        let order = slot.iter().map(|s| order.get(*s as usize).copied().unwrap_or(0)).collect();
        Bvh { nodes, order }
    }

    /// How many triangles can be hit.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The nearest triangle `ray` hits, from either side. `points` and `triangles` must be the
    /// ones the hierarchy was built from.
    pub fn cast(&self, points: &[[f32; 3]], triangles: &[[u32; 3]], ray: &Ray) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        self.walk(ray, |i| {
            if let Some(t) = hit(points, triangles, ray, i)
                && best.is_none_or(|b| t < b.t)
            {
                best = Some(Hit { triangle: i, t });
            }
            best.map_or(f64::INFINITY, |b| b.t)
        });
        best
    }

    /// Every triangle `ray` hits, nearest first, up to `limit` of them (the nearest ones).
    pub fn cast_all(&self, points: &[[f32; 3]], triangles: &[[u32; 3]], ray: &Ray, limit: usize) -> Vec<Hit> {
        let mut hits: Vec<Hit> = Vec::new();
        if limit == 0 {
            return hits;
        }
        self.walk(ray, |i| {
            if let Some(t) = hit(points, triangles, ray, i)
                && (hits.len() < limit || hits.last().is_some_and(|h| t < h.t))
            {
                let at = hits.partition_point(|h| h.t <= t);
                hits.insert(at, Hit { triangle: i, t });
                hits.truncate(limit);
            }
            if hits.len() < limit { f64::INFINITY } else { hits.last().map_or(f64::INFINITY, |h| h.t) }
        });
        hits
    }

    /// Visits the triangles of every leaf whose box the ray enters no farther than the reach
    /// `visit` returns (how far a hit still counts), nearer boxes first.
    fn walk(&self, ray: &Ray, mut visit: impl FnMut(u32) -> f64) {
        let inv = ray.direction.map(|d| 1.0 / d);
        let mut reach = f64::INFINITY;
        let enter = |n: u32| self.nodes.get(n as usize).and_then(|node| slab(ray, &inv, node).map(|t| (t, node)));
        let mut stack: Vec<(f64, u32)> = Vec::new();
        if let Some((t, _)) = enter(0) {
            stack.push((t, 0));
        }
        while let Some((t, n)) = stack.pop() {
            let Some(node) = self.nodes.get(n as usize) else { continue };
            if t > reach {
                continue;
            }
            if node.count == 0 {
                let (a, b) = (enter(node.first), enter(node.first + 1));
                let mut kids = [a.map(|(t, _)| (t, node.first)), b.map(|(t, _)| (t, node.first + 1))];
                // The farther child goes on the stack first, so the nearer one is visited first.
                if let [Some(x), Some(y)] = kids
                    && x.0 < y.0
                {
                    kids = [Some(y), Some(x)];
                }
                stack.extend(kids.into_iter().flatten());
                continue;
            }
            let (s, e) = (node.first as usize, node.first as usize + node.count as usize);
            for i in self.order.get(s..e).unwrap_or_default() {
                reach = visit(*i);
            }
        }
    }
}

/// Where `ray` hits triangle `i`, if it does.
pub(crate) fn hit(points: &[[f32; 3]], triangles: &[[u32; 3]], ray: &Ray, i: u32) -> Option<f64> {
    triangles.get(i as usize).and_then(|tri| corners(points, tri)).and_then(|c| intersect(ray, &c))
}

/// How far the exit of a box is moved out: 1 + 2γ₃ and then some.
const SLACK: f64 = 1.0 + 4.0 * f64::EPSILON;

/// How far outside a triangle (in its barycentric coordinates) a hit still counts: a ray through
/// a shared edge or vertex hits the triangles on both sides, as the rasteriser draws them.
const EDGE: f64 = 1e-9;

/// Where the ray enters the node's box (0 when it starts inside), or `None` when it misses.
fn slab(ray: &Ray, inv: &[f64; 3], node: &Node) -> Option<f64> {
    let (mut near, mut far) = (0.0_f64, f64::INFINITY);
    let axes = node.lo.iter().zip(&node.hi).zip(ray.origin.iter().zip(&ray.direction)).zip(inv);
    for (((lo, hi), (o, d)), i) in axes {
        let (lo, hi, o, d, i) = (f64::from(*lo), f64::from(*hi), *o, *d, *i);
        if d == 0.0 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let a = (lo - o) * i;
        let b = (hi - o) * i;
        near = near.max(a.min(b));
        // Rounding in the products can put the exit a hair before the entry for a ray that
        // grazes a face or edge of a flat box (most boundary faces are axis-aligned): the exit
        // is moved out by a few ulps so such a box is never skipped (Ize, "Robust BVH Ray
        // Traversal", 2013).
        far = far.min(a.max(b) * SLACK);
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// Möller–Trumbore, both sides; the hit's `t`, if any, with `t ≥ 0`.
fn intersect(ray: &Ray, c: &[[f32; 3]; 3]) -> Option<f64> {
    let p = |i: usize| c.get(i).map_or([0.0; 3], |v| v.map(f64::from));
    let (a, b, d) = (p(0), p(1), p(2));
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let cross =
        |x: [f64; 3], y: [f64; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let (e1, e2) = (sub(b, a), sub(d, a));
    let h = cross(ray.direction, e2);
    let det = dot(e1, h);
    // A triangle without area, or seen edge-on, is not hit.
    let scale = dot(e1, e1).sqrt() * dot(e2, e2).sqrt() * dot(ray.direction, ray.direction).sqrt();
    if det.abs().partial_cmp(&(1e-12 * scale)) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let f = 1.0 / det;
    let s = sub(ray.origin, a);
    let u = f * dot(s, h);
    if !(-EDGE..=1.0 + EDGE).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(ray.direction, q);
    if v < -EDGE || u + v > 1.0 + EDGE {
        return None;
    }
    let t = f * dot(e2, q);
    (t >= 0.0 && t.is_finite()).then_some(t)
}

/// The cheapest split of `range` by the surface-area heuristic: an axis and a centroid
/// coordinate (entries below it go left), or `None` when no split is cheaper than a leaf.
fn best_split(range: &[u32], boxes: &[Bounds], centres: &[[f32; 3]], cb: &Bounds, parent: f64) -> Option<(usize, f32)> {
    let mut best: Option<(f64, usize, f32)> = None;
    for axis in 0..3 {
        let (lo, hi) = (cb.lo[axis], cb.hi[axis]);
        if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
            continue;
        }
        let width = (hi - lo) / BINS as f32;
        let mut bins = [(Bounds::EMPTY, 0usize); BINS];
        for s in range {
            let (Some(c), Some(b)) = (centres.get(*s as usize), boxes.get(*s as usize)) else { continue };
            let k = (((c[axis] - lo) / width) as usize).min(BINS - 1);
            if let Some(bin) = bins.get_mut(k) {
                bin.0.join(b);
                bin.1 += 1;
            }
        }
        // Costs of cutting after each bin: left sweep, then right sweep.
        let mut left = [(0.0, 0usize); BINS];
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for (k, bin) in bins.iter().enumerate() {
            acc.join(&bin.0);
            n += bin.1;
            if let Some(l) = left.get_mut(k) {
                *l = (area(&acc.lo, &acc.hi), n);
            }
        }
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for k in (1..BINS).rev() {
            if let Some(bin) = bins.get(k) {
                acc.join(&bin.0);
                n += bin.1;
            }
            let Some((la, ln)) = left.get(k - 1).copied() else { continue };
            if ln == 0 || n == 0 {
                continue;
            }
            let cost = la * ln as f64 + area(&acc.lo, &acc.hi) * n as f64;
            if best.is_none_or(|b| cost < b.0) {
                best = Some((cost, axis, lo + width * k as f32));
            }
        }
    }
    // A leaf costs one test per triangle; a split, a box test plus its children's share.
    let leaf = parent * range.len() as f64;
    best.filter(|b| b.0 < leaf || range.len() > 4 * LEAF).map(|b| (b.1, b.2))
}

/// Moves the entries for which `left` holds to the front; returns how many there are.
fn partition(v: &mut [u32], left: impl Fn(&u32) -> bool) -> usize {
    let mut i = 0;
    for j in 0..v.len() {
        if v.get(j).is_some_and(&left) {
            v.swap(i, j);
            i += 1;
        }
    }
    i
}
