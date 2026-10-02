//! Script dispatch summary for the profiler's recorded script scopes.

use crate::trace_data::TraceData;
use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    h_flex, v_flex, ActiveTheme,
};

#[derive(Clone)]
struct ScriptStat {
    name: String,
    calls: usize,
    total_ns: u64,
}

/// Shows calls and elapsed time for script scopes in the selected recording.
pub struct ScriptsPanel {
    trace_data: Arc<TraceData>,
    focus_handle: FocusHandle,
}

impl ScriptsPanel {
    pub fn new(trace_data: Arc<TraceData>, cx: &mut Context<Self>) -> Self {
        Self {
            trace_data,
            focus_handle: cx.focus_handle(),
        }
    }

    fn stats(&self) -> Vec<ScriptStat> {
        let mut by_name: HashMap<String, (usize, u64)> = HashMap::new();
        for span in &self.trace_data.get_frame().spans {
            let Some(name) = span.name.strip_prefix("script:") else {
                continue;
            };
            let entry = by_name.entry(name.to_owned()).or_default();
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(span.duration_ns);
        }
        let mut stats: Vec<_> = by_name
            .into_iter()
            .map(|(name, (calls, total_ns))| ScriptStat {
                name,
                calls,
                total_ns,
            })
            .collect();
        stats.sort_by(|a, b| {
            b.total_ns
                .cmp(&a.total_ns)
                .then_with(|| a.name.cmp(&b.name))
        });
        stats
    }
}

impl EventEmitter<PanelEvent> for ScriptsPanel {}

ui_common::panel_boilerplate!(ScriptsPanel);

impl Panel for ScriptsPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_scripts"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().child("Scripts").into_any_element()
    }
}

impl Render for ScriptsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let stats = self.stats();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .w_full()
                    .h(px(32.0))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .bg(theme.sidebar)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("Script function")
                    .child("Calls   Total"),
            )
            .child(if stats.is_empty() {
                div()
                    .flex_1()
                    .p_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No script scopes in this recording.")
                    .into_any_element()
            } else {
                div()
                    .id("scripts-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(stats.into_iter().take(100).map(|stat| {
                        h_flex()
                            .w_full()
                            .min_h(px(28.0))
                            .px_3()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .border_b_1()
                            .border_color(theme.border.opacity(0.3))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(stat.name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "{}   {:.2} ms",
                                        stat.calls,
                                        stat.total_ns as f64 / 1_000_000.0
                                    )),
                            )
                    }))
                    .into_any_element()
            })
    }
}
