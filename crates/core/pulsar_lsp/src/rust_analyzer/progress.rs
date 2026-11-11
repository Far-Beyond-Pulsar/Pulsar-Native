//! Indexing progress: draining the reader thread's channel on the UI thread,
//! coalescing updates and emitting [`AnalyzerEvent`]s. Entry point:
//! [`RustAnalyzerManager::update_progress_from_thread`].
use gpui::Context;
use std::time::{Duration, Instant};

use super::{
    AnalyzerEvent, AnalyzerStatus, ProgressUpdate, RustAnalyzerManager,
    PROGRESS_EMIT_MILESTONE_STEP, PROGRESS_EMIT_MIN_INTERVAL,
};

impl RustAnalyzerManager {
    /// Drain the progress channel and apply any pending updates.
    ///
    /// Should be called periodically from the UI thread (e.g., from a timer
    /// subscription on the `RustAnalyzerManager` entity).
    pub fn update_progress_from_thread(&mut self, cx: &mut Context<Self>) {
        let mut updates = Vec::new();
        if let Some(rx) = &self.progress_rx {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        for update in updates {
            self.handle_progress_update(update, cx);
        }

        if !self.initial_analysis_complete {
            if let Some(last_update) = self.last_update {
                if last_update.elapsed() > Duration::from_secs(3)
                    && matches!(self.status, AnalyzerStatus::Indexing { .. })
                {
                    self.flush_coalesced_progress(cx);
                    self.initial_analysis_complete = true;
                    self.status = AnalyzerStatus::Ready;
                    tracing::debug!("✅ Initial analysis complete (timeout - no updates for 3s)");
                    cx.emit(AnalyzerEvent::Ready);
                    cx.notify();
                }
            }
        }
    }

    /// Whether a progress update at `progress` should be emitted now, or held
    /// back by the coalescing window. `self.status` always carries the latest
    /// values either way, so suppressed updates are only ever invisible to
    /// subscribers until the next emission or completion flush.
    pub(super) fn should_emit_progress(&self, progress: f32) -> bool {
        let interval_elapsed = self.last_progress_emit.map_or(true, |last| {
            Instant::now().duration_since(last) >= PROGRESS_EMIT_MIN_INTERVAL
        });
        let milestone_crossed = self.last_emitted_progress.map_or(true, |last| {
            Self::progress_milestone(progress) > Self::progress_milestone(last)
        });
        interval_elapsed || milestone_crossed
    }

    pub(super) fn progress_milestone(progress: f32) -> u32 {
        ((progress.clamp(0.0, 100.0)) / PROGRESS_EMIT_MILESTONE_STEP) as u32
    }

    /// Emit any progress suppressed by coalescing so subscribers observe a
    /// final `IndexingProgress` before a Ready/Error transition. A no-op when
    /// the latest status was already emitted or is not an indexing state.
    pub(super) fn flush_coalesced_progress(&mut self, cx: &mut Context<Self>) {
        if let AnalyzerStatus::Indexing { progress, message } = self.status.clone() {
            if self.last_emitted_progress != Some(progress) {
                self.last_progress_emit = Some(Instant::now());
                self.last_emitted_progress = Some(progress);
                cx.emit(AnalyzerEvent::IndexingProgress { progress, message });
            }
        }
    }

    pub(super) fn handle_progress_update(
        &mut self,
        update: ProgressUpdate,
        cx: &mut Context<Self>,
    ) {
        match update {
            ProgressUpdate::Progress { progress, message } => {
                self.status = AnalyzerStatus::Indexing {
                    progress,
                    message: message.clone(),
                };
                self.last_update = Some(Instant::now());
                if self.should_emit_progress(progress) {
                    self.last_progress_emit = Some(Instant::now());
                    self.last_emitted_progress = Some(progress);
                    cx.emit(AnalyzerEvent::IndexingProgress { progress, message });
                    cx.notify();
                }
            }
            ProgressUpdate::Ready => {
                if !self.initial_analysis_complete {
                    self.flush_coalesced_progress(cx);
                    self.initial_analysis_complete = true;
                    self.status = AnalyzerStatus::Ready;
                    tracing::debug!("✅ Initial analysis marked as complete");
                    cx.emit(AnalyzerEvent::Ready);
                    cx.notify();
                }
            }
            ProgressUpdate::Error(e) => {
                self.flush_coalesced_progress(cx);
                self.status = AnalyzerStatus::Error(e.clone());
                cx.emit(AnalyzerEvent::Error(e));
                cx.notify();
            }
            ProgressUpdate::ProcessExited(status) => {
                self.flush_coalesced_progress(cx);
                let error_msg = if status.success() {
                    "rust-analyzer exited normally".to_string()
                } else {
                    format!("rust-analyzer exited with error (status: {:?})", status)
                };
                tracing::debug!("❌ {}", error_msg);
                self.status = AnalyzerStatus::Error(error_msg.clone());
                self.initialized = false;
                cx.emit(AnalyzerEvent::Error(error_msg));
                cx.notify();
            }
            ProgressUpdate::CompletionTriggers(triggers) => {
                self.completion_triggers = triggers;
            }
            ProgressUpdate::FileDiagnostics { key, diagnostics } => {
                if diagnostics.is_empty() {
                    self.file_diagnostics.remove(&key);
                } else {
                    self.file_diagnostics
                        .insert(key.clone(), diagnostics.clone());
                }
                cx.emit(AnalyzerEvent::FileDiagnostics { key, diagnostics });
            }
            ProgressUpdate::Diagnostics(diagnostics) => {
                if self.first_diagnostics_time.is_none() {
                    self.first_diagnostics_time = Some(Instant::now());
                    tracing::debug!("📊 First diagnostics received - analyzer is working");
                }

                if let Some(first_time) = self.first_diagnostics_time {
                    if !self.initial_analysis_complete
                        && first_time.elapsed() > Duration::from_secs(2)
                    {
                        self.initial_analysis_complete = true;
                        self.status = AnalyzerStatus::Ready;
                        tracing::debug!(
                            "✅ Initial analysis complete based on diagnostics (received for 2s)"
                        );
                        cx.emit(AnalyzerEvent::Ready);
                        cx.notify();
                    }
                }

                tracing::debug!(
                    "📤 EMITTING AnalyzerEvent::Diagnostics with {} diagnostics",
                    diagnostics.len()
                );
                cx.emit(AnalyzerEvent::Diagnostics(diagnostics));
            }
        }
    }
}
