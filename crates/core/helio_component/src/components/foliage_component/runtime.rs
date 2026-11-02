//! Foliage reaches the foliage passes through its derived source row
//! (`environment_rows::FoliageSourceRow`), built from the mappings here.

use engine_class_derive::register_world_component;

use super::FoliageComponent;

pub(crate) fn gpu_type(
    component: &FoliageComponent,
) -> helio_pass_foliage_place::components::FoliageTypeComponent {
    use helio_pass_foliage_place::{FoliageKind, pack_kind_and_flags};

    let flags = u32::from(component.rendering.two_sided)
        * helio_pass_foliage_place::FOLIAGE_FLAG_TWO_SIDED
        | u32::from(component.rendering.casts_shadow)
            * helio_pass_foliage_place::FOLIAGE_FLAG_CASTS_SHADOW
        | u32::from(component.interaction.receives_interaction)
            * helio_pass_foliage_place::FOLIAGE_FLAG_RECEIVES_INTERACTION
        | u32::from(component.rendering.ground_colour)
            * helio_pass_foliage_place::FOLIAGE_FLAG_GROUND_COLOUR;
    let kind = match component.general.kind {
        super::FoliageForm::Blade => FoliageKind::Blade,
        super::FoliageForm::Block => FoliageKind::Block,
    };
    // A bit per ground id below 32; 0 grows on any.
    let ground_mask = component
        .placement
        .grows_on
        .0
        .iter()
        .filter(|&&id| id < 32)
        .fold(0u32, |mask, &id| mask | 1 << id);
    let [red, green, blue, _] = component.rendering.base_color;
    let [base_color, roughness_metallic] = helio_pass_foliage_place::FoliageMaterial {
        base_color: [red, green, blue],
        roughness: component.rendering.roughness,
        metallic: component.rendering.metallic,
    }
    .pack();
    let tufts = &component.tufts;
    let mut row = helio_pass_foliage_place::GpuFoliageType {
        density: component.general.density,
        height_range: [
            component.placement.height_min,
            component.placement.height_max,
        ],
        width_range: [component.placement.width_min, component.placement.width_max],
        slope_range: [
            component.placement.slope_max_degrees.to_radians().cos(),
            component.placement.slope_min_degrees.to_radians().cos(),
        ],
        altitude_range: [
            component.placement.altitude_min,
            component.placement.altitude_max,
        ],
        lod_distances: [
            component.rendering.lod_distance_0,
            component.rendering.lod_distance_1,
            component.rendering.lod_distance_2,
            component.rendering.lod_distance_3,
        ],
        wind_response: [
            component.wind.trunk_sway,
            component.wind.branch_flutter,
            component.wind.leaf_jitter,
        ],
        interaction_stiffness: component.wind.interaction_stiffness,
        // The authored colour, roughness and metallic travel in the row itself
        // (`base_color`, `roughness_metallic`). Slot zero stays the default until
        // foliage resolves materials through the material table.
        material_id: 0u32,
        density_layer: component.general.density_layer as u32,
        kind_and_flags: pack_kind_and_flags(kind, flags),
        mesh_or_impostor_id: u32::MAX,
        base_color,
        roughness_metallic,
        ground_mask,
        lean_range: [tufts.lean_center, tufts.lean_rim],
        curve: tufts.curve,
        fall_chance: tufts.fall_chance,
        patch_scale: tufts.patch_scale,
        patch_strength: tufts.patch_strength,
        ..helio_pass_foliage_place::GpuFoliageType::default()
    };
    row.set_blades_per_tuft([tufts.blades_min, tufts.blades_max]);
    row.set_tip_color(tufts.tip_colored.then_some(tufts.tip_color));
    row.into()
}

/// The component's own wind. Its `_pad[0]` is 1 when the component opts
/// out of the level's global wind (`use_global_wind` off): the environment
/// join gives such a component's wind precedence over the global one. The
/// wind clock (`time_prev_time`) is unused: the foliage passes read the
/// renderer's frame clock.
pub(crate) fn wind(component: &FoliageComponent) -> helio_pass_foliage_place::GpuWind {
    let mut wind = helio_pass_foliage_place::Wind {
        direction: glam::Vec3::from_array(component.wind.wind_direction),
        speed: if component.wind.wind_enabled {
            component.wind.wind_speed
        } else {
            0.0
        },
        gust_amplitude: component.wind.gust_amplitude,
        gust_frequency: component.wind.gust_frequency,
        turbulence_scale: component.wind.turbulence_scale,
        ..Default::default()
    }
    .to_gpu();
    wind._pad[0] = if component.wind.use_global_wind {
        0.0
    } else {
        1.0
    };
    wind
}

