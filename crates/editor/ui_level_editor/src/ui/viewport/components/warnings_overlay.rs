//! Virtualized viewport warnings, refreshed from the renderer's published
//! diagnostics, shown as a log: one colour-coded line per diagnostic.
//!
//! The card is an RnD panel on the viewport's Panel Canvas: its header is the
//! drag handle, and the canvas's resize handles size it (the canvas tracks a
//! resize at the window, so it keeps going when the pointer leaves the card).

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    h_flex,
    panel_canvas::{DragHandle, HandleEdge, PanelRef, RndPanel},
    scroll::{Scrollbar, ScrollbarState},
    v_flex, ActiveTheme, Icon, IconName, Sizable as _,
};

use crate::ui::viewport::performance::RendererStatsSource;

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;

const REFRESH: Duration = Duration::from_millis(250);
/// Every log line is this tall (two lines of text): the list is uniform, so
/// it virtualizes.
const ROW_HEIGHT: f32 = 40.0;

/// How serious a diagnostic is, from its prefix (`WARNING:`, `error:`,
/// `note:`; see Helio's `pipeline_status`). Unprefixed lines are warnings:
/// the renderer publishes this list as its warnings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    severity: Severity,
    /// The line without its severity prefix.
    message: SharedString,
}

impl Entry {
    fn parse(line: &str) -> Self {
        let trimmed = line.trim_start();
        let prefixes = [
            ("error:", Severity::Error),
            ("warning:", Severity::Warning),
            ("note:", Severity::Note),
            ("info:", Severity::Note),
        ];
        for (prefix, severity) in prefixes {
            if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix)
            {
                return Self {
                    severity,
                    message: trimmed[prefix.len()..].trim().to_string().into(),
                };
            }
        }
        Self {
            severity: Severity::Warning,
            message: trimmed.to_string().into(),
        }
    }
}

impl Severity {
    fn icon(self) -> IconName {
        match self {
            Self::Error => IconName::CircleX,
            Self::Warning => IconName::TriangleAlert,
            Self::Note => IconName::Info,
        }
    }

    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Self::Error => theme.danger,
            Self::Warning => theme.warning,
            Self::Note => theme.info,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Note => "Note",
        }
    }
}

pub struct WarningsOverlay {
    entries: Rc<[Entry]>,
    scroll: UniformListScrollHandle,
    scrollbar: ScrollbarState,
    source: RendererStatsSource,
    /// The card this overlay fills; its header moves it.
    panel: PanelRef,
    _tick: Task<()>,
}

impl WarningsOverlay {
    pub fn new(gpu_engine: GpuEngine, panel: PanelRef, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            entries: Rc::from([]),
            scroll: UniformListScrollHandle::default(),
            scrollbar: ScrollbarState::default(),
            source: RendererStatsSource::new(gpu_engine),
            panel,
            _tick: Task::ready(()),
        };
        this.poll();
        this._tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH).await;
            if this
                .update(cx, |this, cx| {
                    if this.poll() {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        });
        this
    }

    fn poll(&mut self) -> bool {
        let next: Rc<[Entry]> = self
            .source
            .latest()
            .and_then(|stats| stats.details)
            .map(|details| {
                details
                    .warnings
                    .iter()
                    .map(|line| Entry::parse(line))
                    .collect()
            })
            .unwrap_or_else(|| Rc::from([]));
        if self.entries == next {
            return false;
        }
        self.entries = next;
        true
    }

    fn count(&self, severity: Severity) -> usize {
        self.entries
            .iter()
            .filter(|e| e.severity == severity)
            .count()
    }
}

/// The warnings card: a resizable RnD panel in the viewport's `left` slot.
pub fn render_warnings_overlay<V>(
    gpu_engine: &GpuEngine,
    panel: PanelRef,
    slot: &std::cell::RefCell<Option<Entity<WarningsOverlay>>>,
    cx: &mut Context<V>,
) -> RndPanel
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let engine = gpu_engine.clone();
            let panel = panel.clone();
            cx.new(|cx| WarningsOverlay::new(engine, panel, cx))
        })
        .clone();
    let theme = cx.theme();
    RndPanel::new(panel)
        .default_slot("left")
        .default_size(size(px(410.0), px(250.0)))
        .min_size(size(px(300.0), px(150.0)))
        .max_size(size(px(760.0), px(560.0)))
        .resizable([
            HandleEdge::Right,
            HandleEdge::Bottom,
            HandleEdge::BottomRight,
        ])
        .bg(theme.background.opacity(0.88))
        .rounded_lg()
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .shadow_lg()
        .child(
            AnyView::from(overlay)
                .cached(StyleRefinement::default().size_full())
                .isolated(),
        )
}

