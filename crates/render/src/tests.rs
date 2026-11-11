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

/// A row of `n` unit hexahedra along x (cell `i` from x = i to i + 1), its ends patch `ends`,
/// the rest `sides`, and the cells `zoned` in a cell zone `brick`.
pub(crate) fn row(n: usize, zoned: &[u32]) -> PolyMesh {
    row_swapped(n, zoned, false)
}

/// [`row`], with the first cell's two sides at y = 0 and y = 1 in each other's places (the same
/// owner, patch and sizes).
fn row_swapped(n: usize, zoned: &[u32], swap: bool) -> PolyMesh {
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    let p = |i: usize, j: usize, k: usize| i * 4 + j * 2 + k;
    let mut points = String::new();
    for i in 0..=n {
        for j in 0..2 {
            for k in 0..2 {
                points += &format!("({i} {j} {k})\n");
            }
        }
    }
    let x_face = |i: usize| vec![p(i, 0, 0), p(i, 1, 0), p(i, 1, 1), p(i, 0, 1)];
    let mut faces: Vec<Vec<usize>> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    let mut neighbour: Vec<usize> = Vec::new();
    for i in 1..n {
        faces.push(x_face(i));
        owner.push(i - 1);
        neighbour.push(i);
    }
    faces.push(x_face(0));
    owner.push(0);
    faces.push(x_face(n));
    owner.push(n - 1);
    for i in 0..n {
        faces.push(vec![p(i, 0, 0), p(i + 1, 0, 0), p(i + 1, 0, 1), p(i, 0, 1)]);
        faces.push(vec![p(i, 1, 0), p(i, 1, 1), p(i + 1, 1, 1), p(i + 1, 1, 0)]);
        faces.push(vec![p(i, 0, 0), p(i, 1, 0), p(i + 1, 1, 0), p(i + 1, 0, 0)]);
        faces.push(vec![p(i, 0, 1), p(i + 1, 0, 1), p(i + 1, 1, 1), p(i, 1, 1)]);
        owner.extend([i; 4]);
    }
    if swap {
        faces.swap(n + 1, n + 2);
    }
    let list =
        |v: &[usize]| format!("{}\n(\n{}\n)\n", v.len(), v.iter().map(usize::to_string).collect::<Vec<_>>().join(" "));
    let faces_text: String = faces
        .iter()
        .map(|f| format!("4({})\n", f.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")))
        .collect();
    let internal = n - 1;
    let zones = format!(
        "{}1\n(\nbrick\n{{\n    type cellZone;\n    cellLabels List<label> {}({});\n}}\n)\n",
        h("regIOobject"),
        zoned.len(),
        zoned.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")
    );
    let fs = MemFs::new()
        .with("m/points", format!("{}{}\n(\n{points})\n", h("vectorField"), (n + 1) * 4))
        .with("m/faces", format!("{}{}\n(\n{faces_text})\n", h("faceList"), faces.len()))
        .with("m/owner", format!("{}{}", h("labelList"), list(&owner)))
        .with("m/neighbour", format!("{}{}", h("labelList"), list(&neighbour)))
        .with("m/cellZones", zones)
        .with(
            "m/boundary",
            format!(
                "{}2\n(\n    ends\n    {{\n        type patch;\n        nFaces 2;\n        startFace {internal};\n    }}\n    sides\n    {{\n        type wall;\n        nFaces {};\n        startFace {};\n    }}\n)\n",
                h("polyBoundaryMesh"),
                4 * n,
                internal + 2
            ),
        );
    PolyMesh::read(&fs, "m", &Limits::default()).unwrap()
}

#[test]
fn a_cut_shows_the_cells_a_plane_crosses_whole() {
    use crate::Plane;
    let mesh = row(3, &[1]);
    assert_eq!(mesh.n_cells(), 3);
    let cut = |x: f64, normal: Vec3| {
        Object::from_cut("air", &mesh, Plane { point: Vec3::new(x, 0.5, 0.5), normal }, Vec3::ZERO)
    };
    // Through the middle cell: it alone, its six faces (two triangles each), in its zone.
    let o = cut(1.5, Vec3::new(1.0, 0.0, 0.0)).unwrap();
    assert_eq!(o.source, Source::Cut { region: "air".into() });
    assert_eq!(o.triangles.len(), 12);
    assert_eq!(o.classes, ["brick"]);
    assert_eq!(o.kinds, ["cellZone"]);
    let mut faces: Vec<u32> = o.origin_index.clone();
    faces.dedup();
    assert_eq!(faces.len(), 6);
    // Every point of the cell, none else: x from 1 to 2.
    assert_eq!(o.points.len(), 8);
    assert!(o.points.iter().all(|p| (1.0..=2.0).contains(&p[0])));
    // The face edges, not the diagonals: four a face.
    let edges: u32 = o.edges.iter().map(|e| e.count_ones()).sum();
    assert_eq!(edges, 6 * 4);
    // On the faces between cells 0 and 1: the layer behind the plane only, not both.
    let o = cut(1.0, Vec3::new(1.0, 0.0, 0.0)).unwrap();
    assert!(o.points.iter().all(|p| (0.0..=1.0).contains(&p[0])), "{:?}", o.points);
    assert_eq!(o.classes, ["cells"]);
    // Turned round, the other layer; the normal's length doesn't matter.
    let o = cut(1.0, Vec3::new(-7.0, 0.0, 0.0)).unwrap();
    assert!(o.points.iter().all(|p| (1.0..=2.0).contains(&p[0])), "{:?}", o.points);
    // Along the row: all three, the zone's and the others: their skin, not the faces between
    // them.
    let o = cut(0.0, Vec3::new(0.0, 1.0, 0.0)).unwrap();
    assert_eq!(o.triangles.len(), 2 * (2 + 12));
    assert_eq!(o.classes.len(), 2);
    // Past the mesh: nothing; a zero normal: no cut.
    assert!(cut(9.0, Vec3::new(1.0, 0.0, 0.0)).unwrap().triangles.is_empty());
    assert!(cut(1.5, Vec3::ZERO).is_none());
    assert!(cut(1.5, Vec3::new(f64::NAN, 0.0, 0.0)).is_none());
    // A face with no finite point (the far end of the last cell) tells nothing of its cell: the
    // last cell is not cut for it.
    let mut broken = row(3, &[1]);
    for p in broken.points.iter_mut().filter(|p| p[0] == 3.0) {
        *p = [f64::NAN; 3];
    }
    let o = Object::from_cut(
        "air",
        &broken,
        Plane { point: Vec3::new(1.5, 0.5, 0.5), normal: Vec3::new(1.0, 0.0, 0.0) },
        Vec3::ZERO,
    )
    .unwrap();
    assert_eq!(o.classes, ["brick"]);
    assert_eq!(o.triangles.len(), 12);
    // Nor does one at infinity (behind the plane, where it would make the last cell cut).
    for p in broken.points.iter_mut().filter(|p| p[0].is_nan()) {
        *p = [f64::NEG_INFINITY, 0.5, 0.5];
    }
    let o = Object::from_cut(
        "air",
        &broken,
        Plane { point: Vec3::new(1.5, 0.5, 0.5), normal: Vec3::new(1.0, 0.0, 0.0) },
        Vec3::ZERO,
    )
    .unwrap();
    assert_eq!(o.triangles.len(), 12);
}

#[test]
fn what_a_check_found_is_drawn_where_it_is() {
    use crate::{Found, FoundClass};
    let mesh = row(3, &[]);
    let class = |name: &str, cells: bool, labels: &[u32]| FoundClass {
        name: name.into(),
        cells,
        labels: labels.to_vec(),
        count: labels.len(),
    };
    // Face 0 (between cells 0 and 1, at x = 1) twice, a label past the mesh; cell 2 whole, and
    // cells 1 and 2 for another check.
    let found = Found {
        classes: vec![
            class("skew faces", false, &[0, 0, 9999]),
            class("open cells", true, &[2, 2]),
            class("concave cells", true, &[1, 2]),
        ],
        mesh: Found::mesh_of(&mesh),
    };
    let o = Object::from_found("air", &mesh, &found, Vec3::ZERO);
    assert_eq!(o.source, Source::Cut { region: "air".into() });
    assert_eq!(o.classes, ["skew faces", "open cells", "concave cells"]);
    assert_eq!(o.kinds, ["faces", "cells", "cells"]);
    let of = |k: u32| -> Vec<u32> {
        let mut f: Vec<u32> = o.origin_index.iter().zip(&o.class).filter(|(_, c)| **c == k).map(|(f, _)| *f).collect();
        f.dedup();
        f
    };
    // The face once, two triangles, at x = 1.
    assert_eq!(of(0), [0]);
    assert_eq!(o.class.iter().filter(|c| **c == 0).count(), 2);
    // Cell 2's six faces; cells 1 and 2: eleven (the face between them once).
    assert_eq!(of(1).len(), 6);
    assert_eq!(of(2).len(), 11);
    assert!(o.points.iter().all(|p| (1.0..=3.0).contains(&p[0])));
    // Nothing found: nothing drawn.
    assert!(Object::from_found("air", &mesh, &Found::default(), Vec3::ZERO).triangles.is_empty());
}

#[test]
fn a_mesh_whose_faces_trade_places_or_points_move_is_another_mesh() {
    use crate::Found;
    let (mesh, swapped) = (row(3, &[]), row_swapped(3, &[], true));
    // Same sizes, owners and neighbours: only the faces' points tell them apart.
    assert_eq!((mesh.n_faces(), &mesh.owner, &mesh.neighbour), (swapped.n_faces(), &swapped.owner, &swapped.neighbour));
    assert_ne!(mesh.face(4), swapped.face(4));
    assert_ne!(Found::mesh_of(&mesh), Found::mesh_of(&swapped));
    assert_eq!(Found::mesh_of(&mesh), Found::mesh_of(&row(3, &[])));
    // Nor is one moved: the same faces, a point elsewhere.
    let mut moved = row(3, &[]);
    moved.points[0][0] = -0.25;
    assert_ne!(Found::mesh_of(&mesh), Found::mesh_of(&moved));
}

#[test]
fn the_layers_at_a_patch_are_walked_through_the_opposite_faces() {
    let mesh = row(5, &[]);
    let classes_of = |o: &Object, k: u32| -> Vec<u32> {
        let mut f: Vec<u32> = o.origin_index.iter().zip(&o.class).filter(|(_, c)| **c == k).map(|(f, _)| *f).collect();
        f.dedup();
        f
    };
    // From both ends of the row: cells 0 and 4, then 1 and 3, then 2 (reached from x = 0
    // first; the column from x = 5 stops at it, short of nothing).
    let o = Object::from_layers("air", &mesh, "ends", 5, Vec3::ZERO).unwrap();
    assert_eq!(o.source, Source::Cut { region: "air".into() });
    assert_eq!(o.classes, ["layer 1: 2 cells", "layer 2: 2 cells", "layer 3: 1 cell"]);
    assert_eq!(o.kinds, ["layer"; 3]);
    // The skin (the 2 ends and 20 sides) and the 4 faces between layers: 26 faces, 52 triangles.
    assert_eq!(o.triangles.len(), 52);
    // The faces between two layers in the lower one's class: face 0 (cells 0|1) in layer 1,
    // face 1 (1|2) in layer 2.
    assert!(classes_of(&o, 0).contains(&0) && classes_of(&o, 1).contains(&1));
    // Two layers asked for: only those.
    let two = Object::from_layers("air", &mesh, "ends", 2, Vec3::ZERO).unwrap();
    assert_eq!(two.classes, ["layer 1: 2 cells", "layer 2: 2 cells"]);
    // A side's column ends at the opposite side, the boundary: one layer, nothing short.
    let sides = Object::from_layers("air", &mesh, "sides", 3, Vec3::ZERO).unwrap();
    assert_eq!(sides.classes, ["layer 1: 5 cells"]);
    // A patch the mesh doesn't have.
    let e = Object::from_layers("air", &mesh, "roof", 3, Vec3::ZERO).unwrap_err();
    assert!(e.contains("patch `roof`"), "{e}");
}

#[test]
fn a_column_stopped_short_marks_its_patch_face() {
    // One tetrahedron: every face shares a point with every other, so no face is opposite.
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    let fs = MemFs::new()
        .with("t/points", format!("{}4\n(\n(0 0 0)\n(1 0 0)\n(0 1 0)\n(0 0 1)\n)\n", h("vectorField")))
        .with("t/faces", format!("{}4\n(\n3(0 2 1)\n3(0 1 3)\n3(1 2 3)\n3(0 3 2)\n)\n", h("faceList")))
        .with("t/owner", format!("{}4\n(\n0 0 0 0\n)\n", h("labelList")))
        .with("t/neighbour", format!("{}0\n(\n)\n", h("labelList")))
        .with(
            "t/boundary",
            format!(
                "{}2\n(\n    walls\n    {{\n        type wall;\n        nFaces 4;\n        startFace 0;\n    }}\n    none\n    {{\n        type wall;\n        nFaces 0;\n        startFace 4;\n    }}\n)\n",
                h("polyBoundaryMesh")
            ),
        );
    let mesh = PolyMesh::read(&fs, "t", &Limits::default()).unwrap();
    let o = Object::from_layers("", &mesh, "walls", 2, Vec3::ZERO).unwrap();
    assert_eq!(o.classes, ["layer 1: 1 cell", "stopped short of 2 layers (a cell split or collapsed): 4 faces"]);
    assert_eq!(o.kinds, ["layer", "faces"]);
    // Its four faces, each a column of its own, all marked.
    assert_eq!(o.triangles.len(), 4);
    let marked: Vec<u32> = o.origin_index.iter().zip(&o.class).filter(|(_, c)| **c == 1).map(|(f, _)| *f).collect();
    assert_eq!(marked, [0, 1, 2, 3]);
    // A patch without faces: no layer.
    let none = Object::from_layers("", &mesh, "none", 3, Vec3::ZERO).unwrap();
    assert!(none.classes.is_empty() && none.triangles.is_empty());
    // One layer asked for: none short.
    assert_eq!(Object::from_layers("", &mesh, "walls", 1, Vec3::ZERO).unwrap().classes, ["layer 1: 1 cell"]);
}

/// A 2 × 2 × 1 block of unit cells (cell `i + 2 j` at x = i, y = j), its boundary the patch
/// `corner` (cell 0's faces at x = 0 and y = 0) and `rest`.
fn corner_block() -> PolyMesh {
    let h = |class: &str| format!("FoamFile\n{{\n    version 2.0;\n    format ascii;\n    class {class};\n}}\n");
    let p = |i: usize, j: usize, k: usize| k * 9 + j * 3 + i;
    let mut points = String::new();
    for k in 0..2 {
        for j in 0..3 {
            for i in 0..3 {
                points += &format!("({i} {j} {k})\n");
            }
        }
    }
    let x_face = |i: usize, j: usize| [p(i, j, 0), p(i, j + 1, 0), p(i, j + 1, 1), p(i, j, 1)];
    let y_face = |i: usize, j: usize| [p(i, j, 0), p(i, j, 1), p(i + 1, j, 1), p(i + 1, j, 0)];
    let z_face = |i: usize, j: usize, k: usize| [p(i, j, k), p(i + 1, j, k), p(i + 1, j + 1, k), p(i, j + 1, k)];
    // Internal (owner < neighbour, by owner), then `corner`, then `rest`.
    let mut faces: Vec<([usize; 4], usize)> =
        vec![(x_face(1, 0), 0), (y_face(0, 1), 0), (y_face(1, 1), 1), (x_face(1, 1), 2)];
    let neighbour = [1usize, 2, 3, 3];
    // At x = 0, y = 0 and z = 0 the faces turned round, so that each points out of its cell.
    let out = |f: [usize; 4]| [f[0], f[3], f[2], f[1]];
    faces.extend([(out(x_face(0, 0)), 0), (out(y_face(0, 0)), 0)]);
    faces.extend([(out(y_face(1, 0)), 1), (x_face(2, 0), 1), (out(x_face(0, 1)), 2), (y_face(0, 2), 2)]);
    faces.extend([(x_face(2, 1), 3), (y_face(1, 2), 3)]);
    for c in 0..4 {
        faces.extend([(out(z_face(c % 2, c / 2, 0)), c), (z_face(c % 2, c / 2, 1), c)]);
    }
    let text: String = faces.iter().map(|(f, _)| format!("4({} {} {} {})\n", f[0], f[1], f[2], f[3])).collect();
    let owner: Vec<String> = faces.iter().map(|(_, o)| o.to_string()).collect();
    let nb: Vec<String> = neighbour.iter().map(usize::to_string).collect();
    let patch = |n: &str, count: usize, start: usize| {
        format!("    {n}\n    {{\n        type wall;\n        nFaces {count};\n        startFace {start};\n    }}\n")
    };
    let fs = MemFs::new()
        .with("c/points", format!("{}18\n(\n{points})\n", h("vectorField")))
        .with("c/faces", format!("{}{}\n(\n{text})\n", h("faceList"), faces.len()))
        .with("c/owner", format!("{}{}\n(\n{}\n)\n", h("labelList"), owner.len(), owner.join(" ")))
        .with("c/neighbour", format!("{}4\n(\n{}\n)\n", h("labelList"), nb.join(" ")))
        .with(
            "c/boundary",
            format!("{}2\n(\n{}{})\n", h("polyBoundaryMesh"), patch("corner", 2, 4), patch("rest", faces.len() - 6, 6)),
        );
    PolyMesh::read(&fs, "c", &Limits::default()).unwrap()
}

#[test]
fn a_cell_at_a_corner_starts_a_column_from_each_of_its_patch_faces() {
    // Cell 0 at the corner: from its face at x = 0 the column goes on to cell 1, from the one at
    // y = 0 to cell 2; then both reach the boundary.
    let o = Object::from_layers("air", &corner_block(), "corner", 3, Vec3::ZERO).unwrap();
    assert_eq!(o.classes, ["layer 1: 1 cell", "layer 2: 2 cells"]);
}
