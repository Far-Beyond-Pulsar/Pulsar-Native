//! The inspector: the take, the rig, the live filter and the calibration.

use crate::model::SessionModel;
use gpui::prelude::*;
use gpui::*;
use mocap_capture::FilterSettings;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::{h_flex, v_flex, ActiveTheme, Selectable as _, Sizable as _};

/// Read-only facts and the live filter's controls.
pub struct InspectorPanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
}

impl InspectorPanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, focus_handle: cx.focus_handle() }
    }

    fn section(title: &'static str, cx: &App) -> Div {
        v_flex()
            .p_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().muted_foreground).child(title))
    }

    fn fact(label: &'static str, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
        h_flex()
            .justify_between()
            .text_xs()
            .child(div().text_color(cx.theme().muted_foreground).child(label))
            .child(div().truncate().child(value.into()))
    }

    fn take_section(&self, cx: &App) -> impl IntoElement {
        let model = self.model.read(cx);
        let section = Self::section("Take", cx);
        let Some(take) = &model.take else {
            return section.child(div().text_xs().child(if model.loading { "Loading..." } else { "No take open" }));
        };
        let meta = &take.meta;
        section
            .child(Self::fact("Name", meta.name.clone(), cx))
            .child(Self::fact("Frames", take.frame_count().to_string(), cx))
            .child(Self::fact("Rate", format!("{:.3} fps", meta.fps()), cx))
            .child(Self::fact("Length", format!("{:.2} s", take.duration()), cx))
            .child(Self::fact("Markers", take.markers.len().to_string(), cx))
            .child(Self::fact("Sources", meta.sources.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(", "), cx))
            .child(Self::fact("Rig", format!("{} ({} joints, {} channels)", take.rig.name, take.rig.joints.len(), take.rig.channels.len()), cx))
    }

    fn channel_section(&self, cx: &App) -> impl IntoElement {
        let model = self.model.read(cx);
        let section = Self::section("Channel", cx);
        let (Some(take), Some(channel)) = (&model.take, model.selected_channel) else {
            return section.child(div().text_xs().child("Select a channel in the timeline"));
        };
        let Some(desc) = take.rig.channels.get(channel) else { return section };
        let at = take.rig.column_offsets()[channel];
        let (mut gaps, frames) = (0usize, take.frame_count());
        for value in &take.columns[at] {
            gaps += usize::from(!value.is_finite());
        }
        section
            .child(Self::fact("Name", desc.name.clone(), cx))
            .child(Self::fact("Kind", format!("{:?}", desc.kind), cx))
            .child(Self::fact("Storage", format!("{:?}", desc.quantisation), cx))
            .child(Self::fact("Gaps", format!("{gaps} of {frames} frames"), cx))
    }

    fn filter_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.model.read(cx).filter;
        let nudge = |id: &'static str, label: &'static str, change: fn(FilterSettings) -> FilterSettings| {
            Button::new(id).label(label).ghost().small().on_click(cx.listener(move |this, _, _, cx| {
                this.model.update(cx, |m, cx| {
                    let next = change(m.filter);
                    m.set_filter(next);
                    cx.notify();
                });
            }))
        };
        Self::section("Live filter (One Euro)", cx)
            .child(
                h_flex().gap_1().child(
                    Button::new("filter-enabled")
                        .label(if filter.enabled { "Enabled" } else { "Off" })
                        .ghost()
                        .small()
                        .selected(filter.enabled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |m, cx| {
                                let next = FilterSettings { enabled: !m.filter.enabled, ..m.filter };
                                m.set_filter(next);
                                cx.notify();
                            });
                        })),
                ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .child(div().w(px(96.)).child(format!("Smoothing {:.2} Hz", filter.min_cutoff)))
                    .child(nudge("cutoff-down", "-", |f| FilterSettings { min_cutoff: (f.min_cutoff / 1.25).max(0.05), ..f }))
                    .child(nudge("cutoff-up", "+", |f| FilterSettings { min_cutoff: (f.min_cutoff * 1.25).min(30.0), ..f })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .child(div().w(px(96.)).child(format!("Response {:.2}", filter.beta)))
                    .child(nudge("beta-down", "-", |f| FilterSettings { beta: (f.beta / 1.5).max(0.001), ..f }))
                    .child(nudge("beta-up", "+", |f| FilterSettings { beta: (f.beta * 1.5).min(20.0), ..f })),
            )
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Lower smoothing is steadier and lags more; response lets fast motion through."))
    }

    fn calibration_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let calibrated = self.model.read(cx).calibration.is_some();
        let scale = self.model.read(cx).calibration.as_ref().map(|c| c.scale);
        Self::section("Calibration", cx)
            .child(Self::fact("State", if calibrated { "Calibrated" } else { "Uncalibrated" }, cx))
            .when_some(scale, |s, scale| s.child(Self::fact("Height scale", format!("{scale:.3}"), cx)))
            .child(
                h_flex()
                    .gap_1()
                    .child(Button::new("calibrate-now").label("Calibrate (T-pose)").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.calibrate();
                            cx.notify();
                        });
                    })))
                    .child(Button::new("calibrate-clear").label("Clear").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.clear_calibration();
                            cx.notify();
                        });
                    }))),
            )
    }
}

impl EventEmitter<PanelEvent> for InspectorPanel {}

impl Focusable for InspectorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for InspectorPanel {
    fn panel_name(&self) -> &'static str {
        "mocap-inspector"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Inspector").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }
}

impl Render for InspectorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .overflow_y_hidden()
            .bg(cx.theme().background)
            .child(self.take_section(cx))
            .child(self.channel_section(cx))
            .child(self.filter_section(cx))
            .child(self.calibration_section(cx))
    }
}
