use crate::entry_screen::{
    types::{
        InstalledPlugin, OnboardingTab, PluginInstallMethod, PluginInstallPhase, RegistryPlugin,
    },
    EntryScreen, InstallProgress, InstallStatus,
};
use gpui::{prelude::*, *};
use std::process::Command;
use std::sync::{Arc, Mutex};
use ui::{
    button::{Button, ButtonVariants},
    h_flex, input::InputState, scroll::ScrollbarAxis, v_flex, ActiveTheme, Disableable, Icon,
    IconName, Sizable, StyledExt,
};

#[cfg(target_os = "windows")]
const RUSTUP_URL: &str =
    "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe";

#[cfg(any(target_os = "linux", target_os = "macos"))]
const RUSTUP_URL: &str = "https://sh.rustup.rs";

pub fn render_onboarding(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let (rust_installed, build_tools_installed) = screen
        .dependency_status
        .as_ref()
        .map(|s| (s.rust_installed, s.build_tools_installed))
        .unwrap_or((false, false));

    let all_deps_ok = rust_installed && build_tools_installed;
    let bg = cx.theme().background;
    let accent = cx.theme().accent;
    let fg = cx.theme().foreground;
    let muted = cx.theme().muted_foreground;

    div()
        .absolute()
        .size_full()
        .inset_0()
        .flex()
        .flex_col()
        .bg(bg)
        // ── Header ──────────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .px_12()
                .pt_10()
                .pb_6()
                .gap_2()
                .child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(Icon::new(IconName::Star).size_8().text_color(accent))
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(FontWeight::BOLD)
                                .text_color(fg)
                                .child("Welcome to Pulsar"),
                        ),
                )
                .child(
                    div()
                        .text_base()
                        .text_color(muted)
                        .child("Get your environment ready in a few steps"),
                ),
        )
        // ── Body ────────────────────────────────────────────────
        .child(
            h_flex()
                .w_full()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .px_12()
                .pb_6()
                .gap_6()
                .child(render_left_column(screen, cx))
                .child(render_right_column(
                    rust_installed,
                    build_tools_installed,
                    screen,
                    cx,
                )),
        )
        // ── Footer ──────────────────────────────────────────────
        .child(
            h_flex()
                .w_full()
                .px_12()
                .py_6()
                .border_t_1()
                .border_color(cx.theme().border)
                .gap_3()
                .justify_between()
                .child(
                    Button::new("skip-onboarding")
                        .label("Skip All")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_onboarding = false;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("finish-onboarding")
                        .label("Get Started")
                        .primary()
                        .disabled(!all_deps_ok)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_onboarding = false;
                            cx.notify();
                        })),
                ),
        )
}

// ── Left column: tabbed Theme / Plugins ─────────────────────

fn render_left_column(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let tab = screen.onboarding_tab;
    let bg = cx.theme().background;
    let border = cx.theme().border;

    let tab_bar = render_tab_bar(tab, cx);

    let content: AnyElement = match tab {
        OnboardingTab::Theme => render_theme_content(cx).into_any_element(),
        OnboardingTab::Plugins => render_plugin_content(screen, cx).into_any_element(),
    };

    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .overflow_hidden()
        .child(tab_bar)
        .child(content)
}

fn render_tab_bar(active: OnboardingTab, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(render_tab_button(
            "tab-theme",
            IconName::Palette,
            "Themes",
            active == OnboardingTab::Theme,
            OnboardingTab::Theme,
            cx,
        ))
        .child(render_tab_button(
            "tab-plugins",
            IconName::Package,
            "Plugins",
            active == OnboardingTab::Plugins,
            OnboardingTab::Plugins,
            cx,
        ))
}

fn render_tab_button(
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
        .border_color(if is_active { theme.accent } else { gpui::transparent_white() })
        .bg(gpui::transparent_white())
        .hover(|s| s.bg(theme.accent.opacity(0.06)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |screen, _, _, cx| {
                let was_on_plugin_tab = screen.onboarding_tab == OnboardingTab::Plugins;
                screen.onboarding_tab = tab;
                // Kick off a registry refresh the first time the plugin tab is opened
                if tab == OnboardingTab::Plugins
                    && !was_on_plugin_tab
                    && screen.registry_plugins.is_empty()
                    && !screen.registry_refresh_in_progress
                {
                    screen.refresh_registries(cx);
                }
                cx.notify();
            }),
        )
        .child(
            Icon::new(icon)
                .size_4()
                .text_color(if is_active { theme.accent } else { theme.foreground.opacity(0.6) }),
        )
        .child(
            div()
                .text_sm()
                .font_weight(if is_active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if is_active { theme.foreground } else { theme.foreground.opacity(0.6) })
                .child(label),
        )
}

