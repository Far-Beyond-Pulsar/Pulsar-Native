//! A radial (pie) quick-action menu.
//!
//! Pure presentation and selection: a ring of wedges around a centre point,
//! one per item, painted with tessellated paths. What opens it, which items it
//! shows and what runs on confirm belong to the host (see `ui_core`'s
//! hold-Tab menu); the host feeds input in through [`RadialMenu::select_next`],
//! [`RadialMenu::select_prev`], [`RadialMenu::select_at`] and
//! [`RadialMenu::scroll`].

use std::f32::consts::{PI, TAU};

use gpui::{
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px, Action, App, Bounds,
    Hsla, InteractiveElement as _, IntoElement, MouseMoveEvent, ParentElement, PathBuilder, Pixels,
    Point, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled, Window,
};
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};

/// Most items shown; more would make wedges too thin to aim at.
pub const MAX_ITEMS: usize = 12;

/// Pixels of scroll that step the selection by one item.
const SCROLL_STEP: f32 = 24.0;

/// Gap between neighbouring wedges, in radians.
const WEDGE_GAP: f32 = 0.035;

/// How far the selected wedge is pushed out from the centre.
const SELECTED_OFFSET: f32 = 7.0;

/// Size of the box holding each item's icon and label.
const LABEL_BOX: (f32, f32) = (78.0, 44.0);

pub struct RadialMenuItem {
    pub label: SharedString,
    pub icon: IconName,
    pub action: Box<dyn Action>,
}

