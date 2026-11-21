//! The timeline: a virtualized list of channel lanes with decimated curves,
//! a ruler with markers, a scrubbable playhead, zoom and pan.

use crate::curves::{curve_polylines, tick_step, value_range, CurveSpec};
use crate::model::SessionModel;
use crate::paint::{pt, stroke_polyline};
use gpui::prelude::*;
use gpui::*;
use mocap_format::{ChannelKind, Take};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::PixelsExt as _;
use ui::{h_flex, v_flex, v_virtual_list, ActiveTheme, IconName, Sizable as _, VirtualListScrollHandle};

const LABEL_WIDTH: f32 = 210.0;
const ROW_HEIGHT: f32 = 28.0;
const RULER_HEIGHT: f32 = 24.0;
const MIN_FRAMES_PER_PX: f64 = 0.02;

/// Per-channel value ranges for a take, computed once when it loads.
struct TakeCache {
    take: Arc<Take>,
    ranges: Vec<(f32, f32)>,
}

impl TakeCache {
    fn new(take: Arc<Take>) -> Self {
        let offsets = take.rig.column_offsets();
        let ranges = take
            .rig
            .channels
            .iter()
            .zip(offsets)
            .map(|(channel, at)| {
                let columns: Vec<&[f32]> = (at..at + channel.kind.width()).map(|c| take.columns[c].as_slice()).collect();
                let fixed = match channel.kind {
                    ChannelKind::Rotation4 => Some((-1.0, 1.0)),
                    ChannelKind::Confidence => Some((0.0, 1.0)),
                    _ => None,
                };
                value_range(&columns, fixed)
            })
            .collect();
        Self { take, ranges }
    }
}

/// What a lane paints with: everything it needs, owned, so the paint closure is `'static`.
struct Lane {
    take: Arc<Take>,
    channel: usize,
    range: (f32, f32),
    first_frame: f64,
    frames_per_px: f64,
    playhead: f64,
    colors: [Hsla; 5],
}

fn paint_lane(bounds: Bounds<Pixels>, lane: &Lane, window: &mut Window) {
    let (width, height) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let channel = &lane.take.rig.channels[lane.channel];
    let at = lane.take.rig.column_offsets()[lane.channel];
    let spec = CurveSpec {
        first_frame: lane.first_frame,
        frames_per_px: lane.frames_per_px,
        width,
        height: height - 4.0,
        lo: lane.range.0,
        hi: lane.range.1,
    };
    for k in 0..channel.kind.width() {
        let color = lane.colors[if channel.kind.width() == 1 { 4 } else { k.min(3) }];
        for line in curve_polylines(&lane.take.columns[at + k], &spec) {
            let points: Vec<_> = line.iter().map(|p| pt(ox + p[0], oy + 2.0 + p[1])).collect();
            stroke_polyline(window, &points, 1.2, color);
        }
    }
    let x = ((lane.playhead - lane.first_frame) / lane.frames_per_px) as f32;
    if (0.0..=width).contains(&x) {
        stroke_polyline(window, &[pt(ox + x, oy), pt(ox + x, oy + height)], 1.0, lane.colors[3].opacity(0.9));
    }
}

/// The timeline.
pub struct TimelinePanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
    scroll: VirtualListScrollHandle,
    first_frame: f64,
    frames_per_px: f64,
    lane_bounds: Rc<Cell<Bounds<Pixels>>>,
    scrubbing: bool,
    cache: Option<TakeCache>,
}

