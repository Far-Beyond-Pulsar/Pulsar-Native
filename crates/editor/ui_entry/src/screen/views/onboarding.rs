use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex,
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt,
};

use crate::core::types::*;
use crate::screen::EntryScreen;
use crate::service::auth_service::AuthService;
use crate::service::dependency_service::DependencyService;

pub fn render_onboarding(
    screen: &mut EntryScreen,
    _window: &mut Window,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let (rust_installed, build_tools_installed) = screen
        .state
        .dependency_status
        .as_ref()
        .map(|s| (s.rust_installed, s.build_tools_installed))
        .unwrap_or((false, false));
    let all_deps_ok = rust_installed && build_tools_installed;
    let theme = cx.theme().clone();

    let left_column = render_left_column(screen, cx);
    let right_column = render_right_column(rust_installed, build_tools_installed, screen, cx);

    div()
        .absolute()
        .size_full()
        .inset_0()
        .flex()
        .flex_col()
        .bg(theme.background)
        .child(
            h_flex().w_full().justify_end().px_4().py_3().child(
                Button::new("close-onboarding")
                    .compact()
                    .ghost()
                    .icon(IconName::X)
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.ui.show_onboarding = false;
                        cx.notify();
                    })),
            ),
        )
        .child(
            h_flex()
                .w_full()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .px_12()
                .pb_6()
                .gap_6()
                .child(left_column)
                .child(right_column),
        )
        .child(
            h_flex()
                .w_full()
                .px_12()
                .py_6()
                .border_t_1()
                .border_color(theme.border)
                .gap_3()
                .justify_between()
                .child(
                    Button::new("skip-onboarding")
                        .label("Skip All")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_onboarding = false;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("finish-onboarding")
                        .label("Get Started")
                        .primary()
                        .disabled(!all_deps_ok)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_onboarding = false;
                            cx.notify();
                        })),
                ),
        )
}

fn render_left_column(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let tab = screen.state.ui.onboarding_tab;
    let theme = cx.theme();

    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .overflow_hidden()
        .child(render_tab_bar(tab, cx))
        .child(match tab {
            OnboardingTab::Theme => render_theme_content(screen, cx).into_any_element(),
            OnboardingTab::Plugins => render_plugin_content(screen, cx).into_any_element(),
        })
}

fn render_tab_bar(active: OnboardingTab, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(tab_button(
            "tab-theme",
            IconName::Palette,
            "Themes",
            active == OnboardingTab::Theme,
            OnboardingTab::Theme,
            cx,
        ))
        .child(tab_button(
            "tab-plugins",
            IconName::Package,
            "Plugins",
            active == OnboardingTab::Plugins,
            OnboardingTab::Plugins,
            cx,
        ))
}

