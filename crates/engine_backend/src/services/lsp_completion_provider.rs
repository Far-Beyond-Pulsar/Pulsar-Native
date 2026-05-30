/// LSP Completion Provider that connects to the global rust-analyzer manager
/// This provides real-time code completions from rust-analyzer
use anyhow::Result;
use gpui::{App, Context, Task, Window};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc,
};
use ui::input::{CompletionProvider, DefinitionProvider, InputState, RopeExt};

use super::rust_analyzer_manager::RustAnalyzerManager;
use gpui::Entity;

/// Completion provider that uses the global rust-analyzer instance
pub struct GlobalRustAnalyzerCompletionProvider {
    /// Reference to the global rust-analyzer manager
    analyzer: Entity<RustAnalyzerManager>,
    /// Current file path
    file_path: PathBuf,
    /// Workspace root
    workspace_root: PathBuf,
    /// Tracks whether this document has been didOpen'd in the current analyzer session.
    did_open_sent: Arc<AtomicBool>,
    /// Monotonically increasing version for textDocument/didChange notifications.
    text_version: Arc<AtomicI32>,
}

impl GlobalRustAnalyzerCompletionProvider {
    pub fn new(
        analyzer: Entity<RustAnalyzerManager>,
        file_path: PathBuf,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            analyzer,
            file_path,
            workspace_root,
            did_open_sent: Arc::new(AtomicBool::new(false)),
            text_version: Arc::new(AtomicI32::new(1)),
        }
    }

    /// Convert file path to LSP URI
    fn path_to_uri(&self) -> String {
        super::path_utils::path_to_uri(&self.file_path)
    }

    fn ensure_document_open_with_ra(&self, text: &ropey::Rope, cx: &mut App) {
        let content = text.to_string();
        let path = self.file_path.clone();
        let sent = self.did_open_sent.clone();

        let result = self.analyzer.update(cx, move |analyzer, _| {
            match analyzer.did_open_file(&path, &content, "rust") {
                Ok(()) => {
                    tracing::debug!("[LSP SYNC] didOpen succeeded for {:?}", path.file_name());
                    // Only mark as sent if didOpen actually succeeded
                    sent.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::debug!(
                        "[LSP SYNC] didOpen failed for {:?}: {} (will retry on next hover)",
                        path.file_name(),
                        e
                    );
                    // DO NOT set sent flag - allow retry on next request
                }
            }
        });

        tracing::debug!("[LSP SYNC] analyzer.update returned: {:?}", result);
    }
}

