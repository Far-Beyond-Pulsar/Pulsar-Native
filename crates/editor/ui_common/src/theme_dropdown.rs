use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, DismissEvent, Entity,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement, Styled as _, Window,
};
use ui::{
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, Theme, ThemeConfig,
    ThemeRegistry,
};

// ── ThemePicker ───────────────────────────────────────────────────────────────

pub struct ThemePicker {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
}

impl ThemePicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search themes…"));
        Self {
            focus_handle: cx.focus_handle(),
            search_input,
        }
    }
}

impl gpui::EventEmitter<DismissEvent> for ThemePicker {}

impl Focusable for ThemePicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Swatch helper ─────────────────────────────────────────────────────────────

fn parse_hex(s: Option<&SharedString>, fallback: Hsla) -> Hsla {
    s.and_then(|hex| gpui::Rgba::try_from(hex.as_ref()).ok().map(Hsla::from))
        .unwrap_or(fallback)
}

fn render_swatch(config: &Rc<ThemeConfig>) -> impl IntoElement {
    let is_dark = config.mode.is_dark();

    let bg = parse_hex(
        config.colors.background.as_ref(),
        if is_dark {
            gpui::hsla(0., 0., 0.1, 1.)
        } else {
            gpui::hsla(0., 0., 0.97, 1.)
        },
    );
    let border = parse_hex(
        config.colors.border.as_ref(),
        if is_dark {
            gpui::hsla(0., 0., 0.25, 1.)
        } else {
            gpui::hsla(0., 0., 0.82, 1.)
        },
    );
    let fg = parse_hex(
        config.colors.foreground.as_ref(),
        if is_dark {
            gpui::hsla(0., 0., 0.95, 1.)
        } else {
            gpui::hsla(0., 0., 0.05, 1.)
        },
    );

    div()
        .w(px(36.))
        .h(px(24.))
        .rounded_md()
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(3.))
        .px(px(4.))
        .flex_shrink_0()
        .child(div().h(px(2.)).rounded_full().bg(fg).w(px(22.)))
        .child(
            div()
                .h(px(2.))
                .rounded_full()
                .bg(fg.opacity(0.55))
                .w(px(16.)),
        )
        .child(
            div()
                .h(px(2.))
                .rounded_full()
                .bg(fg.opacity(0.3))
                .w(px(10.)),
        )
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for ThemePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read search query from input state.
        let query = self.search_input.read(cx).value().to_string();
        let query_lower = query.to_lowercase();

        let current_name = cx.theme().theme_name().clone();

        // Collect matching themes.
        let themes: Vec<Rc<ThemeConfig>> = ThemeRegistry::global(cx)
            .sorted_themes()
            .into_iter()
            .filter(|t| query_lower.is_empty() || t.name.to_lowercase().contains(&query_lower))
            .cloned()
            .collect();

        let is_empty = themes.is_empty();

        // Snapshot theme colors for the panel chrome.
        let bg = cx.theme().background;
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let hover_bg = cx.theme().secondary;
        let active_bg = cx.theme().list_active;

        v_flex()
            .w(px(300.))
            .bg(bg)
            .rounded_xl()
            .shadow_xl()
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            // ── Search header ────────────────────────────────────────────────
            .child(
                h_flex()
                    .px_2()
                    .py(px(6.))
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .child(TextInput::new(&self.search_input).small()),
                    ),
            )
            // ── Scrollable list ──────────────────────────────────────────────
            .child(
                div()
                    .id("theme-picker-list")
                    .max_h(px(400.))
                    .overflow_y_scroll()
                    .py_1()
                    // Empty state
                    .when(is_empty, |el| {
                        el.child(
                            div()
                                .px_4()
                                .py_4()
                                .text_sm()
                                .text_color(muted)
                                .child("No themes match your search."),
                        )
                    })
                    // Theme rows
                    .children(themes.into_iter().map(|config| {
                        let name = config.name.clone();
                        let is_active = name == current_name;
                        let is_dark = config.mode.is_dark();
                        let name_for_click = name.clone();

                        let mode_badge_bg = if is_dark {
                            gpui::hsla(0., 0., 0.15, 1.)
                        } else {
                            gpui::hsla(0., 0., 0.9, 1.)
                        };
                        let mode_badge_fg = if is_dark {
                            gpui::hsla(0., 0., 0.65, 1.)
                        } else {
                            gpui::hsla(0., 0., 0.4, 1.)
                        };

                        h_flex()
                            .id(SharedString::from(format!("theme-{}", name)))
                            .w_full()
                            .px_3()
                            .py(px(6.))
                            .gap_2()
                            .items_center()
                            .cursor_pointer()
                            .bg(if is_active { active_bg } else { bg })
                            .hover(|s| s.bg(hover_bg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    if let Some(cfg) = ThemeRegistry::global(cx)
                                        .themes()
                                        .get(&name_for_click)
                                        .cloned()
                                    {
                                        Theme::global_mut(cx).apply_config(&cfg);
                                        cx.refresh_windows();
                                    }
                                }),
                            )
                            // Swatch
                            .child(render_swatch(&config))
                            // Theme name
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(fg)
                                    .overflow_hidden()
                                    .child(name.to_string()),
                            )
                            // Dark / Light badge
                            .child(
                                div()
                                    .px(px(5.))
                                    .py(px(2.))
                                    .rounded_full()
                                    .bg(mode_badge_bg)
                                    .text_xs()
                                    .text_color(mode_badge_fg)
                                    .child(if is_dark { "Dark" } else { "Light" }),
                            )
                            // Active indicator
                            .when(is_active, |el| {
                                el.child(Icon::new(IconName::Check).size(px(14.)).text_color(fg))
                            })
                    })),
            )
    }
}
