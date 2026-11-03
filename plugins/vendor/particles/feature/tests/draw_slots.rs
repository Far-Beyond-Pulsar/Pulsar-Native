//! The draw's vertex shader takes its slot from the vertex buffer, not
//! `@builtin(instance_index)`, which ignores `first_instance` in DX12
//! indirect draws (as the G-buffer's drawers do,
//! `helio_pass_gbuffer::DRAW_SLOT_LOCATION`).

#[test]
fn the_vertex_shader_reads_the_slot_attribute() {
    let code: String = include_str!("../shaders/corona_render.wgsl")
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("builtin(instance_index)"),
        "corona: reads @builtin(instance_index)"
    );
    let attribute = format!("@location({})", helio_pass_gbuffer::DRAW_SLOT_LOCATION);
    assert!(code.contains(&attribute), "corona: has no {attribute} slot input");
}
