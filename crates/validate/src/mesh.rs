//! Mesh quality: the geometric checks `checkMesh` runs by default, computed from the mesh
//! itself, with `checkMesh`'s thresholds.
//!
//! Ported from OpenFOAM-12 (GPL-3.0-or-later, as this crate):
//! - face areas and centres: `src/OpenFOAM/meshes/meshShapes/face/faceTemplates.C`
//!   (`face::areaAndCentre`), Copyright (C) 2011-2023 OpenFOAM Foundation;
//! - cell centres and volumes: `src/OpenFOAM/meshes/primitiveMesh/primitiveMeshCellCentresAndVols.C`,
//!   Copyright (C) 2011-2018 OpenFOAM Foundation;
//! - the checks: `src/meshCheck/primitiveMeshCheck/primitiveMeshCheck.C` (`faceSkewness`,
//!   `boundaryFaceSkewness`, `faceOrthogonality`, `facePyramidVolume`, `cellClosedness`,
//!   `checkClosedBoundary`, `checkClosedCells`, `checkFaceAreas`, `checkCellVolumes`,
//!   `checkFacePyramids`), Copyright (C) 2011-2024 OpenFOAM Foundation;
//!   `src/meshCheck/polyMeshCheck/polyMeshCheck.C` (`checkFaceOrthogonality`,
//!   `checkFaceSkewness`), Copyright (C) 2012-2024 OpenFOAM Foundation; and the thresholds of
//!   `src/meshCheck/checkGeometry.C`, Copyright (C) 2011-2024 OpenFOAM Foundation; the
//!   directions a mesh solves in: `src/OpenFOAM/meshes/polyMesh/polyMesh.C`
//!   (`calcDirections`), Copyright (C) 2011-2024 OpenFOAM Foundation.
//!
//! The test `matches_checkmesh_on_the_3x3_case` compares the results with `checkMesh`'s.
//!
//! `checkMesh` stays the authority: this is a preview for meshes nobody checked, and the
//! faces and cells it finds are what the 3D view highlights.
//!
//! Faces of coupled patches (`cyclic`, `processor`) are taken as boundary faces: their partner
//! cell is not followed, so their non-orthogonality is not computed and their skewness is the
//! boundary formula. Wedge directions are not removed from the aspect ratio.

use umcf_foam_mesh::PolyMesh;

type V = [f64; 3];

// OpenFOAM-12's `vSmall`, `rootVSmall` and `small` (src/OpenFOAM/primitives/Scalar/doubleScalar/doubleScalar.H).
const V_SMALL: f64 = f64::MIN_POSITIVE;
/// sqrt(f64::MIN_POSITIVE).
const ROOT_V_SMALL: f64 = 1.491_668_146_240_041_3e-154;
const SMALL: f64 = f64::EPSILON;

/// `checkMesh`'s thresholds (`checkGeometry.C`, `functionObjects/checkMesh`).
pub const NON_ORTH_THRESHOLD_DEG: f64 = 70.0;
pub const SKEW_THRESHOLD: f64 = 4.0;
pub const ASPECT_THRESHOLD: f64 = 1000.0;
pub const CLOSED_THRESHOLD: f64 = 1.0e-6;
/// `checkFacePyramids(mesh, true, -small, …)`.
pub const MIN_PYRAMID_VOLUME: f64 = -SMALL;

