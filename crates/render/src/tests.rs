use std::io::Read;

use umcf_core::MemFs;
use umcf_foam_mesh::{Limits, PolyMesh};
use umcf_geom::bvh::Ray;
use umcf_geom::stl::{Format, Stl, Triangle};
use umcf_geom::{BBox, Vec3};

use crate::camera::{Camera, View};
use crate::raster::{Rect, SELECTED, Stroke, Style, covered, draw, palette};
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

/// The cube with a ninth point, far away and used by no face.
fn cube_with_stray_point() -> PolyMesh {
    let mut m = cube([0.0; 3]);
    m.points.push([1e40, 0.0, 0.0]);
    m
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
    // A selected face (the sky is face 1), its patch not selected.
    style.selected.clear();
    style.selected_faces.insert((0, 1));
    let img = draw(&s, &cam, &style);
    assert!(close(img.pixel(60, 50).unwrap(), SELECTED, 0.02));
    style.selected_faces = [(0, 0)].into_iter().collect();
    let img = draw(&s, &cam, &style);
    assert!(close(img.pixel(60, 50).unwrap(), palette(1), 0.02));
    style.selected_faces.clear();
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
    let img = draw(&s, &odd, &Style { width: 0, height: 9000, ..Style::default() });
    assert_eq!((img.width, img.height), (1, crate::raster::MAX_SIDE));
    // At most MAX_PIXELS: the width is kept, the height cut.
    assert_eq!(crate::raster::size(8192, 8192), (8192, 2048));
    assert_eq!(crate::raster::size(0, 0), (1, 1));
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

/// A point no boundary face uses doesn't move the scene; one that a face uses but that is too
/// far to draw drops that face, and says so.
#[test]
fn far_points_are_left_out_and_counted() {
    let m = cube_with_stray_point();
    let b = Object::boundary_bounds(&m).unwrap();
    assert_eq!((b.min, b.max), (Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)));
    let o = Object::from_mesh("air", &m, b.centre());
    assert_eq!((o.triangles.len(), o.dropped), (12, 0));
    let mut far = cube([0.0; 3]);
    far.points[7] = [1e40, 1.0, 1.0];
    let o = Object::from_mesh("air", &far, Vec3::new(0.5, 0.5, 0.5));
    // Point 7 is in the top and two side faces.
    assert_eq!((o.triangles.len(), o.dropped), (6, 3));
}

/// One entry per patch along a ray, however many of its triangles the ray crosses.
#[test]
fn picks_list_each_patch_once() {
    let s = scene_of(&cube([0.0; 3]));
    // Through the diagonal of the bottom and top faces: both triangles of each are hit.
    let ray = Ray { origin: [0.5, 0.5, 5.0], direction: [0.0, 0.0, -1.0] };
    assert_eq!(s.pick(&ray, 100).len(), 4);
    let classes: Vec<u32> = s.pick_classes(&ray).iter().map(|p| p.class).collect();
    assert_eq!(classes, [1, 0]);
}

/// The faces of `covered`'s triangles, by object.
fn faces_of(s: &Scene, hits: &[(usize, u32)]) -> Vec<(usize, u32)> {
    let mut f: Vec<(usize, u32)> = hits.iter().map(|(k, t)| (*k, s.objects[*k].origin_index[*t as usize])).collect();
    f.dedup();
    f
}