impl CompletionProvider for GlobalRustAnalyzerCompletionProvider {
    fn completions(
        &self,
        text: &ropey::Rope,
        offset: usize,
        trigger: lsp_types::CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<lsp_types::CompletionResponse>> {
        // Check if analyzer is ready (fast check)
        let status = self.analyzer.read(cx).status().clone();
        let is_ready = self.analyzer.read(cx).is_running();
        tracing::debug!(
            "[LSP COMPLETION] file={:?} is_running={} status={:?} offset={}",
            self.file_path.file_name(),
            is_ready,
            status,
            offset
        );
        if !is_ready {
            tracing::debug!("[LSP COMPLETION] early-exit: analyzer not running");
            return Task::ready(Ok(lsp_types::CompletionResponse::Array(vec![])));
        }

        tracing::debug!("[LSP COMPLETION] BEFORE ensure_document_open_with_ra");
        self.ensure_document_open_with_ra(text, cx);
        tracing::debug!("[LSP COMPLETION] AFTER ensure_document_open_with_ra");

        // Send didChange to keep rust-analyzer in sync with current editor content.
        // This is essential: without it, rust-analyzer uses stale content from the original didOpen.
        if self.did_open_sent.load(Ordering::Relaxed) {
            let content = text.to_string();
            let path = self.file_path.clone();
            let version = self.text_version.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.analyzer.update(cx, move |analyzer, _| {
                if let Err(e) = analyzer.did_change_file(&path, &content, version) {
                    tracing::debug!("[LSP SYNC] didChange failed: {}", e);
                } else {
                    tracing::debug!(
                        "[LSP SYNC] didChange sent version={} for {:?}",
                        version,
                        path.file_name()
                    );
                }
            });
        }

        // Clone only what we need - DO NOT convert rope to string here (blocks UI!)
        let uri = self.path_to_uri();
        tracing::debug!(
            "[LSP COMPLETION] sending textDocument/completion for uri={}",
            uri
        );
        let _file_path = self.file_path.clone();
        let analyzer = self.analyzer.clone();
        let text_clone = text.clone(); // Rope clone is cheap (it's a rope, not a copy)

        let trigger_kind = match trigger.trigger_kind {
            lsp_types::CompletionTriggerKind::INVOKED => 1,
            lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER => 2,
            lsp_types::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS => 3,
            _ => 1,
        };

        let trigger_char = trigger.trigger_character.clone();

        // Spawn immediately - do ALL potentially slow work in the async block
        cx.spawn_in(window, async move |_, cx| {
            // Convert to position in background (can be slow for large files).
            // LSP positions are allowed at EOF, so keep offset == len untouched.
            // Only clamp truly out-of-bounds offsets.
            let safe_offset = if offset > text_clone.len() {
                text_clone.len()
            } else {
                offset
            };
            let position = text_clone.offset_to_position(safe_offset);

            // DON'T sync file content here - it should already be synced via the text editor's change handler!
            // Calling did_change_file here causes "unexpected DidChangeTextDocument" errors from rust-analyzer.
            // The text editor already calls did_change_file on every edit.

            // Send completion request immediately (async, non-blocking!)
            let response_rx = match analyzer
                .update(cx, |analyzer, _| {
                    let mut context = json!({
                        "triggerKind": trigger_kind
                    });

                    // Include trigger character if present
                    if let Some(ref ch) = trigger_char {
                        context["triggerCharacter"] = json!(ch);
                    }

                    let params = json!({
                        "textDocument": {
                            "uri": uri
                        },
                        "position": {
                            "line": position.line,
                            "character": position.character
                        },
                        "context": context
                    });

                    analyzer.send_request_async("textDocument/completion", params)
                })
                .ok()
            {
                Some(rx) => rx,
                None => {
                    tracing::error!("⚠️  Failed to send completion request");
                    return Ok(lsp_types::CompletionResponse::Array(vec![]));
                }
            };

            // Wait for response asynchronously (non-blocking!)
            let response = match response_rx.recv_async().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::error!("⚠️  Failed to receive completion response: {}", e);
                    return Ok(lsp_types::CompletionResponse::Array(vec![]));
                }
            };

            // Check for error in response
            if let Some(error) = response.get("error") {
                tracing::error!("❌ rust-analyzer completion error: {}", error);
                return Ok(lsp_types::CompletionResponse::Array(vec![]));
            }

            // Parse the response
            if let Some(result) = response.get("result") {
                // Check if result is null
                if result.is_null() {
                    tracing::debug!("📦 Received 0 completions (null result)");
                    return Ok(lsp_types::CompletionResponse::Array(vec![]));
                }

                // Try as array first
                if let Ok(mut items) =
                    serde_json::from_value::<Vec<lsp_types::CompletionItem>>(result.clone())
                {
                    // Sort items by sort_text (rust-analyzer provides this for relevance)
                    // Items with no sort_text go to the end
                    items.sort_by(|a, b| match (&a.sort_text, &b.sort_text) {
                        (Some(a_sort), Some(b_sort)) => a_sort.cmp(b_sort),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.label.cmp(&b.label),
                    });

                    tracing::debug!("📦 Received {} completions (Array)", items.len());
                    return Ok(lsp_types::CompletionResponse::Array(items));
                }

                // Try as completion list
                if let Ok(mut list) =
                    serde_json::from_value::<lsp_types::CompletionList>(result.clone())
                {
                    // Sort items in the list as well
                    list.items
                        .sort_by(|a, b| match (&a.sort_text, &b.sort_text) {
                            (Some(a_sort), Some(b_sort)) => a_sort.cmp(b_sort),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => a.label.cmp(&b.label),
                        });

                    tracing::debug!("📦 Received {} completions (List)", list.items.len());
                    return Ok(lsp_types::CompletionResponse::List(list));
                }

                // If we get here, parsing failed
                tracing::error!("⚠️  Failed to parse completion response: {:?}", result);
            } else {
                tracing::error!("⚠️  No 'result' field in response");
            }

            // Return empty on error or no response
            tracing::debug!("❌ No completions - hiding menu");
            Ok(lsp_types::CompletionResponse::Array(vec![]))
        })
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // VSCode behavior: Trigger on almost every keystroke to let rust-analyzer decide
        // rust-analyzer is smart enough to return empty results when appropriate

