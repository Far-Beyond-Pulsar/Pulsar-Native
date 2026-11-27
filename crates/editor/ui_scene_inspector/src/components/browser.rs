//! The left pane: the scene as objects, entities, component types or
//! archetypes.

use gpui::{prelude::*, *};
use ui::{h_flex, input::TextInput, v_flex, ActiveTheme, Icon, IconName, Sizable as _};

use crate::components::widgets::{chip, empty_state, virtual_list, MONO};
use crate::screen::SceneInspectorView;
use crate::utils::format;
use crate::utils::index::{EntityKind, SceneIndex};
use crate::utils::outline::{Mode, Row};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;
/// Rows are indented no further than this, so a deep tree stays readable.
const MAX_INDENT_DEPTH: u16 = 16;

impl SceneInspectorView {
    pub(crate) fn render_browser(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let modes: Vec<AnyElement> = Mode::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, mode)| {
                chip(
                    ("browser-mode", ix),
                    mode.label(),
                    self.mode == mode,
                    cx,
                    move |this, cx| this.set_mode(mode, cx),
                )
                .into_any_element()
            })
            .collect();

        let body = if self.rows.is_empty() {
            let (title, detail) = if self.index.is_none() {
                ("Reading the scene…", "")
            } else if !self.query.is_empty() {
                (
                    "Nothing matches",
                    "Terms match names and ids. has:Type matches by component, -term excludes.",
                )
            } else {
                (
                    "Nothing here",
                    "The scene has nothing to list in this view.",
                )
            };
            empty_state(IconName::Search, title, detail, cx)
        } else {
            virtual_list(
                "scene-browser",
                self.rows.len(),
                &self.browser_scroll,
                cx,
                |this, range, cx| {
                    let Some(index) = this.index.clone() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|position| {
                            let row = *this.rows.get(position)?;
                            Some(this.browser_row(&index, position, row, cx))
                        })
                        .collect()
                },
            )
            .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(
                v_flex()
                    .flex_none()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(h_flex().gap_1().flex_wrap().children(modes))
                    .child(
                        TextInput::new(&self.search)
                            .small()
                            .cleanable()
                            .prefix(Icon::new(IconName::Search).size_3p5()),
                    ),
            )
            .child(
                v_flex()
                    .id("scene-browser-keys")
                    .track_focus(&self.browser_focus)
                    .on_key_down(cx.listener(Self::on_browser_key))
                    .flex_1()
                    .min_h_0()
                    .child(body),
            )
            .into_any_element()
    }

    fn browser_row(
        &self,
        index: &SceneIndex,
        position: usize,
        row: Row,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let base = h_flex()
            .id(("browser-row", position))
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .gap_1()
            .pr_3()
            .text_xs()
            .cursor_pointer()
            .hover(|row| row.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.browser_focus, cx);
                this.activate_row(position, cx);
            }));

        let chevron = |open: Option<bool>, cx: &mut Context<Self>| {
            let slot = div()
                .id(("browser-chevron", position))
                .flex_none()
                .w(px(16.0))
                .h_full()
                .flex()
                .items_center()
                .justify_center();
            match open {
                Some(open) => slot
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_row(position, cx);
                    })),
                None => slot,
            }
        };
        let name = |text: SharedString| div().flex_1().min_w_0().truncate().child(text);
        let aside = |text: String| {
            div()
                .flex_none()
                .font_family(MONO)
                .text_color(theme.muted_foreground)
                .child(text)
        };

        match row {
            Row::Entity {
                index: ix,
                depth,
                open,
                under,
                matched,
            } => {
                let entity = &index.entities[ix as usize];
                let selected = self.selected == Some(entity.entity)
                    && (under.is_none() || under == self.wanted.map(|wanted| wanted.id));
                let (icon, color) = match entity.kind {
                    EntityKind::Object => (IconName::Cube, theme.primary),
                    EntityKind::Instance { enabled, .. } => (
                        IconName::Component,
                        if enabled {
                            theme.info
                        } else {
                            theme.muted_foreground
                        },
                    ),
                    EntityKind::Other => (IconName::Circle, theme.muted_foreground),
                };
                let indent = f32::from(depth.min(MAX_INDENT_DEPTH)) * INDENT + 4.0;
                base.pl(px(indent))
                    .when(selected, |row| row.bg(theme.list_active))
                    .text_color(if matched {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(chevron(open, cx))
                    .child(Icon::new(icon).size_3p5().text_color(color))
                    .child(name(entity.label.to_string().into()))
                    .child(aside(format::entity(entity.entity)))
                    .into_any_element()
            }
            Row::Type { index: ix, open } => {
                let ty = &index.types[ix as usize];
                base.pl(px(4.0))
                    .text_color(theme.foreground)
                    .child(chevron(Some(open), cx))
                    .child(Icon::new(IconName::Component).size_3p5().text_color(
                        if ty.class.is_some() {
                            theme.info
                        } else {
                            theme.muted_foreground
                        },
                    ))
                    .child(name(ty.name.to_string().into()))
                    .child(aside(format!(
                        "{} × {}",
                        format::count(ty.count),
                        format::bytes(ty.size as u64)
                    )))
                    .into_any_element()
            }
            Row::Archetype {
                index: ix,
                open,
                shown,
            } => {
                let archetype = &index.archetypes[ix as usize];
                let types = archetype
                    .types
                    .iter()
                    .map(|&t| &*index.types[t as usize].name)
                    .collect::<Vec<_>>();
                let label = if types.is_empty() {
                    "(no components)".to_string()
                } else {
                    types.join(", ")
                };
                let tooltip = SharedString::from(label.clone());
                base.pl(px(4.0))
                    .text_color(theme.foreground)
                    .tooltip(move |window, cx| {
                        ui::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                    })
                    .child(chevron(Some(open), cx))
                    .child(aside(format!("#{}", archetype.id)))
                    .child(name(label.into()))
                    .child(aside(format::count(shown as usize)))
                    .into_any_element()
            }
        }
    }
}
