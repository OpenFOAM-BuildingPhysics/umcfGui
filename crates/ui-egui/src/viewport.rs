//! The 3D view: the engine's scene drawn by the CPU rasteriser, or on the GPU (the `gpu` module,
//! where the window runs on wgpu and the view is switched to it), into a texture, an orbit camera
//! (drag to turn, right or middle drag to pan, wheel to zoom), standard views, and picking. A
//! click doesn't select anything itself: it becomes a `select.pick` command with the ray under
//! the cursor, like every other change; a rectangle dragged in *Box select* becomes a
//! `select.rect` with the camera and the rectangle in the image's pixels, and a stroke painted
//! with the *Brush* a `select.brush` with the stroke's points and its radius in those pixels.

use eframe::egui::{
    self, Color32, ColorImage, PointerButton, Pos2, Rect, Sense, TextureHandle, TextureId, TextureOptions, pos2, vec2,
};
use eframe::egui_wgpu;
use umcf_engine::Sel;
use umcf_geom::Vec3;
use umcf_geom::bvh::Ray;
use umcf_render::camera::{Camera, View};
use umcf_render::raster::{ColourBy, Display, Mode, Style, draw};
use umcf_render::{Object, Scene, Source};

use crate::theme::Tokens;

/// What draws the 3D view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    /// The CPU rasteriser (`umcf_render::raster`), everywhere.
    #[default]
    Cpu,
    /// The GPU (the `gpu` module), where the window runs on a GPU device that can.
    Gpu,
}

impl Renderer {
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Cpu => "CPU",
            Renderer::Gpu => "GPU",
        }
    }
}

/// The GPU view and the window's texture it draws into.
struct GpuView {
    gpu: crate::gpu::Gpu,
    /// The window's device and egui's renderer, which shows the image as a texture.
    state: egui_wgpu::RenderState,
    /// The texture egui shows, and the size it was registered at.
    texture: Option<(TextureId, [u32; 2])>,
}

impl Drop for GpuView {
    /// The window's texture of the image goes with the view (switched off after a failure).
    fn drop(&mut self) {
        if let Some((id, _)) = self.texture.take() {
            self.state.renderer.write().free_texture(&id);
        }
    }
}

impl GpuView {
    /// Draws into the GPU view's image, shown as a texture of the window; its id.
    fn draw(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        style: &Style,
        size: [u32; 2],
        revision: u64,
    ) -> Result<TextureId, crate::gpu::NotDrawn> {
        let target = self.gpu.draw(scene, camera, style, size, revision)?;
        let filter = egui_wgpu::wgpu::FilterMode::Linear;
        let mut renderer = self.state.renderer.write();
        let device = &self.state.device;
        let id = match self.texture {
            Some((id, at)) if at == target.size => id,
            Some((id, _)) => {
                renderer.update_egui_texture_from_wgpu_texture(device, &target.view, filter, id);
                id
            }
            None => renderer.register_native_texture(device, &target.view, filter),
        };
        self.texture = Some((id, target.size));
        Ok(id)
    }
}

/// The view's state: the camera and what the texture shows.
pub struct Viewport {
    /// `None` until the scene is first shown (then it frames the scene).
    pub camera: Option<Camera>,
    pub edges: bool,
    /// The grid on the ground and the origin's planes and axes (`render::guides`).
    pub guides: umcf_render::guides::Guides,
    /// A dark view (the default), else a light one, whatever the window's theme.
    pub dark: bool,
    /// What draws the view (asked for): see [`Viewport::drawn_with`] for what did.
    pub renderer: Renderer,
    texture: Option<TextureHandle>,
    gpu: Option<GpuView>,
    /// Why there is no GPU view, when there is none.
    pub no_gpu: Option<String>,
    /// The image last shown.
    image: Option<TextureId>,
    /// What drew the image last shown, and why it was the CPU when the GPU was asked for.
    drawn_with: (Renderer, Option<String>),
    drawn: Option<Drawn>,
    /// What is under the cursor, for the status bar.
    pub hover: Option<String>,
    last_hover: Option<Pos2>,
    /// Whether the hover text was made picking faces.
    hover_faces: bool,
    /// Where the view was last shown, in points.
    pub rect: Option<Rect>,
    /// What a primary drag does.
    pub drag: Drag,
    /// A click picks the mesh face under it, not its patch.
    pub faces: bool,
    /// A rectangle or a brush stroke takes everything in it, hidden or not.
    pub through: bool,
    /// Where the rectangle being dragged started, in points.
    corner: Option<Pos2>,
    /// The brush's radius, in points.
    pub brush: f32,
    /// The stroke being painted, in points.
    stroke: Vec<Pos2>,
    /// A cut's plane to draw and drag by its frame or arrow (the cut card's, set by the app
    /// each frame); `active` is the view's own.
    pub cut_plane: Option<umcf_render::guides::CutPlane>,
    /// A grid of blocks previewed (the box card's, set by the app each frame).
    pub blocks: Option<umcf_render::guides::BlockPreview>,
    /// The cut's plane held by the pointer: where, along its normal, it was taken.
    holding: Option<Hold>,
}

