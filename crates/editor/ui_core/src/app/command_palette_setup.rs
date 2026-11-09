//! Registers the editor's command palette: fixed commands, GPUI actions,
//! registered windows and project files.
//!
//! Entry point: [`register_command_palette`], called once from
//! `PulsarApp::new_internal` after `app.state` exists. The palette is filled
//! from four sources, one `add_*` function each; project files come from the
//! loading screen's pre-scan when available and from a background scan
//! otherwise.

use gpui::{Context, Window};
use rust_i18n::t;
use ui::ContextModal;
use ui::IconName;
use ui_common::command_palette::{Palette, PaletteManager};

use crate::actions::*;

use super::PulsarApp;

/// Creates the "commands" palette, fills it, and stores its handles in
/// `app.state`. Depends on `app.state.project_path` being set (used for the
/// background file scan).
pub(super) fn register_command_palette(
    app: &mut PulsarApp,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    // Drain the file list pre-scanned by the loading-screen background
    // thread.  When the normal loading-screen path is used this is
    // already populated; the vec is empty only when the editor is opened
    // without a loading screen (e.g. first launch / entry-screen path).
    let preloaded_files = ui_loading_screen::take_preloaded_files();
    let had_preloaded_files = !preloaded_files.is_empty();

    let mut registered_actions: Vec<_> = gpui::generate_list_of_all_registered_actions().collect();
    registered_actions.sort_by(|a, b| a.name.cmp(b.name));
    let registered_windows = {
        use gpui::UpdateGlobal as _;
        let mut names = window_manager::WindowRegistry::update_global(cx, |registry, _| {
            registry.registered_names()
        });
        names.sort_unstable();
        names
    };

    let (palette_id, palette_ref) = PaletteManager::register_palette("commands", window, cx);

    // Populate with command items
    palette_ref.update(cx, |palette, cx| {
        add_fixed_commands(palette, cx);
        add_registered_actions(palette, registered_actions, cx);
        add_registered_windows(palette, registered_windows, cx);
        add_preloaded_files(palette, preloaded_files, cx);
    });

    app.state.command_palette_id = Some(palette_id);
    app.state.command_palette = Some(palette_ref.clone());

    // Slow-path fallback: when there were no pre-loaded files (entry-screen
    // launch, project-switcher, URI open without loading screen) scan the
    // project directory on the background executor so the main thread never
    // blocks.  The palette entries appear shortly after the window opens.
    if !had_preloaded_files {
        if let Some(ref project_path) = app.state.project_path {
            use ui_common::file_utils::find_openable_files;
            let project_path_bg = project_path.clone();
            cx.spawn(async move |_, cx| {
                let files = cx
                    .background_executor()
                    .spawn(async move { find_openable_files(&project_path_bg, Some(1000)) })
                    .await;
                cx.update(|cx| {
                    palette_ref.update(cx, |palette, cx| {
                        for file in files {
                            let path = file.path.clone();
                            let open_path = path.clone();
                            let item_id = palette.add_item(
                                file.name.clone(),
                                file.path.to_string_lossy().to_string(),
                                IconName::SubmitDocument,
                                "Files",
                                move |window, cx| {
                                    window.dispatch_action(
                                        Box::new(OpenFile {
                                            path: open_path.clone(),
                                        }),
                                        cx,
                                    );
                                },
                                cx,
                            );
                            palette.set_item_thumbnail_path(item_id, path);
                        }
                    });
                });
            })
            .detach();
        }
    }
}

/// The hand-written commands (toggles, settings, build/run). These are the
/// ones whose action names `add_registered_actions` skips as duplicates.
fn add_fixed_commands(palette: &mut Palette, cx: &mut Context<Palette>) {
    palette.add_item(
        "Toggle File Manager",
        "Show or hide the file manager panel",
        IconName::Folder,
        "View",
        |window, cx| {
            window.dispatch_action(Box::new(ToggleFileManager), cx);
        },
        cx,
    );

    palette.add_item(
        "Toggle Unified Sidebar",
        "Switch between the tab bar and the left sidebar of editors and folders",
        IconName::PanelLeft,
        "View",
        |window, cx| {
            window.dispatch_action(Box::new(crate::actions::ToggleUnifiedSidebar), cx);
        },
        cx,
    );

    palette.add_item(
        "Open Settings",
        "Open application settings",
        IconName::Settings,
        "Application",
        |window, cx| {
            window.dispatch_action(Box::new(ui::OpenSettings), cx);
        },
        cx,
    );

    palette.add_item(
        "Toggle Multiplayer",
        "Enable or disable multiplayer collaboration",
        IconName::User,
        "Application",
        |window, cx| {
            window.dispatch_action(Box::new(ToggleMultiplayer), cx);
        },
        cx,
    );

    palette.add_item(
        "Toggle Agent Chat",
        "Show or hide the global agent chat side panel",
        IconName::PanelRight,
        "View",
        |window, cx| {
            window.dispatch_action(Box::new(ToggleAgentChat), cx);
        },
        cx,
    );

    palette.add_item(
        "Toggle Problems",
        "Show or hide the problems panel",
        IconName::TriangleAlert,
        "View",
        |window, cx| {
            window.dispatch_action(Box::new(ToggleProblems), cx);
        },
        cx,
    );

    palette.add_item(
        "Build Project",
        "Build the current project",
        IconName::Hammer,
        "Project",
        |window, cx| {
            window.push_notification(
                ui::notification::Notification::info(t!("Notification.Title.Build").to_string())
                    .message(t!("Notification.Message.BuildingProject").to_string()),
                cx,
            );
        },
        cx,
    );

    palette.add_item(
        "Run Project",
        "Run the current project",
        IconName::Play,
        "Project",
        |window, cx| {
            window.push_notification(
                ui::notification::Notification::info(t!("Notification.Title.Run").to_string())
                    .message(t!("Notification.Message.RunningProject").to_string()),
                cx,
            );
        },
        cx,
    );
}

