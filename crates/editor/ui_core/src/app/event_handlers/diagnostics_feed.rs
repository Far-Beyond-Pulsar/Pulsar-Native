//! The diagnostics store in the Problems drawer: each source's complete set
//! goes to the drawer under that source's name, and quick-fix hints are then
//! fetched, from the server that published each diagnostic, in the background.
//! Entry point: [`refresh_source`].
//!
//! The pure steps ([`split_hints`], [`to_problems`], [`attach_hints`],
//! [`compute_before_after`], [`code_action_requests`]) take plain data so they
//! can be unit tested without GPUI.

use futures::FutureExt;
use gpui::{AsyncApp, Context, Entity};
use pulsar_lsp::{DiagnosticSource, DiagnosticsStore, LanguageServer, ServerId};
use smol::Timer;
use std::path::PathBuf;
use std::time::Duration;

use crate::app::PulsarApp;

/// What one diagnostic needs to ask its server for quick fixes.
#[derive(Clone)]
struct CodeActionRequest {
    /// Index of the diagnostic in its source's drawer slot.
    index: usize,
    file_path: String,
    /// The server that published it: the only one that can fix it.
    server: ServerId,
    /// The server's own diagnostic, which matches fixes better than a
    /// reconstructed range.
    raw_diagnostic: serde_json::Value,
    /// Fixes already carried in the diagnostic; nothing to fetch then.
    has_embedded_actions: bool,
}

/// Show everything `label` (a source's name) has in `store` in the drawer,
/// then request quick fixes for the diagnostics without embedded ones.
pub(super) fn refresh_source(
    app: &mut PulsarApp,
    store: &Entity<DiagnosticsStore>,
    label: &str,
    cx: &mut Context<PulsarApp>,
) {
    let entries: Vec<(DiagnosticSource, ui_common::Diagnostic)> = store
        .read(cx)
        .all()
        .into_iter()
        .filter(|(_, source, _)| source.label() == label)
        .map(|(_, source, diagnostic)| (source, diagnostic))
        .collect();
    let diagnostics: Vec<ui_common::Diagnostic> =
        entries.iter().map(|(_, d)| d.clone()).collect();

    let (errors_warnings, hints) = split_hints(&diagnostics);
    // The server of each error or warning, in the order of `errors_warnings`.
    let owners: Vec<Option<ServerId>> = entries
        .iter()
        .filter(|(_, d)| d.severity != ui_common::DiagnosticSeverity::Hint)
        .map(|(source, _)| match source {
            DiagnosticSource::Server(id) => Some(id.clone()),
            DiagnosticSource::Compiler(_) => None,
        })
        .collect();
    let fetchable = owners.iter().any(Option::is_some);
    let mut problems = to_problems(&errors_warnings, fetchable);
    attach_hints(&mut problems, hints);
    let requests = code_action_requests(&errors_warnings, &owners);

    let problems_drawer = app.state.problems_drawer.clone();
    let language_servers = app.state.language_servers.clone();
    let source = label.to_string();
    // The app root only needs to redraw if the drawer's contents changed;
    // notifying it on every republish rebuilt the whole editor shell.
    let changed = problems_drawer.update(cx, |drawer, cx| {
        drawer.set_source_diagnostics(&source, problems, cx)
    });
    if changed {
        cx.notify();
    }

    // An unchanged republish keeps the hints already fetched for it, so there
    // is nothing to request (one LSP round trip per diagnostic).
    let needing_fetch: Vec<CodeActionRequest> = if changed {
        requests
            .into_iter()
            .filter(|request| !request.has_embedded_actions)
            .collect()
    } else {
        Vec::new()
    };
    if needing_fetch.is_empty() {
        return;
    }
    tracing::debug!(
        "fetching quick fixes for {} diagnostics of {source}",
        needing_fetch.len()
    );
    cx.spawn(async move |_this, cx| {
        for request in needing_fetch {
            let server = cx.update(|cx| language_servers.read(cx).server(&request.server, cx));
            let Some(server) = server else {
                continue;
            };
            let hints = fetch_code_action_hints(&server, &request, cx).await;
            let _ = cx.update(|cx| {
                problems_drawer.update(cx, |drawer, cx| {
                    drawer.update_source_hints(&source, request.index, hints, cx);
                });
            });
        }
    })
    .detach();
}

