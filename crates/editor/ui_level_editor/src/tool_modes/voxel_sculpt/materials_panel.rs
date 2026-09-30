//! Searchable terrain-material palette. Selection and tool activation are explicit.

use super::panel::VoxelSculptPanel;
use crate::state::voxel::{VoxelSculptDomain, VoxelSculptMode as Mode, MATERIALS};
use gpui::*;
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme, Sizable,
};

pub(super) fn material_label(id: u32) -> String {
    let keys = [
        "LevelEditor.VoxelPanel.Grass",
        "LevelEditor.VoxelPanel.Dirt",
        "LevelEditor.VoxelPanel.Stone",
        "LevelEditor.VoxelPanel.Sand",
        "LevelEditor.VoxelPanel.Snow",
        "LevelEditor.VoxelPanel.Water",
        "LevelEditor.VoxelPanel.Gravel",
        "LevelEditor.VoxelPanel.Sandstone",
        "LevelEditor.VoxelPanel.DarkStone",
        "LevelEditor.VoxelPanel.Wood",
        "LevelEditor.VoxelPanel.Leaves",
        "LevelEditor.VoxelPanel.Clay",
        "LevelEditor.VoxelPanel.Brick",
        "LevelEditor.VoxelPanel.Planks",
        "LevelEditor.VoxelPanel.Cobble",
    ];
    keys.get(id.saturating_sub(1) as usize)
        .map(|key| t!(*key).to_string())
        .unwrap_or_default()
}

/// Base sRGB colors from helio-pass-voxel-planet/shaders/surface.wgsl::palette.
/// Lighting and procedural texture variation are intentionally absent in these chips.
pub(super) fn material_color(id: u32) -> Hsla {
    let colors = [
        0x689636, 0x846040, 0x8a8780, 0xd8c68e, 0xecf1f6, 0x1c485c, 0x706b65, 0xc89868, 0x5a5856,
        0x705032, 0x3e7028, 0xa67658, 0x984838, 0xa47a4c, 0x7a7a78,
    ];
    rgb(*colors
        .get(id.saturating_sub(1) as usize)
        .unwrap_or(&0x808080))
    .into()
}

impl VoxelSculptPanel {
    pub(super) fn render_materials(
        &self,
        brush: VoxelSculptDomain,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let favorite = brush.is_favorite(brush.material);
        let favorite_button = Button::new("voxel_favorite_material")
            .label(
                t!(if favorite {
                    "LevelEditor.VoxelPanel.Unfavorite"
                } else {
                    "LevelEditor.VoxelPanel.Favorite"
                })
                .to_string(),
            )
            .small()
            .ghost()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state
                    .write()
                    .editor
                    .voxel
                    .toggle_favorite(brush.material);
                cx.notify();
            }));
        let selected = v_flex()
            .gap_2()
            .p_2()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .size(px(42.))
                            .rounded(px(4.))
                            .bg(material_color(brush.material)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(material_label(brush.material)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("#{}", brush.material)),
                            ),
                    ),
            )
            .child(favorite_button)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child({
                        let button = self.button(
                            "voxel_material_build",
                            t!("LevelEditor.VoxelPanel.UseBuild").to_string(),
                            cx,
                            |v| v.mode = Mode::Build,
                        );
                        if brush.mode == Mode::Build {
                            button.primary()
                        } else {
                            button.ghost()
                        }
                    })
                    .child({
                        let button = self.button(
                            "voxel_material_paint",
                            t!("LevelEditor.VoxelPanel.UsePaint").to_string(),
                            cx,
                            |v| v.mode = Mode::Paint,
                        );
                        if brush.mode == Mode::Paint {
                            button.primary()
                        } else {
                            button.ghost()
                        }
                    }),
            );

        let query = self
            .search
            .read(cx)
            .text()
            .to_string()
            .trim()
            .to_lowercase();
        let mut palette = h_flex().w_full().flex_wrap().gap_1();
        let mut count = 0;
        for id in MATERIALS {
            let label = material_label(id);
            let engine_name = helio_component::voxel_world::material::NAMES[(id - 1) as usize];
            let favorite = brush.is_favorite(id);
            if self.favorites_only && !favorite {
                continue;
            }
            if !query.is_empty()
                && !label.to_lowercase().contains(&query)
                && !engine_name.to_lowercase().contains(&query)
                && id.to_string() != query
            {
                continue;
            }
            count += 1;
            let active = id == brush.material;
            palette = palette.child(
                v_flex()
                    .id(SharedString::from(format!("voxel_material_{id}")))
                    .w(px(110.))
                    .p_2()
                    .gap_1()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if active { theme.primary } else { theme.border })
                    .bg(if active {
                        theme.primary.opacity(0.15)
                    } else {
                        theme.muted.opacity(0.1)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted.opacity(0.25)))
                    .child(
                        div()
                            .w_full()
                            .h(px(26.))
                            .rounded(px(3.))
                            .bg(material_color(id)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{}#{id}", if favorite { "★ " } else { "" })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.write().editor.voxel.set_material(id);
                        cx.notify();
                    })),
            );
        }
        let filter = Button::new("voxel_favorites_filter")
            .label(t!("LevelEditor.VoxelPanel.Favorites").to_string())
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                this.favorites_only = !this.favorites_only;
                cx.notify();
            }));
        let filter = if self.favorites_only {
            filter.primary()
        } else {
            filter.ghost()
        };
        let clear = Button::new("voxel_material_clear_search")
            .label(t!("LevelEditor.VoxelPanel.Clear").to_string())
            .small()
            .ghost()
            .on_click(cx.listener(|this, _, window, cx| {
                this.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.favorites_only = false;
                cx.notify();
            }));
        let mut library = v_flex()
            .gap_2()
            .child(TextInput::new(&self.search).small())
            .child(h_flex().flex_wrap().gap_1().child(filter).child(clear))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.VoxelPanel.MaterialCount", count => count).to_string()),
            );
        if count == 0 {
            library = library.child(
                div()
                    .text_xs()
                    .child(t!("LevelEditor.VoxelPanel.NoMaterials").to_string()),
            );
        } else {
            library = library.child(palette);
        }
        v_flex()
            .gap_2()
            .child(selected)
            .child(self.section(
                "voxel_palette_section",
                "LevelEditor.VoxelPanel.Palette",
                library.into_any_element(),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.VoxelPanel.PaletteHint").to_string()),
            )
            .into_any_element()
    }
}
