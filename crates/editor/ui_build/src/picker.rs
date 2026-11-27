//! The dropdown behind the Build button: a searchable list of build
//! configurations, with a fixed "Edit Configurations…" button below it.
//!
//! The list scrolls; the footer does not, so the way to the configurator is
//! always in the same place however many configurations there are.

use engine_state::build_config::{
    BuildConfiguration, PLAY_IN_EDITOR_ID, build_configurations, ensure_loaded, persist,
};
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    UpdateGlobal as _, Window, div, prelude::FluentBuilder as _, px,
};
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
use ui::input::{InputEvent, InputState, TextInput};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

pub const CONFIGURATOR_WINDOW: &str = "BuildConfiguratorWindow";

/// Whether the search `query` matches the built-in Play in Editor entry, by
/// the same rule as a configuration's.
fn play_in_editor_matches(query: &str) -> bool {
    let name = t!("Build.Button.PlayInEditor").to_lowercase();
    query
        .split_whitespace()
        .all(|word| name.contains(&word.to_lowercase()))
}

/// An icon for what a configuration mainly does.
pub fn config_icon(config: &BuildConfiguration) -> IconName {
    let steps = config.steps;
    if steps.run {
        IconName::Play
    } else if steps.build {
        IconName::Hammer
    } else if steps.check {
        IconName::Check
    } else {
        IconName::Refresh
    }
}

pub struct BuildPicker {
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
    /// Row the keyboard is on, an index into the filtered list.
    highlighted: usize,
    _subscriptions: Vec<Subscription>,
    _watch: Task<()>,
}

impl EventEmitter<DismissEvent> for BuildPicker {}