        tracing::debug!(
            "[LSP TRIGGER] is_completion_trigger called new_text={:?}",
            new_text
        );
        if new_text.is_empty() {
            // Explicit/manual completion invocation paths pass empty text.
            return true;
        }

        let last_char = new_text.chars().last().unwrap();

        // ALWAYS trigger on:
        // 1. Identifier characters (alphanumeric or underscore) - this enables completions as you type
        // 2. rust-analyzer trigger characters (., :, <) - these are special LSP triggers
        // 3. Space after keywords like 'pub', 'use', 'fn', etc.

        // Trigger on identifier characters - this is the most important for continuous completions
        if last_char.is_alphanumeric() || last_char == '_' {
            return true;
        }

        // rust-analyzer registered trigger characters (from LSP spec)
        if matches!(last_char, '.' | ':' | '<') {
            return true;
        }

        // Space is important for keyword completion (e.g., "pub ", "use ", "fn ")
        if last_char == ' ' {
            return true;
        }

        // Additional useful triggers for function calls, generics, etc.
        if matches!(last_char, '(' | ',' | '[') {
            return true;
        }

        // Don't trigger on other special characters
        false
    }
}

impl DefinitionProvider for GlobalRustAnalyzerCompletionProvider {
    fn definitions(
        &self,
        text: &ropey::Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::LocationLink>>> {
        // Check if analyzer is ready (fast check)
        let is_ready = self.analyzer.read(cx).is_running();
        if !is_ready {
            tracing::debug!("⚠️  rust-analyzer is not running, cannot get definitions");
            return Task::ready(Ok(vec![]));
        }

        let uri = self.path_to_uri();
        let position = text.offset_to_position(offset);
        let word = text.word_at(offset);

        tracing::debug!("[LSP DEFINITION] BEFORE ensure_document_open_with_ra");
        self.ensure_document_open_with_ra(text, cx);
        tracing::debug!("[LSP DEFINITION] AFTER ensure_document_open_with_ra");

        // Prepare the request parameters
        let params = json!({
            "textDocument": {
                "uri": uri
            },
            "position": {
                "line": position.line,
                "character": position.character
            }
        });

        // Send the request synchronously (while we still have access to the entity)
        let response_rx = match self
            .analyzer
            .read(cx)
            .send_request_async("textDocument/definition", params)
        {
            Ok(rx) => rx,
            Err(e) => {
                tracing::error!("⚠️  Failed to send definition request: {}", e);
                return Task::ready(Ok(vec![]));
            }
        };

        // Use foreground executor to handle the async work
        let executor = cx.foreground_executor().clone();
        executor.spawn(async move {
            // Wait for response
            let response = match response_rx.recv_async().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::error!("⚠️  Failed to receive definition response: {}", e);
                    return Ok(vec![]);
                }
            };

            // Check for errors
            if let Some(error) = response.get("error") {
                tracing::error!("❌ rust-analyzer definition error: {}", error);
                return Ok(vec![]);
            }

            // Parse the result
            if let Some(result) = response.get("result") {
                if result.is_null() {
                    tracing::debug!("📍 No definition found for '{}'", word);
                    return Ok(vec![]);
                }

                // Try to parse as LocationLink array
                if let Ok(links) =
                    serde_json::from_value::<Vec<lsp_types::LocationLink>>(result.clone())
                {
                    tracing::debug!("✅ Found {} definition(s) for '{}'", links.len(), word);
                    return Ok(links);
                }

                // Try to parse as Location array and convert to LocationLink
                if let Ok(locations) =
                    serde_json::from_value::<Vec<lsp_types::Location>>(result.clone())
                {
                    let links: Vec<lsp_types::LocationLink> = locations
                        .into_iter()
                        .map(|loc| lsp_types::LocationLink {
                            origin_selection_range: None,
                            target_uri: loc.uri,
                            target_range: loc.range,
                            target_selection_range: loc.range,
                        })
                        .collect();
                    tracing::debug!("✅ Found {} definition(s) for '{}'", links.len(), word);
                    return Ok(links);
                }

                // Try single Location
                if let Ok(location) = serde_json::from_value::<lsp_types::Location>(result.clone())
                {
                    let link = lsp_types::LocationLink {
                        origin_selection_range: None,
                        target_uri: location.uri,
                        target_range: location.range,
                        target_selection_range: location.range,
                    };
                    tracing::debug!("✅ Found definition for '{}'", word);
                    return Ok(vec![link]);
                }

                tracing::error!("⚠️  Unexpected definition response format");
            }

            Ok(vec![])
        })
    }
}

