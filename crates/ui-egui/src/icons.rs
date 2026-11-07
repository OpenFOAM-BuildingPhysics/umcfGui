//! Icons, drawn in code (no image assets), on a grid of 64 by 64 (y down). The application bar's
//! and the navigation's are monochrome line art in the text's colour; the others are one object
//! in three greys outlined in the ink (isometric boxes for regions, patches, meshes and views; a
//! triangulated sheet for surfaces; a document, a folder) with one cue in the accent: what the
//! command does or checks. Strokes have round caps and joins, in three weights (outlines,
//! actions, navigation). The ink and the accent come from `theme::Tokens` (through [`Ink`]), the
//! greys from its `icon` shades (the dark theme's for a light ink), and the 3D view's colours (its
//! dark background, the origin's planes and axes) from `umcf_render`. A faded ink fades the whole
//! icon. In the spirit of SolidCraft's icons, drawn anew.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2};

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
    /// The 3D view drawn on the GPU: a chip with its pins.
    Gpu,
    /// The grid on the ground: an isometric lattice.
    Grid,
    /// The origin: three axes from a point, with a square between two.
    Origin,
    /// The 3D view dark: a square, half of it filled.
    DarkView,
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
    /// Picking single faces: a grid with one cell taken.
    Faces,
    /// Dragging a rectangle selects: a dashed box with a pointer.
    BoxSelect,
    /// Painting selects: a brush and the swath it left.
    Brush,
    /// A rectangle takes hidden things too: two boxes, one behind the other, and a ray.
    Through,
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

impl Icon {
    /// Every icon, for the contact sheet.
    pub const ALL: [Icon; 53] = [
        Icon::Open,
        Icon::Save,
        Icon::Undo,
        Icon::Redo,
        Icon::Info,
        Icon::Check,
        Icon::Controls,
        Icon::Import,
        Icon::SurfaceCheck,
        Icon::Repair,
        Icon::Scale,
        Icon::MeshCheck,
        Icon::SelectNone,
        Icon::Snapshot,
        Icon::ViewIso,
        Icon::ViewTop,
        Icon::ViewFront,
        Icon::ViewRight,
        Icon::Edges,
        Icon::Gpu,
        Icon::Grid,
        Icon::Origin,
        Icon::DarkView,
        Icon::Theme,
        Icon::Search,
        Icon::Home,
        Icon::Orbit,
        Icon::Pan,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::Fit,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Region,
        Icon::Patch,
        Icon::Faces,
        Icon::BoxSelect,
        Icon::Brush,
        Icon::Through,
        Icon::Condition,
        Icon::Surface,
        Icon::Folder,
        Icon::Case,
        Icon::Setup,
        Icon::Run,
        Icon::Results,
        Icon::Command,
        Icon::Close,
        Icon::First,
        Icon::Previous,
        Icon::Next,
        Icon::Last,
        Icon::Logo,
    ];
}

/// The colours an icon is drawn in.
#[derive(Debug, Clone, Copy)]
pub struct Ink {
    pub line: Color32,
    pub fill: Color32,
}

