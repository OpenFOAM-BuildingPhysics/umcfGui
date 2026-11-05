//! Theme tokens: every colour and size the interface uses, light and dark. Panels take their
//! colours from [`Tokens`], never from literals.
//!
//! The look (a dark application bar, floating cards over a full-window 3D view) follows
//! SolidCraft's (MIT OR Apache-2.0, Copyright (c) 2026 SolidCraft contributors); the values here
//! are umcfGui's own.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle,
};

/// The colours and sizes of one theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// The application bar along the top.
    pub bar: Color32,
    pub bar_text: Color32,
    pub bar_muted: Color32,
    /// The bar's controls when hovered, and the open case's tab.
    pub bar_raised: Color32,
    /// The icons' coloured parts on the bar.
    pub bar_accent: Color32,
    /// Behind the ribbon cards.
    pub chrome: Color32,
    /// Cards: ribbon, browser, properties, navigation bar, history.
    pub card: Color32,
    pub card_border: Color32,
    /// Rows and buttons when hovered.
    pub hover: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    /// Icons' coloured parts, the current tab, primary buttons.
    pub accent: Color32,
    /// A soft accent: the current tab's and selected rows' background.
    pub accent_soft: Color32,
    /// The selection in the 3D view (orange, as the renderer draws it).
    pub selection: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub axis: [Color32; 3],
    pub spacing: f32,
    /// Small controls; cards.
    pub radius: u8,
    pub radius_card: u8,
    /// Body text size, points.
    pub text_size: f32,
    pub bar_height: f32,
    pub ribbon_height: f32,
    /// Gap between the window's edge and the floating cards, and between cards.
    pub gap: f32,
}

