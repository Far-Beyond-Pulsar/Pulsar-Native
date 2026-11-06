//! The bar over the list, the banner for problems, and the status bar under it.

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex,
};

use crate::handlers;
use crate::screen::{KeybindingsWindow, Notice};
use crate::utils::store::KeybindingStore;
use crate::utils::{Filter, file};

impl KeybindingsWindow {
    pub(crate) fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let filter = self.filter;
        let armed = self.confirm_reset_all;
        let has_changes = self.count(Filter::Customized) > 0;

        h_flex()
            .w_full()
            .h(px(56.0))
            .flex_none()
            .px_4()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div().flex_1().max_w(px(480.0)).child(
                    TextInput::new(&self.search_input)
                        .w_full()
                        .prefix(
                            Icon::new(IconName::Search)
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .appearance(true)
                        .bordered(true),
                ),
            )
            .child(
                ButtonGroup::new("keybinding-filter")
                    .children(Filter::ALL.map(|candidate| {
                        let count = self.count(candidate);
                        let label = if candidate == Filter::All || count == 0 {
                            candidate.label().to_owned()
                        } else {
                            format!("{} {count}", candidate.label())
                        };
                        Button::new(candidate.label())
                            .label(label)
                            .small()
                            .selected(filter == candidate)
                    }))
                    .ghost()
                    .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                        if let Some(filter) = selected.first().and_then(|ix| Filter::ALL.get(*ix)) {
                            handlers::set_filter(this, *filter, cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child({
                let button = Button::new("reset-all-keybindings")
                    .icon(IconName::Undo)
                    .label(if armed { "Click again to reset all" } else { "Reset all" })
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| handlers::reset_all(this, cx)));
                if armed {
                    button.danger().into_any_element()
                } else if has_changes {
                    button.ghost().into_any_element()
                } else {
                    button.ghost().opacity(0.4).into_any_element()
                }
            })
            .child(
                Button::new("reveal-keybindings-file")
                    .icon(IconName::FolderOpen)
                    .label("Show file")
                    .small()
                    .ghost()
                    .tooltip("Show keybindings.json in the project's .pulsar folder")
                    .on_click(cx.listener(|this, _, _, cx| handlers::reveal_file(this, cx))),
            )
            .into_any_element()
    }

    /// Problems worth saying out loud: nothing to save to, a file that cannot be
    /// written, entries that were skipped, the last save failing.
    pub(crate) fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let project = Self::project();
        let (read_only, warnings) = {
            let store = KeybindingStore::for_project(project.as_deref(), cx);
            (store.read_only.clone(), store.warnings.clone())
        };

        let mut lines: Vec<(bool, String)> = Vec::new();
        if project.is_none() {
            lines.push((
                true,
                "No project is open, so changes cannot be saved. Open a project to edit shortcuts."
                    .to_owned(),
            ));
        }
        if let Some(reason) = read_only {
            lines.push((true, reason));
        }
        if let Some(Notice::Error(error)) = &self.notice {
            lines.push((true, error.clone()));
        }
        if let Some(first) = warnings.first() {
            let more = warnings.len() - 1;
            lines.push((
                false,
                if more == 0 {
                    format!("Skipped: {first}")
                } else {
                    format!("Skipped: {first} (and {more} more; see the log)")
                },
            ));
        }
        if lines.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .w_full()
                .flex_none()
                .children(lines.into_iter().map(|(error, text)| {
                    let color = if error { theme.danger } else { theme.warning };
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_1p5()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .text_color(theme.foreground)
                        .bg(color.opacity(0.12))
                        .border_b_1()
                        .border_color(color.opacity(0.3))
                        .child(Icon::new(IconName::TriangleAlert).size_4().text_color(color))
                        .child(div().flex_1().min_w_0().child(text))
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let total = self.entries.len();
        let bound = self.entries.iter().filter(|e| !e.bindings.is_empty()).count();
        let changed = self.count(Filter::Customized);
        let conflicts = self.count(Filter::Conflicts);
        let path = Self::project()
            .map(|project| file::keybindings_path(&project).display().to_string());

        let summary = format!(
            "{} of {total} actions shown  ·  {bound} bound  ·  {changed} changed{}",
            self.visible.len(),
            if conflicts > 0 {
                format!("  ·  {conflicts} in conflict")
            } else {
                String::new()
            }
        );

        h_flex()
            .w_full()
            .h(px(28.0))
            .flex_none()
            .px_4()
            .gap_3()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(summary)
            .child(match &self.notice {
                Some(Notice::Info(message)) => div()
                    .text_color(theme.success)
                    .child(message.clone())
                    .into_any_element(),
                _ => div()
                    .min_w_0()
                    .truncate()
                    .child(path.unwrap_or_else(|| "not saved: no project".to_owned()))
                    .into_any_element(),
            })
            .into_any_element()
    }
}