fn tab_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    is_active: bool,
    tab: OnboardingTab,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .px_5()
        .py_3()
        .gap_2()
        .items_center()
        .cursor_pointer()
        .border_b_2()
        .border_color(if is_active {
            theme.accent
        } else {
            gpui::transparent_white()
        })
        .bg(gpui::transparent_white())
        .hover(|s| s.bg(theme.accent.opacity(0.06)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |screen, _, _, cx| {
                let was_on_plugin_tab = screen.state.ui.onboarding_tab == OnboardingTab::Plugins;
                screen.state.ui.onboarding_tab = tab;
                if tab == OnboardingTab::Plugins
                    && !was_on_plugin_tab
                    && screen.state.registry_plugins.is_empty()
                    && !screen.state.registry_refresh_in_progress
                {
                    screen.refresh_plugin_registry(cx);
                }
                cx.notify();
            }),
        )
        .child(Icon::new(icon).size_4().text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        }))
        .child(
            div()
                .text_sm()
                .font_weight(if is_active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if is_active {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .child(label),
        )
}

fn render_theme_content(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let current_name = cx.theme().theme_name().clone();
    let themes: Vec<std::rc::Rc<ui::ThemeConfig>> = ui::ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .cloned()
        .collect();

    v_flex().flex_1().min_h_0().child(
        v_flex()
            .id("theme-scroll")
            .flex_1()
            .min_h_0()
            .scrollable(ScrollbarAxis::Vertical)
            .gap_3()
            .p_4()
            .child(
                h_flex().flex_wrap().gap_3().children(
                    themes
                        .into_iter()
                        .map(|config| render_theme_card(config, &current_name, screen, cx)),
                ),
            ),
    )
}

fn render_theme_card(
    config: std::rc::Rc<ui::ThemeConfig>,
    current_name: &str,
    _screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let name = config.name.clone();
    let is_active = name == current_name;
    let name_for_click = name.clone();
    let is_dark = config.mode.is_dark();

    let swatch_bg = config
        .colors
        .background
        .as_ref()
        .and_then(|h| gpui::Rgba::try_from(h.as_ref()).ok().map(gpui::Hsla::from))
        .unwrap_or_else(|| {
            if is_dark {
                gpui::hsla(0., 0., 0.1, 1.)
            } else {
                gpui::hsla(0., 0., 0.97, 1.)
            }
        });
    let swatch_fg = config
        .colors
        .foreground
        .as_ref()
        .and_then(|h| gpui::Rgba::try_from(h.as_ref()).ok().map(gpui::Hsla::from))
        .unwrap_or_else(|| {
            if is_dark {
                gpui::hsla(0., 0., 0.95, 1.)
            } else {
                gpui::hsla(0., 0., 0.05, 1.)
            }
        });
    let accent_col = config
        .colors
        .accent
        .as_ref()
        .and_then(|h| gpui::Rgba::try_from(h.as_ref()).ok().map(gpui::Hsla::from))
        .unwrap_or_else(|| {
            if is_dark {
                gpui::hsla(0.6, 0.7, 0.5, 1.)
            } else {
                gpui::hsla(0.6, 0.7, 0.4, 1.)
            }
        });

    v_flex()
        .id(SharedString::from(format!("theme-card-{}", name)))
        .w(px(160.))
        .p(px(12.))
        .rounded_lg()
        .cursor_pointer()
        .bg(if is_active {
            theme.secondary.opacity(0.4)
        } else {
            theme.secondary.opacity(0.15)
        })
        .hover(|this| this.bg(theme.secondary.opacity(0.3)))
        .border_1()
        .border_color(if is_active {
            theme.accent
        } else {
            theme.border
        })
        .gap_2()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _, _, cx| {
                if let Some(cfg) = ui::ThemeRegistry::global(cx)
                    .themes()
                    .get(&name_for_click)
                    .cloned()
                {
                    ui::Theme::global_mut(cx).apply_config(&cfg);
                    cx.refresh_windows();
                }
            }),
        )
        .child(
            div()
                .w_full()
                .h(px(40.))
                .rounded_md()
                .bg(swatch_bg)
                .border_1()
                .border_color(theme.border)
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.))
                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(accent_col))
                .child(div().w(px(24.)).h(px(4.)).rounded_full().bg(swatch_fg)),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(name.to_string()),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(if is_dark {
                    theme.primary
                } else {
                    theme.warning
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if is_dark { "Dark" } else { "Light" }),
                ),
        )
}

