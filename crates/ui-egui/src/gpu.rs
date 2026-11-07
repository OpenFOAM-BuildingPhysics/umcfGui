//! The GPU view: the scene drawn with wgpu into a texture the window shows, as the CPU
//! rasteriser's image is shown. It draws what the CPU view draws for opaque surfaces, wireframes
//! and points: each class in its colour (the selection orange, in front of what lies in the same
//! place), flat-shaded with the light at the eye (per triangle, as the CPU view), the faces'
//! edges when asked for, wireframes and points over the surfaces (seen where nothing opaque is in
//! front, blended with their opacity), outlines where one class meets another or the background,
//! over the background's gradient, with an infinite far plane. Transparent layers are blended
//! over all that, order-independent (weighted by opacity and nearness): one layer as the CPU
//! view blends it; where layers of different colours lie over each other, the nearer counts
//! more rather than being drawn last. A device that can't blend half-float images leaves them
//! to the CPU view ([`Gpu::draws`]).
//!
//! Each object is uploaded once, as it is (its points, its triangles' corners and classes,
//! split into chunks a storage binding holds), and kept while the scene shows it; its edges and
//! points as wireframes and points draw them the first time a class shows them. What changes
//! from frame to frame is small: the camera, and a colour and a mode per class (the selection,
//! what is hidden, what is a wireframe). The vertex shader reads the corners from storage buffers
//! (vertex pulling), so a point shared by triangles of several classes is stored once, and each
//! vertex sees its whole triangle, to shade it as one and to find its edges on the screen.
//! Picking stays on the CPU (the scene's BVH), as do snapshots on the command line.

use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;
use umcf_render::camera::Camera;
use umcf_render::raster::{
    AHEAD, BOTTOM, ColourBy, EDGE, EDGE_REACH, FRONT as CPU_FRONT, LINE, Mode, OUTLINE, SELECTED, Style, TOP, dot_size,
    palette,
};
use umcf_render::{Object, Scene};

/// Depth-bias steps of the objects after the first (the last one for the rest), and how much
/// each is pushed back: in units of the depth's last bit, and per unit of the surface's depth
/// slope across a pixel.
const RANKS: usize = 8;
const BIAS: i32 = 8;
const SLOPE: f32 = 2.0;

/// The colours' texture's width: texels past it go on the next rows (the shader's 1024). Each
/// class has two: its colour and what it is (the alpha: one of the `CODE`s), then its opacity.
const PALETTE_WIDTH: u32 = 1024;
/// What a class is, in its colour's alpha.
const CODE_HIDDEN: f32 = 0.0;
const CODE_SURFACE: f32 = 1.0;
const CODE_SELECTED: f32 = 2.0;
const CODE_WIREFRAME: f32 = 3.0;
const CODE_POINTS: f32 = 4.0;
const CODE_GLASS: f32 = 5.0;
/// The transparent layers' sum of weighted colours and what they let through.
const SUMS: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const THROUGH: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// The most classes an object has here: a triangle's class shares its word with its edges (the
/// top three bits), an edge's or a point's with whether its face is selected (the top bit).
const MAX_CLASSES: usize = 1 << 29;
/// The image's format: as egui's own textures (`Rgba8Unorm` holding sRGB values, which egui's
/// shader reads as such), so the bytes are the CPU view's.
pub const COLOUR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The depth buffer's format: reversed (1 near, 0 at infinity), so that far things keep their
/// precision.
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Each pixel's class, for the outlines: 0 for the background, else 1 + the class's number in
/// the scene (the classes of the objects before it, then its own).
const ID: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

const SHADER: &str = r#"
struct View {
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    // x and y: from view to clip coordinates; z: the near plane's distance.
    lens: vec4<f32>,
    top: vec4<f32>,
    bottom: vec4<f32>,
    // x and y: the image's size in pixels; z: a point's side in pixels; w: the camera's distance
    // to what it looks at (the transparent layers' weights are relative to it).
    pixels: vec4<f32>,
};
struct Obj {
    // 0: the classes not selected; 1: the selected faces, in the selection's colour; 2: the
    // selected classes (drawn after the others, in front of what lies in the same place); 3:
    // the transparent classes; 4: the selected faces of those (what the selection's colour
    // changes of their colour).
    mode: u32,
    // The number of the object's first class in the scene, for the ids.
    base: u32,
    // 1: the faces' edges are drawn.
    edges: u32,
    pad: u32,
};
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<storage, read> points: array<f32>;
// The triangles' corners (three per triangle); for wireframes the edges' ends (two per edge),
// for points the points.
@group(1) @binding(1) var<storage, read> corners: array<u32>;
// Each triangle's class (its edges in the top three bits, as `Object::edges`); for wireframes and
// points each one's class (the top bit: its face is selected).
@group(1) @binding(2) var<storage, read> classes: array<u32>;
@group(1) @binding(3) var colours: texture_2d<f32>;
@group(1) @binding(4) var<uniform> obj: Obj;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
    @location(1) @interpolate(flat) id: u32,
    // The triangle's corners on the image (pixels from its top left): the first two, the third.
    @location(2) @interpolate(flat) ab: vec4<f32>,
    @location(3) @interpolate(flat) c: vec2<f32>,
    // Its edges drawn (bit 0: the first corner to the second, 1: the second to the third, 2:
    // the third to the first).
    @location(4) @interpolate(flat) edges: u32,
    // Its distance in front of the eye.
    @location(5) z: f32,
};

struct Pixel {
    @location(0) colour: vec4<f32>,
    @location(1) id: u32,
    // near / distance (as the depth buffer, but without the bias that puts one object or the
    // selection in front of another), as the bits of a float: what wireframes and points are
    // tested against, as in the CPU view.
    @location(2) depth: u32,
};
@group(2) @binding(0) var depths: texture_2d<u32>;

// Point `k`, in view coordinates.
fn at(k: u32) -> vec3<f32> {
    let p = vec3<f32>(points[3u * k], points[3u * k + 1u], points[3u * k + 2u]);
    let d = p - view.eye.xyz;
    return vec3<f32>(dot(d, view.right.xyz), dot(d, view.up.xyz), dot(d, view.forward.xyz));
}

// Corner `j` of the chunk, in view coordinates.
fn corner(j: u32) -> vec3<f32> {
    return at(corners[j]);
}

// Texel `i` of the colours: class c's colour and code at 2c, its opacity at 2c + 1.
fn texel(i: u32) -> vec4<f32> {
    return textureLoad(colours, vec2<i32>(i32(i % 1024u), i32(i / 1024u)), 0);
}

fn code(c: vec4<f32>) -> u32 {
    return u32(round(c.a));
}

// A point in view coordinates on the image, in pixels from its top left, as the CPU view.
fn on_image(v: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(
        (1.0 + v.x * view.lens.x / v.z) * 0.5 * view.pixels.x,
        (1.0 - v.y * view.lens.y / v.z) * 0.5 * view.pixels.y,
    );
}

// Outside the view: never drawn.
const OUTSIDE: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let t = i / 3u;
    let v = corner(i);
    let word = classes[t];
    let which = word & 0x1fffffffu;
    let c = texel(2u * which);
    let kind = code(c);
    let glass = obj.mode >= 3u;
    var shown = kind == 1u || kind == 2u;
    if glass {
        shown = kind == 5u;
    }
    let selected = kind == 2u;
    var colour = c.rgb;
    if obj.mode == 1u && shown {
        // A selected face of a class shown: the selection's colour, in front.
        colour = SELECTED;
    }
    if obj.mode == 4u {
        // A selected face of a transparent class: what turns its colour into the selection's
        // (added to the class's colour drawn in mode 3).
        colour = SELECTED - c.rgb;
    }
    // Lambert with the light at the eye, both sides lit alike, for the whole triangle (its
    // normal against the direction to its middle), as the CPU view.
    let a = corner(3u * t);
    let b = corner(3u * t + 1u);
    let e = corner(3u * t + 2u);
    let n = cross(b - a, e - a);
    let g = (a + b + e) / 3.0;
    let m = length(n) * length(g);
    var cosine = 1.0;
    if m > 0.0 {
        cosine = abs(dot(n, g)) / m;
    }
    let shade = 0.3 + 0.7 * cosine;
    var out: Out;
    // Each pass draws its own classes (the others are put outside the view, never drawn): mode
    // 0 the surfaces shown, mode 2 the selected; mode 1 the selected faces of the surfaces.
    var skip = !shown;
    if obj.mode == 0u && selected {
        skip = true;
    }
    if obj.mode == 2u && !selected {
        skip = true;
    }
    // Reversed depth: near / distance.
    out.pos = vec4<f32>(v.x * view.lens.x, v.y * view.lens.y, view.lens.z, v.z);
    out.colour = vec4<f32>(colour * shade, 1.0);
    if glass {
        out.colour.a = texel(2u * which + 1u).r;
    }
    if skip {
        out.pos = OUTSIDE;
        out.colour.a = -1.0;
    }
    out.id = obj.base + which + 1u;
    // Its edges on the image, where the whole triangle is in front of the near plane (the
    // pieces of one cut by it get none, as in the CPU view).
    let near = view.lens.z;
    out.edges = 0u;
    if obj.edges == 1u && a.z >= near && b.z >= near && e.z >= near {
        out.edges = word >> 29u;
    }
    out.ab = vec4<f32>(on_image(a), on_image(b));
    out.c = on_image(e);
    out.z = v.z;
    return out;
}

// Whether pixel middle `x` is within EDGE_REACH pixels of the line through `p` and `q`.
fn on_edge(p: vec2<f32>, q: vec2<f32>, x: vec2<f32>) -> bool {
    let d = q - p;
    let l = length(d);
    if !(l > 0.0) {
        return false;
    }
    return abs(d.x * (x.y - p.y) - d.y * (x.x - p.x)) / l < EDGE_REACH;
}

// How much the faces' edges darken pixel `x` of the triangle `in`: EDGE on one, else 1.
fn edging(in: Out) -> f32 {
    let x = in.pos.xy;
    if (in.edges & 1u) != 0u && on_edge(in.ab.xy, in.ab.zw, x) { return EDGE; }
    if (in.edges & 2u) != 0u && on_edge(in.ab.zw, in.c, x) { return EDGE; }
    if (in.edges & 4u) != 0u && on_edge(in.c, in.ab.xy, x) { return EDGE; }
    return 1.0;
}

// An opaque surface's pixel: its colour (darkened on an edge), its class and its depth.
fn surface(in: Out) -> Pixel {
    if in.colour.a < 0.5 {
        discard;
    }
    let k = edging(in);
    var out: Pixel;
    out.colour = vec4<f32>(in.colour.rgb * k, 1.0);
    out.id = in.id;
    // The selection a little nearer, as the CPU view stores it: what lies in the same place
    // (a transparent layer, a wireframe) is behind it.
    var depth = view.lens.z / in.z;
    if obj.mode == 1u || obj.mode == 2u {
        depth = depth * (1.0 + CPU_FRONT);
    }
    out.depth = bitcast<u32>(depth);
    return out;
}

@fragment
fn fs(in: Out) -> Pixel {
    return surface(in);
}

