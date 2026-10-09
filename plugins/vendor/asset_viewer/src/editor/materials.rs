//! Default material assignment for native `.mesh` assets.
//!
//! The viewer edits the asset's own per-slot default materials (stored in the
//! `.mesh` metadata by `helio_component::mesh_cache::set_default_materials`).
//! Placed `StaticMeshComponent`s inherit them for any slot they do not
//! override, so a mesh can ship with its materials already applied.

use gpui::*;
use ui_common::asset_picker::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

use super::panel::AssetViewerPanel;

/// One material slot of the open `.mesh` and its picker.
pub struct MaterialSlotRow {
    pub name: String,
    pub material: String,
    pub picker: Entity<MeshAssetPicker>,
}

/// Material-assignment state of the open `.mesh`.
pub struct MeshMaterials {
    pub rows: Vec<MaterialSlotRow>,
    pub apply_all: Entity<MeshAssetPicker>,
}

fn material_queries() -> Vec<AssetQuery> {
    vec![
        AssetQuery::extension("mat"),
        AssetQuery::folder_marker("shader_graph_save.json"),
    ]
}

fn new_picker(
    selected: String,
    window: &mut Window,
    cx: &mut Context<AssetViewerPanel>,
) -> Entity<MeshAssetPicker> {
    let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
    cx.new(|cx| MeshAssetPicker::new(selected, Vec::new(), project_root, material_queries(), window, cx))
}

impl AssetViewerPanel {
    /// Read the `.mesh`'s slots and build one picker per slot plus an
    /// "apply to all" picker. Does nothing for other file types.
    pub(crate) fn init_mesh_materials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.current_path.clone() else {
            return;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("mesh") {
            return;
        }
        let Some(asset) = helio_component::subsystems::load_mesh_asset_upload(&path) else {
            log::error!("Could not read native mesh {:?}", path);
            return;
        };
        self.mesh_sections = asset
            .sections
            .iter()
            .map(|s| (s.first_index, s.index_count, s.material_slot as usize))
            .collect();
        self.slot_surfaces = asset.material_slots.iter().map(|s| s.surface).collect();
        let mut rows = Vec::new();
        for (index, slot) in asset.material_slots.iter().enumerate() {
            let picker = new_picker(slot.material_asset.clone(), window, cx);
            self.subscriptions.push(cx.subscribe_in(
                &picker,
                window,
                move |this: &mut Self, picker, _: &AssetPickedEvent, _window, cx| {
                    let selected = picker.read(cx).selected_path().to_owned();
                    this.set_slot_material(index, selected, cx);
                },
            ));
            rows.push(MaterialSlotRow {
                name: if slot.name.is_empty() {
                    format!("Material Slot {}", index + 1)
                } else {
                    slot.name.clone()
                },
                material: slot.material_asset.clone(),
                picker,
            });
        }
        let apply_all = new_picker(String::new(), window, cx);
        self.subscriptions.push(cx.subscribe_in(
            &apply_all,
            window,
            |this: &mut Self, picker, _: &AssetPickedEvent, _window, cx| {
                let selected = picker.read(cx).selected_path().to_owned();
                if !selected.is_empty() {
                    this.apply_material_to_all(selected, cx);
                }
            },
        ));
        self.mesh_materials = Some(MeshMaterials { rows, apply_all });
        self.refresh_slot_colors();
    }

