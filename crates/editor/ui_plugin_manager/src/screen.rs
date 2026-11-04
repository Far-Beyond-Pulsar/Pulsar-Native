use gpui::prelude::FluentBuilder;
use gpui::{uniform_list, *};
use plugin_editor_api::PluginId;
use plugin_manager::{DependencyState, PluginDetails};
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, StyledExt, TitleBar,
};

use crate::components::{render_empty_state, render_plugin_details, render_plugin_item};
use crate::handlers;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegistryView {
    #[default]
    Extensions,
    Renderer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PluginFilter {
    #[default]
    All,
    BuiltIn,
    External,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PluginSort {
    #[default]
    Name,
    Contributions,
}

pub struct PluginManagerWindow {
    pub(crate) plugins: Vec<PluginDetails>,
    pub(crate) selected: Option<PluginId>,
    visible_plugin_indices: Vec<usize>,
    search_input: Entity<InputState>,
    search_query: String,
    filter: PluginFilter,
    sort: PluginSort,
    view: RegistryView,
    render_features: Vec<String>,
    selected_feature: Option<String>,
    _search_subscription: Subscription,
    pub(crate) focus_handle: FocusHandle,
}

fn load_plugins() -> Vec<PluginDetails> {
    let Some(pm_lock) = plugin_manager::global() else {
        return Vec::new();
    };
    let mut plugins = pm_lock.read().plugin_details();
    plugins.sort_by(|a, b| {
        a.metadata
            .name
            .to_lowercase()
            .cmp(&b.metadata.name.to_lowercase())
    });
    plugins
}

fn load_renderer_features() -> Vec<String> {
    let mut features =
        engine_backend::subsystems::render::helio_renderer::render_features::plugin_features()
            .ids()
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
    features.sort_unstable();
    features
}

impl PluginManagerWindow {
    pub fn new_global(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search extensions and capabilities…")
        });
        let search_subscription =
            cx.subscribe(&search_input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_query = input.read(cx).text().to_string();
                    this.rebuild_visible_plugins();
                    cx.notify();
                }
            });
        let plugins = load_plugins();
        let render_features = load_renderer_features();
        let selected = plugins.first().map(|plugin| plugin.metadata.id.clone());
        let selected_feature = render_features.first().cloned();
        let mut this = Self {
            plugins,
            selected,
            visible_plugin_indices: Vec::new(),
            search_input,
            search_query: String::new(),
            filter: PluginFilter::All,
            sort: PluginSort::Name,
            view: RegistryView::Extensions,
            render_features,
            selected_feature,
            _search_subscription: search_subscription,
            focus_handle: cx.focus_handle(),
        };
        this.rebuild_visible_plugins();
        this
    }

    fn rebuild_visible_plugins(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        let mut indices = self
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(index, plugin)| {
                let matches_filter = match self.filter {
                    PluginFilter::All => true,
                    PluginFilter::BuiltIn => plugin.builtin,
                    PluginFilter::External => !plugin.builtin,
                    PluginFilter::NeedsAttention => has_unmet_required_dependency(plugin),
                };
                let matches_query = query.is_empty() || plugin_matches_query(plugin, &query);
                (matches_filter && matches_query).then_some(index)
            })
            .collect::<Vec<_>>();

        match self.sort {
            PluginSort::Name => indices.sort_by(|a, b| {
                self.plugins[*a]
                    .metadata
                    .name
                    .to_lowercase()
                    .cmp(&self.plugins[*b].metadata.name.to_lowercase())
            }),
            PluginSort::Contributions => indices.sort_by(|a, b| {
                contribution_count(&self.plugins[*b])
                    .cmp(&contribution_count(&self.plugins[*a]))
                    .then_with(|| {
                        self.plugins[*a]
                            .metadata
                            .name
                            .to_lowercase()
                            .cmp(&self.plugins[*b].metadata.name.to_lowercase())
                    })
            }),
        }
        self.visible_plugin_indices = indices;
        if !self.selected.as_ref().is_some_and(|selected| {
            self.visible_plugin_indices
                .iter()
                .any(|index| &self.plugins[*index].metadata.id == selected)
        }) {
            self.selected = self
                .visible_plugin_indices
                .first()
                .map(|index| self.plugins[*index].metadata.id.clone());
        }
    }

    pub(crate) fn select(&mut self, plugin: Option<PluginId>, cx: &mut Context<Self>) {
        self.selected = plugin;
        cx.notify();
    }

    fn select_feature(&mut self, feature: String, cx: &mut Context<Self>) {
        self.selected_feature = Some(feature);
        cx.notify();
    }

    fn set_filter(&mut self, filter: PluginFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.rebuild_visible_plugins();
        cx.notify();
    }

    fn set_view(&mut self, view: RegistryView, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    fn toggle_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            PluginSort::Name => PluginSort::Contributions,
            PluginSort::Contributions => PluginSort::Name,
        };
        self.rebuild_visible_plugins();
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.plugins = load_plugins();
        self.render_features = load_renderer_features();
        if !self
            .render_features
            .iter()
            .any(|id| Some(id) == self.selected_feature.as_ref())
        {
            self.selected_feature = self.render_features.first().cloned();
        }
        self.rebuild_visible_plugins();
        cx.notify();
    }
}

