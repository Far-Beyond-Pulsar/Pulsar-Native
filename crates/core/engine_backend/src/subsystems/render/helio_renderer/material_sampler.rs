//! The sampler every scene material texture is read through.
//!
//! The renderer shares one sampler across material texture slots. Left alone
//! it is wgpu's default: nearest filtering and clamp-to-edge, which makes mips
//! pick a level by screen footprint but then read it with point sampling, and
//! stops a texture tiling past `[0, 1]`. The engine sets a better default
//! here, and projects can change it:
//!
//! | project setting (`streaming`) | default | meaning |
//! |---|---|---|
//! | `texture_filter` | `"trilinear"` | `"trilinear"`, `"bilinear"` (no blend between mips) or `"nearest"` (pixel art) |
//! | `texture_anisotropy` | `8` | 1 to 16; only applies to `"trilinear"` |
//! | `texture_wrap` | `"repeat"` | `"repeat"` or `"clamp"` |

/// How texture coordinates outside `[0, 1]` are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    Repeat,
    Clamp,
}

/// How texels, and the mip levels between them, are filtered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Trilinear,
    Bilinear,
    Nearest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerSettings {
    pub filter: Filter,
    /// Requested anisotropy, 1 to 16. Honoured only for trilinear filtering.
    pub anisotropy: u16,
    pub wrap: Wrap,
}

impl Default for SamplerSettings {
    fn default() -> Self {
        Self {
            filter: Filter::Trilinear,
            anisotropy: 8,
            wrap: Wrap::Repeat,
        }
    }
}

impl SamplerSettings {
    /// Read the project's `streaming.texture_*` settings, falling back to the
    /// defaults for anything missing or unrecognised.
    pub fn from_project() -> Self {
        let get = |key: &str| {
            engine_state::settings::global_config()
                .get(engine_state::settings::NS_PROJECT, "streaming", key)
                .ok()
        };
        let text = |key: &str| match get(key) {
            Some(engine_state::settings::ConfigValue::String(value)) => Some(value),
            _ => None,
        };
        let anisotropy = match get("texture_anisotropy") {
            Some(engine_state::settings::ConfigValue::Int(value)) => Some(value),
            _ => None,
        };
        Self::from_values(
            text("texture_filter").as_deref(),
            anisotropy,
            text("texture_wrap").as_deref(),
        )
    }

    /// The settings the given raw values mean.
    pub fn from_values(filter: Option<&str>, anisotropy: Option<i64>, wrap: Option<&str>) -> Self {
        let defaults = Self::default();
        Self {
            filter: match filter.map(str::to_ascii_lowercase).as_deref() {
                Some("nearest") | Some("point") => Filter::Nearest,
                Some("bilinear") | Some("linear") => Filter::Bilinear,
                Some("trilinear") => Filter::Trilinear,
                _ => defaults.filter,
            },
            anisotropy: anisotropy.map_or(defaults.anisotropy, |n| n.clamp(1, 16) as u16),
            wrap: match wrap.map(str::to_ascii_lowercase).as_deref() {
                Some("clamp") => Wrap::Clamp,
                Some("repeat") => Wrap::Repeat,
                _ => defaults.wrap,
            },
        }
    }

    /// The wgpu sampler these settings describe.
    ///
    /// wgpu rejects an anisotropy above 1 unless the magnification, minification
    /// and mipmap filters are all linear, so anisotropy is applied only for
    /// trilinear filtering.
    pub fn descriptor(&self) -> wgpu::SamplerDescriptor<'static> {
        let (texel, mip, anisotropy) = match self.filter {
            Filter::Trilinear => (
                wgpu::FilterMode::Linear,
                wgpu::MipmapFilterMode::Linear,
                self.anisotropy.max(1),
            ),
            Filter::Bilinear => (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Nearest, 1),
            Filter::Nearest => (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest, 1),
        };
        let address = match self.wrap {
            Wrap::Repeat => wgpu::AddressMode::Repeat,
            Wrap::Clamp => wgpu::AddressMode::ClampToEdge,
        };
        wgpu::SamplerDescriptor {
            label: Some("Scene Material Sampler"),
            address_mode_u: address,
            address_mode_v: address,
            address_mode_w: address,
            mag_filter: texel,
            min_filter: texel,
            mipmap_filter: mip,
            anisotropy_clamp: anisotropy,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_trilinear_anisotropic_and_repeating() {
        let descriptor = SamplerSettings::default().descriptor();
        assert_eq!(descriptor.mag_filter, wgpu::FilterMode::Linear);
        assert_eq!(descriptor.min_filter, wgpu::FilterMode::Linear);
        assert_eq!(descriptor.mipmap_filter, wgpu::MipmapFilterMode::Linear);
        assert_eq!(descriptor.anisotropy_clamp, 8);
        assert_eq!(descriptor.address_mode_u, wgpu::AddressMode::Repeat);
        assert_eq!(descriptor.address_mode_v, wgpu::AddressMode::Repeat);
    }

    #[test]
    fn settings_are_read_case_insensitively_and_unknown_values_fall_back() {
        let nearest = SamplerSettings::from_values(Some("NEAREST"), Some(4), Some("Clamp"));
        assert_eq!(nearest.filter, Filter::Nearest);
        assert_eq!(nearest.wrap, Wrap::Clamp);
        assert_eq!(
            SamplerSettings::from_values(Some("sparkly"), None, Some("mirror")),
            SamplerSettings::default(),
            "unknown words mean the defaults"
        );
        assert_eq!(SamplerSettings::from_values(None, Some(0), None).anisotropy, 1);
        assert_eq!(SamplerSettings::from_values(None, Some(99), None).anisotropy, 16);
    }

    #[test]
    fn anisotropy_only_goes_with_filters_wgpu_accepts_it_for() {
        for filter in [Filter::Bilinear, Filter::Nearest] {
            let descriptor = SamplerSettings { filter, anisotropy: 16, wrap: Wrap::Repeat }.descriptor();
            assert_eq!(descriptor.anisotropy_clamp, 1, "{filter:?} cannot be anisotropic");
        }
        let nearest = SamplerSettings { filter: Filter::Nearest, ..Default::default() }.descriptor();
        assert_eq!(nearest.mag_filter, wgpu::FilterMode::Nearest);
        let bilinear = SamplerSettings { filter: Filter::Bilinear, ..Default::default() }.descriptor();
        assert_eq!(bilinear.mipmap_filter, wgpu::MipmapFilterMode::Nearest, "no blend between mips");
    }

    /// Every combination must be a sampler a real device accepts: wgpu panics
    /// on an invalid descriptor, and a default that does that would stop the
    /// editor at startup.
    #[test]
    fn every_combination_is_a_valid_sampler_on_a_real_device() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let Ok((device, _queue)) = pollster::block_on(adapter.request_device(&Default::default())) else {
            eprintln!("no GPU device; skipping");
            return;
        };
        for filter in [Filter::Trilinear, Filter::Bilinear, Filter::Nearest] {
            for wrap in [Wrap::Repeat, Wrap::Clamp] {
                for anisotropy in [1, 2, 8, 16] {
                    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                    let _sampler = device.create_sampler(&SamplerSettings { filter, anisotropy, wrap }.descriptor());
                    let error = pollster::block_on(scope.pop());
                    assert!(error.is_none(), "{filter:?}/{wrap:?}/{anisotropy}: {error:?}");
                }
            }
        }
    }
}
