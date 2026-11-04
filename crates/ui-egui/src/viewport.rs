//! The 3D view: the engine's scene drawn by the CPU rasteriser into a texture, an orbit camera
//! (drag to turn, right or middle drag to pan, wheel to zoom), standard views, and picking. A
//! click doesn't select anything itself: it becomes a `select.pick` command with the ray under
//! the cursor, like every other change.

use eframe::egui::{
    self, Color32, ColorImage, PointerButton, Pos2, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2,
};
use umcf_engine::Sel;
use umcf_geom::bvh::Ray;
use umcf_render::camera::{Camera, View};
use umcf_render::raster::{ColourBy, Style, draw};
use umcf_render::{Object, Scene, Source};

use crate::theme::Tokens;

/// The view's state: the camera and what the texture shows.
pub struct Viewport {
    /// `None` until the scene is first shown (then it frames the scene).
    pub camera: Option<Camera>,
    pub edges: bool,
    texture: Option<TextureHandle>,
    drawn: Option<Drawn>,
    /// What is under the cursor, for the status bar.
    pub hover: Option<String>,
    last_hover: Option<Pos2>,
    /// Where the view was last shown, in points.
    pub rect: Option<Rect>,
    /// A primary drag turns the view (else it pans).
    pub orbit: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            camera: None,
            edges: false,
            texture: None,
            drawn: None,
            hover: None,
            last_hover: None,
            rect: None,
            orbit: true,
        }
    }
}

/// What the texture was drawn from.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    camera: [f64; 7],
    size: [u32; 2],
    selection: Vec<Sel>,
    edges: bool,
    revision: u64,
    dark: bool,
}

/// What the user asked the view for, done by the app (it runs commands).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Pick along `ray`: `select.pick` with this mode.
    Pick { ray: Ray, mode: &'static str },
    /// Frame the scene from this view (the axis triad).
    Fit(View),
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
    }
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
    /// The view's width over its height (1 before it was shown).
    pub fn aspect(&self) -> f64 {
        self.rect.map_or(1.0, |r| f64::from(r.width().max(1.0) / r.height().max(1.0)))
    }

    /// Frames the scene from `view`.
    pub fn fit(&mut self, scene: &Scene, view: View, aspect: f64) {
        if let Some(b) = scene.bbox() {
            self.camera = Some(Camera::fit(view, &b, aspect));
        }
    }

    /// Shows `scene` in the rest of `ui`; returns what the user asked for.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        scene: &Scene,
        selection: &[Sel],
        revision: u64,
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

        // Navigation.
        let dragging = response.dragged();
        let primary = response.dragged_by(PointerButton::Primary);
        let other = response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle);
        if primary && self.orbit {
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
            selection: selection.to_vec(),
            edges: self.edges,
            revision,
            dark: tokens.dark,
        };
        if self.drawn.as_ref() != Some(&drawn) || self.texture.is_none() {
            let mut selected = Vec::new();
            for (k, o) in scene.objects.iter().enumerate() {
                for (c, name) in o.classes.iter().enumerate() {
                    let sel = match &o.source {
                        Source::Mesh { region } => Sel::Patch { region: region.clone(), patch: name.clone() },
                        Source::Surface { file } => Sel::Solid { file: file.clone(), solid: name.clone() },
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
                colour_by: ColourBy::Class,
                background: Some(tokens.view_background()),
            };
            let image = draw(scene, &cam, &style);
            let colour = ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &image.rgba);
            match &mut self.texture {
                Some(t) => t.set(colour, TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("viewport", colour, TextureOptions::LINEAR)),
            }
            self.drawn = Some(drawn);
        }
        if let Some(t) = &self.texture {
            ui.painter().image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // What is under the cursor (read only: the scene's BVH, no command).
        let shown = self.drawn.as_ref().map_or(size, |d| d.size);
        match response.hover_pos() {
            Some(p) if !dragging && self.last_hover != Some(p) => {
                self.last_hover = Some(p);
                let hits = scene.pick_classes(&ray_at(&cam, rect, p, shown));
                self.hover = hits.first().and_then(|h| scene.objects.get(h.object).map(|o| class_label(o, h.class)));
            }
            Some(_) => {}
            None => {
                self.last_hover = None;
                self.hover = None;
            }
        }

        let mut action = self.triad(ui, rect, &cam, tokens);
        if response.clicked()
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
            action = Some(Action::Pick { ray: ray_at(&cam, rect, p, shown), mode });
        }
        action
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
