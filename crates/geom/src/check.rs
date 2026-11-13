//! What in an STL surface breaks meshing ([`analyse`]).
//!
//! Vertices are joined where they are in the same place (an STL file repeats each vertex in
//! every triangle that uses it), within the distance OpenFOAM joins them at when it reads one
//! (`mergePoints` with `100*SMALL` for ASCII and `10*small` for binary files, 1e-13 and 1e-14
//! m). Edges then follow from the triangles:
//!
//! - an edge of one triangle is *open*: the surface has a hole or a border there;
//! - an edge of more than two triangles is *non-manifold*;
//! - an edge two triangles run along in the same direction makes their normals
//!   *inconsistent*: one of them faces the other way.
//!
//! A *part* is a set of triangles connected through their edges (as `surfaceCheck` counts
//! them). A part with no open or non-manifold edges is *closed*; its normals point *inward*
//! when its volume, from the winding, is negative.
//!
//! Duplicate triangles stay in the edges, as they do for OpenFOAM: a copy on its own makes a
//! closed pair, a copy among other triangles non-manifold edges.

use std::collections::HashMap;

use crate::stl::{Stl, Triangle};
use crate::{BBox, Vec3};

/// The most triangles listed per finding; the counts are complete.
pub const MAX_LISTED: usize = 1000;

/// Below this [`quality`] a triangle is degenerate: the threshold below which `surfaceCheck`
/// warns that a surface "might give problems in self-intersection testing" (`small`).
pub const DEGENERATE_QUALITY: f64 = 1e-15;

/// How many triangles have a defect, and the first [`MAX_LISTED`] of them (indices into
/// [`Stl::triangles`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub count: usize,
    pub triangles: Vec<u32>,
}

impl Listed {
    fn add(&mut self, t: u32) {
        self.count += 1;
        if self.triangles.len() < MAX_LISTED {
            self.triangles.push(t);
        }
    }
}

/// What [`analyse`] found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub triangles: usize,
    /// Distinct vertices.
    pub points: usize,
    /// The surface's regions as OpenFOAM names them, in its order: the solids' names made
    /// into words, in the order they first appear (solids of the same name are one region), or
    /// for a binary file `patch0` up to `patch<N>` for the largest attribute value N. The
    /// first [`MAX_LISTED`].
    pub regions: Vec<String>,
    /// How many regions there are.
    pub region_count: usize,
    /// Regions without triangles (empty solids, or attribute values no triangle has): OpenFOAM
    /// makes each an empty patch.
    pub empty_regions: usize,
    /// Solids whose name OpenFOAM changes (`Building 1` → `Building1`): (name, region). The
    /// first [`MAX_LISTED`].
    pub renamed: Vec<(String, String)>,
    pub bbox: Option<BBox>,
    /// Triangles with a coordinate that is not a finite number.
    pub non_finite: Listed,
    /// Triangles with two vertices in the same place, or all three in a line.
    pub degenerate: Listed,
    /// Second and later copies of a triangle (the same three vertices, in any order).
    pub duplicate: Listed,
    pub open_edges: usize,
    /// Triangles with an open edge.
    pub open: Listed,
    pub non_manifold_edges: usize,
    pub non_manifold: Listed,
    pub inconsistent_edges: usize,
    /// Triangles on an edge whose two triangles disagree about the side they face.
    pub inconsistent: Listed,
    pub parts: usize,
    pub closed_parts: usize,
    /// Closed, consistently wound parts whose normals point inward.
    pub inward_parts: usize,
    pub inward: Listed,
    /// Triangles whose normal written in the file points against the winding's. OpenFOAM uses
    /// the winding; another program may not.
    pub stored_normals: Listed,
}

impl Report {
    /// Whether the surface is closed and consistently wound outward, without degenerate,
    /// duplicate or broken triangles.
    pub fn is_clean_closed(&self) -> bool {
        self.triangles > 0
            && self.open_edges == 0
            && self.non_manifold_edges == 0
            && self.inconsistent_edges == 0
            && self.inward_parts == 0
            && self.degenerate.count == 0
            && self.duplicate.count == 0
            && self.non_finite.count == 0
    }
}

