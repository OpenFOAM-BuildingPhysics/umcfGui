//! Ribbon groups and tabs that give way on a narrow window, the way a word processor's ribbon
//! does: groups give up their buttons' labels (compact), then fold into a single drop-down
//! button that opens the whole group (collapsed), starting from the right; whatever room that
//! frees is given back from the left. The ribbon still scrolls sideways if even fully collapsed
//! groups don't fit (‹ › at its edges, the mouse wheel or a drag). Tabs that don't fit go into a
//! » menu, from the end; the chosen tab stays on the strip.
//!
//! Ported from WordCraft (<https://github.com/storytold/wordcraft>, MIT or Apache-2.0, at
//! commit ad5e1a6): `plan` from `crates/ui-egui/src/ribbon_fit.rs`; `fit_tabs` from
//! `crates/ui-egui/src/ribbon/tabs.rs`; the tabs' » menu ([`more_tabs`]), the ribbon row that
//! scrolls with no bar and takes a pending offset ([`row`], [`take_offset`]) and its edge chevrons
//! ([`edge_chevrons`]) from `crates/ui-egui/src/ribbon.rs` `ribbon`, with `edge_cue` from
//! `crates/ui-egui/src/ribbon/common.rs`. `App::collapsed_group` in `lib.rs` follows WordCraft's
//! `collapsed_group` (`crates/ui-egui/src/widgets/groups.rs`).
//! Copyright (c) 2026 ArtCraft Team and the WordCraft contributors, under the MIT licence
//! (`assets/licenses/MIT-WordCraft.txt`; `ATTRIBUTION.md`).
//!
//! Changed here: a group's width at each level is computed from its labels (umcfGui's groups are
//! plain rows of buttons) rather than measured on earlier frames, so the plan needs no state; a
//! compact group's buttons drop their labels and keep their icons (umcfGui has only large
//! buttons); a level no narrower than the group's present one is skipped (a one-button group
//! stays compact rather than collapsing wider); the chevrons take their colours as arguments.

use eframe::egui;

/// The group as designed: each button its icon over its label.
pub const FULL: u8 = 0;
/// Each button its icon only (the tooltip keeps the name).
pub const COMPACT: u8 = 1;
/// One drop-down button holding the whole group.
pub const COLLAPSED: u8 = 2;

/// Each group's level (by position) so that the row fits `avail`: groups shrink from the right,
/// first all to compact, then to collapsed, stopping as soon as the row fits. Then whatever room
/// is left is given back, from the left. `groups` holds each group's width at the three levels.
pub fn plan(groups: &[[f32; 3]], avail: f32) -> Vec<u8> {
    let width = |g: &[f32; 3], level: u8| -> f32 { g.get(usize::from(level)).copied().unwrap_or(0.0) };
    let mut levels = vec![FULL; groups.len()];
    let mut total: f32 = groups.iter().map(|g| width(g, FULL)).sum();
    for target in [COMPACT, COLLAPSED] {
        for i in (0..groups.len()).rev() {
            if total <= avail {
                break;
            }
            let (Some(g), Some(level)) = (groups.get(i), levels.get_mut(i)) else { continue };
            if width(g, target) >= width(g, *level) {
                continue;
            }
            total += width(g, target) - width(g, *level);
            *level = target;
        }
    }
    // Shrinking a wide group can make room the groups shrunk before it no longer need, and a
    // group may gain nothing collapsed: give back whatever still fits.
    for (g, level) in groups.iter().zip(levels.iter_mut()) {
        while *level > FULL {
            let wider = *level - 1;
            let next = total + width(g, wider) - width(g, *level);
            if !next.is_finite() || next > avail {
                break;
            }
            total = next;
            *level = wider;
        }
    }
    levels
}

/// Which tabs stay on the strip in `avail` points: `tabs` holds each tab's width and whether it
/// must stay (the chosen one). The others leave from the end until the rest and the » button
/// (`more` wide) fit.
pub fn fit_tabs(tabs: &[(f32, bool)], avail: f32, more: f32) -> Vec<bool> {
    let mut fits = vec![true; tabs.len()];
    let mut total: f32 = tabs.iter().map(|(w, _)| w).sum();
    if total <= avail {
        return fits;
    }
    for (fit, (w, keep)) in fits.iter_mut().zip(tabs).rev() {
        if total + more <= avail {
            break;
        }
        if !keep {
            *fit = false;
            total -= w;
        }
    }
    fits
}