#[test]
fn a_brush_stroke_covers_what_is_seen_under_it_or_everything_through_it() {
    // Two cubes side by side, seen from above, as for the rectangle: their middles at about
    // (69, 50) and (131, 50); the image's top rows are background.
    let mut s = scene_of(&cube([0.0; 3]));
    s.objects.push(Object::from_mesh("veg", &cube([2.0, 0.0, 0.0]), s.origin));
    let b = BBox { min: Vec3::ZERO, max: Vec3::new(3.0, 1.0, 1.0) };
    let cam = Camera::fit(View::Top, &b, 2.0);
    let st = Style { width: 200, height: 100, ..Style::default() };
    let all = |s: &Scene, area: Stroke, through: bool| faces_of(s, &covered(s, &cam, &st, area, through, &|_, _| true));
    // A dab on the left cube: its sky seen; through it, its sky and its ground (whose middle,
    // further, is seen a little nearer the image's middle).
    let dab = Stroke { points: vec![[69.0, 50.0]], radius: 5.0 };
    assert_eq!(all(&s, dab.clone(), false), [(0, 1)]);
    assert_eq!(all(&s, dab, true), [(0, 0), (0, 1)]);
    // A line across both: both skies.
    let line = Stroke { points: vec![[60.0, 50.0], [140.0, 50.0]], radius: 2.0 };
    let both = all(&s, line, false);
    assert!(both.contains(&(0, 1)) && both.contains(&(1, 1)), "{both:?}");
    // Up from the left cube's middle and right along the top row: its bounds hold the right
    // cube, its path doesn't come near it.
    let hook = Stroke { points: vec![[69.0, 50.0], [69.0, 1.0], [199.0, 1.0]], radius: 1.0 };
    let left = all(&s, hook.clone(), false);
    assert!(!left.is_empty() && left.iter().all(|(k, _)| *k == 0), "{left:?}");
    // The same with both cubes half transparent (seen through, counted where they are
    // blended): still the left one only, and its ground seen through its sky.
    let mut glass = st.clone();
    for k in 0..2 {
        for c in 0..s.objects[k].classes.len() as u32 {
            glass.display.insert((k, c), crate::raster::Display { mode: crate::raster::Mode::Surface, opacity: 0.5 });
        }
    }
    let seen = faces_of(&s, &covered(&s, &cam, &glass, hook, false, &|_, _| true));
    assert!(seen.contains(&(0, 0)) && seen.contains(&(0, 1)), "{seen:?}");
    assert!(seen.iter().all(|(k, _)| *k == 0), "{seen:?}");
    // The smallest stroke still takes the pixel under it; one with no points, or not finite,
    // nothing.
    assert_eq!(all(&s, Stroke { points: vec![[69.2, 50.3]], radius: 0.0 }, false), [(0, 1)]);
    for empty in [
        Stroke { points: vec![], radius: 5.0 },
        Stroke { points: vec![[f64::NAN, 50.0]], radius: 5.0 },
        Stroke { points: vec![[69.0, 50.0]], radius: f64::INFINITY },
        Stroke { points: vec![[69.0, 50.0]; crate::raster::MAX_STROKE + 1], radius: 5.0 },
    ] {
        assert!(all(&s, empty.clone(), false).is_empty() && all(&s, empty, true).is_empty());
    }
    // Off the image: nothing.
    assert!(all(&s, Stroke { points: vec![[300.0, 50.0]], radius: 5.0 }, false).is_empty());
}

