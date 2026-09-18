//! Foliage dock panel — the set/member library and its brush.
//!
//! Hierarchy, top to bottom:
//!
//! ```text
//! Foliage                       header
//! ● Active tool                 activation switch
//! BRUSH      radius · paint density
//! SETS       [+ Add Set]
//!   ▾ ☑ Meadow          3        set row (expand · enable · name · count · ✕)
//!       ☑ oak.mesh               member row (enable · mesh · ✕)
//!       ☑ bush.mesh
//!       [+ Add Mesh]
//!   ▸ ☐ Rocks           1
//! INSPECTOR  (selected set or mesh)
//!   set:    name
//!   mesh:   mesh picker · density · scale range · ground offset · rotation
//! ```
//!
//! All state lives in `TerrainDomain::foliage_sets`
//! ([`crate::level_editor::state::foliage_sets`]); this file is only the view.
//! One [`MeshAssetPicker`] serves the whole panel: it always edits the
//! *selected* member, so "Add Mesh" creates an empty member, selects it, and
//! the inspector's picker fills it in.

use gpui::*;
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{InputEvent, InputState, TextInput},
    popover::Popover,
    v_flex, ActiveTheme, Icon, IconName, Sizable,
};
use ui_common::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

use super::widgets::{
    checkbox_row, collapsible_header, panel_header, stepper_row, tool_grid, SharedState, ToolSpec,
};
use crate::level_editor::state::foliage_sets::{
    FoliageSelection, FoliageSetLibrary, MemberId, MemberPlacement, SetId,
};
use crate::level_editor::state::terrain::{FoliageTool, TerrainDomain};
use gpui::prelude::FluentBuilder as _;
use std::collections::HashSet;
use std::sync::Arc;

/// Everything the panel draws, for the frame pump's cheap diff.
#[derive(Clone, Debug, PartialEq)]
struct FoliageSignature {
    library: FoliageSetLibrary,
    radius_m: f32,
    paint_density: f32,
    erase_density: f32,
    tool: FoliageTool,
    /// Whether the foliage brush (rather than the sculpt brush) is live.
    active: bool,
}

impl FoliageSignature {
    fn of(state: &SharedState) -> Self {
        let st = state.read();
        let terrain = &st.editor.terrain;
        Self {
            library: terrain.foliage_sets.clone(),
            radius_m: terrain.foliage.radius_m,
            paint_density: terrain.foliage_paint_density.0,
            erase_density: terrain.foliage_erase_density.0,
            tool: terrain.foliage_tool,
            active: terrain.paint_foliage,
        }
    }
}

