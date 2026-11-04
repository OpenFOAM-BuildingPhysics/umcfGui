//! Safe repairs of an STL surface. Each changes only what it says and returns what it did;
//! [`crate::check::analyse`] afterwards shows what is left.

use std::collections::HashMap;

use crate::Vec3;
use crate::check::{NONE, is_degenerate, parts, same_direction, topology, volume6};
use crate::stl::{Stl, Triangle};

/// What [`drop_degenerate`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Triangles with a coordinate that is not a number, removed.
    pub non_finite: usize,
    /// Copies of other triangles, removed (the first copy stays).
    pub duplicate: usize,
    /// Triangles without area removed as they were: two vertices in the same place, or none of
    /// their edges shared with exactly one other triangle (removing them opens nothing).
    pub degenerate: usize,
    /// Triangles without area removed by splitting the triangle across their long edge at
    /// their middle vertex, so the surface stays closed there.
    pub split: usize,
    /// Triangles without area left as they were (their long edge is shared by more than two
    /// triangles while a short one is shared with one, or by another triangle without area
    /// that could not be fixed).
    pub left: usize,
}

impl Dropped {
    /// Triangles without area or broken that are gone.
    pub fn removed(&self) -> usize {
        self.non_finite + self.duplicate + self.degenerate + self.split
    }
}

/// The most passes [`drop_degenerate`] makes (each fixes what the one before left in its way).
const MAX_PASSES: usize = 16;

/// Removes the triangles [`crate::check::analyse`] reports as not finite, duplicate (the first
/// copy stays) or degenerate. A degenerate triangle none of whose edges is shared with exactly
/// one other triangle is removed as it is: that opens nothing. One whose vertices are in a line
/// and whose long edge it shares with one neighbour would leave a crack along that edge, so the
/// neighbour is split in two at the middle vertex instead, which closes it.
pub fn drop_degenerate(stl: &mut Stl) -> Dropped {
    let mut d = Dropped::default();
    for _ in 0..MAX_PASSES {
        let topo = topology(stl);
        let n = topo.tri.len();
        let mut remove = vec![false; n];
        let mut added = Vec::new();
        let mut progress = false;
        for k in 0..n {
            let Some(t) = stl.triangles.get(k) else { break };
            if remove.get(k).copied().unwrap_or(true) {
                continue;
            }
            let gone = if !t.is_finite() {
                d.non_finite += 1;
                true
            } else if topo.duplicate.get(k).copied().unwrap_or(false) {
                d.duplicate += 1;
                true
            } else if topo.collapsed.get(k).copied().unwrap_or(false) {
                d.degenerate += 1;
                true
            } else if is_degenerate(t) {
                match split_cap(stl, &topo, k, &remove) {
                    Cap::Border => {
                        d.degenerate += 1;
                        true
                    }
                    Cap::Split(nb, a, b) => {
                        if let Some(x) = remove.get_mut(nb) {
                            *x = true;
                        }
                        added.push(a);
                        added.push(b);
                        d.split += 1;
                        true
                    }
                    Cap::Later => false,
                }
            } else {
                false
            };
            if gone {
                progress = true;
                if let Some(x) = remove.get_mut(k) {
                    *x = true;
                }
            }
        }
        if !progress {
            break;
        }
        let mut k = 0;
        stl.triangles.retain(|_| {
            k += 1;
            !remove.get(k - 1).copied().unwrap_or(false)
        });
        stl.triangles.extend(added);
    }
    d.left = stl.triangles.iter().filter(|t| t.is_finite() && is_degenerate(t)).count();
    d
}

enum Cap {
    /// Removing the triangle as it is opens no edge.
    Border,
    /// Remove the neighbour (this index) and add the two halves.
    Split(usize, Triangle, Triangle),
    /// Not now: the long edge is non-manifold, or the neighbour is changing or degenerate too.
    Later,
}

