//! Per-instance virtual-geometry overrides (plan:
//! `.agents/plans/virtual-geometry.md` section 3 items 2 and 7).
//!
//! A static mesh with **no** `VirtualGeometryComponent` uses the global
//! defaults (virtual geometry on when `VirtualGeometrySettings` allows it,
//! tier `Auto`, error scale 1, casts shadows), so existing scenes work
//! untouched. Attaching the component overrides those per instance.
//!
//! The component is its own component-instance entity linked to its owner
//! object through `ComponentOwner`, like `LODComponent`. Its authored values
//! reach the GPU as one [`VgOverrideGpu`] row in the variable-length buffer
//! `vg_instance_overrides` (handles in `vg_instance_overrides::handles`,
//! keyed by the component's entity row). The virtual-geometry scene
//! derivation ([`crate::virtual_geometry_instances`]) joins it to the
//! owner's static-mesh instances on the GPU.
//!
//! This component declares no `declare_unfinished_component!`: the issue link
//! that declaration requires does not exist for this feature. Until the
//! virtual-geometry passes are wired into the default graph (task T10) its
//! ledger row is `unverified` instead.

use engine_class_derive::{engine_class, register_world_component};
use helio_vg_format::{
    TierLock, INST_ENABLED, INST_FORCE_ROOT_ONLY, INST_SHADOW_CASTER, INST_TIER_SHIFT,
};
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

/// Which raster tier an instance is locked to.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum VgTierLock {
    /// Each cluster picks its tier from its projected triangle size.
    #[default]
    Auto,
    /// Every cluster of the instance is rasterised by the hardware tier.
    HwRaster,
    /// Every cluster of the instance is rasterised by the software
    /// micro-polygon tier.
    SwRaster,
    /// Every cluster of the instance goes through the dense-overlap tier.
    Dense,
}

impl From<VgTierLock> for TierLock {
    fn from(tier: VgTierLock) -> Self {
        match tier {
            VgTierLock::Auto => Self::Auto,
            VgTierLock::HwRaster => Self::HwRaster,
            VgTierLock::SwRaster => Self::SwRaster,
            VgTierLock::Dense => Self::Dense,
        }
    }
}

/// Smallest and largest per-instance multiplier of the screen-space error
/// threshold. Finer than 1/64 or coarser than 64x is never what an author
/// wants, and bounding it keeps a typo from collapsing or exploding the cut.
pub const LOD_ERROR_SCALE_RANGE: (f32, f32) = (1.0 / 64.0, 64.0);

/// Reflected per-instance overrides for virtual geometry. See the module doc.
#[engine_class(
    category = "Rendering",
    clone,
    debug,
    serialize,
    deserialize,
    scene_store
)]
pub struct VirtualGeometryComponent {
    /// Whether this mesh uses virtual geometry at all. A disabled instance
    /// draws through the classic mesh path (and its authored LOD applies).
    #[property]
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Pin the instance to one raster tier instead of choosing per cluster.
    #[property]
    #[serde(default)]
    pub tier_lock: VgTierLock,

    /// Multiplies the screen-space error threshold for this instance: above 1
    /// keeps coarser clusters (cheaper), below 1 keeps finer ones.
    #[property(min = 0.0625, max = 16.0, step = 0.05)]
    #[serde(default = "default_one")]
    pub lod_error_scale: f32,

    /// Draw only the coarsest cut of the cluster hierarchy (useful for
    /// debugging and for very distant fillers).
    #[property]
    #[serde(default)]
    pub force_root_only: bool,

    /// Whether this instance is drawn into shadow maps.
    #[property]
    #[serde(default = "default_true")]
    pub shadow_caster: bool,

    /// The GPU source row derived from the properties above (one element).
    /// Held as a variable-length range so the component follows the same
    /// handle/pool scheme as `LODComponent`.
    #[gpu(buffer = "vg_instance_overrides")]
    #[serde(skip)]
    pub gpu_override: Vec<VgOverrideGpu>,
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

impl Default for VirtualGeometryComponent {
    fn default() -> Self {
        let mut component = Self {
            enabled: true,
            tier_lock: VgTierLock::Auto,
            lod_error_scale: 1.0,
            force_root_only: false,
            shadow_caster: true,
            gpu_override: Vec::new(),
        };
        refresh_gpu_override(&mut component);
        component
    }
}

/// One component's GPU row: `INST_*` flags (enabled, shadow caster, force root
/// only, tier lock in bits 8..10; `INST_LOD_SUPPRESS`/`INST_CONE_CULL` are
/// derived per instance, never authored) and the sanitised error scale.
/// 8 bytes, no padding. Mirrored by `struct Override` in
/// `virtual_geometry_instances.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VgOverrideGpu {
    pub flags: u32,
    pub lod_error_scale: f32,
}

// SAFETY: `repr(C)`, all fields are `Pod`, and the 8-byte layout has no
// implicit padding.
unsafe impl pulsar_scenedb::page::Pod for VgOverrideGpu {}

/// `INST_*` flags the authored values map to.
pub fn vg_override_flags(
    enabled: bool,
    tier_lock: VgTierLock,
    force_root_only: bool,
    shadow_caster: bool,
) -> u32 {
    let mut flags = 0;
    if enabled {
        flags |= INST_ENABLED;
    }
    if shadow_caster {
        flags |= INST_SHADOW_CASTER;
    }
    if force_root_only {
        flags |= INST_FORCE_ROOT_ONLY;
    }
    flags | ((TierLock::from(tier_lock) as u32) << INST_TIER_SHIFT)
}