// The selection (selected classes, selected faces): as `fs`, its depth 1 + DEPTH_FRONT nearer
// (CPU_FRONT, as the CPU view puts it, where the depth buffer keeps that precision), so that it
// is seen over what lies in the same place, and only there (a bias that grows with the
// surface's slope would bring surfaces behind others to the front, near their edges).
struct Front {
    @location(0) colour: vec4<f32>,
    @location(1) id: u32,
    @location(2) depth: u32,
    @builtin(frag_depth) z: f32,
};

@fragment
fn fs_front(in: Out) -> Front {
    let p = surface(in);
    var out: Front;
    out.colour = p.colour;
    out.id = p.id;
    out.depth = p.depth;
    out.z = min(in.pos.z * (1.0 + DEPTH_FRONT), 1.0);
    return out;
}

// Wireframes and points: in their class's colour (the selection's where their face is
// selected), darkened, blended with the class's opacity, in a pass of their own over the
// surfaces, seen where no surface is nearer by more than AHEAD (on the surface they lie on, as
// in the CPU view).
struct Mark {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
    // Its distance in front of the eye: its depth is found from it, not from the position's
    // (which GL without clip control maps less precisely).
    @location(1) z: f32,
};

// Point `k` of a class `word` (as `classes` holds it) that draws when the class is `wanted`.
fn mark(k: u32, word: u32, wanted: u32) -> Mark {
    let which = word & 0x7fffffffu;
    let c = texel(2u * which);
    var out: Mark;
    out.pos = OUTSIDE;
    out.colour = vec4<f32>(0.0);
    out.z = 1.0;
    if code(c) != wanted {
        return out;
    }
    var colour = c.rgb;
    if (word >> 31u) == 1u {
        colour = SELECTED;
    }
    let v = at(k);
    out.pos = vec4<f32>(v.x * view.lens.x, v.y * view.lens.y, view.lens.z * (1.0 + AHEAD), v.z);
    out.colour = vec4<f32>(colour * LINE, texel(2u * which + 1u).r);
    out.z = v.z;
    return out;
}

@vertex
fn vs_line(@builtin(vertex_index) i: u32) -> Mark {
    return mark(corners[i], classes[i / 2u], 3u);
}

@vertex
fn vs_dot(@builtin(vertex_index) i: u32) -> Mark {
    let n = i / 6u;
    let k = corners[n];
    var out = mark(k, classes[n], 4u);
    let v = at(k);
    if out.colour.a <= 0.0 || !(v.z >= view.lens.z) {
        out.pos = OUTSIDE;
        return out;
    }
    // A square on the pixels the CPU view fills: from the nearest pixel corner to its top left,
    // its side rounded; two triangles of corners (0, 0) (1, 0) (0, 1), (1, 0) (1, 1) (0, 1).
    let side = view.pixels.z;
    let first = floor(on_image(v) - vec2<f32>(side * 0.5) + vec2<f32>(0.5));
    let j = i % 6u;
    let unit = vec2<f32>(select(0.0, 1.0, j == 1u || j == 3u || j == 4u), select(0.0, 1.0, j == 2u || j == 4u || j == 5u));
    let p = first + unit * max(floor(side + 0.5), 1.0);
    // Back to clip coordinates, at the point's depth.
    let ndc = vec2<f32>(p.x / view.pixels.x * 2.0 - 1.0, 1.0 - p.y / view.pixels.y * 2.0);
    out.pos = vec4<f32>(ndc * v.z, view.lens.z * (1.0 + AHEAD), v.z);
    return out;
}

@fragment
fn fs_mark(in: Mark) -> @location(0) vec4<f32> {
    // Seen where its depth, a little nearer (AHEAD), is in front of the surface's: the CPU
    // view's test.
    let under = bitcast<f32>(textureLoad(depths, vec2<i32>(in.pos.xy), 0).r);
    if !(in.colour.a > 0.0) || !(view.lens.z * (1.0 + AHEAD) / in.z > under) {
        discard;
    }
    return in.colour;
}

// Transparent layers, in any order (weighted, blended: order-independent): what each adds to
// the sum of the layers' colours (weighted by opacity, and by nearness so that the nearer
// counts more where layers of different colours lie over each other), and to the sum of
// -log(1 - opacity), from which what they let through together is found (both sums added, so
// the two images blend alike, which every device can). Seen where no surface is nearer, as in
// the CPU view.
struct Glass {
    @location(0) sum: vec4<f32>,
    @location(1) through: f32,
};

@fragment
fn fs_glass(in: Out) -> Glass {
    let a = in.colour.a;
    let under = bitcast<f32>(textureLoad(depths, vec2<i32>(in.pos.xy), 0).r);
    if !(a > 0.0) || !(view.lens.z / in.z > under) {
        discard;
    }
    let k = edging(in);
    // A tenth of the weight, which leaves the mean colour as it is: the half-float sums
    // overflow (the pixel's layers are left out) only past thousands of near layers, while far
    // ones (1e-3) stay within half-float precision.
    let w = clamp(pow(view.pixels.w / in.z, 4.0), 0.01, 100.0) * 0.1;
    var out: Glass;
    if obj.mode == 4u {
        // Only the colour changes: neither the layer's weight nor what it lets through.
        out.sum = vec4<f32>(in.colour.rgb * k * a * w, 0.0);
        out.through = 0.0;
    } else {
        out.sum = vec4<f32>(in.colour.rgb * k * a * w, a * w);
        out.through = -log(1.0 - min(a, 0.9999));
    }
    return out;
}

struct Back {
    @builtin(position) pos: vec4<f32>,
    @location(0) s: f32,
};

@vertex
fn vs_back(@builtin(vertex_index) i: u32) -> Back {
    // One triangle over the whole image.
    let p = vec2<f32>(f32((i << 1u) & 2u) * 2.0 - 1.0, f32(i & 2u) * 2.0 - 1.0);
    var out: Back;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.s = (1.0 - p.y) * 0.5;
    return out;
}

@fragment
fn fs_back(in: Back) -> Pixel {
    var out: Pixel;
    out.colour = vec4<f32>(mix(view.top.rgb, view.bottom.rgb, clamp(in.s, 0.0, 1.0)), 1.0);
    out.id = 0u;
    out.depth = 0u;
    return out;
}
"#;

/// Darkens the pixels where one class meets another or the background, as the CPU view.
const OUTLINES: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var ids: texture_2d<u32>;
// The transparent layers: their weighted colours and weights, and what they let through.
@group(0) @binding(2) var sums: texture_2d<f32>;
@group(0) @binding(3) var through: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((i << 1u) & 2u) * 2.0 - 1.0, f32(i & 2u) * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(ids));
    let p = vec2<i32>(pos.xy);
    let id = textureLoad(ids, p, 0).r;
    var c = textureLoad(image, p, 0);
    // The transparent layers over what is behind them: one layer as the CPU view blends it.
    // Sums that overflowed half floats (hundreds of near layers) are left out.
    let sum = textureLoad(sums, p, 0);
    let covered = 1.0 - exp(-textureLoad(through, p, 0).r);
    if covered > 0.0 && sum.a > 0.0 && all(abs(sum) < vec4<f32>(65000.0)) {
        c = vec4<f32>(sum.rgb / sum.a * covered + c.rgb * (1.0 - covered), 1.0);
    }
    var outline = false;
    if id != 0u {
        if p.x + 1 < size.x && textureLoad(ids, p + vec2<i32>(1, 0), 0).r != id { outline = true; }
        if p.x > 0 && textureLoad(ids, p - vec2<i32>(1, 0), 0).r != id { outline = true; }
        if p.y + 1 < size.y && textureLoad(ids, p + vec2<i32>(0, 1), 0).r != id { outline = true; }
        if p.y > 0 && textureLoad(ids, p - vec2<i32>(0, 1), 0).r != id { outline = true; }
    }
    if outline {
        return vec4<f32>(c.rgb * OUTLINE, 1.0);
    }
    return c;
}
"#;

/// Why the GPU view didn't draw a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotDrawn {
    /// The CPU draws this one (transparent layers on a device that can't blend half floats, an
    /// object too big for the device); the next may be the GPU's again.
    Here(String),
    /// The GPU failed (out of memory, a limit of the device): the view goes back to the CPU.
    Failed(String),
}

/// Whether a wgpu adapter can draw this view: it needs storage buffers in the vertex shader.
pub fn usable(adapter: &wgpu::Adapter) -> Result<(), String> {
    let caps = adapter.get_downlevel_capabilities();
    let limits = adapter.limits();
    if !caps.flags.contains(wgpu::DownlevelFlags::VERTEX_STORAGE) || limits.max_storage_buffers_per_shader_stage < 3 {
        let info = adapter.get_info();
        return Err(format!("{} ({:?}) has no storage buffers in vertex shaders", info.name, info.backend));
    }
    Ok(())
}

/// The camera as the shaders take it: `View` in the shader.
fn view_uniform(scene: &Scene, camera: &Camera, size: [u32; 2], background: [[f32; 3]; 2]) -> [f32; 32] {
    let f = camera.frame();
    let eye = f.eye - scene.origin;
    let (w, h) = (f64::from(size[0].max(1)), f64::from(size[1].max(1)));
    // As the CPU view's lens: the focal length in pixels over half the image's side.
    let fy = 1.0 / f.tan_half;
    let fx = fy * h / w;
    let near = (camera.distance * 1e-4).max(1e-9);
    let v = |x: f64, y: f64, z: f64| [x as f32, y as f32, z as f32, 0.0];
    let [top, bottom] = background;
    let rows = [
        v(eye.x, eye.y, eye.z),
        v(f.right.x, f.right.y, f.right.z),
        v(f.up.x, f.up.y, f.up.z),
        v(f.forward.x, f.forward.y, f.forward.z),
        v(fx, fy, near),
        [top[0], top[1], top[2], 1.0],
        [bottom[0], bottom[1], bottom[2], 1.0],
        [w as f32, h as f32, dot_size(size[1]) as f32, camera.distance as f32],
    ];
    let mut out = [0.0; 32];
    for (o, x) in out.iter_mut().zip(rows.iter().flatten()) {
        *o = *x;
    }
    out
}

/// Each class's two texels in the GPU view: its colour (or the selection's) with what it is (one
/// of the `CODE`s: hidden, a surface, a selected surface, a wireframe, points, a transparent
/// surface), then its opacity.
/// A wireframe or points of opacity 0 are hidden (the CPU view doesn't draw them).
fn class_colours(scene: &Scene, k: usize, style: &Style) -> Vec<[f32; 8]> {
    let Some(obj) = scene.objects.get(k) else { return Vec::new() };
    (0..obj.classes.len())
        .map(|c| {
            let c32 = u32::try_from(c).unwrap_or(u32::MAX);
            let selected = style.selected.contains(&(k, c32));
            let colour = if selected {
                SELECTED
            } else {
                match style.colour_by {
                    ColourBy::Class => palette(c),
                    ColourBy::Object => palette(k),
                }
            };
            let d = style.display_of(k, c32);
            let code = match d.mode {
                Mode::Hidden => CODE_HIDDEN,
                // Not drawn here: a deliberate difference (the CPU view sends it to its
                // transparent layers, whose blend with a NaN opacity is undefined); the window's
                // display card only takes opacities from 0 to 1.
                Mode::Surface if d.opacity.is_nan() => CODE_HIDDEN,
                // Transparent (an opacity of 0 is drawn as one, and adds nothing, as on the CPU).
                Mode::Surface if d.opacity < 1.0 => CODE_GLASS,
                Mode::Surface if selected => CODE_SELECTED,
                Mode::Surface => CODE_SURFACE,
                _ if d.opacity.is_nan() || d.opacity <= 0.0 => CODE_HIDDEN,
                Mode::Wireframe => CODE_WIREFRAME,
                Mode::Points => CODE_POINTS,
            };
            [colour[0], colour[1], colour[2], code, d.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0]
        })
        .collect()
}