    /// Recompute each slot's preview colour from its current material: the
    /// scalar surface of a `.mat`, else the imported surface. (Shader-graph
    /// materials are not evaluated here and show the imported surface.)
    fn refresh_slot_colors(&mut self) {
        let Some(materials) = self.mesh_materials.as_ref() else {
            return;
        };
        self.slot_colors = materials
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let slot = helio_component::components::StaticMeshMaterialSlot {
                    imported_surface: self.slot_surfaces.get(index).copied().unwrap_or_default(),
                    material_asset: row.material.clone(),
                    ..Default::default()
                };
                helio_component::components::resolve_slot_material(Some(&slot), None)
                    .surface
                    .base_color
            })
            .collect();
        self.rebuild_graph_draws();
    }

    /// Compile a pipeline for every slot whose material is a shader graph.
    /// Needs the GPU device, so it is a no-op until the surface exists;
    /// `init_surface` calls it again once it does.
    pub(crate) fn rebuild_graph_draws(&mut self) {
        let (Some(device), Some(queue), Some(config), Some(layout), Some(globals_layout), Some(materials)) = (
            self.device.clone(),
            self.queue.clone(),
            self.surface_config.clone(),
            self.mesh_bgl.clone(),
            self.globals_layout.clone(),
            self.mesh_materials.as_ref(),
        ) else {
            return;
        };
        let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from) else {
            return;
        };
        let ctx = super::graph_material::GraphContext {
            device: &device,
            queue: &queue,
            target_format: config.format,
            globals_layout: &globals_layout,
            uniform_layout: &layout,
        };
        let vertex_layout = super::panel_render::mesh_vertex_layout();
        let draws = materials
            .rows
            .iter()
            .map(|row| {
                let preview =
                    helio_component::graph_preview::compile_graph_preview(&root, &row.material)?;
                let built = preview.and_then(|preview| {
                    super::graph_material::build(&ctx, &preview, &[Some(vertex_layout.clone())])
                });
                built
                    .map_err(|error| log::warn!("graph material {:?}: {error}", row.material))
                    .ok()
            })
            .collect();
        self.graph_draws = draws;
    }

    fn set_slot_material(&mut self, index: usize, material: String, cx: &mut Context<Self>) {
        let Some(materials) = self.mesh_materials.as_mut() else {
            return;
        };
        let Some(row) = materials.rows.get_mut(index) else {
            return;
        };
        if row.material == material {
            return;
        }
        row.material = material;
        self.refresh_slot_colors();
        self.save_mesh_materials();
        cx.notify();
    }

    fn apply_material_to_all(&mut self, material: String, cx: &mut Context<Self>) {
        let Some(materials) = self.mesh_materials.as_mut() else {
            return;
        };
        for row in &mut materials.rows {
            row.material = material.clone();
            row.picker
                .update(cx, |picker, _| picker.set_selected_path(material.clone()));
        }
        self.refresh_slot_colors();
        self.save_mesh_materials();
        cx.notify();
    }

    /// Persist the assignments into the `.mesh`; placed meshes are told to
    /// reload by `set_default_materials`' asset-update event.
    fn save_mesh_materials(&self) {
        let (Some(path), Some(materials)) = (self.current_path.as_ref(), self.mesh_materials.as_ref())
        else {
            return;
        };
        let assignments: Vec<String> = materials.rows.iter().map(|row| row.material.clone()).collect();
        if let Err(error) = helio_component::mesh_cache::set_default_materials(path, &assignments) {
            log::error!("Could not save default materials for {:?}: {error}", path);
        }
    }
}

fn picker_button(
    id: String,
    label: String,
    picker: Entity<MeshAssetPicker>,
) -> impl IntoElement {
    use ui::button::{Button, ButtonVariants as _};
    use ui::{popover::Popover, Sizable};
    Popover::<MeshAssetPicker>::new(format!("{id}-popover"))
        .anchor(Corner::BottomRight)
        .trigger(
            Button::new(format!("{id}-btn"))
                .label(label)
                .small()
                .ghost()
                .dropdown_caret(true),
        )
        .content(move |_window, _cx| picker.clone())
}

fn file_label(path: &str, empty: &str) -> String {
    if path.is_empty() {
        return empty.to_owned();
    }
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// The "Default Materials" section of the properties panel: an
/// apply-to-all picker and one picker per slot. `None` for non-`.mesh` files.
pub fn materials_section(cx: &App, panel: &Entity<AssetViewerPanel>) -> Option<AnyElement> {
    use rust_i18n::t;
    use ui::{h_flex, v_flex, ActiveTheme};

    let materials = panel.read(cx).mesh_materials.as_ref()?;
    let muted = cx.theme().muted_foreground;
    let none = t!("AssetViewer.NoMaterial").to_string();
    let mut column = v_flex()
        .gap_2()
        .child(div().text_xs().text_color(muted).child(t!("AssetViewer.DefaultMaterials").to_string()))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(div().text_xs().child(t!("AssetViewer.ApplyToAllSlots").to_string()))
                .child(picker_button(
                    "mesh-default-material-all".to_owned(),
                    t!("AssetViewer.ApplyToAllSlots").to_string(),
                    materials.apply_all.clone(),
                )),
        );
    for (index, row) in materials.rows.iter().enumerate() {
        column = column.child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(div().text_xs().child(row.name.clone()))
                .child(picker_button(
                    format!("mesh-default-material-{index}"),
                    file_label(&row.material, &none),
                    row.picker.clone(),
                )),
        );
    }
    Some(
        column
            .child(div().text_xs().text_color(muted).child(t!("AssetViewer.MaterialsHint").to_string()))
            .into_any_element(),
    )
}
