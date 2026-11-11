use gpui::prelude::FluentBuilder;
use gpui::{uniform_list, *};
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, StyledExt, TitleBar,
};

use crate::components::{render_empty_state, render_plugin_details, render_plugin_item};
use crate::handlers;
use crate::model::{Entry, Extensions, Filter, Sort};

/// The Extensions viewer: every installed plugin, loaded plugin library and
/// the engine's own registrations, with what each contributes and the
/// contracts it provides and requires, all from one catalog snapshot
/// ([`crate::model::Extensions`]).
pub struct PluginManagerWindow {
    pub(crate) model: Extensions,
    pub(crate) selected: Option<String>,
    visible: Vec<usize>,
    search_input: Entity<InputState>,
    search_query: String,
    filter: Filter,
    sort: Sort,
    /// What the shown data was loaded from: the plugin manager's revision
    /// and the project's enabled set (`pulsar_plugins::generation`). The
    /// window reloads when either changes ([`PluginManagerWindow::reload_if_changed`]).
    loaded_from: (usize, u64),
    _search_subscription: Subscription,
    pub(crate) focus_handle: FocusHandle,
}

/// The revision of what the window shows: the plugin manager's plugin set
/// and the project's enabled plugins.
fn current_revision() -> (usize, u64) {
    let installed = plugin_manager::global().map_or(0, |pm| pm.read().revision());
    (installed, pulsar_plugins::generation())
}

/// The catalog now, with the plugin libraries the manager loaded.
fn load() -> Extensions {
    let catalog = match plugin_manager::global() {
        Some(manager) => manager.read().catalog(),
        None => plugin_manager::CatalogSnapshot::snapshot(),
    };
    Extensions::from_snapshot(&catalog)
}

impl PluginManagerWindow {
    pub fn new_global(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search extensions, contributions and contracts…")
        });
        let search_subscription =
            cx.subscribe(&search_input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_query = input.read(cx).text().to_string();
                    this.rebuild_visible();
                    cx.notify();
                }
            });
        let mut this = Self {
            model: Extensions::default(),
            selected: None,
            visible: Vec::new(),
            search_input,
            search_query: String::new(),
            filter: Filter::All,
            sort: Sort::Name,
            loaded_from: (0, 0),
            _search_subscription: search_subscription,
            focus_handle: cx.focus_handle(),
        };
        this.reload();
        this
    }

    fn rebuild_visible(&mut self) {
        self.visible = self
            .model
            .visible(self.filter, &self.search_query, self.sort);
        let shown = |id: &String| {
            self.visible
                .iter()
                .any(|index| &self.model.entries[*index].id == id)
        };
        if !self.selected.as_ref().is_some_and(shown) {
            self.selected = self
                .visible
                .first()
                .map(|index| self.model.entries[*index].id.clone());
        }
    }

    pub(crate) fn select(&mut self, entry: Option<String>, cx: &mut Context<Self>) {
        self.selected = entry;
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.rebuild_visible();
        cx.notify();
    }

    fn toggle_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Sort::Name => Sort::Contributions,
            Sort::Contributions => Sort::Name,
        };
        self.rebuild_visible();
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload();
        cx.notify();
    }

    /// Reload when the plugins changed since the data was loaded: a plugin
    /// library finished installing after the window opened, or another
    /// project (with other plugins enabled) was opened. Checked each time the
    /// window draws; both counters are atomic reads.
    fn reload_if_changed(&mut self) {
        if current_revision() != self.loaded_from {
            self.reload();
        }
    }

    fn reload(&mut self) {
        self.loaded_from = current_revision();
        self.model = load();
        self.rebuild_visible();
    }
}

impl Focusable for PluginManagerWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PluginManagerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reload_if_changed();
        let total = self.model.entries.len();
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.model.entry(id))
            .cloned();
        let search_input = self.search_input.clone();
        let filter = self.filter;
        let sort = self.sort;
        let counts = Filter::ALL.map(|filter| (filter, self.model.count(filter)));

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Puzzle)
                                .size(px(15.))
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child("Extensions"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(cx.theme().secondary.opacity(0.45))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{total} installed")),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("refresh-plugins")
                                .icon(IconName::Refresh)
                                .ghost()
                                .xsmall()
                                .tooltip("Refresh the plugin catalog")
                                .on_click(cx.listener(handlers::on_refresh)),
                        ),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(render_page_header(cx))
                    .child(render_extension_toolbar(
                        &search_input,
                        filter,
                        sort,
                        &counts,
                        cx,
                    ))
                    .when(!self.model.unattributed_problems.is_empty(), |this| {
                        this.child(render_problem_banner(&self.model.unattributed_problems, cx))
                    })
                    .child(render_extension_workspace(self, selected.as_ref(), cx))
                    .child(render_footer(self.visible.len(), total, cx)),
            )
    }
}