/// Whether `p` is inside the surface `stl`, which should be closed: the parity of the
/// triangles a ray from `p` crosses. Rays in a few fixed directions, none of them along an axis,
/// are tried until one passes no triangle's edge or vertex and starts on none; `None` when none
/// does (`p` on the surface, say) or `p` isn't finite. For an open surface the answer depends on
/// the ray: [`Report::is_clean_closed`] first.
pub fn contains(stl: &Stl, p: [f64; 3]) -> Option<bool> {
    let o = Vec3 { x: p[0], y: p[1], z: p[2] };
    if !o.is_finite() {
        return None;
    }
    const DIRS: [[f64; 3]; 5] = [
        [0.6123, 0.3217, 0.7219],
        [-0.4123, 0.8317, 0.3717],
        [0.2679, -0.6913, -0.6691],
        [-0.7862, -0.4153, 0.4583],
        [0.1414, 0.9899, -0.0173],
    ];
    'dirs: for d in DIRS {
        let d = Vec3 { x: d[0], y: d[1], z: d[2] };
        let mut crossings = 0usize;
        for t in &stl.triangles {
            if !t.is_finite() {
                continue;
            }
            let [a, b, c] = t.points();
            let (e1, e2) = (b - a, c - a);
            let h = d.cross(e2);
            let det = e1.dot(h);
            let scale = e1.length() * e2.length();
            if scale == 0.0 || det.abs() <= 1e-12 * scale {
                // Parallel to the ray, or degenerate: a ray in its plane can't be told.
                let n = e1.cross(e2);
                if n.length() > 0.0 && (o - a).dot(n).abs() <= 1e-12 * n.length() * (o - a).length().max(1.0) {
                    continue 'dirs;
                }
                continue;
            }
            let inv = 1.0 / det;
            let s = o - a;
            let u = s.dot(h) * inv;
            let q = s.cross(e1);
            let v = d.dot(q) * inv;
            let w = 1.0 - u - v;
            let tt = e2.dot(q) * inv;
            const EDGE: f64 = 1e-9;
            if u < -EDGE || v < -EDGE || w < -EDGE {
                continue;
            }
            if u < EDGE || v < EDGE || w < EDGE {
                // Through an edge or a vertex: counted once or twice, unknown.
                if tt > -EDGE * scale.sqrt() {
                    continue 'dirs;
                }
                continue;
            }
            if tt.abs() <= EDGE * scale.sqrt().max(1.0) {
                return None;
            }
            if tt > 0.0 {
                crossings += 1;
            }
        }
        return Some(crossings % 2 == 1);
    }
    None
}

const OPEN: u8 = 1;
const NON_MANIFOLD: u8 = 2;
const INCONSISTENT: u8 = 4;
/// No neighbour across this edge (or more than one).
pub(crate) const NONE: u32 = u32::MAX;

/// The surface as joined vertices and edges.
pub(crate) struct Topology {
    /// Per triangle: its three vertices, `None` when it is left out of the edges (a
    /// coordinate not finite, or two vertices in the same place).
    pub tri: Vec<Option<[u32; 3]>>,
    pub points: Vec<Vec3>,
    /// Distinct vertices, once those within the merge distance are joined.
    pub distinct: usize,
    /// Per triangle and edge (`k` → `k + 1`): the one other triangle on it, or [`NONE`].
    pub neighbour: Vec<[u32; 3]>,
    pub flags: Vec<u8>,
    /// Per triangle: the part it is in (a triangle of that part).
    pub part: Vec<u32>,
    /// Per triangle and edge: how many triangles the edge has (255 for that many or more).
    pub count: Vec<[u8; 3]>,
    pub collapsed: Vec<bool>,
    pub duplicate: Vec<bool>,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    pub inconsistent_edges: usize,
}

/// A coordinate's key for joining: `-0` and `0` are the same place.
fn key(v: f32) -> u32 {
    if v == 0.0 { 0 } else { v.to_bits() }
}