/// A cut's plane held: its point and unit normal when taken, and where along the normal the
/// pointer's ray passed then.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hold {
    point: Vec3,
    normal: Vec3,
    base: Vec3,
    taken: f64,
}

/// How near (points) the pointer has to be to a handle of the cut's plane to take it.
const GRAB: f32 = 8.0;

/// The brush's smallest, default and largest radius, in points.
pub const BRUSH: (f32, f32, f32) = (2.0, 12.0, 200.0);

/// Most points a stroke keeps (as `select.brush` takes them): past them it stops growing.
const STROKE_POINTS: usize = umcf_render::raster::MAX_STROKE;

/// What a primary drag in the view does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Drag {
    /// Turns the view.
    #[default]
    Orbit,
    /// Moves the view.
    Pan,
    /// Draws a rectangle that selects what is in it.
    Select,
    /// Paints a stroke that selects what is seen under it.
    Brush,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            camera: None,
            edges: false,
            guides: umcf_render::guides::Guides::ALL,
            dark: true,
            renderer: Renderer::Cpu,
            texture: None,
            gpu: None,
            no_gpu: Some("the window doesn't run on a GPU device".to_string()),
            image: None,
            drawn_with: (Renderer::Cpu, None),
            drawn: None,
            hover: None,
            last_hover: None,
            hover_faces: false,
            rect: None,
            drag: Drag::Orbit,
            faces: false,
            through: false,
            corner: None,
            brush: BRUSH.1,
            stroke: Vec::new(),
            cut_plane: None,
            blocks: None,
            holding: None,
        }
    }
}

/// What the texture was drawn from.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    camera: [f64; 7],
    size: [u32; 2],
    edges: bool,
    guides: umcf_render::guides::Guides,
    /// The cut's plane drawn, as it is drawn (where, and whether held or under the pointer).
    cut: Option<umcf_render::guides::CutPlane>,
    /// The grid of blocks previewed.
    blocks: Option<umcf_render::guides::BlockPreview>,
    /// Picking faces, for the hover text.
    faces: bool,
    /// The case's revision and the layers' look (how they are drawn).
    revision: [u64; 2],
    dark: bool,
    renderer: Renderer,
}

