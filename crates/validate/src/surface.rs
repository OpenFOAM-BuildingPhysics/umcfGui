//! STL surfaces (the geometry in `constant/triSurface` that `snappyHexMesh` and
//! `surfaceFeatures` read): what [`umcf_geom::check::analyse`] found, as diagnostics.
//!
//! Open edges are not a problem in themselves: terrain and a ground that reaches the domain
//! boundary are open. A surface that is to be cut out of the mesh as a solid, or to mark a
//! `cellZone` (vegetation), must be closed and consistently wound, because `snappyHexMesh`
//! decides what is inside from the surface's normals.

use umcf_geom::check::Report;
use umcf_geom::stl::Stl;

use crate::{Diagnostic, Level};

/// A surface larger than this (metres, in any direction) is probably not in metres.
pub const MILLIMETRE_EXTENT: f64 = 20_000.0;

/// Beyond this distance from the origin (metres) an STL file's numbers (32-bit floats) are a
/// centimetre or more apart.
pub const FAR_FROM_ORIGIN: f64 = 131_072.0;

/// The spacing of 32-bit floats at `x`.
fn f32_spacing(x: f64) -> f64 {
    let x = x.abs() as f32;
    let next = f32::from_bits(x.to_bits().saturating_add(1));
    f64::from(next - x)
}