#[test]
fn a_rectangle_covers_what_is_seen_in_it_or_everything_through_it() {
    // Two cubes side by side, seen from above: the left one in the left half of the image.
    let mut s = scene_of(&cube([0.0; 3]));
    let right = cube([2.0, 0.0, 0.0]);
    s.objects.push(Object::from_mesh("veg", &right, s.origin));
    let b = BBox { min: Vec3::ZERO, max: Vec3::new(3.0, 1.0, 1.0) };
    let cam = Camera::fit(View::Top, &b, 2.0);
    let st = Style { width: 200, height: 100, ..Style::default() };
    let all = Rect { a: [0.0, 0.0], b: [200.0, 100.0] };
    // Seen: the two skies (face 1) with both their triangles, and the walls perspective shows
    // from above, not the grounds (face 0) under them.
    let seen = covered(&s, &cam, &st, all, false, &|_, _| true);
    for k in 0..2 {
        assert!(seen.contains(&(k, 2)) && seen.contains(&(k, 3)), "{seen:?}");
        assert!(!seen.contains(&(k, 0)) && !seen.contains(&(k, 1)), "{seen:?}");
    }
    assert!(seen.windows(2).all(|p| p[0] < p[1]));
    // Through: every face of both (their middles are all inside the image), by its first
    // triangle.
    let through = covered(&s, &cam, &st, all, true, &|_, _| true);
    assert_eq!(faces_of(&s, &through), (0..2).flat_map(|k| (0..6).map(move |f| (k, f))).collect::<Vec<_>>());
    assert_eq!(through.len(), 12);
    assert!(through.iter().all(|(_, t)| t % 2 == 0));
    // The left half, corners in either order: the left cube only.
    let left = Rect { a: [100.0, 100.0], b: [0.0, 0.0] };
    assert!(faces_of(&s, &covered(&s, &cam, &st, left, false, &|_, _| true)).iter().all(|(k, f)| *k == 0 && *f != 0));
    assert_eq!(faces_of(&s, &covered(&s, &cam, &st, left, true, &|_, _| true)).len(), 6);
    // A small one in the left cube's middle: through it, the sky and the ground (the walls'
    // middles are outside).
    // (The left cube's middle is seen at about (69, 50).)
    let mid = Rect { a: [64.0, 45.0], b: [74.0, 55.0] };
    assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, true, &|_, _| true)), [(0, 0), (0, 1)]);
    assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, false, &|_, _| true)), [(0, 1)]);
    // No pixel's middle inside, outside the image, or not finite: nothing.
    for r in [
        Rect { a: [10.2, 10.0], b: [10.4, 20.0] },
        Rect { a: [300.0, 0.0], b: [400.0, 100.0] },
        Rect { a: [f64::NAN, 0.0], b: [10.0, 10.0] },
    ] {
        assert!(covered(&s, &cam, &st, r, false, &|_, _| true).is_empty(), "{r:?}");
        if r.a[0].is_nan() {
            assert!(covered(&s, &cam, &st, r, true, &|_, _| true).is_empty(), "{r:?}");
        }
    }
    // Through a rectangle past the image's edge: only the middles on the image. Four times as
    // close, the right cube's middle (seen at x 131) is at 224, off the image 200 wide.
    let mut close = cam;
    close.distance /= 4.0;
    let past = Rect { a: [150.0, -50.0], b: [400.0, 150.0] };
    let off = faces_of(&s, &covered(&s, &close, &st, past, true, &|_, _| true));
    // Its wall at x 2, nearer the middle, is on the image and taken; its sky is not.
    assert!(off.contains(&(1, 4)) && !off.contains(&(1, 1)), "{off:?}");
    // A rectangle narrower than a pixel holds no pixel's middle: nothing is seen in it, but
    // through it the middles inside it count. Seen from above, the cube's middle is the image's.
    let one = scene_of(&cube([0.0; 3]));
    let above = Camera::fit(View::Top, &one.bbox().unwrap(), 1.0);
    let square = Style { width: 100, height: 100, ..Style::default() };
    let thin = Rect { a: [49.9, 49.9], b: [50.1, 50.1] };
    assert!(covered(&one, &above, &square, thin, false, &|_, _| true).is_empty());
    assert_eq!(faces_of(&one, &covered(&one, &above, &square, thin, true, &|_, _| true)), [(0, 0), (0, 1)]);
    // Above both, looking up: everything is behind the camera.
    let mut inside = cam;
    inside.distance = 0.0;
    inside.target = Vec3::new(1.5, 0.5, 2.0);
    inside.elevation = -90.0;
    assert!(covered(&s, &inside, &st, all, true, &|_, _| true).is_empty());
    // Only what `keep` takes counts, and hides: without the skies, the ground is seen in the
    // middle.
    let no_left = |k: usize, _: u32| k == 1;
    assert!(faces_of(&s, &covered(&s, &cam, &st, left, false, &no_left)).is_empty());
    let no_sky = |_: usize, c: u32| c != 1;
    let seen = faces_of(&s, &covered(&s, &cam, &st, mid, false, &no_sky));
    assert_eq!(seen, [(0, 0)]);
    assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, true, &no_sky)), [(0, 0)]);
    // An empty scene, an empty image.
    assert!(covered(&Scene::new(Vec3::ZERO), &cam, &st, all, false, &|_, _| true).is_empty());
    // A size of 0 draws 1 × 1: its one pixel looks down between the two cubes.
    assert!(covered(&s, &cam, &Style::default(), all, false, &|_, _| true).is_empty());
}