pub struct FoliageSetsPanel {
    state: SharedState,
    focus_handle: FocusHandle,
    last_signature: FoliageSignature,
    pump_started: bool,
    /// Created on first render (needs a `Window`).
    picker: Option<Entity<MeshAssetPicker>>,
    /// Which member the picker's highlight currently reflects, so it is
    /// re-pointed only when the selection actually moves.
    picker_for: Option<(SetId, MemberId)>,
    rename_input: Entity<InputState>,
    /// Which set the rename box currently shows, so its text is replaced only
    /// when the selection moves (never while the user is typing).
    rename_for: Option<SetId>,
    /// Filters the sets list by set or mesh name (Unreal's "Search Foliage").
    search_input: Entity<InputState>,
    collapsed: HashSet<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl FoliageSetsPanel {
    pub fn new(state: SharedState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rename_input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(
            &rename_input,
            window,
            |this, input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).text().to_string();
                    this.apply_rename(text);
                }
            },
        );
        let search_input = cx.new(|cx| InputState::new(window, cx));
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |_this, _input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        let last_signature = FoliageSignature::of(&state);
        Self {
            state,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
            picker: None,
            picker_for: None,
            rename_input,
            rename_for: None,
            search_input,
            collapsed: HashSet::new(),
            _subscriptions: vec![subscription, search_subscription],
        }
    }

    fn toggle_section(&mut self, id: &'static str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id);
        }
    }

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

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _window, cx| {
                let signature = FoliageSignature::of(&this.state);
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }

    fn apply_rename(&self, text: String) {
        let mut st = self.state.write();
        let library = &mut st.editor.terrain.foliage_sets;
        if let Some(FoliageSelection::Set(id)) = library.selection {
            if let Some(set) = library.set_mut(id) {
                if set.name != text {
                    set.name = text;
                }
            }
        }
    }

    fn ensure_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = ["mesh", "fbx", "gltf", "glb", "obj"]
            .into_iter()
            .map(AssetQuery::extension)
            .collect();
        let picker = cx.new(|cx| {
            MeshAssetPicker::new(String::new(), vec![], project_root, queries, window, cx)
        });
        cx.subscribe(&picker, |this, picker, _: &AssetPickedEvent, cx| {
            let path = picker.read(cx).selected_path().to_string();
            let mut st = this.state.write();
            let library = &mut st.editor.terrain.foliage_sets;
            if let Some(FoliageSelection::Member(set, member)) = library.selection {
                if let Some(target) = library.member_mut(set, member) {
                    target.mesh = path;
                }
            }
            drop(st);
            cx.notify();
        })
        .detach();
        self.picker = Some(picker);
    }

    /// Keep the rename box and mesh picker pointed at the current selection.
    fn sync_selection_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selection, set_name, member_mesh) = {
            let st = self.state.read();
            let library = &st.editor.terrain.foliage_sets;
            let name = match library.selection {
                Some(FoliageSelection::Set(id)) => library.set(id).map(|s| s.name.clone()),
                _ => None,
            };
            let mesh = match library.selection {
                Some(FoliageSelection::Member(set, member)) => library
                    .set(set)
                    .and_then(|s| s.members.iter().find(|m| m.id == member))
                    .map(|m| (set, member, m.mesh.clone())),
                _ => None,
            };
            (library.selection, name, mesh)
        };

        let selected_set = match selection {
            Some(FoliageSelection::Set(id)) => Some(id),
            _ => None,
        };
        if selected_set != self.rename_for {
            self.rename_for = selected_set;
            if let Some(name) = set_name {
                self.rename_input
                    .update(cx, |input, cx| input.set_value(&name, window, cx));
            }
        }

        let picker_for = member_mesh.as_ref().map(|(s, m, _)| (*s, *m));
        if picker_for != self.picker_for {
            self.picker_for = picker_for;
            if let (Some(picker), Some((_, _, mesh))) = (&self.picker, member_mesh) {
                picker.update(cx, |picker, _| picker.set_selected_path(mesh));
            }
        }
    }
}

impl EventEmitter<ui::dock::PanelEvent> for FoliageSetsPanel {}

ui_common::panel_boilerplate!(FoliageSetsPanel);

