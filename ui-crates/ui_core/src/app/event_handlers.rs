//! Event handler implementations

use gpui::{AppContext, Context, DismissEvent, Entity, Focusable, Window};
use ui::{ContextModal, dock::PanelEvent};
use ui_file_manager::{FileManagerDrawer, FileSelected, PopoutFileManagerEvent};
use ui_problems::ProblemsDrawer;
use ui_entry::{EntryScreen, ProjectSelected};

// TODO:!!! We must find a way to migrate these to their plugins and decouple from core UI!

// use ui_script_editor::{ScriptEditorPanel, TextEditorEvent};
// use ui_alias_editor::ShowTypePickerRequest;
use engine_backend::services::rust_analyzer_manager::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
use std::path::PathBuf;
use futures::FutureExt;
use smol::Timer;
use std::time::Duration;

use super::PulsarApp;

pub fn on_analyzer_event(
    app: &mut PulsarApp,
    _manager: &Entity<RustAnalyzerManager>,
    event: &AnalyzerEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!("🎯 on_analyzer_event CALLED with event type: {}", match event {
        AnalyzerEvent::StatusChanged(_) => "StatusChanged",
        AnalyzerEvent::Ready => "Ready",
        AnalyzerEvent::Error(_) => "Error",
        AnalyzerEvent::Diagnostics(_) => "Diagnostics",
        AnalyzerEvent::IndexingProgress { .. } => "IndexingProgress",
    });
    
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
            
            tracing::debug!("🔔 DIAGNOSTICS EVENT RECEIVED: {} diagnostics", diagnostics.len());
            
            // First, separate hints from errors/warnings
            let mut errors_warnings: Vec<_> = Vec::new();
            let mut hints: Vec<_> = Vec::new();
            
            for d in diagnostics.iter() {
                match d.severity {
                    ui_common::DiagnosticSeverity::Hint => hints.push(d),
                    _ => errors_warnings.push(d),
                }
            }
            
            // Convert errors/warnings to problems diagnostics
            // Also convert any embedded code_actions from relatedInformation into hints
            let mut problems_diagnostics: Vec<ui_problems::Diagnostic> = errors_warnings.iter().map(|d| {
                // Convert embedded code_actions to hints with before/after content
                let embedded_hints: Vec<_> = d.code_actions.iter().filter_map(|action| {
                    if action.edits.is_empty() {
                        return None;
                    }
                    
                    let first_edit = &action.edits[0];
                    let edit_file = &first_edit.file_path;
                    
                    tracing::debug!("🔧 Converting embedded code action '{}' to hint for {}", action.title, edit_file);
                    let (before_content, after_content) = compute_before_after(edit_file, &action.edits);
                    
                    tracing::debug!("✓ Embedded hint - before: {} chars, after: {} chars",
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
                }).collect();
                
                // If we have embedded hints, we don't need to load more
                let loading_actions = embedded_hints.is_empty();
                
                if !embedded_hints.is_empty() {
                    tracing::debug!("📋 Diagnostic has {} embedded code action hints", embedded_hints.len());
                }
                
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
                    hints: embedded_hints,
                    subitems: Vec::new(),
                    loading_actions,
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
            
            // Store diagnostic info for code action requests (including raw LSP diagnostic)
            // Also track if the diagnostic already has embedded code actions (from relatedInformation)
            let diagnostic_infos: Vec<_> = errors_warnings.iter().enumerate().map(|(idx, d)| {
                (
                    idx,
                    d.file_path.clone(),
                    d.line,
                    d.column,
                    d.end_line.unwrap_or(d.line),
                    d.end_column.unwrap_or(d.column + 1),
                    d.message.clone(),
                    d.raw_lsp_diagnostic.clone(),
                    !d.code_actions.is_empty(), // has_embedded_actions
                )
            }).collect();
            
            // Set the diagnostics (with embedded hints already populated)
            app.state.problems_drawer.update(cx, |drawer, cx| {
                drawer.set_diagnostics(problems_diagnostics, cx);
            });
            cx.notify();
            
            // Spawn async task to fetch code actions for diagnostics that don't have embedded actions
            let diagnostics_needing_fetch: Vec<_> = diagnostic_infos.iter()
                .filter(|(_, _, _, _, _, _, _, _, has_embedded)| !has_embedded)
                .cloned()
                .collect();
            
            tracing::debug!("🚀 Spawning code action fetch task for {} diagnostics (skipping {} with embedded actions)", 
                diagnostics_needing_fetch.len(),
                diagnostic_infos.len() - diagnostics_needing_fetch.len());
            
            cx.spawn(async move |_this, cx| {
                tracing::debug!("🎯 Code action task started, processing {} diagnostics", diagnostics_needing_fetch.len());
                for (idx, file_path, line, column, end_line, end_column, message, raw_diagnostic, _) in diagnostics_needing_fetch {
                    tracing::debug!("🔍 Requesting code actions for {}:{}:{} - {} (has raw: {})", 
                        file_path, line, column, message, raw_diagnostic.is_some());
                    
                    // Try to request code actions - prefer using raw diagnostic if available
                    let rx_result = cx.update(|cx| {
                        let analyzer = rust_analyzer.read(cx);
                        if let Some(ref raw_diag) = raw_diagnostic {
                            // Use the raw diagnostic for better code action matching
                            analyzer.request_code_actions_with_diagnostic(
                                &PathBuf::from(&file_path),
                                raw_diag,
                            )
                        } else {
                            // Fall back to reconstructed diagnostic
                            analyzer.request_code_actions_async(
                                &PathBuf::from(&file_path),
                                line,
                                column,
                                end_line,
                                end_column,
                                Some(&message),
                            )
                        }
                    });
                    
                    let hints = match rx_result {
                        Ok(Ok(rx)) => {
                            // Wait for response with async timeout
                            let recv_future = rx.recv_async();
                            let timeout_future = Timer::after(Duration::from_secs(5));
                            
                            let response_result = futures::select! {
                                response = recv_future.fuse() => Some(response),
                                _ = timeout_future.fuse() => None,
                            };
                            
                            match response_result {
                                Some(Ok(response)) => {
                                    tracing::debug!("📦 Got code action response: {} items in array", 
                                        response.as_array().map(|a| a.len()).unwrap_or(0));
                                    
                                    // First, get already resolved actions
                                    let mut all_actions = RustAnalyzerManager::parse_code_actions(&response);
                                    tracing::debug!("✓ Parsed {} already-resolved actions", all_actions.len());
                                    
                                    // Then resolve any unresolved actions
                                    let unresolved = RustAnalyzerManager::get_unresolved_actions(&response);
                                    tracing::debug!("🔄 Found {} unresolved actions to resolve", unresolved.len());
                                    
                                    for unresolved_action in unresolved {
                                        let title = unresolved_action.get("title")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("unknown");
                                        tracing::debug!("🔄 Resolving action: {}", title);
                                        
                                        // Try to resolve
                                        let resolve_rx = cx.update(|cx| {
                                            rust_analyzer.read(cx).resolve_code_action_async(&unresolved_action)
                                        });
                                        
                                        if let Ok(Ok(resolve_rx)) = resolve_rx {
                                            // Async timeout for resolve
                                            let resolve_future = resolve_rx.recv_async();
                                            let resolve_timeout = Timer::after(Duration::from_secs(2));
                                            
                                            let resolve_result = futures::select! {
                                                resolved = resolve_future.fuse() => Some(resolved),
                                                _ = resolve_timeout.fuse() => None,
                                            };
                                            
                                            if let Some(Ok(resolved)) = resolve_result {
                                                // Parse the resolved action
                                                if let Some(action) = RustAnalyzerManager::parse_single_code_action(&resolved) {
                                                    tracing::debug!("✓ Resolved action '{}' with {} edits", action.title, action.edits.len());
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
                                    
                                    tracing::debug!("📋 Total actions after resolving: {}", all_actions.len());
                                    
                                    // Convert to hints with before/after content
                                    all_actions.into_iter().filter_map(|action| {
                                        if action.edits.is_empty() {
                                            tracing::debug!("⚠️ Skipping action '{}' with no edits", action.title);
                                            return None;
                                        }
                                        
                                        let first_edit = &action.edits[0];
                                        let edit_file = &first_edit.file_path;
                                        
                                        tracing::debug!("🔧 Computing diff for '{}' on {}", action.title, edit_file);
                                        let (before_content, after_content) = compute_before_after(edit_file, &action.edits);
                                        
                                        tracing::debug!("✓ Diff computed - before: {} chars, after: {} chars",
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
                                Some(Err(e)) => {
                                    tracing::warn!("⚠️ Code actions request error: {:?}", e);
                                    Vec::new()
                                }
                                None => {
                                    tracing::warn!("⚠️ Code actions request timed out");
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
                    
                    tracing::debug!("📋 Updating diagnostic {} with {} hints", idx, hints.len());
                    
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
/// Returns the FULL modified file content for both before and after
/// The diff algorithm will handle alignment with spacers
fn compute_before_after(file_path: &str, edits: &[ui_common::TextEdit]) -> (Option<String>, Option<String>) {
    if edits.is_empty() {
        return (None, None);
    }
    
    let first_edit = &edits[0];
    
    // Read the original file content
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return (None, Some(first_edit.new_text.clone())),
    };
    
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
    
    // Extract before content from original file
    let before_content = if start < lines.len() {
        Some(lines[start..end].join("\n"))
    } else {
        None
    };
    
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
        let mut found_start = false;
        let mut found_end = false;
        
        for (i, ch) in modified_content.char_indices() {
            if current_line == edit.start_line && current_col == edit.start_column {
                offset_start = i;
                found_start = true;
            }
            if current_line == edit.end_line && current_col == edit.end_column {
                offset_end = i;
                found_end = true;
            }
            
            if ch == '\n' {
                current_line += 1;
                current_col = 1;
            } else {
                current_col += 1;
            }
        }
        
        // Handle end of file or end of line
        if !found_end {
            if edit.end_line > current_line || 
               (edit.end_line == current_line && edit.end_column > current_col) {
                offset_end = modified_content.len();
            }
        }
        
        // Apply the edit
        if found_start && offset_start <= offset_end && offset_end <= modified_content.len() {
            modified_content = format!(
                "{}{}{}",
                &modified_content[..offset_start],
                &edit.new_text,
                &modified_content[offset_end..]
            );
        }
    }
    
    // Extract the SAME context range from modified content
    // But use line counting to get equivalent context
    let modified_lines: Vec<&str> = modified_content.lines().collect();
    
    // Calculate how many lines were added/removed by the edits
    let mut lines_delta: i32 = 0;
    for edit in edits {
        let deleted_lines = (edit.end_line as i32) - (edit.start_line as i32);
        let added_lines = edit.new_text.matches('\n').count() as i32;
        // If edit.end_line == edit.start_line but there's content, it's a single line edit
        // If new_text is empty but range spans lines, those lines are deleted
        if edit.end_line > edit.start_line {
            lines_delta += added_lines - deleted_lines;
        } else {
            lines_delta += added_lines;
        }
    }
    
    // For the "after" content, we want to show the equivalent region
    // The start stays the same (context before the edit)
    // The end adjusts based on how many lines were added/removed
    let after_end = ((end as i32) + lines_delta).max(0) as usize;
    let after_end = after_end.min(modified_lines.len());
    
    let after_content = if start < modified_lines.len() {
        Some(modified_lines[start..after_end].join("\n"))
    } else if !first_edit.new_text.is_empty() {
        Some(first_edit.new_text.clone())
    } else {
        Some(String::new())
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
    tracing::debug!("[PROJECT_SELECTED] Received path: {:?}", event.path);
    
    app.state.project_path = Some(event.path.clone());
    app.state.entry_screen = None;

    // Update file manager with project path
    app.state.file_manager_drawer.update(cx, |drawer, cx| {
        drawer.set_project_path(event.path.clone(), cx);
    });

    // Update problems drawer with project root for relative paths
    app.state.problems_drawer.update(cx, |drawer, cx| {
        drawer.set_project_root(Some(event.path.clone()), cx);
    });

    // Update type debugger drawer with project root for relative paths
    app.state.type_debugger_drawer.update(cx, |drawer, cx| {
        drawer.set_project_root(Some(event.path.clone()), cx);
    });

    // Start rust analyzer for the project
    app.state.rust_analyzer.update(cx, |analyzer, cx| {
        analyzer.start(event.path.clone(), window, cx);
    });

    // Update Discord presence with new project
    app.update_discord_presence(cx);

    tracing::debug!("Project selected: {:?}", event.path);
    cx.notify();
}

pub fn on_tab_panel_event(
    app: &mut PulsarApp,
    _tabs: &Entity<ui::dock::TabPanel>,
    event: &PanelEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    println!("[PANEL_EVENT] Received event: {:?}", match event {
        PanelEvent::MoveToNewWindow(_, pos) => format!("MoveToNewWindow at {:?}", pos),
        PanelEvent::TabClosed(id) => format!("TabClosed({:?})", id),
        PanelEvent::TabChanged { active_index } => format!("TabChanged({})", active_index),
        PanelEvent::ZoomIn => format!("ZoomIn"),
        PanelEvent::ZoomOut => format!("ZoomOut"),
        PanelEvent::LayoutChanged => format!("LayoutChanged"),
    });

    match event {
        PanelEvent::MoveToNewWindow(panel, position) => {
            println!("[PANEL_EVENT] Handling MoveToNewWindow event");
            app.create_detached_window(panel.clone(), *position, window, cx);
        }
        PanelEvent::TabClosed(entity_id) => {
            // Editor tracking has been migrated to plugins
            // app.state.daw_editors.retain(|e| e.entity_id() != *entity_id);
            // app.state.database_editors.retain(|e| e.entity_id() != *entity_id);
            // app.state.struct_editors.retain(|e| e.entity_id() != *entity_id);
            // app.state.enum_editors.retain(|e| e.entity_id() != *entity_id);
            // app.state.trait_editors.retain(|e| e.entity_id() != *entity_id);
            // app.state.alias_editors.retain(|e| e.entity_id() != *entity_id);

            // Update Discord presence when tab is closed
            app.update_discord_presence(cx);
        }
        PanelEvent::TabChanged { active_index: _ } => {
            // Update Discord presence when active tab changes
            app.update_discord_presence(cx);
        }
        PanelEvent::ZoomIn | PanelEvent::ZoomOut | PanelEvent::LayoutChanged => {
            // These events are handled by the dock system, do nothing here
        }
    }
}

pub fn on_file_selected(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    event: &FileSelected,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!("Opening file: {:?}", event.path);

    // Open via plugin system
    app.open_path(event.path.clone(), window, cx);
    
    app.state.drawer_open = false;
    app.update_discord_presence(cx);
    cx.notify();
}

pub fn on_popout_file_manager(
    app: &mut PulsarApp,
    drawer: &Entity<FileManagerDrawer>,
    event: &PopoutFileManagerEvent,
    _window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
    use ui::Root;

    // Get project path from the drawer
    let project_path = drawer.read(cx).project_path.clone();
    
    app.state.drawer_open = false;
    cx.notify();

    // Use mouse position from event for window placement
    let window_origin = Point {
        x: event.position.x - px(500.0), // Center window at mouse
        y: event.position.y - px(350.0),
    };

    // Open the file manager window
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: window_origin,
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
    tracing::debug!(
        "📂 Navigating to diagnostic: {:?} at line {}, column {}",
        event.file_path, event.line, event.column
    );

    app.open_path(event.file_path.clone(), window, cx);

    // Script editor has been migrated to plugins
    // if let Some(script_editor) = &app.state.script_editor {
    //     script_editor.update(cx, |editor, cx| {
    //         editor.go_to_line(event.line, event.column, window, cx);
    //     });
    // }
}

// Type picker functionality has been migrated to plugins
// pub fn on_show_type_picker_request(
//     app: &mut PulsarApp,
//     editor: &Entity<ui_alias_editor::AliasEditor>,
//     event: &ShowTypePickerRequest,
//     window: &mut Window,
//     cx: &mut Context<PulsarApp>,
// ) {
//     use crate::unified_palette::AnyPaletteDelegate;
//     use ui_common::command_palette::GenericPalette;
//
//     app.state.active_type_picker_editor = Some(editor.clone());
//
//     if let Some(palette) = &app.state.command_palette {
//         palette.update(cx, |palette, cx| {
//             let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
//             palette.swap_delegate(delegate, window, cx);
//         });
//
//         let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
//         input_handle.focus(window);
//     } else {
//         let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
//         let palette = cx.new(|cx| GenericPalette::new(delegate, window, cx));
// 
//         cx.subscribe_in(&palette, window, |app: &mut PulsarApp, palette, _event: &DismissEvent, window, cx| {
//             let selected_item = palette.update(cx, |palette, _cx| {
//                 palette.delegate_mut().take_selected_command()
//             });
// 
//             let selected_type = palette.update(cx, |palette, _cx| {
//                 palette.delegate_mut().take_selected_type()
//             });
// 
//             if let Some(item) = selected_item {
//                 app.handle_command_or_file_selected(item, window, cx);
//             }
// 
//             if let Some((type_item, target_slot)) = selected_type {
//                 if let Some(editor) = &app.state.active_type_picker_editor {
//                     editor.update(cx, |ed, cx| {
//                         ed.add_type_from_picker(&type_item, target_slot, cx);
//                     });
//                 }
//                 app.state.active_type_picker_editor = None;
//             }
// 
//             app.state.command_palette_open = false;
//             app.state.focus_handle.focus(window);
//             cx.notify();
//         }).detach();
// 
//         let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
//         input_handle.focus(window);
// 
//         app.state.command_palette = Some(palette);
//     }
// 
//     app.state.command_palette_open = true;
//     cx.notify();
// }
