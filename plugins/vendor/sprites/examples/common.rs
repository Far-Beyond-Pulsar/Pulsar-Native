//! What the sprites plugin's examples share: the part of Helio's
//! `v3_demo_common` they use, the plugin's rows as standalone hosts
//! author them, and the preset with the plugin's features.
#![allow(dead_code)]

// ── SceneDB-owned scene content ─────────────────────────────────────────────
//
// A demo's placed objects and lights are authored as *pass-owned* SceneDB
// components — `helio_mesh_host::StaticObjectComponent` and
// `helio_components::LightComponent` — not as example-local structs or
// entries in a Helio-owned `Vec`/arena. Each is `#[derive(SceneStore)]` +
// `#[gpu(layout = packed, ...)]`, exactly like the already-shipped
// `CoronaEmitterComponent`: the pass that consumes a kind
// of scene content owns its schema and its generated GPU buffer, and a World
// row is the only authoritative record of "this exists."
//
// The renderer consumes the SceneDB GPU projection directly. Asset handles are
// resolved when the component is authored, while existence and transforms stay
// exclusively in the World row.

// ── Participating media and light shafts (component API) ─────────────────────
//
// Fog is world data: global and local media are SceneDB components owned by
// the volumetric fog pass, in physical units (one world unit = one metre,
// extinction in m^-1). Shafts are not a separate effect: they are the shadowed
// part of lit medium, so a light only forms them when it (a) participates in
// the medium and (b) has a shadow map to be occluded by.

// ── Headless capture (`--probe` modes) ──────────────────────────────────────

// ── The sprites plugin ─────────────────────────────────────────────────────

/// The deferred preset with the plugin's features (Helio's presets do not
/// list them: the sprites are a Pulsar plugin's).
pub fn preset() -> helio_presets::PresetGraph {
    helio_mesh_host::presets::with(
        helio_presets::Preset::Deferred,
        &helio_feature_api::FeatureSet::new()
            .with(helio_pass_sprite_batch::SpriteFeature)
            // The lighting plugins' (Helio's presets list neither).
            .with(helio_pass_sky::SkyFeature)
            .with(helio_pass_gi::GiFeature),
    )
}

// The plugin's rows as standalone hosts author them (contract rows, as
// `helio_components` does for Helio's own kinds).
use pulsar_scenedb_derive::SceneStore;

/// Ties a component to its contract row: same size, every field at the same
/// offset, stored under the contract's key. Adds the conversions. (As
/// `helio_components::same_layout!`, for rows Helio no longer has.)
macro_rules! same_layout {
    ($component:ident, $row:path, $contract:path, $key:literal; $($field:ident),* $(,)?) => {
        const _: () = {
            assert!(::core::mem::size_of::<$component>() == ::core::mem::size_of::<$row>());
            assert!(::core::mem::align_of::<$component>() == ::core::mem::align_of::<$row>());
            $(
                assert!(
                    ::core::mem::offset_of!($component, $field)
                        == ::core::mem::offset_of!($row, $field)
                );
            )*
            $contract.assert_producer::<$component>($key);
        };

        impl ::core::convert::From<$row> for $component {
            fn from(row: $row) -> Self {
                ::bytemuck::cast(row)
            }
        }

        impl ::core::convert::From<$component> for $row {
            fn from(component: $component) -> Self {
                ::bytemuck::cast(component)
            }
        }

        impl $component {
            /// This component as its contract row.
            pub fn row(&self) -> $row {
                ::bytemuck::cast(*self)
            }
        }

        impl ::core::ops::Deref for $component {
            type Target = $row;
            fn deref(&self) -> &$row {
                ::bytemuck::cast_ref(self)
            }
        }

        impl ::core::ops::DerefMut for $component {
            fn deref_mut(&mut self) -> &mut $row {
                ::bytemuck::cast_mut(self)
            }
        }
    };
}

/// SceneDB-owned persistent sprite row. The packed ABI intentionally matches
/// [`SpriteInstance`] and `shaders/sprite.wgsl`; cull/sort/indirect buffers
/// remain renderer-derived state.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, SceneStore, bytemuck::Pod, bytemuck::Zeroable)]
#[gpu(layout = packed, buffer = "sprite_instances")]
pub struct SpriteComponent {
    #[gpu]
    pub position_x: f32,
    #[gpu]
    pub position_y: f32,
    #[gpu]
    pub size_x: f32,
    #[gpu]
    pub size_y: f32,
    #[gpu]
    pub rotation: f32,
    #[gpu]
    pub depth: f32,
    #[gpu]
    pub _pad_uv_x: f32,
    #[gpu]
    pub _pad_uv_y: f32,
    #[gpu]
    pub uv_rect: [f32; 4],
    #[gpu]
    pub color: [f32; 4],
    #[gpu]
    pub atlas_layer: u32,
    #[gpu]
    pub _pad_tail_0: u32,
    #[gpu]
    pub _pad_tail_1: u32,
    #[gpu]
    pub _pad_tail_2: u32,
}
same_layout!(SpriteComponent, helio_pass_sprite_batch::SpriteRow, helio_pass_sprite_batch::SPRITE_INSTANCES, "sprite_instances"; position_x, position_y, size_x, size_y, rotation, depth, _pad_uv_x, _pad_uv_y, uv_rect, color, atlas_layer, _pad_tail_0, _pad_tail_1, _pad_tail_2);

impl From<helio_pass_sprite_batch::SpriteInstance> for SpriteComponent {
    fn from(value: helio_pass_sprite_batch::SpriteInstance) -> Self {
        helio_pass_sprite_batch::SpriteRow::from(value).into()
    }
}

impl From<SpriteComponent> for helio_pass_sprite_batch::SpriteInstance {
    fn from(value: SpriteComponent) -> Self {
        helio_pass_sprite_batch::SpriteRow::from(value).into()
    }
}
