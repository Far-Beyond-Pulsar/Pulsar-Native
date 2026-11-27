//! The middle pane: the selected entity and its components, with those of
//! the component instances attached to it.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable as _,
};

use crate::components::widgets::{empty_state, virtual_list, MONO};
use crate::screen::SceneInspectorView;
use crate::utils::detail::EntityDetail;
use crate::utils::format;

const ROW_HEIGHT: f32 = 26.0;

/// A row of the list.
#[derive(Clone, Copy)]
enum Entry {
    Section(usize),
    Item(usize, usize),
}

fn entries(detail: &EntityDetail) -> Vec<Entry> {
    detail
        .sections
        .iter()
        .enumerate()
        .flat_map(|(section, content)| {
            std::iter::once(Entry::Section(section))
                .chain((0..content.items.len()).map(move |item| Entry::Item(section, item)))
        })
        .collect()
}

impl SceneInspectorView {
    pub(crate) fn render_components(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pane = v_flex()
            .size_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border);
        let Some(detail) = self.detail.clone() else {
            let (title, text) = match self.selected {
                Some(entity) if !self.refresh.running => (
                    "No longer in the scene".to_string(),
                    format!("{} has been despawned.", format::entity(entity)),
                ),
                Some(_) => ("Reading…".to_string(), String::new()),
                None => (
                    "Nothing selected".to_string(),
                    "Pick an entity on the left, or select an object in the editor.".to_string(),
                ),
            };
            return pane
                .child(empty_state(IconName::Cube, title, text, cx))
                .into_any_element();
        };

        let list = std::rc::Rc::new(entries(&detail));
        let shown = self.data.as_ref().map(|data| data.target);
        let rows = {
            let detail = detail.clone();
            virtual_list(
                "scene-components",
                list.len(),
                &self.component_scroll,
                cx,
                move |_, range, cx| {
                    range
                        .filter_map(|position| {
                            let entry = *list.get(position)?;
                            Some(component_row(&detail, entry, position, shown, cx))
                        })
                        .collect()
                },
            )
        };

        pane.child(self.entity_header(&detail, cx))
            .child(rows)
            .into_any_element()
    }

    fn entity_header(&self, detail: &EntityDetail, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let entity = detail.entity;
        let line = |label: &'static str, value: String| {
            h_flex()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .flex_none()
                        .w(px(62.0))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(MONO)
                        .text_color(theme.foreground)
                        .child(value),
                )
        };
        let identity = {
            let mut text = format!(
                "entity {}\nbits {:#018x}\narchetype #{} row {}",
                format::entity(entity),
                entity.bits(),
                detail.archetype,
                detail.row
            );
            if let Some(id) = &detail.stable_id {
                text.push_str(&format!("\nstable id {id}"));
            }
            text
        };

        v_flex()
            .flex_none()
            .gap_0p5()
            .p_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(detail.label.clone()),
                    )
                    .child(
                        Button::new("copy-identity")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy this entity's identity")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy(identity.clone(), "the entity's identity", cx)
                            })),
                    ),
            )
            .child(line(
                "Entity",
                format!(
                    "{} (index {}, generation {})",
                    format::entity(entity),
                    entity.index(),
                    entity.generation()
                ),
            ))
            .child(line("Bits", format!("{:#018x}", entity.bits())))
            .child(line(
                "Archetype",
                format!("#{} · row {}", detail.archetype, format::count(detail.row)),
            ))
            .when_some(detail.stable_id.clone(), |header, id| {
                header.child(line("Stable id", id))
            })
            .when_some(detail.owner.clone(), |header, (owner, label)| {
                header.child(
                    h_flex()
                        .id("instance-owner")
                        .gap_2()
                        .text_xs()
                        .cursor_pointer()
                        .child(
                            div()
                                .flex_none()
                                .w(px(62.0))
                                .text_color(theme.muted_foreground)
                                .child("Owner"),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(theme.link)
                                .child(format!("{label} ({})", format::entity(owner))),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.follow_link(owner, cx))),
                )
            })
            .into_any_element()
    }
}

fn component_row(
    detail: &EntityDetail,
    entry: Entry,
    position: usize,
    shown: Option<crate::utils::detail::ComponentRef>,
    cx: &mut Context<SceneInspectorView>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let row = h_flex()
        .id(("component-row", position))
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .gap_2()
        .px_2()
        .text_xs();
    match entry {
        Entry::Section(section) => {
            let content = &detail.sections[section];
            let own = content.entity == detail.entity;
            let entity = content.entity;
            row.bg(theme.sidebar)
                .border_b_1()
                .border_color(theme.border)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(div().flex_1().min_w_0().truncate().child(if own {
                    "On this entity".to_string()
                } else {
                    format!("Attached: {}", content.title)
                }))
                .when(content.enabled == Some(false), |row| {
                    row.child(div().text_color(theme.warning).child("disabled"))
                })
                .when(!own, |row| {
                    row.cursor_pointer()
                        .hover(|row| row.bg(theme.list_hover))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_color(theme.link)
                                .child(format::entity(entity)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.follow_link(entity, cx)))
                })
                .into_any_element()
        }
        Entry::Item(section, item) => {
            let item = &detail.sections[section].items[item];
            let target = item.target;
            let tooltip = SharedString::from(item.full);
            row.pl_3()
                .cursor_pointer()
                .hover(|row| row.bg(theme.list_hover))
                .when(shown == Some(target), |row| row.bg(theme.list_active))
                .tooltip(move |window, cx| {
                    ui::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .child(Icon::new(IconName::Component).size_3p5().text_color(
                    if item.class.is_some() {
                        theme.info
                    } else {
                        theme.muted_foreground
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.foreground)
                        .child(item.name.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(MONO)
                        .text_color(theme.muted_foreground)
                        .child(format!("{} B", item.size)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_component(target, cx)))
                .into_any_element()
        }
    }
}
