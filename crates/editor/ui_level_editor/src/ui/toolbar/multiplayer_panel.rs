//! Quick multiplayer configuration for Play-In-Editor.
//!
//! A popover on the global toolbar. It edits
//! `engine_state::playback::PlaybackState::multiplayer` directly; like the rest
//! of the global toolbar it knows nothing about which editor will run the
//! session.

use engine_state::playback::{
    MAX_PLAYERS, MultiplayerMode, MultiplayerSettings, PlaybackState, playback,
};
use gpui::*;
use ui::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};

const TICK_RATES: [u16; 4] = [20, 30, 60, 120];

/// The popover content view.
pub struct MultiplayerPanel {
    focus_handle: FocusHandle,
    /// Holds `players - 1`: the slider thumb maths assumes a 0 minimum.
    players: Entity<SliderState>,
    latency: Entity<SliderState>,
    packet_loss: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
    _watch: Task<()>,
}

impl EventEmitter<DismissEvent> for MultiplayerPanel {}

impl Focusable for MultiplayerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MultiplayerPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let initial = playback().get().multiplayer;
        let players = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max((MAX_PLAYERS - 1) as f32)
                .step(1.)
                .default_value((initial.players.max(1) - 1) as f32)
        });
        let latency = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(500.)
                .step(10.)
                .default_value(initial.latency_ms as f32)
        });
        let packet_loss = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(30.)
                .step(1.)
                .default_value(initial.packet_loss_pct as f32)
        });

        let subscriptions = vec![
            cx.subscribe(&players, |_, _, event: &SliderEvent, _| {
                let SliderEvent::Change(v) = event;
                let players = v.end().round() as u8 + 1;
                edit(|m| m.players = players.clamp(1, MAX_PLAYERS));
            }),
            cx.subscribe(&latency, |_, _, event: &SliderEvent, _| {
                let SliderEvent::Change(v) = event;
                edit(|m| m.latency_ms = v.end().round() as u16);
            }),
            cx.subscribe(&packet_loss, |_, _, event: &SliderEvent, _| {
                let SliderEvent::Change(v) = event;
                edit(|m| m.packet_loss_pct = v.end().round() as u8);
            }),
        ];

        let state = playback();
        let watch = cx.spawn(async move |this, cx| {
            let mut seen = state.version();
            loop {
                let changed = state.changed();
                if state.version() == seen {
                    changed.await;
                }
                seen = state.version();
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            players,
            latency,
            packet_loss,
            _subscriptions: subscriptions,
            _watch: watch,
        }
    }
}

fn edit(f: impl FnOnce(&mut MultiplayerSettings)) {
    playback().update(|s| f(&mut s.multiplayer));
}

fn mode_label(mode: MultiplayerMode) -> &'static str {
    match mode {
        MultiplayerMode::Standalone => "Standalone",
        MultiplayerMode::ListenServer => "Listen Server",
        MultiplayerMode::DedicatedServer => "Dedicated Server",
        MultiplayerMode::Client => "Client",
    }
}

fn mode_blurb(mode: MultiplayerMode) -> &'static str {
    match mode {
        MultiplayerMode::Standalone => "Single player, no networking",
        MultiplayerMode::ListenServer => "One player hosts; the rest join them",
        MultiplayerMode::DedicatedServer => "Headless server; every player is a client",
        MultiplayerMode::Client => "Join a running server",
    }
}

fn mode_icon(mode: MultiplayerMode) -> IconName {
    match mode {
        MultiplayerMode::Standalone => IconName::Gamepad,
        MultiplayerMode::ListenServer => IconName::Server,
        MultiplayerMode::DedicatedServer => IconName::Cpu,
        MultiplayerMode::Client => IconName::Network,
    }
}

/// Toolbar trigger label, e.g. `Listen · 3`.
pub fn summary(state: &PlaybackState) -> String {
    let m = &state.multiplayer;
    match m.mode {
        MultiplayerMode::Standalone => "Standalone".into(),
        MultiplayerMode::ListenServer => format!("Listen · {}", m.players),
        MultiplayerMode::DedicatedServer => format!("Dedicated · {}", m.players),
        MultiplayerMode::Client => "Client".into(),
    }
}

pub fn trigger_icon(state: &PlaybackState) -> IconName {
    mode_icon(state.multiplayer.mode)
}

fn row(label: &'static str, value: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .text_xs()
        .child(label)
        .child(div().text_color(muted).child(value))
}

impl Render for MultiplayerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = playback().get().multiplayer;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let count_applies = m.mode.has_player_count();
        let networked = m.mode != MultiplayerMode::Standalone;

        v_flex()
            .track_focus(&self.focus_handle)
            .w(px(300.))
            .p_3()
            .gap_3()
            .child(div().text_xs().text_color(muted).child("NET MODE"))
            .child(v_flex().gap_0p5().children(MultiplayerMode::ALL.map(|mode| {
                Button::new(SharedString::from(format!("net-mode-{mode:?}")))
                    .w_full()
                    .ghost()
                    .small()
                    .icon(mode_icon(mode))
                    .label(mode_label(mode))
                    .tooltip(mode_blurb(mode))
                    .selected(m.mode == mode)
                    .on_click(move |_, _, _| edit(|m| m.mode = mode))
            })))
            .child(div().h_px().w_full().bg(border))
            .child(
                v_flex()
                    .gap_1()
                    .opacity(if count_applies { 1.0 } else { 0.5 })
                    .child(row("Players", m.players.to_string(), muted))
                    .child(Slider::new(&self.players).disabled(!count_applies)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .opacity(if networked { 1.0 } else { 0.5 })
                    .child(row(
                        "Simulated latency",
                        format!("{} ms", m.latency_ms),
                        muted,
                    ))
                    .child(Slider::new(&self.latency).disabled(!networked))
                    .child(row(
                        "Packet loss",
                        format!("{}%", m.packet_loss_pct),
                        muted,
                    ))
                    .child(Slider::new(&self.packet_loss).disabled(!networked)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .opacity(if networked { 1.0 } else { 0.5 })
                    .child(div().text_xs().child("Server tick rate"))
                    .child(h_flex().gap_1().children(TICK_RATES.map(|hz| {
                        Button::new(SharedString::from(format!("tick-{hz}")))
                            .small()
                            .ghost()
                            .label(format!("{hz} Hz"))
                            .selected(m.tick_rate_hz == hz)
                            .on_click(move |_, _, _| edit(|m| m.tick_rate_hz = hz))
                    }))),
            )
            .child(
                Switch::new("net-separate-windows")
                    .label("Run each player in a separate window")
                    .checked(m.separate_windows)
                    .on_click(|checked, _, _| edit(|m| m.separate_windows = *checked)),
            )
    }
}