/// How many face or cell labels a [`Quality`] keeps per problem; the count is always complete.
pub const MAX_LISTED: usize = 1000;

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(s: f64, a: V) -> V {
    [s * a[0], s * a[1], s * a[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn mag(a: V) -> f64 {
    dot(a, a).sqrt()
}
fn cmpt_mag(a: V) -> V {
    [a[0].abs(), a[1].abs(), a[2].abs()]
}

/// The points of face `i` into `buf` (labels were checked when the mesh was read).
fn face_points_into(mesh: &PolyMesh, i: usize, buf: &mut Vec<V>) {
    buf.clear();
    buf.extend(mesh.face(i).unwrap_or(&[]).iter().filter_map(|p| mesh.points.get(*p as usize).copied()));
}

/// Area vector and centre of a face (`face::areaAndCentre`).
pub fn face_area_centre(mesh: &PolyMesh, i: usize) -> (V, V) {
    let mut buf = Vec::new();
    face_points_into(mesh, i, &mut buf);
    area_centre(&buf)
}

fn area_centre(ps: &[V]) -> (V, V) {
    if let [a, b, c] = ps {
        return (scale(0.5, cross(sub(*b, *a), sub(*c, *a))), scale(1.0 / 3.0, add(add(*a, *b), *c)));
    }
    if ps.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let n = ps.len();
    let p_avg = scale(1.0 / n as f64, ps.iter().fold([0.0; 3], |s, p| add(s, *p)));
    let at = |k: usize| ps.get(k % n).copied().unwrap_or(p_avg);
    let tri = |k: usize| {
        let (p, next) = (at(k), at(k + 1));
        (p, next, cross(sub(next, p), sub(p_avg, p)))
    };
    let sum_a = (0..n).fold([0.0; 3], |s, k| add(s, tri(k).2));
    let m = mag(sum_a);
    let hat = if m > V_SMALL { scale(1.0 / m, sum_a) } else { [0.0; 3] };
    let (mut sum_an, mut sum_anc) = (0.0, [0.0; 3]);
    for k in 0..n {
        let (p, next, a) = tri(k);
        let an = dot(a, hat);
        sum_an += an;
        sum_anc = add(sum_anc, scale(an, add(add(p, next), p_avg)));
    }
    let centre = if sum_an > V_SMALL { scale(1.0 / (3.0 * sum_an), sum_anc) } else { p_avg };
    (scale(0.5, sum_a), centre)
}

/// Cell centres and volumes (`primitiveMesh::makeCellCentresAndVols`). Face geometry is
/// computed on the fly rather than stored: a 16-million-cell mesh has 50 million faces.
pub fn cell_centres_volumes(mesh: &PolyMesh) -> (Vec<V>, Vec<f64>) {
    let n_cells = mesh.n_cells();
    let (own, nei) = (&mesh.owner, &mesh.neighbour);
    let mut c_est = vec![[0.0; 3]; n_cells];
    let mut n_faces = vec![0u32; n_cells];
    let mut buf = Vec::new();
    let mut geometry = |i: usize| {
        face_points_into(mesh, i, &mut buf);
        area_centre(&buf)
    };
    for i in 0..mesh.n_faces() {
        let (_, c) = geometry(i);
        for cell in [own.get(i), nei.get(i)].into_iter().flatten() {
            if let (Some(e), Some(k)) = (c_est.get_mut(*cell as usize), n_faces.get_mut(*cell as usize)) {
                *e = add(*e, c);
                *k += 1;
            }
        }
    }
    for (e, k) in c_est.iter_mut().zip(&n_faces) {
        if *k > 0 {
            *e = scale(1.0 / f64::from(*k), *e);
        }
    }
    let mut centres = vec![[0.0; 3]; n_cells];
    let mut vols = vec![0.0; n_cells];
    for i in 0..mesh.n_faces() {
        let (a, c) = geometry(i);
        if let Some(o) = own.get(i).map(|o| *o as usize)
            && let Some(est) = c_est.get(o).copied()
        {
            let pyr3 = dot(a, sub(c, est));
            let pc = add(scale(0.75, c), scale(0.25, est));
            if let (Some(cc), Some(v)) = (centres.get_mut(o), vols.get_mut(o)) {
                *cc = add(*cc, scale(pyr3, pc));
                *v += pyr3;
            }
        }
        if let Some(nb) = nei.get(i).map(|n| *n as usize)
            && let Some(est) = c_est.get(nb).copied()
        {
            let pyr3 = dot(a, sub(est, c));
            let pc = add(scale(0.75, c), scale(0.25, est));
            if let (Some(cc), Some(v)) = (centres.get_mut(nb), vols.get_mut(nb)) {
                *cc = add(*cc, scale(pyr3, pc));
                *v += pyr3;
            }
        }
    }
    for ((cc, v), est) in centres.iter_mut().zip(vols.iter_mut()).zip(&c_est) {
        *cc = if v.abs() > V_SMALL { scale(1.0 / *v, *cc) } else { *est };
        *v /= 3.0;
    }
    (centres, vols)
}

/// Labels of the faces or cells with one problem: the first [`MAX_LISTED`], and how many.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub count: usize,
    /// How many faces or cells (`count` can be larger: two pyramids of one face).
    pub places: usize,
    pub first: Vec<u32>,
}

impl Found {
    fn add(&mut self, i: usize) {
        self.add_n(i, 1);
    }

    /// `n` problems at one face or cell (both pyramids of a face): counted `n` times, as
    /// `checkMesh` counts them, listed once.
    fn add_n(&mut self, i: usize, n: usize) {
        self.count += n;
        self.places += 1;
        if self.first.len() < MAX_LISTED {
            self.first.push(i as u32);
        }
    }
}

/// What the checks found, with `checkMesh`'s numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quality {
    pub n_cells: usize,
    pub n_faces: usize,
    /// The directions the mesh solves in (`false` for the normal of `empty` patches).
    pub directions: [bool; 3],
    /// `sum(Sf)/sum(|Sf|)` over the boundary, per component: zero for a closed boundary.
    pub boundary_openness: V,
    pub max_openness: f64,
    pub open_cells: Found,
    pub max_aspect_ratio: f64,
    pub high_aspect_cells: Found,
    pub min_face_area: f64,
    pub max_face_area: f64,
    pub zero_area_faces: Found,
    pub min_volume: f64,
    pub max_volume: f64,
    pub total_volume: f64,
    pub negative_volume_cells: Found,
    /// Degrees; over the internal faces.
    pub max_non_orthogonality: f64,
    pub average_non_orthogonality: f64,
    /// Above 70°.
    pub severe_non_orthogonal_faces: Found,
    /// 90° or more: the owner and neighbour centres are on the wrong sides of the face.
    pub non_orthogonality_errors: Found,
    pub max_skewness: f64,
    pub skew_faces: Found,
    /// Faces whose pyramid to the owner (or neighbour) centre has a negative volume; the
    /// count is of pyramids, as `checkMesh` counts.
    pub wrong_pyramids: Found,
    /// Points with a coordinate that is not a number or infinite: every other number is then
    /// meaningless.
    pub non_finite_points: Found,
    /// Faces whose area or centre, and cells whose volume or centre, overflowed (finite points
    /// too far apart, `1e300`).
    pub non_finite_faces: Found,
    pub non_finite_cells: Found,
}

/// `polyMesh::calcDirections` for `empty` patches: a direction is solved unless the empty
/// faces' area vectors (summed component by component, in magnitude) point along it.
fn directions(mesh: &PolyMesh) -> [bool; 3] {
    let mut sum = [0.0; 3];
    let mut any = false;
    for p in mesh.boundary.iter().filter(|p| p.kind == "empty" && p.n_faces > 0) {
        any = true;
        for i in p.faces() {
            let (a, _) = face_area_centre(mesh, i);
            sum = add(sum, cmpt_mag(a));
        }
    }
    let m = mag(sum);
    if !any || m <= V_SMALL {
        return [true; 3];
    }
    let n = scale(1.0 / m, sum);
    [n[0] <= 1e-6, n[1] <= 1e-6, n[2] <= 1e-6]
}

/// `pyramidPointFaceRef(face, apex).mag()`: a third of the area vector dotted with the height.
fn pyramid_volume(apex: V, area: V, centre: V) -> f64 {
    dot(area, sub(apex, centre)) / 3.0
}

/// Runs the checks (see the module documentation).
pub fn quality(mesh: &PolyMesh) -> Quality {
    let (centres, vols) = cell_centres_volumes(mesh);
    let n_cells = mesh.n_cells();
    let n_internal = mesh.n_internal_faces();
    let mut q = Quality {
        n_cells,
        n_faces: mesh.n_faces(),
        directions: directions(mesh),
        min_face_area: f64::INFINITY,
        max_face_area: f64::NEG_INFINITY,
        min_volume: f64::INFINITY,
        max_volume: f64::NEG_INFINITY,
        ..Quality::default()
    };
    for (i, p) in mesh.points.iter().enumerate() {
        if !p.iter().all(|x| x.is_finite()) {
            q.non_finite_points.add(i);
        }
    }
    let cos_severe = NON_ORTH_THRESHOLD_DEG.to_radians().cos();
    let mut ps: Vec<V> = Vec::new();
    let mut sum_closed = vec![[0.0; 3]; n_cells];
    let mut sum_mag_closed = vec![[0.0; 3]; n_cells];
    let (mut boundary_sum, mut boundary_mag) = ([0.0; 3], 0.0);
    let (mut min_ortho, mut sum_ortho) = (f64::INFINITY, 0.0);
    let mut max_skew: f64 = 0.0;
    for i in 0..mesh.n_faces() {
        face_points_into(mesh, i, &mut ps);
        let (a, fc) = area_centre(&ps);
        if !a.iter().chain(&fc).all(|x| x.is_finite()) || !mag(a).is_finite() {
            q.non_finite_faces.add(i);
        }
        let area = mag(a);
        if area < V_SMALL {
            q.zero_area_faces.add(i);
        }
        q.min_face_area = q.min_face_area.min(area);
        q.max_face_area = q.max_face_area.max(area);
        let Some(o) = mesh.owner.get(i).map(|o| *o as usize) else { continue };
        let Some(oc) = centres.get(o).copied() else { continue };
        if let (Some(s), Some(m)) = (sum_closed.get_mut(o), sum_mag_closed.get_mut(o)) {
            *s = add(*s, a);
            *m = add(*m, cmpt_mag(a));
        }
        // Pyramid to the owner centre: the face points away from it.
        let mut wrong = usize::from(-pyramid_volume(oc, a, fc) < MIN_PYRAMID_VOLUME);
        if let Some(nb) = mesh.neighbour.get(i).map(|n| *n as usize)
            && let Some(nc) = centres.get(nb).copied()
        {
            if let (Some(s), Some(m)) = (sum_closed.get_mut(nb), sum_mag_closed.get_mut(nb)) {
                *s = sub(*s, a);
                *m = add(*m, cmpt_mag(a));
            }
            wrong += usize::from(pyramid_volume(nc, a, fc) < MIN_PYRAMID_VOLUME);
            // Non-orthogonality (faceOrthogonality).
            let d = sub(nc, oc);
            let ortho = dot(d, a) / (mag(d) * area + ROOT_V_SMALL);
            if ortho < cos_severe {
                if ortho > SMALL {
                    q.severe_non_orthogonal_faces.add(i);
                } else {
                    q.non_orthogonality_errors.add(i);
                }
            }
            min_ortho = min_ortho.min(ortho);
            sum_ortho += ortho;
            // Skewness (faceSkewness).
            let cpf = sub(fc, oc);
            let s = skewness(&ps, a, fc, cpf, d, 0.2);
            max_skew = max_skew.max(s);
            if s > SKEW_THRESHOLD {
                q.skew_faces.add(i);
            }
        } else {
            boundary_sum = add(boundary_sum, a);
            boundary_mag += area;
            // boundaryFaceSkewness
            let cpf = sub(fc, oc);
            let normal = scale(1.0 / (area + ROOT_V_SMALL), a);
            let d = scale(dot(normal, cpf), normal);
            let s = skewness(&ps, a, fc, cpf, d, 0.4);
            max_skew = max_skew.max(s);
            if s > SKEW_THRESHOLD {
                q.skew_faces.add(i);
            }
        }
        if wrong > 0 {
            q.wrong_pyramids.add_n(i, wrong);
        }
    }
    q.max_skewness = max_skew;
    if n_internal > 0 {
        let deg = |c: f64| c.clamp(-1.0, 1.0).acos().to_degrees();
        q.max_non_orthogonality = deg(min_ortho);
        q.average_non_orthogonality = deg(sum_ortho / n_internal as f64);
    }
    q.boundary_openness = scale(1.0 / (boundary_mag + V_SMALL), boundary_sum);
    // cellClosedness
    let dims = q.directions.iter().filter(|d| **d).count();
    for (c, ((s, m), v)) in sum_closed.iter().zip(&sum_mag_closed).zip(&vols).enumerate() {
        let openness = s.iter().zip(m).map(|(s, m)| s.abs() / (m + ROOT_V_SMALL)).fold(0.0, f64::max);
        q.max_openness = q.max_openness.max(openness);
        if openness > CLOSED_THRESHOLD {
            q.open_cells.add(c);
        }
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for (x, solved) in m.iter().zip(q.directions) {
            if solved {
                (lo, hi) = (lo.min(*x), hi.max(*x));
            }
        }
        let mut aspect = if dims == 0 { 0.0 } else { hi / (lo + ROOT_V_SMALL) };
        if dims == 3 {
            let vol = v.max(ROOT_V_SMALL);
            aspect = aspect.max(m.iter().sum::<f64>() / 6.0 / vol.powf(2.0 / 3.0));
        }
        q.max_aspect_ratio = q.max_aspect_ratio.max(aspect);
        if aspect > ASPECT_THRESHOLD {
            q.high_aspect_cells.add(c);
        }
        if *v < V_SMALL {
            q.negative_volume_cells.add(c);
        }
        if !v.is_finite() || !centres.get(c).is_some_and(|x| x.iter().all(|x| x.is_finite())) {
            q.non_finite_cells.add(c);
        }
        q.min_volume = q.min_volume.min(*v);
        q.max_volume = q.max_volume.max(*v);
        q.total_volume += v;
    }
    if n_cells == 0 {
        (q.min_volume, q.max_volume) = (0.0, 0.0);
    }
    if q.n_faces == 0 {
        (q.min_face_area, q.max_face_area) = (0.0, 0.0);
    }
    q
}

/// `faceSkewness` / `boundaryFaceSkewness`: `factor` is 0.2 inside, 0.4 on the boundary.
fn skewness(ps: &[V], a: V, fc: V, cpf: V, d: V, factor: f64) -> f64 {
    let sv = sub(cpf, scale(dot(a, cpf) / (dot(a, d) + ROOT_V_SMALL), d));
    let sv_hat = scale(1.0 / (mag(sv) + ROOT_V_SMALL), sv);
    let mut fd = factor * mag(d) + ROOT_V_SMALL;
    for p in ps {
        fd = fd.max(dot(sv_hat, sub(*p, fc)).abs());
    }
    mag(sv) / fd
}

/// `1000.4`, `2046`, or `5.9256e102` for a degenerate cell.
fn number(x: f64) -> String {
    if x.abs() < 1e4 {
        format!("{x:.1}")
    } else if x.abs() < 1e6 {
        format!("{x:.0}")
    } else {
        format!("{x:.4e}")
    }
}

fn listed(f: &Found) -> String {
    let shown: Vec<String> = f.first.iter().take(10).map(u32::to_string).collect();
    let more = if f.places > shown.len() { ", …" } else { "" };
    format!("{}{more}", shown.join(", "))
}

/// The problems of `q` as diagnostics, for the mesh of `region` (`""` for a single-region
/// case). The levels follow what each problem does to a run: an error makes the solver fail
/// or compute nonsense, a warning (what `checkMesh` reports with `*`) hurts accuracy or
/// convergence.
pub fn diagnostics(q: &Quality, region: &str) -> Vec<crate::Diagnostic> {
    use crate::{Diagnostic, Level};
    let path = umcf_foam_mesh::mesh_dir(region);
    let mut out = Vec::new();
    let mut push = |level, code, message: String| {
        let d = Diagnostic::new(level, code, &path, message);
        out.push(if region.is_empty() { d } else { d.region(region) });
    };
    let (p, fa, ce) = (&q.non_finite_points, &q.non_finite_faces, &q.non_finite_cells);
    if p.count + fa.count + ce.count > 0 {
        let mut parts = Vec::new();
        if p.count > 0 {
            parts.push(format!(
                "{} points with a coordinate that is not a number or infinite (points {})",
                p.count,
                listed(p)
            ));
        }
        if fa.count > 0 {
            parts.push(format!("{} faces whose area or centre overflows (faces {})", fa.count, listed(fa)));
        }
        if ce.count > 0 {
            parts.push(format!("{} cells whose volume or centre overflows (cells {})", ce.count, listed(ce)));
        }
        push(
            Level::Error,
            "mesh.non-finite",
            format!("the mesh has {}, so no other check can be trusted", parts.join(", ")),
        );
    }
    let open = q.boundary_openness.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if open > CLOSED_THRESHOLD {
        push(
            Level::Warning,
            "mesh.open-boundary",
            format!(
                "the boundary is not closed: the face areas add up to ({:.3e} {:.3e} {:.3e}) of their total (OpenFOAM-8's checkMesh fails above {CLOSED_THRESHOLD:e})",
                q.boundary_openness[0], q.boundary_openness[1], q.boundary_openness[2]
            ),
        );
    }
    let f = &q.open_cells;
    if f.count > 0 {
        push(
            Level::Error,
            "mesh.open-cells",
            format!(
                "{} cells are not closed (openness up to {:.3e}, above {CLOSED_THRESHOLD:e}): cells {}",
                f.count,
                q.max_openness,
                listed(f)
            ),
        );
    }
    let f = &q.zero_area_faces;
    if f.count > 0 {
        push(Level::Error, "mesh.zero-area", format!("{} faces have zero area: faces {}", f.count, listed(f)));
    }
    let f = &q.negative_volume_cells;
    if f.count > 0 {
        push(
            Level::Error,
            "mesh.negative-volume",
            format!(
                "{} cells have zero or negative volume (down to {:.3e} m³): cells {}",
                f.count,
                q.min_volume,
                listed(f)
            ),
        );
    }
    let f = &q.wrong_pyramids;
    if f.count > 0 {
        push(
            Level::Error,
            "mesh.face-pyramids",
            format!(
                "{} pyramids on {} faces are negative (the faces are wrongly oriented or twisted): faces {}",
                f.count,
                f.places,
                listed(f)
            ),
        );
    }
    let f = &q.non_orthogonality_errors;
    if f.count > 0 {
        push(
            Level::Error,
            "mesh.non-orthogonality-error",
            format!(
                "{} faces are 90° or more non-orthogonal (the cell centres are on the wrong side): faces {}",
                f.count,
                listed(f)
            ),
        );
    }
    let f = &q.severe_non_orthogonal_faces;
    if f.count > 0 {
        push(
            Level::Warning,
            "mesh.non-orthogonal",
            format!(
                "{} faces are more than {NON_ORTH_THRESHOLD_DEG}° non-orthogonal (max {:.1}°, average {:.1}°): use non-orthogonal correctors and limited schemes; faces {}",
                f.count,
                q.max_non_orthogonality,
                q.average_non_orthogonality,
                listed(f)
            ),
        );
    }
    let f = &q.skew_faces;
    if f.count > 0 {
        push(
            Level::Warning,
            "mesh.skewness",
            format!(
                "{} faces are highly skewed (max {:.2}, above {SKEW_THRESHOLD}): faces {}",
                f.count,
                q.max_skewness,
                listed(f)
            ),
        );
    }
    let f = &q.high_aspect_cells;
    if f.count > 0 {
        push(
            Level::Warning,
            "mesh.aspect-ratio",
            format!(
                "{} cells have an aspect ratio above {ASPECT_THRESHOLD} (max {}): cells {}",
                f.count,
                number(q.max_aspect_ratio),
                listed(f)
            ),
        );
    }
    out
}

/// `mesh.unreadable`: the mesh of `region` could not be read (`why` says how).
pub fn unreadable(region: &str, why: &str) -> crate::Diagnostic {
    let d = crate::Diagnostic::new(
        crate::Level::Error,
        "mesh.unreadable",
        umcf_foam_mesh::mesh_dir(region),
        format!("the mesh could not be read: {why}"),
    );
    if region.is_empty() { d } else { d.region(region) }
}

/// What a set holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOf {
    Faces,
    Cells,
    Points,
}