/// Draws `icon` into `rect` (the largest square in it): `ink.line` is the line art's colour (the
/// text's; faded, it fades the whole icon), `ink.fill` the accent.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, ink: Ink) {
    let d = Draw::new(p, rect, ink);
    let (out, act, nav) = (d.out, d.act, d.nav);
    // An isometric box of half side 12 and height 24 at the middle, its faces passive.
    let box_ = |fills: [Color32; 3]| d.block(32.0, 33.0, 12.0, 24.0, fills);
    let passive = [d.p1, d.p2, d.p3];
    match icon {
        // --- The application bar and files: an object in grey, one cue in the accent.
        Icon::Open | Icon::Folder => {
            d.shape(&[(8.0, 50.0), (8.0, 18.0), (25.0, 18.0), (29.0, 23.0), (56.0, 23.0), (56.0, 50.0)], d.p2, out);
            if icon == Icon::Open {
                d.shape(&[(14.0, 31.0), (60.0, 31.0), (54.0, 50.0), (8.0, 50.0)], d.p1, out);
                d.arrow((44.0, 41.0), (44.0, 9.0), act, d.a3);
            } else {
                d.shape(&[(8.0, 28.0), (56.0, 28.0), (56.0, 50.0), (8.0, 50.0)], d.p1, out);
            }
        }
        Icon::Save => {
            d.shape(&[(10.0, 10.0), (46.0, 10.0), (54.0, 18.0), (54.0, 54.0), (10.0, 54.0)], d.p1, out);
            d.shape(&[(20.0, 10.0), (42.0, 10.0), (42.0, 24.0), (20.0, 24.0)], d.p3, out);
            d.shape(&[(18.0, 34.0), (46.0, 34.0), (46.0, 54.0), (18.0, 54.0)], d.a1, out);
            d.line(&[(24.0, 42.0), (40.0, 42.0)], out, d.a3);
        }
        Icon::Case => {
            d.shape(&[(14.0, 6.0), (40.0, 6.0), (52.0, 18.0), (52.0, 58.0), (14.0, 58.0)], d.p1, out);
            d.line(&[(40.0, 6.0), (40.0, 18.0), (52.0, 18.0)], out, d.ink);
            for y in [30.0, 39.0, 48.0] {
                d.line(&[(22.0, y), (44.0, y)], act * 0.7, if y < 31.0 { d.a3 } else { d.guide });
            }
        }
        Icon::Command => {
            d.shape(&[(6.0, 12.0), (58.0, 12.0), (58.0, 52.0), (6.0, 52.0)], d.p1, out);
            d.line(&[(16.0, 24.0), (26.0, 32.0), (16.0, 40.0)], act, d.a3);
            d.line(&[(31.0, 41.0), (46.0, 41.0)], act, d.ink);
        }
        Icon::Info => {
            d.disc(32.0, 32.0, 25.0, d.a1, out);
            d.dot(32.0, 19.5, 3.4, d.a3);
            d.line(&[(32.0, 28.0), (32.0, 45.0)], act * 1.1, d.a3);
        }
        Icon::Check => {
            d.disc(32.0, 32.0, 25.0, d.a1, out);
            d.line(&[(20.0, 33.0), (28.0, 41.0), (44.0, 23.0)], act * 1.1, d.a3);
        }
        Icon::Controls => {
            for (y, k) in [(16.0, 40.0), (32.0, 22.0), (48.0, 34.0)] {
                d.line(&[(8.0, y), (56.0, y)], out * 1.2, d.guide);
                d.disc(k, y, 6.0, d.a2, out);
            }
        }
        Icon::Snapshot => {
            d.shape(
                &[
                    (6.0, 52.0),
                    (6.0, 20.0),
                    (20.0, 20.0),
                    (25.0, 12.0),
                    (39.0, 12.0),
                    (44.0, 20.0),
                    (58.0, 20.0),
                    (58.0, 52.0),
                ],
                d.p2,
                out,
            );
            d.disc(32.0, 35.0, 11.0, d.a1, out);
            d.dot(32.0, 35.0, 4.5, d.a3);
        }
        Icon::Results => {
            d.line(&[(8.0, 54.0), (56.0, 54.0)], out, d.ink);
            for (x, top, c) in [(12.0, 38.0, d.p2), (24.0, 28.0, d.p2), (36.0, 18.0, d.a2), (48.0, 8.0, d.a3)] {
                d.shape(&[(x, top), (x + 8.0, top), (x + 8.0, 54.0), (x, 54.0)], c, out);
            }
        }
        Icon::Run => {
            d.shape(&[(18.0, 10.0), (52.0, 32.0), (18.0, 54.0)], d.a2, out);
        }
        Icon::Theme => {
            d.disc(32.0, 32.0, 24.0, d.p1, out);
            d.half_disc(32.0, 32.0, 24.0, d.ink);
            d.circle(32.0, 32.0, 24.0, out, d.ink);
        }
        Icon::DarkView => {
            d.shape(&[(6.0, 12.0), (58.0, 12.0), (58.0, 52.0), (6.0, 52.0)], d.p1, out);
            d.shape(&[(58.0, 12.0), (58.0, 52.0), (6.0, 52.0)], d.dark_view, out);
            d.dot(44.0, 24.0, 4.0, d.a2);
        }
        Icon::Gpu => {
            for k in [22.0, 32.0, 42.0] {
                for (a, b) in
                    [((k, 6.0), (k, 14.0)), ((k, 50.0), (k, 58.0)), ((6.0, k), (14.0, k)), ((50.0, k), (58.0, k))]
                {
                    d.line(&[a, b], out * 1.3, d.ink);
                }
            }
            d.shape(&[(14.0, 14.0), (50.0, 14.0), (50.0, 50.0), (14.0, 50.0)], d.p2, out);
            d.shape(&[(23.0, 23.0), (41.0, 23.0), (41.0, 41.0), (23.0, 41.0)], d.a2, out);
        }
        Icon::Logo => {
            // Buildings with wind lines: an urban microclimate.
            d.shape(&[(4.0, 59.0), (4.0, 28.0), (22.0, 28.0), (22.0, 59.0)], d.accent, 0.0);
            d.shape(&[(24.0, 59.0), (24.0, 5.0), (42.0, 5.0), (42.0, 59.0)], d.accent, 0.0);
            for (k, y) in [16.0, 32.0, 48.0].into_iter().enumerate() {
                d.line(&[(47.0 + 3.0 * k as f32, y), (62.0, y)], out * 1.6, d.ink);
            }
        }
        // --- Navigation: monochrome line art in the text's colour.
        Icon::Undo | Icon::Redo => {
            let m = |x: f32| if icon == Icon::Undo { x } else { 64.0 - x };
            let pts: Vec<(f32, f32)> = (0..=14)
                .map(|i| {
                    let a = (200.0 + 160.0 * i as f32 / 14.0f32).to_radians();
                    (m(34.0 + 18.0 * a.cos()), 40.0 + 18.0 * a.sin())
                })
                .collect();
            d.line(&pts, nav, d.ink);
            d.head((m(17.1), 33.8), (m(16.5), 46.0), nav, d.ink);
        }
        Icon::Search => {
            d.circle(27.0, 27.0, 16.0, nav, d.ink);
            d.line(&[(39.0, 39.0), (54.0, 54.0)], nav * 1.15, d.ink);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            d.circle(27.0, 27.0, 16.0, nav, d.ink);
            d.line(&[(39.0, 39.0), (54.0, 54.0)], nav * 1.15, d.ink);
            d.line(&[(19.0, 27.0), (35.0, 27.0)], nav * 0.8, d.ink);
            if icon == Icon::ZoomIn {
                d.line(&[(27.0, 19.0), (27.0, 35.0)], nav * 0.8, d.ink);
            }
        }
        Icon::Home => {
            d.line(&[(8.0, 30.0), (32.0, 9.0), (56.0, 30.0)], nav, d.ink);
            d.line(&[(15.0, 26.0), (15.0, 55.0), (49.0, 55.0), (49.0, 26.0)], nav, d.ink);
            d.line(&[(27.0, 55.0), (27.0, 41.0), (37.0, 41.0), (37.0, 55.0)], nav * 0.8, d.ink);
        }
        Icon::Orbit => {
            // A box, outlined, and the way round it: an ellipse open at the back, its head in
            // front, to the right.
            d.block_lines(32.0, 30.0, 9.0, 16.0, out * 1.1);
            let pts: Vec<(f32, f32)> = (0..=28)
                .map(|i| {
                    let a = (-150.0 + 270.0 * i as f32 / 28.0f32).to_radians();
                    (32.0 + 27.0 * a.cos(), 38.0 + 12.0 * a.sin())
                })
                .collect();
            d.line(&pts, nav * 0.75, d.ink);
            if let (Some(a), Some(b)) = (pts.get(pts.len().saturating_sub(4)), pts.last()) {
                d.head(*a, *b, nav * 0.75, d.ink);
            }
        }
        Icon::Pan => {
            d.line(&[(32.0, 8.0), (32.0, 56.0)], nav * 0.8, d.ink);
            d.line(&[(8.0, 32.0), (56.0, 32.0)], nav * 0.8, d.ink);
            for (a, b) in [
                ((32.0, 16.0), (32.0, 6.0)),
                ((32.0, 48.0), (32.0, 58.0)),
                ((16.0, 32.0), (6.0, 32.0)),
                ((48.0, 32.0), (58.0, 32.0)),
            ] {
                d.head(a, b, nav * 0.8, d.ink);
            }
        }
        Icon::Fit => {
            for (x, y, sx, sy) in
                [(8.0, 8.0, 1.0, 1.0), (56.0, 8.0, -1.0, 1.0), (8.0, 56.0, 1.0, -1.0), (56.0, 56.0, -1.0, -1.0)]
            {
                d.line(&[(x, y + 12.0 * sy), (x, y), (x + 12.0 * sx, y)], nav * 0.85, d.ink);
            }
            d.block(32.0, 32.0, 8.0, 14.0, passive);
        }
        Icon::Close => {
            d.line(&[(16.0, 16.0), (48.0, 48.0)], nav, d.ink);
            d.line(&[(48.0, 16.0), (16.0, 48.0)], nav, d.ink);
        }
        Icon::First | Icon::Last => {
            let m = |x: f32| if icon == Icon::First { x } else { 64.0 - x };
            d.line(&[(m(16.0), 14.0), (m(16.0), 50.0)], nav, d.ink);
            d.shape(&[(m(48.0), 14.0), (m(24.0), 32.0), (m(48.0), 50.0)], d.ink, out);
        }
        Icon::Previous | Icon::Next => {
            let m = |x: f32| if icon == Icon::Previous { x } else { 64.0 - x };
            d.shape(&[(m(44.0), 14.0), (m(20.0), 32.0), (m(44.0), 50.0)], d.ink, out);
        }
        Icon::Eye | Icon::EyeOff => {
            let lid = |sign: f32| -> Vec<(f32, f32)> {
                (0..=16)
                    .map(|i| {
                        let t = i as f32 / 16.0;
                        (6.0 + 52.0 * t, 32.0 + sign * 17.0 * (std::f32::consts::PI * t).sin())
                    })
                    .collect()
            };
            d.line(&lid(-1.0), nav * 0.75, d.ink);
            d.line(&lid(1.0), nav * 0.75, d.ink);
            d.disc(32.0, 32.0, 8.0, if icon == Icon::Eye { d.a3 } else { d.p2 }, 0.0);
            if icon == Icon::EyeOff {
                d.line(&[(12.0, 54.0), (52.0, 10.0)], nav * 0.75, d.ink);
            }
        }
        Icon::BoxSelect => {
            d.dashed(&[(8.0, 10.0), (46.0, 10.0), (46.0, 40.0), (8.0, 40.0), (8.0, 10.0)], out * 1.4, d.ink);
            d.shape(
                &[(32.0, 26.0), (56.0, 42.0), (45.0, 45.0), (51.0, 58.0), (45.0, 60.0), (40.0, 48.0), (32.0, 54.0)],
                d.a2,
                out,
            );
        }
        Icon::Brush => {
            // The swath, then the brush: its handle, its ferrule, its tip in the accent.
            d.line(&[(6.0, 57.0), (12.0, 51.0), (19.0, 53.0), (24.0, 47.0)], act * 1.8, d.a1);
            d.line(&[(57.0, 7.0), (41.0, 23.0)], nav * 1.1, d.ink);
            d.shape(&[(37.0, 17.0), (47.0, 27.0), (39.0, 35.0), (29.0, 25.0)], d.p2, out);
            d.shape(&[(29.0, 25.0), (39.0, 35.0), (33.0, 45.0), (17.0, 50.0), (21.0, 35.0)], d.a2, out);
        }
        Icon::Through => {
            d.shape(&[(22.0, 8.0), (56.0, 8.0), (56.0, 38.0), (22.0, 38.0)], d.p3, out);
            d.shape(&[(8.0, 24.0), (42.0, 24.0), (42.0, 54.0), (8.0, 54.0)], d.p1, out);
            d.line(&[(4.0, 60.0), (60.0, 4.0)], act * 0.8, d.a3);
        }
        Icon::SelectNone => {
            d.dashed(&[(10.0, 10.0), (54.0, 10.0), (54.0, 54.0), (10.0, 54.0), (10.0, 10.0)], out * 1.4, d.ink);
            d.line(&[(22.0, 22.0), (42.0, 42.0)], act, d.ink);
            d.line(&[(42.0, 22.0), (22.0, 42.0)], act, d.ink);
        }
        Icon::Faces => {
            d.shape(&[(8.0, 8.0), (56.0, 8.0), (56.0, 56.0), (8.0, 56.0)], d.p1, out);
            d.shape(&[(24.0, 24.0), (40.0, 24.0), (40.0, 40.0), (24.0, 40.0)], d.a2, 0.0);
            for k in [24.0, 40.0] {
                d.line(&[(k, 8.0), (k, 56.0)], out, d.ink);
                d.line(&[(8.0, k), (56.0, k)], out, d.ink);
            }
        }
        // --- The case in 3D: isometric volumes in grey, the accent on what a command acts on.
        Icon::Region | Icon::Setup => {
            box_(passive);
            // Cells on its faces: a mesh.
            d.on_box(32.0, 33.0, 12.0, 24.0, out * 0.7);
            if icon == Icon::Setup {
                d.badge(48.0, 48.0, Badge::Gear);
            }
        }
        Icon::ViewIso => box_([d.a1, d.a2, d.a3]),
        Icon::ViewTop => box_([d.a1, d.p2, d.p3]),
        Icon::ViewFront => box_([d.p1, d.a2, d.p3]),
        Icon::ViewRight => box_([d.p1, d.p2, d.a3]),
        Icon::Patch => {
            box_(passive);
            d.block_face(32.0, 33.0, 12.0, 24.0, 2, d.a2);
            d.dot(41.0, 40.0, 3.4, d.a3);
        }
        Icon::Edges => {
            box_(passive);
            d.block_edges(32.0, 33.0, 12.0, 24.0, act * 0.8, d.a3);
        }
        Icon::MeshCheck => {
            box_(passive);
            d.on_box(32.0, 33.0, 12.0, 24.0, out * 0.7);
            d.badge(48.0, 48.0, Badge::Tick);
        }
        Icon::Scale => {
            d.block(20.0, 46.0, 6.0, 12.0, passive);
            d.dashed_block(36.0, 36.0, 14.0, 28.0, out);
            d.arrow((24.0, 36.0), (52.0, 10.0), act, d.a3);
        }
        Icon::Surface | Icon::SurfaceCheck | Icon::Repair | Icon::Import | Icon::Condition => {
            // A triangulated surface, lying flat (an STL), lower in the icon for the cue above.
            let low = if icon == Icon::Surface { 0.0 } else { 10.0 };
            let h = if icon == Icon::Surface { 18.0 } else { 17.0 };
            d.sheet(32.0, 32.0 + low, h, out);
            match icon {
                Icon::SurfaceCheck => d.badge(48.0, 48.0, Badge::Tick),
                Icon::Repair => d.sheet_cell(32.0, 32.0 + low, h, d.a2, out),
                Icon::Import => d.arrow((32.0, 4.0), (32.0, 34.0), act, d.a3),
                Icon::Condition => {
                    d.sheet_cell(32.0, 32.0 + low, h, d.a1, out);
                    d.arrow((20.0, 4.0), (30.0, 34.0), act, d.a3);
                    d.arrow((40.0, 6.0), (38.0, 32.0), act * 0.75, d.a2);
                }
                _ => {}
            }
        }
        Icon::Grid => {
            d.sheet_grid(32.0, 34.0, 18.0, out);
        }
        Icon::Origin => {
            let o = (22.0, 44.0);
            d.shape(&[o, (42.0, 44.0), (42.0, 24.0), (22.0, 24.0)], d.origin_plane, out * 0.8);
            d.arrow(o, (58.0, 44.0), act * 0.8, d.axes[0]);
            d.arrow(o, (22.0, 6.0), act * 0.8, d.axes[2]);
            d.arrow(o, (8.0, 56.0), act * 0.8, d.axes[1]);
            d.dot(22.0, 44.0, 4.0, d.ink);
        }
    }
}

