//! Tool selection, brush setup, size presets, and stroke guidance.

use super::{
    materials_panel::{material_color, material_label},
    panel::VoxelSculptPanel,
};
use crate::state::voxel::{VoxelSculptDomain, VoxelSculptMode as Mode, MAX_RADIUS_M, MIN_RADIUS_M};
use gpui::*;
use helio_voxel_data::VoxelBrushShape;
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable,
};

impl VoxelSculptPanel {
    pub(super) fn render_brush(
        &mut self,
        brush: VoxelSculptDomain,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let mut tools = h_flex().w_full().flex_wrap().gap_1();
        for (id, key, icon, mode) in [
            (
                "voxel_dig",
                "LevelEditor.Voxel.Dig",
                IconName::ArrowDown,
                Mode::Dig,
            ),
            (
                "voxel_build",
                "LevelEditor.Voxel.Build",
                IconName::ArrowUp,
                Mode::Build,
            ),
            (
                "voxel_paint",
                "LevelEditor.Voxel.Paint",
                IconName::Palette,
                Mode::Paint,
            ),
        ] {
            let active = brush.mode == mode;
            tools = tools.child(
                v_flex()
                    .id(id)
                    .w(px(72.))
                    .h(px(62.))
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if active { theme.primary } else { theme.border })
                    .bg(if active {
                        theme.primary.opacity(0.2)
                    } else {
                        theme.muted.opacity(0.1)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted.opacity(0.25)))
                    .child(Icon::new(icon).size_5())
                    .child(div().text_xs().child(t!(key).to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.write().editor.voxel.mode = mode;
                        cx.notify();
                    })),
            );
        }
        let help_key = match brush.mode {
            Mode::Dig => "LevelEditor.VoxelPanel.DigHint",
            Mode::Build => "LevelEditor.VoxelPanel.BuildHint",
            Mode::Paint => "LevelEditor.VoxelPanel.PaintHint",
        };
        let tools = v_flex()
            .gap_2()
            .child(tools)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!(help_key).to_string()),
            )
            .into_any_element();

        let mut shapes = h_flex().gap_1();
        for (id, key, shape) in [
            (
                "voxel_sphere",
                "LevelEditor.Voxel.Sphere",
                VoxelBrushShape::Sphere,
            ),
            (
                "voxel_cube",
                "LevelEditor.Voxel.Cube",
                VoxelBrushShape::Cube,
            ),
        ] {
            let button = self
                .button(id, t!(key).to_string(), cx, move |v| v.shape = shape)
                .disabled(brush.single_block);
            shapes = shapes.child(if brush.shape == shape {
                button.primary()
            } else {
                button.ghost()
            });
        }
        let precise = Checkbox::new("voxel_single_block")
            .label(t!("LevelEditor.Voxel.SingleBlock").to_string())
            .checked(brush.single_block)
            .on_click(cx.listener(|this, _, _, cx| {
                let mut state = this.state.write();
                state.editor.voxel.single_block = !state.editor.voxel.single_block;
                this.radius_error = false;
                cx.notify();
            }));

        // Keep external changes visible, but never replace a number being typed.
        let value = format!("{:.2}", brush.radius_m);
        if !self.radius_error
            && !self.radius.read(cx).focus_handle(cx).is_focused(window)
            && self.radius.read(cx).text().to_string() != value
        {
            self.radius
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        let step = if brush.radius_m < 2. { 0.1 } else { 0.5 };
        let decrease = self
            .button("voxel_radius_decrease", "−".into(), cx, move |v| {
                v.set_radius(v.radius_m - step)
            })
            .disabled(brush.single_block || brush.radius_m <= MIN_RADIUS_M);
        let increase = self
            .button("voxel_radius_increase", "+".into(), cx, move |v| {
                v.set_radius(v.radius_m + step)
            })
            .disabled(brush.single_block || brush.radius_m >= MAX_RADIUS_M);
        let radius = h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .child(decrease)
            .child(
                div().flex_1().min_w_0().child(
                    TextInput::new(&self.radius)
                        .small()
                        .disabled(brush.single_block),
                ),
            )
            .child(increase)
            .child(div().text_xs().child("m"));
        let size_key = if brush.shape == VoxelBrushShape::Cube {
            "LevelEditor.VoxelPanel.HalfExtent"
        } else {
            "LevelEditor.Voxel.Radius"
        };
        let mut settings = v_flex()
            .gap_2()
            .child(precise)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.VoxelPanel.SingleBlockHint").to_string()),
            )
            .child(shapes)
            .child(div().text_xs().child(t!(size_key).to_string()))
            .child(radius);
        if self.radius_error {
            settings = settings.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(t!("LevelEditor.VoxelPanel.InvalidRadius").to_string()),
            );
        }
        let footprint = if brush.single_block {
            t!("LevelEditor.VoxelPanel.OneVoxel").to_string()
        } else {
            t!("LevelEditor.VoxelPanel.Footprint", size => format!("{:.2}", brush.radius_m * 2.))
                .to_string()
        };
        settings = settings.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(footprint),
        );

        let mut presets = h_flex().flex_wrap().gap_1();
        for (id, key, radius) in [
            ("voxel_preset_fine", "LevelEditor.VoxelPanel.Fine", 0.25),
            ("voxel_preset_detail", "LevelEditor.VoxelPanel.Detail", 0.5),
            (
                "voxel_preset_standard",
                "LevelEditor.VoxelPanel.Standard",
                1.5,
            ),
            ("voxel_preset_broad", "LevelEditor.VoxelPanel.Broad", 4.),
            ("voxel_preset_large", "LevelEditor.VoxelPanel.Large", 8.),
            (
                "voxel_preset_massive",
                "LevelEditor.VoxelPanel.ExtraLarge",
                16.,
            ),
        ] {
            let button = self
                .button(id, format!("{} · {radius} m", t!(key)), cx, move |v| {
                    v.set_radius(radius)
                })
                .disabled(brush.single_block);
            presets = presets.child(if (brush.radius_m - radius).abs() < 0.001 {
                button.primary()
            } else {
                button.ghost()
            });
        }
        let reset = Button::new("voxel_reset_brush")
            .label(t!("LevelEditor.VoxelPanel.ResetBrush").to_string())
            .small()
            .ghost()
            .on_click(cx.listener(|this, _, window, cx| {
                let defaults = VoxelSculptDomain::default();
                {
                    let mut state = this.state.write();
                    let brush = &mut state.editor.voxel;
                    brush.shape = defaults.shape;
                    brush.set_radius(defaults.radius_m);
                    brush.single_block = false;
                }
                this.radius_error = false;
                this.radius.update(cx, |input, cx| {
                    input.set_value(format!("{:.2}", defaults.radius_m), window, cx);
                });
                cx.notify();
            }));
        let presets = v_flex()
            .gap_2()
            .child(presets)
            .child(reset)
            .into_any_element();

        let material = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(4.))
                            .bg(material_color(brush.material)),
                    )
                    .child(div().text_sm().child(material_label(brush.material))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.VoxelPanel.MaterialHint").to_string()),
            )
            .into_any_element();
        let guidance = v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .child(t!("LevelEditor.VoxelPanel.StrokeHint").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .child(t!("LevelEditor.VoxelPanel.ShiftHint").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .child(t!("LevelEditor.VoxelPanel.TargetHint").to_string()),
            )
            .into_any_element();

        v_flex()
            .gap_2()
            .child(self.section(
                "voxel_tools_section",
                "LevelEditor.VoxelPanel.Operation",
                tools,
                cx,
            ))
            .child(self.section(
                "voxel_brush_section",
                "LevelEditor.VoxelPanel.Brush",
                settings.into_any_element(),
                cx,
            ))
            .child(self.section(
                "voxel_presets_section",
                "LevelEditor.VoxelPanel.Presets",
                presets,
                cx,
            ))
            .child(self.section(
                "voxel_material_section",
                "LevelEditor.VoxelPanel.ActiveMaterial",
                material,
                cx,
            ))
            .child(self.section(
                "voxel_help_section",
                "LevelEditor.VoxelPanel.Controls",
                guidance,
                cx,
            ))
            .into_any_element()
    }
}