// ── Theme content ────────────────────────────────────────────

fn render_theme_content(cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let current_name = cx.theme().theme_name().clone();
    let themes: Vec<std::rc::Rc<ui::ThemeConfig>> = ui::ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .cloned()
        .collect();

    v_flex()
        .flex_1()
        .min_h_0()
        .child(
            div()
                .px_5()
                .pt_4()
                .pb_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Choose your editor appearance"),
        )
        .child(
            v_flex()
                .id("theme-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(ScrollbarAxis::Vertical)
                .gap_3()
                .p_4()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .children(
                            themes
                                .into_iter()
                                .map(|config| render_theme_card(config, &current_name, cx)),
                        ),
                ),
        )
}

// ── Plugin content ───────────────────────────────────────────

fn render_plugin_content(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let search_input = screen.plugin_search_input.clone();
    let query = screen.plugin_search_query.to_lowercase();
    let is_refreshing = screen.registry_refresh_in_progress;
    let phase: Option<PluginInstallPhase> = screen.plugin_install_phase.clone();
    let has_phase = phase.is_some();

    // Snapshot needed data before closures borrow cx
    let muted = cx.theme().muted_foreground;
    let accent = cx.theme().accent;

    // Filter registry plugins by search query
    let available: Vec<RegistryPlugin> = screen
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

    // Set of installed repo URLs for quick lookup
    let installed_urls: std::collections::HashSet<String> = screen
        .installed_plugins
        .iter()
        .map(|p| p.repo_url.clone())
        .collect();

    // Collect installed plugins not in any registry (for the "custom" footer)
    let custom_installed: Vec<InstalledPlugin> = screen
        .installed_plugins
        .iter()
        .filter(|p| {
            !screen
                .registry_plugins
                .iter()
                .any(|rp| rp.repo_url == p.repo_url)
        })
        .cloned()
        .collect();

    let registries_empty = screen.registry_plugins.is_empty();

    v_flex()
        .flex_1()
        .min_h_0()
        // ── Toolbar ─────────────────────────────────────────
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
                        .small()
                        .disabled(is_refreshing)
                        .tooltip(if is_refreshing {
                            "Refreshing…"
                        } else {
                            "Refresh plugin registries"
                        })
                        .on_click(cx.listener(|screen, _, _, cx| {
                            screen.refresh_registries(cx);
                        })),
                ),
        )
        // ── Install phase banner ─────────────────────────────
        .when(has_phase, |this| {
            this.child(render_plugin_phase(phase.unwrap(), cx))
        })
        // ── Registry plugin list ─────────────────────────────
        .child(
            v_flex()
                .id("registry-plugin-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(ScrollbarAxis::Vertical)
                .px_4()
                .pb_2()
                .gap_2()
                // Empty / loading states
                .when(is_refreshing && registries_empty, |this| {
                    this.child(
                        div()
                            .py_10()
                            .text_sm()
                            .text_color(muted)
                            .text_center()
                            .child("Fetching plugin registry…"),
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
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .text_center()
                                    .child("No plugins found. Click ↺ to refresh registries."),
                            ),
                    )
                })
                .when(!registries_empty && available.is_empty(), |this| {
                    this.child(
                        div()
                            .py_8()
                            .text_sm()
                            .text_color(muted)
                            .text_center()
                            .child("No plugins match your search"),
                    )
                })
                // Plugin cards
                .children(
                    available
                        .into_iter()
                        .map(|plugin| {
                            let is_installed = installed_urls.contains(&plugin.repo_url);
                            render_registry_plugin_card(plugin, is_installed, cx)
                        }),
                )
                // ── Custom-installed divider ─────────────────
                .when(!custom_installed.is_empty(), |this| {
                    this.child(
                        h_flex()
                            .mt_2()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(muted.opacity(0.25)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Custom installed"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(muted.opacity(0.25)),
                            ),
                    )
                    .children(
                        custom_installed
                            .into_iter()
                            .enumerate()
                            .map(|(idx, plugin)| render_custom_plugin_row(idx, plugin, cx)),
                    )
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
        // ── Top row: name + action button ────────────────────
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
                            Button::new(SharedString::from(format!("remove-reg-{}", repo_url_for_remove)))
                                .ghost()
                                .small()
                                .icon(IconName::Trash)
                                .tooltip("Remove")
                                .on_click(cx.listener(move |screen, _, _, cx| {
                                    if let Some(idx) = screen
                                        .installed_plugins
                                        .iter()
                                        .position(|p| p.repo_url == repo_url_for_remove)
                                    {
                                        screen.remove_plugin(idx, cx);
                                    }
                                })),
                        )
                        .into_any_element()
                } else {
                    Button::new(SharedString::from(format!("install-reg-{}", repo_url)))
                        .label("Install")
                        .primary()
                        .small()
                        .on_click({
                            let url = repo_url.clone();
                            cx.listener(move |screen, _, _, cx| {
                                screen.install_plugin(url.clone(), cx);
                            })
                        })
                        .into_any_element()
                }),
        )
        // ── Tags ─────────────────────────────────────────────
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
                            .bg(theme.accent.opacity(0.12))
                            .text_xs()
                            .text_color(theme.accent)
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
            "Fetching release info…",
            String::new(),
            None,
            false,
        ),
        PluginInstallPhase::Downloading { progress } => (
            IconName::Download,
            theme.accent,
            "Downloading binary…",
            format!("{:.0}%", progress * 100.0),
            Some(*progress),
            true,
        ),
        PluginInstallPhase::Building { logs } => (
            IconName::Settings,
            theme.accent,
            "Building from source…",
            logs.last().cloned().unwrap_or_default(),
            None,
            false,
        ),
        PluginInstallPhase::Complete(p) => (
            IconName::Check,
            theme.success_foreground,
            "Installed successfully!",
            format!("{} — {}", p.name, p.version),
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
                .when(matches!(phase, PluginInstallPhase::Complete(_) | PluginInstallPhase::Error(_)), |this| {
                    this.child(
                        Button::new("dismiss-phase")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .on_click(cx.listener(|screen, _, _, cx| {
                                screen.plugin_install_phase = None;
                                cx.notify();
                            })),
                    )
                }),
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
        .child(Icon::new(IconName::Package).size_4().text_color(theme.accent))
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
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(repo),
                        ),
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
                .small()
                .icon(IconName::Trash)
                .tooltip("Remove plugin")
                .on_click(cx.listener(move |screen, _, _, cx| {
                    screen.remove_plugin(idx, cx);
                })),
        )
}

