//! Event handler implementations

use gpui::{AppContext, Context, DismissEvent, Entity, Focusable, Window};
use ui::{ContextModal, dock::PanelEvent};
use ui::notification::Notification;
use ui_editor::{FileManagerDrawer, FileSelected, DrawerFileType as FileType, PopoutFileManagerEvent, ProblemsDrawer, ScriptEditorPanel, TextEditorEvent};
use ui_entry::{EntryScreen, ProjectSelected};
use ui_alias_editor::ShowTypePickerRequest;
use engine_backend::services::rust_analyzer_manager::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};

use super::PulsarApp;

pub fn on_analyzer_event(
    app: &mut PulsarApp,
    _manager: &Entity<RustAnalyzerManager>,
    event: &AnalyzerEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    match event {
        AnalyzerEvent::StatusChanged(status) => {
            match status {
                AnalyzerStatus::Idle => {
                    app.state.analyzer_status_text = "Idle".to_string();
                    app.state.analyzer_detail_message = String::new();
                    app.state.analyzer_progress = 0.0;
                }
                AnalyzerStatus::Starting => {
                    app.state.analyzer_status_text = "Starting...".to_string();
                    app.state.analyzer_detail_message = "Initializing language server".to_string();
                    app.state.analyzer_progress = 0.0;
                }
                AnalyzerStatus::Indexing { progress, message } => {
                    app.state.analyzer_status_text = "Indexing".to_string();
                    app.state.analyzer_detail_message = message.clone();
                    app.state.analyzer_progress = *progress;
                }
                AnalyzerStatus::Ready => {
                    app.state.analyzer_status_text = "Ready".to_string();
                    app.state.analyzer_detail_message = "Code intelligence active".to_string();
                    app.state.analyzer_progress = 1.0;
                    window.push_notification(
                        ui::notification::Notification::success("Rust Analyzer Ready")
                            .message("Code intelligence is now available"),
                        cx
                    );
                }
                AnalyzerStatus::Error(e) => {
                    app.state.analyzer_status_text = "Error".to_string();
                    app.state.analyzer_detail_message = e.to_string();
                    app.state.analyzer_progress = 0.0;
                    window.push_notification(
                        ui::notification::Notification::error("Analyzer Error")
                            .message(e.to_string()),
                        cx
                    );
                }
                AnalyzerStatus::Stopped => {
                    app.state.analyzer_status_text = "Stopped".to_string();
                    app.state.analyzer_detail_message = String::new();
                    app.state.analyzer_progress = 0.0;
                }
            };
            cx.notify();
        }
        AnalyzerEvent::IndexingProgress { progress, message } => {
            app.state.analyzer_status_text = "Indexing".to_string();
            app.state.analyzer_detail_message = message.clone();
            app.state.analyzer_progress = *progress;
            cx.notify();
        }
        AnalyzerEvent::Ready => {
            app.state.analyzer_status_text = "Ready".to_string();
            app.state.analyzer_detail_message = "Code intelligence active".to_string();
            app.state.analyzer_progress = 1.0;
            window.push_notification(
                ui::notification::Notification::success("Code Intelligence Ready")
                    .message("Rust Analyzer indexing complete"),
                cx
            );
            cx.notify();
        }
        AnalyzerEvent::Error(e) => {
            app.state.analyzer_status_text = "Error".to_string();
            app.state.analyzer_detail_message = e.to_string();
            app.state.analyzer_progress = 0.0;
            window.push_notification(
                ui::notification::Notification::error("Analyzer Error").message(e.to_string()),
                cx
            );
            cx.notify();
        }
        AnalyzerEvent::Diagnostics(diagnostics) => {
            // Convert and forward diagnostics to the problems drawer
            let problems_diagnostics: Vec<ui_problems::Diagnostic> = diagnostics.iter().map(|d| {
                ui_problems::Diagnostic {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    column: d.column,
                    severity: match d.severity {
                        ui_common::DiagnosticSeverity::Error => ui_problems::DiagnosticSeverity::Error,
                        ui_common::DiagnosticSeverity::Warning => ui_problems::DiagnosticSeverity::Warning,
                        ui_common::DiagnosticSeverity::Information => ui_problems::DiagnosticSeverity::Information,
                        ui_common::DiagnosticSeverity::Hint => ui_problems::DiagnosticSeverity::Hint,
                    },
                    message: d.message.clone(),
                    source: d.source.clone(),
                }
            }).collect();
            app.state.problems_drawer.update(cx, |drawer, cx| {
                drawer.set_diagnostics(problems_diagnostics, cx);
            });
            cx.notify();
        }
    }
}

