//! Helio's voxel meadow is a level's foliage: every row of it, tall stands
//! included, comes back from its component bit for bit.

use helio_component::FoliageComponent;

#[test]
fn the_engine_meadow_is_authored_foliage() {
    let meadow = helio_pass_voxel_planet::meadow::meadow().with_tall_stands();
    for row in &meadow.types {
        let row = helio_pass_foliage_place::GpuFoliageType::from(*row);
        let component = FoliageComponent::from_type(&row);
        let derived: helio_pass_foliage_place::GpuFoliageType = component.type_row().into();
        // The component's rows carry no impostor (the meadow's is unused).
        let derived = helio_pass_foliage_place::GpuFoliageType {
            mesh_or_impostor_id: row.mesh_or_impostor_id,
            ..derived
        };
        assert_eq!(
            bytemuck::bytes_of(&derived),
            bytemuck::bytes_of(&row),
            "{row:?}"
        );
    }
}
