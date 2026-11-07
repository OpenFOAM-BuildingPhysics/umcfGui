//! The GPU view: the scene drawn with wgpu into a texture the window shows, as the CPU
//! rasteriser's image is shown. It draws what the CPU view draws for opaque surfaces: each class
//! in its colour (the selection orange, in front of what lies in the same place), flat-shaded
//! with the light at the eye (per triangle, as the CPU view), outlines where one class meets
//! another or the background, over the background's gradient, with an infinite far plane.
//! Edges, wireframes, points and transparent layers are drawn by the CPU view for now
//! ([`Gpu::draws`] says which styles this one draws).
//!
//! Each object is uploaded once, as it is (its points, its triangles' corners and classes,
//! split into chunks a storage binding holds), and kept while the scene shows it; what changes
//! from frame to frame is small: the camera, and a colour per class (the selection, what is
//! hidden). The vertex shader reads the corners from storage buffers (vertex pulling), so a
//! point shared by triangles of several classes is stored once, and each vertex sees its whole
//! triangle, to shade it as one. Picking stays on the CPU (the scene's BVH), as do snapshots on
//! the command line.

use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;
use umcf_render::camera::Camera;
use umcf_render::raster::{ColourBy, Mode, SELECTED, Style, palette};
use umcf_render::{Object, Scene};

/// Depth-bias steps of the objects after the first (the last one for the rest), and how much
/// each is pushed back: in units of the depth's last bit, and per unit of the surface's depth
/// slope across a pixel.
const RANKS: usize = 8;
const BIAS: i32 = 8;
const SLOPE: f32 = 2.0;
/// How far the selection is pulled forward: past every object's push.
const FRONT: i32 = 128;
const FRONT_SLOPE: f32 = 32.0;

/// The colours' texture's width: classes past it go on the next rows (the shader's 1024).
const PALETTE_WIDTH: u32 = 1024;
/// The depth buffer's format: reversed (1 near, 0 at infinity), so that far things keep their
/// precision.
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The image's format: as egui's own textures (`Rgba8Unorm` holding sRGB values, which egui's
/// shader reads as such), so the bytes are the CPU view's.
pub const COLOUR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
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
};
struct Obj {
    // 0: the classes not selected; 1: the selected faces, in the selection's colour; 2: the
    // selected classes (drawn after the others, in front of what lies in the same place).
    mode: u32,
    // The number of the object's first class in the scene, for the ids.
    base: u32,
    pad0: u32,
    pad1: u32,
};
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<storage, read> points: array<f32>;
@group(1) @binding(1) var<storage, read> corners: array<u32>;
@group(1) @binding(2) var<storage, read> classes: array<u32>;
@group(1) @binding(3) var colours: texture_2d<f32>;
@group(1) @binding(4) var<uniform> obj: Obj;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
    @location(1) @interpolate(flat) id: u32,
};

struct Pixel {
    @location(0) colour: vec4<f32>,
    @location(1) id: u32,
};