/// Where two regions' patches lie on each other, the selected one is what a rectangle sees, as
/// it is what is drawn.
#[test]
fn a_rectangle_sees_the_selection_where_two_regions_meet() {
    let mesh = cube([0.0; 3]);
    let mut s = scene_of(&mesh);
    s.objects.push(Object::from_mesh("veg", &mesh, s.origin));
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let mid = Rect { a: [15.0, 15.0], b: [25.0, 25.0] };
    for k in 0..2 {
        let st = Style { width: 40, height: 40, selected: vec![(k, 1)], ..Style::default() };
        assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, false, &|_, _| true)), [(k, 1)]);
        let mut st = Style { width: 40, height: 40, ..Style::default() };
        st.selected_faces.insert((k, 1));
        assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, false, &|_, _| true)), [(k, 1)]);
    }
}

#[test]
fn layers_are_drawn_as_surfaces_wireframes_points_or_not_at_all() {
    use crate::raster::{Display, Mode};
    let s = scene_of(&cube([0.0; 3]));
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let style = |sky: Display| {
        let mut st = Style { width: 100, height: 100, ..Style::default() };
        st.display.insert((0, 1), sky);
        st
    };
    let mid = |st: &Style| draw(&s, &cam, st).pixel(50, 50).unwrap();
    // The sky as it is, then hidden: the ground under it is seen.
    assert!(close(mid(&style(Display::default())), palette(1), 0.02));
    let hidden = style(Display { mode: Mode::Hidden, opacity: 1.0 });
    assert!(close(mid(&hidden), palette(0), 0.03), "{:?}", mid(&hidden));
    // Half transparent: half of each.
    let glass = style(Display { mode: Mode::Surface, opacity: 0.5 });
    let half: [f32; 3] = [0, 1, 2].map(|k| (palette(1)[k] + palette(0)[k]) * 0.5);
    assert!(close(mid(&glass), half, 0.02), "{:?} vs {half:?}", mid(&glass));
    // With the faces' edges, a transparent sky has them too: its sides are darker.
    let mut edged = glass.clone();
    edged.edges = true;
    let (plain, with) = (draw(&s, &cam, &glass), draw(&s, &cam, &edged));
    assert_ne!(plain.rgba, with.rgba);
    // A wireframe: the ground in the middle, the sky's edges along its sides.
    let wire = style(Display { mode: Mode::Wireframe, opacity: 1.0 });
    let img = draw(&s, &cam, &wire);
    assert!(close(img.pixel(50, 50).unwrap(), palette(0), 0.03));
    let line = palette(1).map(|c| c * 0.85);
    let on_line = (0..100)
        .flat_map(|y| (0..100).map(move |x| (x, y)))
        .filter(|(x, y)| close(img.pixel(*x, *y).unwrap(), line, 0.02))
        .count();
    assert!(on_line > 40, "{on_line} pixels of the sky's edges");
    // Points: the sky's four corners, the ground between them.
    let points = style(Display { mode: Mode::Points, opacity: 1.0 });
    let img = draw(&s, &cam, &points);
    let dots = (0..100)
        .flat_map(|y| (0..100).map(move |x| (x, y)))
        .filter(|(x, y)| close(img.pixel(*x, *y).unwrap(), line, 0.02))
        .count();
    assert!((4..=64).contains(&dots), "{dots} pixels of the sky's four points");
    assert!(close(img.pixel(50, 50).unwrap(), palette(0), 0.03));
    // Everything hidden: the background.
    let mut none = Style { width: 100, height: 100, ..Style::default() };
    for c in 0..3 {
        none.display.insert((0, c), Display { mode: Mode::Hidden, opacity: 1.0 });
    }
    let bg = draw(&Scene::new(Vec3::ZERO), &cam, &Style { width: 100, height: 100, ..Style::default() });
    assert_eq!(draw(&s, &cam, &none).rgba, bg.rgba);
}

