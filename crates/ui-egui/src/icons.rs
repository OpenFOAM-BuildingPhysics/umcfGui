//! Icons, drawn in code (no image assets): strokes in the ink colour with parts filled in the
//! accent colour. Coordinates are in a unit square centred on the icon (−0.5 to 0.5, y down).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2};

/// The icons there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Open,
    Save,
    Undo,
    Redo,
    Info,
    Check,
    Controls,
    Import,
    SurfaceCheck,
    Repair,
    Scale,
    MeshCheck,
    SelectNone,
    Snapshot,
    ViewIso,
    ViewTop,
    ViewFront,
    ViewRight,
    Edges,
    Theme,
    Search,
    Home,
    Orbit,
    Pan,
    ZoomIn,
    ZoomOut,
    Fit,
    Eye,
    EyeOff,
    Region,
    Patch,
    /// A boundary condition: a patch with what flows in through it.
    Condition,
    Surface,
    Folder,
    Case,
    Setup,
    Run,
    Results,
    Command,
    Close,
    First,
    Previous,
    Next,
    Last,
    Logo,
}

/// The colours an icon is drawn in.
#[derive(Debug, Clone, Copy)]
pub struct Ink {
    pub line: Color32,
    pub fill: Color32,
}

/// Draws `icon` into `rect` (the largest square in it).
pub fn paint(p: &Painter, rect: Rect, icon: Icon, ink: Ink) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let w = (s / 13.0).max(1.1);
    let stroke = Stroke::new(w, ink.line);
    let thin = Stroke::new(w * 0.7, ink.line);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|(x, y)| at(*x, *y)).collect(), stroke));
    };
    let closed = |pts: &[(f32, f32)], fill: Color32| {
        p.add(Shape::convex_polygon(pts.iter().map(|(x, y)| at(*x, *y)).collect(), fill, stroke));
    };
    let fill = |pts: &[(f32, f32)], fill: Color32| {
        p.add(Shape::convex_polygon(pts.iter().map(|(x, y)| at(*x, *y)).collect(), fill, Stroke::NONE));
    };
    let circle = |x: f32, y: f32, r: f32, fill: Color32, st: Stroke| {
        p.circle(at(x, y), r * s, fill, st);
    };
    let arc = |x: f32, y: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=16)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / 16.0).to_radians();
                at(x + r * a.cos(), y + r * a.sin())
            })
            .collect()
    };
    let soft = ink.fill.gamma_multiply(0.35);
    // An isometric cube: top, left and right faces.
    let cube = |cx: f32, cy: f32, r: f32, top: Color32, left: Color32, right: Color32| {
        let (h, v) = (r * 0.87, r * 0.5);
        fill(&[(cx, cy - r), (cx + h, cy - v), (cx, cy), (cx - h, cy - v)], top);
        fill(&[(cx - h, cy - v), (cx, cy), (cx, cy + r), (cx - h, cy + v)], left);
        fill(&[(cx, cy), (cx + h, cy - v), (cx + h, cy + v), (cx, cy + r)], right);
        line(&[
            (cx, cy - r),
            (cx + h, cy - v),
            (cx + h, cy + v),
            (cx, cy + r),
            (cx - h, cy + v),
            (cx - h, cy - v),
            (cx, cy - r),
        ]);
        line(&[(cx - h, cy - v), (cx, cy), (cx + h, cy - v)]);
        line(&[(cx, cy), (cx, cy + r)]);
    };
    // A small filled badge with a tick, bottom right.
    let tick = || {
        circle(0.24, 0.24, 0.2, ink.fill, Stroke::NONE);
        p.add(Shape::line(vec![at(0.14, 0.24), at(0.22, 0.32), at(0.35, 0.16)], Stroke::new(w, Color32::WHITE)));
    };
    match icon {
        Icon::Open | Icon::Folder => {
            fill(&[(-0.42, -0.22), (0.38, -0.22), (0.38, 0.3), (-0.42, 0.3)], soft);
            line(&[(-0.42, 0.3), (-0.42, -0.3), (-0.12, -0.3), (-0.04, -0.2), (0.38, -0.2), (0.38, 0.3), (-0.42, 0.3)]);
            if icon == Icon::Open {
                closed(&[(-0.3, -0.02), (0.46, -0.02), (0.34, 0.3), (-0.42, 0.3)], ink.fill);
            }
        }
        Icon::Save => {
            closed(&[(-0.38, -0.38), (0.24, -0.38), (0.38, -0.24), (0.38, 0.38), (-0.38, 0.38)], soft);
            fill(&[(-0.22, -0.38), (0.16, -0.38), (0.16, -0.12), (-0.22, -0.12)], ink.fill);
            p.rect_stroke(Rect::from_min_max(at(-0.24, 0.08), at(0.24, 0.38)), 1.0, thin, StrokeKind::Inside);
        }
        Icon::Undo | Icon::Redo => {
            let k = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let pts: Vec<Pos2> = (0..=14)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32 / 14.0);
                    at(k * (0.05 - 0.28 * a.cos()), 0.12 - 0.26 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, stroke));
            line(&[(k * 0.33, 0.12), (k * 0.33, 0.3)]);
            closed(&[(k * -0.38, 0.1), (k * -0.08, 0.1), (k * -0.23, 0.32)], ink.line);
        }
        Icon::Info => {
            circle(0.0, 0.0, 0.4, soft, stroke);
            circle(0.0, -0.19, 0.055, ink.fill, Stroke::NONE);
            line(&[(0.0, -0.04), (0.0, 0.24)]);
        }
        Icon::Check => {
            circle(0.0, 0.0, 0.4, soft, stroke);
            p.add(Shape::line(vec![at(-0.2, 0.0), at(-0.05, 0.16), at(0.22, -0.16)], Stroke::new(w * 1.4, ink.fill)));
        }
        Icon::Controls => {
            for (k, y) in [-0.24f32, 0.0, 0.24].into_iter().enumerate() {
                line(&[(-0.4, y), (0.4, y)]);
                let x = [0.15, -0.18, 0.05].get(k).copied().unwrap_or(0.0);
                circle(x, y, 0.09, ink.fill, stroke);
            }
        }
        Icon::Import => {
            closed(
                &[
                    (-0.4, 0.05),
                    (-0.18, 0.05),
                    (-0.12, 0.17),
                    (0.12, 0.17),
                    (0.18, 0.05),
                    (0.4, 0.05),
                    (0.4, 0.38),
                    (-0.4, 0.38),
                ],
                soft,
            );
            line(&[(0.0, -0.42), (0.0, 0.05)]);
            closed(&[(-0.14, -0.08), (0.14, -0.08), (0.0, 0.08)], ink.fill);
        }
        Icon::SurfaceCheck | Icon::Surface => {
            closed(&[(-0.42, 0.3), (-0.05, -0.38), (0.32, 0.3)], soft);
            line(&[(-0.24, -0.04), (0.13, -0.04)]);
            line(&[(-0.05, -0.38), (-0.05, 0.3)]);
            if icon == Icon::SurfaceCheck {
                tick();
            }
        }
        Icon::Repair => {
            p.add(Shape::line(vec![at(-0.34, 0.34), at(0.06, -0.06)], Stroke::new(w * 2.4, ink.line)));
            circle(0.16, -0.16, 0.2, ink.fill, stroke);
            circle(0.22, -0.22, 0.08, Color32::WHITE, Stroke::NONE);
        }
        Icon::Scale => {
            p.rect_filled(Rect::from_min_max(at(-0.4, -0.05), at(0.05, 0.4)), 1.0, soft);
            p.rect_stroke(Rect::from_min_max(at(-0.4, -0.05), at(0.05, 0.4)), 1.0, stroke, StrokeKind::Middle);
            line(&[(-0.05, 0.05), (0.38, -0.38)]);
            closed(&[(0.4, -0.4), (0.4, -0.12), (0.12, -0.4)], ink.fill);
        }
        Icon::MeshCheck => {
            fill(&[(-0.4, -0.4), (0.4, -0.4), (0.4, 0.4), (-0.4, 0.4)], soft);
            for k in [-0.4f32, -0.13, 0.13, 0.4] {
                p.add(Shape::line(vec![at(k, -0.4), at(k, 0.4)], thin));
                p.add(Shape::line(vec![at(-0.4, k), at(0.4, k)], thin));
            }
            tick();
        }
        Icon::SelectNone => {
            let corners = [(-0.4, -0.4), (0.4, -0.4), (0.4, 0.4), (-0.4, 0.4), (-0.4, -0.4)];
            for pair in corners.windows(2) {
                if let [a, b] = pair {
                    p.add(Shape::dashed_line(&[at(a.0, a.1), at(b.0, b.1)], thin, 0.08 * s, 0.06 * s));
                }
            }
            line(&[(-0.16, -0.16), (0.16, 0.16)]);
            line(&[(0.16, -0.16), (-0.16, 0.16)]);
        }
        Icon::Snapshot => {
            closed(
                &[
                    (-0.42, -0.2),
                    (-0.16, -0.2),
                    (-0.1, -0.32),
                    (0.1, -0.32),
                    (0.16, -0.2),
                    (0.42, -0.2),
                    (0.42, 0.32),
                    (-0.42, 0.32),
                ],
                soft,
            );
            circle(0.0, 0.05, 0.16, ink.fill, stroke);
        }
        Icon::ViewIso | Icon::Region | Icon::Setup => {
            cube(0.0, 0.0, 0.42, ink.fill, soft, ink.fill.gamma_multiply(0.6))
        }
        Icon::ViewTop => {
            cube(0.0, 0.06, 0.38, ink.fill, soft, soft);
            fill(&[(-0.1, -0.5), (0.1, -0.5), (0.0, -0.38)], ink.line);
        }
        Icon::ViewFront => cube(0.0, 0.0, 0.4, soft, ink.fill, soft),
        Icon::ViewRight => cube(0.0, 0.0, 0.4, soft, soft, ink.fill),
        Icon::Edges => {
            for k in [-0.4f32, 0.0, 0.4] {
                line(&[(k, -0.4), (k, 0.4)]);
                line(&[(-0.4, k), (0.4, k)]);
            }
            circle(0.0, 0.0, 0.08, ink.fill, Stroke::NONE);
        }
        Icon::Theme => {
            circle(0.0, 0.0, 0.36, ink.fill, stroke);
            p.add(Shape::convex_polygon(arc(0.0, 0.0, 0.33, -90.0, 90.0), Color32::WHITE, Stroke::NONE));
        }
        Icon::Search => {
            circle(-0.08, -0.08, 0.26, Color32::TRANSPARENT, stroke);
            p.add(Shape::line(vec![at(0.12, 0.12), at(0.38, 0.38)], Stroke::new(w * 1.6, ink.line)));
        }
        Icon::Home => {
            closed(&[(-0.4, -0.02), (0.0, -0.4), (0.4, -0.02)], ink.fill);
            closed(&[(-0.28, -0.04), (0.28, -0.04), (0.28, 0.38), (-0.28, 0.38)], soft);
        }
        Icon::Orbit => {
            p.add(Shape::line(arc(0.0, 0.0, 0.36, 200.0, 520.0), stroke));
            fill(&[(0.24, -0.44), (0.44, -0.22), (0.16, -0.16)], ink.fill);
            circle(0.0, 0.0, 0.09, ink.fill, Stroke::NONE);
        }
        Icon::Pan => {
            line(&[(-0.42, 0.0), (0.42, 0.0)]);
            line(&[(0.0, -0.42), (0.0, 0.42)]);
            for (x, y) in [(0.42f32, 0.0f32), (-0.42, 0.0), (0.0, 0.42), (0.0, -0.42)] {
                let (nx, ny) = (-y, x);
                fill(
                    &[(x, y), (x * 0.6 + nx * 0.4, y * 0.6 + ny * 0.4), (x * 0.6 - nx * 0.4, y * 0.6 - ny * 0.4)],
                    ink.fill,
                );
            }
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            circle(-0.08, -0.08, 0.28, soft, stroke);
            p.add(Shape::line(vec![at(0.13, 0.13), at(0.4, 0.4)], Stroke::new(w * 1.6, ink.line)));
            line(&[(-0.2, -0.08), (0.04, -0.08)]);
            if icon == Icon::ZoomIn {
                line(&[(-0.08, -0.2), (-0.08, 0.04)]);
            }
        }
        Icon::Fit => {
            for (x, y) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                line(&[(0.42 * x, 0.18 * y), (0.42 * x, 0.42 * y), (0.18 * x, 0.42 * y)]);
            }
            cube(0.0, 0.0, 0.22, ink.fill, soft, ink.fill.gamma_multiply(0.6));
        }
        Icon::Eye | Icon::EyeOff => {
            let mut outline = arc(0.0, 0.32, 0.5, 220.0, 320.0);
            outline.extend(arc(0.0, -0.32, 0.5, 40.0, 140.0));
            p.add(Shape::closed_line(outline, thin));
            if icon == Icon::Eye {
                circle(0.0, 0.0, 0.13, ink.fill, Stroke::NONE);
            } else {
                line(&[(-0.36, -0.3), (0.36, 0.3)]);
            }
        }
        Icon::Patch => {
            closed(&[(-0.36, -0.2), (0.24, -0.36), (0.36, 0.2), (-0.24, 0.36)], soft);
            circle(0.0, 0.0, 0.07, ink.fill, Stroke::NONE);
        }
        Icon::Condition => {
            closed(&[(-0.4, 0.06), (0.16, -0.06), (0.4, 0.22), (-0.16, 0.36)], soft);
            line(&[(0.0, -0.42), (0.0, 0.12)]);
            closed(&[(-0.12, -0.02), (0.12, -0.02), (0.0, 0.16)], ink.fill);
            circle(-0.24, -0.28, 0.08, ink.fill, Stroke::NONE);
        }
        Icon::Case => {
            closed(&[(-0.34, -0.4), (0.14, -0.4), (0.34, -0.2), (0.34, 0.4), (-0.34, 0.4)], soft);
            for y in [-0.08f32, 0.08, 0.24] {
                p.add(Shape::line(vec![at(-0.2, y), at(0.2, y)], Stroke::new(w * 0.8, ink.fill)));
            }
        }
        Icon::Run => closed(&[(-0.26, -0.38), (0.36, 0.0), (-0.26, 0.38)], ink.fill),
        Icon::Results => {
            for (k, h) in [0.3f32, 0.55, 0.42, 0.75].into_iter().enumerate() {
                let x = -0.36 + 0.2 * k as f32;
                let colour = if k == 3 { ink.fill } else { soft };
                p.rect_filled(Rect::from_min_max(at(x, 0.4 - h), at(x + 0.13, 0.4)), 1.0, colour);
            }
            line(&[(-0.42, 0.4), (0.42, 0.4)]);
        }
        Icon::Command => {
            closed(&[(-0.38, -0.38), (0.38, -0.38), (0.38, 0.38), (-0.38, 0.38)], soft);
            line(&[(-0.2, -0.14), (0.0, 0.02), (-0.2, 0.18)]);
            line(&[(0.04, 0.18), (0.22, 0.18)]);
        }
        Icon::Close => {
            line(&[(-0.25, -0.25), (0.25, 0.25)]);
            line(&[(0.25, -0.25), (-0.25, 0.25)]);
        }
        Icon::First | Icon::Last => {
            let k = if icon == Icon::First { -1.0 } else { 1.0 };
            fill(&[(k * 0.22, 0.0), (k * -0.18, -0.3), (k * -0.18, 0.3)], ink.line);
            line(&[(k * 0.3, -0.3), (k * 0.3, 0.3)]);
        }
        Icon::Previous | Icon::Next => {
            let k = if icon == Icon::Previous { -1.0 } else { 1.0 };
            fill(&[(k * 0.24, 0.0), (k * -0.18, -0.3), (k * -0.18, 0.3)], ink.line);
        }
        Icon::Logo => {
            // Buildings with wind lines: an urban microclimate.
            fill(&[(-0.44, 0.42), (-0.44, -0.06), (-0.16, -0.06), (-0.16, 0.42)], ink.fill);
            fill(&[(-0.12, 0.42), (-0.12, -0.42), (0.16, -0.42), (0.16, 0.42)], ink.fill);
            for (k, y) in [-0.24f32, 0.0, 0.24].into_iter().enumerate() {
                let x0 = 0.24 + 0.05 * k as f32;
                p.add(Shape::line(vec![at(x0, y), at(0.48, y)], Stroke::new(w, ink.line)));
            }
        }
    }
}
