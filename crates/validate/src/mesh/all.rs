//! The checks `checkMesh -allGeometry` adds to the default ones, with its thresholds.
//!
//! Ported from OpenFOAM-12 (GPL-3.0-or-later, as this crate):
//! - `src/meshCheck/primitiveMeshCheck/primitiveMeshCheck.C` (`faceConcavity`, `faceFlatness`,
//!   `cellDeterminant`, `checkFaceAngles`, `checkFaceFlatness`, `checkConcaveCells`),
//!   `primitiveMeshCheckEdgeLength.C` (`checkEdgeLength`) and
//!   `primitiveMeshCheckPointNearness.C` (`checkPointNearness`), Copyright (C) 2011-2024
//!   OpenFOAM Foundation;
//! - `src/meshCheck/polyMeshCheck/polyMeshCheck.C` (`faceWeights`, `volRatio`,
//!   `checkCellDeterminant`, `checkFaceWeight`, `checkVolRatio`), Copyright (C) 2012-2024
//!   OpenFOAM Foundation;
//! - `src/OpenFOAM/meshes/polyMesh/polyMeshTetDecomposition/polyMeshTetDecomposition.C`
//!   (`minQuality`, `findSharedBasePoint`, `findBasePoint`, `checkFaceTets`) and
//!   `src/OpenFOAM/meshes/primitiveShapes/tetrahedron/tetrahedronI.H` (`circumSphere`,
//!   `quality`), Copyright (C) 2011-2024 OpenFOAM Foundation;
//! - the wedge directions of `src/OpenFOAM/meshes/polyMesh/polyMesh.C` (`calcDirections`) and
//!   `polyPatches/constraint/wedge/wedgePolyPatch.C` (`calcGeometry`), Copyright (C)
//!   2011-2024 OpenFOAM Foundation;
//! - the thresholds of `src/meshCheck/checkGeometry.C`, Copyright (C) 2011-2024 OpenFOAM
//!   Foundation.
//!
//! As in the default checks, faces of coupled patches are boundary faces: their weight and
//! volume ratio are 1, and their tets are checked against the owner cell only.
//!
//! Near points are found as `checkMesh` finds them (two points closer than the near distance
//! whose squared distances from the origin differ by less than its square), but through a
//! grid rather than `checkMesh`'s sorted list, whose search is quadratic on lattices centred at
//! the origin.
//!
//! Some work grows with the square of a face's points or a cell's faces, which a hostile mesh
//! can make large: a face with more than [`MAX_PAIRWISE`] points is not searched for a tet base
//! point, a cell with more than [`MAX_PAIRWISE`] faces not checked for concavity, a grid cell
//! with more than [`MAX_PAIRWISE`] points not searched for near points, and none of this runs
//! once [`WORK_PER_LABEL`] times the mesh's size is spent. Each is counted in `unchecked`.

use umcf_foam_mesh::PolyMesh;

use super::{
    Found, MAX_LISTED, ROOT_V_SMALL, SMALL, V, V_SMALL, add, area_centre, cmpt_mag, cross, dot, face_area_centre,
    face_points_into, mag, scale, sub,
};

/// `great` (OpenFOAM-12: `1/small`).
const GREAT: f64 = 1.0 / SMALL;

/// `polyMeshTetDecomposition::minTetQuality`.
pub const MIN_TET_QUALITY: f64 = SMALL * SMALL;
/// `checkFaceAngles(mesh, true, degToRad(10), …)`.
pub const MAX_CONCAVE_DEG: f64 = 10.0;
/// `checkFaceFlatness(mesh, true, 0.8, …)`.
pub const WARN_FLATNESS: f64 = 0.8;
/// `checkCellDeterminant`'s `warnDet`.
pub const WARN_DETERMINANT: f64 = 1.0e-3;
/// `checkConcaveCells(mesh, planarCosAngle, …)`.
pub const PLANAR_COS_ANGLE: f64 = 1.0e-6;
/// `checkFaceWeight(mesh, true, 0.05, …)`.
pub const MIN_WEIGHT: f64 = 0.05;
/// `checkVolRatio(mesh, true, 0.01, …)`.
pub const MIN_VOLUME_RATIO: f64 = 0.01;
/// Short edges and near points: closer than this fraction of the bounding box's diagonal.
pub const NEAR_FRACTION: f64 = 1.0e-6;