impl FoliageComponent {
    /// A foliage type row as authored: the component whose type row is
    /// `row` (`gpu_type`), growing everywhere (a row has no layer) in the
    /// level's global wind. A row without an authored material takes the
    /// component's default colour.
    pub fn from_type(row: &helio_pass_foliage_place::GpuFoliageType) -> Self {
        use helio_pass_foliage_place::{
            FOLIAGE_FLAG_CASTS_SHADOW, FOLIAGE_FLAG_GROUND_COLOUR, FOLIAGE_FLAG_RECEIVES_INTERACTION,
            FOLIAGE_FLAG_TWO_SIDED, FoliageKind, unpack_unorm8,
        };
        let mut component = Self::default();
        let flags = row.flags();
        component.general.density = row.density;
        component.general.density_layer = u64::from(row.density_layer);
        component.general.kind = match row.kind() {
            Some(FoliageKind::Block) => super::FoliageForm::Block,
            _ => super::FoliageForm::Blade,
        };
        let placement = &mut component.placement;
        [placement.height_min, placement.height_max] = row.height_range;
        [placement.width_min, placement.width_max] = row.width_range;
        placement.slope_max_degrees = row.slope_range[0].clamp(-1.0, 1.0).acos().to_degrees();
        placement.slope_min_degrees = row.slope_range[1].clamp(-1.0, 1.0).acos().to_degrees();
        [placement.altitude_min, placement.altitude_max] = row.altitude_range;
        placement.has_infinite_extent = true;
        placement.grows_on = crate::VoxelMaterialList((0..32).filter(|id| row.ground_mask >> id & 1 != 0).collect());
        let [blades_min, blades_max] = row.blades_per_tuft();
        component.tufts = super::sub_props::TuftFoliageProps {
            blades_min,
            blades_max,
            lean_center: row.lean_range[0],
            lean_rim: row.lean_range[1],
            curve: row.curve,
            fall_chance: row.fall_chance,
            tip_colored: row.tip_color != 0,
            tip_color: [0, 8, 16].map(|shift| unpack_unorm8((row.tip_color >> shift) as u8)),
            patch_scale: row.patch_scale,
            patch_strength: row.patch_strength,
        };
        let rendering = &mut component.rendering;
        rendering.two_sided = flags & FOLIAGE_FLAG_TWO_SIDED != 0;
        rendering.casts_shadow = flags & FOLIAGE_FLAG_CASTS_SHADOW != 0;
        rendering.ground_colour = flags & FOLIAGE_FLAG_GROUND_COLOUR != 0;
        [
            rendering.lod_distance_0,
            rendering.lod_distance_1,
            rendering.lod_distance_2,
            rendering.lod_distance_3,
        ] = row.lod_distances;
        if let Some(material) = row.material() {
            let [r, g, b] = material.base_color;
            rendering.base_color = [r, g, b, 1.0];
            rendering.roughness = material.roughness;
            rendering.metallic = material.metallic;
        }
        component.interaction.receives_interaction = flags & FOLIAGE_FLAG_RECEIVES_INTERACTION != 0;
        [
            component.wind.trunk_sway,
            component.wind.branch_flutter,
            component.wind.leaf_jitter,
        ] = row.wind_response;
        component.wind.interaction_stiffness = row.interaction_stiffness;
        component.wind.use_global_wind = true;
        component
    }
}

#[register_world_component]
impl FoliageComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helio's voxel meadow is a level's foliage: every row of it, tall
    /// stands included, comes back from its component bit for bit.
    #[test]
    fn the_engine_meadow_is_authored_foliage() {
        let meadow = helio_pass_voxel_planet::meadow::meadow().with_tall_stands();
        for row in &meadow.types {
            let row = helio_pass_foliage_place::GpuFoliageType::from(*row);
            let component = FoliageComponent::from_type(&row);
            let derived: helio_pass_foliage_place::GpuFoliageType = gpu_type(&component).into();
            // The component's rows carry no impostor (the meadow's is unused).
            let derived = helio_pass_foliage_place::GpuFoliageType {
                mesh_or_impostor_id: row.mesh_or_impostor_id,
                ..derived
            };
            assert_eq!(bytemuck::bytes_of(&derived), bytemuck::bytes_of(&row), "{row:?}");
        }
    }
    #[test]
    fn registers_under_its_class_name() {
        assert_eq!(
            pulsar_world_registry::component_id_for_class("FoliageComponent"),
            Some(pulsar_scenedb::component_id::<FoliageComponent>())
        );
    }
}
