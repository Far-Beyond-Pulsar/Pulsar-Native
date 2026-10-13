//! The controls floating over the canvas: zoom, screen size, view toggles
//! and alignment.

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::menu::DropdownMenu;
use ui::tag::Tag;
use ui::{h_flex, ActiveTheme, Disableable, IconName, Selectable, Sizable, StyledExt};

use crate::display::PreviewState;
use crate::model::{SlotKind, Vec2};

use super::super::canvas::geometry::AlignKind;
use super::super::common::chrome::toolbar_divider;
use super::super::state::{Screen, SCREEN_PRESETS};
use super::DesignerPanel;

fn align_icon(kind: AlignKind) -> IconName {
    match kind {
        AlignKind::Left => IconName::AlignLeftBox,
        AlignKind::CenterH => IconName::AlignHorizontalCenters,
        AlignKind::Right => IconName::AlignRightBox,
        AlignKind::Top => IconName::AlignTopBox,
        AlignKind::CenterV => IconName::AlignVerticalCenters,
        AlignKind::Bottom => IconName::AlignBottomBox,
    }
}

impl DesignerPanel {
    pub(super) fn render_toolbar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = self.ctx.view.read(cx);
        let widget = self.ctx.widget(cx);
        let zoom = view.zoom;
        let screen_label = view.screen_label(widget);
        let dpi = view.dpi;
        let state = view.state;
        let (grid, snap, outlines, safe, placeholders) = (view.show_grid, view.snap, view.outlines, view.safe_zone, view.placeholders);
        let canvas_selected = self
            .ctx
            .selected(cx)
            .iter()
            .filter(|&&id| widget.slot_kind_of(id) == SlotKind::Canvas)
            .count();

        let ctx = self.ctx.clone();
        let toggle = |id: &'static str, icon: IconName, tip: &'static str, on: bool, f: fn(&mut super::super::state::View)| {
            let ctx = ctx.clone();
            Button::new(id)
                .icon(icon)
                .ghost()
                .small()
                .selected(on)
                .tooltip(tip)
                .on_click(move |_, _, cx| {
                    ctx.view.update(cx, |v, cx| {
                        f(v);
                        cx.notify();
                    })
                })
        };

        let zoom_ctx = self.ctx.clone();
        let canvas = self.canvas_size();
        let zoom_by = move |factor: f32, cx: &mut App| {
            let (c, at) = (zoom_ctx.clone(), Vec2::new(canvas.x * 0.5, canvas.y * 0.5));
            c.view.update(cx, |v, cx| {
                let z = v.zoom * factor;
                v.set_zoom_about(z, at);
                cx.notify();
            });
        };
        let (zi, zo, zr) = (zoom_by.clone(), zoom_by.clone(), zoom_by);
        let fit_ctx = self.ctx.clone();

        let screen_ctx = self.ctx.clone();
        let dpi_ctx = self.ctx.clone();
        let state_ctx = self.ctx.clone();