/// The most points of a face, faces of a cell, or points of a near-point grid cell compared
/// pairwise.
pub const MAX_PAIRWISE: usize = 1000;
/// The pairwise work (tets, face pairs, point pairs) allowed per face label and cell side of
/// the mesh: an ordinary mesh needs a few dozen.
pub const WORK_PER_LABEL: u64 = 256;

/// What the `-allGeometry` checks found, with `checkMesh`'s numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllGeometry {
    /// Faces whose decomposition into tets has a tet of low quality or negative volume; the
    /// count is of failed tests (up to three per face), as `checkMesh` counts.
    pub low_quality_tets: Found,
    pub min_edge_length: f64,
    pub max_edge_length: f64,
    /// [`NEAR_FRACTION`] of the bounding box diagonal (of the finite points). When it is not
    /// finite (coordinates near the largest number), short edges and near points are not
    /// looked for.
    pub near_distance: f64,
    /// The points of edges shorter than `near_distance`.
    pub short_edge_points: Found,
    /// Pairs of points closer than `near_distance` (the count is of pairs, the list of points).
    pub near_points: Found,
    /// Faces with an angle between consecutive edges concave by more than [`MAX_CONCAVE_DEG`].
    pub concave_faces: Found,
    /// Degrees.
    pub max_concave_angle: f64,
    /// Area over the sum of the areas of its triangles: 1 for a flat face.
    pub min_flatness: f64,
    pub average_flatness: f64,
    pub warped_faces: Found,
    /// The directions the determinant is taken in (`geometricD`: without the normals of
    /// `empty` and `wedge` patches).
    pub geometric_directions: [bool; 3],
    pub min_determinant: f64,
    pub average_determinant: f64,
    pub underdetermined_cells: Found,
    pub concave_cells: Found,
    /// Over the internal faces (a boundary face's weight is 1).
    pub min_weight: f64,
    pub average_weight: f64,
    pub low_weight_faces: Found,
    pub min_volume_ratio: f64,
    pub average_volume_ratio: f64,
    pub low_volume_ratio_faces: Found,
    /// Faces, cells and points left out of a pairwise check (see the module documentation).
    pub unchecked: usize,
}

/// Pairwise work left (see [`WORK_PER_LABEL`]).
struct Budget(u64);

impl Budget {
    fn take(&mut self, n: u64) -> bool {
        match self.0.checked_sub(n) {
            Some(left) => {
                self.0 = left;
                true
            }
            None => false,
        }
    }
}

/// `tetrahedron::quality`: the volume over that of the regular tet with the same circumradius.
fn tet_quality(a: V, b: V, c: V, d: V) -> f64 {
    let (ea, eb, ec) = (sub(b, a), sub(c, a), sub(d, a));
    let ba = cross(eb, ea);
    let ca = cross(ec, ea);
    let lambda = dot(ec, ec) - dot(ea, ec);
    let mu = dot(eb, eb) - dot(ea, eb);
    let num = sub(scale(lambda, ba), scale(mu, ca));
    let denom = dot(ec, ba);
    let sqrt3 = 3f64.sqrt();
    let r = if denom.abs() < ROOT_V_SMALL { sqrt3 * GREAT } else { mag(scale(0.5, add(ea, scale(1.0 / denom, num)))) };
    let volume = dot(cross(ea, eb), ec) / 6.0;
    volume / ((8.0 / 27.0) * sqrt3 * r.min(GREAT).powi(3) + ROOT_V_SMALL)
}

