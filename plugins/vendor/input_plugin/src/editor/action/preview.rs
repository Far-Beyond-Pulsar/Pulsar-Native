//! The timeline: the action tried against a made-up input, drawn.

use std::rc::Rc;

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt};

use crate::model::ActionAsset;
use crate::preview::{peak, simulate, Pattern, Sample};
use crate::runtime::system::Phase;
use crate::runtime::trigger::TriggerState;

use super::super::common::chrome::panel_header;
use super::super::common::panel::dock_panel;
use super::super::form::chip;
use super::ActionCtx;

pub struct PreviewPanel {
    ctx: ActionCtx,
    pattern: Pattern,
    cache: Option<(ActionAsset, Pattern, Rc<Vec<Sample>>)>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(PreviewPanel, "input-action-preview", "Preview");

const LANE: f32 = 44.0;

impl PreviewPanel {
    pub fn new(ctx: ActionCtx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify())];
        Self { ctx, pattern: Pattern::Hold, cache: None, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    /// The simulation for the action as it is now, rerun only when it changed.
    fn samples(&mut self, asset: &ActionAsset) -> Rc<Vec<Sample>> {
        if let Some((cached, pattern, samples)) = &self.cache {
            if cached == asset && *pattern == self.pattern {
                return samples.clone();
            }
        }
        let samples = Rc::new(simulate(asset, self.pattern));
        self.cache = Some((asset.clone(), self.pattern, samples.clone()));
        samples
    }
}

/// One lane of the timeline: `paint` draws into the lane's bounds.
fn lane(label: &'static str, hint: String, paint: impl Fn(Bounds<Pixels>, &mut Window) + 'static, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .gap_2()
        .child(
            v_flex()
                .w(px(96.0))
                .flex_shrink_0()
                .justify_center()
                .child(div().text_xs().font_semibold().text_color(theme.foreground).child(label))
                .child(div().text_xs().text_color(theme.muted_foreground).child(hint)),
        )
        .child(
            div()
                .flex_1()
                .h(px(LANE))
                .rounded(px(4.0))
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .child(canvas(|_, _, _| (), move |bounds, _, window, _| paint(bounds, window)).size_full()),
        )
}

impl Render for PreviewPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let asset = self.ctx.asset(cx);
        let samples = self.samples(&asset);
        let seconds = self.pattern.seconds();
        let scale = peak(&samples);
        let count = |phase: Phase| samples.iter().flat_map(|s| &s.phases).filter(|p| **p == phase).count();

        let patterns = h_flex().gap_1().children(Pattern::ALL.map(|p| {
            let button = Button::new(SharedString::from(format!("preview-{}", p.label())))
                .label(p.label())
                .small()
                .tooltip(p.description())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pattern = p;
                    cx.notify();
                }));
            if p == self.pattern {
                button.primary()
            } else {
                button.outline()
            }
        }));

        // The three lanes read the same samples.
        let (input_samples, value_samples, state_samples) = (samples.clone(), samples.clone(), samples.clone());
        let (accent, good, warn, bad, muted) = (theme.info, theme.success, theme.warning, theme.danger, theme.muted_foreground);
        let bar = move |bounds: Bounds<Pixels>, t: f32, dt: f32, height: f32, color: Hsla, window: &mut Window| {
            if height <= 0.0 {
                return;
            }
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let x = f32::from(bounds.origin.x) + t / seconds * w;
            let bar_h = (h - 6.0) * height.clamp(0.0, 1.0);
            let y = f32::from(bounds.origin.y) + h - 3.0 - bar_h;
            let rect = Bounds::new(point(px(x), px(y)), size(px((dt / seconds * w).max(1.0) + 0.5), px(bar_h)));
            window.paint_quad(fill(rect, color));
        };

        let lanes = v_flex()
            .w_full()
            .gap_2()
            .child(lane(
                "Input",
                self.pattern.label().to_string(),
                move |bounds, window| {
                    for s in input_samples.iter() {
                        bar(bounds, s.time, crate::preview::STEP, s.input, accent.opacity(0.8), window);
                    }
                },
                cx,
            ))
            .child(lane(
                "Value",
                format!("up to {scale:.2}"),
                move |bounds, window| {
                    for s in value_samples.iter() {
                        let size = crate::model::behavior::magnitude(s.value) / scale;
                        bar(bounds, s.time, crate::preview::STEP, size, accent, window);
                    }
                },
                cx,
            ))
            .child(lane(
                "State",
                "idle · building · firing".to_string(),
                move |bounds, window| {
                    for s in state_samples.iter() {
                        let (height, color) = match s.state {
                            TriggerState::None => (0.0, muted),
                            TriggerState::Ongoing => (0.45, warn),
                            TriggerState::Triggered => (1.0, good),
                        };
                        bar(bounds, s.time, crate::preview::STEP, height, color, window);
                        // Where it ended without firing.
                        if s.phases.contains(&Phase::Canceled) {
                            bar(bounds, s.time, crate::preview::STEP * 2.0, 1.0, bad, window);
                        }
                    }
                },
                cx,
            ));

        let phases = Phase::ALL.iter().map(|&p| chip(format!("{} {}", count(p), p.word()), cx));

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Activity, "Preview", Some(patterns.into_any_element()), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .gap_2()
                    .child(lanes)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_center()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{seconds:.1} seconds:")))
                            .children(phases),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Tried with the action alone on one key, as the game would. The key drives X only; use a mapping's own modifiers to aim it."),
                    ),
            )
    }
}
