use gpui::*;
use std::time::Duration;

use crate::tasks::{TaskStatus, TASKS};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn task_list_widget(
    anim_tick: u32,
    statuses: &[TaskStatus],
    running_elapsed: Option<Duration>,
) -> impl IntoElement {
    let spinner = SPINNER[(anim_tick / 15) as usize % SPINNER.len()];
    let running = statuses
        .iter()
        .position(|s| *s == TaskStatus::Running)
        .unwrap_or(statuses.len().saturating_sub(1));
    const VISIBLE: usize = 5;
    const ABOVE: usize = 2;
    let window_start = running.saturating_sub(ABOVE);
    let window_end = (window_start + VISIBLE).min(statuses.len());
    let window_start = window_end.saturating_sub(VISIBLE);

    div().flex().flex_col().items_end().gap(px(6.0)).children(
        (window_start..window_end).map(|i| {
            let status = statuses[i];
            let label = TASKS[i].0;
            let dist_above = running.saturating_sub(i);
            let (icon, base_opacity): (&str, f32) = match status {
                TaskStatus::Done(_) => ("✓", 0.9),
                TaskStatus::Running => (spinner, 1.0),
                TaskStatus::Pending => ("·", 0.35),
            };
            let opacity = if dist_above > 0 {
                (base_opacity - dist_above as f32 * 0.25).max(0.18)
            } else {
                base_opacity
            };
            let color: Hsla = gpui::white().opacity(opacity);
            let is_running = matches!(status, TaskStatus::Running);
            let timing_label: Option<String> = match status {
                TaskStatus::Done(d) => Some(format!("{}ms", d.as_millis())),
                TaskStatus::Running => running_elapsed.map(|d| format!("{}ms", d.as_millis())),
                TaskStatus::Pending => None,
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .children(timing_label.map(|t| {
                    div()
                        .text_xs()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(gpui::white().opacity(opacity * 0.6))
                        .child(t)
                }))
                .child(if is_running {
                    let chars: Vec<char> = label.chars().collect();
                    let len = chars.len();
                    let speed = 0.2f32;
                    let tail = 5.0f32;
                    let cycle = (len as f32 + tail).max(1.0);
                    let t = ((anim_tick as f32 * speed) % cycle) / cycle;
                    let eased = t * t * (3.0 - 2.0 * t);
                    let front = eased * cycle;
                    div()
                        .text_sm()
                        .font_weight(FontWeight::NORMAL)
                        .flex()
                        .flex_row()
                        .children(chars.into_iter().enumerate().map(|(i, c)| {
                            let dist = front - i as f32;
                            let char_opacity = if dist < 0.0 {
                                opacity * 0.2
                            } else {
                                let t = (dist / tail).min(1.0);
                                opacity * (0.2 + 0.8 * (1.0 - t))
                            };
                            div()
                                .text_sm()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(gpui::white().opacity(char_opacity))
                                .child(c.to_string())
                        }))
                        .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(color)
                        .child(label)
                        .into_any_element()
                })
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .w(px(16.0))
                        .text_center()
                        .child(icon),
                )
        }),
    )
}
