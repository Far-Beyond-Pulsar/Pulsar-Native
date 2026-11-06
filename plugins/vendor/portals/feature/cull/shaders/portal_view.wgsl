// One active portal's render data (`helio_pass_portal_cull::
// PORTAL_VIEW_SNIPPET`, marker `portal_view`): `GpuPortalView` field for
// field, 144 bytes. The cull and the duplicate draw, its mask and its editor
// overlay declare their own bindings of `array<GpuPortalView>` and take the
// row from here; none declares the row.

struct GpuPortalView {
    transform:         mat4x4<f32>,  // portal-local -> world (the surface's own pose)
    inverse_transform: mat4x4<f32>,  // world -> portal-local (the clip test)
    half_extent:       vec2<f32>,    // the opening's half-extent in local X/Y
    coordinate_space:  u32,          // index into `coordinate_spaces[]`
    _pad:              u32,
}