/// Whether a class of object `k` is a transparent surface.
fn has_glass(scene: &Scene, k: usize, style: &Style) -> bool {
    let n = scene.objects.get(k).map_or(0, |o| o.classes.len());
    (0..n).any(|c| {
        let d = style.display_of(k, u32::try_from(c).unwrap_or(u32::MAX));
        d.mode == Mode::Surface && d.opacity < 1.0 && d.opacity > 0.0
    })
}

/// Whether a class of object `k` is drawn with `mode` (of an opacity above 0).
fn shows(scene: &Scene, k: usize, style: &Style, mode: Mode) -> bool {
    let n = scene.objects.get(k).map_or(0, |o| o.classes.len());
    (0..n).any(|c| {
        let d = style.display_of(k, u32::try_from(c).unwrap_or(u32::MAX));
        d.mode == mode && d.opacity > 0.0
    })
}

/// Edges or points of an object on the GPU, in chunks a storage binding holds: each chunk's
/// bind group, how many it has, and the buffer of their classes, written again (with the
/// selected faces' marked) when the selected faces change.
type Marks = Vec<(wgpu::BindGroup, u32, wgpu::Buffer)>;

/// `obj`'s edges or points (`lines`) in `range` as the shader takes them, as bytes: each one's
/// triangle's class, the top bit set where its face is one of `faces` (sorted). One chunk at a
/// time, so that no copy of a whole object's is held.
fn marks(obj: &Object, lines: bool, faces: &[u32], range: std::ops::Range<usize>) -> Vec<u8> {
    let wire = obj.wire();
    let word = |t: u32| {
        let class = obj.class.get(t as usize).copied().unwrap_or(0) & 0x7fff_ffff;
        let picked =
            !faces.is_empty() && obj.origin_index.get(t as usize).is_some_and(|f| faces.binary_search(f).is_ok());
        class | (u32::from(picked) << 31)
    };
    let bytes = |w: u32| w.to_le_bytes();
    if lines {
        wire.edges.get(range).unwrap_or(&[]).iter().flat_map(|e| bytes(word(e[2]))).collect()
    } else {
        wire.points.get(range).unwrap_or(&[]).iter().flat_map(|p| bytes(word(p[1]))).collect()
    }
}

/// A triangle's class as the shader takes it: its edges (`Object::edges`) in the top three bits.
fn with_edges(class: u32, edges: u8) -> u32 {
    (class & 0x1fff_ffff) | (u32::from(edges & 0b111) << 29)
}

/// What an object of the scene is on the GPU.
struct Uploaded {
    /// What draws it: for chunks of its triangles, a bind group for the classes shown, one for
    /// the selected classes and one for the transparent classes (sharing the chunk's buffers),
    /// and how many triangles each has.
    chunks: Vec<([wgpu::BindGroup; 3], u32)>,
    palette: wgpu::Texture,
    points: wgpu::Buffer,
    /// Its uniforms, by mode: the classes shown, the selected faces, the selected classes, the
    /// transparent classes, their selected faces.
    uniforms: [wgpu::Buffer; 5],
    /// The selected faces it was last drawn with, and their triangles in chunks (none: no
    /// triangle has them), drawn over it in the selection's colour: a bind group for the
    /// opaque surfaces' and one for the transparent ones'.
    selected: (Vec<u32>, Vec<([wgpu::BindGroup; 2], u32)>),
    /// The selection its selected faces were last looked at for.
    looked: Option<u64>,
    /// Its edges and its points, as wireframes and points draw them, once a class showed them.
    lines: Option<Marks>,
    dots: Option<Marks>,
}

/// The image drawn (`texture`) and what it is drawn from.
pub struct Target {
    pub size: [u32; 2],
    /// The image itself (the window shows `view`; tests read it back).
    #[cfg_attr(not(test), allow(dead_code))]
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    scene: wgpu::TextureView,
    ids: wgpu::TextureView,
    /// The surfaces' depths without bias, and what reads them (wireframes and points).
    depths: wgpu::TextureView,
    marks: wgpu::BindGroup,
    /// The transparent layers' weighted colours and what they let through (cleared each frame).
    sums: wgpu::TextureView,
    through: wgpu::TextureView,
    depth: wgpu::TextureView,
    outline: wgpu::BindGroup,
}

/// The GPU view: its pipelines and what it has uploaded.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The surfaces of the objects, by rank (the first, the second, …, the rest).
    surfaces: Vec<wgpu::RenderPipeline>,
    selected: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    dots: wgpu::RenderPipeline,
    /// The transparent layers, where the device blends half-float images (else the CPU view
    /// draws them).
    glass: Option<wgpu::RenderPipeline>,
    back: wgpu::RenderPipeline,
    outline: wgpu::RenderPipeline,
    object_layout: wgpu::BindGroupLayout,
    depths_layout: wgpu::BindGroupLayout,
    outline_layout: wgpu::BindGroupLayout,
    view_buffer: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    objects: HashMap<u64, Uploaded>,
    /// The most a storage binding holds, bytes.
    binding: u64,
    /// The most a chunk of triangles, edges or points holds, bytes: `binding` (smaller in tests,
    /// to split a small object).
    chunk: u64,
    target: Option<Target>,
    /// An error the device reported outside the scopes this view sets (wgpu's default panics).
    uncaptured: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// What wakes the window when the device reports one.
    wake: Wake,
}

/// What wakes the window (egui's repaint), set when the view is attached to it.
type Wake = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>>;

/// Keeps the first error the device reported, and wakes the window to show it.
fn keep(slot: &std::sync::Mutex<Option<String>>, wake: &Wake, error: String) {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert(error);
    // Called outside the lock: a wake that set another one would wait for itself.
    let w = wake.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if let Some(w) = w {
        w();
    }
}