fn render_theme_card(
    config: std::rc::Rc<ui::ThemeConfig>,
    current_name: &str,
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
            if is_dark { gpui::hsla(0., 0., 0.1, 1.) } else { gpui::hsla(0., 0., 0.97, 1.) }
        });
    let swatch_fg = config
        .colors
        .foreground
        .as_ref()
        .and_then(|h| gpui::Rgba::try_from(h.as_ref()).ok().map(gpui::Hsla::from))
        .unwrap_or_else(|| {
            if is_dark { gpui::hsla(0., 0., 0.95, 1.) } else { gpui::hsla(0., 0., 0.05, 1.) }
        });
    let accent_col = config
        .colors
        .accent
        .as_ref()
        .and_then(|h| gpui::Rgba::try_from(h.as_ref()).ok().map(gpui::Hsla::from))
        .unwrap_or_else(|| {
            if is_dark { gpui::hsla(0.6, 0.7, 0.5, 1.) } else { gpui::hsla(0.6, 0.7, 0.4, 1.) }
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
        .border_color(if is_active { theme.accent } else { theme.border })
        .gap_2()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
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
                .child(
                    div()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(if is_dark { theme.primary } else { theme.warning }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if is_dark { "Dark" } else { "Light" }),
                ),
        )
}

// ── Right column: Deps + Account ────────────────────────────

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
        .child(
            div()
                .flex_1()
                .min_h_0()
                .child(render_deps_card(
                    rust_installed,
                    build_tools_installed,
                    screen,
                    cx,
                )),
        )
        .child(
            div()
                .flex_shrink_0()
                .child(render_account_card(screen, cx)),
        )
}

// ── Deps card ───────────────────────────────────────────────