impl TimelinePanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            focus_handle: cx.focus_handle(),
            scroll: VirtualListScrollHandle::new(),
            first_frame: 0.0,
            frames_per_px: 0.5,
            lane_bounds: Rc::new(Cell::new(Bounds::default())),
            scrubbing: false,
            cache: None,
        }
    }

    fn lane_width(&self) -> f32 {
        self.lane_bounds.get().size.width.as_f32().max(1.0)
    }

    /// Frames of the loaded take.
    fn frames(&self) -> f64 {
        self.cache.as_ref().map_or(0.0, |c| c.take.frame_count() as f64)
    }

    fn fit(&mut self) {
        self.first_frame = 0.0;
        self.frames_per_px = (self.frames() / f64::from(self.lane_width())).max(MIN_FRAMES_PER_PX);
    }

    fn zoom_around(&mut self, factor: f64, anchor_x: f32) {
        let anchor_frame = self.first_frame + f64::from(anchor_x) * self.frames_per_px;
        self.frames_per_px = (self.frames_per_px * factor).clamp(MIN_FRAMES_PER_PX, (self.frames().max(1.0)) / 8.0);
        self.first_frame = (anchor_frame - f64::from(anchor_x) * self.frames_per_px).max(0.0);
    }

    fn frame_at(&self, x: Pixels) -> f64 {
        let lane = self.lane_bounds.get();
        self.first_frame + f64::from((x - lane.origin.x).as_f32()) * self.frames_per_px
    }

    fn scrub_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let frame = self.frame_at(x);
        self.model.update(cx, |m, cx| {
            m.seek(frame);
            cx.notify();
        });
    }

    fn sync_cache(&mut self, cx: &App) {
        let take = self.model.read(cx).take.clone();
        let current = self.cache.as_ref().map(|c| Arc::as_ptr(&c.take));
        if take.as_ref().map(Arc::as_ptr) != current {
            self.cache = take.map(TakeCache::new);
            self.fit();
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.model.read(cx);
        let playing = model.playing;
        let (frame, total, fps) = match &self.cache {
            Some(c) => (model.playhead, c.take.frame_count(), c.take.meta.fps()),
            None => (0.0, 0, 60.0),
        };
        let seconds = frame / fps;
        h_flex()
            .h(px(34.))
            .px_2()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("timeline-play")
                    .icon(if playing { IconName::Pause } else { IconName::Play })
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.toggle_play();
                            cx.notify();
                        });
                    })),
            )
            .child(div().w(px(190.)).text_xs().child(format!("Frame {:>6} / {:<6}  {:>7.2} s", frame as u64, total, seconds)))
            .child(
                Button::new("timeline-zoom-out").label("-").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                    let w = this.lane_width() / 2.0;
                    this.zoom_around(1.5, w);
                    cx.notify();
                })),
            )
            .child(
                Button::new("timeline-zoom-in").label("+").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                    let w = this.lane_width() / 2.0;
                    this.zoom_around(1.0 / 1.5, w);
                    cx.notify();
                })),
            )
            .child(Button::new("timeline-fit").label("Fit").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                this.fit();
                cx.notify();
            })))
    }

    fn ruler(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lane_bounds = Rc::clone(&self.lane_bounds);
        let (first, fpp) = (self.first_frame, self.frames_per_px);
        let width = self.lane_width();
        let fps = self.cache.as_ref().map_or(60.0, |c| c.take.meta.fps());
        let step = tick_step(fpp, 70.0);
        let first_tick = (first / step as f64).floor() as i64;
        let ticks: Vec<(f32, u64)> = (first_tick..)
            .map(|i| i * step as i64)
            .filter(|f| *f >= 0)
            .map(|f| (((f as f64 - first) / fpp) as f32, f as u64))
            .take_while(|(x, _)| *x <= width)
            .collect();
        let markers: Vec<f32> = self
            .cache
            .as_ref()
            .map(|c| c.take.markers.iter().map(|m| ((m.frame as f64 - first) / fpp) as f32).collect())
            .unwrap_or_default();
        let (tick_color, marker_color, head_color) = (theme.border, theme.warning, theme.danger);
        let playhead = ((self.model.read(cx).playhead - first) / fpp) as f32;
        let paint_ticks: Vec<f32> = ticks.iter().map(|(x, _)| *x).collect();
        h_flex()
            .h(px(RULER_HEIGHT))
            .w_full()
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(px(LABEL_WIDTH)).h_full().px_2().text_xs().text_color(theme.muted_foreground).child("Channels"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.scrubbing = true;
                            this.scrub_to(event.position.x, cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                lane_bounds.set(bounds);
                                bounds
                            },
                            move |_, bounds, window, _| {
                                let (ox, oy, h) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32(), bounds.size.height.as_f32());
                                for x in &paint_ticks {
                                    stroke_polyline(window, &[pt(ox + x, oy + h * 0.55), pt(ox + x, oy + h)], 1.0, tick_color);
                                }
                                for x in &markers {
                                    stroke_polyline(window, &[pt(ox + x, oy), pt(ox + x, oy + h)], 2.0, marker_color);
                                }
                                stroke_polyline(window, &[pt(ox + playhead, oy), pt(ox + playhead, oy + h)], 2.0, head_color);
                            },
                        )
                        .size_full(),
                    )
                    .children(ticks.into_iter().map(|(x, frame)| {
                        div()
                            .absolute()
                            .left(px(x + 3.0))
                            .top_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{:.1}s", frame as f64 / fps))
                    })),
            )
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(cache) = &self.cache else { return Vec::new() };
        let theme = cx.theme().clone();
        let selected = self.model.read(cx).selected_channel;
        let playhead = self.model.read(cx).playhead;
        let colors = [theme.danger, theme.success, theme.info, theme.foreground.opacity(0.6), theme.accent];
        range
            .filter(|c| *c < cache.take.rig.channels.len())
            .map(|channel| {
                let lane = Lane {
                    take: Arc::clone(&cache.take),
                    channel,
                    range: cache.ranges[channel],
                    first_frame: self.first_frame,
                    frames_per_px: self.frames_per_px,
                    playhead,
                    colors,
                };
                let name = cache.take.rig.channels[channel].name.clone();
                h_flex()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .border_b_1()
                    .border_color(theme.border.opacity(0.4))
                    .when(selected == Some(channel), |row| row.bg(theme.accent.opacity(0.14)))
                    .child(
                        div()
                            .w(px(LABEL_WIDTH))
                            .h_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .text_xs()
                            .truncate()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.model.update(cx, |m, cx| {
                                        m.selected_channel = Some(channel);
                                        cx.notify();
                                    });
                                }),
                            )
                            .child(name),
                    )
                    .child(canvas(|bounds, _, _| bounds, move |_, bounds, window, _| paint_lane(bounds, &lane, window)).flex_1().h_full())
                    .into_any_element()
            })
            .collect()
    }
}