fn render_plugin_content(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let search_input = screen.inputs().plugin_search.clone();
    let query = screen.state.input.plugin_search_query.to_lowercase();
    let is_refreshing = screen.state.registry_refresh_in_progress;
    let phase = screen.state.plugin_install_phase.clone();
    let has_phase = phase.is_some();

    let available: Vec<RegistryPlugin> = screen
        .state
        .registry_plugins
        .iter()
        .filter(|p| {
            query.is_empty()
                || p.name.to_lowercase().contains(&query)
                || p.description.to_lowercase().contains(&query)
                || p.author.to_lowercase().contains(&query)
                || p.tags.iter().any(|t| t.to_lowercase().contains(&query))
        })
        .cloned()
        .collect();

    let installed_urls: std::collections::HashSet<String> = screen
        .state
        .installed_plugins
        .iter()
        .map(|p| p.repo_url.clone())
        .collect();

    let custom_installed: Vec<InstalledPlugin> = screen
        .state
        .installed_plugins
        .iter()
        .filter(|p| {
            !screen
                .state
                .registry_plugins
                .iter()
                .any(|rp| rp.repo_url == p.repo_url)
        })
        .cloned()
        .collect();

    let registries_empty = screen.state.registry_plugins.is_empty();
    let available_empty = available.is_empty();
    let phase_element = phase.map(|p| render_plugin_phase(p, cx));
    let plugin_cards: Vec<_> = available
        .into_iter()
        .map(|plugin| {
            let is_installed = installed_urls.contains(&plugin.repo_url);
            render_registry_plugin_card(plugin, is_installed, cx)
        })
        .collect();
    let custom_plugin_rows: Vec<_> = custom_installed
        .into_iter()
        .enumerate()
        .map(|(idx, plugin)| render_custom_plugin_row(idx, plugin, cx))
        .collect();

    v_flex()
        .flex_1()
        .min_h_0()
        .child(
            h_flex()
                .px_4()
                .pt_3()
                .pb_2()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(ui::input::Input::new(&search_input)))
                .child(
                    Button::new("refresh-registries-btn")
                        .icon(IconName::Refresh)
                        .ghost()
                        .compact()
                        .disabled(is_refreshing)
                        .tooltip(if is_refreshing {
                            "Refreshing..."
                        } else {
                            "Refresh plugin registries"
                        })
                        .on_click(cx.listener(|screen, _, _, cx| {
                            screen.refresh_plugin_registry(cx);
                        })),
                ),
        )
        .when_some(phase_element, |this, el| this.child(el))
        .child(
            v_flex()
                .id("registry-plugin-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(ScrollbarAxis::Vertical)
                .px_4()
                .pb_2()
                .gap_2()
                .when(is_refreshing && registries_empty, |this| {
                    this.child(
                        div()
                            .py_10()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .text_center()
                            .child("Fetching plugin registry..."),
                    )
                })
                .when(!is_refreshing && registries_empty, |this| {
                    this.child(
                        v_flex()
                            .py_10()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Package)
                                    .size_8()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .text_center()
                                    .child(
                                        "No plugins found. Click \u{21BA} to refresh registries.",
                                    ),
                            ),
                    )
                })
                .when(!registries_empty && available_empty, |this| {
                    this.child(
                        div()
                            .py_8()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .text_center()
                            .child("No plugins match your search"),
                    )
                })
                .children(plugin_cards)
                .when(!custom_plugin_rows.is_empty(), |this| {
                    this.child(
                        h_flex()
                            .mt_2()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(theme.muted_foreground.opacity(0.25)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Custom installed"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(theme.muted_foreground.opacity(0.25)),
                            ),
                    )
                    .children(custom_plugin_rows)
                }),
        )
}

fn render_registry_plugin_card(
    plugin: RegistryPlugin,
    is_installed: bool,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let repo_url = plugin.repo_url.clone();
    let repo_url_for_remove = repo_url.clone();
    let name = plugin.name.clone();
    let desc = plugin.description.clone();
    let author = plugin.author.clone();
    let tags = plugin.tags.clone();

    v_flex()
        .id(SharedString::from(format!("rp-{}", repo_url)))
        .w_full()
        .p_3()
        .gap_2()
        .rounded_lg()
        .bg(theme.secondary.opacity(0.12))
        .border_1()
        .border_color(if is_installed {
            theme.success_foreground.opacity(0.3)
        } else {
            theme.border
        })
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.foreground)
                                        .truncate()
                                        .child(name),
                                )
                                .when(!author.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!("by {author}")),
                                    )
                                }),
                        )
                        .when(!desc.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(desc),
                            )
                        }),
                )
                .child(if is_installed {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .flex_shrink_0()
                        .child(
                            div()
                                .px_2()
                                .py(px(2.))
                                .rounded_full()
                                .bg(theme.success_foreground.opacity(0.15))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.success_foreground)
                                .child("Installed"),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "remove-reg-{}",
                                repo_url_for_remove
                            )))
                            .ghost()
                            .compact()
                            .icon(IconName::Trash)
                            .on_click(cx.listener(
                                move |screen, _, _, cx| {
                                    if let Some(idx) = screen
                                        .state
                                        .installed_plugins
                                        .iter()
                                        .position(|p| p.repo_url == repo_url_for_remove)
                                    {
                                        screen.remove_plugin(idx, cx);
                                    }
                                },
                            )),
                        )
                        .into_any_element()
                } else {
                    Button::new(SharedString::from(format!("install-reg-{}", repo_url)))
                        .label("Install")
                        .primary()
                        .compact()
                        .on_click({
                            let url = repo_url.clone();
                            cx.listener(move |screen, _, _, cx| {
                                if let Some(plugin) = screen
                                    .state
                                    .registry_plugins
                                    .iter()
                                    .find(|p| p.repo_url == url)
                                    .cloned()
                                {
                                    screen.install_registry_plugin(plugin, cx);
                                }
                            })
                        })
                        .into_any_element()
                }),
        )
        .when(!tags.is_empty(), |this| {
            this.child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(tags.into_iter().map(|tag| {
                        div()
                            .px_2()
                            .py(px(1.))
                            .rounded_full()
                            .bg(theme.accent)
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.accent_foreground)
                            .child(tag)
                    })),
            )
        })
}