/// Whether every tet of face `ps` split from its point `base` towards the cell centre `cc` is
/// better than `tol` (`polyMeshTetDecomposition::minQuality(…) > tol`).
fn base_ok(ps: &[V], cc: V, owner: bool, base: usize, tol: f64) -> bool {
    let n = ps.len();
    let at = |k: usize| ps.get(k % n).copied().unwrap_or([0.0; 3]);
    let base_pt = at(base);
    (1..n.saturating_sub(1)).all(|t| {
        let (i, j) = ((t + base) % n, (t + base + 1) % n);
        let (pa, pb) = if owner { (at(i), at(j)) } else { (at(j), at(i)) };
        tet_quality(cc, base_pt, pa, pb) > tol
    })
}

/// Failed tests of `checkFaceTets` for one face (0 to 3), and whether its base point could be
/// searched for (within [`MAX_PAIRWISE`] and the budget).
fn face_tet_errors(ps: &[V], fc: V, own: V, nei: Option<V>, budget: &mut Budget) -> (usize, bool) {
    let n = ps.len();
    let at = |k: usize| ps.get(k % n).copied().unwrap_or([0.0; 3]);
    let tol = MIN_TET_QUALITY;
    let mut errors = usize::from((0..n).any(|k| tet_quality(at(k), at(k + 1), fc, own) > -tol));
    if let Some(nc) = nei {
        errors += usize::from((0..n).any(|k| tet_quality(at(k), at(k + 1), fc, nc) < tol));
    }
    if n > MAX_PAIRWISE {
        return (errors, false);
    }
    // findSharedBasePoint (internal faces), findBasePoint (boundary faces).
    for b in 0..n {
        if !budget.take(2 * n as u64) {
            return (errors, false);
        }
        if base_ok(ps, own, true, b, tol) && nei.is_none_or(|nc| base_ok(ps, nc, false, b, tol)) {
            return (errors, true);
        }
    }
    (errors + 1, true)
}

/// `faceConcavity`: the largest sine of a concave angle between consecutive edges (0 when
/// none is concave by more than `max_sin`).
fn concavity(ps: &[V], area: V, max_sin: f64) -> f64 {
    let normal = scale(1.0 / (mag(area) + ROOT_V_SMALL), area);
    let (Some(first), Some(last)) = (ps.first(), ps.last()) else { return 0.0 };
    let mut e_prev = sub(*first, *last);
    let mut mag_prev = mag(e_prev);
    e_prev = scale(1.0 / (mag_prev + ROOT_V_SMALL), e_prev);
    let mut max_edge_sin: f64 = 0.0;
    for (k, p0) in ps.iter().enumerate() {
        let p1 = ps.get(k + 1).unwrap_or(first);
        let mut e10 = sub(*p1, *p0);
        let mag10 = mag(e10);
        e10 = scale(1.0 / (mag10 + ROOT_V_SMALL), e10);
        if mag_prev > SMALL && mag10 > SMALL {
            let edge_normal = cross(e_prev, e10);
            let m = mag(edge_normal);
            if m >= max_sin && dot(scale(1.0 / m, edge_normal), normal) < SMALL {
                max_edge_sin = max_edge_sin.max(m);
            }
        }
        (e_prev, mag_prev) = (e10, mag10);
    }
    max_edge_sin
}

/// `faceFlatness`, for a face of more than three points with an area.
fn flatness(ps: &[V], area: V, fc: V) -> f64 {
    let n = ps.len();
    let sum: f64 = ps
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let next = ps.get((k + 1) % n).copied().unwrap_or(*p);
            mag(scale(0.5, cross(sub(next, *p), sub(fc, *p))))
        })
        .sum();
    mag(area) / (sum + ROOT_V_SMALL)
}

/// One face of a cell: area vector, centre, internal, owned by the cell.
type CellFace = (V, V, bool, bool);

