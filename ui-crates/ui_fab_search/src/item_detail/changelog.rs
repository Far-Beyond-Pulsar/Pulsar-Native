use gpui::{prelude::*, *};
use ui::{divider::Divider, v_flex, ActiveTheme, StyledExt};

/// A single changelog entry with a version/date header and body text.
pub struct ChangelogEntry {
    pub date: SharedString,
    pub content: SharedString,
}

/// Renders the release history of an asset in a readable list.
#[derive(IntoElement)]
pub struct ChangelogSection {
    pub entries: Vec<ChangelogEntry>,
}

impl ChangelogSection {
    pub fn new(entries: Vec<ChangelogEntry>) -> Self {
        Self { entries }
    }
}

impl RenderOnce for ChangelogSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        let entries = self.entries;

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            // section label
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .child("Changelog"),
            )
            .children(
                entries
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, entry)| {
                        let divider = if i > 0 {
                            Some(
                                div()
                                    .w_full()
                                    .child(Divider::horizontal().color(border))
                                    .into_any_element(),
                            )
                        } else {
                            None
                        };

                        let row = v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(muted)
                                    .child(entry.date),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(fg)
                                    .line_height(relative(1.6))
                                    .child(entry.content),
                            )
                            .into_any_element();

                        [divider, Some(row)]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