/// The largest triangle index the edge records hold (two bits of the record name the edge).
const MAX_TRIANGLE: usize = (u32::MAX >> 2) as usize;

/// Below this size two different `f32` coordinates can be within OpenFOAM's merge distance:
/// above it their spacing is larger.
const TINY: f32 = 2e-6;

/// The distance OpenFOAM joins a file's vertices within.
fn merge_distance(format: crate::stl::Format) -> f64 {
    match format {
        crate::stl::Format::Ascii => 1e-13,
        crate::stl::Format::Binary => 1e-14,
    }
}

/// For each point, the point it is joined to (itself, or one before it within `distance`),
/// and how many points are left. Only points with a coordinate near zero can be that close
/// and not the same: they are grouped by their other coordinates, then joined within each
/// group on a grid of the merge distance.
fn join_near(points: &[Vec3], distance: f64) -> (Vec<u32>, usize) {
    let mut to: Vec<u32> = (0..points.len()).map(|k| u32::try_from(k).unwrap_or(NONE)).collect();
    let tiny = |c: f64| c.abs() < f64::from(TINY);
    let mut groups: HashMap<[u64; 3], Vec<u32>> = HashMap::new();
    for (k, p) in points.iter().enumerate() {
        let c = [p.x, p.y, p.z];
        if c.iter().any(|&x| tiny(x))
            && let Ok(k) = u32::try_from(k)
        {
            groups.entry(c.map(|x| if tiny(x) { u64::MAX } else { x.to_bits() })).or_default().push(k);
        }
    }
    let mut joined = 0;
    for members in groups.values().filter(|m| m.len() > 1) {
        let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for &k in members {
            let Some(p) = points.get(k as usize).copied() else { continue };
            // Coordinates that are not near zero are the same in the whole group: cell 0.
            let cell = [p.x, p.y, p.z].map(|x| if tiny(x) { (x / distance).floor() as i64 } else { 0 });
            let [x, y, z] = cell;
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        for &q in grid.get(&[x + dx, y + dy, z + dz]).into_iter().flatten() {
                            let near = points.get(q as usize).is_some_and(|&o| (o - p).length() <= distance);
                            if near {
                                found = Some(q);
                                break 'search;
                            }
                        }
                    }
                }
            }
            match found {
                Some(q) => {
                    if let Some(t) = to.get_mut(k as usize) {
                        *t = q;
                    }
                    joined += 1;
                }
                None => grid.entry(cell).or_default().push(k),
            }
        }
    }
    (to, points.len() - joined)
}

