//! The editor's findings collectors: they read signals the engine already
//! keeps (task stalls, plugin problems, the texture detail bias, the asset
//! cache) and publish them to the Problems panel (`pulsar_findings`), and
//! they run every enabled plugin's `FindingSource`. Renderers publish their
//! own findings (`engine_backend::services::renderer_findings`).
//!
//! Everything runs on a worker: each collector reads process-wide state that
//! is safe from any thread and never waits on the frame.

mod editor;
mod performance;
mod project;

use std::collections::BTreeSet;
use std::time::Duration;

use gpui::Context;

/// How often the collectors run.
const TICK: Duration = Duration::from_secs(1);
/// The project collector reads the whole plugin catalog: every few ticks.
const PROJECT_EVERY: u32 = 5;

/// Run the collectors for the app's lifetime.
pub(crate) fn watch<T: 'static>(cx: &mut Context<T>) {
    cx.spawn(async move |_, cx| {
        let mut performance = performance::Performance::default();
        let mut plugin_sources = BTreeSet::new();
        let mut tick = 0u32;
        loop {
            let run_project = tick % PROJECT_EVERY == 0;
            tick = tick.wrapping_add(1);
            let previous = std::mem::take(&mut plugin_sources);
            let (next_performance, next_sources) = cx
                .background_executor()
                .spawn(async move {
                    pulsar_findings::publish(editor::SOURCE, editor::collect());
                    let findings = performance.collect();
                    pulsar_findings::publish(performance::SOURCE, findings);
                    if run_project {
                        pulsar_findings::publish(project::SOURCE, project::collect());
                    }
                    let sources = pulsar_findings::collect_registered(&previous);
                    (performance, sources)
                })
                .await;
            performance = next_performance;
            plugin_sources = next_sources;
            cx.background_executor().timer(TICK).await;
        }
    })
    .detach();
}