fn render_plugin_phase(
    phase: PluginInstallPhase,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, color, headline, sub, progress_val, show_bar) = match &phase {
        PluginInstallPhase::FetchingMetadata => (
            IconName::Download,
            theme.accent,
            "Fetching release info...",
            String::new(),
            None,
            false,
        ),
        PluginInstallPhase::Downloading { progress } => (
            IconName::Download,
            theme.accent,
            "Downloading binary...",
            format!("{:.0}%", progress * 100.0),
            Some(*progress),
            true,
        ),
        PluginInstallPhase::Building { logs } => (
            IconName::Settings,
            theme.accent,
            "Building from source...",
            logs.last().cloned().unwrap_or_default(),
            None,
            false,
        ),
        PluginInstallPhase::Complete(p) => (
            IconName::Check,
            theme.success_foreground,
            "Installed successfully!",
            format!("{} \u{2014} {}", p.name, p.version),
            Some(1.0),
            true,
        ),
        PluginInstallPhase::Error(e) => (
            IconName::WarningTriangle,
            gpui::red(),
            "Installation failed",
            e.clone(),
            None,
            false,
        ),
    };

    v_flex()
        .mx_4()
        .mb_3()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(color.opacity(0.35))
        .bg(color.opacity(0.08))
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(icon).size_4().text_color(color))
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(headline),
                )
                .when(
                    matches!(
                        phase,
                        PluginInstallPhase::Complete(_) | PluginInstallPhase::Error(_)
                    ),
                    |this| {
                        this.child(
                            Button::new("dismiss-phase")
                                .ghost()
                                .compact()
                                .icon(IconName::X)
                                .on_click(cx.listener(|screen, _, _, cx| {
                                    screen.state.plugin_install_phase = None;
                                    cx.notify();
                                })),
                        )
                    },
                ),
        )
        .when(!sub.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(sub),
            )
        })
        .when(show_bar, |this| {
            let pv = progress_val.unwrap_or(0.0);
            this.child(
                div()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(color)
                            .w(relative(pv.max(0.0).min(1.0))),
                    ),
            )
        })
}

fn render_custom_plugin_row(
    idx: usize,
    plugin: InstalledPlugin,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let method_label = match plugin.install_method {
        PluginInstallMethod::BinaryDownload => "Binary",
        PluginInstallMethod::BuiltFromSource => "Source",
    };
    let method_color = match plugin.install_method {
        PluginInstallMethod::BinaryDownload => theme.success_foreground,
        PluginInstallMethod::BuiltFromSource => theme.accent,
    };
    let name = plugin.name.clone();
    let version = plugin.version.clone();
    let repo = plugin.repo_url.clone();

    h_flex()
        .id(SharedString::from(format!("plugin-row-{idx}")))
        .w_full()
        .px_3()
        .py_2()
        .gap_3()
        .items_center()
        .rounded_lg()
        .bg(theme.secondary.opacity(0.15))
        .border_1()
        .border_color(theme.border)
        .child(
            Icon::new(IconName::Package)
                .size_4()
                .text_color(theme.foreground),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(repo),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(version),
        )
        .child(
            div()
                .flex_shrink_0()
                .px_2()
                .py(px(2.))
                .rounded_full()
                .bg(method_color.opacity(0.15))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(method_color)
                .child(method_label),
        )
        .child(
            Button::new(SharedString::from(format!("remove-plugin-{idx}")))
                .ghost()
                .compact()
                .icon(IconName::Trash)
                .on_click(cx.listener(move |screen, _, _, cx| {
                    screen.remove_plugin(idx, cx);
                })),
        )
}