/// The sets `checkMesh` writes (OpenFOAM-8 and 12, `checkGeometry.C` and `checkTopology.C`;
/// OpenFOAM-8 on every run, OpenFOAM-12 with `-writeSets`), with what each holds. Other sets in `polyMesh/sets` (from `topoSet`,
/// `snappyHexMesh`) are not mesh problems.
pub const CHECKMESH_SETS: &[(&str, SetOf, &str)] = &[
    ("nonClosedCells", SetOf::Cells, "cells that are not closed"),
    ("highAspectRatioCells", SetOf::Cells, "cells with an aspect ratio above 1000"),
    ("zeroVolumeCells", SetOf::Cells, "cells with zero or negative volume"),
    ("underdeterminedCells", SetOf::Cells, "cells whose faces leave a direction undetermined"),
    ("concaveCells", SetOf::Cells, "concave cells"),
    ("illegalCells", SetOf::Cells, "cells with too few faces"),
    ("zipUpCells", SetOf::Cells, "cells whose edges don't close"),
    ("oneInternalFaceCells", SetOf::Cells, "cells with one internal face"),
    ("twoInternalFacesCells", SetOf::Cells, "cells with two internal faces"),
    ("zeroAreaFaces", SetOf::Faces, "faces with zero area"),
    ("nonOrthoFaces", SetOf::Faces, "faces more than 70° non-orthogonal"),
    ("wrongOrientedFaces", SetOf::Faces, "wrongly oriented faces (negative pyramids)"),
    ("skewFaces", SetOf::Faces, "faces with skewness above 4"),
    ("concaveFaces", SetOf::Faces, "concave faces"),
    ("warpedFaces", SetOf::Faces, "warped (non-flat) faces"),
    ("lowWeightFaces", SetOf::Faces, "faces with a low interpolation weight"),
    ("lowVolRatioFaces", SetOf::Faces, "faces between cells of very different volume"),
    ("lowQualityTetFaces", SetOf::Faces, "faces whose tetrahedral decomposition is poor"),
    ("upperTriangularFace", SetOf::Faces, "faces out of upper-triangular order"),
    ("outOfRangeFaces", SetOf::Faces, "faces with point labels out of range"),
    ("edgeFaces", SetOf::Faces, "faces with duplicate or illegal edges"),
    ("coupledFaces", SetOf::Faces, "coupled faces whose points don't match"),
    ("meshQualityFaces", SetOf::Faces, "faces failing the meshQualityDict criteria"),
    ("shortEdges", SetOf::Points, "points of very short edges"),
    ("nearPoints", SetOf::Points, "points very close to each other"),
    ("unusedPoints", SetOf::Points, "points no face uses"),
    ("nonAlignedEdges", SetOf::Points, "points of edges not aligned with the empty direction"),
    ("nonManifoldPoints", SetOf::Points, "points where the boundary is not manifold"),
    ("multiRegionPoints", SetOf::Points, "points shared by unconnected parts of the boundary"),
];

