//! Terrain dock panel — one panel, three tabs, modelled on Unreal's
//! Landscape mode:
//!
//! - **Manage** — the terrain actors in the level (planets, flat worlds):
//!   create, pick the one being edited, inspect it.
//! - **Sculpt** — Raise / Lower / Flatten tool grid plus brush options.
//! - **Paint**  — the Paint tool, brush options, and the material palette.
//!
//! Picking a tool or material both selects *and activates* it (the terrain
//! brush becomes the live one), so there is no separate "use this brush"
//! switch. Tabs and section collapse state are view-local; everything else
//! lives in `TerrainDomain`.

use std::collections::HashSet;

use engine_backend::services::terrain_edit::{
    TerrainBodyDefinition, TerrainEditApi, TerrainTarget as BodyTarget, LOD0_CELL_SIZE_METERS,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable,
};

use super::widgets::{
    collapsible_header, info_row, panel_header, segmented_row, stepper_row, swatch_color,
    tool_grid, SharedState, ToolSpec,
};
use crate::level_editor::state::terrain::{
    BrushShape, SculptBrush, SculptMode, TerrainDomain, TerrainTarget,
};
use crate::level_editor::tool_modes::dispatcher::{
    ToolModeDispatcher, ToolWidgetEdit, CREATE_FLAT_WORLD,
};

/// Number of selectable terrain materials (`SculptBrush::material` is 1-based
/// and the palette shows every slot the brush accepts).
const MATERIAL_SLOTS: u32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Manage,
    Sculpt,
    Paint,
}

/// One terrain actor as the Manage tab shows it.
#[derive(Clone, Debug, PartialEq)]
struct BodyRow {
    hex: String,
    is_planet: bool,
    /// Planet radius or flat-world width, in meters.
    size_m: f64,
    material: u8,
}

impl BodyRow {
    fn of(definition: &TerrainBodyDefinition) -> Self {
        let hex = BodyTarget::of(definition).to_hex();
        match definition {
            TerrainBodyDefinition::Planet(planet) => Self {
                hex,
                is_planet: true,
                size_m: planet.radius_cells as f64 * LOD0_CELL_SIZE_METERS,
                material: planet.material,
            },
            TerrainBodyDefinition::Volume(volume) => Self {
                hex,
                is_planet: false,
                size_m: 2.0 * f64::from(volume.flat.extent.0) * volume.flat.cell_size_m,
                material: volume.material,
            },
        }
    }

    fn short_id(&self) -> &str {
        &self.hex[..self.hex.len().min(6)]
    }

    fn is_target(&self, target: &TerrainTarget) -> bool {
        match target {
            TerrainTarget::Planet(id) => self.is_planet && *id == self.hex,
            TerrainTarget::Volume(id) => !self.is_planet && *id == self.hex,
            TerrainTarget::None => false,
        }
    }

    fn as_target(&self) -> TerrainTarget {
        if self.is_planet {
            TerrainTarget::Planet(self.hex.clone())
        } else {
            TerrainTarget::Volume(self.hex.clone())
        }
    }
}

/// Everything the panel draws (view-local tab/collapse state is separate).
#[derive(Clone, Debug, PartialEq)]
struct Signature {
    brush: SculptBrush,
    terrain_brush_active: bool,
    target: TerrainTarget,
    bodies: Vec<BodyRow>,
}

impl Signature {
    fn of(state: &SharedState) -> Self {
        let st = state.read();
        let terrain = &st.editor.terrain;
        Self {
            brush: terrain.sculpt,
            terrain_brush_active: !terrain.paint_foliage,
            target: terrain.target.clone(),
            bodies: body_rows(st.editor.terrain_api.as_ref()),
        }
    }
}

fn body_rows(api: Option<&TerrainEditApi>) -> Vec<BodyRow> {
    api.map(|api| api.bodies().iter().map(BodyRow::of).collect())
        .unwrap_or_default()
}

pub struct TerrainPanel {
    state: SharedState,
    focus_handle: FocusHandle,
    last_signature: Signature,
    pump_started: bool,
    tab: Tab,
    collapsed: HashSet<&'static str>,
}

