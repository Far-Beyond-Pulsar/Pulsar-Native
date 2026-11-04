//! GI's settings: what a renderer config carries for it
//! (`RendererConfig::features`) and the node builds read, and their host
//! description ([`GI`], as `helio_settings` describes the renderer's):
//! engines build their settings UI from it and read the values back
//! through it.

use helio_settings::{number, Group, Resolved};

use crate::{mesh_rays_supported, GiConfig};

/// GI's settings (a renderer config's `features`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GiSettings {
    /// The finest probes' spacing (metres), or `None` for the default:
    /// open ground's ([`crate::DEFAULT_FINEST_SPACING`]) with a terrain
    /// tracer, half a metre (rooms, pillars, furniture) without one.
    pub probe_spacing: Option<f32>,
}

impl GiSettings {
    /// The probes' configuration these settings ask for, with a terrain
    /// tracing them every frame or not ([`helio_gi_api::TerrainTracer`]).
    pub fn config(&self, terrain_traced: bool) -> GiConfig {
        let default = if terrain_traced {
            crate::DEFAULT_FINEST_SPACING
        } else {
            0.5
        };
        GiConfig {
            finest_spacing: self.probe_spacing.map_or(default, f64::from),
            needs_tlas: !terrain_traced,
        }
    }

    /// Whether there is GI on `device`: a terrain traces it, or the meshes
    /// can (hardware ray queries).
    pub fn active(&self, device: &wgpu::Device, terrain_traced: bool) -> bool {
        terrain_traced || mesh_rays_supported(device)
    }
}

/// GI's host settings: a host lists it with its other settings groups.
pub static GI: Group = Group {
    key: "gi",
    title: "Global Illumination",
    description: "World-space diffuse GI: the probe clipmap",
    settings: &[number(
        "probe_spacing",
        "GI Probe Spacing",
        "Global Illumination",
        (0.0, 8.0, 0.25),
        0.0,
        "Metres between the finest GI probes (each coarser level doubles it); 0: the renderer's choice \
         (1 m over voxel terrain, 0.5 m in mesh scenes)",
    )],
};

impl GiSettings {
    /// The settings `settings` (of [`GI`]) hold.
    pub fn from_settings(settings: &Resolved) -> Self {
        debug_assert!(std::ptr::eq(settings.group(), &GI));
        Self {
            probe_spacing: settings.optional("probe_spacing").map(|m| m as f32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_defaults_are_the_default_settings() {
        assert_eq!(GiSettings::from_settings(&GI.defaults()), GiSettings::default());
    }

    #[test]
    fn a_spacing_maps() {
        let settings = GI.resolve(|key| {
            (key == "probe_spacing").then_some(helio_settings::Value::Number(2.0))
        });
        assert_eq!(GiSettings::from_settings(&settings).probe_spacing, Some(2.0));
    }
}
