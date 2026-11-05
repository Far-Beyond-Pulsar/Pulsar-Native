//! Terrain dock panel: create the level's voxel world in one click from a
//! ready-made preset (Earth-like planet, Moon, desert planet, flat block
//! world: `voxel_component::presets`), and the level's voxel terrains, each
//! selected with a click so the inspector shows its layers and features.
//! Worlds are sculpted with the voxel sculpt tool; foliage is the foliage
//! plugin's Foliage mode.
//!
//! The section collapse state is view-local.

use std::collections::HashSet;

use engine_backend::scene::{SceneWorldExt, attachments};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use ui::{
    ActiveTheme, Icon, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use voxel_component::presets::VoxelWorldPreset;
use voxel_component::{VoxelTerrainComponent, VoxelWorldShape};

use ui_level_editor::commands::{SceneCommand, execute_command};
use ui_level_editor::extend::panel_kit::{SharedState, collapsible_header};

/// One voxel terrain as the World tab shows it.
#[derive(Clone, Debug, PartialEq)]
struct TerrainRow {
    /// Its object's id.
    id: String,
    name: String,
    shape: VoxelWorldShape,
    /// Radius of a planet, edge of a plane, in metres.
    size_m: f64,
    edits: usize,
}

impl TerrainRow {
    fn kind(&self) -> String {
        match self.shape {
            VoxelWorldShape::Sphere => t!(
                "LevelEditor.TerrainPanel.Planet",
                radius => format!("{:.0}", self.size_m / 1000.0)
            )
            .to_string(),
            VoxelWorldShape::Plane => t!(
                "LevelEditor.TerrainPanel.Plane",
                size => format!("{:.0}", self.size_m)
            )
            .to_string(),
            VoxelWorldShape::InfinitePlane => {
                t!("LevelEditor.TerrainPanel.InfinitePlane").to_string()
            }
        }
    }
}

/// Everything the panel draws (view-local tab/collapse state is separate).
#[derive(Clone, Debug, PartialEq)]
struct Signature {
    terrains: Vec<TerrainRow>,
    selected: Option<String>,
}

impl Signature {
    fn of(state: &SharedState) -> Self {
        let st = state.read();
        let world = st.scene.world();
        let mut terrains: Vec<TerrainRow> = world
            .query::<&VoxelTerrainComponent>()
            .filter_map(|(instance, terrain)| {
                let owner = attachments::owner_of(&world, instance)?;
                Some(TerrainRow {
                    id: world.stable_id_of(owner)?.to_string(),
                    name: world
                        .get::<engine_backend::scene::Name>(owner)
                        .map(|name| name.0.clone())
                        .unwrap_or_default(),
                    shape: terrain.shape,
                    size_m: match terrain.shape {
                        VoxelWorldShape::Sphere => terrain.planet_radius,
                        _ => terrain.plane_size,
                    },
                    edits: terrain.edits.len(),
                })
            })
            .collect();
        terrains.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            terrains,
            selected: world.selected_id(),
        }
    }
}

/// A preset's button: its icon and locale keys (name, description).
fn preset_entry(preset: VoxelWorldPreset) -> (IconName, &'static str, &'static str) {
    match preset {
        VoxelWorldPreset::Earth => (
            IconName::Globe,
            "LevelEditor.TerrainPanel.Preset.Earth",
            "LevelEditor.TerrainPanel.Preset.EarthDesc",
        ),
        VoxelWorldPreset::Moon => (
            IconName::Moon,
            "LevelEditor.TerrainPanel.Preset.Moon",
            "LevelEditor.TerrainPanel.Preset.MoonDesc",
        ),
        VoxelWorldPreset::Desert => (
            IconName::Sun,
            "LevelEditor.TerrainPanel.Preset.Desert",
            "LevelEditor.TerrainPanel.Preset.DesertDesc",
        ),
        VoxelWorldPreset::Blocks => (
            IconName::Box,
            "LevelEditor.TerrainPanel.Preset.Blocks",
            "LevelEditor.TerrainPanel.Preset.BlocksDesc",
        ),
    }
}

pub struct TerrainPanel {
    state: SharedState,
    focus_handle: FocusHandle,
    last_signature: Signature,
    pump_started: bool,
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
            collapsed: HashSet::new(),
        }
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        ui_level_editor::extend::spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let signature = Signature::of(&this.state);
            if signature != this.last_signature {
                this.last_signature = signature;
                cx.notify();
            }
        });
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
    ) -> AnyElement {
        collapsible_header(
            theme,
            id,
            t!(label_key).to_string(),
            self.collapsed.contains(id),
            None,
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

        let subtitle = match signature.terrains.as_slice() {
            [] => t!("LevelEditor.TerrainPanel.NoTerrain").to_string(),
            [terrain] => format!("{} · {}", terrain.name, terrain.kind()),
            terrains => {
                t!("LevelEditor.TerrainPanel.Terrains", count => terrains.len()).to_string()
            }
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(theme.sidebar)
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border.opacity(0.55))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    ),
            )
            .child(
                v_flex()
                    .id("terrain-panel-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_2()
                    .child(self.render_world(&signature, &theme, cx)),
            )
    }
}