/// Every documented, zero-argument GPUI action not already added by hand.
fn add_registered_actions(
    palette: &mut Palette,
    registered_actions: Vec<gpui::MacroActionData>,
    cx: &mut Context<Palette>,
) {
    // GPUI's inventory-backed action registry is the source of
    // commandable actions. Only documented, zero-argument actions
    // are promoted automatically; required-parameter actions stay
    // with the workflows that know how to supply those values.
    let manually_named = [
        "ToggleFileManager",
        "ToggleUnifiedSidebar",
        "ToggleProblems",
        "ToggleMultiplayer",
        "ToggleAgentChat",
    ];
    for action in registered_actions {
        let Some(description) = action.documentation else {
            continue;
        };
        let short_name = action.name.rsplit("::").next().unwrap_or(action.name);
        if manually_named
            .iter()
            .any(|manual| short_name.ends_with(manual))
            || (action.build)(serde_json::json!({})).is_err()
        {
            continue;
        }
        let action_name = action.name.to_string();
        let display_name = humanize_action_name(short_name);
        let build_action = action.build;
        palette.add_item_with_keywords(
            display_name,
            description,
            IconName::Settings,
            "Actions",
            vec![action_name.clone(), "action".into(), "command".into()],
            move |window, cx| match build_action(serde_json::json!({})) {
                Ok(action) => window.dispatch_action(action, cx),
                Err(error) => {
                    tracing::warn!(%error, "Could not build command palette action")
                }
            },
            cx,
        );
    }
}

/// One "Open <window>" entry per window in the window registry.
fn add_registered_windows(
    palette: &mut Palette,
    registered_windows: Vec<&'static str>,
    cx: &mut Context<Palette>,
) {
    // Registered windows can be opened by name, so surface them
    // without maintaining a second hand-written command list.
    for name in registered_windows {
        let label = format!("Open {}", humanize_action_name(name));
        let registered_name = name;
        palette.add_item_with_keywords(
            label,
            "Open this registered engine window",
            IconName::Folder,
            "Windows",
            vec![name.to_string(), "window".into(), "open".into()],
            move |_window, cx| {
                use gpui::UpdateGlobal as _;
                window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                    registry.open(registered_name, cx)
                });
            },
            cx,
        );
    }
}

/// Project files the loading screen already scanned, so the main thread does
/// no disk I/O.
fn add_preloaded_files(
    palette: &mut Palette,
    preloaded_files: Vec<ui_loading_screen::PreloadedFileEntry>,
    cx: &mut Context<Palette>,
) {
    // Fast path: use the file list that the loading-screen background
    // thread already scanned — no disk I/O on the main thread.
    for entry in preloaded_files {
        let path = entry.path.clone();
        let open_path = path.clone();
        let item_id = palette.add_item(
            entry.name,
            entry.path.to_string_lossy().to_string(),
            IconName::SubmitDocument,
            "Files",
            move |window, cx| {
                window.dispatch_action(
                    Box::new(OpenFile {
                        path: open_path.clone(),
                    }),
                    cx,
                );
            },
            cx,
        );
        palette.set_item_thumbnail_path(item_id, path);
    }
}

fn humanize_action_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 8);
    let mut previous = None;
    for character in name.chars() {
        if matches!(character, '_' | '-') {
            if !result.ends_with(' ') {
                result.push(' ');
            }
        } else {
            if character.is_ascii_uppercase()
                && previous.is_some_and(|previous: char| {
                    previous.is_ascii_lowercase() || previous.is_ascii_digit()
                })
                && !result.ends_with(' ')
            {
                result.push(' ');
            }
            result.push(character);
        }
        previous = Some(character);
    }
    result.trim().to_string()
}
