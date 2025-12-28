//! Event handler implementations

use gpui::{AppContext, Context, DismissEvent, Entity, Focusable, Window};
use ui::{ContextModal, dock::PanelEvent};
use ui_editor::{FileManagerDrawer, FileSelected, DrawerFileType as FileType, PopoutFileManagerEvent, ProblemsDrawer, ScriptEditorPanel, TextEditorEvent};
use ui_entry::{EntryScreen, ProjectSelected};
use ui_alias_editor::ShowTypePickerRequest;
use engine_backend::services::rust_analyzer_manager::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
use std::path::PathBuf;

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
            // Then asynchronously request code actions for each diagnostic
            
            // First, separate hints from errors/warnings
            let mut errors_warnings: Vec<_> = Vec::new();
            let mut hints: Vec<_> = Vec::new();
            
            for d in diagnostics.iter() {
                match d.severity {
                    ui_common::DiagnosticSeverity::Hint => hints.push(d),
                    _ => errors_warnings.push(d),
                }
            }
            
            // Convert errors/warnings to problems diagnostics (with loading state)
            let mut problems_diagnostics: Vec<ui_problems::Diagnostic> = errors_warnings.iter().map(|d| {
                ui_problems::Diagnostic {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    column: d.column,
                    end_line: d.end_line,
                    end_column: d.end_column,
                    severity: match d.severity {
                        ui_common::DiagnosticSeverity::Error => ui_problems::DiagnosticSeverity::Error,
                        ui_common::DiagnosticSeverity::Warning => ui_problems::DiagnosticSeverity::Warning,
                        ui_common::DiagnosticSeverity::Information => ui_problems::DiagnosticSeverity::Information,
                        ui_common::DiagnosticSeverity::Hint => ui_problems::DiagnosticSeverity::Hint,
                    },
                    message: d.message.clone(),
                    source: d.source.clone(),
                    hints: Vec::new(),
                    subitems: Vec::new(),
                    loading_actions: true, // Mark as loading
                }
            }).collect();
            
            // Attach hints to their closest parent error/warning in the same file
            for hint in hints {
                let mut best_match: Option<usize> = None;
                let mut best_distance: usize = usize::MAX;
                
                for (i, parent) in problems_diagnostics.iter().enumerate() {
                    if parent.file_path == hint.file_path {
                        let distance = if hint.line >= parent.line {
                            hint.line - parent.line
                        } else {
                            parent.line - hint.line
                        };
                        
                        if distance < best_distance {
                            best_distance = distance;
                            best_match = Some(i);
                        }
                    }
                }
                
                if let Some(parent_idx) = best_match {
                    // Add the hint message (actual code actions will be fetched async)
                    problems_diagnostics[parent_idx].hints.push(ui_problems::Hint {
                        message: hint.message.clone(),
                        before_content: None,
                        after_content: None,
                        file_path: Some(hint.file_path.clone()),
                        line: Some(hint.line),
                        loading: false,
                    });
                } else {
                    // No parent found, add as a standalone diagnostic
                    problems_diagnostics.push(ui_problems::Diagnostic {
                        file_path: hint.file_path.clone(),
                        line: hint.line,
                        column: hint.column,
                        end_line: None,
                        end_column: None,
                        severity: ui_problems::DiagnosticSeverity::Hint,
                        message: hint.message.clone(),
                        source: hint.source.clone(),
                        hints: Vec::new(),
                        subitems: Vec::new(),
                        loading_actions: false,
                    });
                }
            }
            
            // Clone what we need for the async task
            let rust_analyzer = app.state.rust_analyzer.clone();
            let problems_drawer = app.state.problems_drawer.clone();
            
            // Store diagnostic info for code action requests
            let diagnostic_infos: Vec<_> = problems_diagnostics.iter().enumerate().map(|(idx, d)| {
                (
                    idx,
                    d.file_path.clone(),
                    d.line,
                    d.column,
                    d.end_line.unwrap_or(d.line),
                    d.end_column.unwrap_or(d.column + 1),
                )
            }).collect();
            
            // Set the diagnostics (with loading state)
            app.state.problems_drawer.update(cx, |drawer, cx| {
                drawer.set_diagnostics(problems_diagnostics, cx);
            });
            cx.notify();
            
            // Spawn async task to fetch code actions for each diagnostic
            cx.spawn(async move |_this, cx| {
                for (idx, file_path, line, column, end_line, end_column) in diagnostic_infos {
                    tracing::info!("🔍 Requesting code actions for {}:{}:{}", file_path, line, column);
                    
                    // Try to request code actions
                    let rx_result = cx.update(|cx| {
                        rust_analyzer.read(cx).request_code_actions_async(
                            &PathBuf::from(&file_path),
                            line,
                            column,
                            end_line,
                            end_column,
                        )
                    });
                    
                    let hints = match rx_result {
                        Ok(Ok(rx)) => {
                            // Wait for response with timeout
                            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                                Ok(response) => {
                                    tracing::info!("📦 Got code action response: {} items in array", 
                                        response.as_array().map(|a| a.len()).unwrap_or(0));
                                    
                                    // First, get already resolved actions
                                    let mut all_actions = RustAnalyzerManager::parse_code_actions(&response);
                                    tracing::info!("✓ Parsed {} already-resolved actions", all_actions.len());
                                    
                                    // Then resolve any unresolved actions
                                    let unresolved = RustAnalyzerManager::get_unresolved_actions(&response);
                                    tracing::info!("🔄 Found {} unresolved actions to resolve", unresolved.len());
                                    
                                    for unresolved_action in unresolved {
                                        let title = unresolved_action.get("title")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("unknown");
                                        tracing::info!("🔄 Resolving action: {}", title);
                                        
                                        // Try to resolve
                                        let resolve_rx = cx.update(|cx| {
                                            rust_analyzer.read(cx).resolve_code_action_async(&unresolved_action)
                                        });
                                        
                                        if let Ok(Ok(resolve_rx)) = resolve_rx {
                                            if let Ok(resolved) = resolve_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                                                // Parse the resolved action
                                                if let Some(action) = RustAnalyzerManager::parse_single_code_action(&resolved) {
                                                    tracing::info!("✓ Resolved action '{}' with {} edits", action.title, action.edits.len());
                                                    all_actions.push(action);
                                                } else {
                                                    tracing::warn!("⚠️ Failed to parse resolved action: {:?}", resolved);
                                                }
                                            } else {
                                                tracing::warn!("⚠️ Resolve timeout for action: {}", title);
                                            }
                                        } else {
                                            tracing::warn!("⚠️ Failed to send resolve request for: {}", title);
                                        }
                                    }
                                    
                                    tracing::info!("📋 Total actions after resolving: {}", all_actions.len());
                                    
                                    // Convert to hints with before/after content
                                    all_actions.into_iter().filter_map(|action| {
                                        if action.edits.is_empty() {
                                            tracing::info!("⚠️ Skipping action '{}' with no edits", action.title);
                                            return None;
                                        }
                                        
                                        let first_edit = &action.edits[0];
                                        let edit_file = &first_edit.file_path;
                                        
                                        tracing::info!("🔧 Computing diff for '{}' on {}", action.title, edit_file);
                                        let (before_content, after_content) = compute_before_after(edit_file, &action.edits);
                                        
                                        tracing::info!("✓ Diff computed - before: {} chars, after: {} chars",
                                            before_content.as_ref().map(|s| s.len()).unwrap_or(0),
                                            after_content.as_ref().map(|s| s.len()).unwrap_or(0));
                                        
                                        Some(ui_problems::Hint {
                                            message: action.title.clone(),
                                            before_content,
                                            after_content,
                                            file_path: Some(edit_file.clone()),
                                            line: Some(first_edit.start_line),
                                            loading: false,
                                        })
                                    }).collect::<Vec<_>>()
                                }
                                Err(e) => {
                                    tracing::warn!("⚠️ Code actions request timed out: {:?}", e);
                                    Vec::new()
                                },
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("⚠️ Failed to request code actions: {:?}", e);
                            Vec::new()
                        }
                        Err(e) => {
                            tracing::warn!("⚠️ Context update failed: {:?}", e);
                            Vec::new()
                        }
                    };
                    
                    tracing::info!("📋 Updating diagnostic {} with {} hints", idx, hints.len());
                    
                    // Update the drawer with the loaded hints
                    let _ = cx.update(|cx| {
                        problems_drawer.update(cx, |drawer, cx| {
                            drawer.update_diagnostic_hints(idx, hints, cx);
                        });
                    });
                }
            }).detach();
        }
    }
}