/// A finite, in-range error scale; anything else is the neutral 1.
pub fn sanitize_lod_error_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(LOD_ERROR_SCALE_RANGE.0, LOD_ERROR_SCALE_RANGE.1)
    } else {
        1.0
    }
}

impl VirtualGeometryComponent {
    /// The GPU row for the current authored values.
    pub fn gpu_row(&self) -> VgOverrideGpu {
        VgOverrideGpu {
            flags: vg_override_flags(
                self.enabled,
                self.tier_lock,
                self.force_root_only,
                self.shadow_caster,
            ),
            lod_error_scale: sanitize_lod_error_scale(self.lod_error_scale),
        }
    }
}

fn refresh_gpu_override(component: &mut VirtualGeometryComponent) {
    component.gpu_override = vec![component.gpu_row()];
}

fn decode_virtual_geometry_component(
    data: &serde_json::Value,
) -> Result<VirtualGeometryComponent, String> {
    let mut component: VirtualGeometryComponent = pulsar_world_registry::decode_json(data)?;
    refresh_gpu_override(&mut component);
    Ok(component)
}

fn virtual_geometry_property_written(
    component: &mut VirtualGeometryComponent,
    property: Option<&str>,
) {
    if matches!(
        property,
        None | Some("enabled" | "tier_lock" | "lod_error_scale" | "force_root_only" | "shadow_caster")
    ) {
        refresh_gpu_override(component);
    }
}

#[register_world_component(
    decode = decode_virtual_geometry_component,
    property_written = virtual_geometry_property_written
)]
impl VirtualGeometryComponent {}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_vg_format::{INST_CONE_CULL, INST_LOD_SUPPRESS};
    use std::mem::size_of;

    #[test]
    fn row_layout_is_pinned() {
        assert_eq!(size_of::<VgOverrideGpu>(), 8);
    }

    #[test]
    fn default_component_is_enabled_auto_and_casts_shadows() {
        let component = VirtualGeometryComponent::default();
        assert!(component.enabled && component.shadow_caster && !component.force_root_only);
        assert_eq!(component.tier_lock, VgTierLock::Auto);
        assert_eq!(component.lod_error_scale, 1.0);
        let row = component.gpu_override[0];
        assert_eq!(row.flags, INST_ENABLED | INST_SHADOW_CASTER);
        assert_eq!(row.lod_error_scale, 1.0);
    }

    #[test]
    fn flags_encode_every_authored_value_and_never_the_derived_bits() {
        let flags = vg_override_flags(true, VgTierLock::SwRaster, true, false);
        assert_eq!(
            flags,
            INST_ENABLED | INST_FORCE_ROOT_ONLY | ((TierLock::SwRaster as u32) << INST_TIER_SHIFT)
        );
        assert_eq!(flags & (INST_LOD_SUPPRESS | INST_CONE_CULL), 0);
        assert_eq!(vg_override_flags(false, VgTierLock::Auto, false, false), 0);
        for tier in [VgTierLock::Auto, VgTierLock::HwRaster, VgTierLock::SwRaster, VgTierLock::Dense] {
            let row = vg_override_flags(true, tier, false, true);
            assert_eq!(TierLock::from_bits(row >> INST_TIER_SHIFT), TierLock::from(tier));
        }
    }

    #[test]
    fn error_scale_is_sanitised() {
        assert_eq!(sanitize_lod_error_scale(f32::NAN), 1.0);
        assert_eq!(sanitize_lod_error_scale(f32::INFINITY), 1.0);
        assert_eq!(sanitize_lod_error_scale(0.0), LOD_ERROR_SCALE_RANGE.0);
        assert_eq!(sanitize_lod_error_scale(1000.0), LOD_ERROR_SCALE_RANGE.1);
        assert_eq!(sanitize_lod_error_scale(2.5), 2.5);
    }

    #[test]
    fn property_writes_refresh_the_gpu_row() {
        let mut component = VirtualGeometryComponent::default();
        component.enabled = false;
        component.lod_error_scale = 4.0;
        virtual_geometry_property_written(&mut component, Some("enabled"));
        assert_eq!(component.gpu_override.len(), 1);
        assert_eq!(component.gpu_override[0].flags & INST_ENABLED, 0);
        assert_eq!(component.gpu_override[0].lod_error_scale, 4.0);
        // An unrelated property does not touch it.
        component.tier_lock = VgTierLock::Dense;
        virtual_geometry_property_written(&mut component, Some("something_else"));
        assert_eq!(component.gpu_override[0].flags >> INST_TIER_SHIFT, 0);
    }

    /// A component saved with fewer (or no) fields, or by a build that knew
    /// fewer, still loads with the defaults for the rest.
    #[test]
    fn serde_defaults_keep_old_and_partial_records_loading() {
        let empty: VirtualGeometryComponent = serde_json::from_str("{}").expect("empty record");
        assert!(empty.enabled && empty.shadow_caster);
        assert_eq!(empty.lod_error_scale, 1.0);
        assert_eq!(empty.tier_lock, VgTierLock::Auto);

        let partial: VirtualGeometryComponent =
            serde_json::from_str(r#"{"enabled": false, "tier_lock": "Dense"}"#).expect("partial");
        assert!(!partial.enabled && partial.shadow_caster);
        assert_eq!(partial.tier_lock, VgTierLock::Dense);

        // The GPU row is derived, never persisted.
        let json = serde_json::to_value(VirtualGeometryComponent::default()).unwrap();
        assert!(json.get("gpu_override").is_none());
    }
}