/// What a `checkMesh` log says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckMeshLog {
    /// The log file (case-relative).
    pub file: String,
    /// `-region`'s value; `""` for the default region.
    pub region: String,
    /// `Build : 12`.
    pub version: Option<String>,
    /// The command line (`Exec`).
    pub command: String,
    /// `Time = 3`: a run over several times (`-time 3,8`, `-allTime`) checks one mesh per
    /// time; each gets its own result. `None` when the log doesn't say.
    pub time: Option<String>,
    /// `Failed N mesh checks.`; `None` with `Mesh OK.` or when the log stops early.
    pub failed: Option<usize>,
    pub ok: bool,
    /// The failed checks (`***` lines).
    pub problems: Vec<String>,
    /// The remarks (`*` lines).
    pub notes: Vec<String>,
    /// The size of the mesh it checked (`Mesh stats`): `cells:`, `faces:`, `points:`.
    pub cells: Option<u64>,
    pub faces: Option<u64>,
    pub points: Option<u64>,
    /// `FOAM FATAL ERROR`: checkMesh stopped; the message's first line.
    pub fatal: Option<String>,
    /// This time had more than [`MAX_LOG_FINDINGS`] findings: the rest are not listed.
    pub truncated: bool,
    /// Failed checks (`***`) among the findings not listed.
    pub unlisted_problems: usize,
    /// A summary of times past [`MAX_LOG_RESULTS`], which are scanned without being kept: one
    /// per size of mesh they checked (`cells`, `faces`, `points`; unknown past
    /// [`MAX_SKIPPED_SIZES`] sizes).
    pub skipped: Option<Skipped>,
}

