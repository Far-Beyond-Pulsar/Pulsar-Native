//! Compiler - Compile shader graphs to WGSL code via PSGC

use crate::editor::panel::{CompilationHistoryEntry, ShaderEditorPanel};
use super::{CompilationState, CompilationStatus};
use gpui::*;

// ── ShaderEditorPanel helpers ──────────────────────────────────────────────

impl ShaderEditorPanel {
    pub(crate) fn push_compilation_history(
        &mut self,
        state: CompilationState,
        stage: impl Into<String>,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        const MAX_HISTORY_ENTRIES: usize = 2000;

        let now = chrono::Local::now();
        self.compilation_history.push(CompilationHistoryEntry {
            timestamp: now.format("%H:%M:%S").to_string(),
            state,
            stage: stage.into(),
            message: message.into(),
            detail,
        });

        if self.compilation_history.len() > MAX_HISTORY_ENTRIES {
            let overflow = self.compilation_history.len() - MAX_HISTORY_ENTRIES;
            self.compilation_history.drain(0..overflow);
        }
    }

    /// Record the material preview's latest shader build for the Compiler
    /// Output panel: an error is listed (and added to the history once per
    /// distinct message) until a build succeeds and clears it.
    pub(crate) fn set_material_preview_error(
        &mut self,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.material_preview_error == error {
            return;
        }
        if let Some(message) = &error {
            self.push_compilation_history(
                CompilationState::Error,
                "preview",
                "Material preview failed",
                Some(message.clone()),
            );
        }
        self.material_preview_error = error;
        cx.notify();
    }

    /// Compile to WGSL via PSGC
    pub fn compile_to_wgsl(&self, cx: &App) -> Result<String, String> {
        Self::compile_document(self.document_snapshot(cx)?)
    }

    /// PSGC compile plus WGSL validation. Pure CPU work on owned data: run
    /// it on a worker, never inside a UI update.
    fn compile_document(graph: graphy::GraphDescription) -> Result<String, String> {
        let wgsl = super::lowering::compile_graph_to_wgsl(&graph)?;
        validate_wgsl(&wgsl)?;
        Ok(wgsl)
    }

    /// Start compilation (called from toolbar)
    pub fn start_compilation(&mut self, cx: &mut Context<Self>) {
        let panel_entity = cx.weak_entity();
        cx.spawn(async move |_entity, mut cx| {
            Self::compile_async(panel_entity, &mut cx).await;
        })
        .detach();
    }

    /// Compile in background with status updates
    pub async fn compile_async(panel_entity: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp) {
        let started_at = std::time::Instant::now();

        let result = panel_entity.update(cx, |panel, cx| {
            panel.compilation_status = CompilationStatus {
                state: CompilationState::Compiling,
                message: "Compiling shader...".to_string(),
                progress: 0.0,
                is_compiling: true,
            };

            panel.push_compilation_history(
                CompilationState::Compiling,
                "prepare",
                "Compilation started",
                Some("Compiling shader graph to WGSL".to_string()),
            );

            panel.document_snapshot(cx)
        });

        // The compile itself runs on the background executor; the editor
        // keeps handling input while it works.
        let result = match result {
            Ok(Ok(graph)) => Ok(cx
                .background_executor()
                .spawn(async move { Self::compile_document(graph) })
                .await),
            Ok(Err(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        };

        match result {
            Ok(Ok(wgsl_code)) => {
                let _ = panel_entity.update(cx, |panel, cx| {
                    let elapsed_ms = started_at.elapsed().as_millis();

                    panel.last_compiled_wgsl = Some(wgsl_code.clone());

                    panel.compilation_status = CompilationStatus {
                        state: CompilationState::Success,
                        message: "✓ Compilation successful".to_string(),
                        progress: 1.0,
                        is_compiling: false,
                    };

                    panel.push_compilation_history(
                        CompilationState::Success,
                        "complete",
                        "Compilation successful",
                        Some(format!(
                            "Duration: {} ms | WGSL output: {} bytes",
                            elapsed_ms,
                            wgsl_code.len()
                        )),
                    );

                    cx.notify();
                });
            }
            Ok(Err(e)) => {
                let _ = panel_entity.update(cx, |panel, cx| {
                    let elapsed_ms = started_at.elapsed().as_millis();

                    panel.compilation_status = CompilationStatus {
                        state: CompilationState::Error,
                        message: format!("✗ Compilation failed: {}", e),
                        progress: 0.0,
                        is_compiling: false,
                    };

                    panel.push_compilation_history(
                        CompilationState::Error,
                        "error",
                        "Compilation failed",
                        Some(format!("Duration: {} ms | Reason: {}", elapsed_ms, e)),
                    );

                    cx.notify();
                });
            }
            Err(_) => {
                let _ = panel_entity.update(cx, |panel, cx| {
                    panel.compilation_status = CompilationStatus {
                        state: CompilationState::Error,
                        message: "✗ Compilation failed: panel closed".to_string(),
                        progress: 0.0,
                        is_compiling: false,
                    };
                    panel.push_compilation_history(
                        CompilationState::Error,
                        "error",
                        "Compilation aborted",
                        Some("Editor panel closed before compile completed".to_string()),
                    );
                    cx.notify();
                });
            }
        }

        smol::Timer::after(std::time::Duration::from_secs(3)).await;
        let _ = panel_entity.update(cx, |panel, cx| {
            if panel.compilation_status.state != CompilationState::Compiling {
                panel.compilation_status = CompilationStatus::default();
                cx.notify();
            }
        });
    }
}

pub(crate) fn validate_wgsl(source: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|error| format!("WGSL validation failed: {}", error.emit_to_string(source)))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| format!("WGSL validation failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod wgsl_validation_tests {
    use super::validate_wgsl;

    #[test]
    fn rejects_string_literals_before_wgpu_pipeline_creation() {
        let invalid = r#"
struct FragmentOutput {
    @location(0) color: vec4<f32>,
}

@fragment
fn fragment_main() -> FragmentOutput {
    return FragmentOutput("");
}
"#;

        let error = validate_wgsl(invalid).expect_err("WGSL string literal must be rejected");
        assert!(error.contains("WGSL validation failed"));
    }

    #[test]
    fn accepts_valid_fragment_output() {
        let valid = r#"
struct FragmentOutput {
    @location(0) color: vec4<f32>,
}

@fragment
fn fragment_main() -> FragmentOutput {
    return FragmentOutput(vec4<f32>(0.0, 0.0, 0.0, 1.0));
}
"#;

        validate_wgsl(valid).expect("valid WGSL");
    }
}
