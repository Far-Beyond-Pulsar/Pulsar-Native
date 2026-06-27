use crate::entry_screen::{EntryScreen, InstallProgress, InstallStatus};
use gpui::{prelude::*, *};
use std::process::Command;
use std::sync::{Arc, Mutex};
use ui::{
    button::{Button, ButtonVariants},
    h_flex, scroll::ScrollbarAxis, v_flex, ActiveTheme, Disableable, Icon, IconName, StyledExt,
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
                .child(render_theme_column(screen, cx))
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

// ── Theme column (left) ──────────────────────────────────────

fn render_theme_column(
    _screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let current_name = cx.theme().theme_name().clone();
    let themes: Vec<std::rc::Rc<ui::ThemeConfig>> = ui::ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .cloned()
        .collect();
    let bg = cx.theme().background;
    let border = cx.theme().border;

    let header = render_card_header(IconName::Palette, "Theme", "Choose your editor appearance", cx);

    v_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .overflow_hidden()
        .child(header)
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
                        .children(themes.into_iter().map(|config| {
                            render_theme_card(config, &current_name, cx)
                        })),
                ),
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

                    this.child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                div()
                                    .w(px(56.))
                                    .h(px(56.))
                                    .rounded_full()
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
                                    ),
                            )
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