/// Times of a `checkMesh` log past [`MAX_LOG_RESULTS`], for one size of mesh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skipped {
    pub times: usize,
    /// Times with a failed check or a fatal error.
    pub failed: usize,
    /// Times that end without checkMesh's verdict (the log stops in them).
    pub unfinished: usize,
}

/// The most mesh sizes the skipped times of a log are summarized by.
pub const MAX_SKIPPED_SIZES: usize = 16;

/// One skipped time being scanned.
#[derive(Default)]
struct SkippedTime {
    sizes: [Option<u64>; 3],
    failed: bool,
    /// `Mesh OK.`, `Failed N mesh checks.` or a fatal error was seen.
    verdict: bool,
}

/// Adds a skipped time to the summaries in `out` (after the kept results, from `first`).
fn summarize(out: &mut Vec<CheckMeshLog>, first: usize, head: &CheckMeshLog, t: SkippedTime) {
    let [cells, faces, points] = t.sizes;
    let summaries = out.get(first..).map_or(0, <[CheckMeshLog]>::len);
    let same = |l: &CheckMeshLog| (l.cells, l.faces, l.points) == (cells, faces, points);
    let known = |l: &CheckMeshLog| l.cells.is_some() || l.faces.is_some() || l.points.is_some();
    let at = match out.iter().skip(first).position(same) {
        Some(k) => Some(first + k),
        // Past the sizes kept: an unknown size.
        None if summaries >= MAX_SKIPPED_SIZES => {
            out.iter().skip(first).position(|l| !known(l)).map(|k| first + k).or_else(|| {
                out.push(CheckMeshLog { skipped: Some(Skipped::default()), ..head.clone() });
                Some(out.len() - 1)
            })
        }
        None => {
            out.push(CheckMeshLog { cells, faces, points, skipped: Some(Skipped::default()), ..head.clone() });
            Some(out.len() - 1)
        }
    };
    if let Some(s) = at.and_then(|k| out.get_mut(k)).and_then(|l| l.skipped.as_mut()) {
        s.times += 1;
        s.failed += usize::from(t.failed);
        s.unfinished += usize::from(!t.verdict);
    }
}