impl Gpu {
    /// The GPU view on `device`; an error when it can't draw there.
    pub fn new(adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Gpu, String> {
        usable(adapter)?;
        let limits = device.limits();
        if limits.max_storage_buffers_per_shader_stage < 3 {
            return Err("the device has too few storage buffers per shader stage".to_string());
        }
        // Errors not caught by a scope are kept, and logged, not a panic (wgpu's default): the
        // next frame reports them, and the view goes back to the CPU. They may be egui's own
        // (it shares the device): the message says the device reported them. A lost device (a
        // driver reset) too.
        let uncaptured: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
        // The window is woken to show what went wrong, also while its image doesn't change.
        let wake: Wake = Default::default();
        let (kept, woken) = (uncaptured.clone(), wake.clone());
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            eprintln!("umcf-gui: the GPU device reported: {e}");
            keep(&kept, &woken, format!("the GPU device reported: {e}"));
        }));
        let (kept, woken) = (uncaptured.clone(), wake.clone());
        device.set_device_lost_callback(move |_, message| {
            eprintln!("umcf-gui: the GPU device was lost: {message}");
            keep(&kept, &woken, format!("the GPU device was lost: {message}"));
        });
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        // How much nearer the selection's depth is written: the CPU view's 1e-5, but where
        // the depth goes through -1..1 (OpenGL, as wgpu maps it there) that is below the
        // precision of depths near the middle of the range: 1e-4.
        let depth_front = if adapter.get_info().backend == wgpu::Backend::Gl { 1e-4 } else { CPU_FRONT };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("umcf view"),
            source: wgpu::ShaderSource::Wgsl(with_constants(SHADER, depth_front).into()),
        });
        let outline_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("umcf outlines"),
            source: wgpu::ShaderSource::Wgsl(with_constants(OUTLINES, depth_front).into()),
        });
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, visibility, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("umcf view"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("umcf object"),
            entries: &[
                storage(0),
                storage(1),
                storage(2),
                texture(3, wgpu::ShaderStages::VERTEX, wgpu::TextureSampleType::Float { filterable: false }),
                uniform(4, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });
        let outline_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("umcf outlines"),
            entries: &[
                texture(0, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Float { filterable: false }),
                texture(1, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Uint),
                texture(2, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Float { filterable: false }),
                texture(3, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let layout_of = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let depths_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("umcf depths"),
            entries: &[texture(0, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Uint)],
        });
        let layout = layout_of("umcf view", &[Some(&view_layout), Some(&object_layout)]);
        let marks_layout = layout_of("umcf marks", &[Some(&view_layout), Some(&object_layout), Some(&depths_layout)]);
        let back_layout = layout_of("umcf background", &[Some(&view_layout)]);
        let outline_pipeline_layout = layout_of("umcf outlines", &[Some(&outline_layout)]);
        let targets = [
            Some(wgpu::ColorTargetState { format: COLOUR, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: ID, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: ID, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        // Wireframes and points: blended over what is there with their opacity (the image's
        // alpha stays 1), in a pass of their own.
        let over = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let blended =
            [Some(wgpu::ColorTargetState { format: COLOUR, blend: Some(over), write_mask: wgpu::ColorWrites::ALL })];
        // What a pipeline draws: triangles that fill the depth and the ids, or marks (lines or
        // triangles) blended over them, tested against the depths the triangles wrote.
        let pipeline =
            |label, layout: &wgpu::PipelineLayout, vs, fs, compare, bias, marks: Option<wgpu::PrimitiveTopology>| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some(vs),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: marks.unwrap_or(wgpu::PrimitiveTopology::TriangleList),
                        ..wgpu::PrimitiveState::default()
                    },
                    depth_stencil: marks.is_none().then_some(wgpu::DepthStencilState {
                        format: DEPTH,
                        depth_write_enabled: Some(compare != wgpu::CompareFunction::Always),
                        depth_compare: Some(compare),
                        stencil: wgpu::StencilState::default(),
                        bias,
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fs),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: if marks.is_some() { &blended } else { &targets },
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        // Where surfaces of several objects lie in the same place the first is seen, as in the
        // CPU view: each later object a little farther, more where a surface is seen at a
        // grazing angle (its depth changes fast across a pixel, and so does its rounding).
        let surfaces: Vec<wgpu::RenderPipeline> = (0..RANKS)
            .map(|rank| {
                let r = i32::try_from(rank).unwrap_or(0);
                let bias = wgpu::DepthBiasState { constant: -BIAS * r, slope_scale: -SLOPE * r as f32, clamp: 0.0 };
                pipeline("umcf surfaces", &layout, "vs", "fs", wgpu::CompareFunction::Greater, bias, None)
            })
            .collect();
        // The selection in front of what lies in the same place, whichever object it is in;
        // the selected faces over their own triangles too: its fragments write their depth a
        // little nearer (`fs_front`), as the CPU view does, no bias.
        let selected = pipeline(
            "umcf selection",
            &layout,
            "vs",
            "fs_front",
            wgpu::CompareFunction::GreaterEqual,
            wgpu::DepthBiasState::default(),
            None,
        );
        // Wireframes and points test their depth themselves, against the surfaces' without the
        // bias above, as the CPU view does.
        let none = wgpu::DepthBiasState::default();
        let greater = wgpu::CompareFunction::Greater;
        let lines = pipeline(
            "umcf wireframes",
            &marks_layout,
            "vs_line",
            "fs_mark",
            greater,
            none,
            Some(wgpu::PrimitiveTopology::LineList),
        );
        let dots = pipeline(
            "umcf points",
            &marks_layout,
            "vs_dot",
            "fs_mark",
            greater,
            none,
            Some(wgpu::PrimitiveTopology::TriangleList),
        );
        // The transparent layers: their weighted colours and what they let through (as
        // -log) added up, where the device can render and blend half floats; one blend for
        // both images (no independent blending needed).
        let blends = |f: wgpu::TextureFormat| {
            let features = adapter.get_texture_format_features(f);
            features.allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
                && features.flags.contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
        };
        let glass = (blends(SUMS) && blends(THROUGH)).then(|| {
            let add = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("umcf transparent layers"),
                layout: Some(&marks_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_glass"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: SUMS,
                            blend: Some(wgpu::BlendState { color: add, alpha: add }),
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: THROUGH,
                            blend: Some(wgpu::BlendState { color: add, alpha: add }),
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let back =
            pipeline("umcf background", &back_layout, "vs_back", "fs_back", wgpu::CompareFunction::Always, none, None);
        let outline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("umcf outlines"),
            layout: Some(&outline_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &outline_module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &outline_module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOUR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("umcf view"),
            size: 32 * 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("umcf view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view_buffer.as_entire_binding() }],
        });
        let binding = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        // What went wrong while building it fails it (nothing was built).
        let failed = [
            pollster::block_on(internal.pop()),
            pollster::block_on(validation.pop()),
            pollster::block_on(memory.pop()),
        ];
        if let Some(e) = failed.into_iter().flatten().next() {
            return Err(format!("the GPU view can't be built on this device: {e}"));
        }
        if let Some(e) = uncaptured.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            return Err(format!("the GPU device reported: {e}"));
        }
        Ok(Gpu {
            device: device.clone(),
            queue: queue.clone(),
            surfaces,
            selected,
            lines,
            dots,
            glass,
            back,
            outline,
            object_layout,
            depths_layout,
            outline_layout,
            view_buffer,
            view_group,
            objects: HashMap::new(),
            binding,
            chunk: binding,
            target: None,
            uncaptured,
            wake,
        })
    }

    /// Whether the device has reported an error the next draw fails with.
    pub fn failed(&self) -> bool {
        self.uncaptured.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Wakes the window with `wake` when the device reports an error, so that it is shown
    /// though nothing else changes.
    pub fn wake_with(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.wake.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(wake));
    }

    /// Whether this view draws `style` as the CPU view would: opaque and transparent surfaces and
    /// their edges, wireframes, points, hidden layers, the selection; `Err` names what it can't
    /// (transparent layers, on a device that can't blend half-float images).
    pub fn draws(&self, style: &Style) -> Result<(), &'static str> {
        if self.glass.is_none() && style.display.values().any(|d| d.mode == Mode::Surface && d.opacity < 1.0) {
            return Err("transparent layers");
        }
        Ok(())
    }

    /// Whether `obj` fits the device: its points in one storage binding (its triangles, edges
    /// and points are split), its classes' colours in one texture.
    fn fits(&self, obj: &Object) -> bool {
        let rows = (obj.classes.len() as u64).saturating_mul(2).div_ceil(u64::from(PALETTE_WIDTH));
        obj.classes.len() < MAX_CLASSES
            && (obj.points.len() as u64).saturating_mul(12) <= self.binding
            && rows <= u64::from(self.device.limits().max_texture_dimension_2d)
    }

    /// Draws `scene` as `camera` sees it into an image of `size` pixels; the image, or why it
    /// wasn't drawn.
    ///
    /// `selection` numbers the selection: an object's selected faces are looked at again only
    /// when it changes (a frame of a view that turns doesn't sort them again).
    pub fn draw(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        style: &Style,
        size: [u32; 2],
        selection: u64,
    ) -> Result<&Target, NotDrawn> {
        // An error the device reported first: the view goes back to the CPU, whatever it shows.
        if let Some(e) = self.uncaptured.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            self.objects.clear();
            self.target = None;
            return Err(NotDrawn::Failed(e));
        }
        self.draws(style).map_err(|what| NotDrawn::Here(format!("{what} are drawn by the CPU view")))?;
        if let Some(o) = scene.objects.iter().find(|o| !self.fits(o)) {
            return Err(NotDrawn::Here(format!("{} is too big for one buffer of the GPU", o.label())));
        }
        // What the device reports while drawing (out of memory, a limit) fails the frame.
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        self.draw_unguarded(scene, camera, style, size, selection);
        let errors = [
            pollster::block_on(internal.pop()),
            pollster::block_on(validation.pop()),
            pollster::block_on(memory.pop()),
        ];
        if let Some(e) = errors.into_iter().flatten().next() {
            // What was uploaded may be part of the problem (memory): it goes.
            self.objects.clear();
            self.target = None;
            return Err(NotDrawn::Failed(e.to_string()));
        }
        self.target.as_ref().ok_or_else(|| NotDrawn::Failed("no image to draw into".to_string()))
    }

    /// [`Gpu::draw`]'s work, its errors caught by the device's scopes around it.
    fn draw_unguarded(&mut self, scene: &Scene, camera: &Camera, style: &Style, size: [u32; 2], selection: u64) {
        let max = self.device.limits().max_texture_dimension_2d;
        let size = [size[0].clamp(1, max), size[1].clamp(1, max)];
        // What is no longer shown goes; what is new is uploaded.
        let ids: Vec<u64> = scene.objects.iter().map(Object::id).collect();
        self.objects.retain(|id, _| ids.contains(id));
        for obj in &scene.objects {
            if !self.objects.contains_key(&obj.id()) {
                let up = self.upload(obj);
                self.objects.insert(obj.id(), up);
            }
        }
        self.queue.write_buffer(
            &self.view_buffer,
            0,
            &bytes_f32(&view_uniform(scene, camera, size, style.background.unwrap_or(BACKGROUND))),
        );
        // Colours per class, the ids' bases and the selected faces, for this frame.
        let mut base: u32 = 0;
        for (k, obj) in scene.objects.iter().enumerate() {
            let colours = class_colours(scene, k, style);
            let Some(up) = self.objects.get(&obj.id()) else { continue };
            write_palette(&self.queue, &up.palette, &colours);
            for (mode, u) in up.uniforms.iter().enumerate() {
                let words = [u32::try_from(mode).unwrap_or(0), base, u32::from(style.edges), 0];
                self.queue.write_buffer(u, 0, &bytes_u32(&words));
            }
            base = base.saturating_add(u32::try_from(obj.classes.len()).unwrap_or(u32::MAX));
            if up.looked != Some(selection) {
                let mut faces: Vec<u32> =
                    style.selected_faces.iter().filter(|(o, _)| *o == k).map(|(_, f)| *f).collect();
                faces.sort_unstable();
                faces.dedup();
                let changed = up.selected.0 != faces;
                let groups = changed.then(|| self.selected_faces(obj, up, &faces));
                if changed {
                    // Their edges and points in the selection's colour too.
                    for (m, lines) in [(&up.lines, true), (&up.dots, false)] {
                        if let Some(m) = m {
                            self.write_marks(obj, m, lines, &faces);
                        }
                    }
                }
                if let Some(up) = self.objects.get_mut(&obj.id()) {
                    up.looked = Some(selection);
                    if let Some(groups) = groups {
                        up.selected = (faces, groups);
                    }
                }
            }
            // The edges and points the first time a class shows them.
            let Some(up) = self.objects.get(&obj.id()) else { continue };
            let lines = (up.lines.is_none() && shows(scene, k, style, Mode::Wireframe))
                .then(|| self.upload_marks(obj, up, true));
            let dots =
                (up.dots.is_none() && shows(scene, k, style, Mode::Points)).then(|| self.upload_marks(obj, up, false));
            if let Some(up) = self.objects.get_mut(&obj.id()) {
                up.lines = up.lines.take().or(lines);
                up.dots = up.dots.take().or(dots);
            }
        }
        self.ensure_target(size);
        let Some(target) = &self.target else { return };
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("umcf view") });
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("umcf view"),
                color_attachments: &[attachment(&target.scene), attachment(&target.ids), attachment(&target.depths)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.view_group, &[]);
            pass.set_pipeline(&self.back);
            pass.draw(0..3, 0..1);
            for (k, obj) in scene.objects.iter().enumerate() {
                let Some(up) = self.objects.get(&obj.id()) else { continue };
                let Some(pipeline) = self.surfaces.get(k.min(RANKS - 1)) else { continue };
                pass.set_pipeline(pipeline);
                for ([group, ..], n) in &up.chunks {
                    pass.set_bind_group(1, group, &[]);
                    pass.draw(0..n.saturating_mul(3), 0..1);
                }
            }
            // The selected classes and faces, in front.
            pass.set_pipeline(&self.selected);
            for (k, obj) in scene.objects.iter().enumerate() {
                let Some(up) = self.objects.get(&obj.id()) else { continue };
                let any =
                    (0..obj.classes.len()).any(|c| style.selected.contains(&(k, u32::try_from(c).unwrap_or(u32::MAX))));
                if any {
                    for ([_, front, _], n) in &up.chunks {
                        pass.set_bind_group(1, front, &[]);
                        pass.draw(0..n.saturating_mul(3), 0..1);
                    }
                }
                for ([group, _], n) in &up.selected.1 {
                    pass.set_bind_group(1, group, &[]);
                    pass.draw(0..n.saturating_mul(3), 0..1);
                }
            }
        }
        {
            // Wireframes, then points, over the surfaces, as in the CPU view: tested against the
            // depths the surfaces wrote, without the bias that puts one object or the selection
            // in front of another.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("umcf marks"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.scene,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.view_group, &[]);
            pass.set_bind_group(2, &target.marks, &[]);
            for (pipeline, mode, vertices) in [(&self.lines, Mode::Wireframe, 2), (&self.dots, Mode::Points, 6)] {
                pass.set_pipeline(pipeline);
                for (k, obj) in scene.objects.iter().enumerate() {
                    let Some(up) = self.objects.get(&obj.id()) else { continue };
                    let marks = if mode == Mode::Wireframe { &up.lines } else { &up.dots };
                    let Some(marks) = marks.as_ref().filter(|_| shows(scene, k, style, mode)) else { continue };
                    for (group, n, _) in marks {
                        pass.set_bind_group(1, group, &[]);
                        pass.draw(0..n.saturating_mul(vertices), 0..1);
                    }
                }
            }
        }
        {
            // The transparent layers, over all that (as in the CPU view, after the wireframes
            // and points), into their own images (always cleared: the outlines' pass reads
            // them), seen where no surface is nearer.
            let clear = |view, value| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(value), store: wgpu::StoreOp::Store },
                })
            };
            let none = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("umcf transparent layers"),
                color_attachments: &[clear(&target.sums, none), clear(&target.through, none)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(glass) = &self.glass {
                pass.set_pipeline(glass);
                pass.set_bind_group(0, &self.view_group, &[]);
                pass.set_bind_group(2, &target.marks, &[]);
                for (k, obj) in scene.objects.iter().enumerate() {
                    let Some(up) = self.objects.get(&obj.id()).filter(|_| has_glass(scene, k, style)) else {
                        continue;
                    };
                    for ([_, _, group], n) in &up.chunks {
                        pass.set_bind_group(1, group, &[]);
                        pass.draw(0..n.saturating_mul(3), 0..1);
                    }
                    for ([_, group], n) in &up.selected.1 {
                        pass.set_bind_group(1, group, &[]);
                        pass.draw(0..n.saturating_mul(3), 0..1);
                    }
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("umcf outlines"),
                color_attachments: &[attachment(&target.view)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.outline);
            pass.set_bind_group(0, &target.outline, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    fn ensure_target(&mut self, size: [u32; 2]) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let make = |label, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let drawn = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = make("umcf view", COLOUR, drawn | wgpu::TextureUsages::COPY_SRC);
        let scene = make("umcf scene", COLOUR, drawn);
        let ids = make("umcf ids", ID, drawn);
        let depths = make("umcf depths", ID, drawn);
        let depth = make("umcf depth", DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
        // Half floats where the device blends them (the transparent layers are drawn), else
        // bytes, only cleared: no layer there.
        let (sums_format, through_format) = if self.glass.is_some() {
            (SUMS, THROUGH)
        } else {
            (wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::R8Unorm)
        };
        let sums = view_of_texture(&make("umcf transparent layers", sums_format, drawn));
        let through = view_of_texture(&make("umcf let through", through_format, drawn));
        let view_of = view_of_texture;
        let (view, scene, ids, depth) = (view_of(&texture), view_of(&scene), view_of(&ids), view_of(&depth));
        let depths = view_of(&depths);
        let marks = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("umcf depths"),
            layout: &self.depths_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depths) }],
        });
        let outline = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("umcf outlines"),
            layout: &self.outline_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&scene) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&ids) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&sums) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&through) },
            ],
        });
        self.target = Some(Target { size, texture, view, scene, ids, depths, marks, sums, through, depth, outline });
    }

    /// `obj` on the GPU: its points, its triangles' corners and classes in chunks a storage
    /// binding holds, and a texture for its classes' colours.
    fn upload(&self, obj: &Object) -> Uploaded {
        // Straight to bytes: no copy of the points in between (millions of them).
        let points: Vec<u8> = obj.points.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        let points = self.storage("umcf points", &points);
        // Two texels per class.
        let texels = u32::try_from(obj.classes.len()).unwrap_or(u32::MAX).max(1).saturating_mul(2);
        let palette = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("umcf colours"),
            size: wgpu::Extent3d {
                width: texels.min(PALETTE_WIDTH),
                height: texels.div_ceil(PALETTE_WIDTH),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let uniform = || {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("umcf object"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniforms = [uniform(), uniform(), uniform(), uniform(), uniform()];
        // Triangles per chunk: their corners (12 bytes each) within a binding.
        let per = usize::try_from((self.chunk.min(self.binding) / 12).max(1)).unwrap_or(usize::MAX);
        let mut chunks = Vec::new();
        let n = obj.triangles.len().min(obj.class.len());
        let mut start = 0;
        while start < n {
            let end = start.saturating_add(per).min(n);
            let triangles = obj.triangles.get(start..end).unwrap_or(&[]);
            let corners: Vec<u8> = triangles.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
            let classes: Vec<u32> = obj
                .class
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(i, c)| with_edges(*c, obj.edges.get(start + i).copied().unwrap_or(0)))
                .collect();
            let [shown, _, selected, glass, _] = &uniforms;
            let corners = self.storage("umcf corners", &corners);
            let classes = self.storage("umcf classes", &bytes_u32(&classes));
            let group = self.object_group(&points, &corners, &classes, &palette, shown);
            let front = self.object_group(&points, &corners, &classes, &palette, selected);
            let glass = self.object_group(&points, &corners, &classes, &palette, glass);
            chunks.push(([group, front, glass], u32::try_from(end - start).unwrap_or(u32::MAX)));
            start = end;
        }
        Uploaded {
            chunks,
            palette,
            points,
            uniforms,
            selected: (Vec::new(), Vec::new()),
            looked: None,
            lines: None,
            dots: None,
        }
    }

    /// The bind groups of the selected `faces` (sorted) of `obj`, in chunks a storage binding
    /// holds, and how many triangles each has: their triangles, drawn in the selection's colour.
    fn selected_faces(&self, obj: &Object, up: &Uploaded, faces: &[u32]) -> Vec<([wgpu::BindGroup; 2], u32)> {
        if faces.is_empty() {
            return Vec::new();
        }
        let picked: Vec<(&[u32; 3], u32)> = obj
            .triangles
            .iter()
            .zip(&obj.origin_index)
            .zip(&obj.class)
            .enumerate()
            .filter(|(_, ((_, f), _))| faces.binary_search(f).is_ok())
            .map(|(n, ((t, _), c))| (t, with_edges(*c, obj.edges.get(n).copied().unwrap_or(0))))
            .collect();
        let per = usize::try_from((self.chunk.min(self.binding) / 12).max(1)).unwrap_or(usize::MAX);
        let [_, faces_uniform, _, _, glass_uniform] = &up.uniforms;
        picked
            .chunks(per)
            .map(|chunk| {
                let corners: Vec<u8> = chunk.iter().flat_map(|(t, _)| **t).flat_map(u32::to_le_bytes).collect();
                // Their own classes: the outlines are those of the patches, as in the CPU view.
                let classes: Vec<u32> = chunk.iter().map(|(_, c)| *c).collect();
                let corners = self.storage("umcf corners", &corners);
                let classes = self.storage("umcf classes", &bytes_u32(&classes));
                let group = self.object_group(&up.points, &corners, &classes, &up.palette, faces_uniform);
                let glass = self.object_group(&up.points, &corners, &classes, &up.palette, glass_uniform);
                ([group, glass], u32::try_from(chunk.len()).unwrap_or(u32::MAX))
            })
            .collect()
    }

    /// `obj`'s edges (`lines`) or points as wireframes and points draw them, the selected faces
    /// `up` was last drawn with marked: in chunks whose ends fit a storage binding and whose
    /// vertices a draw's count.
    ///
    /// Built one chunk at a time: what is held besides the object is one chunk's bytes.
    fn upload_marks(&self, obj: &Object, up: &Uploaded, lines: bool) -> Marks {
        let wire = obj.wire();
        let count = if lines { wire.edges.len() } else { wire.points.len() };
        let (each, vertices): (u64, u64) = if lines { (2, 2) } else { (1, 6) };
        let per =
            usize::try_from((self.chunk.min(self.binding) / (4 * each)).min(u64::from(u32::MAX) / vertices).max(1))
                .unwrap_or(usize::MAX);
        let [shown, ..] = &up.uniforms;
        (0..count)
            .step_by(per)
            .map(|first| {
                let range = first..first.saturating_add(per).min(count);
                let n = range.len();
                let ends: Vec<u8> = if lines {
                    let edges = wire.edges.get(range.clone()).unwrap_or(&[]);
                    edges.iter().flat_map(|e| [e[0], e[1]]).flat_map(u32::to_le_bytes).collect()
                } else {
                    let points = wire.points.get(range.clone()).unwrap_or(&[]);
                    points.iter().flat_map(|p| p[0].to_le_bytes()).collect()
                };
                let ends = self.storage("umcf ends", &ends);
                let classes = self.storage("umcf marks", &marks(obj, lines, &up.selected.0, range));
                let group = self.object_group(&up.points, &ends, &classes, &up.palette, shown);
                (group, u32::try_from(n).unwrap_or(u32::MAX), classes)
            })
            .collect()
    }

    /// Writes `obj`'s edges or points (`lines`) into `m`'s chunks again with the selected
    /// `faces` (sorted) marked, one chunk at a time.
    fn write_marks(&self, obj: &Object, m: &Marks, lines: bool, faces: &[u32]) {
        let mut start = 0usize;
        for (_, n, buffer) in m {
            let end = start.saturating_add(*n as usize);
            let bytes = marks(obj, lines, faces, start..end);
            if !bytes.is_empty() {
                self.queue.write_buffer(buffer, 0, &bytes);
            }
            start = end;
        }
    }

    fn object_group(
        &self,
        points: &wgpu::Buffer,
        corners: &wgpu::Buffer,
        classes: &wgpu::Buffer,
        palette: &wgpu::Texture,
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let view = palette.create_view(&wgpu::TextureViewDescriptor::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("umcf object"),
            layout: &self.object_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: corners.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: classes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 4, resource: uniform.as_entire_binding() },
            ],
        })
    }

    /// A storage buffer holding `bytes` (at least 4: a binding can't be empty).
    fn storage(&self, label: &str, bytes: &[u8]) -> wgpu::Buffer {
        let size = (bytes.len() as u64).max(4).next_multiple_of(4);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !bytes.is_empty() {
            self.queue.write_buffer(&buffer, 0, bytes);
        }
        buffer
    }
}

