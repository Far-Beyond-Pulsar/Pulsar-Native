//! Quick multiplayer configuration for Play-In-Editor.
//!
//! A popover on the global toolbar. It edits
//! `engine_state::playback::PlaybackState::multiplayer` directly; like the rest
//! of the global toolbar it knows nothing about which editor will run the
//! session.

use engine_state::playback::{
    playback, MultiplayerMode, MultiplayerSettings, PlaybackState, MAX_PLAYERS,
};
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::{Scrollbar, ScrollbarState},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
};

const TICK_RATES: [u16; 4] = [20, 30, 60, 120];

/// The popover content view.
pub struct MultiplayerPanel {
    focus_handle: FocusHandle,
    /// Holds `players - 1`: the slider thumb maths assumes a 0 minimum.
    players: Entity<SliderState>,
    latency: Entity<SliderState>,
    packet_loss: Entity<SliderState>,
    scroll: ScrollHandle,
    scroll_state: ScrollbarState,
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
            scroll: ScrollHandle::new(),
            scroll_state: ScrollbarState::default(),
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

/// Network-condition presets: label, one-way latency in ms, packet loss in %.
const NETWORK_PRESETS: [(&str, u16, u8); 4] = [
    ("None", 0, 0),
    ("Good", 30, 0),
    ("Typical", 80, 1),
    ("Poor", 200, 5),
];

/// One line saying what the session will be, shown at the foot of the panel.
pub fn describe(m: &MultiplayerSettings) -> String {
    let conditions = match (m.latency_ms, m.packet_loss_pct) {
        (0, 0) => "ideal network".to_string(),
        (ms, 0) => format!("{ms} ms"),
        (ms, loss) => format!("{ms} ms, {loss}% loss"),
    };
    match m.mode {
        MultiplayerMode::Standalone => "Standalone: single player, no networking".to_string(),
        MultiplayerMode::Client => format!("Client · {} Hz · {conditions}", m.tick_rate_hz),
        MultiplayerMode::ListenServer | MultiplayerMode::DedicatedServer => format!(
            "{} · {} player{} · {} Hz · {conditions}",
            mode_label(m.mode),
            m.players,
            if m.players == 1 { "" } else { "s" },
            m.tick_rate_hz,
        ),
    }
}

/// Colours the sections share, copied out of the theme.
#[derive(Clone, Copy)]
struct Palette {
    card: Hsla,
    border: Hsla,
    fg: Hsla,
    muted: Hsla,
    primary: Hsla,
    hover: Hsla,
}

impl Palette {
    fn of(cx: &App) -> Self {
        let t = cx.theme();
        Self {
            card: t.sidebar.opacity(0.45),
            border: t.border,
            fg: t.foreground,
            muted: t.muted_foreground,
            primary: t.primary,
            hover: t.secondary,
        }
    }
}

/// A titled card. When `enabled` is false it is dimmed and `why` replaces `note`.
fn section(
    title: &'static str,
    note: &'static str,
    enabled: bool,
    why: &'static str,
    p: Palette,
    content: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(if enabled { note } else { why }),
                ),
        )
        .child(
            div()
                .opacity(if enabled { 1.0 } else { 0.45 })
                .child(content),
        )
}