/// How far a chevron at the ribbon's edge scrolls it.
const SCROLL_STEP: f32 = 160.0;
/// A chevron's width.
const CHEVRON_W: f32 = 18.0;

/// The chevrons' colours: what they cover, and their glyph's (hovered and not).
#[derive(Clone, Copy)]
pub struct Ink {
    pub back: egui::Color32,
    pub text: egui::Color32,
    pub dim: egui::Color32,
}

/// The tabs that don't fit (`left_out`: index and name) behind a » button `w` wide; the one
/// picked in its menu, if any.
pub fn more_tabs(
    ui: &mut egui::Ui,
    left_out: &[(usize, &str)],
    chosen: usize,
    w: f32,
    radius: impl Into<egui::CornerRadius>,
    ink: Ink,
) -> Option<usize> {
    if left_out.is_empty() {
        return None;
    }
    // A fixed id: the button moves when the tabs come back as the search closes, and a press on
    // it must still complete its click.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 25.0), egui::Sense::hover());
    let r = ui.interact(rect, egui::Id::unique("ribbon-more-tabs"), egui::Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, radius, ink.back);
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "»", egui::FontId::proportional(14.0), ink.text);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "More tabs"));
    let r = r.on_hover_text("More tabs");
    let mut picked = None;
    egui::Popup::menu(&r).show(|ui| {
        for &(k, name) in left_out {
            if ui.selectable_label(chosen == k, name).clicked() {
                picked = Some(k);
                ui.close();
            }
        }
    });
    picked
}

/// The ribbon's row of groups (`add`), in a sideways scroll area with no bar: the mouse wheel
/// (plain, as the row scrolls one way only) or a drag scrolls it, and an offset a chevron left
/// under `pending` is applied. Draw [`edge_chevrons`] with its output.
pub fn row<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    pending: egui::Id,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let offset = take_offset(ui, pending);
    ui.scope(|ui| {
        ui.style_mut().always_scroll_the_only_direction = true;
        let mut area = egui::ScrollArea::horizontal()
            .id_salt(salt)
            .auto_shrink([false, true])
            // A mouse drag scrolls it too, not only a touch.
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Always,
                ..Default::default()
            })
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if let Some(x) = offset {
            area = area.horizontal_scroll_offset(x);
        }
        area.show(ui, add)
    })
    .inner
}

/// The offset a chevron's click left for this frame's scroll area (egui keeps a scroll area's
/// offset private: the next frame builds it at the one asked for).
pub fn take_offset(ui: &egui::Ui, id: egui::Id) -> Option<f32> {
    ui.data_mut(|d| d.remove_temp::<f32>(id))
}

/// The bar is hidden, so a row wider than the ribbon would be clipped with no sign: chevrons on
/// its edges show where there is more and scroll it on a click (the offset is left under `id`
/// for [`take_offset`]).
pub fn edge_chevrons<R>(ui: &mut egui::Ui, scroll: &egui::scroll_area::ScrollAreaOutput<R>, id: egui::Id, ink: Ink) {
    let r = scroll.inner_rect;
    let overflow = scroll.content_size.x - r.width();
    let offset = scroll.state.clamped_offset().x;
    let mut next = None;
    if offset > 1.0 {
        let left = egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + CHEVRON_W, r.bottom()));
        if edge_cue(ui, left, "‹", "Scroll ribbon left", ink) {
            next = Some(offset - SCROLL_STEP);
        }
    }
    if overflow - offset > 1.0 {
        let right = egui::Rect::from_min_max(egui::pos2(r.right() - CHEVRON_W, r.top()), r.right_bottom());
        if edge_cue(ui, right, "›", "Scroll ribbon right", ink) {
            next = Some(offset + SCROLL_STEP);
        }
    }
    if let Some(x) = next {
        let x = x.clamp(0.0, overflow.max(0.0));
        ui.data_mut(|d| d.insert_temp(id, x));
        ui.ctx().request_repaint();
    }
}

