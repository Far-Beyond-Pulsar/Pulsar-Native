//! The Build Configurations window.
//!
//! A searchable list of the project's configurations on the left; the selected
//! one is edited on the right. Edits apply as you make them and are saved a
//! moment after the last one, so there is no Save button to forget.
//!
//! Layout of the code: this file is the window (state, actions, sidebar);
//! [`form`] holds the general / profile / advanced sections, [`platforms`] the
//! platform picker and [`steps`] the step toggles with the live pipeline
//! preview.

mod form;
mod platforms;
mod steps;

use std::collections::HashSet;
use std::time::Duration;

use engine_state::build_config::{
    BuildConfiguration, ConfigId, PlatformFamily, build_configurations, ensure_loaded, persist,
};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use ui::button::{Button, ButtonVariants as _};
use ui::input::{InputEvent, InputState, TextInput};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, TitleBar, h_flex, v_flex,
};

use crate::picker::config_icon;

/// How long after the last edit the configurations are written to disk.
const SAVE_DELAY: Duration = Duration::from_millis(350);
/// How long "Click again to delete" stays armed.
const DELETE_ARM: Duration = Duration::from_secs(3);

/// Colours the sections share, copied out of the theme so render code can use
/// them while it holds `&mut` borrows.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub bg: gpui::Hsla,
    pub card: gpui::Hsla,
    pub border: gpui::Hsla,
    pub muted: gpui::Hsla,
    pub primary: gpui::Hsla,
    pub danger: gpui::Hsla,
    pub warning: gpui::Hsla,
    pub hover: gpui::Hsla,
    pub active: gpui::Hsla,
}

impl Palette {
    fn of(cx: &App) -> Self {
        let t = cx.theme();
        Self {
            bg: t.background,
            card: t.sidebar.opacity(0.45),
            border: t.border,
            muted: t.muted_foreground,
            primary: t.primary,
            danger: t.danger,
            warning: t.warning,
            hover: t.secondary,
            active: t.list_active,
        }
    }
}

pub struct BuildConfiguratorWindow {
    focus_handle: FocusHandle,
    /// The configuration shown on the right (not necessarily the active one).
    editing: Option<ConfigId>,
    /// The configuration the text inputs were last filled from.
    synced: Option<ConfigId>,

    list_search: Entity<InputState>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    features: Entity<InputState>,
    extra_args: Entity<InputState>,
    platform_search: Entity<InputState>,

    name_error: Option<String>,
    expanded: HashSet<PlatformFamily>,
    delete_armed: bool,

    persist_task: Option<Task<()>>,
    arm_task: Option<Task<()>>,
    list_scroll: ScrollHandle,
    list_scroll_state: ScrollbarState,
    form_scroll: ScrollHandle,
    form_scroll_state: ScrollbarState,
    platform_scroll: ScrollHandle,
    platform_scroll_state: ScrollbarState,

    _subscriptions: Vec<Subscription>,
    _watch: Task<()>,
}

impl Focusable for BuildConfiguratorWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for BuildConfiguratorWindow {
    fn drop(&mut self) {
        // Closing within the save delay must not lose the last edit.
        persist();
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for BuildConfiguratorWindow {
    type Params = ();

    fn window_name() -> &'static str {
        crate::picker::CONFIGURATOR_WINDOW
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1120.0, 780.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx))
    }
}

