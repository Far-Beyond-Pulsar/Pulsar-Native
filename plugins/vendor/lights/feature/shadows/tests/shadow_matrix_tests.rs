use helio_light_api::{
    GpuShadowMatrix, ResidencyTable, ShadowResident, MAX_SHADOW_CASTERS, MAX_SHADOW_FACES,
};
#[test]
fn packed_shadow_abi_and_bounded_pool() {
    assert_eq!(std::mem::size_of::<GpuShadowMatrix>(), 96);
    assert_eq!(std::mem::size_of::<ShadowResident>(), 128);
    assert_eq!(
        std::mem::size_of::<ResidencyTable>(),
        16 + 128 * MAX_SHADOW_CASTERS
    );
    assert_eq!(MAX_SHADOW_FACES, 6 * MAX_SHADOW_CASTERS);
    assert!(MAX_SHADOW_CASTERS > 42);
}