/// A count of one severity in the header: its icon and number, dimmed at 0.
fn badge(severity: Severity, count: usize, cx: &App) -> impl IntoElement {
    let color = if count == 0 {
        cx.theme().muted_foreground.opacity(0.6)
    } else {
        severity.color(cx)
    };
    h_flex()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(px(4.0))
        .when(count > 0, |el| el.bg(severity.color(cx).opacity(0.12)))
        .child(Icon::new(severity.icon()).xsmall().text_color(color))
        .child(
            div()
                .text_xs()
                .font_family("monospace")
                .text_color(color)
                .child(count.to_string()),
        )
}

/// One log line: a severity stripe and icon, its number, and the message
/// (two lines at most). Clicking copies it.
fn row(index: usize, entry: &Entry, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let color = entry.severity.color(cx);
    let hover = theme.secondary.opacity(0.6);
    let text = entry.message.clone();
    let copied = format!("{}: {}", entry.severity.label(), entry.message);
    h_flex()
        .id(("viewport-warning", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_start()
        .gap_2()
        .pr_2()
        .when(index % 2 == 1, |el| el.bg(theme.secondary.opacity(0.25)))
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())))
        .child(div().w(px(3.0)).h_full().flex_shrink_0().bg(color))
        .child(
            div()
                .w(px(22.0))
                .pt(px(5.0))
                .flex_shrink_0()
                .text_xs()
                .font_family("monospace")
                .text_color(theme.muted_foreground.opacity(0.7))
                .text_right()
                .child((index + 1).to_string()),
        )
        .child(
            div()
                .pt(px(5.0))
                .flex_shrink_0()
                .child(Icon::new(entry.severity.icon()).xsmall().text_color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pt(px(4.0))
                .text_xs()
                .line_height(px(15.0))
                .text_color(theme.foreground)
                .line_clamp(2)
                .child(text),
        )
}

impl Render for WarningsOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let entries = self.entries.clone();
        let empty = entries.is_empty();
        let header = DragHandle::new(self.panel.clone())
            .no_grip()
            .w_full()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children((0..3).map(|_| div().w(px(10.0)).h(px(1.5)).bg(muted))),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child("Renderer diagnostics"),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(badge(Severity::Error, self.count(Severity::Error), cx))
                    .child(badge(Severity::Warning, self.count(Severity::Warning), cx))
                    .child(badge(Severity::Note, self.count(Severity::Note), cx)),
            );
        let body = if empty {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Icon::new(IconName::CircleCheck).text_color(theme.success.opacity(0.8)))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("No active renderer warnings"),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .rounded(px(4.0))
                .border_1()
                .border_color(border.opacity(0.4))
                .bg(theme.background.opacity(0.5))
                .overflow_hidden()
                .occlude()
                .child(
                    uniform_list("viewport-warnings", entries.len(), move |range, _, cx| {
                        range
                            .filter_map(|index| {
                                entries.get(index).map(|entry| row(index, entry, cx))
                            })
                            .collect()
                    })
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::uniform_scroll(&self.scrollbar, &self.scroll)),
                )
                .into_any_element()
        };
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .child(header)
            .child(body)
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_color(muted)
                    .child("Click a line to copy it. For lighting overloads, select Lighting: Tile Light Count in the view-mode menu."),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Severity};

    #[test]
    fn severity_comes_from_the_prefix_which_is_stripped() {
        let warning = Entry::parse("WARNING: a screen tile has 300 lights");
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.message.as_ref(), "a screen tile has 300 lights");
        assert_eq!(Entry::parse("error: pass failed").severity, Severity::Error);
        assert_eq!(Entry::parse("  note: rebuilt").severity, Severity::Note);
        // Unprefixed lines are warnings, kept whole.
        let bare = Entry::parse("something odd");
        assert_eq!(bare.severity, Severity::Warning);
        assert_eq!(bare.message.as_ref(), "something odd");
    }
}