/// Helper function to compute before/after content from text edits
fn compute_before_after(file_path: &str, edits: &[ui_common::TextEdit]) -> (Option<String>, Option<String>) {
    if edits.is_empty() {
        return (None, None);
    }
    
    let first_edit = &edits[0];
    
    let before_content = if let Ok(content) = std::fs::read_to_string(file_path) {
        let lines: Vec<&str> = content.lines().collect();
        
        // Find the range that covers all edits
        let mut min_line = first_edit.start_line.saturating_sub(1);
        let mut max_line = first_edit.end_line.saturating_sub(1);
        
        for edit in edits {
            min_line = min_line.min(edit.start_line.saturating_sub(1));
            max_line = max_line.max(edit.end_line.saturating_sub(1));
        }
        
        // Add context lines
        let context = 2;
        let start = min_line.saturating_sub(context);
        let end = (max_line + context + 1).min(lines.len());
        
        if start < lines.len() {
            Some(lines[start..end].join("\n"))
        } else {
            None
        }
    } else {
        None
    };
    
    let after_content = if let Ok(content) = std::fs::read_to_string(file_path) {
        let lines: Vec<&str> = content.lines().collect();
        
        // Find the range that covers all edits
        let mut min_line = first_edit.start_line.saturating_sub(1);
        let mut max_line = first_edit.end_line.saturating_sub(1);
        
        for edit in edits {
            min_line = min_line.min(edit.start_line.saturating_sub(1));
            max_line = max_line.max(edit.end_line.saturating_sub(1));
        }
        
        // Add context lines
        let context = 2;
        let start = min_line.saturating_sub(context);
        let end = (max_line + context + 1).min(lines.len());
        
        if start < lines.len() {
            // Apply edits to compute after content
            let mut modified_content = content.clone();
            
            // Sort edits by position (reverse order so we can apply from end to start)
            let mut sorted_edits = edits.to_vec();
            sorted_edits.sort_by(|a, b| {
                let a_pos = (a.start_line, a.start_column);
                let b_pos = (b.start_line, b.start_column);
                b_pos.cmp(&a_pos) // Reverse order
            });
            
            for edit in &sorted_edits {
                // Convert line/column to byte offset
                let mut offset_start = 0;
                let mut offset_end = 0;
                let mut current_line = 1;
                let mut current_col = 1;
                
                for (i, ch) in modified_content.char_indices() {
                    if current_line == edit.start_line && current_col == edit.start_column {
                        offset_start = i;
                    }
                    if current_line == edit.end_line && current_col == edit.end_column {
                        offset_end = i;
                    }
                    
                    if ch == '\n' {
                        current_line += 1;
                        current_col = 1;
                    } else {
                        current_col += 1;
                    }
                }
                
                // Handle end of file
                if edit.end_line > current_line || 
                   (edit.end_line == current_line && edit.end_column > current_col) {
                    offset_end = modified_content.len();
                }
                
                // Apply the edit
                if offset_start <= offset_end && offset_end <= modified_content.len() {
                    modified_content = format!(
                        "{}{}{}",
                        &modified_content[..offset_start],
                        &edit.new_text,
                        &modified_content[offset_end..]
                    );
                }
            }
            
            // Extract the same range from modified content
            let modified_lines: Vec<&str> = modified_content.lines().collect();
            let mod_end = end.min(modified_lines.len());
            if start < modified_lines.len() {
                Some(modified_lines[start..mod_end].join("\n"))
            } else {
                Some(first_edit.new_text.clone())
            }
        } else {
            Some(first_edit.new_text.clone())
        }
    } else {
        Some(first_edit.new_text.clone())
    };
    
    (before_content, after_content)
}

pub fn on_project_selected(
    app: &mut PulsarApp,
    _selector: &Entity<EntryScreen>,
    event: &ProjectSelected,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::info!("[PROJECT_SELECTED] Received path: {:?}", event.path);
    
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

    tracing::info!("Project selected: {:?}", event.path);
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
    tracing::info!(
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