impl Focusable for PluginManagerWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PluginManagerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plugin_count = self.plugins.len();
        let attention_count = self
            .plugins
            .iter()
            .filter(|plugin| has_unmet_required_dependency(plugin))
            .count();
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.plugins.iter().find(|plugin| &plugin.metadata.id == id))
            .cloned();
        let search_input = self.search_input.clone();
        let visible_count = self.visible_plugin_indices.len();
        let filter = self.filter;
        let sort = self.sort;
        let view = self.view;

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
                                .child(format!("{plugin_count} loaded")),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("refresh-plugins")
                                .icon(IconName::Refresh)
                                .ghost()
                                .xsmall()
                                .tooltip("Refresh extension registry")
                                .on_click(cx.listener(handlers::on_refresh)),
                        ),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(render_page_header(
                        plugin_count,
                        self.render_features.len(),
                        view,
                        cx,
                    ))
                    .when(view == RegistryView::Extensions, |this| {
                        this.child(render_extension_toolbar(
                            &search_input,
                            filter,
                            sort,
                            attention_count,
                            cx,
                        ))
                        .child(render_extension_workspace(
                            self,
                            selected.as_ref(),
                            visible_count,
                            cx,
                        ))
                    })
                    .when(view == RegistryView::Renderer, |this| {
                        this.child(render_renderer_workspace(self, cx))
                    })
                    .child(render_footer(view, self.visible_plugin_indices.len(), cx)),
            )
    }
}

fn render_page_header(
    plugin_count: usize,
    feature_count: usize,
    view: RegistryView,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .flex_shrink_0()
        .px_5()
        .pt_3()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child("Extensions"),
                )
                .child(div().flex_1())
                .child(
                    registry_tab(
                        "extensions-tab",
                        "Extensions",
                        plugin_count,
                        view == RegistryView::Extensions,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_view(RegistryView::Extensions, cx)),
                    ),
                )
                .child(
                    registry_tab(
                        "renderer-tab",
                        "Renderer",
                        feature_count,
                        view == RegistryView::Renderer,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_view(RegistryView::Renderer, cx)),
                    ),
                ),
        )
        .child(
            div()
                .pt_1()
                .pb_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if view == RegistryView::Extensions {
                    "Inspect what each extension adds to your editor and runtime."
                } else {
                    "Process-wide renderer features and their render graph registrations."
                }),
        )
        .border_b_1()
        .border_color(cx.theme().border)
}

fn registry_tab(id: &'static str, label: &'static str, count: usize, selected: bool) -> Button {
    Button::new(id)
        .small()
        .ghost()
        .selected(selected)
        .label(format!("{label}  {count}"))
}

