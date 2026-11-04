use std::mem::{align_of, size_of};

use helio_pass_billboard::{BillboardInstance, BillboardRow, BILLBOARD_INSTANCES};

#[test]
fn billboard_component_matches_the_shader_instance_record() {
    assert_eq!(size_of::<BillboardRow>(), 48);
    assert_eq!(align_of::<BillboardRow>(), 4);
    assert_eq!(size_of::<BillboardInstance>(), size_of::<BillboardRow>());
    assert_eq!(size_of::<BillboardRow>() % 16, 0);
}

#[test]
fn contract_names_the_shader_buffer() {
    assert_eq!(BILLBOARD_INSTANCES.name(), "billboard_instances");
}
