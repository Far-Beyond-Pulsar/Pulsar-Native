use crate::parser::fmt_count;
use gpui::{prelude::*, *};
use ui::{ActiveTheme, StyledExt, h_flex, v_flex};

struct StatRow {
    label: &'static str,
    value: String,
}

/// Displays technical model statistics: geometry, textures, PBR, and similar metadata.
#[derive(IntoElement)]
pub struct ModelStatsSection {
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
        face_count: Option<i64>,
        vertex_count: Option<i64>,
        material_count: Option<i32>,
        texture_count: Option<i32>,
        animation_count: i32,
        sound_count: i32,
        pbr_type: Option<String>,
    ) -> Self {
        Self {
            face_count,
            vertex_count,
            material_count,
            texture_count,
            animation_count,
            sound_count,
            pbr_type,
        }
    }
}

impl RenderOnce for ModelStatsSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        let mut rows: Vec<StatRow> = Vec::new();
        if let Some(f) = self.face_count {
            rows.push(StatRow {
                label: "Faces",
                value: fmt_count(f),
            });
        }
        if let Some(v) = self.vertex_count {
            rows.push(StatRow {
                label: "Vertices",
                value: fmt_count(v),
            });
        }
        if let Some(m) = self.material_count {
            rows.push(StatRow {
                label: "Materials",
                value: m.to_string(),
            });
        }
        if let Some(t) = self.texture_count {
            rows.push(StatRow {
                label: "Textures",
                value: t.to_string(),
            });
        }
        if self.animation_count > 0 {
            rows.push(StatRow {
                label: "Animations",
                value: self.animation_count.to_string(),
            });
        }
        if self.sound_count > 0 {
            rows.push(StatRow {
                label: "Sounds",
                value: self.sound_count.to_string(),
            });
        }
        if let Some(pbr) = self.pbr_type {
            rows.push(StatRow {
                label: "PBR Type",
                value: pbr,
            });
        }

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_2()
            .border_t_1()
            .border_color(border)
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .child("Model Stats"),
            )
            .children(rows.into_iter().map(|row| {
                h_flex()
                    .w_full()
                    .justify_between()
                    .py(px(3.0))
                    .child(div().text_xs().text_color(muted).child(row.label))
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(fg)
                            .child(row.value),
                    )
            }))
    }
}