fn rgb(c: [f32; 3]) -> Color32 {
    let b = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

impl Tokens {
    pub fn light() -> Tokens {
        Tokens {
            dark: false,
            bar: Color32::from_rgb(0x2a, 0x30, 0x3a),
            bar_text: Color32::from_rgb(0xf2, 0xf4, 0xf7),
            bar_muted: Color32::from_rgb(0xa9, 0xb1, 0xbd),
            bar_raised: Color32::from_rgb(0x3b, 0x43, 0x50),
            bar_accent: Color32::from_rgb(0x6a, 0xa8, 0xf0),
            chrome: Color32::from_rgb(0xe9, 0xec, 0xf0),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            card_border: Color32::from_rgb(0xd9, 0xdd, 0xe3),
            hover: Color32::from_rgb(0xee, 0xf1, 0xf5),
            text: Color32::from_rgb(0x1f, 0x25, 0x2e),
            text_muted: Color32::from_rgb(0x66, 0x6f, 0x7c),
            accent: Color32::from_rgb(0x2b, 0x6c, 0xb8),
            accent_soft: Color32::from_rgb(0xe2, 0xed, 0xf9),
            selection: rgb(umcf_render::raster::SELECTED),
            error: Color32::from_rgb(0xc0, 0x2b, 0x2b),
            warning: Color32::from_rgb(0xa8, 0x63, 0x00),
            info: Color32::from_rgb(0x3d, 0x6c, 0x9e),
            axis: [
                Color32::from_rgb(0xd9, 0x3b, 0x3b),
                Color32::from_rgb(0x3b, 0x9b, 0x4a),
                Color32::from_rgb(0x2f, 0x6c, 0xd6),
            ],
            spacing: 6.0,
            radius: 5,
            radius_card: 9,
            text_size: 13.5,
            bar_height: 34.0,
            ribbon_height: 122.0,
            gap: 7.0,
        }
    }

    pub fn dark() -> Tokens {
        Tokens {
            dark: true,
            bar: Color32::from_rgb(0x17, 0x1a, 0x1f),
            bar_text: Color32::from_rgb(0xe8, 0xeb, 0xef),
            bar_muted: Color32::from_rgb(0x95, 0x9d, 0xa9),
            bar_raised: Color32::from_rgb(0x2c, 0x32, 0x3b),
            chrome: Color32::from_rgb(0x1d, 0x21, 0x27),
            card: Color32::from_rgb(0x27, 0x2c, 0x34),
            card_border: Color32::from_rgb(0x3a, 0x41, 0x4b),
            hover: Color32::from_rgb(0x32, 0x38, 0x42),
            text: Color32::from_rgb(0xe3, 0xe6, 0xea),
            text_muted: Color32::from_rgb(0x9a, 0xa2, 0xae),
            accent: Color32::from_rgb(0x5c, 0x9d, 0xe8),
            accent_soft: Color32::from_rgb(0x2a, 0x3a, 0x50),
            selection: rgb(umcf_render::raster::SELECTED),
            error: Color32::from_rgb(0xf0, 0x6a, 0x6a),
            warning: Color32::from_rgb(0xe8, 0xa3, 0x3d),
            info: Color32::from_rgb(0x7c, 0xaf, 0xe0),
            axis: [
                Color32::from_rgb(0xf0, 0x5d, 0x5d),
                Color32::from_rgb(0x5c, 0xc0, 0x6c),
                Color32::from_rgb(0x5c, 0x94, 0xf0),
            ],
            ..Tokens::light()
        }
    }

    /// The 3D view's background, top and bottom, as the renderer takes it.
    pub fn view_background(&self) -> [[f32; 3]; 2] {
        let f = |c: Color32| [c.r(), c.g(), c.b()].map(|x| f32::from(x) / 255.0);
        if self.dark {
            [f(Color32::from_rgb(0x34, 0x39, 0x42)), f(Color32::from_rgb(0x1e, 0x21, 0x26))]
        } else {
            [f(Color32::from_rgb(0xf3, 0xf5, 0xf8)), f(Color32::from_rgb(0xd2, 0xd8, 0xe0))]
        }
    }

    /// A floating card's frame.
    pub fn card_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.card)
            .stroke(Stroke::new(1.0, self.card_border))
            .corner_radius(self.radius_card)
            .inner_margin(egui::Margin::same(8))
            .shadow(Shadow {
                offset: [0, 2],
                blur: 10,
                spread: 0,
                color: Color32::from_black_alpha(if self.dark { 90 } else { 28 }),
            })
    }

    /// Applies the tokens to egui's style.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.panel_fill = self.chrome;
        visuals.window_fill = self.card;
        visuals.extreme_bg_color = if self.dark { self.chrome } else { Color32::from_rgb(0xf6, 0xf7, 0xf9) };
        visuals.faint_bg_color = self.hover;
        visuals.override_text_color = Some(self.text);
        visuals.hyperlink_color = self.accent;
        visuals.selection.bg_fill = self.accent_soft;
        visuals.selection.stroke = Stroke::new(1.0, self.accent);
        visuals.window_stroke = Stroke::new(1.0, self.card_border);
        visuals.window_corner_radius = CornerRadius::same(self.radius_card);
        visuals.menu_corner_radius = CornerRadius::same(self.radius_card);
        visuals.error_fg_color = self.error;
        visuals.warn_fg_color = self.warning;
        let r = CornerRadius::same(self.radius);
        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.corner_radius = r;
        }
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.card_border);
        visuals.widgets.noninteractive.bg_fill = self.card;
        visuals.widgets.inactive.bg_fill = self.hover;
        visuals.widgets.inactive.weak_bg_fill = self.hover;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.card_border);
        visuals.widgets.hovered.weak_bg_fill = self.accent_soft;
        visuals.widgets.hovered.bg_fill = self.accent_soft;
        ctx.set_visuals(visuals);
        ctx.global_style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(self.spacing, self.spacing * 0.75);
            style.spacing.button_padding = egui::vec2(self.spacing * 1.4, self.spacing * 0.6);
            style.spacing.interact_size.y = 24.0;
            style.text_styles = [
                (TextStyle::Small, FontId::new(self.text_size * 0.85, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(self.text_size, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(self.text_size, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(self.text_size * 1.1, FontFamily::Name(STRONG.into()))),
                (TextStyle::Monospace, FontId::new(self.text_size * 0.9, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

/// The semi-bold family: headings, the current tab, names.
pub const STRONG: &str = "strong";

/// A font of the semi-bold family.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// The fonts: Inter for text, Inter SemiBold for headings, JetBrains Mono for parameters and
/// results (all SIL Open Font License; see ATTRIBUTION.md).
pub fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::empty();
    let add = |f: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        f.font_data.insert(name.to_string(), std::sync::Arc::new(FontData::from_static(bytes)));
    };
    add(&mut f, "inter", include_bytes!("../../../assets/fonts/Inter-Regular.ttf"));
    add(&mut f, "inter-semibold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut f, "mono", include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"));
    f.families.insert(FontFamily::Proportional, vec!["inter".into(), "mono".into()]);
    f.families.insert(FontFamily::Monospace, vec!["mono".into(), "inter".into()]);
    f.families.insert(FontFamily::Name(STRONG.into()), vec!["inter-semibold".into(), "inter".into()]);
    f
}