/// The most results (times) read from one `checkMesh` log.
pub const MAX_LOG_RESULTS: usize = 1000;
/// The most failed checks and remarks read for one result (checkMesh makes a few dozen checks).
pub const MAX_LOG_FINDINGS: usize = 200;

/// Reads a `checkMesh` log: one result per time it checked (`Time = …`), or one for the whole
/// log. Empty when the text is not a `checkMesh` log (its `Exec` line runs something else).
pub fn parse_checkmesh_log(file: &str, text: &str) -> Vec<CheckMeshLog> {
    let mut head = CheckMeshLog { file: file.to_string(), ..CheckMeshLog::default() };
    let mut is_checkmesh = false;
    let mut out: Vec<CheckMeshLog> = Vec::new();
    // Past MAX_LOG_RESULTS: each time's mesh size and whether it failed, summarized from
    // index `first_skipped` of `out` on.
    let mut skipping: Option<SkippedTime> = None;
    let mut first_skipped = 0;
    let failure = |t: &str| {
        t.starts_with("***")
            || t.starts_with("--> FOAM FATAL")
            || t.strip_prefix("Failed ")
                .and_then(|r| r.strip_suffix(" mesh checks."))
                .is_some_and(|n| n.trim().parse::<u64>().is_ok_and(|n| n > 0))
    };
    for line in text.lines() {
        let t = line.trim();
        // A parallel run prefixes each line with its processor: `[0] `.
        let t = match t.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((n, rest)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => rest.trim(),
            _ => t,
        };
        if let Some(cur) = skipping.as_mut() {
            if t.starts_with("Time = ") {
                let done = std::mem::take(cur);
                summarize(&mut out, first_skipped, &head, done);
            } else if failure(t) {
                cur.failed = true;
                cur.verdict |= !t.starts_with("***");
            } else if t == "Mesh OK."
                || t.strip_prefix("Failed ").and_then(|r| r.strip_suffix(" mesh checks.")).is_some()
            {
                cur.verdict = true;
            } else {
                for (k, key) in ["cells:", "faces:", "points:"].iter().enumerate() {
                    if let Some(n) = t.strip_prefix(key).and_then(|r| r.trim().parse::<u64>().ok())
                        && let Some(slot @ None) = cur.sizes.get_mut(k)
                    {
                        *slot = Some(n);
                    }
                }
            }
            continue;
        }
        if let Some(cmd) = t.strip_prefix("Exec").map(str::trim_start).and_then(|r| r.strip_prefix(':')) {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            is_checkmesh = words.first().is_some_and(|w| w.rsplit('/').next() == Some("checkMesh"));
            if let Some(k) = words.iter().position(|w| *w == "-region") {
                head.region = words.get(k + 1).map(|s| s.to_string()).unwrap_or_default();
            }
            head.command = words.join(" ");
            continue;
        }
        if let Some(v) = t.strip_prefix("Build").map(str::trim_start).and_then(|r| r.strip_prefix(':')) {
            head.version = Some(v.trim().to_string());
            continue;
        }
        if let Some(time) = t.strip_prefix("Time = ") {
            // The banner section before the first `Time =` is not a time.
            if out.iter().filter(|l| l.time.is_some()).count() >= MAX_LOG_RESULTS {
                first_skipped = out.len();
                skipping = Some(SkippedTime::default());
                continue;
            }
            out.push(CheckMeshLog { time: Some(time.trim().to_string()), ..head.clone() });
            continue;
        }
        if out.is_empty() {
            out.push(head.clone());
        }
        let Some(cur) = out.last_mut() else { continue };
        if cur.fatal.as_deref() == Some("") && !t.is_empty() {
            cur.fatal = Some(t.to_string());
            continue;
        }
        let count = |key: &str| t.strip_prefix(key).and_then(|r| r.trim().parse::<u64>().ok());
        if t.starts_with("--> FOAM FATAL") {
            cur.fatal = Some(String::new());
        } else if let (None, Some(n)) = (cur.cells, count("cells:")) {
            cur.cells = Some(n);
        } else if let (None, Some(n)) = (cur.faces, count("faces:")) {
            cur.faces = Some(n);
        } else if let (None, Some(n)) = (cur.points, count("points:")) {
            cur.points = Some(n);
        } else if let Some(p) = t.strip_prefix("***") {
            if cur.problems.len() + cur.notes.len() < MAX_LOG_FINDINGS {
                cur.problems.push(p.trim().to_string());
            } else {
                cur.truncated = true;
                cur.unlisted_problems += 1;
            }
        } else if let Some(n) = t.strip_prefix('*') {
            if cur.problems.len() + cur.notes.len() < MAX_LOG_FINDINGS {
                cur.notes.push(n.trim().to_string());
            } else {
                cur.truncated = true;
            }
        } else if let Some(n) = t.strip_prefix("Failed ").and_then(|r| r.strip_suffix(" mesh checks.")) {
            cur.failed = n.trim().parse().ok();
        } else if t == "Mesh OK." {
            cur.ok = true;
        }
    }
    if let Some(last) = skipping {
        summarize(&mut out, first_skipped, &head, last);
    }
    if !is_checkmesh {
        return Vec::new();
    }
    // Sections before the header lines were read: give them the header.
    for l in &mut out {
        (l.region, l.version, l.command) = (head.region.clone(), head.version.clone(), head.command.clone());
    }
    // A section with nothing in it before the first `Time =` (the banner) is not a result.
    if out.len() > 1 {
        out.retain(|l| {
            l.time.is_some()
                || l.ok
                || l.failed.is_some()
                || l.fatal.is_some()
                || !l.problems.is_empty()
                || !l.notes.is_empty()
                || l.skipped.is_some()
        });
    }
    if out.is_empty() {
        out.push(head);
    }
    out
}