fn render_page_header(cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    v_flex()
        .w_full()
        .flex_shrink_0()
        .px_5()
        .pt_3()
        .child(
            div()
                .text_lg()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child("Extensions"),
        )
        .child(
            div()
                .pt_1()
                .pb_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Every plugin, its render features and nodes, settings, editors and the \
                     contracts it provides and requires, read from what is registered.",
                ),
        )
        .border_b_1()
        .border_color(cx.theme().border)
}

fn render_extension_toolbar(
    search_input: &Entity<InputState>,
    filter: Filter,
    sort: Sort,
    counts: &[(Filter, usize)],
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let buttons = counts.iter().map(|(option, count)| {
        let option = *option;
        Button::new(SharedString::from(format!("filter-{}", option.label())))
            .small()
            .ghost()
            .selected(filter == option)
            .label(format!("{} · {count}", option.label()))
            .when(option == Filter::NeedsAttention && *count > 0, |button| {
                button.text_color(cx.theme().warning)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(option, cx)))
    });
    h_flex()
        .w_full()
        .flex_shrink_0()
        .px_4()
        .py_2()
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.8))
        .child(
            div().w(px(300.)).child(
                TextInput::new(search_input).small().cleanable().prefix(
                    Icon::new(IconName::Search)
                        .size(px(14.))
                        .text_color(cx.theme().muted_foreground),
                ),
            ),
        )
        .children(buttons)
        .child(div().flex_1())
        .child(
            Button::new("extension-sort")
                .small()
                .ghost()
                .icon(IconName::List)
                .label(match sort {
                    Sort::Name => "Name",
                    Sort::Contributions => "Contributions",
                })
                .tooltip("Toggle sort order")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sort(cx))),
        )
}

/// Problems that name no entry (a registration from an unknown origin).
fn render_problem_banner(
    problems: &[String],
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .flex_shrink_0()
        .px_4()
        .py_2()
        .gap_1()
        .bg(cx.theme().warning.opacity(0.1))
        .border_b_1()
        .border_color(cx.theme().border)
        .children(problems.iter().map(|problem| {
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size_3()
                        .text_color(cx.theme().warning),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(problem.clone()),
                )
        }))
}

fn render_extension_workspace(
    this: &mut PluginManagerWindow,
    selected: Option<&Entry>,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let total_count = this.model.entries.len();
    let visible_count = this.visible.len();
    let model = this.model.clone();

    h_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .child(
            v_flex()
                .w(px(360.))
                .flex_shrink_0()
                .h_full()
                .overflow_hidden()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .w_full()
                        .h(px(36.))
                        .flex_shrink_0()
                        .px_3()
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.7))
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("EXTENSIONS"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{visible_count} / {total_count}")),
                        ),
                )
                .child(if visible_count == 0 {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::Search).size_5())
                        .child("No extensions match this search")
                        .into_any_element()
                } else {
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(
                            uniform_list(
                                "plugin-manager-list",
                                visible_count,
                                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                    range
                                        .filter_map(|visible_index| {
                                            this.visible
                                                .get(visible_index)
                                                .and_then(|index| this.model.entries.get(*index))
                                        })
                                        .map(|entry| {
                                            let is_selected =
                                                this.selected.as_ref() == Some(&entry.id);
                                            div()
                                                .w_full()
                                                .h(px(68.))
                                                .px_2()
                                                .child(render_plugin_item(entry, is_selected, cx))
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .size_full(),
                        )
                        .into_any_element()
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .when_some(selected, |this, selected| {
                    this.child(render_plugin_details(selected, &model, cx))
                })
                .when(selected.is_none(), |this| {
                    this.child(render_empty_state(cx))
                }),
        )
}

fn render_footer(
    visible_count: usize,
    total: usize,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(26.))
        .flex_shrink_0()
        .px_3()
        .items_center()
        .justify_between()
        .border_t_1()
        .border_color(cx.theme().border.opacity(0.75))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(format!(
            "{visible_count} of {total} shown · a project enables plugins in its Pulsar.toml"
        ))
        .child("Plugin libraries stay loaded until the editor restarts")
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for PluginManagerWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "PluginManagerWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1140.0, 800.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(|cx| PluginManagerWindow::new_global(window, cx))
    }
}