/// How to remove degenerate triangle `k` without opening the surface.
fn split_cap(stl: &Stl, topo: &crate::check::Topology, k: usize, remove: &[bool]) -> Cap {
    let (Some(t), Some(Some(v))) = (stl.triangles.get(k), topo.tri.get(k)) else { return Cap::Later };
    // The long edge, and the vertex opposite it (which lies on it).
    let p = t.points();
    let len = |e: usize| {
        let (a, b) = (p.get(e % 3).copied().unwrap_or_default(), p.get((e + 1) % 3).copied().unwrap_or_default());
        (b - a).length_squared()
    };
    let e = (0..3).max_by(|&a, &b| len(a).total_cmp(&len(b))).unwrap_or(0);
    let middle = t.vertices.get((e + 2) % 3).copied().unwrap_or_default();
    let counts = topo.count.get(k).copied().unwrap_or([2; 3]);
    if !counts.contains(&2) {
        return Cap::Border;
    }
    let nb = topo.neighbour.get(k).and_then(|x| x.get(e)).copied().unwrap_or(NONE);
    if nb == NONE {
        return Cap::Later;
    }
    let nb = nb as usize;
    let (Some(n), Some(Some(nv))) = (stl.triangles.get(nb), topo.tri.get(nb)) else { return Cap::Later };
    if remove.get(nb).copied().unwrap_or(true) || is_degenerate(n) {
        return Cap::Later;
    }
    let (a, b) = crate::check::edge(v, e as u32);
    // The neighbour's slot of the shared edge, in its own direction: x → y, then r.
    let Some(f) = (0..3u32).find(|&f| {
        let (x, y) = crate::check::edge(nv, f);
        (x, y) == (a, b) || (x, y) == (b, a)
    }) else {
        return Cap::Later;
    };
    let at = |i: u32| n.vertices.get((i % 3) as usize).copied().unwrap_or_default();
    let (x, y, r) = (at(f), at(f + 1), at(f + 2));
    let half = |vertices| Triangle { vertices, normal: n.normal, solid: n.solid };
    Cap::Split(nb, half([x, middle, r]), half([middle, y, r]))
}

/// What [`weld`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Welded {
    /// Distinct vertices moved onto another within the tolerance.
    pub moved: usize,
    /// Triangles removed because two of their vertices became one.
    pub removed: usize,
}

/// Moves every vertex within `tolerance` of one seen before onto it (the first one wins), then
/// removes triangles that lost their area that way. Closes gaps narrower than the tolerance
/// between triangles that should share an edge. A tolerance that is not a positive number
/// changes nothing.
pub fn weld(stl: &mut Stl, tolerance: f64) -> Welded {
    let mut w = Welded::default();
    if !(tolerance > 0.0 && tolerance.is_finite()) {
        return w;
    }
    let cell = |p: Vec3| -> Option<[i64; 3]> {
        let c = [p.x, p.y, p.z].map(|x| (x / tolerance).floor());
        // Beyond this the cell number is no longer exact: such a vertex is left alone.
        c.iter().all(|x| x.abs() < 1e15).then(|| c.map(|x| x as i64))
    };
    let mut grid: HashMap<[i64; 3], Vec<[f32; 3]>> = HashMap::new();
    // Each place is decided once: every copy of a vertex goes where the first went, so the
    // triangles that share it still do.
    let mut decided: HashMap<[u32; 3], [f32; 3]> = HashMap::new();
    let key = |v: [f32; 3]| v.map(|x| if x == 0.0 { 0 } else { x.to_bits() });
    let t2 = tolerance * tolerance;
    for t in &mut stl.triangles {
        for v in &mut t.vertices {
            if let Some(q) = decided.get(&key(*v)) {
                *v = *q;
                continue;
            }
            let p = Vec3::from_f32(*v);
            let Some(c) = cell(p) else { continue };
            let mut best: Option<([f32; 3], f64)> = None;
            for d in neighbours(c) {
                for &q in grid.get(&d).into_iter().flatten() {
                    let d2 = (Vec3::from_f32(q) - p).length_squared();
                    if d2 <= t2 && best.is_none_or(|(_, b)| d2 < b) {
                        best = Some((q, d2));
                    }
                }
            }
            match best {
                Some((q, _)) => {
                    decided.insert(key(*v), q);
                    if q != *v {
                        *v = q;
                        w.moved += 1;
                    }
                }
                // Kept vertices are more than the tolerance apart, so a cell holds a handful.
                None => {
                    decided.insert(key(*v), *v);
                    grid.entry(c).or_default().push(*v);
                }
            }
        }
    }
    let before = stl.triangles.len();
    stl.triangles.retain(|t| {
        let [a, b, c] = t.vertices;
        a != b && b != c && a != c
    });
    w.removed = before - stl.triangles.len();
    w
}

