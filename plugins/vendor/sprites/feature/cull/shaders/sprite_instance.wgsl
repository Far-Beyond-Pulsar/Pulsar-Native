// A sprite's row (`helio_pass_sprite_cull::SPRITE_INSTANCE_SNIPPET`, marker
// `sprite_instance`): `helio_pass_sprite_batch::SpriteInstance` field for
// field, 80 bytes. The batch's draw shaders, the cull and the simulation
// declare their own bindings of `array<SpriteInstance>` and take the row
// from here; none declares the row.

struct SpriteInstance {
    position: vec2<f32>,    // world-space centre
    size: vec2<f32>,        // width/height before rotation
    rotation: f32,          // radians about the centre
    depth: f32,             // back-to-front sort key (larger draws later)
    _pad_uv: vec2<f32>,     // `uv_rect`'s 16-byte alignment, spelled out
    uv_rect: vec4<f32>,     // [u0, v0, u1, v1] within `atlas_layer`
    color: vec4<f32>,       // straight-alpha RGBA tint
    atlas_layer: u32,       // atlas layer, or the scene's texture slot
}