/// The failed checks (`***`) of `checkMesh` that are about quality, not correctness (the
/// messages of OpenFOAM-8 and 12): warnings, like the native checks. Every other `***`
/// message (wrong orientation, zero or negative volumes and areas, open cells,
/// non-orthogonality errors, invalid labels, boundary and zone errors, wedge errors) is an
/// error.
const CHECKMESH_WARNINGS: &[&str] = &[
    "High aspect ratio cells found",
    "Max skewness",
    "Faces with small interpolation weight",
    "Faces with small volume ratio",
    "Cells with small determinant",
    "Concave cells",
    "Number of edges not aligned",
    "Boundary openness",
];

fn checkmesh_level(line: &str) -> crate::Level {
    if CHECKMESH_WARNINGS.iter().any(|w| line.starts_with(w)) { crate::Level::Warning } else { crate::Level::Error }
}

/// Whether the native result found a problem.
type FoundNatively = fn(&Quality) -> bool;

/// The failed checks (`***`) of `checkMesh` that the native check also makes (its default
/// geometric checks), with whether the native result found the same problem.
const NATIVE_COUNTERPARTS: &[(&str, FoundNatively)] = &[
    ("High aspect ratio cells found", |q| q.high_aspect_cells.count > 0),
    ("Max skewness", |q| q.skew_faces.count > 0),
    ("Error in face pyramids", |q| q.wrong_pyramids.count > 0),
    ("Zero or negative cell volume", |q| q.negative_volume_cells.count > 0),
    ("Zero or negative face area", |q| q.zero_area_faces.count > 0),
    // Not "Open cells found, number of cells", the topological zip-up check.
    ("Open cells found, max cell openness", |q| q.open_cells.count > 0),
    ("Number of non-orthogonality errors", |q| q.non_orthogonality_errors.count > 0),
    ("Boundary openness", |q| q.boundary_openness.iter().any(|x| x.abs() > CLOSED_THRESHOLD)),
];