impl TerrainPanel {
    pub fn new(state: SharedState, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let last_signature = Signature::of(&state);
        Self {
            state,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
            tab: Tab::Manage,
            collapsed: HashSet::new(),
        }
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _window, cx| {
                let signature = Signature::of(&this.state);
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }

    fn toggle_section(&mut self, id: &'static str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id);
        }
    }

    /// A collapsible section header wired to this panel's collapse set.
    fn header(
        &self,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
        id: &'static str,
        label_key: &'static str,
        trailing: Option<AnyElement>,
    ) -> AnyElement {
        collapsible_header(
            theme,
            id,
            label_key,
            self.collapsed.contains(id),
            trailing,
            cx.listener(move |this, _, _, cx| {
                this.toggle_section(id);
                cx.notify();
            }),
        )
        .into_any_element()
    }
}

impl EventEmitter<ui::dock::PanelEvent> for TerrainPanel {}

ui_common::panel_boilerplate!(TerrainPanel);

impl Render for TerrainPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("terrain panel: render");
        let _t = gpui::render_stats::scope("terrain panel: render");

        self.start_pump(window, cx);
        self.last_signature = Signature::of(&self.state);
        let signature = self.last_signature.clone();
        let theme = cx.theme().clone();

        let subtitle = signature
            .bodies
            .iter()
            .find(|b| b.is_target(&signature.target))
            .map(|b| {
                format!(
                    "{} {}",
                    if b.is_planet {
                        t!("LevelEditor.TerrainPanel.Kind.Planet").to_string()
                    } else {
                        t!("LevelEditor.TerrainPanel.Kind.FlatWorld").to_string()
                    },
                    b.short_id()
                )
            })
            .unwrap_or_else(|| t!("LevelEditor.TerrainPalette.NoTarget").to_string());

        let body = match self.tab {
            Tab::Manage => self.render_manage(&signature, &theme, cx).into_any_element(),
            Tab::Sculpt => self.render_sculpt(&signature, &theme, cx).into_any_element(),
            Tab::Paint => self.render_paint(&signature, &theme, cx).into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(panel_header(
                &theme,
                t!("LevelEditor.TerrainPanel.Title").to_string(),
                subtitle,
            ))
            .child(self.render_tab_bar(cx))
            .child(body)
    }
}

