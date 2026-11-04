use std::io::Read;

use umcf_core::MemFs;
use umcf_foam_mesh::{Limits, PolyMesh};
use umcf_geom::bvh::Ray;
use umcf_geom::stl::{Format, Stl, Triangle};
use umcf_geom::{BBox, Vec3};

use crate::camera::{Camera, View};
use crate::raster::{SELECTED, Style, draw, palette};
use crate::{Object, Scene, Source, png};

/// A unit cube mesh at `at`, its bottom face patch `ground`, top `sky`, the sides `walls`.
fn cube(at: [f64; 3]) -> PolyMesh {
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    let pts: String = (0..8)
        .map(|i| {
            let b = |k: usize| if (i >> k) & 1 == 1 { 1.0 } else { 0.0 };
            format!("({} {} {})\n", at[0] + b(0), at[1] + b(1), at[2] + b(2))
        })
        .collect();
    // Outward: bottom, top, then the four sides.
    let faces = "6\n(\n4(0 2 3 1)\n4(4 5 7 6)\n4(0 1 5 4)\n4(2 6 7 3)\n4(0 4 6 2)\n4(1 3 7 5)\n)\n";
    let patch = |n: &str, k: &str, count: u32, start: u32| {
        format!("    {n}\n    {{\n        type {k};\n        nFaces {count};\n        startFace {start};\n    }}\n")
    };
    let fs = MemFs::new()
        .with("p/points", format!("{}8\n(\n{pts})\n", h("vectorField")))
        .with("p/faces", format!("{}{faces}", h("faceList")))
        .with("p/owner", format!("{}6\n(\n0 0 0 0 0 0\n)\n", h("labelList")))
        .with("p/neighbour", format!("{}0\n(\n)\n", h("labelList")))
        .with(
            "p/boundary",
            format!(
                "{}3\n(\n{}{}{})\n",
                h("polyBoundaryMesh"),
                patch("ground", "wall", 1, 0),
                patch("sky", "patch", 1, 1),
                patch("walls", "wall", 4, 2)
            ),
        );
    PolyMesh::read(&fs, "p", &Limits::default()).unwrap()
}

fn scene_of(mesh: &PolyMesh) -> Scene {
    let (lo, hi) = mesh.bounds().unwrap();
    let b = BBox { min: Vec3::new(lo[0], lo[1], lo[2]), max: Vec3::new(hi[0], hi[1], hi[2]) };
    let mut s = Scene::new(Scene::origin_for([b]));
    s.objects.push(Object::from_mesh("air", mesh, s.origin));
    s
}

#[test]
fn a_mesh_boundary_is_triangles_by_patch_with_its_face_edges() {
    let s = scene_of(&cube([0.0; 3]));
    let o = &s.objects[0];
    assert_eq!(o.source, Source::Mesh { region: "air".to_string() });
    assert_eq!(o.label(), "mesh air");
    assert_eq!(o.classes, ["ground", "sky", "walls"]);
    assert_eq!(o.kinds, ["wall", "patch", "wall"]);
    assert_eq!((o.points.len(), o.triangles.len()), (8, 12));
    assert_eq!(&o.class[..4], &[0, 0, 1, 1]);
    assert_eq!(&o.origin_index[..6], &[0, 0, 1, 1, 2, 2]);
    // A quad's two triangles: the diagonal (0→2) is not an edge of the face.
    assert_eq!(&o.edges[..2], &[0b011, 0b110]);
    assert_eq!(s.origin, Vec3::new(0.5, 0.5, 0.5));
    assert_eq!(s.bbox().unwrap().size(), Vec3::new(1.0, 1.0, 1.0));
    assert_eq!(s.triangles(), 12);
}