        h_flex()
            .occlude()
            .absolute()
            .top(px(8.0))
            .left(px(8.0))
            .right(px(8.0))
            .gap_1()
            .items_center()
            .px_1p5()
            .py_1()
            .rounded(px(8.0))
            .bg(theme.popover.opacity(0.94))
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .child(Button::new("zoom-out").icon(IconName::ZoomOut).ghost().small().tooltip("Zoom out").on_click(move |_, _, cx| zo(1.0 / 1.25, cx)))
            .child(
                Button::new("zoom-reset")
                    .label(format!("{}%", (zoom * 100.0).round()))
                    .ghost()
                    .small()
                    .tooltip("Zoom to 100% (Ctrl+0)")
                    .on_click(move |_, _, cx| {
                        zr(1.0 / zoom.max(0.01), cx);
                    }),
            )
            .child(Button::new("zoom-in").icon(IconName::ZoomIn).ghost().small().tooltip("Zoom in").on_click(move |_, _, cx| zi(1.25, cx)))
            .child(
                Button::new("zoom-fit")
                    .icon(IconName::Maximize)
                    .ghost()
                    .small()
                    .tooltip("Fit the screen to the window (Home)")
                    .on_click(move |_, _, cx| {
                        fit_ctx.view.update(cx, |v, cx| {
                            v.auto_fit = true;
                            cx.notify();
                        })
                    }),
            )
            .child(toolbar_divider(cx))
            .child(
                Button::new("screen-size")
                    .icon(IconName::AppWindow)
                    .label(screen_label)
                    .ghost()
                    .small()
                    .tooltip("The screen size to preview the widget at")
                    .popup_menu(move |menu, _, _| {
                        let mut menu = menu;
                        let c = screen_ctx.clone();
                        menu = menu.menu_handler("Design size", move |_, cx| {
                            c.view.update(cx, |v, cx| {
                                v.screen = Screen::Design;
                                v.auto_fit = true;
                                cx.notify();
                            })
                        });
                        menu = menu.separator();
                        for (i, preset) in SCREEN_PRESETS.iter().enumerate() {
                            let c = screen_ctx.clone();
                            menu = menu.menu_handler(preset.name, move |_, cx| {
                                c.view.update(cx, |v, cx| {
                                    v.screen = Screen::Preset(i);
                                    v.auto_fit = true;
                                    cx.notify();
                                })
                            });
                        }
                        menu
                    }),
            )
            .child(
                Button::new("preview-scale")
                    .label(format!("{dpi}\u{d7}"))
                    .ghost()
                    .small()
                    .tooltip("Preview with the interface scaled, as on a high-density screen")
                    .popup_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for value in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0f32] {
                            let c = dpi_ctx.clone();
                            menu = menu.menu_handler(format!("{value}\u{d7}"), move |_, cx| {
                                c.view.update(cx, |v, cx| {
                                    v.dpi = value;
                                    v.auto_fit = true;
                                    cx.notify();
                                })
                            });
                        }
                        menu
                    }),
            )
            .child(
                Button::new("preview-state")
                    .label(state.label())
                    .ghost()
                    .small()
                    .tooltip("Draw buttons in this state")
                    .popup_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for s in PreviewState::ALL {
                            let c = state_ctx.clone();
                            menu = menu.menu_handler(s.label(), move |_, cx| {
                                c.view.update(cx, |v, cx| {
                                    v.state = s;
                                    cx.notify();
                                })
                            });
                        }
                        menu
                    }),
            )
            .child(toolbar_divider(cx))
            .child(toggle("view-grid", IconName::GridPlus, "Show the grid", grid, |v| v.show_grid = !v.show_grid))
            .child(toggle("view-snap", IconName::Pin, "Snap to the grid and to other widgets", snap, |v| v.snap = !v.snap))
            .child(toggle("view-outlines", IconName::FrameSelect, "Outline every widget", outlines, |v| v.outlines = !v.outlines))
            .child(toggle("view-safe", IconName::Frame, "Show the title-safe and action-safe areas", safe, |v| v.safe_zone = !v.safe_zone))
            .child(toggle("view-placeholders", IconName::PlusSquareDashed, "Show stand-ins for empty slots and missing widgets", placeholders, |v| {
                v.placeholders = !v.placeholders
            }))
            .when(canvas_selected >= 2, |bar| {
                let mut bar = bar.child(toolbar_divider(cx));
                for kind in AlignKind::ALL {
                    let c = self.ctx.clone();
                    bar = bar.child(
                        Button::new(SharedString::from(format!("align-{kind:?}")))
                            .icon(align_icon(kind))
                            .ghost()
                            .small()
                            .tooltip(kind.label())
                            .on_click(move |_, _, cx| c.align_selected(kind, cx)),
                    );
                }
                let (h, v) = (self.ctx.clone(), self.ctx.clone());
                bar.child(
                    Button::new("distribute-h")
                        .icon(IconName::AlignHorizontalSpacing)
                        .ghost()
                        .small()
                        .disabled(canvas_selected < 3)
                        .tooltip("Distribute horizontally")
                        .on_click(move |_, _, cx| h.distribute_selected(true, cx)),
                )
                .child(
                    Button::new("distribute-v")
                        .icon(IconName::AlignVerticalSpacing)
                        .ghost()
                        .small()
                        .disabled(canvas_selected < 3)
                        .tooltip("Distribute vertically")
                        .on_click(move |_, _, cx| v.distribute_selected(false, cx)),
                )
            })
            .child(div().flex_1())
            .when(self.gesture.is_some() && self.space_down, |bar| bar.child(Tag::info().small().child("Panning")))
    }

    /// Pointer position, selection and the last message, along the bottom.
    pub(super) fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = self.ctx.view.read(cx);
        let widget = self.ctx.widget(cx);
        let pointer = self.mouse.map(|m| view.to_widget(m));
        let selected = self.ctx.sel.read(cx).len();
        let notice = view.notice.clone();
        let screen = view.screen_size(widget);

        h_flex()
            .absolute()
            .bottom(px(8.0))
            .left(px(8.0))
            .right(px(8.0))
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .occlude()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.0))
                    .bg(theme.popover.opacity(0.9))
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} \u{d7} {}", screen.x.round(), screen.y.round()))
                    .children(pointer.map(|p| format!("x {}  y {}", p.x.round(), p.y.round())))
                    .when(selected > 0, |d| d.child(format!("{selected} selected"))),
            )
            .children(notice.map(|message| {
                h_flex()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.0))
                    .bg(theme.warning.opacity(0.16))
                    .border_1()
                    .border_color(theme.warning.opacity(0.5))
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(message)
            }))
    }
}