impl Focusable for BuildPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BuildPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_loaded();
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("Build.Search.Configurations").to_string())
        });

        let subscription = cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => {
                    this.highlighted = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.choose_highlighted(cx),
                _ => {}
            },
        );

        let store = build_configurations();
        let watch = cx.spawn(async move |this, cx| {
            let mut seen = store.version();
            loop {
                let changed = store.changed();
                if store.version() == seen {
                    changed.await;
                }
                seen = store.version();
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            search,
            scroll_handle: ScrollHandle::new(),
            scroll_state: ScrollbarState::default(),
            highlighted: 0,
            _subscriptions: vec![subscription],
            _watch: watch,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().to_string()
    }

    fn filtered(&self, cx: &App) -> Vec<BuildConfiguration> {
        let query = self.query(cx);
        build_configurations()
            .read()
            .search(&query)
            .into_iter()
            .cloned()
            .collect()
    }

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        build_configurations().update(|store| store.select(id));
        persist();
        cx.emit(DismissEvent);
    }

    /// The rows the search matches, in order: Play in Editor, then the
    /// configurations.
    fn rows(&self, cx: &App) -> Vec<String> {
        let built_in = play_in_editor_matches(&self.query(cx))
            .then(|| PLAY_IN_EDITOR_ID.to_owned());
        built_in
            .into_iter()
            .chain(self.filtered(cx).into_iter().map(|config| config.id))
            .collect()
    }

    fn choose_highlighted(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        if let Some(id) = rows.get(self.highlighted.min(rows.len().saturating_sub(1))) {
            let id = id.clone();
            self.select(&id, cx);
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        if rows.is_empty() {
            return;
        }
        let len = rows.len() as isize;
        self.highlighted = (self.highlighted as isize + delta).rem_euclid(len) as usize;
        // The built-in row stays above the scrolling list.
        let fixed = usize::from(rows[0] == PLAY_IN_EDITOR_ID);
        if let Some(listed) = self.highlighted.checked_sub(fixed) {
            self.scroll_handle.scroll_to_item(listed);
        }
        cx.notify();
    }

    fn open_configurator(&mut self, cx: &mut Context<Self>) {
        window_manager::WindowRegistry::update_global(cx, |registry, cx| {
            registry.open(CONFIGURATOR_WINDOW, cx)
        });
        cx.emit(DismissEvent);
    }
}

impl Render for BuildPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered = self.filtered(cx);
        let rows = self.rows(cx);
        let highlighted = self.highlighted.min(rows.len().saturating_sub(1));
        let highlighted_id = rows.get(highlighted).cloned();
        let selected = {
            let store = build_configurations();
            let configs = store.read();
            if configs.plays_in_editor() {
                Some(PLAY_IN_EDITOR_ID.to_owned())
            } else {
                configs.selected_id().map(str::to_owned)
            }
        };
        let query = self.query(cx);
        let total = build_configurations().read().configs().len();

        let theme = cx.theme();
        let (bg, border, fg, muted) = (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let (hover_bg, active_bg) = (theme.secondary, theme.list_active);

        v_flex()
            .w(px(380.))
            .bg(bg)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "down" => this.move_highlight(1, cx),
                    "up" => this.move_highlight(-1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            // Search
            .child(
                h_flex()
                    .px_2()
                    .py(px(6.))
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                    .child(div().flex_1().child(TextInput::new(&self.search).small())),
            )
            // The built-in entry, above the list: not a configuration, so it
            // has nothing to edit or remove.
            .when(
                rows.first().is_some_and(|id| id == PLAY_IN_EDITOR_ID),
                |el| {
                    let is_selected = selected.as_deref() == Some(PLAY_IN_EDITOR_ID);
                    let is_highlighted = highlighted_id.as_deref() == Some(PLAY_IN_EDITOR_ID);
                    el.child(
                        h_flex()
                            .id("build-play-in-editor")
                            .w_full()
                            .px_3()
                            .py(px(6.))
                            .gap_3()
                            .items_center()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(border)
                            .bg(if is_highlighted {
                                hover_bg
                            } else if is_selected {
                                active_bg
                            } else {
                                bg
                            })
                            .hover(|s| s.bg(hover_bg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.select(PLAY_IN_EDITOR_ID, cx)),
                            )
                            .child(Icon::new(IconName::Play).size(px(16.)).text_color(muted))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(t!("Build.Button.PlayInEditor").to_string()),
                                    )
                                    .child(
                                        div().text_xs().text_color(muted).child(
                                            t!("Build.Picker.PlayInEditorHint").to_string(),
                                        ),
                                    ),
                            )
                            .when(is_selected, |el| {
                                el.child(Icon::new(IconName::Check).size(px(14.)).text_color(fg))
                            }),
                    )
                },
            )
            // Scrolling list
            .child(
                div()
                    .id("build-picker-list")
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("build-picker-scroll")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .py_1()
                            .when(filtered.is_empty(), |el| {
                                let message = if total == 0 {
                                    t!("Build.Picker.Empty").to_string()
                                } else {
                                    t!("Build.Search.NoConfigurationsMatch", query = query)
                                        .to_string()
                                };
                                el.child(
                                    div()
                                        .px_4()
                                        .py_4()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(message),
                                )
                            })
                            .children(filtered.iter().map(|config| {
                                let id = config.id.clone();
                                let is_selected = selected.as_deref() == Some(config.id.as_str());
                                let is_highlighted =
                                    highlighted_id.as_deref() == Some(config.id.as_str());
                                h_flex()
                                    .id(SharedString::from(format!("build-config-{}", config.id)))
                                    .w_full()
                                    .px_3()
                                    .py(px(6.))
                                    .gap_3()
                                    .items_center()
                                    .cursor_pointer()
                                    .bg(if is_highlighted {
                                        hover_bg
                                    } else if is_selected {
                                        active_bg
                                    } else {
                                        bg
                                    })
                                    .hover(|s| s.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| this.select(&id, cx)),
                                    )
                                    .child(
                                        Icon::new(config_icon(config))
                                            .size(px(16.))
                                            .text_color(muted),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(fg)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(config.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(crate::text::subtitle(config)),
                                            ),
                                    )
                                    .when(is_selected, |el| {
                                        el.child(
                                            Icon::new(IconName::Check).size(px(14.)).text_color(fg),
                                        )
                                    })
                            })),
                    )
                    .child(Scrollbar::vertical(&self.scroll_state, &self.scroll_handle)),
            )
            // Fixed footer, outside the scrolling area
            .child(
                v_flex().border_t_1().border_color(border).p_1().child(
                    Button::new("build-picker-edit")
                        .w_full()
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .label(t!("Build.Picker.Edit").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.open_configurator(cx))),
                ),
            )
    }
}