// Corner `j` of the chunk, in view coordinates.
fn corner(j: u32) -> vec3<f32> {
    let k = corners[j];
    let p = vec3<f32>(points[3u * k], points[3u * k + 1u], points[3u * k + 2u]);
    let d = p - view.eye.xyz;
    return vec3<f32>(dot(d, view.right.xyz), dot(d, view.up.xyz), dot(d, view.forward.xyz));
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let t = i / 3u;
    let v = corner(i);
    let which = classes[t];
    var c = textureLoad(colours, vec2<i32>(i32(which % 1024u), i32(which / 1024u)), 0);
    if obj.mode == 1u && c.a >= 0.25 {
        // A selected face of a class shown: the selection's colour, in front.
        c = vec4<f32>(1.0, 0.55, 0.1, 1.0);
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
    // Alpha: 0 hidden, 1 selected, between: shown. Each pass draws its own (the others are
    // put outside the view, never drawn): mode 0 the shown, mode 2 the selected; mode 1 the
    // selected faces of the classes not hidden.
    let selected = c.a > 0.75;
    var skip = c.a < 0.25;
    if obj.mode == 0u && selected {
        skip = true;
    }
    if obj.mode == 2u && !selected {
        skip = true;
    }
    // Reversed depth: near / distance.
    out.pos = vec4<f32>(v.x * view.lens.x, v.y * view.lens.y, view.lens.z, v.z);
    if skip {
        out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        c.a = 0.0;
    }
    out.colour = vec4<f32>(c.rgb * shade, c.a);
    out.id = obj.base + which + 1u;
    return out;
}

@fragment
fn fs(in: Out) -> Pixel {
    if in.colour.a < 0.25 {
        discard;
    }
    var out: Pixel;
    out.colour = vec4<f32>(in.colour.rgb, 1.0);
    out.id = in.id;
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
    return out;
}
"#;

/// Darkens the pixels where one class meets another or the background, as the CPU view.
const OUTLINE: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var ids: texture_2d<u32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((i << 1u) & 2u) * 2.0 - 1.0, f32(i & 2u) * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(ids));
    let p = vec2<i32>(pos.xy);
    let id = textureLoad(ids, p, 0).r;
    let c = textureLoad(image, p, 0);
    var outline = false;
    if id != 0u {
        if p.x + 1 < size.x && textureLoad(ids, p + vec2<i32>(1, 0), 0).r != id { outline = true; }
        if p.x > 0 && textureLoad(ids, p - vec2<i32>(1, 0), 0).r != id { outline = true; }
        if p.y + 1 < size.y && textureLoad(ids, p + vec2<i32>(0, 1), 0).r != id { outline = true; }
        if p.y > 0 && textureLoad(ids, p - vec2<i32>(0, 1), 0).r != id { outline = true; }
    }
    if outline {
        return vec4<f32>(c.rgb * 0.55, 1.0);
    }
    return c;
}
"#;

/// Why the GPU view didn't draw a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotDrawn {
    /// The CPU draws this one (a style it doesn't draw yet, an object too big for the
    /// device); the next may be the GPU's again.
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
fn view_uniform(scene: &Scene, camera: &Camera, size: [u32; 2], background: [[f32; 3]; 2]) -> [f32; 28] {
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
    ];
    let mut out = [0.0; 28];
    for (o, x) in out.iter_mut().zip(rows.iter().flatten()) {
        *o = *x;
    }
    out
}

/// Each class's colour in the GPU view, as RGBA bytes: its colour (or the selection's), alpha 0
/// for hidden, 255 for selected (drawn in front), 128 else.
fn class_colours(scene: &Scene, k: usize, style: &Style) -> Vec<[u8; 4]> {
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
            let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            let alpha = match style.display_of(k, c32).mode {
                Mode::Hidden => 0,
                _ if selected => 255,
                _ => 128,
            };
            [byte(colour[0]), byte(colour[1]), byte(colour[2]), alpha]
        })
        .collect()
}