/// A small mark at a corner of an icon: what a command checks or sets up.
#[derive(Clone, Copy)]
enum Badge {
    Tick,
    Gear,
}

/// What an icon is drawn with: its square on the screen, its colours by role, its strokes.
struct Draw<'a> {
    p: &'a Painter,
    /// The top left corner and the size of a 64th of the icon, in points.
    o: Pos2,
    u: f32,
    /// Object outlines, actions and cues, navigation line art (points).
    out: f32,
    act: f32,
    nav: f32,
    /// The roles: ink (outlines, line art), three greys (top, left and right faces), a guide, the
    /// accent's pale, middle and strong shades, the app's accent itself.
    ink: Color32,
    p1: Color32,
    p2: Color32,
    p3: Color32,
    guide: Color32,
    a1: Color32,
    a2: Color32,
    a3: Color32,
    accent: Color32,
    dark_view: Color32,
    origin_plane: Color32,
    axes: [Color32; 3],
    /// The background the badges cut out of.
    gap: Color32,
    /// The badges' marks, faded with the ink.
    white: Color32,
    /// Drawn small (under 26 points): less detail.
    small: bool,
}

impl<'a> Draw<'a> {
    fn new(p: &'a Painter, rect: Rect, ink: Ink) -> Draw<'a> {
        let s = rect.width().min(rect.height());
        let o = rect.center() - eframe::egui::vec2(s * 0.5, s * 0.5);
        let u = s / 64.0;
        // A light ink is a dark theme; a faded ink (a command that can't run) fades it all. The
        // colours are stored multiplied by their alpha: the brightness is the one before.
        let [r, g, b, _] = ink.line.to_srgba_unmultiplied();
        let dark = u32::from(r) + u32::from(g) + u32::from(b) > 3 * 128;
        let fade = f32::from(ink.line.a()) / 255.0;
        let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]).gamma_multiply(fade);
        let rgbf = |c: [f32; 3]| rgb(c.map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8));
        let shades = if dark { crate::theme::Tokens::dark() } else { crate::theme::Tokens::light() }.icon;
        let pick = |c: Color32| c.gamma_multiply(fade);
        let accent = ink.fill;
        let mix = |a: Color32, b: Color32, t: f32| {
            let m = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
            Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
        };
        let paper = pick(shades.paper);
        Draw {
            p,
            o,
            u,
            out: (2.6 * u).max(1.0),
            act: (4.2 * u).max(1.2),
            nav: (5.2 * u).max(1.3),
            ink: ink.line,
            p1: pick(shades.light),
            p2: pick(shades.mid),
            p3: pick(shades.deep),
            guide: pick(shades.guide),
            a1: mix(accent, paper, if dark { 0.45 } else { 0.68 }),
            a2: accent,
            a3: mix(accent, (if dark { Color32::WHITE } else { Color32::BLACK }).gamma_multiply(fade), 0.22),
            accent,
            // The 3D view's own colours: its dark background, the origin's planes and axes.
            dark_view: rgbf(umcf_render::raster::TOP),
            origin_plane: rgb(umcf_render::guides::PLANE.0),
            axes: umcf_render::guides::AXES.map(rgb),
            gap: paper,
            white: rgb([255, 255, 255]),
            small: s < 26.0,
        }
    }

    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.o.x + x * self.u, self.o.y + y * self.u)
    }

    /// A polyline `width` points wide with round caps and joins.
    fn line(&self, pts: &[(f32, f32)], width: f32, c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.p.add(Shape::line(v.clone(), Stroke::new(width, c)));
        for q in v {
            self.p.circle_filled(q, width * 0.5, c);
        }
    }

    /// A polyline drawn in dashes (two parts dash, one part gap).
    fn dashed(&self, pts: &[(f32, f32)], width: f32, c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.p.extend(Shape::dashed_line(&v, Stroke::new(width, c), 5.0 * self.u, 3.5 * self.u));
    }

    /// A closed shape filled with `fill`, outlined `width` wide in the ink (none for 0).
    fn shape(&self, pts: &[(f32, f32)], fill: Color32, width: f32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.at(*x, *y)).collect();
        // Not every shape here is convex: drawn as a fan of triangles from the first corner, so
        // each shape starts at a corner from which it is star-shaped (every edge seen from
        // inside; for the folder and the camera, the bottom-left one).
        if v.len() >= 3 {
            let mut mesh = eframe::egui::Mesh::default();
            for q in &v {
                mesh.colored_vertex(*q, fill);
            }
            for i in 1..v.len() as u32 - 1 {
                mesh.add_triangle(0, i, i + 1);
            }
            self.p.add(Shape::mesh(mesh));
        }
        if width > 0.0 {
            let mut closed: Vec<(f32, f32)> = pts.to_vec();
            if let Some(first) = pts.first() {
                closed.push(*first);
            }
            self.line(&closed, width, self.ink);
        }
    }

    fn circle(&self, x: f32, y: f32, r: f32, width: f32, c: Color32) {
        self.p.circle_stroke(self.at(x, y), r * self.u, Stroke::new(width, c));
    }

    fn disc(&self, x: f32, y: f32, r: f32, fill: Color32, width: f32) {
        self.p.circle(
            self.at(x, y),
            r * self.u,
            fill,
            if width > 0.0 { Stroke::new(width, self.ink) } else { Stroke::NONE },
        );
    }

    fn dot(&self, x: f32, y: f32, r: f32, c: Color32) {
        self.p.circle_filled(self.at(x, y), r * self.u, c);
    }

    /// The right half of a disc.
    fn half_disc(&self, x: f32, y: f32, r: f32, c: Color32) {
        let pts: Vec<(f32, f32)> = (0..=16)
            .map(|i| {
                let a = (-90.0 + 180.0 * i as f32 / 16.0f32).to_radians();
                (x + r * a.cos(), y + r * a.sin())
            })
            .collect();
        self.shape(&pts, c, 0.0);
    }

    /// An arrowhead at `tip`, pointing from `from`.
    fn head(&self, from: (f32, f32), tip: (f32, f32), width: f32, c: Color32) {
        let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
        let l = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (ux, uy) = (dx / l, dy / l);
        let k = 9.0;
        let a = (tip.0 - ux * k - uy * k * 0.75, tip.1 - uy * k + ux * k * 0.75);
        let b = (tip.0 - ux * k + uy * k * 0.75, tip.1 - uy * k - ux * k * 0.75);
        self.line(&[a, tip, b], width, c);
    }

    /// An arrow from `a` to `b`.
    fn arrow(&self, a: (f32, f32), b: (f32, f32), width: f32, c: Color32) {
        self.line(&[a, b], width, c);
        self.head(a, b, width, c);
    }

    /// Where the point `(x, y, z)` of an isometric box lies on the icon, the box's footprint
    /// centred at `(cx, cy)`: x goes to the right and down, y to the left and down, z up.
    fn iso(cx: f32, cy: f32, x: f32, y: f32, z: f32) -> (f32, f32) {
        (cx + (x - y) * 0.866, cy + (x + y) * 0.5 - z)
    }

    /// The faces of a box of half side `h` and height `t` (top, screen left, screen right).
    fn faces(cx: f32, cy: f32, h: f32, t: f32) -> [[(f32, f32); 4]; 3] {
        let z0 = -t * 0.5;
        let z1 = t * 0.5;
        let p = |x: f32, y: f32, z: f32| Draw::iso(cx, cy, x, y, z);
        [
            [p(-h, -h, z1), p(h, -h, z1), p(h, h, z1), p(-h, h, z1)],
            [p(-h, h, z0), p(h, h, z0), p(h, h, z1), p(-h, h, z1)],
            [p(h, -h, z0), p(h, h, z0), p(h, h, z1), p(h, -h, z1)],
        ]
    }

    fn block(&self, cx: f32, cy: f32, h: f32, t: f32, fills: [Color32; 3]) {
        for (f, c) in Draw::faces(cx, cy, h, t).iter().zip(fills) {
            self.shape(f, c, self.out);
        }
    }

    fn block_face(&self, cx: f32, cy: f32, h: f32, t: f32, face: usize, fill: Color32) {
        if let Some(f) = Draw::faces(cx, cy, h, t).get(face) {
            self.shape(f, fill, self.out);
        }
    }

    /// The box's visible edges, drawn over it.
    fn block_edges(&self, cx: f32, cy: f32, h: f32, t: f32, width: f32, c: Color32) {
        for f in Draw::faces(cx, cy, h, t) {
            let mut v = f.to_vec();
            v.push(f[0]);
            self.line(&v, width, c);
        }
    }

    /// A box's edges only, in the ink (line art).
    fn block_lines(&self, cx: f32, cy: f32, h: f32, t: f32, width: f32) {
        for f in Draw::faces(cx, cy, h, t) {
            let mut v = f.to_vec();
            v.push(f[0]);
            self.line(&v, width, self.ink);
        }
    }

    fn dashed_block(&self, cx: f32, cy: f32, h: f32, t: f32, width: f32) {
        for f in Draw::faces(cx, cy, h, t) {
            let mut v = f.to_vec();
            v.push(f[0]);
            self.dashed(&v, width, self.guide);
        }
    }

    /// Cells on the box's three faces: a mesh.
    fn on_box(&self, cx: f32, cy: f32, h: f32, t: f32, width: f32) {
        let p = |x: f32, y: f32, z: f32| Draw::iso(cx, cy, x, y, z);
        let (z0, z1) = (-t * 0.5, t * 0.5);
        let ks: &[f32] = if self.small { &[0.0] } else { &[-h / 3.0, h / 3.0] };
        for &k in ks {
            self.line(&[p(k, -h, z1), p(k, h, z1)], width, self.guide);
            self.line(&[p(-h, k, z1), p(h, k, z1)], width, self.guide);
            self.line(&[p(k, h, z0), p(k, h, z1)], width, self.guide);
            self.line(&[p(h, k, z0), p(h, k, z1)], width, self.guide);
        }
        let zs: Vec<f32> = if self.small { vec![0.0] } else { vec![z0 + t / 3.0, z0 + 2.0 * t / 3.0] };
        for z in zs {
            self.line(&[p(-h, h, z), p(h, h, z)], width, self.guide);
            self.line(&[p(h, -h, z), p(h, h, z)], width, self.guide);
        }
    }

    /// A flat triangulated sheet (an STL surface) of half side `h` at `(cx, cy)`.
    fn sheet(&self, cx: f32, cy: f32, h: f32, width: f32) {
        let p = |x: f32, y: f32| Draw::iso(cx, cy, x, y, 0.0);
        self.shape(&[p(-h, -h), p(h, -h), p(h, h), p(-h, h)], self.p1, 0.0);
        let n = if self.small { 2 } else { 3 };
        let step = 2.0 * h / n as f32;
        for i in 0..=n {
            let k = -h + step * i as f32;
            self.line(&[p(k, -h), p(k, h)], width * 0.8, self.guide);
            self.line(&[p(-h, k), p(h, k)], width * 0.8, self.guide);
        }
        for i in 0..n {
            for j in 0..n {
                let (x, y) = (-h + step * i as f32, -h + step * j as f32);
                if !self.small {
                    self.line(&[p(x, y), p(x + step, y + step)], width * 0.6, self.guide);
                }
            }
        }
        self.shape(&[p(-h, -h), p(h, -h), p(h, h), p(-h, h)], Color32::TRANSPARENT, width);
    }

    /// One cell (two triangles) of [`Draw::sheet`] filled with `fill`: the middle one.
    fn sheet_cell(&self, cx: f32, cy: f32, h: f32, fill: Color32, width: f32) {
        let p = |x: f32, y: f32| Draw::iso(cx, cy, x, y, 0.0);
        let s = h / 3.0;
        self.shape(&[p(-s, -s), p(s, -s), p(s, s), p(-s, s)], fill, width);
    }

    /// A ground grid in perspective-free isometry, its middle lines stronger.
    fn sheet_grid(&self, cx: f32, cy: f32, h: f32, width: f32) {
        let p = |x: f32, y: f32| Draw::iso(cx, cy, x, y, 0.0);
        for i in 0..=4 {
            let k = -h + h * 0.5 * i as f32;
            let (w, c) = if i == 2 { (width * 1.3, self.ink) } else { (width * 0.9, self.guide) };
            self.line(&[p(k, -h), p(k, h)], w, c);
            self.line(&[p(-h, k), p(h, k)], w, c);
        }
    }

    /// A badge bottom right, cut out of what is under it.
    fn badge(&self, x: f32, y: f32, b: Badge) {
        self.dot(x, y, 14.0, self.gap);
        self.dot(x, y, 11.0, self.a2);
        let white = self.white;
        match b {
            Badge::Tick => {
                self.line(&[(x - 5.5, y + 0.5), (x - 1.5, y + 4.5), (x + 5.5, y - 4.0)], self.act * 0.75, white)
            }
            Badge::Gear => {
                for k in 0..6 {
                    let a = (k as f32 * 60.0f32).to_radians();
                    self.line(
                        &[(x + 4.0 * a.cos(), y + 4.0 * a.sin()), (x + 8.0 * a.cos(), y + 8.0 * a.sin())],
                        self.act * 0.6,
                        white,
                    );
                }
                self.dot(x, y, 4.5, white);
                self.dot(x, y, 2.0, self.a2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tokens;
    use eframe::egui;

    /// Writes every icon, on the light and the dark theme's card, at the ribbon's size and a
    /// small one, to the PNG named by `UMCF_ICON_SHEET`, else `umcf-icon-sheet.png` in the
    /// system's temporary folder (`cargo test -p umcf-ui-egui --lib
    /// icon_sheet -- --ignored`): the contact sheet the icons are reviewed on.
    #[test]
    #[ignore = "writes a contact sheet for review; needs a GPU device (or lavapipe)"]
    fn icon_sheet() {
        let path = std::env::var("UMCF_ICON_SHEET")
            .unwrap_or_else(|_| std::env::temp_dir().join("umcf-icon-sheet.png").to_string_lossy().into_owned());
        const COLUMNS: usize = 10;
        let rows = Icon::ALL.len().div_ceil(COLUMNS);
        let cell = egui::vec2(96.0, 92.0);
        let size = egui::vec2(cell.x * COLUMNS as f32, cell.y * rows as f32 * 2.0);
        let mut harness = egui_kittest::Harness::builder().with_size(size).with_pixels_per_point(1.5).build_ui(|ui| {
            ui.ctx().set_fonts(crate::theme::fonts());
            {
                let p = ui.painter();
                for (half, t) in [Tokens::light(), Tokens::dark()].iter().enumerate() {
                    let top = half as f32 * cell.y * rows as f32;
                    p.rect_filled(
                        Rect::from_min_size(pos2(0.0, top), egui::vec2(size.x, cell.y * rows as f32)),
                        0.0,
                        t.card,
                    );
                    let ink = Ink { line: t.text, fill: t.accent };
                    for (k, icon) in Icon::ALL.iter().enumerate() {
                        let x = (k % COLUMNS) as f32 * cell.x;
                        let y = top + (k / COLUMNS) as f32 * cell.y;
                        paint(p, Rect::from_min_size(pos2(x + 8.0, y + 6.0), egui::vec2(40.0, 40.0)), *icon, ink);
                        paint(p, Rect::from_min_size(pos2(x + 58.0, y + 18.0), egui::vec2(20.0, 20.0)), *icon, ink);
                        p.text(
                            pos2(x + cell.x / 2.0, y + 60.0),
                            egui::Align2::CENTER_TOP,
                            format!("{icon:?}"),
                            egui::FontId::proportional(11.0),
                            t.text_muted,
                        );
                    }
                }
            }
        });
        harness.run();
        let image = harness.render().expect("the sheet renders");
        image.save(&path).expect("the sheet is written");
    }
}