pub fn on_project_selected(
    app: &mut PulsarApp,
    _selector: &Entity<EntryScreen>,
    event: &ProjectSelected,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    println!("[PROJECT_SELECTED] Received path: {:?}", event.path);
    
    app.state.project_path = Some(event.path.clone());
    app.state.entry_screen = None;

    // Update file manager with project path
    app.state.file_manager_drawer.update(cx, |drawer, cx| {
        drawer.set_project_path(event.path.clone(), cx);
    });

    // Start rust analyzer for the project
    app.state.rust_analyzer.update(cx, |analyzer, cx| {
        analyzer.start(event.path.clone(), window, cx);
    });

    // Update Discord presence with new project
    app.update_discord_presence(cx);

    println!("Project selected: {:?}", event.path);
    cx.notify();
}

pub fn on_tab_panel_event(
    app: &mut PulsarApp,
    _tabs: &Entity<ui::dock::TabPanel>,
    event: &PanelEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    match event {
        PanelEvent::MoveToNewWindow(panel, position) => {
            app.create_detached_window(panel.clone(), *position, window, cx);
        }
        PanelEvent::TabClosed(entity_id) => {
            app.state.daw_editors.retain(|e| e.entity_id() != *entity_id);
            app.state.database_editors.retain(|e| e.entity_id() != *entity_id);
            app.state.struct_editors.retain(|e| e.entity_id() != *entity_id);
            app.state.enum_editors.retain(|e| e.entity_id() != *entity_id);
            app.state.trait_editors.retain(|e| e.entity_id() != *entity_id);
            app.state.alias_editors.retain(|e| e.entity_id() != *entity_id);

            // Update Discord presence when tab is closed
            app.update_discord_presence(cx);
        }
        PanelEvent::TabChanged { active_index: _ } => {
            // Update Discord presence when active tab changes
            app.update_discord_presence(cx);
        }
        _ => {}
    }
}

pub fn on_file_selected(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    event: &FileSelected,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!(
        "FileSelected event received - path: {:?}, type: {:?}",
        event.path, event.file_type
    );

    // Try to open via plugin system first
    match app.state.plugin_manager.create_editor_for_file(&event.path, window, cx) {
        Ok((panel, _editor_instance)) => {
            tracing::info!("✅ Plugin system successfully created editor for: {:?}", event.path);

            app.state.center_tabs.update(cx, |tabs, cx| {
                tabs.add_panel(panel, window, cx);
            });

            app.state.drawer_open = false;
            app.update_discord_presence(cx);
            cx.notify();
            return;
        }
        Err(e) => {
            tracing::debug!("Plugin manager couldn't open file: {} - falling through to legacy code", e);
        }
    }

    // Legacy hardcoded file opening
    match event.file_type {
        FileType::Class => {
            tracing::error!("Blueprint editor tried to load via legacy code, but it's now plugin-only!");
        }
        FileType::Script | FileType::Config | FileType::Document => {
            tracing::warn!("Opening script tab using legacy code (should use plugin system)");
            app.open_script_tab(event.path.clone(), window, cx);
        }
        FileType::DawProject => {
            tracing::warn!("Opening DAW tab using legacy code: {:?}", event.path);
            app.open_daw_tab(event.path.clone(), window, cx);
        }
        FileType::LevelScene => {
            tracing::info!("Opening level editor for scene: {:?}", event.path);
            app.open_level_editor_tab(event.path.clone(), window, cx);
        }
        FileType::Database => {
            tracing::warn!("Opening database tab using legacy code: {:?}", event.path);
            app.open_database_tab(event.path.clone(), window, cx);
        }
        FileType::StructType => {
            tracing::warn!("Opening struct editor using legacy code: {:?}", event.path);
            app.open_struct_tab(event.path.clone(), window, cx);
        }
        FileType::EnumType => {
            tracing::warn!("Opening enum editor using legacy code: {:?}", event.path);
            app.open_enum_tab(event.path.clone(), window, cx);
        }
        FileType::TraitType => {
            tracing::warn!("Opening trait editor using legacy code: {:?}", event.path);
            app.open_trait_tab(event.path.clone(), window, cx);
        }
        FileType::AliasType => {
            tracing::warn!("Opening alias editor using legacy code: {:?}", event.path);
            app.open_alias_tab(event.path.clone(), window, cx);
        }
        _ => {
            tracing::debug!("Unknown file type, ignoring");
        }
    }

    app.state.drawer_open = false;
    cx.notify();
}