/// The CPU view's background, top and bottom, where the style gives none.
const BACKGROUND: [[f32; 3]; 2] = [TOP, BOTTOM];

/// `source` with the CPU view's constants it uses (the selection's colour, how much outlines,
/// edges, wireframes and points darken, how near an edge is, how far in front they are seen)
/// written in, so that the two views share them.
fn with_constants(source: &str, depth_front: f32) -> String {
    let [r, g, b] = SELECTED;
    let reach = EDGE_REACH as f32;
    format!(
        "const SELECTED: vec3<f32> = vec3<f32>({r:?}, {g:?}, {b:?});\nconst OUTLINE: f32 = {OUTLINE:?};\n\
         const EDGE: f32 = {EDGE:?};\nconst EDGE_REACH: f32 = {reach:?};\nconst LINE: f32 = {LINE:?};\n\
         const AHEAD: f32 = {AHEAD:?};\nconst CPU_FRONT: f32 = {CPU_FRONT:?};\n\
         const DEPTH_FRONT: f32 = {depth_front:?};\n{source}"
    )
}

fn write_palette(queue: &wgpu::Queue, palette: &wgpu::Texture, colours: &[[f32; 8]]) {
    let size = palette.size();
    // 16 bytes a texel, two texels a class.
    let mut bytes = vec![0u8; (size.width as usize).saturating_mul(size.height as usize).saturating_mul(16)];
    for (px, c) in bytes.as_chunks_mut::<32>().0.iter_mut().zip(colours) {
        for (b, x) in px.as_chunks_mut::<4>().0.iter_mut().zip(c) {
            *b = x.to_le_bytes();
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: palette,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * 16), rows_per_image: None },
        size,
    );
}