impl ui::input::HoverProvider for GlobalRustAnalyzerCompletionProvider {
    fn hover(
        &self,
        text: &ropey::Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        // Check if analyzer is ready (fast check)
        let is_ready = self.analyzer.read(cx).is_running();
        let status = self.analyzer.read(cx).status().clone();
        tracing::debug!(
            "[LSP HOVER] file={:?} is_running={} status={:?} offset={}",
            self.file_path.file_name(),
            is_ready,
            status,
            offset
        );
        if !is_ready {
            tracing::debug!("[LSP HOVER] early-exit: analyzer not running");
            tracing::debug!("⚠️  rust-analyzer is not running, cannot get hover info");
            return Task::ready(Ok(None));
        }

        let uri = self.path_to_uri();
        let position = text.offset_to_position(offset);
        let word = text.word_at(offset);

        tracing::debug!("[LSP HOVER] BEFORE ensure_document_open_with_ra");
        self.ensure_document_open_with_ra(text, cx);
        tracing::debug!("[LSP HOVER] AFTER ensure_document_open_with_ra");

        // Sync current content to rust-analyzer before hovering.
        if self.did_open_sent.load(Ordering::Relaxed) {
            let content = text.to_string();
            let path = self.file_path.clone();
            let version = self.text_version.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.analyzer.update(cx, move |analyzer, _| {
                if let Err(e) = analyzer.did_change_file(&path, &content, version) {
                    tracing::debug!("[LSP SYNC] hover didChange failed: {}", e);
                }
            });
        }

        tracing::debug!("[LSP HOVER] URI being used: {}", uri);
        tracing::debug!(
            "[LSP HOVER] sending textDocument/hover uri={} line={} char={} word={:?}",
            uri,
            position.line,
            position.character,
            word
        );

        // Prepare the request parameters
        let params = json!({
            "textDocument": {
                "uri": uri
            },
            "position": {
                "line": position.line,
                "character": position.character
            }
        });

        // Send the request synchronously (while we still have access to the entity)
        let response_rx = match self
            .analyzer
            .read(cx)
            .send_request_async("textDocument/hover", params)
        {
            Ok(rx) => {
                tracing::debug!("[LSP HOVER] request sent, awaiting response");
                rx
            }
            Err(e) => {
                tracing::debug!("[LSP HOVER] send_request_async failed: {}", e);
                tracing::error!("⚠️  Failed to send hover request: {}", e);
                return Task::ready(Ok(None));
            }
        };

        // Use foreground executor to handle the async work
        let executor = cx.foreground_executor().clone();
        executor.spawn(async move {
            // Wait for response
            let response = match response_rx.recv_async().await {
                Ok(resp) => {
                    tracing::debug!("[LSP HOVER] got response: {:?}", resp);
                    resp
                }
                Err(e) => {
                    tracing::debug!("[LSP HOVER] recv failed: {}", e);
                    tracing::error!("⚠️  Failed to receive hover response: {}", e);
                    return Ok(None);
                }
            };

            // Check for errors
            if let Some(error) = response.get("error") {
                tracing::debug!("[LSP HOVER] error from rust-analyzer: {}", error);
                tracing::error!("❌ rust-analyzer hover error: {}", error);
                return Ok(None);
            }

            // Parse the result
            if let Some(result) = response.get("result") {
                if result.is_null() {
                    tracing::debug!("[LSP HOVER] result is null");
                    return Ok(None);
                }

                // Try to parse as Hover
                if let Ok(hover) = serde_json::from_value::<lsp_types::Hover>(result.clone()) {
                    tracing::debug!(
                        "[LSP HOVER] parsed hover successfully: {:?}",
                        hover.contents
                    );
                    return Ok(Some(hover));
                }

                tracing::error!("⚠️  Unexpected hover response format: {:?}", result);
            }

            Ok(None)
        })
    }
}