fn render_deps_card(
    rust_installed: bool,
    build_tools_installed: bool,
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let show_downloading = screen
        .install_progress
        .as_ref()
        .map(|p| matches!(p.status, InstallStatus::Downloading | InstallStatus::Installing))
        .unwrap_or(false);
    let bg = cx.theme().background;
    let border = cx.theme().border;

    let header = render_card_header(IconName::Package, "Dependencies", "Required build tools for Pulsar projects", cx);

    v_flex()
        .w_full()
        .flex_1()
        .h_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .overflow_hidden()
        .child(header)
        .child(
            v_flex()
                .id("deps-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(ScrollbarAxis::Vertical)
                .gap_3()
                .p_4()
                .child(render_dep_item(
                    "Rust Toolchain", rust_installed, None, cx,
                ))
                .child(render_dep_item(
                    "C/C++ Build Tools",
                    build_tools_installed,
                    screen
                        .dependency_status
                        .as_ref()
                        .and_then(|s| s.compiler_info.clone()),
                    cx,
                ))
                .children(
                    screen
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

// ── Account card ────────────────────────────────────────────

fn render_account_card(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let bg = cx.theme().background;
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    let accent = cx.theme().accent;
    let fg = cx.theme().foreground;
    let header = render_card_header(
        IconName::Group,
        "Account",
        "Sync settings and collaborate",
        cx,
    );

    let profile = screen.auth_profile();
    let code = screen.auth_device_code.clone();
    let message = screen.auth_message.clone();
    let loading = screen.auth_loading;

    // Load avatar if profile has a new URL
    if let Some(ref p) = profile {
        let url = p.avatar_url.clone().unwrap_or_default();
        if !url.is_empty() && screen.onboarding_avatar_url.as_deref() != Some(url.as_str()) {
            screen.onboarding_avatar_url = Some(url.clone());
            screen.onboarding_avatar = None;
            let url_clone = url.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let client = reqwest::blocking::Client::builder()
                            .timeout(std::time::Duration::from_secs(10))
                            .user_agent("Pulsar-Native/1.0")
                            .build()
                            .map_err(|e| e.to_string())?;
                        let response = client.get(&url_clone).send().map_err(|e| e.to_string())?;
                        let bytes = response.bytes().map_err(|e| e.to_string())?;
                        let rgba = image::load_from_memory(&bytes)
                            .map_err(|e| format!("decode: {e}"))?
                            .into_rgba8();
                        let frame = image::Frame::new(rgba);
                        Ok::<_, String>(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
                    })
                    .await;
                if let Ok(img) = result {
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.onboarding_avatar = Some(img);
                            cx.notify();
                        });
                    });
                }
            })
            .detach();
        }
    }

    let avatar_img = screen.onboarding_avatar.clone();

    v_flex()
        .w_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .overflow_hidden()
        .child(header)
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
                            .bg(accent.opacity(0.2))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(accent)
                                    .child(initial),
                            )
                            .into_any_element()
                    };

                    this.child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(avatar)
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .child(profile.display_name.unwrap_or(profile.login.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(muted)
                                            .child(format!("@{}", profile.login)),
                                    ),
                            ),
                    )
                })
                .when(
                    profile.is_none() && code.is_none() && !loading,
                    |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(muted)
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
                    },
                )
                .when(loading, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Signing in…"),
                    )
                })
                .when_some(code, |this, code| {
                    this.child(
                        v_flex()
                            .gap_2()
                            .p_3()
                            .bg(accent.opacity(0.12))
                            .rounded_lg()
                            .border_1()
                            .border_color(accent.opacity(0.35))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Paste this code in the browser window:"),
                            )
                            .child(
                                div()
                                    .text_center()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
                                    .child(code),
                            ),
                    )
                })
                .when_some(message, |this, msg| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(msg),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Your data stays private. Sign-in is optional."),
                ),
        )
}

// ── Shared helpers ──────────────────────────────────────────

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
        .child(Icon::new(icon).size_5().text_color(theme.accent))
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
                .children(
                    progress
                        .logs
                        .iter()
                        .rev()
                        .take(20)
                        .rev()
                        .map(|log| {
                            div().text_xs().text_color(theme.muted_foreground).child(log.clone())
                        }),
                ),
        )
}

// ── Install logic ───────────────────────────────────────────

