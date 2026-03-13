use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::parser::fmt_count;

struct StatRow {
    label: &'static str,
    value: String,
}

/// Displays model statistics: views, likes, downloads, geometry, textures, PBR, etc.
#[derive(IntoElement)]
pub struct ModelStatsSection {
    pub view_count: i64,
    pub like_count: i64,
    pub download_count: i64,
    pub face_count: Option<i64>,
    pub vertex_count: Option<i64>,
    pub material_count: Option<i32>,
    pub texture_count: Option<i32>,
    pub animation_count: i32,
    pub sound_count: i32,
    pub pbr_type: Option<String>,
}

impl ModelStatsSection {
    pub fn new(
        view_count: i64,
        like_count: i64,
        download_count: i64,
        face_count: Option<i64>,
        vertex_count: Option<i64>,
        material_count: Option<i32>,
        texture_count: Option<i32>,
        animation_count: i32,
        sound_count: i32,
        pbr_type: Option<String>,
    ) -> Self {
        Self {
            view_count, like_count, download_count, face_count, vertex_count,
            material_count, texture_count, animation_count, sound_count, pbr_type,
        }
    }
}

impl RenderOnce for ModelStatsSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        let mut rows: Vec<StatRow> = Vec::new();
        rows.push(StatRow { label: "Views",      value: fmt_count(self.view_count) });
        rows.push(StatRow { label: "Likes",      value: fmt_count(self.like_count) });
        if self.download_count > 0 {
            rows.push(StatRow { label: "Downloads",  value: fmt_count(self.download_count) });
        }
        if let Some(f) = self.face_count {
            rows.push(StatRow { label: "Faces",      value: fmt_count(f) });
        }
        if let Some(v) = self.vertex_count {
            rows.push(StatRow { label: "Vertices",   value: fmt_count(v) });
        }
        if let Some(m) = self.material_count {
            rows.push(StatRow { label: "Materials",  value: m.to_string() });
        }
        if let Some(t) = self.texture_count {
            rows.push(StatRow { label: "Textures",   value: t.to_string() });
        }
        if self.animation_count > 0 {
            rows.push(StatRow { label: "Animations", value: self.animation_count.to_string() });
        }
        if self.sound_count > 0 {
            rows.push(StatRow { label: "Sounds",     value: self.sound_count.to_string() });
        }
        if let Some(pbr) = self.pbr_type {
            rows.push(StatRow { label: "PBR Type",   value: pbr });
        }

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_2()
            .border_t_1()
            .border_color(border)
            .child(
                div().text_xs().font_bold().text_color(muted).child("Model Stats"),
            )
            .children(rows.into_iter().map(|row| {
                h_flex()
                    .w_full()
                    .justify_between()
                    .py(px(3.0))
                    .child(div().text_xs().text_color(muted).child(row.label))
                    .child(div().text_xs().font_medium().text_color(fg).child(row.value))
            }))
    }
}
