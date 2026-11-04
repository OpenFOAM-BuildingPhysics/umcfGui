//! Reads STL files and prints what `umcf_geom::check::analyse` finds in each:
//! `cargo run --release -p umcf-geom --example check_stl -- <file.stl>...`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

fn main() {
    let repair = std::env::args().any(|a| a == "--repair");
    for path in std::env::args().skip(1).filter(|a| a != "--repair") {
        let bytes = std::fs::read(&path).expect("read");
        let t = std::time::Instant::now();
        let stl = umcf_geom::stl::read(&bytes, &Default::default()).expect("parse");
        let read = t.elapsed();
        let mut stl = stl;
        if repair {
            println!("  {:?}", umcf_geom::repair::drop_degenerate(&mut stl));
            println!("  {:?}", umcf_geom::repair::orient(&mut stl));
        }
        let r = umcf_geom::check::analyse(&stl);
        println!(
            "{path}: {} {:?} {:?}, solids {:?} (read {read:?}, checked {:?})",
            stl.format.label(),
            stl.notes,
            stl.openfoam,
            stl.solids.len(),
            t.elapsed() - read
        );
        println!(
            "  triangles {} points {} regions {:?} bbox {:?}\n  open edges {} ({} tris) non-manifold edges {} ({} tris) inconsistent edges {} ({} tris)\n  degenerate {} duplicate {} non-finite {} parts {} closed {} inward {} stored normals against winding {}",
            r.triangles,
            r.points,
            r.regions,
            r.bbox,
            r.open_edges,
            r.open.count,
            r.non_manifold_edges,
            r.non_manifold.count,
            r.inconsistent_edges,
            r.inconsistent.count,
            r.degenerate.count,
            r.duplicate.count,
            r.non_finite.count,
            r.parts,
            r.closed_parts,
            r.inward_parts,
            r.stored_normals.count
        );
    }
}