fn run_setup_script(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) {
    screen.install_progress = Some(InstallProgress {
        logs: vec!["Starting installation...".to_string()],
        progress: 0.0,
        status: InstallStatus::Downloading,
    });

    let progress = Arc::new(Mutex::new(screen.install_progress.clone().unwrap()));
    let progress_clone = Arc::clone(&progress);

    cx.spawn(async move |this, cx| {
        let result = cx
            .background_executor()
            .spawn(async move { install_rust_with_progress(progress_clone) })
            .await;

        if let Err(e) = result {
            let mut prog = progress.lock().unwrap();
            prog.status = InstallStatus::Error(format!("Installation failed: {}", e));
            prog.logs.push(format!("Error: {}", e));
        }

        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;

            let should_break = cx
                .update(|cx| {
                    this.update(cx, |screen, cx| {
                        if let Ok(prog) = progress.lock() {
                            screen.install_progress = Some(prog.clone());
                            cx.notify();
                            matches!(
                                prog.status,
                                InstallStatus::Complete | InstallStatus::Error(_)
                            )
                        } else {
                            false
                        }
                    })
                })
                .unwrap_or(false);

            if should_break {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.check_dependencies_async(cx);
                    });
                });
                break;
            }
        }
    })
    .detach();
}

fn install_rust_with_progress(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        install_rust_windows(progress)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        install_rust_unix(progress)
    }
}

#[cfg(target_os = "windows")]
fn install_rust_windows(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;

    let exe_path = std::env::temp_dir().join("rustup-init.exe");

    let rustup_exists = Command::new("rustup").arg("--version").output().is_ok();

    if rustup_exists {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("Existing Rust installation detected".to_string());
        prog.logs.push("Stopping all Rust processes...".to_string());
        prog.progress = 0.02;
        drop(prog);

        let rust_processes = [
            "rustc",
            "cargo",
            "rustup",
            "rust-analyzer",
            "rls",
            "rustfmt",
            "cargo-clippy",
            "cargo-fmt",
            "rustdoc",
        ];

        for process in &rust_processes {
            let _ = Command::new("taskkill")
                .args(["/F", "/IM", &format!("{}.exe", process)])
                .creation_flags(0x08000000)
                .output();
        }

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Waiting for processes to terminate...".to_string());
            prog.progress = 0.04;
        }

        std::thread::sleep(std::time::Duration::from_secs(3));

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Uninstalling old Rust version...".to_string());
            prog.progress = 0.05;
        }

        let _ = Command::new("rustup")
            .args(["self", "uninstall", "-y"])
            .creation_flags(0x08000000)
            .output();

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Cleaning up installation directories...".to_string());
            prog.progress = 0.07;
        }

        std::thread::sleep(std::time::Duration::from_secs(3));

        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let cargo_home = format!("{}/.cargo", home);
        let rustup_home = format!("{}/.rustup", home);

        let _ = std::fs::remove_dir_all(&cargo_home);
        let _ = std::fs::remove_dir_all(&rustup_home);

        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Old installation cleaned up".to_string());
            prog.progress = 0.09;
        }
    }

    {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("Downloading rustup installer...".to_string());
        prog.progress = 0.1;
        prog.status = InstallStatus::Downloading;
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;

    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push(format!("Downloaded {} bytes", bytes.len()));
        prog.progress = 0.3;
    }

    let mut file = std::fs::File::create(&exe_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("Running rustup installer with elevated privileges...".to_string());
        prog.logs
            .push("Please accept the UAC prompt if it appears".to_string());
        prog.progress = 0.4;
        prog.status = InstallStatus::Installing;
    }

    let status = runas::Command::new(&exe_path)
        .args(&[
            "-y",
            "--default-toolchain",
            "stable",
            "--profile",
            "minimal",
        ])
        .show(false)
        .status()
        .map_err(|e| e.to_string())?;

    if status.success() {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("✅ Rust installed successfully!".to_string());
        prog.logs
            .push("Adding Windows Defender exclusions...".to_string());
        drop(prog);

        add_windows_defender_exclusions(&progress);

        let mut prog = progress.lock().unwrap();
        prog.progress = 1.0;
        prog.status = InstallStatus::Complete;
    } else {
        return Err(format!("Rustup installer exited with status: {:?}", status));
    }

    let _ = std::fs::remove_file(&exe_path);

    Ok(())
}