/// Separate hints from errors/warnings: hints attach to a parent later.
fn split_hints(
    diagnostics: &[ui_common::Diagnostic],
) -> (Vec<&ui_common::Diagnostic>, Vec<&ui_common::Diagnostic>) {
    let mut errors_warnings = Vec::new();
    let mut hints = Vec::new();
    for d in diagnostics {
        match d.severity {
            ui_common::DiagnosticSeverity::Hint => hints.push(d),
            _ => errors_warnings.push(d),
        }
    }
    (errors_warnings, hints)
}

fn problem_severity(severity: ui_common::DiagnosticSeverity) -> ui_problems::DiagnosticSeverity {
    match severity {
        ui_common::DiagnosticSeverity::Error => ui_problems::DiagnosticSeverity::Error,
        ui_common::DiagnosticSeverity::Warning => ui_problems::DiagnosticSeverity::Warning,
        ui_common::DiagnosticSeverity::Information => ui_problems::DiagnosticSeverity::Information,
        ui_common::DiagnosticSeverity::Hint => ui_problems::DiagnosticSeverity::Hint,
    }
}

/// One drawer hint from a code action, with the before/after file content
/// the diff view shows. `None` for an action without edits.
fn hint_from_action(action: &ui_common::CodeAction) -> Option<ui_problems::Hint> {
    let first_edit = action.edits.first()?;
    let edit_file = &first_edit.file_path;

    tracing::debug!("🔧 Computing diff for '{}' on {}", action.title, edit_file);
    let (before_content, after_content) = compute_before_after(edit_file, &action.edits);

    tracing::debug!(
        "✓ Diff computed - before: {} chars, after: {} chars",
        before_content.as_ref().map(|s| s.len()).unwrap_or(0),
        after_content.as_ref().map(|s| s.len()).unwrap_or(0)
    );

    Some(ui_problems::Hint {
        message: action.title.clone(),
        before_content,
        after_content,
        file_path: Some(edit_file.clone()),
        line: Some(first_edit.start_line),
        loading: false,
    })
}

/// Convert errors/warnings to drawer diagnostics, turning any code actions
/// embedded in `relatedInformation` into hints with before/after content.
fn to_problems(
    errors_warnings: &[&ui_common::Diagnostic],
    fetchable: bool,
) -> Vec<ui_problems::Diagnostic> {
    errors_warnings
        .iter()
        .map(|d| {
            let embedded_hints: Vec<_> =
                d.code_actions.iter().filter_map(hint_from_action).collect();

            // If we have embedded hints, we don't need to load more
            let loading_actions =
                fetchable && embedded_hints.is_empty() && d.raw_lsp_diagnostic.is_some();

            if !embedded_hints.is_empty() {
                tracing::debug!(
                    "📋 Diagnostic has {} embedded code action hints",
                    embedded_hints.len()
                );
            }

            ui_problems::Diagnostic {
                file_path: d.file_path.clone(),
                line: d.line,
                column: d.column,
                end_line: d.end_line,
                end_column: d.end_column,
                severity: problem_severity(d.severity),
                message: d.message.clone(),
                source: d.source.clone(),
                hints: embedded_hints,
                subitems: Vec::new(),
                loading_actions,
            }
        })
        .collect()
}