pub fn on_popout_file_manager(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    event: &PopoutFileManagerEvent,
    _window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
    use ui::Root;

    let project_path = event.project_path.clone();
    app.state.drawer_open = false;
    cx.notify();

    // Open the file manager window
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point {
                    x: px(100.0),
                    y: px(100.0),
                },
                size: size(px(1000.0), px(700.0)),
            })),
            titlebar: Some(gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: None,
            }),
            kind: WindowKind::Normal,
            is_resizable: true,
            window_decorations: Some(gpui::WindowDecorations::Client),
            window_min_size: Some(gpui::Size {
                width: px(600.),
                height: px(400.),
            }),
            ..Default::default()
        },
        move |window, cx| {
            let new_drawer = cx.new(|cx| FileManagerDrawer::new_in_window(project_path.clone(), window, cx));
            let file_manager_window = cx.new(|cx| ui_file_manager::FileManagerWindow::new(new_drawer, window, cx));
            cx.new(|cx| Root::new(file_manager_window.into(), window, cx))
        },
    );
}

pub fn on_navigate_to_diagnostic(
    app: &mut PulsarApp,
    _drawer: &Entity<ProblemsDrawer>,
    event: &ui_problems::NavigateToDiagnostic,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    println!(
        "📂 Navigating to diagnostic: {:?} at line {}, column {}",
        event.file_path, event.line, event.column
    );

    app.open_script_tab(event.file_path.clone(), window, cx);

    if let Some(script_editor) = &app.state.script_editor {
        script_editor.update(cx, |editor, cx| {
            editor.go_to_line(event.line, event.column, window, cx);
        });
    }
}

pub fn on_text_editor_event(
    _app: &mut PulsarApp,
    _editor: &Entity<ScriptEditorPanel>,
    _event: &TextEditorEvent,
    _window: &mut Window,
    _cx: &mut Context<PulsarApp>,
) {
    // LSP notifications are now handled by ScriptEditor internally
}

pub fn on_show_type_picker_request(
    app: &mut PulsarApp,
    editor: &Entity<ui_alias_editor::AliasEditor>,
    event: &ShowTypePickerRequest,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    use crate::unified_palette::AnyPaletteDelegate;
    use ui_common::command_palette::GenericPalette;

    app.state.active_type_picker_editor = Some(editor.clone());

    if let Some(palette) = &app.state.command_palette {
        palette.update(cx, |palette, cx| {
            let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
            palette.swap_delegate(delegate, window, cx);
        });

        let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
        input_handle.focus(window);
    } else {
        let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
        let palette = cx.new(|cx| GenericPalette::new(delegate, window, cx));

        cx.subscribe_in(&palette, window, |app: &mut PulsarApp, palette, _event: &DismissEvent, window, cx| {
            let selected_item = palette.update(cx, |palette, _cx| {
                palette.delegate_mut().take_selected_command()
            });

            let selected_type = palette.update(cx, |palette, _cx| {
                palette.delegate_mut().take_selected_type()
            });

            if let Some(item) = selected_item {
                app.handle_command_or_file_selected(item, window, cx);
            }

            if let Some((type_item, target_slot)) = selected_type {
                if let Some(editor) = &app.state.active_type_picker_editor {
                    editor.update(cx, |ed, cx| {
                        ed.add_type_from_picker(&type_item, target_slot, cx);
                    });
                }
                app.state.active_type_picker_editor = None;
            }

            app.state.command_palette_open = false;
            app.state.focus_handle.focus(window);
            cx.notify();
        }).detach();

        let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
        input_handle.focus(window);

        app.state.command_palette = Some(palette);
    }

    app.state.command_palette_open = true;
    cx.notify();
}