impl TerrainPanel {
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = h_flex()
            .w_full()
            .gap_1()
            .p(px(2.0))
            .rounded(px(6.0))
            .bg(cx.theme().muted.opacity(0.1));
        for (tab, icon, label_key) in [
            (Tab::Manage, IconName::Globe, "LevelEditor.TerrainPanel.Tab.Manage"),
            (Tab::Sculpt, IconName::ArrowUp, "LevelEditor.TerrainPanel.Tab.Sculpt"),
            (Tab::Paint, IconName::Palette, "LevelEditor.TerrainPanel.Tab.Paint"),
        ] {
            let button = Button::new(format!("terrain_tab_{label_key}"))
                .icon(icon)
                .label(t!(label_key))
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }));
            bar = bar.child(div().flex_1().child(if self.tab == tab {
                button.primary().w_full()
            } else {
                button.ghost().w_full()
            }));
        }
        bar
    }

    // ── Manage ──────────────────────────────────────────────────────────

    fn render_manage(
        &self,
        signature: &Signature,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let create = Button::new("terrain_create_flat_world")
            .icon(IconName::Plus)
            .label(t!("LevelEditor.Terrain.CreateFlatWorld"))
            .xsmall()
            .primary()
            .on_click(move |_, _, _| {
                let api = state.read().editor.terrain_api.clone();
                let mut st = state.write();
                ToolModeDispatcher::dispatch_widget_edit_with_terrain(
                    &mut st,
                    api.as_ref(),
                    &ToolWidgetEdit::Invoke {
                        id: CREATE_FLAT_WORLD,
                    },
                );
            })
            .into_any_element();

        let mut actors = v_flex().w_full().gap_1();
        if signature.bodies.is_empty() {
            actors = actors.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.TerrainPanel.NoActors").to_string()),
            );
        }
        for body in &signature.bodies {
            let selected = body.is_target(&signature.target);
            let state = self.state.clone();
            let target = body.as_target();
            let (bg, border) = if selected {
                (theme.primary.opacity(0.24), theme.primary)
            } else {
                (theme.muted.opacity(0.08), theme.border.opacity(0.5))
            };
            let hover_bg = theme.muted.opacity(0.22);
            actors = actors.child(
                h_flex()
                    .id(SharedString::from(format!("actor_{}", body.hex)))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(border)
                    .bg(bg)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_bg))
                    .on_click(move |_, _, _| {
                        state.write().editor.terrain.set_target(target.clone());
                    })
                    .child(Icon::new(if body.is_planet {
                        IconName::Globe
                    } else {
                        IconName::Square
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(
                                div().text_xs().font_weight(FontWeight::SEMIBOLD).child(
                                    if body.is_planet {
                                        t!("LevelEditor.TerrainPanel.Kind.Planet").to_string()
                                    } else {
                                        t!("LevelEditor.TerrainPanel.Kind.FlatWorld").to_string()
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(body.short_id().to_string()),
                            ),
                    ),
            );
        }

        let details = signature
            .bodies
            .iter()
            .find(|b| b.is_target(&signature.target))
            .map(|b| {
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(info_row(
                        theme,
                        t!("LevelEditor.TerrainPanel.Detail.Type").to_string(),
                        if b.is_planet {
                            t!("LevelEditor.TerrainPanel.Kind.Planet").to_string()
                        } else {
                            t!("LevelEditor.TerrainPanel.Kind.FlatWorld").to_string()
                        },
                    ))
                    .child(info_row(
                        theme,
                        t!("LevelEditor.TerrainPanel.Detail.Id").to_string(),
                        b.hex.clone(),
                    ))
                    .child(info_row(
                        theme,
                        if b.is_planet {
                            t!("LevelEditor.TerrainPanel.Detail.Radius").to_string()
                        } else {
                            t!("LevelEditor.TerrainPanel.Detail.Width").to_string()
                        },
                        format!("{:.1} m", b.size_m),
                    ))
                    .child(info_row(
                        theme,
                        t!("LevelEditor.TerrainPanel.Detail.Material").to_string(),
                        b.material.to_string(),
                    ))
                    .into_any_element()
            });

        v_flex()
            .w_full()
            .gap_1()
            .child(self.header(theme, cx, "manage_actors", "LevelEditor.TerrainPanel.Section.Actors", Some(create)))
            .when(!self.collapsed.contains("manage_actors"), |el| el.child(actors))
            .child(self.header(theme, cx, "manage_details", "LevelEditor.TerrainPanel.Section.Details", None))
            .when(!self.collapsed.contains("manage_details"), |el| {
                el.child(details.unwrap_or_else(|| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("LevelEditor.TerrainPanel.NoSelection").to_string())
                        .into_any_element()
                }))
            })
    }

    // ── Sculpt ──────────────────────────────────────────────────────────

    fn sculpt_tool(
        id: &'static str,
        icon: IconName,
        label_key: &'static str,
        mode: SculptMode,
        signature: &Signature,
    ) -> ToolSpec {
        ToolSpec {
            id,
            icon,
            label_key,
            active: signature.terrain_brush_active && signature.brush.mode == mode,
            apply: Arc::new(move |domain| domain.activate_sculpt_tool(mode)),
        }
    }

    fn render_sculpt(
        &self,
        signature: &Signature,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let brush = signature.brush;
        v_flex()
            .w_full()
            .gap_2()
            .child(self.header(theme, cx, "sculpt_tools", "LevelEditor.TerrainPanel.Section.Tools", None))
            .when(!self.collapsed.contains("sculpt_tools"), |el| {
                el.child(tool_grid(
                    theme,
                    state.clone(),
                    vec![
                        Self::sculpt_tool("raise", IconName::ArrowUp, "LevelEditor.Terrain.Raise", SculptMode::Raise, signature),
                        Self::sculpt_tool("lower", IconName::ArrowDown, "LevelEditor.Terrain.Lower", SculptMode::Lower, signature),
                        Self::sculpt_tool("flatten", IconName::Ruler, "LevelEditor.Terrain.Flatten", SculptMode::Flatten, signature),
                    ],
                ))
            })
            .child(self.header(theme, cx, "sculpt_brush", "LevelEditor.TerrainPanel.Section.BrushOptions", None))
            .when(!self.collapsed.contains("sculpt_brush"), |el| {
                el.child(self.brush_options(&state, brush, true, cx))
            })
    }

    // ── Paint ───────────────────────────────────────────────────────────

    fn render_paint(
        &self,
        signature: &Signature,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let brush = signature.brush;
        let paint_active = signature.terrain_brush_active && brush.mode == SculptMode::Paint;

        let mut swatches = h_flex().w_full().flex_wrap().gap_1();
        for id in 1..=MATERIAL_SLOTS {
            let selected = paint_active && brush.material == id;
            let state = state.clone();
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("material_{id}")))
                    .size(px(38.0))
                    .rounded(px(4.0))
                    .border_2()
                    .border_color(if selected {
                        theme.primary
                    } else if brush.material == id {
                        theme.border
                    } else {
                        theme.border.opacity(0.0)
                    })
                    .bg(swatch_color(id))
                    .flex()
                    .items_end()
                    .justify_end()
                    .p(px(2.0))
                    .cursor_pointer()
                    .on_click(move |_, _, _| {
                        state.write().editor.terrain.activate_paint_material(id);
                    })
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(white())
                            .child(id.to_string()),
                    ),
            );
        }

        v_flex()
            .w_full()
            .gap_2()
            .child(self.header(theme, cx, "paint_tools", "LevelEditor.TerrainPanel.Section.Tools", None))
            .when(!self.collapsed.contains("paint_tools"), |el| {
                el.child(tool_grid(
                    theme,
                    state.clone(),
                    vec![ToolSpec {
                        id: "paint",
                        icon: IconName::Palette,
                        label_key: "LevelEditor.Terrain.Paint",
                        active: paint_active,
                        apply: Arc::new(|domain| domain.activate_sculpt_tool(SculptMode::Paint)),
                    }],
                ))
            })
            .child(self.header(theme, cx, "paint_brush", "LevelEditor.TerrainPanel.Section.BrushOptions", None))
            .when(!self.collapsed.contains("paint_brush"), |el| {
                el.child(self.brush_options(&state, brush, false, cx))
            })
            .child(self.header(theme, cx, "paint_materials", "LevelEditor.TerrainPanel.Section.Materials", None))
            .when(!self.collapsed.contains("paint_materials"), |el| el.child(swatches))
    }

    /// Shape + size (+ strength for sculpting) + falloff. Paint has no
    /// strength — a paint stamp only moves the material channel.
    fn brush_options(
        &self,
        state: &SharedState,
        brush: SculptBrush,
        with_strength: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut column = v_flex()
            .w_full()
            .gap_2()
            .px_1()
            .child(segmented_row(
                state.clone(),
                cx,
                "terrain_shape",
                &[
                    ("LevelEditor.Terrain.Shape.Sphere", "sphere"),
                    ("LevelEditor.Terrain.Shape.Box", "box"),
                ],
                match brush.shape {
                    BrushShape::Sphere => "sphere",
                    BrushShape::Box => "box",
                },
                |domain, value| {
                    domain.set_brush_shape(match value {
                        "box" => BrushShape::Box,
                        _ => BrushShape::Sphere,
                    })
                },
            ))
            .child(stepper_row(
                state.clone(),
                cx,
                "terrain_size".into(),
                t!("LevelEditor.TerrainPanel.BrushSize").to_string(),
                brush.radius_m,
                1.0,
                64.0,
                0.5,
                TerrainDomain::set_brush_radius,
            ));
        if with_strength {
            column = column.child(stepper_row(
                state.clone(),
                cx,
                "terrain_strength".into(),
                t!("LevelEditor.Terrain.Strength").to_string(),
                brush.strength,
                0.1,
                10.0,
                0.1,
                TerrainDomain::set_brush_strength,
            ));
        }
        column.child(stepper_row(
            state.clone(),
            cx,
            "terrain_falloff".into(),
            t!("LevelEditor.Terrain.Falloff").to_string(),
            brush.falloff,
            0.0,
            1.0,
            0.05,
            TerrainDomain::set_brush_falloff,
        ))
    }
}

impl ui::dock::Panel for TerrainPanel {
    fn panel_name(&self) -> &'static str {
        super::super::layout::TERRAIN_PANEL
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        t!("LevelEditor.TerrainPanel.Title")
            .to_string()
            .into_any_element()
    }
}