/// Label on the left, current value on the right.
fn value_row(label: &'static str, value: String, p: Palette) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .text_xs()
        .child(div().text_color(p.muted).child(label))
        .child(
            div()
                .text_color(p.fg)
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
}

/// A selectable card: the control the configurator uses for the Rust build mode.
fn choice(id: SharedString, selected: bool, p: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(if selected { p.primary } else { p.border })
        .bg(if selected {
            p.primary.opacity(0.09)
        } else {
            p.card.opacity(0.0)
        })
        .cursor_pointer()
        .hover(|s| s.bg(p.hover))
}

impl MultiplayerPanel {
    /// Move each slider to its setting when something else changed the setting
    /// (a preset, Reset). Dragging already agrees, so it is left alone.
    fn sync_sliders(&self, m: &MultiplayerSettings, window: &mut Window, cx: &mut Context<Self>) {
        let targets = [
            (&self.players, (m.players.max(1) - 1) as f32),
            (&self.latency, m.latency_ms as f32),
            (&self.packet_loss, m.packet_loss_pct as f32),
        ];
        for (slider, want) in targets {
            if (slider.read(cx).value().end() - want).abs() > 0.5 {
                slider.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
    }
}

impl Render for MultiplayerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = playback().get().multiplayer;
        self.sync_sliders(&m, window, cx);
        let p = Palette::of(cx);
        let count_applies = m.mode.has_player_count();
        let networked = m.mode != MultiplayerMode::Standalone;

        // Net mode: a 2 x 2 grid of cards.
        let mode_card = |mode: MultiplayerMode| {
            choice(
                SharedString::from(format!("net-mode-{mode:?}")),
                m.mode == mode,
                p,
            )
            .on_click(move |_, _, _| edit(|m| m.mode = mode))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(mode_icon(mode)).size(px(14.)).text_color(p.muted))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(mode_label(mode)),
                    ),
            )
            .child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(p.muted)
                    .child(mode_blurb(mode)),
            )
        };
        let [standalone, listen, dedicated, client] = MultiplayerMode::ALL;
        let modes = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_stretch()
                    .child(mode_card(standalone))
                    .child(mode_card(listen)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_stretch()
                    .child(mode_card(dedicated))
                    .child(mode_card(client)),
            );

        // Session: how many players, and where they run.
        let session = v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .opacity(if count_applies { 1.0 } else { 0.45 })
                    .child(value_row("Players", m.players.to_string(), p))
                    .child(Slider::new(&self.players).disabled(!count_applies)),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .child(div().text_sm().child("Separate windows"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted)
                                    .child("Give each player its own window"),
                            ),
                    )
                    .child(
                        Switch::new("net-separate-windows")
                            .checked(m.separate_windows)
                            .on_click(|checked, _, _| edit(|m| m.separate_windows = *checked)),
                    ),
            );

        // Simulated network: presets, then the two sliders they set.
        let presets = h_flex()
            .gap_1()
            .children(NETWORK_PRESETS.map(|(label, ms, loss)| {
                Button::new(SharedString::from(format!("net-preset-{label}")))
                    .small()
                    .label(label)
                    .selected((m.latency_ms, m.packet_loss_pct) == (ms, loss))
                    .on_click(move |_, _, _| {
                        edit(|m| {
                            m.latency_ms = ms;
                            m.packet_loss_pct = loss;
                        })
                    })
            }));
        let conditions = v_flex()
            .gap_3()
            .child(presets)
            .child(
                v_flex()
                    .gap_1()
                    .child(value_row("Latency", format!("{} ms", m.latency_ms), p))
                    .child(Slider::new(&self.latency).disabled(!networked)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(value_row(
                        "Packet loss",
                        format!("{}%", m.packet_loss_pct),
                        p,
                    ))
                    .child(Slider::new(&self.packet_loss).disabled(!networked)),
            );

        // Server tick rate.
        let tick = h_flex().gap_2().children(TICK_RATES.map(|hz| {
            choice(
                SharedString::from(format!("tick-{hz}")),
                m.tick_rate_hz == hz,
                p,
            )
            .on_click(move |_, _, _| edit(|m| m.tick_rate_hz = hz))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{hz} Hz")),
            )
        }));

        let sections = v_flex()
            .gap_3()
            .p_3()
            .child(section(
                "Net mode",
                "How this session joins the network",
                true,
                "",
                p,
                modes,
            ))
            .child(section(
                "Session",
                "Players and windows",
                networked,
                "Not used in Standalone",
                p,
                session,
            ))
            .child(section(
                "Network conditions",
                "Simulated on every connection",
                networked,
                "Not used in Standalone",
                p,
                conditions,
            ))
            .child(section(
                "Server tick rate",
                "Simulation steps per second",
                networked,
                "Not used in Standalone",
                p,
                tick,
            ));

        v_flex()
            .track_focus(&self.focus_handle)
            .w(px(400.))
            // Scrolls when the window is short; the summary below does not.
            .child(
                div()
                    .id("net-panel")
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("net-panel-scroll")
                            .max_h(px(560.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(sections),
                    )
                    .child(Scrollbar::vertical(&self.scroll_state, &self.scroll)),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(p.muted)
                            .child(describe(&m)),
                    )
                    .child(
                        Button::new("net-reset")
                            .small()
                            .ghost()
                            .label("Reset")
                            .tooltip("Back to Standalone with the default settings")
                            .on_click(|_, _, _| {
                                playback()
                                    .update(|s| s.multiplayer = MultiplayerSettings::default())
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: MultiplayerMode, players: u8, ms: u16, loss: u8) -> MultiplayerSettings {
        MultiplayerSettings {
            mode,
            players,
            tick_rate_hz: 60,
            separate_windows: false,
            latency_ms: ms,
            packet_loss_pct: loss,
        }
    }

    #[::core::prelude::v1::test]
    fn the_summary_reads_naturally_for_each_mode() {
        use MultiplayerMode::*;
        assert_eq!(
            describe(&settings(Standalone, 4, 80, 2)),
            "Standalone: single player, no networking"
        );
        assert_eq!(
            describe(&settings(ListenServer, 3, 0, 0)),
            "Listen Server · 3 players · 60 Hz · ideal network"
        );
        assert_eq!(
            describe(&settings(DedicatedServer, 1, 80, 0)),
            "Dedicated Server · 1 player · 60 Hz · 80 ms"
        );
        assert_eq!(
            describe(&settings(Client, 2, 200, 5)),
            "Client · 60 Hz · 200 ms, 5% loss"
        );
    }

    #[::core::prelude::v1::test]
    fn every_preset_is_within_the_slider_ranges() {
        for (_, ms, loss) in NETWORK_PRESETS {
            assert!(ms <= 500 && loss <= 30);
        }
    }
}