#[test]
fn picking_returns_patches_and_faces_nearest_first() {
    let s = scene_of(&cube([0.0; 3]));
    let down = Ray { origin: [0.3, 0.6, 5.0], direction: [0.0, 0.0, -1.0] };
    let hits = s.pick(&down, 10);
    let classes: Vec<&str> = hits.iter().map(|h| s.objects[0].classes[h.class as usize].as_str()).collect();
    assert_eq!(classes, ["sky", "ground"]);
    assert_eq!(hits.iter().map(|h| h.index).collect::<Vec<_>>(), [1, 0]);
    assert!((hits[0].distance - 4.0).abs() < 1e-9 && (hits[1].point.z).abs() < 1e-9);
    assert_eq!(s.pick(&down, 1).len(), 1);
    let past = Ray { origin: [3.0, 3.0, 5.0], direction: [0.0, 0.0, -1.0] };
    assert!(s.pick(&past, 10).is_empty());
    // The camera's ray through the middle of a top view goes straight down onto the sky.
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let hits = s.pick(&cam.ray(49.5, 49.5, 100, 100), 10);
    assert_eq!(hits.first().map(|h| h.class), Some(1));
}

/// Far from the world origin (Swiss LV95 coordinates) points stay precise: relative to the
/// scene's origin.
#[test]
fn far_coordinates_keep_their_precision() {
    let at = [2_600_000.0, 1_200_000.0, 400.0];
    let s = scene_of(&cube(at));
    let ray = Ray { origin: [at[0] + 0.999, at[1] + 0.001, 500.0], direction: [0.0, 0.0, -1.0] };
    let hits = s.pick(&ray, 10);
    assert_eq!(hits.len(), 2);
    assert!((hits[0].point.z - 401.0).abs() < 1e-6);
    let miss = Ray { origin: [at[0] + 1.001, at[1] + 0.5, 500.0], direction: [0.0, 0.0, -1.0] };
    assert!(s.pick(&miss, 10).is_empty());
}

#[test]
fn surfaces_are_triangles_by_region() {
    let t = |z: f32, solid: u32| Triangle {
        vertices: [[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]],
        normal: [0.0; 3],
        solid,
    };
    let mut bad = t(2.0, 0);
    bad.vertices[1][0] = f32::INFINITY;
    let stl = Stl {
        format: Format::Ascii,
        solids: vec!["roof 1".to_string(), "ground".to_string()],
        triangles: vec![t(0.0, 1), bad, t(1.0, 0)],
        notes: Vec::new(),
        openfoam: None,
        header_solid: false,
    };
    let o = Object::from_stl("constant/triSurface/b.stl", &stl, Vec3::ZERO);
    assert_eq!(o.label(), "surface constant/triSurface/b.stl");
    assert_eq!(o.classes, ["roof1", "ground"]);
    // The triangle that is not finite is left out; the others keep their index in the file.
    assert_eq!(o.origin_index, [0, 2]);
    assert_eq!(o.class, [1, 0]);
    let mut s = Scene::new(Vec3::ZERO);
    s.objects.push(o);
    let hits = s.pick(&Ray { origin: [0.2, 0.2, 9.0], direction: [0.0, 0.0, -1.0] }, 10);
    assert_eq!(hits.iter().map(|h| (h.class, h.index)).collect::<Vec<_>>(), [(0, 2), (1, 0)]);
}

fn close(a: [u8; 4], b: [f32; 3], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (f32::from(*x) / 255.0 - y).abs() <= tol)
}

#[test]
fn the_rasteriser_draws_patches_in_their_colours_and_the_selection_in_its_own() {
    let s = scene_of(&cube([0.0; 3]));
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let mut style = Style { width: 120, height: 100, ..Style::default() };
    let img = draw(&s, &cam, &style);
    assert_eq!((img.width, img.height, img.rgba.len()), (120, 100, 120 * 100 * 4));
    // The middle is the sky, seen head on: its own colour, fully lit.
    let mid = img.pixel(60, 50).unwrap();
    assert!(close(mid, palette(1), 0.02), "{mid:?} vs {:?}", palette(1));
    // A corner is background.
    let corner = img.pixel(0, 0).unwrap();
    assert!(!close(corner, palette(1), 0.05));
    assert_eq!(img.pixel(120, 0), None);
    style.selected = vec![(0, 1)];
    let img = draw(&s, &cam, &style);
    assert!(close(img.pixel(60, 50).unwrap(), SELECTED, 0.02));
    // With the faces' edges: the quad's diagonal is not drawn, its sides are.
    style.selected.clear();
    style.edges = true;
    let img = draw(&s, &cam, &style);
    let diagonal = img.pixel(60, 50).unwrap();
    assert!(close(diagonal, palette(1), 0.02), "{diagonal:?}");
}