/// What the user asked the view for, done by the app (it runs commands).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Pick along `ray`: `select.pick` with this mode, the face under it when `faces`.
    Pick { ray: Ray, mode: &'static str, faces: bool },
    /// Select in `rect` (pixels of an image of `size` that `camera` sees): `select.rect`.
    Rect { camera: Camera, size: [u32; 2], rect: [f64; 4], mode: &'static str, faces: bool, through: bool },
    /// Select under `stroke` with `radius` (pixels of an image of `size` that `camera` sees):
    /// `select.brush`.
    Brush {
        camera: Camera,
        size: [u32; 2],
        stroke: Vec<[f64; 2]>,
        radius: f64,
        mode: &'static str,
        faces: bool,
        through: bool,
    },
    /// Frame the scene from this view (the axis triad).
    Fit(View),
    /// The cut's plane dragged: through `point` now (world), its normal as it was.
    MoveCut { point: Vec3 },
}

fn key(c: &Camera) -> [f64; 7] {
    [c.target.x, c.target.y, c.target.z, c.distance, c.azimuth, c.elevation, c.fov]
}

/// A name for people of class `class` of `object`.
pub fn class_label(object: &Object, class: u32) -> String {
    let name = object.classes.get(class as usize).map_or("?", String::as_str);
    match &object.source {
        Source::Mesh { region } if region.is_empty() => format!("patch {name}"),
        Source::Mesh { region } => format!("patch {name} ({region})"),
        Source::Surface { file } => {
            format!("{name} ({})", file.rsplit('/').next().unwrap_or(file))
        }
        Source::Cut { region } if region.is_empty() => format!("cut, {name}"),
        Source::Cut { region } => format!("cut of {region}, {name}"),
    }
}

/// `pos` (a point in `rect`, where an image of `size` pixels is shown) in the image's pixels,
/// from its top left corner.
fn pixel_at(rect: Rect, pos: Pos2, size: [u32; 2]) -> [f64; 2] {
    let [w, h] = size;
    [
        f64::from((pos.x - rect.min.x) / rect.width().max(1.0)) * f64::from(w),
        f64::from((pos.y - rect.min.y) / rect.height().max(1.0)) * f64::from(h),
    ]
}

/// Where `p` (world) is seen in `rect`, in points; `None` behind the eye.
pub(crate) fn to_screen(camera: &Camera, rect: Rect, p: Vec3) -> Option<Pos2> {
    let f = camera.frame();
    let d = p - f.eye;
    let z = d.dot(f.forward);
    if z.is_nan() || z <= 1e-9 {
        return None;
    }
    let (w, h) = (f64::from(rect.width().max(1.0)), f64::from(rect.height().max(1.0)));
    let sx = d.dot(f.right) / (z * f.tan_half * w / h);
    let sy = d.dot(f.up) / (z * f.tan_half);
    let x = rect.min.x + ((sx + 1.0) * 0.5 * w) as f32;
    let y = rect.min.y + ((1.0 - sy) * 0.5 * h) as f32;
    (x.is_finite() && y.is_finite()).then(|| pos2(x, y))
}

/// Whether `pos` is within [`GRAB`] of the frame or the arrow of the cut's plane `h`.
pub(crate) fn on_handle(camera: &Camera, rect: Rect, h: &umcf_render::guides::Handles, pos: Pos2) -> bool {
    let near = |a: Vec3, b: Vec3| match (to_screen(camera, rect, a), to_screen(camera, rect, b)) {
        (Some(a), Some(b)) => {
            let ab = b - a;
            let t = if ab.length_sq() > 0.0 { ((pos - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            (a + ab * t).distance(pos) <= GRAB
        }
        _ => false,
    };
    let edge = |k: usize| h.corners.get(k).zip(h.corners.get((k + 1) % 4)).is_some_and(|(a, b)| near(*a, *b));
    (0..4).any(edge) || near(h.base, h.tip)
}

/// Where along the line through `base` in direction `normal` (unit) `ray` passes nearest;
/// `None` when the ray runs nearly along it (within about 6°: seen end on, a pixel would move
/// the plane far) or the nearest point is behind the eye (the pointer past where the line
/// vanishes).
pub(crate) fn along(ray: &Ray, base: Vec3, normal: Vec3) -> Option<f64> {
    let o = Vec3::new(ray.origin[0], ray.origin[1], ray.origin[2]);
    let d = Vec3::new(ray.direction[0], ray.direction[1], ray.direction[2]);
    let b = normal.dot(d);
    let denom = 1.0 - b * b;
    if denom.is_nan() || denom <= 0.01 {
        return None;
    }
    let w = base - o;
    let t = (b * d.dot(w) - normal.dot(w)) / denom;
    let in_front = d.dot(w) + b * t;
    (t.is_finite() && in_front > 0.0).then_some(t)
}

/// The ray through `pos` (a point in `rect`, where an image of `size` pixels is shown).
fn ray_at(camera: &Camera, rect: Rect, pos: Pos2, size: [u32; 2]) -> Ray {
    let [w, h] = size;
    // Pixel coordinates whose middle is under the cursor.
    let x = f64::from((pos.x - rect.min.x) / rect.width().max(1.0)) * f64::from(w) - 0.5;
    let y = f64::from((pos.y - rect.min.y) / rect.height().max(1.0)) * f64::from(h) - 0.5;
    camera.ray(x, y, w, h)
}

impl Viewport {
    /// The view's background, top and bottom: dark or light as switched.
    pub fn background(&self) -> [[f32; 3]; 2] {
        use umcf_render::raster::{BOTTOM, LIGHT_BOTTOM, LIGHT_TOP, TOP};
        if self.dark { [TOP, BOTTOM] } else { [LIGHT_TOP, LIGHT_BOTTOM] }
    }

    /// The view's width over its height (1 before it was shown).
    pub fn aspect(&self) -> f64 {
        self.rect.map_or(1.0, |r| f64::from(r.width().max(1.0) / r.height().max(1.0)))
    }

    /// Frames the scene from `view`.
    pub fn fit(&mut self, scene: &Scene, view: View, aspect: f64) {
        // An empty scene (only a grid previewed): framed by the grid.
        let grid = || self.blocks.as_ref().and_then(umcf_render::guides::BlockPreview::bbox);
        if let Some(b) = scene.bbox().or_else(grid) {
            self.camera = Some(Camera::fit(view, &b, aspect));
        }
    }

    /// The GPU view on the window's wgpu device, for [`Renderer::Gpu`]; why not, when the
    /// device can't draw it.
    pub fn attach_gpu(&mut self, state: &egui_wgpu::RenderState, ctx: &egui::Context) -> Result<(), String> {
        let gpu = crate::gpu::Gpu::new(&state.adapter, &state.device, &state.queue)?;
        let ctx = ctx.clone();
        gpu.wake_with(move || ctx.request_repaint());
        self.gpu = Some(GpuView { gpu, state: state.clone(), texture: None });
        self.no_gpu = None;
        Ok(())
    }

    /// Whether the view can be drawn on the GPU.
    pub fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// What drew the image last shown, and why the CPU did when the GPU was asked for.
    pub fn drawn_with(&self) -> (Renderer, Option<&str>) {
        (self.drawn_with.0, self.drawn_with.1.as_deref())
    }

    /// The image last drawn, over `rect` (while a job runs); nothing before the first.
    pub fn paint_image(&self, painter: &egui::Painter, rect: Rect) {
        if let Some(id) = self.image {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(id, rect, uv, Color32::WHITE);
        }
    }

    /// The view as last drawn, over `rect`, while its scene is built: the image and the triad.
    pub fn paint_last(&self, ui: &mut egui::Ui, rect: Rect, tokens: &Tokens) {
        self.paint_image(ui.painter(), rect);
        // The triad stays (it would blink for the frames the scene takes); a click on it in
        // those frames does nothing.
        if let Some(cam) = self.camera {
            let _ = self.triad(ui, rect, &cam, tokens);
        }
    }

    /// Shows `scene` in the rest of `ui`; returns what the user asked for.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        scene: &Scene,
        selection: &[Sel],
        display: &std::collections::HashMap<(usize, u32), Display>,
        revision: [u64; 2],
        tokens: &Tokens,
    ) -> Option<Action> {
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        self.rect = Some(rect);
        let aspect = f64::from(rect.width().max(1.0) / rect.height().max(1.0));
        if self.camera.is_none() {
            self.fit(scene, View::Iso, aspect);
        }
        let Some(mut cam) = self.camera else {
            ui.put(rect, egui::Label::new("Nothing to show"));
            return None;
        };

        // The cut's plane: taken by its frame or arrow, then moved along its normal where the
        // pointer's ray passes (by the pointer's position, so that frames the view waits for
        // the scene in lose nothing); let go, it stays.
        let view_size = [rect.width().max(1.0).round() as u32, rect.height().max(1.0).round() as u32];
        let handles = self.cut_plane.and_then(|c| c.handles());
        let pointer_at = ui.input(|i| i.pointer.latest_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        let over = match (&handles, response.hover_pos()) {
            (Some(h), Some(p)) => on_handle(&cam, rect, h, p),
            _ => false,
        };
        let mut moved = None;
        // The card closed, or its plane turned (another axis or region), while it was held.
        let turned = match (self.holding, &handles) {
            (Some(hold), Some(h)) => hold.normal != h.normal,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if turned {
            self.holding = None;
        }
        if let (true, Some(c), Some(h)) = (response.drag_started_by(PointerButton::Primary), self.cut_plane, &handles) {
            let start = ui.input(|i| i.pointer.press_origin()).or(pointer_at);
            if let Some(p) = start.filter(|p| on_handle(&cam, rect, h, *p)) {
                let ray = ray_at(&cam, rect, p, view_size);
                if let Some(taken) = along(&ray, h.base, h.normal) {
                    self.holding = Some(Hold { point: c.point, normal: h.normal, base: h.base, taken });
                }
            }
        }
        // Where the pointer is now, also when let go (the frames before may have been ones the
        // view waited for the scene in): the last move counts.
        let held = self.holding.is_some();
        if let (Some(hold), Some(p)) = (self.holding, pointer_at) {
            let ray = ray_at(&cam, rect, p, view_size);
            if let Some(t) = along(&ray, hold.base, hold.normal) {
                moved = Some(Action::MoveCut { point: hold.point + hold.normal * (t - hold.taken) });
            }
        }
        if !down {
            self.holding = None;
        }
        // Held this frame (let go in it too): the view does nothing else.
        let holding = held;
        if over || self.holding.is_some() {
            ui.ctx().set_cursor_icon(if self.holding.is_some() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }

        // Navigation.
        let primary = response.dragged_by(PointerButton::Primary) && !holding;
        let selecting = matches!(self.drag, Drag::Select | Drag::Brush) && primary;
        // Drawing a rectangle doesn't move the view, so it is drawn at full resolution.
        let dragging = response.dragged() && !selecting && !holding;
        let other = response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle);
        if selecting {
            if response.drag_started_by(PointerButton::Primary) && !holding {
                let start = ui.input(|i| i.pointer.press_origin()).or_else(|| response.interact_pointer_pos());
                if self.drag == Drag::Brush {
                    self.stroke = start.into_iter().collect();
                } else {
                    self.corner = start;
                }
            }
        } else if primary && self.drag == Drag::Orbit {
            let d = response.drag_delta();
            cam.azimuth -= f64::from(d.x) * 0.4;
            cam.elevation = (cam.elevation + f64::from(d.y) * 0.4).clamp(-90.0, 90.0);
        } else if primary || other {
            let d = response.drag_delta();
            let f = cam.frame();
            let per_point = 2.0 * cam.distance * f.tan_half / f64::from(rect.height().max(1.0));
            cam.target = cam.target - f.right * (f64::from(d.x) * per_point) + f.up * (f64::from(d.y) * per_point);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                cam.distance = (cam.distance * (-f64::from(scroll) * 0.002).exp()).max(1e-6);
            }
        }
        self.camera = Some(cam);

        // Draw at the screen's pixels; at half of them while turning, to keep up.
        let ppp = ui.ctx().pixels_per_point() * if dragging { 0.5 } else { 1.0 };
        // Within what the renderer draws and a texture holds, with the view's aspect kept (the
        // image is stretched over the view, and picking uses the size drawn).
        let max_side = ui.ctx().input(|i| i.max_texture_side).clamp(1, umcf_render::raster::MAX_SIDE as usize) as f32;
        let (mut w, mut h) = (rect.width().max(1.0) * ppp, rect.height().max(1.0) * ppp);
        let fit =
            (max_side / w).min(max_side / h).min((umcf_render::raster::MAX_PIXELS as f32 / (w * h)).sqrt()).min(1.0);
        w *= fit;
        h *= fit;
        let size = [w.round().max(1.0) as u32, h.round().max(1.0) as u32];
        let size = {
            let (w, h) = umcf_render::raster::size(size[0], size[1]);
            [w, h]
        };
        let drawn = Drawn {
            camera: key(&cam),
            size,
            faces: self.faces,
            edges: self.edges,
            guides: self.guides,
            cut: self.cut_plane.map(|c| umcf_render::guides::CutPlane { active: over || self.holding.is_some(), ..c }),
            blocks: self.blocks.clone(),
            revision,
            dark: self.dark,
            renderer: self.renderer,
        };
        // An error the GPU device reported since: drawn again, to go back to the CPU, though
        // nothing else changed.
        let gpu_failed = self.renderer == Renderer::Gpu && self.gpu.as_ref().is_some_and(|g| g.gpu.failed());
        if self.drawn.as_ref() != Some(&drawn) || self.image.is_none() || gpu_failed {
            let mut selected = Vec::new();
            for (k, o) in scene.objects.iter().enumerate() {
                for (c, name) in o.classes.iter().enumerate() {
                    let sel = match &o.source {
                        Source::Mesh { region } => Sel::Patch { region: region.clone(), patch: name.clone() },
                        Source::Surface { file } => Sel::Solid { file: file.clone(), solid: name.clone() },
                        // Nothing of a cut is selected.
                        Source::Cut { .. } => continue,
                    };
                    if let (true, Ok(c)) = (selection.contains(&sel), u32::try_from(c)) {
                        selected.push((k, c));
                    }
                }
            }
            let style = Style {
                width: size[0],
                height: size[1],
                edges: self.edges,
                selected,
                selected_faces: umcf_engine::selected_faces(scene, selection),
                colour_by: ColourBy::Class,
                display: display.clone(),
                background: Some(self.background()),
                guides: self.guides,
                cut_plane: drawn.cut,
                blocks: drawn.blocks.clone(),
            };
            // On the GPU when asked for and it can; else, or when it can't draw this, the CPU.
            use crate::gpu::NotDrawn;
            let on_gpu = match (self.renderer, &mut self.gpu) {
                (Renderer::Gpu, Some(g)) => match g.draw(scene, &cam, &style, size, revision[0]) {
                    Ok(id) => Ok(id),
                    Err(NotDrawn::Here(why)) => Err(Some(why)),
                    // The GPU failed: the view is the CPU's from now on, and says why.
                    Err(NotDrawn::Failed(e)) => {
                        let why = format!("the GPU failed: {e}");
                        self.gpu = None;
                        self.no_gpu = Some(why.clone());
                        self.renderer = Renderer::Cpu;
                        Err(Some(why))
                    }
                },
                (Renderer::Gpu, None) => Err(self.no_gpu.clone()),
                (Renderer::Cpu, _) => Err(None),
            };
            match on_gpu {
                Ok(id) => {
                    self.image = Some(id);
                    self.drawn_with = (Renderer::Gpu, None);
                }
                Err(why) => {
                    let image = draw(scene, &cam, &style);
                    let colour =
                        ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &image.rgba);
                    match &mut self.texture {
                        Some(t) => t.set(colour, TextureOptions::LINEAR),
                        None => self.texture = Some(ui.ctx().load_texture("viewport", colour, TextureOptions::LINEAR)),
                    }
                    self.image = self.texture.as_ref().map(TextureHandle::id);
                    self.drawn_with = (Renderer::Cpu, why);
                }
            }
            self.drawn = Some(drawn);
        }
        if let Some(id) = self.image {
            ui.painter().image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // What is under the cursor (read only: the scene's BVH, no command).
        let shown = self.drawn.as_ref().map_or(size, |d| d.size);
        match response.hover_pos() {
            Some(p) if !dragging && (self.last_hover != Some(p) || self.hover_faces != self.faces) => {
                self.last_hover = Some(p);
                self.hover_faces = self.faces;
                let ray = ray_at(&cam, rect, p, shown);
                // What a click picks: the nearest that isn't hidden; picking faces, the nearest
                // mesh face, surfaces passed over.
                let hidden = |o: usize, c: u32| display.get(&(o, c)).is_some_and(|d| d.mode == Mode::Hidden);
                self.hover = scene.pick(&ray, usize::MAX).into_iter().find_map(|h| {
                    let o = scene.objects.get(h.object)?;
                    if hidden(h.object, h.class) {
                        return None;
                    }
                    match (self.faces, &o.source) {
                        // A pick passes through a cut: the hover names what it would pick.
                        (_, Source::Cut { .. }) => None,
                        (true, Source::Mesh { .. }) => Some(format!("face {} of {}", h.index, class_label(o, h.class))),
                        (true, Source::Surface { .. }) => None,
                        (false, _) => Some(class_label(o, h.class)),
                    }
                });
            }
            Some(_) => {}
            None => {
                self.last_hover = None;
                self.hover = None;
            }
        }

        let mut action = self.triad(ui, rect, &cam, tokens);
        // The rectangle being dragged; let go, it selects. Let go with no position known, or
        // found up without a release seen (the view wasn't shown meanwhile), it is dropped.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let released = response.drag_stopped();
        if let Some(a) = self.corner {
            if let (true, Some(b)) = (selecting, pointer) {
                let r = Rect::from_two_pos(a, b).intersect(rect);
                let fill = tokens.accent.gamma_multiply(0.12);
                ui.painter().rect(r, 0.0, fill, egui::Stroke::new(1.0, tokens.accent), egui::StrokeKind::Inside);
            }
            if released || !ui.input(|i| i.pointer.primary_down()) {
                self.corner = None;
            }
            if let (true, Some(b)) = (released, pointer) {
                let r = Rect::from_two_pos(a, b).intersect(rect);
                let m = ui.input(|i| i.modifiers);
                let mode = if m.command {
                    "remove"
                } else if m.shift {
                    "add"
                } else {
                    "replace"
                };
                let (p, q) = (pixel_at(rect, r.min, shown), pixel_at(rect, r.max, shown));
                // A rectangle of no pixels selects nothing: a slip of the mouse, not a choice.
                if (q[0] - p[0]).abs() >= 1.0 && (q[1] - p[1]).abs() >= 1.0 {
                    action = Some(Action::Rect {
                        camera: cam,
                        size: shown,
                        rect: [p[0], p[1], q[0], q[1]],
                        mode,
                        faces: self.faces,
                        through: self.through,
                    });
                }
            }
        }
        if self.drag != Drag::Select {
            self.corner = None;
        }
        if let Some(a) = self.brush_input(ui, &response, rect, &cam, shown, tokens) {
            action = Some(a);
        } else if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let m = ui.input(|i| i.modifiers);
            let mode = if m.command {
                "toggle"
            } else if m.shift {
                "add"
            } else {
                "replace"
            };
            action = Some(Action::Pick { ray: ray_at(&cam, rect, p, shown), mode, faces: self.faces });
        }
        // Dragging the cut's plane is all the view does meanwhile.
        if holding {
            return moved;
        }
        action
    }

    /// The brush: its size (`[` and `]`), its outline under the cursor, the stroke being painted;
    /// let go (or a click, a dab), it selects (Shift adds, Ctrl takes away). Found up without a
    /// release seen, the stroke is dropped.
    fn brush_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        cam: &Camera,
        shown: [u32; 2],
        tokens: &Tokens,
    ) -> Option<Action> {
        if self.drag != Drag::Brush {
            self.stroke.clear();
            return None;
        }
        // Not while a text field takes the keys.
        if response.hovered() && !ui.ctx().text_edit_focused() {
            let (smaller, larger) =
                ui.input(|i| (i.key_pressed(egui::Key::OpenBracket), i.key_pressed(egui::Key::CloseBracket)));
            if smaller {
                self.brush = (self.brush / 1.25).max(BRUSH.0);
            }
            if larger {
                self.brush = (self.brush * 1.25).min(BRUSH.2);
            }
        }
        let r = self.brush;
        let painter = ui.painter().with_clip_rect(rect);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        let released = response.drag_stopped();
        if !self.stroke.is_empty()
            && let Some(p) = pointer
        {
            // A point each third of the radius moved, up to what select.brush takes.
            if self.stroke.last().is_none_or(|q| q.distance(p) >= r / 3.0) && self.stroke.len() < STROKE_POINTS {
                self.stroke.push(p);
            }
        }
        if self.stroke.len() > 1 {
            painter.add(egui::Shape::line(
                self.stroke.clone(),
                egui::Stroke::new(2.0 * r, tokens.accent.gamma_multiply(0.25)),
            ));
        }
        // Its round ends (one disc for a dab).
        let ends = match self.stroke.as_slice() {
            [] => vec![],
            [one] => vec![*one],
            [first, .., last] => vec![*first, *last],
        };
        for q in ends {
            painter.circle_filled(q, r, tokens.accent.gamma_multiply(0.25));
        }
        if let Some(p) = response.hover_pos().or(pointer.filter(|_| !self.stroke.is_empty())) {
            painter.circle_stroke(p, r, egui::Stroke::new(1.0, tokens.accent));
        }
        let m = ui.input(|i| i.modifiers);
        let mode = if m.command {
            "remove"
        } else if m.shift {
            "add"
        } else {
            "replace"
        };
        let dab = response.clicked().then(|| response.interact_pointer_pos()).flatten();
        let points: Vec<Pos2> = if released {
            std::mem::take(&mut self.stroke)
        } else if let Some(p) = dab {
            vec![p]
        } else {
            if !down {
                self.stroke.clear();
            }
            return None;
        };
        if points.is_empty() {
            return None;
        }
        // The radius in the image's pixels (the image is stretched over the view, its aspect
        // kept but for rounding: the mean of the two scales).
        let scale = (shown[0] as f32 / rect.width().max(1.0) + shown[1] as f32 / rect.height().max(1.0)) * 0.5;
        let radius = f64::from(r * scale);
        Some(Action::Brush {
            camera: *cam,
            size: shown,
            stroke: points.iter().map(|p| pixel_at(rect, *p, shown)).collect(),
            radius,
            mode,
            faces: self.faces,
            through: self.through,
        })
    }

    /// The axis triad in the top right corner: the world axes as the camera sees them; a click
    /// on an axis looks along it.
    fn triad(&self, ui: &mut egui::Ui, rect: Rect, cam: &Camera, t: &Tokens) -> Option<Action> {
        let centre = pos2(rect.right() - 62.0, rect.top() + 60.0);
        let p = ui.painter();
        p.circle(centre, 46.0, t.card.gamma_multiply(0.75), egui::Stroke::new(1.0, t.card_border));
        let f = cam.frame();
        let axes = [
            ("X", umcf_geom::Vec3::new(1.0, 0.0, 0.0), View::Right, View::Left),
            ("Y", umcf_geom::Vec3::new(0.0, 1.0, 0.0), View::Back, View::Front),
            ("Z", umcf_geom::Vec3::new(0.0, 0.0, 1.0), View::Top, View::Bottom),
        ];
        // Farther ends first, so nearer ones are drawn over them.
        let mut ends: Vec<(f64, Pos2, usize, bool)> = Vec::new();
        for (k, (_, a, _, _)) in axes.iter().enumerate() {
            let d = vec2(a.dot(f.right) as f32, -a.dot(f.up) as f32) * 34.0;
            let depth = a.dot(f.forward);
            ends.push((depth, centre + d, k, true));
            ends.push((-depth, centre - d, k, false));
        }
        ends.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut action = None;
        for (_, end, k, positive) in ends {
            let Some((name, _, plus, minus)) = axes.get(k) else { continue };
            let colour = t.axis.get(k).copied().unwrap_or(t.text);
            let hit = ui.interact(
                Rect::from_center_size(end, vec2(20.0, 20.0)),
                ui.scope_id().with(("axis", k, positive)),
                Sense::click(),
            );
            let hot = hit.hovered();
            let p = ui.painter();
            if positive {
                p.line_segment([centre, end], egui::Stroke::new(2.5, colour));
                p.circle_filled(end, if hot { 10.0 } else { 9.0 }, colour);
                p.text(end, egui::Align2::CENTER_CENTER, *name, crate::theme::strong(11.0), Color32::WHITE);
            } else {
                p.circle(end, if hot { 7.0 } else { 6.0 }, t.card, egui::Stroke::new(2.0, colour));
            }
            let view = if positive { *plus } else { *minus };
            let label = format!("{} view", if positive { format!("+{name}") } else { format!("-{name}") });
            hit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if hit
                .on_hover_text(format!("Look along {}", if positive { format!("−{name}") } else { format!("+{name}") }))
                .clicked()
            {
                action = Some(Action::Fit(view));
            }
        }
        action
    }
}