impl Clone for RadialMenuItem {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            icon: self.icon.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

/// Menu state: items, which one is selected, and where it is drawn.
#[derive(Clone)]
pub struct RadialMenu {
    items: Vec<RadialMenuItem>,
    selected: Option<usize>,
    /// Centre of the ring, in window coordinates.
    center: Point<Pixels>,
    scroll_accum: f32,
}

impl RadialMenu {
    /// A menu centred on `anchor`, moved inward as needed so the whole ring
    /// fits inside `viewport` (window bounds). Keeps at most [`MAX_ITEMS`].
    pub fn new(
        mut items: Vec<RadialMenuItem>,
        anchor: Point<Pixels>,
        viewport: Bounds<Pixels>,
    ) -> Self {
        items.truncate(MAX_ITEMS);
        let reach = px(Self::outer_radius_for(items.len()) + SELECTED_OFFSET + LABEL_BOX.1);
        let clamp = |v: Pixels, lo: Pixels, hi: Pixels| {
            if hi < lo {
                (lo + hi) / 2.
            } else if v < lo {
                lo
            } else if v > hi {
                hi
            } else {
                v
            }
        };
        let center = point(
            clamp(
                anchor.x,
                viewport.origin.x + reach,
                viewport.origin.x + viewport.size.width - reach,
            ),
            clamp(
                anchor.y,
                viewport.origin.y + reach,
                viewport.origin.y + viewport.size.height - reach,
            ),
        );
        Self {
            selected: (!items.is_empty()).then_some(0),
            items,
            center,
            scroll_accum: 0.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected(&self) -> Option<&RadialMenuItem> {
        self.selected.and_then(|ix| self.items.get(ix))
    }

    /// A copy of the selected item's action.
    pub fn selected_action(&self) -> Option<Box<dyn Action>> {
        self.selected().map(|item| item.action.boxed_clone())
    }

    /// Select the next item clockwise.
    pub fn select_next(&mut self) {
        let n = self.items.len();
        if n > 0 {
            self.selected = Some(self.selected.map_or(0, |ix| (ix + 1) % n));
        }
    }

    /// Select the next item counter-clockwise.
    pub fn select_prev(&mut self) {
        let n = self.items.len();
        if n > 0 {
            self.selected = Some(self.selected.map_or(n - 1, |ix| (ix + n - 1) % n));
        }
    }

    /// Step the selection by accumulated scroll; one item per [`SCROLL_STEP`]
    /// pixels (down / right is clockwise). Returns whether it changed.
    pub fn scroll(&mut self, delta_y: Pixels) -> bool {
        self.scroll_accum += f32::from(delta_y);
        let mut changed = false;
        while self.scroll_accum >= SCROLL_STEP {
            self.scroll_accum -= SCROLL_STEP;
            self.select_next();
            changed = true;
        }
        while self.scroll_accum <= -SCROLL_STEP {
            self.scroll_accum += SCROLL_STEP;
            self.select_prev();
            changed = true;
        }
        changed
    }

    /// Select the wedge in the direction of `position` (window coordinates).
    /// Inside the hub the selection is kept, so small movements near the
    /// centre don't flick between items. Returns whether it changed.
    pub fn select_at(&mut self, position: Point<Pixels>) -> bool {
        let n = self.items.len();
        if n == 0 {
            return false;
        }
        let dx = f32::from(position.x - self.center.x);
        let dy = f32::from(position.y - self.center.y);
        if (dx * dx + dy * dy).sqrt() < self.inner_radius() * 0.6 {
            return false;
        }
        let ix = Self::index_for_angle(dy.atan2(dx), n);
        let changed = self.selected != Some(ix);
        self.selected = Some(ix);
        changed
    }

    /// Which of `n` wedges contains `angle` (radians, 0 = +x, clockwise on
    /// screen). Wedge 0 is centred straight up.
    fn index_for_angle(angle: f32, n: usize) -> usize {
        let step = TAU / n as f32;
        // Rotate so wedge 0's centre (-90°) is at 0, then shift by half a wedge.
        let from_top = (angle + PI / 2.0 + step / 2.0).rem_euclid(TAU);
        ((from_top / step) as usize).min(n - 1)
    }

    /// Centre angle of wedge `ix` of `n`.
    fn wedge_angle(ix: usize, n: usize) -> f32 {
        -PI / 2.0 + ix as f32 * TAU / n as f32
    }

    fn outer_radius_for(n: usize) -> f32 {
        (96.0 + 9.0 * n as f32).clamp(116.0, 170.0)
    }

    fn outer_radius(&self) -> f32 {
        Self::outer_radius_for(self.items.len())
    }

    fn inner_radius(&self) -> f32 {
        self.outer_radius() * 0.42
    }

    /// A ring segment from `a0` to `a1` (radians) between radii `r` and `big_r`,
    /// centred on `c`.
    fn wedge_path(
        c: Point<Pixels>,
        r: f32,
        big_r: f32,
        a0: f32,
        a1: f32,
    ) -> Option<gpui::Path<Pixels>> {
        let at = |radius: f32, angle: f32| {
            point(
                c.x + px(radius * angle.cos()),
                c.y + px(radius * angle.sin()),
            )
        };
        let large = a1 - a0 > PI;
        let mut builder = PathBuilder::fill();
        builder.move_to(at(r, a0));
        builder.line_to(at(big_r, a0));
        // Increasing angle is clockwise on screen (y points down).
        builder.arc_to(
            point(px(big_r), px(big_r)),
            px(0.),
            large,
            true,
            at(big_r, a1),
        );
        builder.line_to(at(r, a1));
        builder.arc_to(point(px(r), px(r)), px(0.), large, false, at(r, a0));
        builder.close();
        builder.build().ok()
    }

    /// The menu as a full-window layer drawn above everything: the ring, item
    /// labels, and the selected item's name in the hub. It blocks the mouse
    /// from reaching the editor below; pointer movement and scrolling go to
    /// `on_hover` (window position) and `on_scroll` (vertical pixels).
    pub fn render(
        &self,
        window: &mut Window,
        cx: &App,
        on_hover: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
        on_scroll: impl Fn(Pixels, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let n = self.items.len();
        let (big_r, r) = (self.outer_radius(), self.inner_radius());
        let center = self.center;
        let selected = self.selected;

        let wedge_fill: Hsla = theme.popover.opacity(0.92);
        let wedge_edge: Hsla = theme.border;
        let selected_fill: Hsla = theme.primary.opacity(0.95);
        let hub_fill: Hsla = theme.background.opacity(0.96);

        let ring = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let step = TAU / n.max(1) as f32;
                for ix in 0..n {
                    let mid = Self::wedge_angle(ix, n);
                    let (a0, a1) = (
                        mid - step / 2.0 + WEDGE_GAP / 2.0,
                        mid + step / 2.0 - WEDGE_GAP / 2.0,
                    );
                    let is_selected = selected == Some(ix);
                    // The selected wedge slides outward along its centre line.
                    let c = if is_selected {
                        point(
                            center.x + px(SELECTED_OFFSET * mid.cos()),
                            center.y + px(SELECTED_OFFSET * mid.sin()),
                        )
                    } else {
                        center
                    };
                    // Edge first (slightly larger), then the fill over it.
                    if let Some(edge) =
                        Self::wedge_path(c, r - 1.0, big_r + 1.0, a0 - 0.004, a1 + 0.004)
                    {
                        window.paint_path(
                            edge,
                            if is_selected {
                                selected_fill
                            } else {
                                wedge_edge
                            },
                        );
                    }
                    if let Some(fill) = Self::wedge_path(c, r, big_r, a0, a1) {
                        window.paint_path(
                            fill,
                            if is_selected {
                                selected_fill
                            } else {
                                wedge_fill
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .size_full();

        let label_r = (r + big_r) / 2.0;
        let labels = self.items.iter().enumerate().map(|(ix, item)| {
            let mid = Self::wedge_angle(ix, n);
            let push = if selected == Some(ix) {
                SELECTED_OFFSET
            } else {
                0.0
            };
            let x = center.x + px((label_r + push) * mid.cos() - LABEL_BOX.0 / 2.0);
            let y = center.y + px((label_r + push) * mid.sin() - LABEL_BOX.1 / 2.0);
            let is_selected = selected == Some(ix);
            let color = if is_selected {
                theme.primary_foreground
            } else {
                theme.popover_foreground
            };
            v_flex()
                .absolute()
                .left(x)
                .top(y)
                .w(px(LABEL_BOX.0))
                .h(px(LABEL_BOX.1))
                .items_center()
                .justify_center()
                .gap_0p5()
                .child(Icon::new(item.icon.clone()).small().text_color(color))
                .child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .max_w(px(LABEL_BOX.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(item.label.clone()),
                )
        });

        let hub_size = px((r - 6.0) * 2.0);
        let hub_label = v_flex()
            .absolute()
            .left(center.x - hub_size / 2.)
            .top(center.y - hub_size / 2.)
            .size(hub_size)
            .rounded_full()
            .bg(hub_fill)
            .border_1()
            .border_color(wedge_edge)
            .items_center()
            .justify_center()
            .px_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .text_center()
                    .child(
                        self.selected()
                            .map(|item| item.label.clone())
                            .unwrap_or_else(|| "Nothing here".into()),
                    ),
            );

        let hint = h_flex()
            .absolute()
            .left(center.x - px(140.))
            .top(center.y + px(big_r + SELECTED_OFFSET + 10.))
            .w(px(280.))
            .justify_center()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("Release Tab to run · Scroll or ←/→ to choose · Esc to cancel");

        // Anchored at the window origin and sized to the window, so every
        // position above is in window coordinates -- the same space the ring
        // is painted in -- wherever the host sits in the layout.
        let layer = div()
            .id("radial-menu")
            .occlude()
            .relative()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .bg(theme.overlay.opacity(0.25))
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                on_hover(event.position, window, cx)
            })
            .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                on_scroll(event.delta.pixel_delta(px(20.)).y, window, cx)
            })
            .child(ring)
            .children(labels)
            .when(n > 0, |el| el.child(hub_label))
            .child(hint);
        deferred(anchored().position(point(px(0.), px(0.))).child(layer)).with_priority(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_map_to_the_wedge_around_them() {
        // 4 items: up, right, down, left (clockwise from the top).
        assert_eq!(RadialMenu::index_for_angle(-PI / 2.0, 4), 0);
        assert_eq!(RadialMenu::index_for_angle(0.0, 4), 1);
        assert_eq!(RadialMenu::index_for_angle(PI / 2.0, 4), 2);
        assert_eq!(RadialMenu::index_for_angle(PI, 4), 3);
        // Just clockwise of the boundary between 0 and 1 (-45°) is item 1.
        assert_eq!(RadialMenu::index_for_angle(-PI / 4.0 + 0.01, 4), 1);
        assert_eq!(RadialMenu::index_for_angle(-PI / 4.0 - 0.01, 4), 0);
        // Every angle lands on a valid index.
        for n in 1..=MAX_ITEMS {
            for step in 0..360 {
                let a = (step as f32).to_radians() - PI;
                assert!(RadialMenu::index_for_angle(a, n) < n);
            }
        }
    }

    #[test]
    fn wedge_centres_start_at_the_top_and_go_clockwise() {
        assert!((RadialMenu::wedge_angle(0, 4) + PI / 2.0).abs() < 1e-6);
        assert!((RadialMenu::wedge_angle(1, 4)).abs() < 1e-6);
        for n in 1..=MAX_ITEMS {
            for ix in 0..n {
                assert_eq!(
                    RadialMenu::index_for_angle(RadialMenu::wedge_angle(ix, n), n),
                    ix
                );
            }
        }
    }
}