#[test]
fn a_rectangle_sees_through_what_is_not_an_opaque_surface() {
    use crate::raster::{Display, Mode};
    let s = scene_of(&cube([0.0; 3]));
    let cam = Camera::fit(View::Top, &s.bbox().unwrap(), 1.0);
    let mid = Rect { a: [45.0, 45.0], b: [55.0, 55.0] };
    for (mode, opacity, seen) in [
        (Mode::Surface, 1.0, vec![(0, 1)]),
        (Mode::Surface, 0.4, vec![(0, 0), (0, 1)]),
        (Mode::Wireframe, 1.0, vec![(0, 0), (0, 1)]),
        (Mode::Points, 1.0, vec![(0, 0), (0, 1)]),
        (Mode::Hidden, 1.0, vec![(0, 0)]),
    ] {
        let mut st = Style { width: 100, height: 100, ..Style::default() };
        st.display.insert((0, 1), Display { mode, opacity });
        assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, false, &|_, _| true)), seen, "{mode:?} {opacity}");
    }
    // Through: everything but what is hidden.
    let mut st = Style { width: 100, height: 100, ..Style::default() };
    st.display.insert((0, 1), Display { mode: Mode::Hidden, opacity: 1.0 });
    assert_eq!(faces_of(&s, &covered(&s, &cam, &st, mid, true, &|_, _| true)), [(0, 0)]);
}

/// Edges and points are found once: an STL's corners welded, a mesh's patches each with their
/// own edges where they meet.
#[test]
fn wireframes_draw_each_edge_and_point_once_per_class() {
    let tri = |v: [[f32; 3]; 3], solid: u32| Triangle { vertices: v, normal: [0.0; 3], solid };
    // A unit square of two triangles in one region, a third triangle on its edge in another.
    let stl = Stl {
        format: Format::Ascii,
        solids: vec!["a".to_string(), "b".to_string()],
        triangles: vec![
            tri([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], 0),
            tri([[-0.0, 0.0, -0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], 0),
            tri([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]], 1),
        ],
        notes: Vec::new(),
        openfoam: None,
        header_solid: false,
    };
    let o = Object::from_stl("constant/triSurface/s.stl", &stl, Vec3::ZERO);
    let w = o.wire();
    // Region a: 5 edges (the diagonal is shared), 4 points; region b: 3 and 3, its edge on the
    // square counted for it too. A corner written -0 is the same as one written 0.
    assert_eq!(w.edges.len(), 8);
    assert_eq!(w.points.len(), 7);
    // A mesh: the cube's 12 edges; each patch has its own where they meet (ground 4, sky 4,
    // walls 4 × 4 less the 4 the sides share).
    let s = scene_of(&cube([0.0; 3]));
    let w = s.objects[0].wire();
    assert_eq!(w.edges.len(), 4 + 4 + 12);
    assert_eq!(w.points.len(), 4 + 4 + 8);
}

/// An edge that passes behind the camera is drawn where it is in front of it.
#[test]
fn an_edge_through_the_near_plane_is_still_drawn() {
    use crate::raster::{Display, Mode};
    let s = scene_of(&cube([0.0; 3]));
    // Inside the cube, looking along +y at a wall: the walls' edges run past the eye.
    let cam = Camera { target: Vec3::new(0.5, 0.5, 0.5), distance: 0.3, azimuth: 0.0, elevation: 0.0, fov: 90.0 };
    let mut st = Style { width: 200, height: 200, ..Style::default() };
    for c in 0..3 {
        st.display.insert((0, c), Display { mode: Mode::Wireframe, opacity: 1.0 });
    }
    let img = draw(&s, &cam, &st);
    let bg = draw(&Scene::new(Vec3::ZERO), &cam, &Style { width: 200, height: 200, ..Style::default() });
    // The far wall is seen from (38, 38) to (162, 162): the corners of the image show only the
    // four edges that run from it past the eye.
    let corner = |x: u32, y: u32| x.min(199 - x) < 30 && y.min(199 - y) < 30;
    let drawn = (0..200u32)
        .flat_map(|y| (0..200u32).map(move |x| (x, y)))
        .filter(|(x, y)| corner(*x, *y) && img.pixel(*x, *y) != bg.pixel(*x, *y))
        .count();
    assert!(drawn >= 4 * 25, "{drawn} pixels of edges in the corners");
}

