//! The bar over the panes (scene, history, live reading) and the line under
//! them.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Disableable as _, IconName, Sizable as _,
};

use crate::components::widgets::{caption, chip};
use crate::screen::SceneInspectorView;
use crate::utils::format;

impl SceneInspectorView {
    pub(crate) fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.scene.as_ref().map(|(id, _)| *id);
        let scenes: Vec<AnyElement> = self
            .scenes
            .clone()
            .into_iter()
            .map(|(id, label)| {
                chip(
                    ("scene", id as usize),
                    label,
                    current == Some(id),
                    cx,
                    move |this, cx| {
                        if this.scene.as_ref().map(|(current, _)| *current) != Some(id) {
                            if let Some(listed) =
                                engine_backend::services::scene_directory::list()
                                    .into_iter()
                                    .find(|scene| scene.id == id)
                            {
                                this.scene =
                                    Some((id, std::sync::Arc::downgrade(&listed.scene)));
                                this.reset_scene(cx);
                            }
                        }
                    },
                )
                .into_any_element()
            })
            .collect();

        h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("history-back")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowLeft)
                    .tooltip("Back to the previous selection")
                    .disabled(self.back.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
            )
            .child(
                Button::new("history-forward")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowRight)
                    .tooltip("Forward")
                    .disabled(self.forward.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
            )
            .child(caption("Scene", cx))
            .children(scenes)
            .child(div().flex_1())
            .child(
                chip(
                    "follow-editor",
                    "Follow editor selection",
                    self.follow_editor,
                    cx,
                    |this, cx| {
                        this.follow_editor = !this.follow_editor;
                        if this.follow_editor {
                            // Jump to what is selected in the editor now.
                            this.followed = None;
                            this.request_refresh(false, cx);
                        }
                        cx.notify();
                    },
                ),
            )
            .child(chip(
                "live",
                if self.live { "● Live" } else { "Paused" },
                self.live,
                cx,
                |this, cx| {
                    this.live = !this.live;
                    if this.live {
                        this.request_refresh(false, cx);
                    }
                    cx.notify();
                },
            ))
            .child(
                Button::new("refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Refresh)
                    .tooltip("Read the scene again now")
                    .on_click(cx.listener(|this, _, _, cx| this.request_refresh(true, cx))),
            )
            .into_any_element()
    }

    pub(crate) fn render_status(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let summary = match &self.index {
            Some(index) => format!(
                "{} entities · {} objects · {} component types · {} archetypes · revision {}",
                format::count(index.entities.len()),
                format::count(index.object_count),
                format::count(index.types.len()),
                format::count(index.archetypes.len()),
                index.revision,
            ),
            None if self.scene.is_some() => "Reading the scene…".to_string(),
            None => String::new(),
        };
        let cost = format!(
            "scan {:.1} ms · read {:.2} ms",
            self.refresh.scan_time.as_secs_f64() * 1000.0,
            self.refresh.read_time.as_secs_f64() * 1000.0,
        );
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.border)
            .child(div().truncate().child(summary))
            .child(div().flex_1())
            .when_some(self.notice.clone(), |bar, (text, _)| {
                bar.child(div().text_color(theme.foreground).child(text))
            })
            .when(!self.live, |bar| {
                bar.child(
                    div()
                        .text_color(theme.warning)
                        .child("Paused: values are as last read"),
                )
            })
            .child(div().flex_none().child(cost))
            .into_any_element()
    }
}