/// A length in metres, to a tenth of a millimetre, without trailing zeros.
fn metres(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// A surface file that could not be read.
pub fn unreadable(file: &str, why: &str) -> Diagnostic {
    Diagnostic::new(Level::Error, "surface.unreadable", file, format!("can't be read as an STL surface: {why}"))
}

/// The diagnostics of the surface in `file`, read into `stl` and analysed into `r`.
pub fn diagnostics(file: &str, stl: &Stl, r: &Report) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut add = |level, code, message: String| out.push(Diagnostic::new(level, code, file, message));
    let stlb = file.ends_with(".stlb") || file.ends_with(".stlb.gz");
    let why = if stlb && stl.format == umcf_geom::stl::Format::Ascii {
        Some("OpenFOAM reads a `.stlb` file as binary, and this one is ASCII".to_string())
    } else if stl.header_solid && !stlb {
        Some("its header starts with `solid`, so OpenFOAM takes it for an ASCII file".to_string())
    } else {
        stl.openfoam.clone()
    };
    if let Some(why) = &why {
        add(
            Level::Error,
            "surface.openfoam",
            format!("OpenFOAM can't read this file: {why}. `geometry.repair` writes it again in a form OpenFOAM reads"),
        );
    }
    for note in &stl.notes {
        add(Level::Info, "surface.read", note.clone());
    }
    let regions =
        if r.region_count > r.regions.len() { format!("{}, …", r.regions.join(", ")) } else { r.regions.join(", ") };
    let extent = r.bbox.map_or(String::new(), |b| {
        let s = b.size();
        let m = metres;
        format!(", {} × {} × {} m from ({} {} {})", m(s.x), m(s.y), m(s.z), m(b.min.x), m(b.min.y), m(b.min.z))
    });
    add(
        Level::Info,
        "surface.summary",
        format!(
            "{} {}, {} ({regions}){extent}; {} ({} closed)",
            stl.format.label(),
            plural(r.triangles, "triangle", "triangles"),
            plural(r.region_count, "region", "regions"),
            plural(r.parts, "part", "parts"),
            r.closed_parts
        ),
    );
    if r.triangles == 0 {
        add(Level::Warning, "surface.empty", "has no triangles".to_string());
        return out;
    }
    if r.empty_regions > 0 {
        let what = match stl.format {
            umcf_geom::stl::Format::Binary => format!(
                "the triangles' attribute values go up to {}, so OpenFOAM makes {} regions (patches after snappyHexMesh), {} of them empty; some programs keep a colour there",
                r.region_count.saturating_sub(1),
                r.region_count,
                r.empty_regions
            ),
            umcf_geom::stl::Format::Ascii => format!(
                "{} without triangles: OpenFOAM still makes {} (empty patches after snappyHexMesh)",
                plural(r.empty_regions, "solid is", "solids are"),
                if r.empty_regions == 1 { "it a region" } else { "each a region" }
            ),
        };
        add(Level::Warning, "surface.regions", what);
    }
    for (name, region) in &r.renamed {
        add(
            Level::Info,
            "surface.renamed",
            format!(
                "solid `{name}` is region `{region}` for OpenFOAM (a name can't have spaces, quotes, `/`, `$`, `;` or braces); patches made from it are named after `{region}`"
            ),
        );
    }
    if r.non_finite.count > 0 {
        add(
            Level::Error,
            "surface.non-finite",
            format!(
                "{} with a coordinate that is not a finite number: OpenFOAM can't use the surface. `geometry.repair` with `degenerate` removes them",
                plural(r.non_finite.count, "triangle", "triangles")
            ),
        );
    }
    if r.degenerate.count > 0 {
        add(
            Level::Warning,
            "surface.degenerate",
            format!(
                "{} without area (two vertices in the same place, or all three in a line; OpenFOAM's triangle quality below 1e-15): they have no normal, and surfaceCheck warns they can upset snappyHexMesh's intersection tests. `geometry.repair` with `degenerate` removes them and keeps the surface closed around them",
                plural(r.degenerate.count, "triangle", "triangles")
            ),
        );
    }
    if r.duplicate.count > 0 {
        add(
            Level::Warning,
            "surface.duplicate",
            format!(
                "{} the same as another (the same three vertices): `geometry.repair` with `degenerate` removes the copies",
                plural(r.duplicate.count, "triangle is", "triangles are")
            ),
        );
    }
    if r.non_manifold_edges > 0 {
        add(
            Level::Warning,
            "surface.non-manifold",
            format!(
                "{} shared by more than two triangles ({}): the surface has no single inside there",
                plural(r.non_manifold_edges, "edge is", "edges are"),
                plural(r.non_manifold.count, "triangle", "triangles")
            ),
        );
    }
    if r.inconsistent_edges > 0 {
        add(
            Level::Warning,
            "surface.normals",
            format!(
                "normals not consistent: across {} two triangles face opposite ways ({} involved). surfaceFeatures then takes such edges for folds (convex ones for concave), and snappyHexMesh decides what is inside a closed surface from its normals (cellZones, inside/outside refinement): `geometry.repair` with `orient` makes them consistent",
                plural(r.inconsistent_edges, "edge", "edges"),
                plural(r.inconsistent.count, "triangle", "triangles")
            ),
        );
    }
    if r.inward_parts > 0 {
        add(
            Level::Warning,
            "surface.inward",
            format!(
                "{} of the surface {} normals pointing inward: snappyHexMesh takes their outside for the inside. `geometry.repair` with `orient` turns them outward",
                plural(r.inward_parts, "closed part", "closed parts"),
                if r.inward_parts == 1 { "has its" } else { "have their" }
            ),
        );
    }
    if r.open_edges > 0 {
        add(
            Level::Info,
            "surface.open",
            format!(
                "open: {} with one triangle ({}). Fine for terrain or a surface that reaches the domain boundary; a solid to cut out of the mesh, or a cellZone, must be closed. Gaps narrower than a tolerance close with `geometry.repair` and `weld`",
                plural(r.open_edges, "edge", "edges"),
                plural(r.open.count, "triangle", "triangles")
            ),
        );
    }
    if r.stored_normals.count > 0 {
        add(
            Level::Info,
            "surface.stored-normals",
            format!(
                "{} a facet normal in the file that points against its winding: OpenFOAM uses the winding and ignores it, other programs may not",
                plural(r.stored_normals.count, "triangle has", "triangles have")
            ),
        );
    }
    if let Some(b) = r.bbox {
        let size = b.size().max_abs();
        if size > MILLIMETRE_EXTENT {
            add(
                Level::Warning,
                "surface.units",
                format!(
                    "{size:.0} m across: if the file is in millimetres, `geometry.scale` with factor 0.001 makes it metres (OpenFOAM reads STL coordinates as metres)"
                ),
            );
        }
        let far = b.min.max_abs().max(b.max.max_abs());
        if far > FAR_FROM_ORIGIN {
            add(
                Level::Warning,
                "surface.far",
                format!(
                    "coordinates up to {far:.0} m from the origin (projected coordinates?): an STL file's numbers are {:.2} m apart there, which moves vertices and can open gaps. Move the geometry near the origin before it is written to STL",
                    f32_spacing(far)
                ),
            );
        }
    }
    out
}