#[test]
fn a_layer_nearly_opaque_is_drawn_as_it_is_opaque() {
    use crate::raster::{Display, Mode};
    // The cube from a corner: the nearer faces in front of the farther ones. At an opacity just
    // below 1 it is the cube drawn opaque, outlines too (they fade with the opacity), not the
    // farther faces mixed in; lower, they show through more and more.
    let s = scene_of(&cube([0.0; 3]));
    let cam = Camera::fit(View::Iso, &s.bbox().unwrap(), 1.0);
    let at = |a: f32| {
        let mut st = Style { width: 120, height: 120, ..Style::default() };
        for c in 0..3 {
            st.display.insert((0, c), Display { mode: Mode::Surface, opacity: a });
        }
        draw(&s, &cam, &st).rgba
    };
    let opaque = at(1.0);
    let off = |img: &[u8], tolerance: u8| {
        opaque.chunks(4).zip(img.chunks(4)).filter(|(p, q)| (0..3).any(|k| p[k].abs_diff(q[k]) > tolerance)).count()
    };
    assert!(off(&at(0.99), 4) == 0, "{} pixels differ at 0.99", off(&at(0.99), 4));
    let mut last = 0;
    for a in [0.9, 0.7, 0.5, 0.3] {
        let n = off(&at(a), 4);
        assert!(n >= last, "{a}: {n} pixels differ, {last} before");
        last = n;
    }
    assert!(last > 1000, "{last}");
}

#[test]
fn transparent_layers_in_the_same_place_give_the_same_nearest_everywhere() {
    use crate::raster::{ColourBy, Display, Mode};
    // A square twice in the same place, split along one diagonal and along the other (as the
    // faces of two regions on their interface), half transparent, each in its own colour: the
    // first is the nearest on every pixel, as where both are split alike (no outline along the
    // diagonals, no triangles of the one or the other).
    let square = |other: bool| {
        let v = |p: [f32; 3]| format!("vertex {} {} {}\n", p[0], p[1], p[2]);
        let tri = |a, b, c| format!("facet normal 0 0 1\nouter loop\n{}{}{}endloop\nendfacet\n", v(a), v(b), v(c));
        let [a, b, c, d] = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 4.0, 0.0]];
        let (t1, t2) = if other { (tri(a, b, d), tri(b, c, d)) } else { (tri(a, b, c), tri(a, c, d)) };
        let text = format!("solid q\n{t1}{t2}endsolid q\n");
        umcf_geom::stl::read(text.as_bytes(), &umcf_geom::stl::Limits::default()).unwrap()
    };
    let draw_pair = |other: bool| {
        let mut s = Scene::new(Vec3::new(2.0, 2.0, 0.0));
        s.objects.push(Object::from_stl("a.stl", &square(false), s.origin));
        s.objects.push(Object::from_stl("b.stl", &square(other), s.origin));
        let cam = Camera::fit(View::Iso, &s.bbox().unwrap(), 1.0);
        let mut st = Style { width: 120, height: 120, colour_by: ColourBy::Object, ..Style::default() };
        for k in 0..2 {
            st.display.insert((k, 0), Display { mode: Mode::Surface, opacity: 0.5 });
        }
        draw(&s, &cam, &st).rgba
    };
    let (alike, crossed) = (draw_pair(false), draw_pair(true));
    let off = alike.chunks(4).zip(crossed.chunks(4)).filter(|(p, q)| (0..3).any(|k| p[k].abs_diff(q[k]) > 4)).count();
    assert!(off < 10, "{off} pixels differ");
}
