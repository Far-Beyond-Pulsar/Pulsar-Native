//! The sources panel: every capture source, its state, and what can be added.

use crate::model::{NewSource, SessionModel, DEFAULT_VMC_PORT};
use gpui::prelude::*;
use gpui::*;
use mocap_capture::SourceState;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable as _};

/// Lists, starts and stops capture sources.
pub struct SourcesPanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
}

impl SourcesPanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, focus_handle: cx.focus_handle() }
    }

    fn row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.model.read(cx);
        let entry = &model.sources[index];
        let status = entry.status();
        let running = entry.running();
        let (state_text, state_color) = match &status.state {
            SourceState::Idle => ("Stopped".to_owned(), theme.muted_foreground),
            SourceState::Starting => ("Starting".to_owned(), theme.warning),
            SourceState::Live => (format!("Live {:.0} Hz", status.rate_hz), theme.success),
            SourceState::Failed(reason) => (reason.clone(), theme.danger),
        };
        let icon = if entry.descriptor.has_video { IconName::VideoCamera } else { IconName::Activity };
        let label = entry.descriptor.label.clone();
        h_flex()
            .w_full()
            .min_h(px(34.))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                Button::new(SharedString::from(format!("source-toggle-{index}")))
                    .icon(if running { IconName::Pause } else { IconName::Play })
                    .ghost()
                    .small()
                    .tooltip(if running { "Stop" } else { "Start" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.toggle_source(index);
                            cx.notify();
                        });
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(h_flex().gap_1().items_center().text_sm().child(ui::Icon::new(icon)).child(div().truncate().child(label)))
                    .child(div().text_xs().text_color(state_color).truncate().child(state_text)),
            )
            .child(
                Button::new(SharedString::from(format!("source-remove-{index}")))
                    .icon(IconName::Trash)
                    .ghost()
                    .small()
                    .tooltip("Remove")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.remove_source(index);
                            cx.notify();
                        });
                    })),
            )
    }

    fn add_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let add = |id: &'static str, icon: IconName, label: &'static str| Button::new(id).icon(icon).label(label).small();
        h_flex()
            .p_2()
            .gap_1()
            .flex_wrap()
            .child(add("add-synthetic", IconName::Activity, "Synthetic").ghost().on_click(cx.listener(|this, _, _, cx| {
                this.model.update(cx, |m, cx| {
                    m.add_source(NewSource::Synthetic);
                    cx.notify();
                });
            })))
            .child(add("add-vmc", IconName::Wifi, "VMC").ghost().tooltip("Listen for VMC/OSC on UDP 39539").on_click(cx.listener(
                |this, _, _, cx| {
                    this.model.update(cx, |m, cx| {
                        m.add_source(NewSource::Vmc(DEFAULT_VMC_PORT));
                        cx.notify();
                    });
                },
            )))
            .child(add("list-cameras", IconName::Camera, "Find cameras").ghost().on_click(cx.listener(|this, _, _, cx| {
                this.model.update(cx, |m, cx| {
                    m.refresh_cameras();
                    cx.notify();
                });
            })))
    }

    fn camera_choices(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let loading = model.cameras_loading;
        let choices = model.cameras.clone();
        v_flex()
            .px_2()
            .gap_1()
            .when(loading, |d| d.child(div().text_xs().text_color(cx.theme().muted_foreground).child("Looking for cameras...")))
            .children(choices.into_iter().enumerate().map(|(i, choice)| {
                let name = choice.name.clone();
                Button::new(SharedString::from(format!("add-camera-{i}")))
                    .icon(IconName::Plus)
                    .label(format!("Add {name}"))
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let choice = choice.clone();
                        this.model.update(cx, |m, cx| {
                            m.add_camera(&choice);
                            cx.notify();
                        });
                    }))
            }))
    }
}

impl EventEmitter<PanelEvent> for SourcesPanel {}

impl Focusable for SourcesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SourcesPanel {
    fn panel_name(&self) -> &'static str {
        "mocap-sources"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Sources").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }
}

impl Render for SourcesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.model.read(cx).sources.len();
        let status = self.model.read(cx).status.clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.add_bar(cx))
            .child(self.camera_choices(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_hidden()
                    .when(count == 0, |d| {
                        d.child(div().p_3().text_xs().text_color(theme.muted_foreground).child("No sources yet. Add a synthetic figure, a VMC listener or a camera."))
                    })
                    .children((0..count).map(|i| self.row(i, cx))),
            )
            .child(div().px_2().py_1().text_xs().text_color(theme.muted_foreground).border_t_1().border_color(theme.border).truncate().child(status))
    }
}