pub(crate) fn topology(stl: &Stl) -> Topology {
    let n = stl.triangles.len().min(MAX_TRIANGLE);
    let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
    let mut points = Vec::new();
    let mut tri = Vec::with_capacity(n);
    for t in stl.triangles.iter().take(n) {
        if !t.is_finite() {
            tri.push(None);
            continue;
        }
        let v = t.vertices.map(|p| {
            *ids.entry(p.map(key)).or_insert_with(|| {
                points.push(Vec3::from_f32(p));
                u32::try_from(points.len() - 1).unwrap_or(NONE)
            })
        });
        tri.push(Some(v));
    }
    drop(ids);
    let (to, distinct) = join_near(&points, merge_distance(stl.format));
    let mut collapsed = vec![false; n];
    for (v, c) in tri.iter_mut().zip(collapsed.iter_mut()) {
        let Some(t) = v else { continue };
        let [a, b, cc] = t.map(|i| to.get(i as usize).copied().unwrap_or(i));
        if a == b || b == cc || a == cc {
            *c = true;
            *v = None;
        } else {
            *v = Some([a, b, cc]);
        }
    }
    drop(to);
    // Duplicates: the same three vertices, in any order. The first copy stays.
    let mut duplicate = vec![false; n];
    let mut sorted: Vec<([u32; 3], u32)> = tri
        .iter()
        .enumerate()
        .filter_map(|(k, v)| {
            let mut v = (*v)?;
            v.sort_unstable();
            Some((v, u32::try_from(k).ok()?))
        })
        .collect();
    sorted.sort_unstable();
    for w in sorted.windows(2) {
        if let [(a, _), (b, k)] = w
            && a == b
            && let Some(d) = duplicate.get_mut(*k as usize)
        {
            *d = true;
        }
    }
    drop(sorted);
    // Edges: (lower vertex, higher vertex) → record `triangle << 2 | edge`.
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(3 * n);
    for (k, v) in tri.iter().enumerate() {
        let (Some(v), Ok(k)) = (v, u32::try_from(k)) else { continue };
        for e in 0..3u32 {
            let (a, b) = edge(v, e);
            edges.push(((u64::from(a.min(b)) << 32) | u64::from(a.max(b)), (k << 2) | e));
        }
    }
    edges.sort_unstable();
    let mut neighbour = vec![[NONE; 3]; n];
    let mut flags = vec![0u8; n];
    let mut count = vec![[0u8; 3]; n];
    let mut parent: Vec<u32> = (0..n).map(|k| u32::try_from(k).unwrap_or(NONE)).collect();
    let (mut open_edges, mut non_manifold_edges, mut inconsistent_edges) = (0, 0, 0);
    let mut flag = |r: u32, f: u8| {
        if let Some(x) = flags.get_mut((r >> 2) as usize) {
            *x |= f;
        }
    };
    for run in edges.chunk_by(|a, b| a.0 == b.0) {
        let k = u8::try_from(run.len()).unwrap_or(u8::MAX);
        for (_, r) in run {
            if let Some(c) = count.get_mut((r >> 2) as usize).and_then(|c| c.get_mut((r & 3) as usize)) {
                *c = k;
            }
        }
        if let Some((_, first)) = run.first() {
            for (_, r) in run.get(1..).unwrap_or_default() {
                union(&mut parent, first >> 2, r >> 2);
            }
        }
        match run {
            [(_, r)] => {
                open_edges += 1;
                flag(*r, OPEN);
            }
            [(_, r), (_, s)] => {
                let forward = |r: u32| {
                    tri.get((r >> 2) as usize).copied().flatten().is_some_and(|v| {
                        let (a, b) = edge(&v, r & 3);
                        a < b
                    })
                };
                if forward(*r) == forward(*s) {
                    inconsistent_edges += 1;
                    flag(*r, INCONSISTENT);
                    flag(*s, INCONSISTENT);
                }
                for (x, y) in [(r, s), (s, r)] {
                    if let Some(slot) = neighbour.get_mut((x >> 2) as usize).and_then(|nb| nb.get_mut((x & 3) as usize))
                    {
                        *slot = y >> 2;
                    }
                }
            }
            _ => {
                non_manifold_edges += 1;
                for (_, r) in run {
                    flag(*r, NON_MANIFOLD);
                }
            }
        }
    }
    let part = (0..n).map(|k| find(&mut parent, u32::try_from(k).unwrap_or(NONE))).collect();
    Topology {
        tri,
        points,
        distinct,
        neighbour,
        flags,
        part,
        count,
        collapsed,
        duplicate,
        open_edges,
        non_manifold_edges,
        inconsistent_edges,
    }
}

/// The root of `k`'s set, halving the path on the way.
fn find(parent: &mut [u32], mut k: u32) -> u32 {
    while let Some(&p) = parent.get(k as usize) {
        if p == k {
            break;
        }
        let gp = parent.get(p as usize).copied().unwrap_or(p);
        if let Some(x) = parent.get_mut(k as usize) {
            *x = gp;
        }
        k = gp;
    }
    k
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b
        && let Some(x) = parent.get_mut(a.max(b) as usize)
    {
        *x = a.min(b);
    }
}

/// Edge `e` of a triangle: from its vertex `e` to the next.
pub(crate) fn edge(v: &[u32; 3], e: u32) -> (u32, u32) {
    let at = |k: u32| v.get((k % 3) as usize).copied().unwrap_or(NONE);
    (at(e), at(e + 1))
}