impl BuildConfiguratorWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_loaded();
        let editing = build_configurations()
            .read()
            .selected_id()
            .map(str::to_owned);

        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let list_search = input("Search configurations…", window, cx);
        let platform_search = input("Search platforms…", window, cx);
        let name = input("Configuration name", window, cx);
        let features = input("feature-a, feature-b", window, cx);
        let extra_args = input("--locked --jobs 8", window, cx);
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("What this configuration is for")
                .multi_line()
                .auto_grow(2, 5)
        });

        let changed = |this: &mut Self,
                       field: Field,
                       input: &Entity<InputState>,
                       event: &InputEvent,
                       window: &mut Window,
                       cx: &mut Context<Self>| match event {
            InputEvent::Change => this.field_changed(field, input, cx),
            InputEvent::Blur if matches!(field, Field::Name) => this.restore_name(window, cx),
            _ => {}
        };
        let subscriptions = vec![
            cx.subscribe_in(&name, window, move |t, i, e: &InputEvent, w, c| {
                changed(t, Field::Name, i, e, w, c)
            }),
            cx.subscribe_in(&description, window, move |t, i, e: &InputEvent, w, c| {
                changed(t, Field::Description, i, e, w, c)
            }),
            cx.subscribe_in(&features, window, move |t, i, e: &InputEvent, w, c| {
                changed(t, Field::Features, i, e, w, c)
            }),
            cx.subscribe_in(&extra_args, window, move |t, i, e: &InputEvent, w, c| {
                changed(t, Field::ExtraArgs, i, e, w, c)
            }),
            // Typing in either search box filters its list.
            cx.observe(&list_search, |_, _, cx| cx.notify()),
            cx.observe(&platform_search, |_, _, cx| cx.notify()),
        ];

        // Keep the window in step with the store (the picker can change the
        // active configuration, another window could edit).
        let store = build_configurations();
        let watch = cx.spawn_in(window, async move |this, cx| {
            let mut seen = store.version();
            loop {
                let changed = store.changed();
                if store.version() == seen {
                    changed.await;
                }
                seen = store.version();
                if this
                    .update_in(cx, |this, window, cx| this.store_changed(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            editing,
            synced: None,
            list_search,
            name,
            description,
            features,
            extra_args,
            platform_search,
            name_error: None,
            expanded: [
                PlatformFamily::Windows,
                PlatformFamily::Linux,
                PlatformFamily::MacOs,
            ]
            .into_iter()
            .collect(),
            delete_armed: false,
            persist_task: None,
            arm_task: None,
            list_scroll: ScrollHandle::new(),
            list_scroll_state: ScrollbarState::default(),
            form_scroll: ScrollHandle::new(),
            form_scroll_state: ScrollbarState::default(),
            platform_scroll: ScrollHandle::new(),
            platform_scroll_state: ScrollbarState::default(),
            _subscriptions: subscriptions,
            _watch: watch,
        };
        this.sync_inputs(window, cx);
        this
    }

    // ── State ────────────────────────────────────────────────────────────────

    /// A copy of the configuration being edited.
    pub(crate) fn current(&self) -> Option<BuildConfiguration> {
        let id = self.editing.as_deref()?;
        build_configurations().read().get(id).cloned()
    }

    /// Edit the configuration being edited and schedule saving.
    pub(crate) fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut BuildConfiguration)) {
        let Some(id) = self.editing.clone() else {
            return;
        };
        build_configurations().update(|store| store.edit(&id, f));
        self.schedule_persist(cx);
        cx.notify();
    }

    fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            _ = this.update(cx, |this, _| {
                this.persist_task = None;
                persist();
            });
        }));
    }

    /// Show `id` on the right and load its values into the inputs.
    fn set_editing(&mut self, id: ConfigId, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(id);
        self.delete_armed = false;
        self.name_error = None;
        self.sync_inputs(window, cx);
        cx.notify();
    }

    /// Fill the text inputs from the configuration being edited, once per
    /// switch of configuration (never while typing, which would fight the
    /// cursor).
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.synced == self.editing {
            return;
        }
        self.synced = self.editing.clone();
        let config = self.current();
        let text = |f: fn(&BuildConfiguration) -> &str| {
            config.as_ref().map(|c| f(c).to_owned()).unwrap_or_default()
        };
        let values = [
            (&self.name, text(|c| &c.name)),
            (&self.description, text(|c| &c.description)),
            (&self.features, text(|c| &c.features)),
            (&self.extra_args, text(|c| &c.extra_args)),
        ];
        for (input, value) in values {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    fn store_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = build_configurations();
        let gone = self
            .editing
            .as_deref()
            .is_none_or(|id| store.read().get(id).is_none());
        if gone {
            self.editing = store
                .read()
                .selected_id()
                .map(str::to_owned)
                .or_else(|| store.read().configs().first().map(|c| c.id.clone()));
            self.name_error = None;
            self.delete_armed = false;
        }
        self.sync_inputs(window, cx);
        cx.notify();
    }

    fn field_changed(&mut self, field: Field, input: &Entity<InputState>, cx: &mut Context<Self>) {
        let text = input.read(cx).value().to_string();
        let Some(config) = self.current() else { return };
        match field {
            Field::Name => {
                let trimmed = text.trim();
                let error = if trimmed.is_empty() {
                    Some("A configuration needs a name.".to_owned())
                } else if build_configurations()
                    .read()
                    .name_taken(trimmed, Some(&config.id))
                {
                    Some("Another configuration already uses this name.".to_owned())
                } else {
                    None
                };
                self.name_error = error;
                if self.name_error.is_none() && trimmed != config.name {
                    let new = trimmed.to_owned();
                    self.edit(cx, |c| c.name = new);
                } else {
                    cx.notify();
                }
            }
            Field::Description if text != config.description => {
                self.edit(cx, |c| c.description = text)
            }
            Field::Features if text != config.features => self.edit(cx, |c| c.features = text),
            Field::ExtraArgs if text != config.extra_args => self.edit(cx, |c| c.extra_args = text),
            _ => {}
        }
    }

    /// Leaving the name field with an invalid name puts the saved one back.
    fn restore_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_error.take().is_none() {
            return;
        }
        if let Some(config) = self.current() {
            self.name
                .update(cx, |input, cx| input.set_value(config.name, window, cx));
        }
        cx.notify();
    }

    // ── Actions ──────────────────────────────────────────────────────────────

    fn new_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = build_configurations().update(|store| store.add_new());
        self.schedule_persist(cx);
        self.set_editing(id, window, cx);
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else {
            return;
        };
        if let Some(copy) = build_configurations().update(|store| store.duplicate(&id)) {
            self.schedule_persist(cx);
            self.set_editing(copy, window, cx);
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else {
            return;
        };
        if !self.delete_armed {
            // Two clicks, so a stray one cannot lose a configuration.
            self.delete_armed = true;
            self.arm_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DELETE_ARM).await;
                _ = this.update(cx, |this, cx| {
                    this.delete_armed = false;
                    cx.notify();
                });
            }));
            cx.notify();
            return;
        }
        self.delete_armed = false;
        self.arm_task = None;
        build_configurations().update(|store| store.remove(&id));
        self.schedule_persist(cx);
        self.store_changed(window, cx);
    }

    fn make_active(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else {
            return;
        };
        build_configurations().update(|store| store.select(&id));
        self.schedule_persist(cx);
        cx.notify();
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    fn render_sidebar(&mut self, p: Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.list_search.read(cx).value().to_string();
        let (matches, total, active) = {
            let store = build_configurations();
            let store = store.read();
            let matches: Vec<BuildConfiguration> =
                store.search(&query).into_iter().cloned().collect();
            (
                matches,
                store.configs().len(),
                store.selected_id().map(str::to_owned),
            )
        };
        let editing = self.editing.clone();

        v_flex()
            .w(px(300.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(p.border)
            .bg(p.card)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Configurations"),
                    )
                    .child(
                        Button::new("bc-new")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label("New")
                            .tooltip("Create a build configuration")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_configuration(window, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .px_2()
                    .pb_2()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(14.))
                            .text_color(p.muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(TextInput::new(&self.list_search).small()),
                    ),
            )
            .child(
                div()
                    .id("bc-list")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .id("bc-list-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.list_scroll)
                            .py_1()
                            .when(matches.is_empty(), |el| {
                                let text = if total == 0 {
                                    "No configurations yet.".to_owned()
                                } else {
                                    format!("No configurations match “{query}”.")
                                };
                                el.child(
                                    div()
                                        .px_4()
                                        .py_4()
                                        .text_sm()
                                        .text_color(p.muted)
                                        .child(text),
                                )
                            })
                            .children(matches.iter().map(|config| {
                                let id = config.id.clone();
                                let is_editing = editing.as_deref() == Some(config.id.as_str());
                                let is_active = active.as_deref() == Some(config.id.as_str());
                                h_flex()
                                    .id(SharedString::from(format!("bc-row-{}", config.id)))
                                    .w_full()
                                    .px_3()
                                    .py(px(7.))
                                    .gap_3()
                                    .items_center()
                                    .cursor_pointer()
                                    .bg(if is_editing {
                                        p.active
                                    } else {
                                        p.card.opacity(0.0)
                                    })
                                    .hover(|s| s.bg(p.hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.set_editing(id.clone(), window, cx)
                                        }),
                                    )
                                    .child(
                                        Icon::new(config_icon(config))
                                            .size(px(16.))
                                            .text_color(p.muted),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(config.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.muted)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(config.subtitle()),
                                            ),
                                    )
                                    .when(is_active, |el| el.child(active_badge(p)))
                            })),
                    )
                    .child(Scrollbar::vertical(
                        &self.list_scroll_state,
                        &self.list_scroll,
                    )),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted)
                    .child(if query.is_empty() {
                        format!("{total} configuration{}", if total == 1 { "" } else { "s" })
                    } else {
                        format!("{} of {total}", matches.len())
                    }),
            )
            .into_any_element()
    }

    fn render_empty(&mut self, p: Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::Hammer)
                    .size(px(40.))
                    .text_color(p.muted),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("No build configurations"),
            )
            .child(div().text_sm().text_color(p.muted).child(
                "A configuration says how to build: the Rust mode, the platforms and the steps.",
            ))
            .child(
                Button::new("bc-empty-new")
                    .primary()
                    .icon(IconName::Plus)
                    .label("Create a configuration")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_configuration(window, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_editor(
        &mut self,
        config: BuildConfiguration,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_active = build_configurations().read().selected_id() == Some(config.id.as_str());
        let armed = self.delete_armed;

        let header = h_flex()
            .px_6()
            .py_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(p.border)
            .child(
                Icon::new(config_icon(&config))
                    .size(px(20.))
                    .text_color(p.muted),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(config.name.clone()),
                    )
                    .child(div().text_xs().text_color(p.muted).child(config.subtitle())),
            )
            .child(if is_active {
                active_badge(p).into_any_element()
            } else {
                Button::new("bc-activate")
                    .small()
                    .label("Set as active")
                    .tooltip("Use this configuration for the Build button")
                    .on_click(cx.listener(|this, _, _, cx| this.make_active(cx)))
                    .into_any_element()
            })
            .child(
                Button::new("bc-duplicate")
                    .small()
                    .ghost()
                    .icon(IconName::Copy)
                    .label("Duplicate")
                    .on_click(cx.listener(|this, _, window, cx| this.duplicate(window, cx))),
            )
            .child(
                Button::new("bc-delete")
                    .small()
                    .ghost()
                    .icon(IconName::Trash)
                    .label(if armed {
                        "Click again to delete"
                    } else {
                        "Delete"
                    })
                    .selected(armed)
                    .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            );

        let body = v_flex()
            .gap_4()
            .p_6()
            .child(self.render_general(&config, p, cx))
            .child(self.render_profile(&config, p, cx))
            .child(self.render_platforms(&config, p, cx))
            .child(self.render_steps(&config, p, cx))
            .child(self.render_advanced(p, cx));

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(
                div()
                    .id("bc-form")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("bc-form-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.form_scroll)
                            .child(body),
                    )
                    .child(Scrollbar::vertical(
                        &self.form_scroll_state,
                        &self.form_scroll,
                    )),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    Description,
    Features,
    ExtraArgs,
}

fn active_badge(p: Palette) -> impl IntoElement {
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .text_xs()
        .text_color(p.primary)
        .bg(p.primary.opacity(0.14))
        .child("Active")
}

impl Render for BuildConfiguratorWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::of(cx);
        let config = self.current();
        let sidebar = self.render_sidebar(p, cx);
        let main = match config {
            Some(config) => self.render_editor(config, p, cx),
            None => self.render_empty(p, cx),
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(p.bg)
            .child(
                TitleBar::new()
                    .unified_background(p.bg)
                    .child("Build Configurations"),
            )
            .child(h_flex().flex_1().min_h_0().child(sidebar).child(main))
    }
}

/// Referenced from [`crate::init`] so the linker keeps this module, and with it
/// the window's `register_window` entry, in any binary that shows the Build
/// button.
#[inline(never)]
pub fn link_anchor() -> &'static str {
    <BuildConfiguratorWindow as window_manager::PulsarWindow>::window_name()
}