fn render_right_column(
    rust_installed: bool,
    build_tools_installed: bool,
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    v_flex()
        .w(px(400.))
        .h_full()
        .flex_shrink_0()
        .gap_6()
        .child(div().flex_1().min_h_0().child(render_deps_card(
            rust_installed,
            build_tools_installed,
            screen,
            cx,
        )))
        .child(div().flex_shrink_0().child(render_account_card(screen, cx)))
}

fn render_deps_card(
    rust_installed: bool,
    build_tools_installed: bool,
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let show_downloading = screen
        .state
        .install_progress
        .as_ref()
        .map(|p| {
            matches!(
                p.status,
                InstallStatus::Downloading | InstallStatus::Installing
            )
        })
        .unwrap_or(false);

    v_flex()
        .w_full()
        .flex_1()
        .h_full()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .overflow_hidden()
        .child(render_card_header(
            IconName::Package,
            "Dependencies",
            "Required build tools for Pulsar projects",
            cx,
        ))
        .child(
            v_flex()
                .id("deps-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(ScrollbarAxis::Vertical)
                .gap_3()
                .p_4()
                .child(render_dep_item("Rust Toolchain", rust_installed, None, cx))
                .child(render_dep_item(
                    "C/C++ Build Tools",
                    build_tools_installed,
                    screen
                        .state
                        .dependency_status
                        .as_ref()
                        .and_then(|s| s.compiler_info.clone()),
                    cx,
                ))
                .children(
                    screen
                        .state
                        .install_progress
                        .clone()
                        .map(|p| render_install_progress(p, cx)),
                )
                .child(
                    Button::new("install-deps-onboarding")
                        .label("Install Missing Dependencies")
                        .primary()
                        .when(show_downloading, |btn| btn.ghost())
                        .on_click(cx.listener(|this, _, _, cx| {
                            run_setup_script(this, cx);
                            cx.notify();
                        })),
                ),
        )
}

fn render_dep_item(
    name: &str,
    installed: bool,
    info: Option<String>,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, color, status) = if installed {
        (
            IconName::Check,
            theme.success_foreground,
            info.unwrap_or_else(|| "Installed".to_string()),
        )
    } else {
        (
            IconName::WarningTriangle,
            gpui::yellow(),
            "Not detected".to_string(),
        )
    };
    h_flex()
        .gap_3()
        .items_center()
        .p_3()
        .bg(theme.secondary.opacity(0.3))
        .rounded_md()
        .child(Icon::new(icon).size_5().text_color(color))
        .child(
            div()
                .flex_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(name.to_string()),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(status),
        )
}

fn render_account_card(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let profile = AuthService::profile();
    let code = screen.state.auth.device_code.clone();
    let message = screen.state.auth.message.clone();
    let loading = screen.state.auth.loading;
    let avatar_img = screen.state.auth.onboarding_avatar.clone();

    v_flex()
        .w_full()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .overflow_hidden()
        .child({
            let header = render_card_header(
                IconName::Group,
                "Account",
                "Sync settings and collaborate",
                cx,
            );
            header
        })
        .child(
            v_flex()
                .gap_3()
                .p_4()
                .when_some(profile.clone(), |this, profile| {
                    let initial = profile
                        .login
                        .chars()
                        .next()
                        .map(|c| c.to_ascii_uppercase().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let avatar = if let Some(render_img) = avatar_img {
                        div()
                            .w(px(56.))
                            .h(px(56.))
                            .rounded_full()
                            .overflow_hidden()
                            .flex_shrink_0()
                            .child(
                                gpui::img(ImageSource::Render(render_img))
                                    .w_full()
                                    .h_full()
                                    .rounded_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w(px(56.))
                            .h(px(56.))
                            .rounded_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme.accent.opacity(0.2))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(initial),
                            )
                            .into_any_element()
                    };
                    this.child(
                        h_flex().gap_4().items_center().child(avatar).child(
                            v_flex()
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.foreground)
                                        .child(
                                            profile.display_name.unwrap_or(profile.login.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("@{}", profile.login)),
                                ),
                        ),
                    )
                })
                .when(profile.is_none() && code.is_none() && !loading, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Sign in to enable cloud sync and multiplayer collaboration."),
                    )
                    .child(
                        Button::new("signin-github-onboarding")
                            .label("Sign In with GitHub")
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.begin_github_sign_in(cx);
                                cx.notify();
                            })),
                    )
                })
                .when(loading, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Signing in..."),
                    )
                })
                .when_some(code, |this, code| {
                    this.child(
                        v_flex()
                            .gap_2()
                            .p_3()
                            .bg(theme.accent.opacity(0.12))
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.accent.opacity(0.35))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Paste this code in the browser window:"),
                            )
                            .child(
                                div()
                                    .text_center()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(code),
                            ),
                    )
                })
                .when_some(message, |this, msg| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(msg),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Your data stays private. Sign-in is optional."),
                ),
        )
}