impl TerrainPanel {
    // ── World ───────────────────────────────────────────────────────────

    fn render_world(
        &self,
        signature: &Signature,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut presets = v_flex().w_full().gap_1();
        for preset in VoxelWorldPreset::ALL {
            let (icon, name_key, description_key) = preset_entry(preset);
            let state = self.state.clone();
            let hover_bg = theme.muted.opacity(0.22);
            presets = presets.child(
                h_flex()
                    .id(SharedString::from(format!("terrain_create_{preset:?}")))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme.border.opacity(0.5))
                    .bg(theme.muted.opacity(0.08))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_bg))
                    .on_click(move |_, _, _| {
                        let mut st = state.write();
                        let result = execute_command(
                            &mut st,
                            SceneCommand::Extension(Box::new(
                                super::super::add_world::AddVoxelWorld { preset },
                            )),
                        );
                        if !result.changed {
                            tracing::warn!("Create voxel terrain: {}", result.no_op_reason);
                        }
                    })
                    .child(Icon::new(icon).text_color(theme.primary))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!(name_key).to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t!(description_key).to_string()),
                            ),
                    ),
            );
        }
        let create_hint = if signature.terrains.is_empty() {
            t!("LevelEditor.TerrainPanel.CreateHint")
        } else {
            t!("LevelEditor.TerrainPanel.ReplaceHint")
        };

        let mut terrains = v_flex().w_full().gap_1();
        if signature.terrains.is_empty() {
            terrains = terrains.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.TerrainPanel.NoTerrain").to_string()),
            );
        }
        for terrain in &signature.terrains {
            let selected = signature.selected.as_deref() == Some(terrain.id.as_str());
            let (bg, border) = if selected {
                (theme.primary.opacity(0.24), theme.primary)
            } else {
                (theme.muted.opacity(0.08), theme.border.opacity(0.5))
            };
            let hover_bg = theme.muted.opacity(0.22);
            let state = self.state.clone();
            let id = terrain.id.clone();
            let details = if terrain.edits > 0 {
                t!(
                    "LevelEditor.TerrainPanel.WithEdits",
                    kind => terrain.kind(),
                    edits => terrain.edits
                )
                .to_string()
            } else {
                terrain.kind()
            };
            terrains = terrains.child(
                h_flex()
                    .id(SharedString::from(format!("terrain_{}", terrain.id)))
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
                        execute_command(
                            &mut state.write(),
                            SceneCommand::SelectObject {
                                id: Some(id.clone()),
                            },
                        );
                    })
                    .child(Icon::new(IconName::Globe))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(terrain.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(details),
                            ),
                    ),
            );
        }
        let sculpt = Button::new("terrain_sculpt")
            .icon(IconName::Cube)
            .label(t!("LevelEditor.TerrainPanel.Sculpt"))
            .small()
            .primary()
            .w_full()
            .on_click(cx.listener(|this, _, window, cx| {
                // The voxel sculpt mode, through the editor's own mode switch.
                window.focus(&this.focus_handle, cx);
                window.dispatch_action(
                    Box::new(ui_level_editor::extend::SetToolMode(
                        super::super::SCULPT_MODE,
                    )),
                    cx,
                );
            }));

        v_flex()
            .w_full()
            .gap_1()
            .child(self.header(
                theme,
                cx,
                "world_create",
                "LevelEditor.TerrainPanel.Section.Create",
            ))
            .when(!self.collapsed.contains("world_create"), |el| {
                el.child(presets).child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(create_hint.to_string()),
                )
            })
            .child(self.header(
                theme,
                cx,
                "world_terrains",
                "LevelEditor.TerrainPanel.Section.Terrains",
            ))
            .when(!self.collapsed.contains("world_terrains"), |el| {
                el.child(terrains)
                    .when(!signature.terrains.is_empty(), |el| {
                        el.child(div().pt_1().child(sculpt)).child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t!("LevelEditor.TerrainPanel.TerrainsHint").to_string()),
                        )
                    })
            })
    }
}

impl ui::dock::Panel for TerrainPanel {
    fn panel_name(&self) -> &'static str {
        "terrain.panel"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        t!("LevelEditor.TerrainPanel.Title")
            .to_string()
            .into_any_element()
    }
}