fn render_extension_toolbar(
    search_input: &Entity<InputState>,
    filter: PluginFilter,
    sort: PluginSort,
    attention_count: usize,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
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
        .child(
            filter_button("filter-all", "All", filter == PluginFilter::All)
                .on_click(cx.listener(|this, _, _, cx| this.set_filter(PluginFilter::All, cx))),
        )
        .child(
            filter_button(
                "filter-built-in",
                "Built-in",
                filter == PluginFilter::BuiltIn,
            )
            .on_click(cx.listener(|this, _, _, cx| this.set_filter(PluginFilter::BuiltIn, cx))),
        )
        .child(
            filter_button(
                "filter-external",
                "External",
                filter == PluginFilter::External,
            )
            .on_click(cx.listener(|this, _, _, cx| this.set_filter(PluginFilter::External, cx))),
        )
        .child(
            filter_button(
                "filter-attention",
                &format!("Needs attention · {attention_count}"),
                filter == PluginFilter::NeedsAttention,
            )
            .when(attention_count > 0, |button| {
                button.text_color(cx.theme().warning)
            })
            .on_click(
                cx.listener(|this, _, _, cx| this.set_filter(PluginFilter::NeedsAttention, cx)),
            ),
        )
        .child(div().flex_1())
        .child(
            Button::new("extension-sort")
                .small()
                .ghost()
                .icon(IconName::List)
                .label(match sort {
                    PluginSort::Name => "Name",
                    PluginSort::Contributions => "Contributions",
                })
                .tooltip("Toggle sort order")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sort(cx))),
        )
}

fn filter_button(id: &'static str, label: &str, selected: bool) -> Button {
    Button::new(id)
        .small()
        .ghost()
        .selected(selected)
        .label(label.to_string())
}

fn render_extension_workspace(
    this: &mut PluginManagerWindow,
    selected: Option<&PluginDetails>,
    visible_count: usize,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let total_count = this.plugins.len();
    let all_plugins = this.plugins.clone();

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
                                .child("LOADED EXTENSIONS"),
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
                        .child(if total_count == 0 {
                            "No extensions are loaded"
                        } else {
                            "No extensions match this search"
                        })
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
                                            this.visible_plugin_indices.get(visible_index).and_then(
                                                |plugin_index| this.plugins.get(*plugin_index),
                                            )
                                        })
                                        .map(|plugin| {
                                            let is_selected =
                                                this.selected.as_ref() == Some(&plugin.metadata.id);
                                            div()
                                                .w_full()
                                                .h(px(68.))
                                                .px_2()
                                                .child(render_plugin_item(plugin, is_selected, cx))
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .size_full(),
                        )
                        .into_any_element()
                })
                .child(
                    h_flex()
                        .w_full()
                        .h(px(30.))
                        .flex_shrink_0()
                        .px_3()
                        .items_center()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().border.opacity(0.7))
                        .child(
                            Icon::new(IconName::Check)
                                .size_3()
                                .text_color(cx.theme().success),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Registry loaded"),
                        ),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .when_some(selected, |this, selected| {
                    this.child(render_plugin_details(&selected, &all_plugins, cx))
                })
                .when(selected.is_none(), |this| {
                    this.child(render_empty_state(cx))
                }),
        )
}