/// Whether two neighbours run along their shared edge in the same direction.
pub(crate) fn same_direction(a: &[u32; 3], b: &[u32; 3]) -> bool {
    (0..3).any(|e| {
        let (p, q) = edge(a, e);
        (0..3).any(|f| edge(b, f) == (p, q))
    })
}

/// A triangle's quality as OpenFOAM measures it: its area over that of the equilateral
/// triangle with the same circumradius (1 for an equilateral triangle, 0 for a collapsed one).
//
// Ported from OpenFOAM-12, src/OpenFOAM/meshes/primitiveShapes/triangle/triangleI.H
// (`triangle::circumCircle`, `triangle::quality`), Copyright (C) 2011-2024 OpenFOAM Foundation.
pub fn quality(t: &Triangle) -> f64 {
    const GREAT: f64 = 1e15;
    const ROOT_VSMALL: f64 = 1e-150;
    let [a, b, c] = t.points();
    let d1 = (c - a).dot(b - a);
    let d2 = -(c - b).dot(b - a);
    let d3 = (c - a).dot(c - b);
    let (c1, c2, c3) = (d2 * d3, d3 * d1, d1 * d2);
    let denom = c1 + c2 + c3;
    let r = if denom.abs() < ROOT_VSMALL {
        3f64.sqrt() * GREAT
    } else {
        ((d1 + d2) * (d2 + d3) * (d3 + d1) / (4.0 * denom)).max(0.0).sqrt()
    };
    let area = 0.5 * t.cross().length();
    area / (0.75 * 3f64.sqrt() * r.min(GREAT).powi(2) + ROOT_VSMALL)
}

/// Whether a triangle has no area to speak of: its [`quality`] is below
/// [`DEGENERATE_QUALITY`] (two vertices in the same place, or all three in a line), or not a
/// number.
pub fn is_degenerate(t: &Triangle) -> bool {
    quality(t).partial_cmp(&DEGENERATE_QUALITY) != Some(std::cmp::Ordering::Greater)
}

/// The parts of the surface: triangles connected through edges of two triangles, each with
/// whether it is closed.
pub(crate) fn parts(topo: &Topology) -> Vec<(Vec<u32>, bool)> {
    let n = topo.tri.len();
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    for start in 0..n {
        if topo.tri.get(start).copied().flatten().is_none() || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Ok(start) = u32::try_from(start) else { break };
        let mut part = vec![start];
        if let Some(s) = seen.get_mut(start as usize) {
            *s = true;
        }
        let mut k = 0;
        let mut closed = true;
        while let Some(&t) = part.get(k) {
            k += 1;
            closed &= topo.flags.get(t as usize).is_some_and(|f| f & (OPEN | NON_MANIFOLD) == 0);
            for &nb in topo.neighbour.get(t as usize).into_iter().flatten() {
                if let Some(s) = seen.get_mut(nb as usize)
                    && !*s
                {
                    *s = true;
                    part.push(nb);
                }
            }
        }
        out.push((part, closed));
    }
    out
}

/// Six times the volume a set of triangles encloses, from their winding (positive when the
/// normals point outward), with `flip` reversing some of them.
pub(crate) fn volume6(topo: &Topology, part: &[u32], flip: impl Fn(u32) -> bool) -> f64 {
    // Relative to a point of the part: far from the origin the products lose their digits.
    let at = |i: u32| topo.points.get(i as usize).copied().unwrap_or(Vec3::ZERO);
    let origin = part.first().and_then(|&t| topo.tri.get(t as usize).copied().flatten()).map_or(Vec3::ZERO, |v| {
        let [a, _, _] = v;
        at(a)
    });
    part.iter()
        .filter_map(|&t| Some((t, topo.tri.get(t as usize).copied().flatten()?)))
        .map(|(t, [a, b, c])| {
            let v = (at(a) - origin).dot((at(b) - origin).cross(at(c) - origin));
            if flip(t) { -v } else { v }
        })
        .sum()
}