#[cfg(target_os = "windows")]
fn add_windows_defender_exclusions(progress: &Arc<Mutex<InstallProgress>>) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let home = match std::env::var("USERPROFILE") {
        Ok(h) => h,
        Err(_) => return,
    };

    let exclusions = vec![format!("{}/.cargo", home), format!("{}/.rustup", home)];

    let mut prog = progress.lock().unwrap();
    prog.logs
        .push("Requesting admin privileges to add exclusions...".to_string());
    drop(prog);

    let mut ps_commands = Vec::new();
    for path in &exclusions {
        ps_commands.push(format!("Add-MpPreference -ExclusionPath '{}'", path));
    }
    ps_commands.push("Add-MpPreference -ExclusionProcess 'rustc.exe'".to_string());
    ps_commands.push("Add-MpPreference -ExclusionProcess 'cargo.exe'".to_string());

    let full_command = ps_commands.join("; ");

    let result = runas::Command::new("powershell")
        .args(&["-NoProfile", "-Command", &full_command])
        .show(false)
        .status();

    let mut prog = progress.lock().unwrap();
    match result {
        Ok(status) if status.success() => {
            prog.logs
                .push("✅ Windows Defender exclusions added successfully!".to_string());
            prog.logs
                .push("Cargo builds will no longer be blocked".to_string());
        }
        Ok(_) => {
            prog.logs
                .push("⚠️ Failed to add Windows Defender exclusions".to_string());
            prog.logs
                .push("You may need to add them manually in Windows Security".to_string());
        }
        Err(e) => {
            prog.logs
                .push(format!("⚠️ Could not add exclusions: {}", e));
            prog.logs
                .push("Builds may be slower due to antivirus scanning".to_string());
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn install_rust_unix(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let script_path = std::env::temp_dir().join("rustup-init.sh");

    let rustup_exists = Command::new("rustup").arg("--version").output().is_ok();

    if rustup_exists {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("Existing Rust installation detected".to_string());
        prog.logs.push("Stopping all Rust processes...".to_string());
        prog.progress = 0.02;
        drop(prog);

        let rust_processes = [
            "rustc",
            "cargo",
            "rustup",
            "rust-analyzer",
            "rls",
            "rustfmt",
            "cargo-clippy",
            "cargo-fmt",
            "rustdoc",
        ];

        for process in &rust_processes {
            let _ = Command::new("pkill").arg(process).output();
        }

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Waiting for processes to terminate...".to_string());
            prog.progress = 0.04;
        }

        std::thread::sleep(std::time::Duration::from_secs(3));

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Uninstalling old Rust version...".to_string());
            prog.progress = 0.05;
        }

        let _ = Command::new("rustup")
            .args(&["self", "uninstall", "-y"])
            .output();

        {
            let mut prog = progress.lock().unwrap();
            prog.logs
                .push("Cleaning up installation directories...".to_string());
            prog.progress = 0.07;
        }

        std::thread::sleep(std::time::Duration::from_secs(3));

        let home = std::env::var("HOME").unwrap_or_default();
        let cargo_home = format!("{}/.cargo", home);
        let rustup_home = format!("{}/.rustup", home);

        let _ = std::fs::remove_dir_all(&cargo_home);
        let _ = std::fs::remove_dir_all(&rustup_home);

        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Old installation cleaned up".to_string());
            prog.progress = 0.09;
        }
    }

    {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("Downloading rustup installer...".to_string());
        prog.progress = 0.1;
        prog.status = InstallStatus::Downloading;
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;

    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push(format!("Downloaded {} bytes", bytes.len()));
        prog.progress = 0.3;
    }

    let mut file = std::fs::File::create(&script_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let mut perms = std::fs::metadata(&script_path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;

    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Running rustup installer...".to_string());
        prog.logs.push("May require sudo password".to_string());
        prog.progress = 0.4;
        prog.status = InstallStatus::Installing;
    }

    let status = Command::new("sh")
        .args(&[
            script_path.to_str().unwrap(),
            "-y",
            "--default-toolchain",
            "stable",
            "--profile",
            "default",
        ])
        .status()
        .map_err(|e| e.to_string())?;

    if status.success() {
        let mut prog = progress.lock().unwrap();
        prog.logs
            .push("✅ Rust installed successfully!".to_string());
        prog.progress = 1.0;
        prog.status = InstallStatus::Complete;
    } else {
        return Err(format!("Rustup installer exited with status: {:?}", status));
    }

    let _ = std::fs::remove_file(&script_path);

    Ok(())
}