fn render_renderer_workspace(
    this: &mut PluginManagerWindow,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let features = this.render_features.clone();
    let selected_feature = this.selected_feature.clone();
    let has_selected_feature = selected_feature.is_some();

    h_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .child(
            v_flex()
                .w(px(420.))
                .flex_shrink_0()
                .h_full()
                .overflow_hidden()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .h(px(36.))
                        .flex_shrink_0()
                        .px_3()
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.7))
                        .child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child("FEATURE ID"))
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{} registered", features.len()))),
                )
                .child(if features.is_empty() {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::Activity).size_5())
                        .child("No renderer features are registered")
                        .into_any_element()
                } else {
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(
                            uniform_list(
                                "renderer-feature-list",
                                features.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                    range
                                        .filter_map(|index| this.render_features.get(index))
                                        .map(|feature| {
                                            let id = feature.clone();
                                            let selected = this.selected_feature.as_ref() == Some(feature);
                                            Button::new(SharedString::from(format!("renderer-feature-{feature}")))
                                                .small()
                                                .ghost()
                                                .selected(selected)
                                                .w_full()
                                                .h(px(42.))
                                                .px_3()
                                                .gap_2()
                                                .items_center()
                                                .on_click(cx.listener(move |this, _, _, cx| this.select_feature(id.clone(), cx)))
                                                .child(Icon::new(IconName::Activity).size_4().text_color(cx.theme().muted_foreground))
                                                .child(div().flex_1().text_sm().font_family("monospace").text_color(cx.theme().foreground).child(feature.clone()))
                                                .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Registered"))
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
                .gap_4()
                .p_5()
                .when_some(selected_feature, |this, feature| {
                    this.child(
                        v_flex()
                            .gap_2()
                            .child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child("FEATURE ID"))
                            .child(div().text_base().font_family("monospace").text_color(cx.theme().foreground).child(feature)),
                    )
                    .child(renderer_detail_section("Registration", "Linked engine feature", cx))
                    .child(renderer_detail_section(
                        "Render graph",
                        "Feature-owned nodes and passes are available to render pipelines.",
                        cx,
                    ))
                    .child(
                        div()
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.7))
                            .bg(cx.theme().sidebar.opacity(0.45))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Renderer features are listed independently from editor plugins because this registry does not currently expose per-plugin ownership."),
                    )
                })
                .when(!has_selected_feature, |this| {
                    this.child(div().text_sm().text_color(cx.theme().muted_foreground).child("No renderer features are registered."))
                }),
        )
}

fn renderer_detail_section(
    title: &'static str,
    body: &'static str,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border.opacity(0.7))
        .bg(cx.theme().sidebar.opacity(0.45))
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body),
        )
}

fn render_footer(
    view: RegistryView,
    visible_count: usize,
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
        .child(if view == RegistryView::Extensions {
            format!(
                "{} extensions shown · plugins remain loaded until editor restart",
                visible_count
            )
        } else {
            "Process-wide render feature registry".to_string()
        })
        .child(if view == RegistryView::Extensions {
            "Search extensions and capabilities"
        } else {
            "Select a feature to inspect its render graph"
        })
}

fn has_unmet_required_dependency(plugin: &PluginDetails) -> bool {
    plugin.dependencies.iter().any(|dependency| {
        !dependency.optional && !matches!(dependency.state, DependencyState::Satisfied { .. })
    })
}

fn contribution_count(plugin: &PluginDetails) -> usize {
    plugin.file_types.len()
        + plugin.editors.len()
        + plugin.components.len()
        + plugin.statusbar_buttons.len()
        + plugin.ai_tools.len()
        + plugin.subsystems.len()
}

fn plugin_matches_query(plugin: &PluginDetails, query: &str) -> bool {
    let fields = [
        plugin.metadata.name.as_str(),
        plugin.metadata.id.as_str(),
        plugin.metadata.description.as_str(),
        plugin.metadata.author.as_str(),
        plugin.metadata.version.as_str(),
    ];
    fields
        .iter()
        .any(|field| field.to_lowercase().contains(query))
        || [
            &plugin.file_types,
            &plugin.editors,
            &plugin.components,
            &plugin.statusbar_buttons,
            &plugin.ai_tools,
            &plugin.subsystems,
        ]
        .iter()
        .flat_map(|items| items.iter())
        .any(|item| item.to_lowercase().contains(query))
        || plugin.dependencies.iter().any(|dependency| {
            dependency
                .dependency
                .id
                .as_str()
                .to_lowercase()
                .contains(query)
                || dependency
                    .dependency
                    .requirement
                    .to_string()
                    .to_lowercase()
                    .contains(query)
        })
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
