//! The camera viewport: the live video on a WGPU surface with draggable HUD
//! cards (record controls, capture stats) on a panel canvas.

use crate::blit::FrameBlit;
use crate::model::SessionModel;
use gpui::prelude::*;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::panel_canvas::{DragHandle, PanelCanvas, PanelCanvasState, PanelRef, RndPanel, Slot, SlotAnchor};
use ui::{h_flex, v_flex, ActiveTheme, IconName, Selectable as _, Sizable as _};

/// The surface's pixel format (the window compositor's, not sRGB).
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// The camera view.
pub struct ViewportPanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
    surface: Option<WgpuSurfaceHandle>,
    blit: Option<FrameBlit>,
    seen_video: u64,
    overlay: Entity<PanelCanvasState>,
}

impl ViewportPanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        let overlay = cx.new(|_| {
            PanelCanvasState::new().with_slots([
                Slot::new("toolbar", SlotAnchor::TopLeft, size(px(470.), px(44.))).inset(px(8.), px(8.)),
                Slot::new("stats", SlotAnchor::TopRight, size(px(300.), px(96.))).inset(px(8.), px(8.)),
            ])
        });
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&overlay, |_, _, cx| cx.notify()).detach();
        Self { model, focus_handle: cx.focus_handle(), surface: None, blit: None, seen_video: 0, overlay }
    }

    fn ensure_surface(&mut self, window: &mut Window) {
        if self.surface.as_ref().is_some_and(WgpuSurfaceHandle::is_device_lost) {
            self.surface = None;
            self.blit = None;
            self.seen_video = 0;
        }
        if self.surface.is_some() {
            return;
        }
        let size = window.bounds().size;
        let width = (size.width.to_f64() as u32).max(64);
        let height = (size.height.to_f64() as u32).max(64);
        self.surface = window
            .create_wgpu_surface(width, height, SURFACE_FORMAT)
            .filter(|surface| !surface.is_device_lost());
    }

    /// Uploads a newer camera frame and publishes the surface.
    fn paint_surface(&mut self, window: &Window, cx: &App) {
        let Some(surface) = &self.surface else { return };
        if window.is_window_resizing() || surface.is_resize_pending() || surface.has_unconsumed_frame() {
            return;
        }
        let Some(_submit) = surface.try_submit_guard() else { return };
        let (device, queue) = (surface.device(), surface.queue());
        let blit = self.blit.get_or_insert_with(|| FrameBlit::new(device, surface.format()));
        let model = self.model.read(cx);
        if let Some((seen, frame)) = model.video.latest_if_newer(self.seen_video) {
            blit.upload(device, queue, &frame);
            self.seen_video = seen;
        }
        let Some((view, size)) = surface.back_view_with_size() else { return };
        let submission = blit.draw(device, queue, &view, size, model.mirror);
        drop(view);
        surface.present_synced_silent(submission);
    }

    fn stats_text(&self, cx: &App) -> Vec<String> {
        let model = self.model.read(cx);
        let mut lines = Vec::new();
        for source in model.sources.iter().filter(|s| s.running()) {
            let status = source.status();
            lines.push(format!("{}  {:.0} Hz", source.descriptor.label, status.rate_hz));
        }
        if let Some(blit) = &self.blit {
            if let Some((w, h)) = blit.video_size() {
                lines.push(format!("Video {w}x{h}"));
            }
        }
        if model.is_recording() {
            let stats = model.record_stats;
            lines.push(format!("REC {} frames, {} gap, {} dropped", stats.frames, stats.gap_frames, model.bus_dropped()));
        } else if lines.is_empty() {
            lines.push("No source running".to_owned());
        }
        lines
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> RndPanel {
        let theme = cx.theme().clone();
        let recording = self.model.read(cx).is_recording();
        let mirror = self.model.read(cx).mirror;
        let panel = PanelRef::new(&self.overlay, "toolbar");
        let record = Button::new("mocap-record")
            .icon(if recording { IconName::Pause } else { IconName::Circle })
            .label(if recording { "Stop" } else { "Record" })
            .small()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.update(cx, |m, cx| {
                    if m.is_recording() {
                        m.stop_recording();
                    } else {
                        m.start_recording();
                    }
                    cx.notify();
                });
            }));
        let record = if recording { record.danger() } else { record.ghost() };
        let action = |id: &'static str, icon: IconName, label: &'static str| Button::new(id).icon(icon).label(label).ghost().small();
        RndPanel::new(panel.clone())
            .bg(theme.background.opacity(0.88))
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .child(DragHandle::new(panel).w(px(14.)).h_full().border_r_1().border_color(theme.border))
                    .child(
                        h_flex()
                            .px_1()
                            .gap_1()
                            .child(record)
                            .child(action("mocap-marker", IconName::Bookmark, "Marker").on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |m, cx| {
                                    m.mark();
                                    cx.notify();
                                });
                            })))
                            .child(action("mocap-calibrate", IconName::User, "Calibrate").on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |m, cx| {
                                    m.calibrate();
                                    cx.notify();
                                });
                            })))
                            .child(action("mocap-mirror", IconName::Camera, "Mirror").selected(mirror).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.model.update(cx, |m, cx| {
                                        m.mirror = !m.mirror;
                                        cx.notify();
                                    });
                                },
                            ))),
                    ),
            )
            .default_slot("toolbar")
            .default_size(size(px(470.), px(40.)))
    }

    fn stats_card(&self, cx: &App) -> RndPanel {
        let theme = cx.theme();
        let panel = PanelRef::new(&self.overlay, "stats");
        RndPanel::new(panel.clone())
            .bg(theme.background.opacity(0.80))
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .child(
                h_flex()
                    .size_full()
                    .child(DragHandle::new(panel).w(px(14.)).h_full().border_r_1().border_color(theme.border))
                    .child(
                        v_flex()
                            .p_2()
                            .gap_0p5()
                            .text_xs()
                            .children(self.stats_text(cx).into_iter().map(|line| div().child(line))),
                    ),
            )
            .default_slot("stats")
            .default_size(size(px(300.), px(88.)))
    }
}

impl EventEmitter<PanelEvent> for ViewportPanel {}

impl Focusable for ViewportPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ViewportPanel {
    fn panel_name(&self) -> &'static str {
        "mocap-viewport"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Camera").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for ViewportPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_surface(window);
        self.paint_surface(window, cx);
        let theme = cx.theme().clone();
        let video_live = self.model.read(cx).live_video();
        let surface = match &self.surface {
            Some(handle) => wgpu_surface(handle.clone()).defer_resize_until_mouse_up(true).size_full().into_any_element(),
            None => div().size_full().into_any_element(),
        };
        let hint = div().absolute().inset_0().flex().items_center().justify_center().text_color(theme.muted_foreground).when(
            !video_live,
            |d| d.child("No camera running. Add one in Sources, or start a pose source."),
        );
        let canvas = PanelCanvas::new("mocap-viewport-canvas", &self.overlay)
            .size_full()
            .child(div().absolute().inset_0().child(surface))
            .child(hint)
            .panel(self.toolbar(cx))
            .panel(self.stats_card(cx));
        div().size_full().bg(theme.background).child(canvas)
    }
}