fn view_of_texture(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

fn bytes_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn bytes_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use umcf_geom::Vec3;
    use umcf_render::camera::View;
    use umcf_render::raster::{Display, draw};

    /// A new selection number for each frame: the tests change the style between them.
    fn next() -> u64 {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// A device to draw on, when this machine has one the view can use (else the test is skipped;
    /// with `UMCF_GPU_TESTS=1`, as on the Linux CI with its software driver, it fails).
    fn device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
        let found = any_device();
        assert!(
            found.is_some() || std::env::var("UMCF_GPU_TESTS").is_err(),
            "UMCF_GPU_TESTS is set but there is no GPU device the view can use"
        );
        found
    }

    /// One device for every test, made once and kept: Vulkan's loader can crash when instances
    /// are made and dropped on several threads at once (seen with NVIDIA's driver, in
    /// `loader_icd_scan`), as the tests run. Error scopes are per thread, so the tests' frames
    /// don't see each other's; but each `Gpu::new` sets the device's handler of errors no scope
    /// catches, so a test of such an error must make its own device (none does yet; the error
    /// test sets the view's kept error itself).
    fn any_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
        static SHARED: std::sync::OnceLock<Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)>> =
            std::sync::OnceLock::new();
        SHARED.get_or_init(find_device).clone()
    }

    fn find_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            eprintln!("SKIP: no GPU adapter");
            return None;
        };
        if let Err(e) = usable(&adapter) {
            eprintln!("SKIP: {e}");
            return None;
        }
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }),
        )
        .map_err(|e| eprintln!("SKIP: no device on the adapter: {e}"))
        .ok()?;
        Some((adapter, device, queue))
    }

    /// The image of `target`, as RGBA bytes.
    fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, target: &Target) -> Vec<u8> {
        let [w, h] = target.size;
        let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        (0..h as usize).flat_map(|y| data[y * row as usize..][..w as usize * 4].to_vec()).collect()
    }

    /// A box of two patches' faces as an STL surface: two solids.
    fn scene() -> Scene {
        scene_split(false)
    }

    /// [`scene`], its faces split along their other diagonal with `other`.
    fn scene_split(other: bool) -> Scene {
        let face = |n: &str, corners: [[f32; 3]; 4]| {
            let v = |p: [f32; 3]| format!("      vertex {} {} {}\n", p[0], p[1], p[2]);
            let tri = |a, b, c| {
                format!("  facet normal 0 0 0\n    outer loop\n{}{}{}    endloop\n  endfacet\n", v(a), v(b), v(c))
            };
            let [a, b, c, d] = corners;
            let (t1, t2) = if other { (tri(a, b, d), tri(b, c, d)) } else { (tri(a, b, c), tri(a, c, d)) };
            format!("solid {n}\n{t1}{t2}endsolid {n}\n")
        };
        let text = [
            face("top", [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]]),
            face("front", [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]),
            face("side", [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]]),
        ]
        .concat();
        let stl = umcf_geom::stl::read(text.as_bytes(), &umcf_geom::stl::Limits::default()).unwrap();
        let mut scene = Scene::new(Vec3::new(0.5, 0.5, 0.5));
        scene.objects.push(Object::from_stl("box.stl", &stl, scene.origin));
        scene
    }

    /// The share of pixels whose channels differ by more than `tolerance`.
    fn differing(a: &[u8], b: &[u8], tolerance: u8) -> f64 {
        let n = a.len().min(b.len()) / 4;
        let off = (0..n).filter(|i| (0..4).any(|c| a[i * 4 + c].abs_diff(b[i * 4 + c]) > tolerance)).count();
        off as f64 / n.max(1) as f64
    }

    /// Whether a pixel is the selection's orange, shaded (red, about half as much green, little
    /// blue).
    fn orange(p: &[u8]) -> bool {
        let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
        r > 60.0 && (0.45..0.65).contains(&(g / r)) && b < 0.2 * r
    }

    #[test]
    fn the_gpu_view_draws_what_the_cpu_view_draws() {
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        let scene = scene();
        let size = [160u32, 120u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 160.0 / 120.0);
        let mut style = Style { width: size[0], height: size[1], ..Style::default() };
        // A patch selected, one hidden.
        style.selected = vec![(0, 0)];
        style.display.insert((0, 2), Display { mode: Mode::Hidden, opacity: 1.0 });
        let cpu = draw(&scene, &camera, &style).rgba;
        let target = gpu.draw(&scene, &camera, &style, size, next()).unwrap();
        let image = read_back(&device, &queue, target);
        // The same picture, but for pixels on the triangles' edges (rasterised a little
        // otherwise) and the shading across a triangle (per pixel here, per triangle there).
        let off = differing(&cpu, &image, 6);
        assert!(off < 0.01, "{:.1}% of the pixels differ", off * 100.0);
        // The background where there is nothing, and the selection's orange somewhere.
        assert!(image.chunks(4).any(orange), "no selection");
        // Drawn again with the selection gone: no orange, nothing uploaded again.
        style.selected.clear();
        let target = gpu.draw(&scene, &camera, &style, size, next()).unwrap();
        let image = read_back(&device, &queue, target);
        assert!(!image.chunks(4).any(orange), "the selection is still drawn");
        assert_eq!(gpu.objects.len(), 1);
    }

    #[test]
    fn of_surfaces_in_the_same_place_the_first_object_is_seen() {
        // Two objects of the same box, coloured by object: the first is seen, as in the CPU
        // view, not a mix of both (single precision depths that differ a little).
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        // Built twice (a copy would be the same object, with the same id), split otherwise: the
        // same surfaces, with depths that differ a little across each.
        let mut scene = scene();
        let again = scene_split(true);
        scene.objects.extend(again.objects);
        let size = [160u32, 120u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 160.0 / 120.0);
        let style = Style { width: size[0], height: size[1], colour_by: ColourBy::Object, ..Style::default() };
        let cpu = draw(&scene, &camera, &style).rgba;
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        // The share of an image's pixels of object `k`'s colour, shaded.
        let of = |img: &[u8], k: usize| {
            let c = palette(k);
            let top = c.iter().copied().fold(0.0f32, f32::max);
            img.chunks(4)
                .filter(|p| {
                    let m = p[..3].iter().map(|x| f32::from(*x)).fold(0.0f32, f32::max);
                    m > 20.0 && (0..3).all(|i| (f32::from(p[i]) / m - c[i] / top).abs() < 0.08)
                })
                .count()
        };
        assert!(of(&cpu, 0) > 1000 && of(&cpu, 1) == 0);
        // A few pixels may be rasterised otherwise on another device.
        let (seen, under) = (of(&image, 0), of(&image, 1));
        assert!(seen.abs_diff(of(&cpu, 0)) <= of(&cpu, 0) / 100 && under <= of(&cpu, 0) / 200, "{seen} {under}");
        let off = differing(&cpu, &image, 6);
        assert!(off < 0.01, "{:.1}% of the pixels differ", off * 100.0);
    }

    #[test]
    fn a_selected_patch_is_seen_over_another_object_in_the_same_place() {
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        let mut scene = scene();
        scene.objects.extend(scene_split(true).objects);
        let size = [160u32, 120u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 160.0 / 120.0);
        // The second object's top: under the first's, but selected.
        let style = Style { width: size[0], height: size[1], selected: vec![(1, 0)], ..Style::default() };
        let cpu = draw(&scene, &camera, &style).rgba;
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        let selected = image.chunks(4).filter(|p| orange(p)).count();
        assert!(selected > 1000, "{selected} pixels of the selection");
        let on_cpu = cpu.chunks(4).filter(|p| orange(p)).count();
        assert!(selected.abs_diff(on_cpu) <= on_cpu / 100, "{selected} against {on_cpu}");
        let off = differing(&cpu, &image, 6);
        assert!(off < 0.01, "{:.1}% of the pixels differ", off * 100.0);
    }

    #[test]
    fn selected_faces_of_a_hidden_patch_stay_hidden() {
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        let scene = scene();
        let size = [160u32, 120u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 160.0 / 120.0);
        // Both triangles of the top (class 0), selected as faces, and the top hidden.
        let mut style = Style {
            width: size[0],
            height: size[1],
            selected_faces: [(0, 0), (0, 1)].into_iter().collect(),
            ..Style::default()
        };
        let shown = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        assert!(shown.chunks(4).any(orange), "the selected faces are drawn while the top is shown");
        style.display.insert((0, 0), Display { mode: Mode::Hidden, opacity: 1.0 });
        let cpu = draw(&scene, &camera, &style).rgba;
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        assert!(!image.chunks(4).any(orange) && !cpu.chunks(4).any(orange));
        assert!(differing(&cpu, &image, 6) < 0.01);
    }

    #[test]
    fn an_error_the_device_reported_fails_the_frame_whatever_the_style() {
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        // As on a device that can't blend half floats: transparent layers go to the CPU.
        gpu.glass = None;
        let scene = scene();
        let size = [64u32, 48u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 64.0 / 48.0);
        let plain = Style { width: size[0], height: size[1], ..Style::default() };
        // Transparent layers: a style the GPU view hands to the CPU.
        let mut glass = plain.clone();
        glass.display.insert((0, 0), Display { mode: Mode::Surface, opacity: 0.5 });
        for style in [&plain, &glass] {
            *gpu.uncaptured.lock().unwrap() = Some("the GPU device reported: lost".to_string());
            assert!(gpu.failed());
            let got = gpu.draw(&scene, &camera, style, size, next()).err();
            assert_eq!(got, Some(NotDrawn::Failed("the GPU device reported: lost".to_string())), "{style:?}");
            assert!(!gpu.failed(), "the error is taken");
        }
        // Without one, the style decides again.
        assert!(matches!(gpu.draw(&scene, &camera, &glass, size, next()).err(), Some(NotDrawn::Here(_))));
        assert!(gpu.draw(&scene, &camera, &plain, size, next()).is_ok());
    }

    #[test]
    fn the_renderer_is_named_on_the_command_line() {
        use crate::Start;
        assert_eq!(Start::parse("cpu"), Some(Start::Cpu));
        assert_eq!(Start::parse("gpu"), Some(Start::Gpu));
        assert_eq!(Start::parse("auto"), Some(Start::Auto));
        assert_eq!(Start::parse("GPU"), None);
        assert_eq!(Start::default(), Start::Cpu);
    }

    #[test]
    fn styles_the_gpu_view_doesnt_draw_are_named() {
        let Some((adapter, device, queue)) = device() else { return };
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        if gpu.glass.is_none() {
            eprintln!("SKIP: the device can't blend half floats");
            return;
        }
        let mut style = Style { edges: true, ..Style::default() };
        for mode in [Mode::Wireframe, Mode::Points, Mode::Hidden, Mode::Surface] {
            style.display.insert((0, 0), Display { mode, opacity: 0.5 });
            assert_eq!(gpu.draws(&style), Ok(()), "{mode:?}");
        }
        // Without half floats to blend, transparent layers are the CPU's.
        gpu.glass = None;
        assert_eq!(gpu.draws(&style), Err("transparent layers"));
    }

    /// A box of two patches as a mesh: three faces of a unit cube in two patches, each face
    /// split in two triangles (a diagonal that is no edge), so that edges and wireframes show
    /// the faces, not the triangles.
    fn mesh_scene() -> Scene {
        let mut scene = Scene::new(Vec3::new(0.5, 0.5, 0.5));
        scene.objects.push(mesh_box(&scene, 1.0));
        scene
    }

    /// [`mesh_scene`]'s box, `scale` times as big about its middle, as an object of `scene`.
    fn mesh_box(scene: &Scene, scale: f32) -> Object {
        let text = scene_text(scale);
        let stl = umcf_geom::stl::read(text.as_bytes(), &umcf_geom::stl::Limits::default()).unwrap();
        let mut obj = Object::from_stl("box.stl", &stl, scene.origin);
        // Each face's diagonal (edge 2 to 0 of its first triangle, 0 to 1 of its second, as
        // `scene_split(false)` splits them) is no face edge, as in a mesh.
        for (n, e) in obj.edges.iter_mut().enumerate() {
            *e = if n % 2 == 0 { 0b011 } else { 0b110 };
        }
        obj
    }

    fn scene_text(scale: f32) -> String {
        let face = |n: &str, corners: [[f32; 3]; 4]| {
            let v = |p: [f32; 3]| {
                let p = p.map(|x| 0.5 + (x - 0.5) * scale);
                format!("      vertex {} {} {}\n", p[0], p[1], p[2])
            };
            let tri = |a, b, c| {
                format!("  facet normal 0 0 0\n    outer loop\n{}{}{}    endloop\n  endfacet\n", v(a), v(b), v(c))
            };
            let [a, b, c, d] = corners;
            format!("solid {n}\n{}{}endsolid {n}\n", tri(a, b, c), tri(a, c, d))
        };
        [
            face("top", [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]]),
            face("front", [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]),
            face("side", [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]]),
        ]
        .concat()
    }

    /// The pixels of `img` where `a` and `b` differ by more than `tolerance`: how many.
    fn changed(a: &[u8], b: &[u8], tolerance: u8) -> usize {
        a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > tolerance)).count()
    }

    /// The CPU's and the GPU's image of `scene` with `style`, at 240 × 180.
    fn both(scene: &Scene, style: &mut Style) -> Option<(Vec<u8>, Vec<u8>)> {
        let (adapter, device, queue) = device()?;
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        let size = [240u32, 180u32];
        (style.width, style.height) = (size[0], size[1]);
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 240.0 / 180.0);
        let cpu = draw(scene, &camera, style).rgba;
        let image = read_back(&device, &queue, gpu.draw(scene, &camera, style, size, next()).unwrap());
        Some((cpu, image))
    }

    /// What `style` adds to the plain picture, on the CPU and on the GPU: how many pixels each
    /// changes, and how many of those the two share.
    fn added(scene: &Scene, style: &Style) -> Option<(usize, usize, usize)> {
        let mut plain = Style { edges: false, display: Default::default(), ..style.clone() };
        // What is drawn otherwise than a surface is hidden in the plain one: what it adds is
        // compared, not what it takes away.
        for (k, d) in &style.display {
            if d.mode != Mode::Surface {
                plain.display.insert(*k, Display { mode: Mode::Hidden, opacity: 1.0 });
            }
        }
        let mut styled = style.clone();
        let (cpu_plain, gpu_plain) = both(scene, &mut plain)?;
        let (cpu, image) = both(scene, &mut styled)?;
        let on_cpu: Vec<bool> = cpu.chunks(4).zip(cpu_plain.chunks(4)).map(|(p, q)| p != q).collect();
        let on_gpu: Vec<bool> =
            image.chunks(4).zip(gpu_plain.chunks(4)).map(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 6)).collect();
        let shared = on_cpu.iter().zip(&on_gpu).filter(|(a, b)| **a && **b).count();
        Some((on_cpu.iter().filter(|x| **x).count(), on_gpu.iter().filter(|x| **x).count(), shared))
    }

    #[test]
    fn the_faces_edges_are_drawn_as_the_cpu_view_draws_them() {
        let scene = mesh_scene();
        let mut style = Style { edges: true, ..Style::default() };
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        // Pixels on an edge's border may be in or out on either side.
        let off = differing(&cpu, &image, 6);
        assert!(off < 0.01, "{:.1}% of the pixels differ", off * 100.0);
        let (on_cpu, on_gpu, shared) = added(&scene, &style).unwrap();
        assert!(on_cpu > 300, "{on_cpu} edge pixels");
        assert!(on_gpu.abs_diff(on_cpu) <= on_cpu / 10 && shared >= on_cpu * 9 / 10, "{on_cpu} {on_gpu} {shared}");
        // No diagonal: the faces' edges only (a diagonal darkens pixels across each face's middle).
        let mut all = scene.clone();
        all.objects[0].edges.fill(0b111);
        let (more, more_gpu, _) = added(&all, &style).unwrap();
        assert!(more > on_cpu + 50, "the diagonals add {} pixels", more.saturating_sub(on_cpu));
        assert!(more_gpu > on_gpu + 50, "the diagonals add {} pixels on the GPU", more_gpu.saturating_sub(on_gpu));
    }

    #[test]
    fn wireframes_are_seen_as_on_the_cpu_where_other_objects_surfaces_lie() {
        // The CPU tests a line against the surfaces' depths as they are; the GPU puts later
        // objects behind and the selection in front of what lies in the same place (a bias),
        // which must not hide nor show lines the CPU doesn't.
        let wire = Display { mode: Mode::Wireframe, opacity: 1.0 };
        // 1. The second box's wireframe on the first's surfaces, selected: in the same place.
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 1.0));
        let mut style = Style { selected: vec![(0, 0), (0, 1), (0, 2)], ..Style::default() };
        for c in 0..3 {
            style.display.insert((1, c), wire);
        }
        let Some((on_cpu, on_gpu, shared)) = added(&scene, &style) else { return };
        assert!(on_cpu > 200, "{on_cpu} line pixels");
        // Lines exactly on a surface are rasterised a little otherwise (which pixels a line
        // takes, and of those which are in front, within AHEAD): about as many, two thirds the
        // same.
        assert!(
            on_gpu.abs_diff(on_cpu) <= on_cpu / 5 && shared >= on_cpu * 6 / 10,
            "on a selection: {on_cpu} on the CPU, {on_gpu} on the GPU, {shared} both"
        );
        // And the selection doesn't change which are seen.
        let (_, unselected, _) = added(&scene, &Style { selected: Vec::new(), ..style.clone() }).unwrap();
        assert!(on_gpu.abs_diff(unselected) <= on_gpu / 50, "{on_gpu} selected, {unselected} not");
        // 2. The first box's wireframe just inside a second box a little bigger: hidden by it
        // (its faces in front), but where the two boxes' outlines meet.
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 1.003));
        let mut style = Style::default();
        for c in 0..3 {
            style.display.insert((0, c), wire);
        }
        let (on_cpu, on_gpu, _) = added(&scene, &style).unwrap();
        let (seen, _, _) = added(&mesh_scene(), &style).unwrap();
        assert!(seen > 300 && on_cpu < seen / 2, "{on_cpu} of {seen} line pixels seen through on the CPU");
        // Where the outlines meet (the bigger box is a fifth of a pixel bigger), one-pixel lines
        // take neighbouring pixels on either side; the rest is hidden on both.
        assert!(
            on_gpu < seen / 2 && on_gpu <= on_cpu * 3 / 2,
            "behind: {on_cpu} on the CPU, {on_gpu} on the GPU, of {seen}"
        );
    }

    #[test]
    fn wireframes_and_points_are_drawn_as_the_cpu_view_draws_them() {
        let scene = mesh_scene();
        for mode in [Mode::Wireframe, Mode::Points] {
            // The top in `mode`, over the other two as surfaces; then all three.
            for classes in [vec![0u32], vec![0, 1, 2]] {
                let mut style = Style::default();
                for c in &classes {
                    style.display.insert((0, *c), Display { mode, opacity: 1.0 });
                }
                let Some((on_cpu, on_gpu, shared)) = added(&scene, &style) else { return };
                // The top's four corners as points: 2 × 2 pixels each, one hidden behind the box.
                let least = if mode == Mode::Wireframe { 40 } else { 10 };
                assert!(on_cpu > least, "{mode:?} {classes:?}: {on_cpu} pixels");
                // One-pixel lines are rasterised a little otherwise (where a line runs between
                // two pixels); points are the same squares.
                let (near, share) = if mode == Mode::Wireframe { (4, 7) } else { (10, 9) };
                assert!(
                    on_gpu.abs_diff(on_cpu) <= on_cpu / near && shared >= on_cpu * share / 10,
                    "{mode:?} {classes:?}: {on_cpu} on the CPU, {on_gpu} on the GPU, {shared} both"
                );
            }
        }
    }

    #[test]
    fn a_selected_face_of_a_wireframe_is_drawn_in_the_selection_colour() {
        let scene = mesh_scene();
        let mut style = Style::default();
        style.display.insert((0, 0), Display { mode: Mode::Wireframe, opacity: 1.0 });
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        assert!(!image.chunks(4).any(orange) && !cpu.chunks(4).any(orange));
        // The top's first face (its two triangles), then none again: the marks are written
        // again when the selection changes.
        let (adapter, device, queue) = device().unwrap();
        let mut gpu = Gpu::new(&adapter, &device, &queue).unwrap();
        let size = [240u32, 180u32];
        (style.width, style.height) = (size[0], size[1]);
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 240.0 / 180.0);
        let plain = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        style.selected_faces = [(0, 0), (0, 1)].into_iter().collect();
        let cpu = draw(&scene, &camera, &style).rgba;
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        let (a, b) = (cpu.chunks(4).filter(|p| orange(p)).count(), image.chunks(4).filter(|p| orange(p)).count());
        assert!(a > 40 && b.abs_diff(a) <= a / 4, "{a} orange pixels on the CPU, {b} on the GPU");
        style.selected_faces.clear();
        let again = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        assert_eq!(changed(&plain, &again, 0), 0, "the selection is still drawn");
        // A selected patch: its edges in the selection's colour.
        style.selected = vec![(0, 0)];
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size, next()).unwrap());
        assert!(image.chunks(4).filter(|p| orange(p)).count() > a, "a selected patch's wireframe");
    }

    #[test]
    fn triangles_edges_and_points_split_in_small_chunks_draw_the_same() {
        // A binding of 16 bytes: one triangle, two edges or four points a chunk.
        let Some((adapter, device, queue)) = device() else { return };
        let scene = mesh_scene();
        let size = [240u32, 180u32];
        let camera = Camera::fit(View::Iso, &scene.bbox().unwrap(), 240.0 / 180.0);
        let mut style = Style { width: size[0], height: size[1], edges: true, ..Style::default() };
        style.display.insert((0, 0), Display { mode: Mode::Wireframe, opacity: 1.0 });
        style.display.insert((0, 1), Display { mode: Mode::Points, opacity: 1.0 });
        let mut whole = Gpu::new(&adapter, &device, &queue).unwrap();
        let mut small = Gpu::new(&adapter, &device, &queue).unwrap();
        small.chunk = 16;
        // Faces of the wireframe, of the points and of the side's surface (its triangles, in
        // chunks too, drawn in the selection's colour).
        for faces in [vec![], vec![(0, 0), (0, 2), (0, 4), (0, 5)]] {
            style.selected_faces = faces.into_iter().collect();
            let selection = next();
            let a = read_back(&device, &queue, whole.draw(&scene, &camera, &style, size, selection).unwrap());
            let b = read_back(&device, &queue, small.draw(&scene, &camera, &style, size, selection).unwrap());
            assert_eq!(changed(&a, &b, 0), 0, "{:?}", style.selected_faces);
        }
        let up = small.objects.values().next().unwrap();
        assert!(up.chunks.len() > 1 && up.lines.as_ref().unwrap().len() > 1 && up.dots.as_ref().unwrap().len() > 1);
        assert!(up.selected.1.len() > 1, "the side's selected faces in one chunk");
    }

    /// The share of pixels of the CPU's and the GPU's image of `scene` with `style` that differ
    /// by more than `tolerance`.
    fn off(scene: &Scene, style: &Style, tolerance: u8) -> Option<f64> {
        let (cpu, image) = both(scene, &mut style.clone())?;
        Some(differing(&cpu, &image, tolerance))
    }

    #[test]
    fn a_transparent_layer_is_blended_as_on_the_cpu() {
        let scene = mesh_scene();
        let glass = |o: f32| Display { mode: Mode::Surface, opacity: o };
        // The top patch at half opacity over the background; then every face (three layers that
        // don't lie over each other here), with edges, at 0.3.
        let mut style = Style::default();
        style.display.insert((0, 0), glass(0.5));
        let Some(top) = off(&scene, &style, 3) else { return };
        assert!(top < 0.01, "top: {:.2}% differ", top * 100.0);
        let mut all = Style { edges: true, ..Style::default() };
        for c in 0..3 {
            all.display.insert((0, c), glass(0.3));
        }
        let every = off(&scene, &all, 3).unwrap();
        assert!(every < 0.01, "all: {:.2}% differ", every * 100.0);
        // Over the background it changes the picture: not drawn as opaque, nor left out.
        let (_, opaque) = both(&scene, &mut Style::default()).unwrap();
        let (_, image) = both(&scene, &mut style.clone()).unwrap();
        assert!(changed(&opaque, &image, 6) > 1000);
    }

    #[test]
    fn an_opaque_box_is_seen_through_a_transparent_one_as_on_the_cpu() {
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 0.5));
        let mut style = Style::default();
        for c in 0..3 {
            style.display.insert((0, c), Display { mode: Mode::Surface, opacity: 0.4 });
        }
        let Some(through) = off(&scene, &style, 3) else { return };
        assert!(through < 0.01, "{:.2}% differ", through * 100.0);
        // A selected face of the transparent box: in the selection's colour, blended once.
        let (cpu_plain, gpu_plain) = both(&scene, &mut style.clone()).unwrap();
        style.selected_faces = [(0, 0), (0, 1)].into_iter().collect();
        let (cpu, image) = both(&scene, &mut style.clone()).unwrap();
        let (a, b) = (changed(&cpu_plain, &cpu, 6), changed(&gpu_plain, &image, 6));
        assert!(a > 500 && b.abs_diff(a) <= a / 50, "the selection changes {a} pixels on the CPU, {b} on the GPU");
        assert!(differing(&cpu, &image, 3) < 0.01);
        // A selected transparent patch too.
        style.selected_faces.clear();
        style.selected = vec![(0, 2)];
        assert!(off(&scene, &style, 3).unwrap() < 0.01);
    }

    #[test]
    fn transparent_layers_over_each_other_mix_the_nearer_more() {
        // Two transparent boxes, one inside the other, of their own colours: the CPU blends the
        // farther first; the GPU weighs them (order-independent), the nearer more. Where they
        // lie over each other, the GPU's colour is nearer the CPU's (the outer box's faces in
        // front) than an even mix of the two would be.
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 0.5));
        let a = 0.5f32;
        let glass = Display { mode: Mode::Surface, opacity: a };
        let hidden = Display { mode: Mode::Hidden, opacity: 1.0 };
        let with = |outer: Display, inner: Display| {
            let mut style = Style { colour_by: ColourBy::Object, ..Style::default() };
            for c in 0..3 {
                style.display.insert((0, c), outer);
                style.display.insert((1, c), inner);
            }
            style
        };
        let Some((cpu, both_gpu)) = both(&scene, &mut with(glass, glass)) else { return };
        let (_, outer) = both(&scene, &mut with(glass, hidden)).unwrap();
        let (_, inner) = both(&scene, &mut with(hidden, glass)).unwrap();
        let (_, none) = both(&scene, &mut with(hidden, hidden)).unwrap();
        let (mut to_cpu, mut even_to_cpu, mut n) = (0.0f64, 0.0f64, 0usize);
        for i in 0..cpu.len() / 4 {
            let px = |v: &[u8], k: usize| f32::from(v[i * 4 + k]) / 255.0;
            if (0..3).all(|k| px(&outer, k) == px(&none, k)) || (0..3).all(|k| px(&inner, k) == px(&none, k)) {
                continue;
            }
            n += 1;
            for k in 0..3 {
                let under = px(&none, k);
                // Each box's own colour, from its layer alone over what is behind it.
                let (o, c) = ((px(&outer, k) - under * (1.0 - a)) / a, (px(&inner, k) - under * (1.0 - a)) / a);
                let even = (o + c) / 2.0 * (1.0 - (1.0 - a) * (1.0 - a)) + under * (1.0 - a) * (1.0 - a);
                to_cpu += f64::from((px(&both_gpu, k) - px(&cpu, k)).abs());
                even_to_cpu += f64::from((even - px(&cpu, k)).abs());
            }
        }
        assert!(n > 500, "{n} pixels where both boxes are");
        assert!(to_cpu < even_to_cpu * 0.6, "{to_cpu:.1} from the CPU's colours, an even mix {even_to_cpu:.1}");
        // Elsewhere the same.
        assert!(differing(&cpu, &both_gpu, 40) < 0.005);
    }

    #[test]
    fn a_transparent_layer_on_a_selected_patch_is_behind_it_as_on_the_cpu() {
        // The same box twice: the first opaque, its top selected; the second transparent, in
        // the same place, split along the other diagonal (depths that differ a little across
        // each face). The selection is a little nearer (as the CPU stores it), so the
        // transparent top doesn't tint it.
        let mut scene = mesh_scene();
        scene.objects.extend(scene_split(true).objects);
        let mut style = Style { selected: vec![(0, 0)], ..Style::default() };
        let mut alone = style.clone();
        for c in 0..3 {
            style.display.insert((1, c), Display { mode: Mode::Surface, opacity: 0.5 });
            alone.display.insert((1, c), Display { mode: Mode::Hidden, opacity: 1.0 });
        }
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        let (cpu_alone, gpu_alone) = both(&scene, &mut alone).unwrap();
        let orange_in = |v: &[u8]| v.chunks(4).filter(|p| orange(p)).count();
        let (on_cpu, on_gpu) = (orange_in(&cpu), orange_in(&image));
        assert!(on_cpu > 1000 && on_cpu.abs_diff(orange_in(&cpu_alone)) <= on_cpu / 100);
        assert!(
            on_gpu.abs_diff(orange_in(&gpu_alone)) <= on_gpu / 100,
            "{on_gpu} orange with, {} without",
            orange_in(&gpu_alone)
        );
    }

    #[test]
    fn a_selected_patch_behind_another_surface_stays_behind_it() {
        // A box a hundredth smaller inside another, opaque, its faces selected: hidden, as on
        // the CPU (the selection is put in front of what lies in the same place only; a bias
        // growing with the slope brought it through near the faces' edges).
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 0.99));
        let mut style = Style { selected: vec![(1, 0), (1, 1), (1, 2)], ..Style::default() };
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        let (cpu_plain, gpu_plain) = both(&scene, &mut Style::default()).unwrap();
        assert_eq!(cpu.chunks(4).filter(|p| orange(p)).count(), 0);
        assert_eq!(changed(&cpu, &cpu_plain, 0), 0);
        let through = changed(&image, &gpu_plain, 6);
        assert_eq!(through, 0, "{through} pixels of the selection seen through the box in front");
    }

    #[test]
    fn a_transparent_layer_behind_an_opaque_surface_is_not_seen() {
        // The small box transparent, inside the big one, opaque: hidden on both views.
        let mut scene = mesh_scene();
        scene.objects.push(mesh_box(&scene, 0.5));
        let mut style = Style::default();
        let mut hidden = Style::default();
        for c in 0..3 {
            style.display.insert((1, c), Display { mode: Mode::Surface, opacity: 0.5 });
            hidden.display.insert((1, c), Display { mode: Mode::Hidden, opacity: 1.0 });
        }
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        let (cpu_hidden, gpu_hidden) = both(&scene, &mut hidden).unwrap();
        assert_eq!(changed(&cpu, &cpu_hidden, 0), 0);
        assert_eq!(changed(&image, &gpu_hidden, 0), 0, "a transparent layer seen through an opaque surface");
    }

    #[test]
    fn a_wireframe_half_opaque_is_blended_as_on_the_cpu() {
        let scene = mesh_scene();
        let mut style = Style::default();
        // The top as a wireframe at half its opacity over the background and the other faces.
        style.display.insert((0, 0), Display { mode: Mode::Wireframe, opacity: 0.5 });
        let Some((cpu, image)) = both(&scene, &mut style) else { return };
        // Where both drew a line, the same colour (the blend rounds alike).
        let (mut same, mut lines) = (0, 0);
        let mut hidden = style.clone();
        hidden.display.insert((0, 0), Display { mode: Mode::Hidden, opacity: 1.0 });
        let (cpu_under, gpu_under) = both(&scene, &mut hidden).unwrap();
        for i in 0..cpu.len() / 4 {
            let p = |v: &[u8]| [v[i * 4], v[i * 4 + 1], v[i * 4 + 2]];
            let (c, g) = (p(&cpu), p(&image));
            if c != p(&cpu_under) && g != p(&gpu_under) {
                lines += 1;
                if (0..3).all(|k| c[k].abs_diff(g[k]) <= 2) {
                    same += 1;
                }
            }
        }
        assert!(lines > 40 && same >= lines * 9 / 10, "{same} of {lines}");
        // Half opaque: neither the colour under it nor the line's own.
        let mut full = style.clone();
        full.display.insert((0, 0), Display { mode: Mode::Wireframe, opacity: 1.0 });
        let (_, opaque) = both(&scene, &mut full).unwrap();
        assert!(changed(&opaque, &image, 6) > 40, "a half-opaque wireframe is drawn as an opaque one");
    }
}