/// A selected patch lying on another region's patch is drawn over it.
#[test]
fn the_selection_wins_where_two_regions_meet() {
    let mesh = cube([0.0; 3]);
    let mut s = scene_of(&mesh);
    s.objects.push(Object::from_mesh("veg", &mesh, s.origin));
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let style = Style { width: 40, height: 40, selected: vec![(1, 1)], ..Style::default() };
    assert!(close(draw(&s, &cam, &style).pixel(20, 20).unwrap(), SELECTED, 0.02));
}

#[test]
fn a_camera_inside_the_box_clips_and_still_draws() {
    let s = scene_of(&cube([0.0; 3]));
    let mut cam = Camera::fit(View::Iso, &s.bbox().unwrap(), 1.0);
    cam.distance = 0.1;
    let img = draw(&s, &cam, &Style { width: 64, height: 64, ..Style::default() });
    // Every pixel is a wall seen from inside.
    let bg = draw(&Scene::new(Vec3::ZERO), &cam, &Style { width: 64, height: 64, ..Style::default() });
    assert!(img.pixel(32, 32) != bg.pixel(32, 32));
    // Nothing breaks on odd sizes or cameras.
    let mut odd = cam;
    odd.distance = f64::NAN;
    odd.fov = -3.0;
    let img = draw(&s, &odd, &Style { width: 0, height: 100_000, ..Style::default() });
    assert_eq!((img.width, img.height), (1, crate::raster::MAX_SIDE));
}

#[test]
fn png_files_decode_to_the_image() {
    let s = scene_of(&cube([0.0; 3]));
    let cam = Camera::fit(View::Iso, &s.bbox().unwrap(), 1.5);
    let img = draw(&s, &cam, &Style { width: 30, height: 20, ..Style::default() });
    let file = png::encode(&img);
    assert_eq!(&file[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&file[12..16], b"IHDR");
    assert_eq!(u32::from_be_bytes(file[16..20].try_into().unwrap()), 30);
    // The IHDR CRC of a 30×20 RGBA image, as any PNG tool computes it.
    let crc = u32::from_be_bytes(file[29..33].try_into().unwrap());
    assert_eq!(crc, crc32_reference(&file[12..29]));
    let len = u32::from_be_bytes(file[33..37].try_into().unwrap()) as usize;
    assert_eq!(&file[37..41], b"IDAT");
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&file[41..41 + len]).read_to_end(&mut raw).unwrap();
    // Undo the Sub filter and compare.
    let row = 30 * 4;
    let mut back = Vec::new();
    for line in raw.chunks(row + 1) {
        assert_eq!(line[0], 1);
        let start = back.len();
        for (i, b) in line[1..].iter().enumerate() {
            let left = if i >= 4 { back[start + i - 4] } else { 0u8 };
            back.push(b.wrapping_add(left));
        }
    }
    assert_eq!(back, img.rgba);
    assert!(file.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
}

/// CRC-32 by the table method, independent of the encoder's bitwise one.
fn crc32_reference(data: &[u8]) -> u32 {
    let table: Vec<u32> =
        (0..256u32).map(|n| (0..8).fold(n, |c, _| if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 })).collect();
    !data.iter().fold(!0u32, |c, b| table[((c ^ u32::from(*b)) & 0xff) as usize] ^ (c >> 8))
}

#[test]
fn views_parse_and_frame_the_box() {
    assert_eq!(View::parse("top"), Some(View::Top));
    assert_eq!(View::parse("Top"), None);
    let b = BBox { min: Vec3::new(-1.0, -1.0, -1.0), max: Vec3::new(1.0, 1.0, 1.0) };
    for (_, v) in View::ALL {
        let c = Camera::fit(v, &b, 2.0);
        let f = c.frame();
        // The axes are unit and at right angles; the eye looks at the target.
        for (x, y) in [(f.right, f.up), (f.up, f.forward), (f.forward, f.right)] {
            assert!(x.dot(y).abs() < 1e-12 && (x.length() - 1.0).abs() < 1e-12);
        }
        assert!(((c.target - f.eye).dot(f.forward) - c.distance).abs() < 1e-9);
    }
    let f = Camera::fit(View::Front, &b, 1.0).frame();
    assert!((f.forward.y - 1.0).abs() < 1e-12 && (f.up.z - 1.0).abs() < 1e-12);
}