fn render_card_header(
    icon: IconName,
    title: &str,
    subtitle: &str,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .px_5()
        .py_4()
        .gap_3()
        .items_center()
        .bg(theme.secondary.opacity(0.15))
        .child(Icon::new(icon).size_5().text_color(theme.foreground))
        .child(
            v_flex()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(subtitle.to_string()),
                ),
        )
}

fn render_install_progress(
    progress: InstallProgress,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, color, status_text) = match &progress.status {
        InstallStatus::Idle => (IconName::Circle, theme.accent, "Ready".to_string()),
        InstallStatus::Downloading => (
            IconName::Download,
            theme.accent,
            "Downloading installer...".to_string(),
        ),
        InstallStatus::Installing => (
            IconName::Settings,
            theme.accent,
            "Installing dependencies...".to_string(),
        ),
        InstallStatus::Complete => (
            IconName::Check,
            theme.success_foreground,
            "Installation complete!".to_string(),
        ),
        InstallStatus::Error(e) => (IconName::WarningTriangle, gpui::red(), e.clone()),
    };

    v_flex()
        .gap_2()
        .p_4()
        .bg(theme.secondary.opacity(0.2))
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).size_4().text_color(color))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(status_text),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(8.))
                .bg(theme.secondary.opacity(0.3))
                .rounded_sm()
                .child(
                    div()
                        .h_full()
                        .rounded_sm()
                        .bg(if matches!(progress.status, InstallStatus::Error(_)) {
                            gpui::red()
                        } else {
                            theme.accent
                        })
                        .w(relative(progress.progress.max(0.0).min(1.0))),
                ),
        )
        .child(
            div()
                .id("install-log-scroll")
                .w_full()
                .max_h(px(200.))
                .p_2()
                .bg(gpui::black().opacity(0.3))
                .rounded_sm()
                .overflow_y_scroll()
                .children(progress.logs.iter().rev().take(20).rev().map(|log| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(log.clone())
                })),
        )
}

fn run_setup_script(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) {
    screen.state.install_progress = Some(InstallProgress {
        logs: vec!["Starting installation...".to_string()],
        progress: 0.0,
        status: InstallStatus::Downloading,
    });

    let progress = std::sync::Arc::new(std::sync::Mutex::new(
        screen.state.install_progress.clone().unwrap(),
    ));
    let progress_clone = std::sync::Arc::clone(&progress);

    cx.spawn(async move |this, cx| {
        let p = progress_clone;
        let _ = cx
            .background_executor()
            .spawn(async move { DependencyService::install_rust(p) })
            .await;

        loop {
            cx.background_executor()
                .spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                })
                .await;

            let should_break = cx
                .update(|cx| {
                    this.update(cx, |screen, cx| {
                        let prog = progress.lock().unwrap();
                        screen.state.install_progress = Some(prog.clone());
                        cx.notify();
                        matches!(
                            prog.status,
                            InstallStatus::Complete | InstallStatus::Error(_)
                        )
                    })
                })
                .unwrap_or(false);

            if should_break {
                break;
            }
        }
    })
    .detach();
}