/// Attach each hint to the closest parent error/warning in the same file, or
/// add it as a standalone diagnostic when the file has none.
fn attach_hints(problems: &mut Vec<ui_problems::Diagnostic>, hints: Vec<&ui_common::Diagnostic>) {
    for hint in hints {
        let mut best_match: Option<usize> = None;
        let mut best_distance: usize = usize::MAX;

        for (i, parent) in problems.iter().enumerate() {
            if parent.file_path == hint.file_path {
                let distance = hint.line.abs_diff(parent.line);

                if distance < best_distance {
                    best_distance = distance;
                    best_match = Some(i);
                }
            }
        }

        if let Some(parent_idx) = best_match {
            // The actual code actions are fetched asynchronously.
            problems[parent_idx].hints.push(ui_problems::Hint {
                message: hint.message.clone(),
                before_content: None,
                after_content: None,
                file_path: Some(hint.file_path.clone()),
                line: Some(hint.line),
                loading: false,
            });
        } else {
            problems.push(ui_problems::Diagnostic {
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
}
/// The quick-fix request for each error/warning, indexed like the drawer's
/// slot for the source; a diagnostic that no server published has none.
fn code_action_requests(
    errors_warnings: &[&ui_common::Diagnostic],
    owners: &[Option<ServerId>],
) -> Vec<CodeActionRequest> {
    errors_warnings
        .iter()
        .zip(owners)
        .enumerate()
        .filter_map(|(index, (d, owner))| {
            Some(CodeActionRequest {
                index,
                file_path: d.file_path.clone(),
                server: owner.clone()?,
                raw_diagnostic: d.raw_lsp_diagnostic.clone()?,
                has_embedded_actions: !d.code_actions.is_empty(),
            })
        })
        .collect()
}

/// Ask the server for the fixes of one diagnostic (5 s timeout) and turn them
/// into hints. Failures and timeouts yield no hints.
async fn fetch_code_action_hints(
    server: &Entity<LanguageServer>,
    request: &CodeActionRequest,
    cx: &mut AsyncApp,
) -> Vec<ui_problems::Hint> {
    let rx = cx.update(|cx| {
        server.read(cx).request_code_actions_with_diagnostic(
            &PathBuf::from(&request.file_path),
            &request.raw_diagnostic,
        )
    });
    let rx = match rx {
        Ok(rx) => rx,
        Err(error) => {
            tracing::warn!("could not request code actions: {error:#}");
            return Vec::new();
        }
    };
    let response = futures::select! {
        response = rx.recv_async().fuse() => response.ok(),
        _ = Timer::after(Duration::from_secs(5)).fuse() => None,
    };
    let Some(response) = response else {
        tracing::warn!("code actions request timed out or failed");
        return Vec::new();
    };
    let all_actions = resolve_actions(&response, server, cx).await;
    all_actions.iter().filter_map(hint_from_action).collect()
}

/// The response's already-resolved actions plus each unresolved one resolved
/// through `codeAction/resolve` (2 s timeout each; failures are skipped).
async fn resolve_actions(
    response: &serde_json::Value,
    server: &Entity<LanguageServer>,
    cx: &mut AsyncApp,
) -> Vec<ui_common::CodeAction> {
    let (mut all_actions, unresolved) = LanguageServer::split_code_actions(response);
    for action in unresolved {
        let Ok(rx) = cx.update(|cx| server.read(cx).resolve_code_action_async(&action)) else {
            continue;
        };
        let resolved = futures::select! {
            resolved = rx.recv_async().fuse() => resolved.ok(),
            _ = Timer::after(Duration::from_secs(2)).fuse() => None,
        };
        if let Some(resolved) = resolved {
            let result = resolved.get("result").unwrap_or(&serde_json::Value::Null);
            if let Some(action) = pulsar_lsp::code_actions::parse_single_code_action(result) {
                all_actions.push(action);
            }
        }
    }
    all_actions
}

/// Helper function to compute before/after content from text edits
/// Returns the FULL modified file content for both before and after
/// The diff algorithm will handle alignment with spacers
pub(super) fn compute_before_after(
    file_path: &str,
    edits: &[ui_common::TextEdit],
) -> (Option<String>, Option<String>) {
    if edits.is_empty() {
        return (None, None);
    }

    let first_edit = &edits[0];

    // Read the original file content
    let content = match engine_fs::virtual_fs::read_to_string(std::path::Path::new(&file_path)) {
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
        if !found_end
            && (edit.end_line > current_line
                || (edit.end_line == current_line && edit.end_column > current_col))
        {
            offset_end = modified_content.len();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(
        file: &str,
        line: usize,
        severity: ui_common::DiagnosticSeverity,
    ) -> ui_common::Diagnostic {
        ui_common::Diagnostic {
            file_path: file.to_string(),
            line,
            column: 1,
            end_line: None,
            end_column: None,
            message: format!("{file}:{line}"),
            severity,
            code: None,
            source: None,
            code_actions: Vec::new(),
            raw_lsp_diagnostic: None,
        }
    }

    #[test]
    fn hints_are_split_from_errors_and_warnings() {
        use ui_common::DiagnosticSeverity::*;
        let all = vec![
            diagnostic("a.rs", 1, Error),
            diagnostic("a.rs", 2, Hint),
            diagnostic("a.rs", 3, Warning),
        ];
        let (errors_warnings, hints) = split_hints(&all);
        assert_eq!(errors_warnings.len(), 2);
        assert_eq!(hints.len(), 1);
    }

    #[test]
    fn a_hint_attaches_to_the_closest_parent_in_the_same_file() {
        use ui_common::DiagnosticSeverity::*;
        let all = vec![
            diagnostic("a.rs", 10, Error),
            diagnostic("a.rs", 40, Warning),
            diagnostic("b.rs", 41, Error),
            diagnostic("a.rs", 38, Hint),
        ];
        let (errors_warnings, hints) = split_hints(&all);
        let mut problems = to_problems(&errors_warnings, true);
        attach_hints(&mut problems, hints);
        assert_eq!(problems.len(), 3);
        assert_eq!(problems[1].hints.len(), 1);
        assert!(problems[0].hints.is_empty());
        assert!(problems[2].hints.is_empty());
    }

    #[test]
    fn a_hint_without_a_parent_becomes_a_standalone_diagnostic() {
        use ui_common::DiagnosticSeverity::*;
        let all = vec![diagnostic("a.rs", 1, Error), diagnostic("c.rs", 5, Hint)];
        let (errors_warnings, hints) = split_hints(&all);
        let mut problems = to_problems(&errors_warnings, true);
        attach_hints(&mut problems, hints);
        assert_eq!(problems.len(), 2);
        assert_eq!(problems[1].file_path, "c.rs");
        assert!(!problems[1].loading_actions);
    }

    #[test]
    fn fix_requests_are_indexed_and_only_for_diagnostics_a_server_published() {
        use ui_common::DiagnosticSeverity::*;
        let mut served = diagnostic("a.rs", 7, Error);
        served.raw_lsp_diagnostic = Some(serde_json::json!({ "message": "x" }));
        let compiler = diagnostic("a.ts", 3, Error);
        let all = vec![served, compiler];
        let (errors_warnings, _) = split_hints(&all);
        let server = ServerId { server: "rust-analyzer", root: PathBuf::from("/p") };
        let requests = code_action_requests(&errors_warnings, &[Some(server.clone()), None]);
        assert_eq!(requests.len(), 1, "a compiler diagnostic has no server to ask");
        assert_eq!(requests[0].index, 0);
        assert_eq!(requests[0].server, server);
        assert!(!requests[0].has_embedded_actions);
    }

    #[test]
    fn only_server_diagnostics_show_a_loading_indicator() {
        use ui_common::DiagnosticSeverity::*;
        let mut served = diagnostic("a.rs", 7, Error);
        served.raw_lsp_diagnostic = Some(serde_json::json!({}));
        let plain = diagnostic("a.ts", 3, Error);
        let all = vec![served, plain];
        let (errors_warnings, _) = split_hints(&all);
        let problems = to_problems(&errors_warnings, true);
        assert!(problems[0].loading_actions);
        assert!(!problems[1].loading_actions);
        assert!(!to_problems(&errors_warnings, false)[0].loading_actions);
    }

    #[test]
    fn edits_with_no_file_still_yield_the_replacement_text() {
        let edit = ui_common::TextEdit {
            file_path: "definitely/not/a/file.rs".into(),
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 2,
            new_text: "fixed".into(),
        };
        assert_eq!(
            compute_before_after(&edit.file_path, &[edit.clone()]),
            (None, Some("fixed".to_string()))
        );
        assert_eq!(compute_before_after("x", &[]), (None, None));
    }
}