/// The determinant of `sum((a/avg)²)` over the internal faces of a cell (`cellDeterminant`).
fn determinant(faces: &[CellFace], n_dims: usize, two_d: usize) -> f64 {
    if n_dims == 1 {
        return 1.0;
    }
    let internal = faces.iter().filter(|f| f.2);
    let n = internal.clone().count();
    if n == 0 {
        return 0.0;
    }
    let avg = internal.clone().map(|f| mag(f.0)).sum::<f64>() / n as f64;
    let (mut xx, mut xy, mut xz, mut yy, mut yz, mut zz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for f in internal {
        let [x, y, z] = scale(1.0 / avg, f.0);
        (xx, xy, xz, yy, yz, zz) = (xx + x * x, xy + x * y, xz + x * z, yy + y * y, yz + y * z, zz + z * z);
    }
    if n_dims == 2 {
        match two_d {
            0 => xx = 1.0,
            1 => yy = 1.0,
            _ => zz = 1.0,
        }
    }
    (xx * yy * zz + 2.0 * xy * yz * xz - xx * yz * yz - yy * xz * xz - zz * xy * xy).abs()
}

/// `checkConcaveCells` for one cell.
fn concave_cell(faces: &[CellFace]) -> bool {
    faces.iter().enumerate().any(|(i, (a, fc, _, owned))| {
        let mut normal = scale(1.0 / mag(*a).max(V_SMALL), *a);
        if !owned {
            normal = scale(-1.0, normal);
        }
        faces.iter().enumerate().any(|(j, (_, other, _, _))| {
            let d = sub(*other, *fc);
            j != i && dot(scale(1.0 / mag(d).max(V_SMALL), d), normal) > -PLANAR_COS_ANGLE
        })
    })
}

/// The faces of each cell (`primitiveMesh::cells`: owned faces, then neighbour faces, each in
/// order): cell `c`'s are `faces[starts[c]..starts[c + 1]]`.
fn cell_faces(mesh: &PolyMesh) -> (Vec<usize>, Vec<u32>) {
    let n_cells = mesh.n_cells();
    // Counts, then the end of each cell's run; filled backwards, each end moves to the start.
    let mut starts = vec![0usize; n_cells + 1];
    for c in mesh.owner.iter().chain(&mesh.neighbour) {
        if let Some(s) = starts.get_mut(*c as usize) {
            *s += 1;
        }
    }
    let mut total = 0usize;
    for s in starts.iter_mut() {
        total += *s;
        *s = total;
    }
    let mut faces = vec![0u32; total];
    let sides = mesh.owner.iter().enumerate().chain(mesh.neighbour.iter().enumerate());
    for (i, c) in sides.rev() {
        if let Some(s) = starts.get_mut(*c as usize)
            && let Some(k) = s.checked_sub(1)
            && let Some(slot) = faces.get_mut(k)
        {
            *s = k;
            *slot = i as u32;
        }
    }
    (starts, faces)
}

/// Points marked in `marked`, in order, as a [`Found`] with `count` problems.
fn points_found(marked: &[bool], count: usize) -> Found {
    let mut f = Found { count, ..Found::default() };
    for (i, _) in marked.iter().enumerate().filter(|(_, m)| **m) {
        f.places += 1;
        if f.first.len() < MAX_LISTED {
            f.first.push(i as u32);
        }
    }
    f
}

/// `polyMesh::calcDirections`' `geometricD`: the solved directions (`solution`, from the
/// `empty` patches), or with `wedge` patches every direction but the one they are normal to
/// (as OpenFOAM sets it, which drops an `empty` direction too).
pub fn geometric_directions(mesh: &PolyMesh, solution: [bool; 3]) -> [bool; 3] {
    let mut sum = [0.0; 3];
    let mut any = false;
    for p in mesh.boundary.iter().filter(|p| p.kind == "wedge" && p.n_faces > 0) {
        // wedgePolyPatch::calcGeometry: the average face normal, and the coordinate plane
        // it is closest to.
        let mut n = [0.0; 3];
        for i in p.faces() {
            let (a, _) = face_area_centre(mesh, i);
            n = add(n, scale(1.0 / mag(a).max(V_SMALL), a));
        }
        let n = scale(1.0 / p.n_faces as f64, n);
        let centre = n.map(|x| x.signum() * (x.abs().max(0.5) - 0.5));
        let m = mag(centre);
        if m > V_SMALL {
            any = true;
            sum = add(sum, cmpt_mag(scale(1.0 / m, centre)));
        }
    }
    let m = mag(sum);
    if !any || m <= V_SMALL {
        return solution;
    }
    sum.map(|x| x / m <= 1e-6)
}

/// The cell of `p` in the near-point grid `shift` (0 to 7): cells `h` wide, offset by `h/2`
/// along the axes whose bit is set. Two points less than `h/2` apart along every axis share a
/// cell in at least one of the eight grids.
fn grid_cell(p: V, lo: V, h: f64, shift: usize) -> [i64; 3] {
    let mut c = [0i64; 3];
    for (k, ck) in c.iter_mut().enumerate() {
        let offset = if (shift >> k) & 1 == 1 { 0.5 * h } else { 0.0 };
        let x = p.get(k).copied().unwrap_or(0.0) - lo.get(k).copied().unwrap_or(0.0);
        // Saturating: the points are finite, the cell index may not fit.
        *ck = ((x + offset) / h).floor() as i64;
    }
    c
}

/// `checkPointNearness`: pairs of points closer than `sqrt(min_dist_sqr)` whose squared
/// distances from the origin differ by less than `min_dist_sqr`; marks their points in `near`.
/// Returns the number of pairs, and the points left out (in grid cells over [`MAX_PAIRWISE`],
/// or past the budget).
fn near_pairs(points: &[V], lo: V, min_dist_sqr: f64, near: &mut [bool], budget: &mut Budget) -> (usize, usize) {
    let h = 2.0 * min_dist_sqr.sqrt();
    let finite = |p: &V| p.iter().all(|x| x.is_finite());
    let hash = |c: [i64; 3]| {
        (c[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (c[1] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (c[2] as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
    };
    let point = |k: u32| points.get(k as usize).copied().unwrap_or([0.0; 3]);
    let mut n_close = 0usize;
    let mut skipped = vec![false; points.len()];
    let mut keys: Vec<(u64, u32)> = Vec::new();
    for shift in 0..8 {
        keys.clear();
        keys.extend(
            points
                .iter()
                .enumerate()
                .filter(|(_, p)| finite(p))
                .map(|(i, p)| (hash(grid_cell(*p, lo, h, shift)), i as u32)),
        );
        keys.sort_unstable();
        for group in keys.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1) {
            let k = group.len() as u64;
            if group.len() > MAX_PAIRWISE || !budget.take(k * (k - 1) / 2) {
                for (_, i) in group {
                    if let Some(s) = skipped.get_mut(*i as usize) {
                        *s = true;
                    }
                }
                continue;
            }
            for (a, (_, i)) in group.iter().enumerate() {
                for (_, j) in group.iter().skip(a + 1) {
                    let (pi, pj) = (point(*i), point(*j));
                    let d = sub(pi, pj);
                    let same = |s: usize| grid_cell(pi, lo, h, s) == grid_cell(pj, lo, h, s);
                    // A pair is counted in the first grid it shares a cell in.
                    if dot(d, d) < min_dist_sqr
                        && (dot(pi, pi) - dot(pj, pj)).abs() < min_dist_sqr
                        && same(shift)
                        && !(0..shift).any(same)
                    {
                        n_close += 1;
                        for m in [*i, *j] {
                            if let Some(x) = near.get_mut(m as usize) {
                                *x = true;
                            }
                        }
                    }
                }
            }
        }
    }
    (n_close, skipped.iter().filter(|s| **s).count())
}

/// Runs the `-allGeometry` checks, with the cell centres and volumes of the default checks and
/// the directions the mesh solves in.
pub fn all_geometry(mesh: &PolyMesh, centres: &[V], vols: &[f64], directions: [bool; 3]) -> AllGeometry {
    let mut g = AllGeometry::default();
    let n_points = mesh.n_points();
    let labels: usize = (0..mesh.n_faces()).map(|i| mesh.face(i).map_or(0, <[u32]>::len)).sum();
    let mut budget = Budget(WORK_PER_LABEL.saturating_mul((labels + 2 * mesh.n_faces()) as u64).max(1 << 20));
    // Bounding box of the finite points (`mesh.bounds()`).
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    let mut any = false;
    for p in mesh.points.iter().filter(|p| p.iter().all(|x| x.is_finite())) {
        any = true;
        for ((l, h), x) in lo.iter_mut().zip(hi.iter_mut()).zip(p) {
            (*l, *h) = (l.min(*x), h.max(*x));
        }
    }
    let span = if any { sub(hi, lo) } else { [0.0; 3] };
    let min_dist_sqr = dot(scale(NEAR_FRACTION, span), scale(NEAR_FRACTION, span));
    g.near_distance = min_dist_sqr.sqrt();
    let lengths_checked = min_dist_sqr.is_finite();

    let max_sin = MAX_CONCAVE_DEG.to_radians().sin();
    let mut short = vec![false; n_points];
    let (mut min_len_sqr, mut max_len_sqr) = (GREAT * GREAT, -GREAT * GREAT);
    let (mut max_edge_sin, mut min_flat, mut sum_flat, mut n_flat) = (0.0f64, GREAT, 0.0, 0usize);
    let (mut min_weight, mut sum_weight) = (GREAT, 0.0);
    let (mut min_ratio, mut sum_ratio) = (GREAT, 0.0);
    let mut ps: Vec<V> = Vec::new();
    for i in 0..mesh.n_faces() {
        face_points_into(mesh, i, &mut ps);
        let (a, fc) = area_centre(&ps);
        let face = mesh.face(i).unwrap_or(&[]);
        // checkEdgeLength
        for (k, p) in ps.iter().enumerate() {
            let next = ps.get((k + 1) % ps.len()).unwrap_or(p);
            let d = sub(*next, *p);
            let len_sqr = dot(d, d);
            if lengths_checked && len_sqr < min_dist_sqr {
                for l in [face.get(k), face.get((k + 1) % face.len().max(1))].into_iter().flatten() {
                    if let Some(s) = short.get_mut(*l as usize) {
                        *s = true;
                    }
                }
            }
            min_len_sqr = min_len_sqr.min(len_sqr);
            max_len_sqr = max_len_sqr.max(len_sqr);
        }
        // checkFaceAngles
        let sin = concavity(&ps, a, max_sin);
        max_edge_sin = max_edge_sin.max(sin);
        if sin > SMALL {
            g.concave_faces.add(i);
        }
        // checkFaceFlatness
        let area = mag(a);
        if ps.len() > 3 && area > V_SMALL {
            let f = if area > ROOT_V_SMALL { flatness(&ps, a, fc) } else { 1.0 };
            sum_flat += f;
            n_flat += 1;
            min_flat = min_flat.min(f);
            if f < WARN_FLATNESS {
                g.warped_faces.add(i);
            }
        }
        // checkFaceWeight, checkVolRatio, checkFaceTets
        let own = mesh.owner.get(i).map(|o| *o as usize);
        let nei = mesh.neighbour.get(i).map(|n| *n as usize);
        let (mut weight, mut ratio) = (1.0, 1.0);
        let own_c = own.and_then(|o| centres.get(o).copied());
        let nei_c = nei.and_then(|n| centres.get(n).copied());
        if let (Some(oc), Some(nc), Some(vo), Some(vn)) =
            (own_c, nei_c, own.and_then(|o| vols.get(o)), nei.and_then(|n| vols.get(n)))
        {
            let d_own = dot(a, sub(fc, oc)).abs();
            let d_nei = dot(a, sub(nc, fc)).abs();
            weight = d_nei.min(d_own) / (d_nei + d_own + V_SMALL);
            ratio = vo.min(*vn) / (vo.max(*vn) + V_SMALL);
        }
        if weight < MIN_WEIGHT {
            g.low_weight_faces.add(i);
        }
        if ratio < MIN_VOLUME_RATIO {
            g.low_volume_ratio_faces.add(i);
        }
        // The minimum and average are over the internal faces (`getInternalOrMasterFaces`).
        if nei.is_some() {
            min_weight = min_weight.min(weight);
            sum_weight += weight;
            min_ratio = min_ratio.min(ratio);
            sum_ratio += ratio;
        }
        if let Some(oc) = own_c {
            let (errors, searched) = face_tet_errors(&ps, fc, oc, nei_c, &mut budget);
            if errors > 0 {
                g.low_quality_tets.add_n(i, errors);
            }
            g.unchecked += usize::from(!searched);
        }
    }
    if mesh.n_faces() > 0 {
        g.min_edge_length = min_len_sqr.sqrt();
        g.max_edge_length = max_len_sqr.max(0.0).sqrt();
    }
    let n_internal = mesh.n_internal_faces();
    (g.min_weight, g.average_weight, g.min_volume_ratio, g.average_volume_ratio) = if n_internal > 0 {
        let n = n_internal as f64;
        (min_weight, sum_weight / n, min_ratio, sum_ratio / n)
    } else {
        (1.0, 1.0, 1.0, 1.0)
    };
    g.max_concave_angle = max_edge_sin.min(1.0).asin().to_degrees();
    (g.min_flatness, g.average_flatness) = if n_flat > 0 { (min_flat, sum_flat / n_flat as f64) } else { (1.0, 1.0) };
    let n_short = short.iter().filter(|s| **s).count();
    g.short_edge_points = points_found(&short, n_short);
    drop(short);

    if lengths_checked && min_dist_sqr > 0.0 {
        let mut near = vec![false; n_points];
        let (n_close, skipped) = near_pairs(&mesh.points, lo, min_dist_sqr, &mut near, &mut budget);
        g.near_points = points_found(&near, n_close);
        g.unchecked += skipped;
    }

    // checkCellDeterminant, checkConcaveCells
    g.geometric_directions = geometric_directions(mesh, directions);
    let n_dims = g.geometric_directions.iter().filter(|d| **d).count();
    let two_d = g.geometric_directions.iter().rposition(|d| !*d).unwrap_or(2);
    let (starts, faces) = cell_faces(mesh);
    let (mut min_det, mut sum_det) = (GREAT, 0.0);
    let mut buf: Vec<CellFace> = Vec::new();
    for c in 0..mesh.n_cells() {
        let range = starts.get(c).copied().unwrap_or(0)..starts.get(c + 1).copied().unwrap_or(0);
        buf.clear();
        for f in faces.get(range).unwrap_or(&[]) {
            let f = *f as usize;
            face_points_into(mesh, f, &mut ps);
            let (a, fc) = area_centre(&ps);
            let owned = mesh.owner.get(f).is_some_and(|o| *o as usize == c);
            buf.push((a, fc, f < n_internal, owned));
        }
        let det = determinant(&buf, n_dims, two_d);
        if det < WARN_DETERMINANT {
            g.underdetermined_cells.add(c);
        }
        min_det = min_det.min(det);
        sum_det += det;
        let k = buf.len() as u64;
        if buf.len() > MAX_PAIRWISE || !budget.take(k * k) {
            g.unchecked += 1;
        } else if concave_cell(&buf) {
            g.concave_cells.add(c);
        }
    }
    if mesh.n_cells() > 0 {
        (g.min_determinant, g.average_determinant) = (min_det, sum_det / mesh.n_cells() as f64);
    }
    g
}