/// Finds what in `stl` breaks meshing. See the module documentation.
pub fn analyse(stl: &Stl) -> Report {
    let topo = topology(stl);
    let mut r = Report {
        triangles: stl.triangles.len(),
        points: topo.distinct,
        bbox: stl.bbox(),
        open_edges: topo.open_edges,
        non_manifold_edges: topo.non_manifold_edges,
        inconsistent_edges: topo.inconsistent_edges,
        ..Report::default()
    };
    // Regions as OpenFOAM makes them (STLReader / triSurface::calcPatches).
    match stl.format {
        crate::stl::Format::Ascii => {
            let mut used = vec![false; stl.solids.len()];
            for t in &stl.triangles {
                if let Some(u) = used.get_mut(t.solid as usize) {
                    *u = true;
                }
            }
            // Region name → whether a triangle is in it, in order of appearance.
            let mut index: HashMap<String, usize> = HashMap::new();
            let mut has: Vec<bool> = Vec::new();
            for (raw, u) in stl.solids.iter().zip(&used) {
                let name = crate::stl::foam_name(raw);
                if !raw.is_empty() && *raw != name && r.renamed.len() < MAX_LISTED {
                    r.renamed.push((raw.clone(), name.clone()));
                }
                let k = *index.entry(name.clone()).or_insert_with(|| {
                    has.push(false);
                    if r.regions.len() < MAX_LISTED {
                        r.regions.push(name);
                    }
                    has.len() - 1
                });
                if let Some(h) = has.get_mut(k) {
                    *h |= *u;
                }
            }
            r.region_count = has.len();
            r.empty_regions = has.iter().filter(|h| !**h).count();
        }
        crate::stl::Format::Binary => {
            let mut values: Vec<u32> = stl.triangles.iter().map(|t| t.solid).collect();
            values.sort_unstable();
            values.dedup();
            if let Some(&max) = values.last() {
                r.region_count = max as usize + 1;
                r.empty_regions = r.region_count - values.len();
                r.regions = (0..=max).take(MAX_LISTED).map(|k| format!("patch{k}")).collect();
            }
        }
    }
    for (k, t) in stl.triangles.iter().enumerate().take(topo.tri.len()) {
        let Ok(k32) = u32::try_from(k) else { break };
        if !t.is_finite() {
            r.non_finite.add(k32);
            continue;
        }
        if topo.collapsed.get(k).copied().unwrap_or(false) || is_degenerate(t) {
            r.degenerate.add(k32);
        } else {
            let stored = Vec3::from_f32(t.normal);
            if stored.is_finite() && stored.dot(t.cross()) < 0.0 {
                r.stored_normals.add(k32);
            }
        }
        if topo.duplicate.get(k).copied().unwrap_or(false) {
            r.duplicate.add(k32);
        }
        let f = topo.flags.get(k).copied().unwrap_or(0);
        if f & OPEN != 0 {
            r.open.add(k32);
        }
        if f & NON_MANIFOLD != 0 {
            r.non_manifold.add(k32);
        }
        if f & INCONSISTENT != 0 {
            r.inconsistent.add(k32);
        }
    }
    // Parts as surfaceCheck counts them: through any edge.
    let mut by_part: HashMap<u32, Vec<u32>> = HashMap::new();
    for (k, (v, p)) in topo.tri.iter().zip(&topo.part).enumerate() {
        if v.is_some()
            && let Ok(k) = u32::try_from(k)
        {
            by_part.entry(*p).or_default().push(k);
        }
    }
    let mut by_part: Vec<(u32, Vec<u32>)> = by_part.into_iter().collect();
    by_part.sort_unstable_by_key(|(p, _)| *p);
    for (_, part) in by_part {
        r.parts += 1;
        let closed = part.iter().all(|&t| topo.flags.get(t as usize).is_some_and(|f| f & (OPEN | NON_MANIFOLD) == 0));
        if !closed {
            continue;
        }
        r.closed_parts += 1;
        let consistent = part.iter().all(|&t| topo.flags.get(t as usize).is_some_and(|f| f & INCONSISTENT == 0));
        if consistent && volume6(&topo, &part, |_| false) < 0.0 {
            r.inward_parts += 1;
            for &t in &part {
                r.inward.add(t);
            }
        }
    }
    r
}