impl Render for FoliageSetsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("terrain foliage panel: render");
        let _t = gpui::render_stats::scope("terrain foliage panel: render");

        self.start_pump(window, cx);
        self.ensure_picker(window, cx);
        self.sync_selection_widgets(window, cx);
        self.last_signature = FoliageSignature::of(&self.state);

        let snapshot = self.last_signature.clone();
        let state = self.state.clone();
        let theme = cx.theme().clone();
        let library = &snapshot.library;

        let search = self.search_input.read(cx).text().to_string().to_lowercase();
        let visible = |set: &crate::level_editor::state::foliage_sets::FoliageSet| {
            search.is_empty()
                || set.name.to_lowercase().contains(&search)
                || set
                    .members
                    .iter()
                    .any(|m| m.mesh.to_lowercase().contains(&search))
        };

        let tool = |id: &'static str,
                    icon: IconName,
                    label_key: &'static str,
                    which: FoliageTool|
         -> ToolSpec {
            ToolSpec {
                id,
                icon,
                label_key,
                active: snapshot.active && snapshot.tool == which,
                apply: Arc::new(move |domain| domain.activate_foliage_tool(which)),
            }
        };

        let add_set = {
            let state = state.clone();
            Button::new("foliage_add_set")
                .icon(IconName::Plus)
                .label(t!("LevelEditor.FoliagePanel.AddSet"))
                .xsmall()
                .primary()
                .on_click(move |_, _, _| {
                    state.write().editor.terrain.foliage_sets.add_set();
                })
                .into_any_element()
        };

        let mut root = v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .child(panel_header(
                &theme,
                t!("LevelEditor.FoliagePanel.Title").to_string(),
                t!(
                    "LevelEditor.FoliagePanel.Summary",
                    sets => library.sets.len(),
                    active => library.paintable_members().count()
                )
                .to_string(),
            ))
            // ── Tools ──
            .child(tool_grid(
                &theme,
                state.clone(),
                vec![
                    tool("paint", IconName::Leaf, "LevelEditor.FoliagePanel.Tool.Paint", FoliageTool::Paint),
                    tool("erase", IconName::Bin, "LevelEditor.FoliagePanel.Tool.Erase", FoliageTool::Erase),
                ],
            ))
            // ── Brush Options ──
            .child(self.header(&theme, cx, "brush", "LevelEditor.FoliagePanel.Section.BrushOptions", None))
            .when(!self.collapsed.contains("brush"), |el| {
                el.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .px_1()
                        .child(stepper_row(
                            state.clone(),
                            cx,
                            "foliage_radius".into(),
                            t!("LevelEditor.TerrainPanel.BrushSize").to_string(),
                            snapshot.radius_m,
                            1.0,
                            64.0,
                            0.5,
                            TerrainDomain::set_foliage_radius,
                        ))
                        .child(stepper_row(
                            state.clone(),
                            cx,
                            "foliage_paint_density".into(),
                            t!("LevelEditor.FoliagePanel.PaintDensity").to_string(),
                            snapshot.paint_density,
                            0.0,
                            1.0,
                            0.05,
                            |domain, value| domain.foliage_paint_density.set(value),
                        ))
                        .child(stepper_row(
                            state.clone(),
                            cx,
                            "foliage_erase_density".into(),
                            t!("LevelEditor.FoliagePanel.EraseDensity").to_string(),
                            snapshot.erase_density,
                            0.0,
                            1.0,
                            0.05,
                            |domain, value| domain.foliage_erase_density.set(value),
                        )),
                )
            })
            // ── Sets ──
            .child(self.header(&theme, cx, "sets", "LevelEditor.FoliagePanel.Section.Sets", Some(add_set)))
            .when(!self.collapsed.contains("sets"), |el| {
                el.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(theme.border.opacity(0.6))
                        .bg(theme.muted.opacity(0.14))
                        .child(Icon::new(IconName::Search).size_3p5().text_color(theme.muted_foreground))
                        .child(TextInput::new(&self.search_input).flex_1()),
                )
            });

        if !self.collapsed.contains("sets") {
            if library.sets.is_empty() {
                root = root.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("LevelEditor.FoliagePanel.NoSets").to_string()),
                );
            }
            for set in library.sets.iter().filter(|s| visible(s)) {
                root = root.child(self.render_set(set.id, library, &state, &theme));
            }
        }

        // ── Inspector ──
        root = root
            .child(self.header(&theme, cx, "inspector", "LevelEditor.FoliagePanel.Section.Inspector", None))
            .when(!self.collapsed.contains("inspector"), |el| {
                el.child(self.render_inspector(library, &state, &theme, cx))
            });
        root
    }
}