/// A chevron over a ribbon edge that is also a button; `true` on the frame it is clicked.
fn edge_cue(ui: &mut egui::Ui, rect: egui::Rect, glyph: &str, hint: &str, ink: Ink) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with(glyph), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, hint));
    let resp = resp.on_hover_text(hint);
    let colour = if resp.hovered() { ink.text } else { ink.dim };
    ui.painter().rect_filled(rect, 0.0, ink.back);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(18.0), colour);
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Groups 100 wide as designed, 60 compact, 40 collapsed.
    fn same(n: usize) -> Vec<[f32; 3]> {
        vec![[100.0, 60.0, 40.0]; n]
    }

    #[test]
    fn groups_shrink_from_the_right_compact_before_collapsed() {
        let g = same(3);
        assert_eq!(plan(&g, 300.0), [FULL, FULL, FULL]);
        assert_eq!(plan(&g, 299.0), [FULL, FULL, COMPACT]);
        assert_eq!(plan(&g, 220.0), [FULL, COMPACT, COMPACT]);
        // All compact before any collapses.
        assert_eq!(plan(&g, 180.0), [COMPACT, COMPACT, COMPACT]);
        assert_eq!(plan(&g, 179.0), [COMPACT, COMPACT, COLLAPSED]);
        assert_eq!(plan(&g, 120.0), [COLLAPSED, COLLAPSED, COLLAPSED]);
        // Narrower still: all collapsed (the ribbon scrolls).
        assert_eq!(plan(&g, 10.0), [COLLAPSED; 3]);
        assert!(plan(&[], 10.0).is_empty());
    }

    #[test]
    fn room_freed_by_a_wide_group_goes_back_from_the_left() {
        // A wide group on the right: collapsing it frees enough for the others to stay whole.
        let g = vec![[100.0, 60.0, 40.0], [100.0, 60.0, 40.0], [400.0, 380.0, 40.0]];
        assert_eq!(plan(&g, 300.0), [FULL, FULL, COLLAPSED]);
        // A group no narrower compact keeps its labels.
        let g = vec![[50.0, 50.0, 40.0], [100.0, 60.0, 40.0]];
        assert_eq!(plan(&g, 110.0), [FULL, COMPACT]);
    }

    /// At every width from fully collapsed up, the row fits.
    #[test]
    fn every_width_from_fully_collapsed_up_fits() {
        let g: Vec<[f32; 3]> = vec![[120.0, 80.0, 60.0], [90.0, 50.0, 60.0], [300.0, 200.0, 60.0], [70.0, 50.0, 60.0]];
        // Fully collapsed is what it falls back to: from there on the row always fits.
        let collapsed: f32 = g.iter().map(|w| w[2]).sum();
        for room in (0..=700).map(|r| r as f32) {
            let levels = plan(&g, room);
            let used: f32 = g.iter().zip(&levels).map(|(w, &l)| w[usize::from(l)]).sum();
            if room >= collapsed {
                assert!(used <= room + 1e-3, "{room}: {used}");
            }
        }
    }

    #[test]
    fn a_level_no_narrower_is_skipped() {
        // One button: compact (an icon and the name under it) is narrower than collapsed.
        let g = vec![[100.0, 43.0, 56.0]];
        assert_eq!(plan(&g, 50.0), [COMPACT]);
        assert_eq!(plan(&g, 10.0), [COMPACT]);
    }

    #[test]
    fn tabs_leave_from_the_end_and_the_chosen_one_stays() {
        let tabs = [(50.0, false), (60.0, false), (70.0, true), (80.0, false)];
        assert_eq!(fit_tabs(&tabs, 260.0, 20.0), [true; 4]);
        assert_eq!(fit_tabs(&tabs, 259.0, 20.0), [true, true, true, false]);
        // The chosen tab stays though the one before it leaves.
        assert_eq!(fit_tabs(&tabs, 150.0, 20.0), [true, false, true, false]);
        assert_eq!(fit_tabs(&tabs, 0.0, 20.0), [false, false, true, false]);
    }
}
