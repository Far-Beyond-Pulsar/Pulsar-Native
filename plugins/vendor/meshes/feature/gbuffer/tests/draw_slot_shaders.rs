//! Every drawer's WGSL reads its draw slot at `helio_mesh_api::DRAW_SLOT_LOCATION`.

mod tests {
    /// Every GPU-driven vertex shader takes its slot from the vertex buffer,
    /// not `@builtin(instance_index)`, which ignores `first_instance` in
    /// DX12 indirect draws. Drawers outside this repo check their own
    /// shaders against [`helio_mesh_api::draw_slots::DRAW_SLOT_LOCATION`].
    #[test]
    fn gpu_driven_vertex_shaders_read_the_slot_attribute() {
        let sources = [
            ("gbuffer", helio_mats::templates::DEFAULT_PBR),
            (
                "forward lit",
                include_str!("../../forward/shaders/forward_lit.wgsl"),
            ),
            (
                "shadow",
                include_str!("../../shadow/shaders/shadow.wgsl"),
            ),
            (
                "shadow transmittance",
                include_str!("../../shadow/shaders/shadow_transmittance.wgsl"),
            ),
            (
                "transparent",
                helio_mats::templates::TRANSPARENT_BASE,
            ),
        ];
        let attribute = format!("@location({})", helio_mesh_api::draw_slots::DRAW_SLOT_LOCATION);
        for (name, source) in sources {
            let code: String = source
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("builtin(instance_index)"),
                "{name}: reads @builtin(instance_index)"
            );
            assert!(
                code.contains(&attribute),
                "{name}: has no {attribute} slot input"
            );
        }
    }
}