impl FoliageSetsPanel {
    fn render_set(
        &self,
        set_id: SetId,
        library: &FoliageSetLibrary,
        state: &SharedState,
        theme: &ui::Theme,
    ) -> impl IntoElement {
        let set = library.set(set_id).expect("rendering an existing set");
        let selected = library.selection == Some(FoliageSelection::Set(set_id));

        let chevron = {
            let state = state.clone();
            Button::new(format!("set_expand_{}", set_id.0))
                .icon(if set.expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .ghost()
                .xsmall()
                .on_click(move |_, _, _| {
                    if let Some(s) = state.write().editor.terrain.foliage_sets.set_mut(set_id) {
                        s.expanded = !s.expanded;
                    }
                })
        };
        let enabled = {
            let state = state.clone();
            let was = set.enabled;
            Checkbox::new(SharedString::from(format!("set_enabled_{}", set_id.0)))
                .checked(set.enabled)
                .on_click(move |_, _, _| {
                    if let Some(s) = state.write().editor.terrain.foliage_sets.set_mut(set_id) {
                        s.enabled = !was;
                    }
                })
        };
        let name = {
            let state = state.clone();
            let button = Button::new(format!("set_select_{}", set_id.0))
                .label(set.name.clone())
                .ghost()
                .small()
                .on_click(move |_, _, _| {
                    state.write().editor.terrain.foliage_sets.selection =
                        Some(FoliageSelection::Set(set_id));
                });
            if selected {
                button.primary()
            } else {
                button
            }
        };
        let remove = {
            let state = state.clone();
            Button::new(format!("set_remove_{}", set_id.0))
                .icon(IconName::X)
                .ghost()
                .xsmall()
                .on_click(move |_, _, _| {
                    state.write().editor.terrain.foliage_sets.remove_set(set_id);
                })
        };

        let mut block = v_flex().w_full().gap_1().child(
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .child(chevron)
                .child(enabled)
                .child(div().flex_1().child(name))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(set.members.len().to_string()),
                )
                .child(remove),
        );

        if set.expanded {
            let mut members = v_flex().w_full().gap_1().pl_6();
            if set.members.is_empty() {
                members = members.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("LevelEditor.FoliagePanel.NoMembers").to_string()),
                );
            }
            for member in &set.members {
                members = members.child(self.render_member(set_id, member.id, library, state, theme));
            }
            let add_mesh = {
                let state = state.clone();
                Button::new(format!("set_add_mesh_{}", set_id.0))
                    .icon(IconName::Plus)
                    .label(t!("LevelEditor.FoliagePanel.AddMesh"))
                    .ghost()
                    .xsmall()
                    .on_click(move |_, _, _| {
                        state
                            .write()
                            .editor
                            .terrain
                            .foliage_sets
                            .add_member(set_id, String::new());
                    })
            };
            block = block.child(members.child(add_mesh));
        }
        block
    }

    fn render_member(
        &self,
        set_id: SetId,
        member_id: MemberId,
        library: &FoliageSetLibrary,
        state: &SharedState,
        theme: &ui::Theme,
    ) -> impl IntoElement {
        let member = library
            .set(set_id)
            .and_then(|s| s.members.iter().find(|m| m.id == member_id))
            .expect("rendering an existing member");
        let selected = library.selection == Some(FoliageSelection::Member(set_id, member_id));

        let enabled = {
            let state = state.clone();
            let was = member.enabled;
            Checkbox::new(SharedString::from(format!("member_enabled_{}", member_id.0)))
                .checked(member.enabled)
                .on_click(move |_, _, _| {
                    if let Some(m) = state
                        .write()
                        .editor
                        .terrain
                        .foliage_sets
                        .member_mut(set_id, member_id)
                    {
                        m.enabled = !was;
                    }
                })
        };
        let label = if member.mesh.is_empty() {
            t!("LevelEditor.FoliagePanel.NoMesh").to_string()
        } else {
            member.display_name()
        };
        let name = {
            let state = state.clone();
            let button = Button::new(format!("member_select_{}", member_id.0))
                .label(label)
                .ghost()
                .small()
                .on_click(move |_, _, _| {
                    state.write().editor.terrain.foliage_sets.selection =
                        Some(FoliageSelection::Member(set_id, member_id));
                });
            if selected {
                button.primary()
            } else {
                button
            }
        };
        let remove = {
            let state = state.clone();
            Button::new(format!("member_remove_{}", member_id.0))
                .icon(IconName::X)
                .ghost()
                .xsmall()
                .on_click(move |_, _, _| {
                    state
                        .write()
                        .editor
                        .terrain
                        .foliage_sets
                        .remove_member(set_id, member_id);
                })
        };

        h_flex()
            .w_full()
            .items_center()
            .gap_1()
            .child(enabled)
            .child(div().flex_1().child(name))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{:.0}", member.placement.density)),
            )
            .child(remove)
    }

    fn render_inspector(
        &self,
        library: &FoliageSetLibrary,
        state: &SharedState,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match library.selection {
            None => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t!("LevelEditor.FoliagePanel.SelectPrompt").to_string())
                .into_any_element(),

            Some(FoliageSelection::Set(_)) => v_flex()
                .w_full()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("LevelEditor.FoliagePanel.SetName").to_string()),
                )
                .child(TextInput::new(&self.rename_input))
                .into_any_element(),

            Some(FoliageSelection::Member(set_id, member_id)) => {
                let Some(member) = library
                    .set(set_id)
                    .and_then(|s| s.members.iter().find(|m| m.id == member_id))
                else {
                    return div().into_any_element();
                };
                self.render_member_inspector(set_id, member_id, member.mesh.clone(), member.placement, state, theme, cx)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_member_inspector(
        &self,
        set_id: SetId,
        member_id: MemberId,
        mesh: String,
        placement: MemberPlacement,
        state: &SharedState,
        theme: &ui::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mesh_label = if mesh.is_empty() {
            t!("LevelEditor.FoliagePanel.NoMesh").to_string()
        } else {
            std::path::Path::new(&mesh)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(mesh)
        };

        let mesh_row = match self.picker.clone() {
            Some(picker) => h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}:", t!("LevelEditor.FoliagePanel.Mesh"))),
                )
                .child(
                    Popover::<MeshAssetPicker>::new(format!("foliage-mesh-picker-{}", member_id.0))
                        .anchor(Corner::BottomRight)
                        .trigger(
                            Button::new(format!("foliage_mesh_btn_{}", member_id.0))
                                .label(mesh_label)
                                .small()
                                .ghost()
                                .dropdown_caret(true),
                        )
                        .content(move |_window, _cx| picker.clone()),
                )
                .into_any_element(),
            None => div().into_any_element(),
        };

        // A member's placement fields all write through the same helper: find
        // the member, apply the clamped setter.
        fn edit<F>(set: SetId, member: MemberId, f: F) -> impl Fn(&mut TerrainDomain, f32) + Clone + Send + Sync + 'static
        where
            F: Fn(&mut MemberPlacement, f32) + Clone + Send + Sync + 'static,
        {
            move |domain, value| {
                if let Some(m) = domain.foliage_sets.member_mut(set, member) {
                    f(&mut m.placement, value);
                }
            }
        }

        v_flex()
            .w_full()
            .gap_2()
            .child(mesh_row)
            .child(stepper_row(
                state.clone(),
                cx,
                format!("member_density_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.Density").to_string(),
                placement.density,
                0.0,
                MemberPlacement::DENSITY_MAX,
                1.0,
                edit(set_id, member_id, |p, v| p.set_density(v)),
            ))
            .child(stepper_row(
                state.clone(),
                cx,
                format!("member_scale_min_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.ScaleMin").to_string(),
                placement.scale_min,
                MemberPlacement::SCALE_MIN_LIMIT,
                MemberPlacement::SCALE_MAX_LIMIT,
                0.05,
                edit(set_id, member_id, |p, v| p.set_scale_min(v)),
            ))
            .child(stepper_row(
                state.clone(),
                cx,
                format!("member_scale_max_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.ScaleMax").to_string(),
                placement.scale_max,
                MemberPlacement::SCALE_MIN_LIMIT,
                MemberPlacement::SCALE_MAX_LIMIT,
                0.05,
                edit(set_id, member_id, |p, v| p.set_scale_max(v)),
            ))
            .child(stepper_row(
                state.clone(),
                cx,
                format!("member_offset_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.GroundOffset").to_string(),
                placement.ground_offset_m,
                -MemberPlacement::OFFSET_LIMIT,
                MemberPlacement::OFFSET_LIMIT,
                0.1,
                edit(set_id, member_id, |p, v| p.set_ground_offset(v)),
            ))
            .child(checkbox_row(
                state.clone(),
                format!("member_yaw_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.RandomYaw").to_string(),
                placement.random_yaw,
                move |domain, on| {
                    if let Some(m) = domain.foliage_sets.member_mut(set_id, member_id) {
                        m.placement.random_yaw = on;
                    }
                },
            ))
            .child(checkbox_row(
                state.clone(),
                format!("member_align_{}", member_id.0),
                t!("LevelEditor.FoliagePanel.AlignToNormal").to_string(),
                placement.align_to_normal,
                move |domain, on| {
                    if let Some(m) = domain.foliage_sets.member_mut(set_id, member_id) {
                        m.placement.align_to_normal = on;
                    }
                },
            ))
            .into_any_element()
    }
}

impl ui::dock::Panel for FoliageSetsPanel {
    fn panel_name(&self) -> &'static str {
        super::super::layout::TERRAIN_FOLIAGE
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        t!("LevelEditor.FoliagePanel.Title")
            .to_string()
            .into_any_element()
    }
}