/// What an object of the scene is on the GPU.
struct Uploaded {
    /// What draws it: for chunks of its triangles, a bind group for the classes shown and one
    /// for the selected classes (sharing the chunk's buffers), and how many triangles each has.
    chunks: Vec<(wgpu::BindGroup, wgpu::BindGroup, u32)>,
    palette: wgpu::Texture,
    points: wgpu::Buffer,
    /// Its uniforms, by mode: the classes shown, the selected faces, the selected classes.
    uniforms: [wgpu::Buffer; 3],
    /// The selected faces it was last drawn with, and their triangles in chunks (none: no
    /// triangle has them), drawn over it in the selection's colour.
    selected: (Vec<u32>, Vec<(wgpu::BindGroup, u32)>),
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
    back: wgpu::RenderPipeline,
    outline: wgpu::RenderPipeline,
    object_layout: wgpu::BindGroupLayout,
    outline_layout: wgpu::BindGroupLayout,
    view_buffer: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    objects: HashMap<u64, Uploaded>,
    /// The most a storage binding holds, bytes.
    binding: u64,
    target: Option<Target>,
    /// An error the device reported outside the scopes this view sets (wgpu's default panics).
    uncaptured: std::sync::Arc<std::sync::Mutex<Option<String>>>,
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
        let kept = uncaptured.clone();
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            eprintln!("umcf-gui: the GPU device reported: {e}");
            let mut slot = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.get_or_insert_with(|| format!("the GPU device reported: {e}"));
        }));
        let kept = uncaptured.clone();
        device.set_device_lost_callback(move |_, message| {
            eprintln!("umcf-gui: the GPU device was lost: {message}");
            let mut slot = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.get_or_insert_with(|| format!("the GPU device was lost: {message}"));
        });
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("umcf view"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let outline_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("umcf outlines"),
            source: wgpu::ShaderSource::Wgsl(OUTLINE.into()),
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
                uniform(4, wgpu::ShaderStages::VERTEX),
            ],
        });
        let outline_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("umcf outlines"),
            entries: &[
                texture(0, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Float { filterable: false }),
                texture(1, wgpu::ShaderStages::FRAGMENT, wgpu::TextureSampleType::Uint),
            ],
        });
        let layout_of = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let layout = layout_of("umcf view", &[Some(&view_layout), Some(&object_layout)]);
        let back_layout = layout_of("umcf background", &[Some(&view_layout)]);
        let outline_pipeline_layout = layout_of("umcf outlines", &[Some(&outline_layout)]);
        let targets = [
            Some(wgpu::ColorTargetState { format: COLOUR, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: ID, blend: None, write_mask: wgpu::ColorWrites::ALL }),
        ];
        let pipeline = |label, layout: &wgpu::PipelineLayout, vs, fs, compare, bias| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
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
                    targets: &targets,
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
                pipeline("umcf surfaces", &layout, "vs", "fs", wgpu::CompareFunction::Greater, bias)
            })
            .collect();
        // The selection in front of what lies in the same place, whichever object it is in;
        // the selected faces over their own triangles too.
        let front = wgpu::DepthBiasState { constant: FRONT, slope_scale: FRONT_SLOPE, clamp: 0.0 };
        let selected = pipeline("umcf selection", &layout, "vs", "fs", wgpu::CompareFunction::GreaterEqual, front);
        let back = pipeline(
            "umcf background",
            &back_layout,
            "vs_back",
            "fs_back",
            wgpu::CompareFunction::Always,
            wgpu::DepthBiasState::default(),
        );
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
            size: 28 * 4,
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
            back,
            outline,
            object_layout,
            outline_layout,
            view_buffer,
            view_group,
            objects: HashMap::new(),
            binding,
            target: None,
            uncaptured,
        })
    }

    /// Whether this view draws `style` as the CPU view would: opaque surfaces, hidden layers,
    /// the selection; `Err` names what it doesn't draw yet.
    pub fn draws(style: &Style) -> Result<(), &'static str> {
        if style.edges {
            return Err("edges");
        }
        for d in style.display.values() {
            match d.mode {
                Mode::Hidden => {}
                Mode::Surface if d.opacity >= 1.0 => {}
                Mode::Surface => return Err("transparent layers"),
                Mode::Wireframe => return Err("wireframes"),
                Mode::Points => return Err("points"),
            }
        }
        Ok(())
    }

    /// Whether `obj` fits the device: its points in one storage binding (its triangles are
    /// split), its classes' colours in one texture.
    fn fits(&self, obj: &Object) -> bool {
        let rows = (obj.classes.len() as u64).div_ceil(u64::from(PALETTE_WIDTH));
        (obj.points.len() as u64).saturating_mul(12) <= self.binding
            && rows <= u64::from(self.device.limits().max_texture_dimension_2d)
    }

    /// Draws `scene` as `camera` sees it into an image of `size` pixels; the image, or why it
    /// wasn't drawn.
    pub fn draw(&mut self, scene: &Scene, camera: &Camera, style: &Style, size: [u32; 2]) -> Result<&Target, NotDrawn> {
        Gpu::draws(style).map_err(|what| NotDrawn::Here(format!("{what} are drawn by the CPU view")))?;
        if let Some(o) = scene.objects.iter().find(|o| !self.fits(o)) {
            return Err(NotDrawn::Here(format!("{} is too big for one buffer of the GPU", o.label())));
        }
        if let Some(e) = self.uncaptured.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            self.objects.clear();
            self.target = None;
            return Err(NotDrawn::Failed(e));
        }
        // What the device reports while drawing (out of memory, a limit) fails the frame.
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        self.draw_unguarded(scene, camera, style, size);
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
    fn draw_unguarded(&mut self, scene: &Scene, camera: &Camera, style: &Style, size: [u32; 2]) {
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
            let mut faces: Vec<u32> = style.selected_faces.iter().filter(|(o, _)| *o == k).map(|(_, f)| *f).collect();
            faces.sort_unstable();
            faces.dedup();
            let Some(up) = self.objects.get(&obj.id()) else { continue };
            write_palette(&self.queue, &up.palette, &colours);
            for (mode, u) in up.uniforms.iter().enumerate() {
                self.queue.write_buffer(u, 0, &bytes_u32(&[u32::try_from(mode).unwrap_or(0), base, 0, 0]));
            }
            base = base.saturating_add(u32::try_from(obj.classes.len()).unwrap_or(u32::MAX));
            if up.selected.0 != faces {
                let groups = self.selected_faces(obj, up, &faces);
                if let Some(up) = self.objects.get_mut(&obj.id()) {
                    up.selected = (faces, groups);
                }
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
                color_attachments: &[attachment(&target.scene), attachment(&target.ids)],
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
                for (group, _, n) in &up.chunks {
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
                    for (_, front, n) in &up.chunks {
                        pass.set_bind_group(1, front, &[]);
                        pass.draw(0..n.saturating_mul(3), 0..1);
                    }
                }
                for (group, n) in &up.selected.1 {
                    pass.set_bind_group(1, group, &[]);
                    pass.draw(0..n.saturating_mul(3), 0..1);
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
        let depth = make("umcf depth", DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let view_of = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (view, scene, ids, depth) = (view_of(&texture), view_of(&scene), view_of(&ids), view_of(&depth));
        let outline = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("umcf outlines"),
            layout: &self.outline_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&scene) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&ids) },
            ],
        });
        self.target = Some(Target { size, texture, view, scene, ids, depth, outline });
    }

    /// `obj` on the GPU: its points, its triangles' corners and classes in chunks a storage
    /// binding holds, and a texture for its classes' colours.
    fn upload(&self, obj: &Object) -> Uploaded {
        let points: Vec<f32> = obj.points.iter().flatten().copied().collect();
        let points = self.storage("umcf points", &bytes_f32(&points));
        let classes = u32::try_from(obj.classes.len()).unwrap_or(u32::MAX).max(1);
        let palette = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("umcf colours"),
            size: wgpu::Extent3d {
                width: classes.min(PALETTE_WIDTH),
                height: classes.div_ceil(PALETTE_WIDTH),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
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
        let uniforms = [uniform(), uniform(), uniform()];
        // Triangles per chunk: their corners (12 bytes each) within a binding.
        let per = usize::try_from((self.binding / 12).max(1)).unwrap_or(usize::MAX);
        let mut chunks = Vec::new();
        let n = obj.triangles.len().min(obj.class.len());
        let mut start = 0;
        while start < n {
            let end = start.saturating_add(per).min(n);
            let corners: Vec<u32> = obj.triangles.get(start..end).unwrap_or(&[]).iter().flatten().copied().collect();
            let classes = obj.class.get(start..end).unwrap_or(&[]);
            let [shown, _, selected] = &uniforms;
            let corners = self.storage("umcf corners", &bytes_u32(&corners));
            let classes = self.storage("umcf classes", &bytes_u32(classes));
            let group = self.object_group(&points, &corners, &classes, &palette, shown);
            let front = self.object_group(&points, &corners, &classes, &palette, selected);
            chunks.push((group, front, u32::try_from(end - start).unwrap_or(u32::MAX)));
            start = end;
        }
        Uploaded { chunks, palette, points, uniforms, selected: (Vec::new(), Vec::new()) }
    }

    /// The bind groups of the selected `faces` (sorted) of `obj`, in chunks a storage binding
    /// holds, and how many triangles each has: their triangles, drawn in the selection's colour.
    fn selected_faces(&self, obj: &Object, up: &Uploaded, faces: &[u32]) -> Vec<(wgpu::BindGroup, u32)> {
        if faces.is_empty() {
            return Vec::new();
        }
        let picked: Vec<(&[u32; 3], u32)> = obj
            .triangles
            .iter()
            .zip(&obj.origin_index)
            .zip(&obj.class)
            .filter(|((_, f), _)| faces.binary_search(f).is_ok())
            .map(|((t, _), c)| (t, *c))
            .collect();
        let per = usize::try_from((self.binding / 12).max(1)).unwrap_or(usize::MAX);
        let [_, faces_uniform, _] = &up.uniforms;
        picked
            .chunks(per)
            .map(|chunk| {
                let corners: Vec<u32> = chunk.iter().flat_map(|(t, _)| **t).collect();
                // Their own classes: the outlines are those of the patches, as in the CPU view.
                let classes: Vec<u32> = chunk.iter().map(|(_, c)| *c).collect();
                let corners = self.storage("umcf corners", &bytes_u32(&corners));
                let classes = self.storage("umcf classes", &bytes_u32(&classes));
                let group = self.object_group(&up.points, &corners, &classes, &up.palette, faces_uniform);
                (group, u32::try_from(chunk.len()).unwrap_or(u32::MAX))
            })
            .collect()
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
const BACKGROUND: [[f32; 3]; 2] = [[0.94, 0.95, 0.96], [0.78, 0.81, 0.85]];

fn write_palette(queue: &wgpu::Queue, palette: &wgpu::Texture, colours: &[[u8; 4]]) {
    let size = palette.size();
    let mut bytes = vec![0u8; (size.width as usize).saturating_mul(size.height as usize).saturating_mul(4)];
    for (px, c) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(colours) {
        *px = *c;
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: palette,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: None },
        size,
    );
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

    /// A device to draw on, when this machine has one the view can use (else the test is skipped).
    fn device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
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
        let target = gpu.draw(&scene, &camera, &style, size).unwrap();
        let image = read_back(&device, &queue, target);
        // The same picture, but for pixels on the triangles' edges (rasterised a little
        // otherwise) and the shading across a triangle (per pixel here, per triangle there).
        let off = differing(&cpu, &image, 6);
        assert!(off < 0.01, "{:.1}% of the pixels differ", off * 100.0);
        // The background where there is nothing, and the selection's orange somewhere.
        assert!(image.chunks(4).any(orange), "no selection");
        // Drawn again with the selection gone: no orange, nothing uploaded again.
        style.selected.clear();
        let target = gpu.draw(&scene, &camera, &style, size).unwrap();
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
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size).unwrap());
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
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size).unwrap());
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
        let shown = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size).unwrap());
        assert!(shown.chunks(4).any(orange), "the selected faces are drawn while the top is shown");
        style.display.insert((0, 0), Display { mode: Mode::Hidden, opacity: 1.0 });
        let cpu = draw(&scene, &camera, &style).rgba;
        let image = read_back(&device, &queue, gpu.draw(&scene, &camera, &style, size).unwrap());
        assert!(!image.chunks(4).any(orange) && !cpu.chunks(4).any(orange));
        assert!(differing(&cpu, &image, 6) < 0.01);
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
    fn styles_the_gpu_view_doesnt_draw_yet_are_named() {
        let mut style = Style::default();
        assert_eq!(Gpu::draws(&style), Ok(()));
        style.edges = true;
        assert_eq!(Gpu::draws(&style), Err("edges"));
        style.edges = false;
        style.display.insert((0, 0), Display { mode: Mode::Surface, opacity: 0.5 });
        assert_eq!(Gpu::draws(&style), Err("transparent layers"));
        style.display.insert((0, 0), Display { mode: Mode::Wireframe, opacity: 1.0 });
        assert_eq!(Gpu::draws(&style), Err("wireframes"));
    }
}