fn neighbours(c: [i64; 3]) -> impl Iterator<Item = [i64; 3]> {
    let [x, y, z] = c;
    (-1..=1).flat_map(move |i| {
        (-1..=1).flat_map(move |j| (-1..=1).map(move |k| [x.wrapping_add(i), y.wrapping_add(j), z.wrapping_add(k)]))
    })
}

/// What [`orient`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Oriented {
    /// Triangles turned over.
    pub flipped: usize,
    /// Closed parts turned inside out (their normals pointed inward).
    pub outward: usize,
    /// Edges where no consistent choice exists (a Möbius-like part): left as they are.
    pub conflicts: usize,
}

/// Makes the normals of each part consistent with each other, through the edges two
/// triangles share, then points those of closed parts outward. An open part keeps the side
/// most of its triangles already faced. Duplicate and degenerate copies are left as they are.
pub fn orient(stl: &mut Stl) -> Oriented {
    let topo = topology(stl);
    let n = topo.tri.len();
    let mut flip = vec![false; n];
    let mut visited = vec![false; n];
    let mut o = Oriented::default();
    let tri = |t: u32| topo.tri.get(t as usize).copied().flatten();
    for (part, closed) in parts(&topo) {
        let mut conflicts = 0;
        if let Some(v) = part.first().and_then(|&t| visited.get_mut(t as usize)) {
            *v = true;
        }
        // `parts` lists a part breadth-first, so each triangle after the first has a visited
        // neighbour by the time it is reached.
        for &t in &part {
            let (Some(a), f) = (tri(t), flip.get(t as usize).copied().unwrap_or(false)) else { continue };
            for &nb in topo.neighbour.get(t as usize).into_iter().flatten() {
                let Some(b) = (nb != NONE).then(|| tri(nb)).flatten() else { continue };
                let want = f ^ same_direction(&a, &b);
                match visited.get(nb as usize) {
                    Some(false) => {
                        if let Some(x) = visited.get_mut(nb as usize) {
                            *x = true;
                        }
                        if let Some(x) = flip.get_mut(nb as usize) {
                            *x = want;
                        }
                    }
                    // Seen from both of its triangles: counted twice.
                    Some(true) if flip.get(nb as usize).copied() != Some(want) => conflicts += 1,
                    _ => {}
                }
            }
        }
        let conflicts = conflicts / 2;
        o.conflicts += conflicts;
        let is_flipped = |t: u32| flip.get(t as usize).copied().unwrap_or(false);
        let invert = if closed && conflicts == 0 {
            let inward = volume6(&topo, &part, is_flipped) < 0.0;
            o.outward += usize::from(inward);
            inward
        } else {
            2 * part.iter().filter(|&&t| is_flipped(t)).count() > part.len()
        };
        if invert {
            for &t in &part {
                if let Some(x) = flip.get_mut(t as usize) {
                    *x = !*x;
                }
            }
        }
    }
    for (t, f) in stl.triangles.iter_mut().zip(&flip) {
        if *f {
            t.flip();
            o.flipped += 1;
        }
    }
    o
}

/// Multiplies every coordinate by `factor` (`0.001` for a file in millimetres). A factor that
/// is not a positive number changes nothing and returns `false` (a negative one would mirror
/// the surface and turn its normals inside out), and so does one that would take a finite
/// coordinate beyond what an STL file's numbers hold.
pub fn scale(stl: &mut Stl, factor: f64) -> bool {
    if !(factor > 0.0 && factor.is_finite()) {
        return false;
    }
    let overflows = stl
        .triangles
        .iter()
        .flat_map(|t| t.vertices.iter().flatten())
        .any(|x| x.is_finite() && !((f64::from(*x) * factor) as f32).is_finite());
    if overflows {
        return false;
    }
    for t in &mut stl.triangles {
        for x in t.vertices.iter_mut().flatten() {
            *x = (f64::from(*x) * factor) as f32;
        }
    }
    true
}