impl EventEmitter<PanelEvent> for TimelinePanel {}

impl Focusable for TimelinePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TimelinePanel {
    fn panel_name(&self) -> &'static str {
        "mocap-timeline"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Timeline").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_cache(cx);
        let theme = cx.theme().clone();
        let channel_count = self.cache.as_ref().map_or(0, |c| c.take.rig.channels.len());
        let item_sizes = Rc::new(vec![size(px(0.0), px(ROW_HEIGHT)); channel_count]);
        let view = cx.entity();
        let body = if channel_count == 0 {
            let status = self.model.read(cx).status.clone();
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if status.is_empty() { "Record a take or open a .mocap file to see its channels".to_owned() } else { status })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    v_virtual_list(view, "mocap-timeline-rows", item_sizes, |this, range, _window, cx| this.rows(range, cx))
                        .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.scrubbing && event.dragging() {
                    this.scrub_to(event.position.x, cx);
                } else if this.scrubbing {
                    this.scrubbing = false;
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.scrubbing = false))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let (dx, dy) = match event.delta {
                    ScrollDelta::Pixels(p) => (p.x.as_f32(), p.y.as_f32()),
                    ScrollDelta::Lines(l) => (l.x * 20.0, l.y * 20.0),
                };
                if event.modifiers.control || event.modifiers.platform {
                    let anchor = (event.position.x - this.lane_bounds.get().origin.x).as_f32();
                    this.zoom_around((1.0 - f64::from(dy) * 0.003).clamp(0.5, 2.0), anchor);
                    cx.notify();
                } else if dx.abs() > dy.abs() {
                    this.first_frame = (this.first_frame - f64::from(dx) * this.frames_per_px).clamp(0.0, this.frames());
                    cx.notify();
                }
            }))
            .child(self.toolbar(cx))
            .child(self.ruler(cx))
            .child(body)
    }
}