/// The findings of a `checkMesh` log, for its region: each failed check (`***`) as an error or
/// a warning (by what it breaks, as for the native checks), each remark (`*`) as a note. A log
/// that ends without its verdict, or was cut short, says so.
///
/// With `native` (the native check of the same mesh), a failed check the native check also
/// found is a note rather than counted twice; one it didn't find keeps its level.
pub fn checkmesh_log_diagnostics(log: &CheckMeshLog, native: Option<&Quality>) -> Vec<crate::Diagnostic> {
    use crate::{Diagnostic, Level};
    let version = log.version.as_deref().map_or(String::new(), |v| format!(", OpenFOAM-{v}"));
    let time = log.time.as_deref().map_or(String::new(), |t| format!(", time {t}"));
    let source = format!("checkMesh ({}{version}{time})", log.file);
    let mut out = Vec::new();
    let mut push = |level, code, message: String| {
        let d = Diagnostic::new(level, code, &log.file, message);
        out.push(if log.region.is_empty() { d } else { d.region(&log.region) });
    };
    for p in &log.problems {
        let counterpart = NATIVE_COUNTERPARTS.iter().find(|(n, _)| p.starts_with(n)).map(|(_, found)| found);
        match (native, counterpart) {
            (Some(q), Some(found)) if found(q) => push(
                Level::Info,
                "mesh.checkmesh",
                format!("{source}: {p} (the native check of this mesh reports it too)"),
            ),
            (Some(_), Some(_)) => push(
                checkmesh_level(p),
                "mesh.checkmesh",
                format!("{source}: {p} (the native check of this mesh doesn't find it)"),
            ),
            _ => push(checkmesh_level(p), "mesh.checkmesh", format!("{source}: {p}")),
        }
    }
    for n in &log.notes {
        push(Level::Info, "mesh.checkmesh", format!("{source}: {n}"));
    }
    if log.problems.is_empty()
        && let Some(n) = log.failed
    {
        push(Level::Warning, "mesh.checkmesh", format!("{source}: failed {n} mesh checks"));
    }
    if let Some(s) = log.skipped {
        // Times past MAX_LOG_RESULTS: only whether they failed is known.
        let source = format!("checkMesh ({}{version}, times past the first {MAX_LOG_RESULTS})", log.file);
        // Past MAX_SKIPPED_SIZES sizes (or without mesh stats), the mesh is unknown.
        let unknown = log.cells.is_none() && log.faces.is_none() && log.points.is_none();
        let failed = format!(
            "{} of these {} times failed a check or stopped with a fatal error (not listed): run checkMesh on fewer times (-time) to see them",
            s.failed, s.times
        );
        if s.failed > 0 && unknown {
            push(
                Level::Warning,
                "mesh.checkmesh-unmatched",
                format!(
                    "{source}: {failed}; their mesh size isn't known (checkMesh prints it only when the topology changes: these are usually times with moved points), so they may be of another mesh"
                ),
            );
        } else if s.failed > 0 {
            push(Level::Error, "mesh.checkmesh-incomplete", format!("{source}: {failed}"));
        }
        if s.unfinished > 0 {
            push(
                Level::Warning,
                "mesh.checkmesh",
                format!("{source}: {} of these {} times stop before checkMesh's verdict", s.unfinished, s.times),
            );
        }
        if s.failed == 0 && s.unfinished == 0 {
            push(
                Level::Info,
                "mesh.checkmesh",
                format!("{source}: none of these {} times failed (not listed)", s.times),
            );
        }
        return out;
    }
    if log.truncated {
        if log.unlisted_problems > 0 {
            push(
                Level::Error,
                "mesh.checkmesh-incomplete",
                format!(
                    "{source}: more than {MAX_LOG_FINDINGS} findings; {} more failed checks are not listed",
                    log.unlisted_problems
                ),
            );
        } else {
            push(
                Level::Info,
                "mesh.checkmesh",
                format!("{source}: more than {MAX_LOG_FINDINGS} findings; the remarks past them are not listed"),
            );
        }
    }
    if let Some(f) = &log.fatal {
        push(Level::Error, "mesh.checkmesh", format!("{source}: checkMesh stopped with a fatal error: {f}"));
    } else if !log.ok && log.failed.is_none() {
        push(Level::Warning, "mesh.checkmesh", format!("{source}: the log stops before checkMesh's verdict"));
    }
    out
}

/// `mesh.stale-set`: a `checkMesh` set names faces, cells or points the mesh doesn't have, so
/// it was written for another mesh.
pub fn stale_set(region: &str, name: &str, largest: u32, size: usize, of: SetOf) -> crate::Diagnostic {
    let what = match of {
        SetOf::Faces => "faces",
        SetOf::Cells => "cells",
        SetOf::Points => "points",
    };
    let d = crate::Diagnostic::new(
        crate::Level::Warning,
        "mesh.stale-set",
        format!("{}/sets/{name}", umcf_foam_mesh::mesh_dir(region)),
        format!(
            "the set `{name}` names {what} up to {largest}, the mesh has {size}: it is from another mesh (run checkMesh -writeSets again)"
        ),
    );
    if region.is_empty() { d } else { d.region(region) }
}

/// Mesh problems in a vegetation region are warnings: the solver solves no flow or transport
/// equations on that mesh (it carries the leaf temperature and the radiation exchange), and
/// cases with a few wrongly oriented faces there run. A mesh that can't be read, or whose
/// coordinates are not numbers, stays an error: the solver still reads it.
pub fn soften_for_vegetation(diagnostics: &mut [crate::Diagnostic]) {
    for d in diagnostics.iter_mut() {
        let kept = ["mesh.unreadable", "mesh.non-finite"];
        if d.level == crate::Level::Error && !kept.contains(&d.code) {
            d.level = crate::Level::Warning;
            d.message.push_str(" (a warning in a vegetation region: the solver solves no flow equations on its mesh)");
        }
    }
}
