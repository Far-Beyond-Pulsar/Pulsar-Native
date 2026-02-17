//! Helio Skies - Volumetric Atmosphere Feature
//! 
//! Provides high-quality volumetric atmosphere rendering with configurable components:
//! - Sky dome with atmospheric scattering
//! - Volumetric clouds
//! - Volumetric fog
//! - Aerial perspective
//!
//! All components are toggleable and support multiple quality levels.

use blade_graphics as gpu;
use helio_features::{Feature, FeatureContext, ShaderInjection, ShaderInjectionPoint};
use glam::Vec3;
use std::sync::Arc;

/// Quality preset for atmosphere rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    Low,     // 16 cloud samples, 4 scatter samples
    Medium,  // 32 cloud samples, 8 scatter samples
    High,    // 64 cloud samples, 16 scatter samples
    Ultra,   // 128 cloud samples, 32 scatter samples
}

impl QualityLevel {
    pub fn cloud_samples(&self) -> u32 {
        match self {
            QualityLevel::Low => 16,
            QualityLevel::Medium => 32,
            QualityLevel::High => 64,
            QualityLevel::Ultra => 128,
        }
    }
    
    pub fn scatter_samples(&self) -> u32 {
        match self {
            QualityLevel::Low => 4,
            QualityLevel::Medium => 8,
            QualityLevel::High => 16,
            QualityLevel::Ultra => 32,
        }
    }
}

impl Default for QualityLevel {
    fn default() -> Self {
        QualityLevel::Medium
    }
}

/// Component flags for enabling/disabling features
#[derive(Debug, Clone, Copy)]
pub struct ComponentFlags {
    pub sky: bool,
    pub clouds: bool,
    pub fog: bool,
    pub aerial_perspective: bool,
}

impl Default for ComponentFlags {
    fn default() -> Self {
        Self {
            sky: true,
            clouds: false,
            fog: false,
            aerial_perspective: true,
        }
    }
}

/// Atmospheric scattering parameters (physically-based)
#[derive(Debug, Clone, Copy)]
pub struct AtmosphereParameters {
    /// Planet radius in km (Earth = 6371.0)
    pub planet_radius: f32,
    /// Atmosphere thickness in km (Earth = 60.0)
    pub atmosphere_thickness: f32,
    /// Rayleigh scattering coefficient (RGB wavelength-dependent)
    pub rayleigh_coefficient: Vec3,
    /// Mie scattering coefficient
    pub mie_coefficient: f32,
    /// Sun intensity multiplier
    pub sun_intensity: f32,
}

impl Default for AtmosphereParameters {
    fn default() -> Self {
        Self {
            planet_radius: 6371.0,
            atmosphere_thickness: 60.0,
            rayleigh_coefficient: Vec3::new(5.8e-6, 13.5e-6, 33.1e-6),
            mie_coefficient: 21e-6,
            sun_intensity: 20.0,
        }
    }
}

/// Cloud parameters
#[derive(Debug, Clone, Copy)]
pub struct CloudParameters {
    pub base_altitude: f32,
    pub thickness: f32,
    pub coverage: f32,
    pub density: f32,
}

impl Default for CloudParameters {
    fn default() -> Self {
        Self {
            base_altitude: 1.5,
            thickness: 2.0,
            coverage: 0.5,
            density: 1.0,
        }
    }
}

/// Fog parameters
#[derive(Debug, Clone, Copy)]
pub struct FogParameters {
    pub color: Vec3,
    pub density: f32,
    pub height_falloff: f32,
    pub max_distance: f32,
}

impl Default for FogParameters {
    fn default() -> Self {
        Self {
            color: Vec3::new(0.7, 0.8, 0.9),
            density: 0.001,
            height_falloff: 0.2,
            max_distance: 1000.0,
        }
    }
}

/// Helio Skies feature for volumetric atmosphere rendering
pub struct HelioSkies {
    enabled: bool,
    quality: QualityLevel,
    components: ComponentFlags,
    atmosphere: AtmosphereParameters,
    clouds: CloudParameters,
    fog: FogParameters,
    sun_direction: Vec3,
    
    // GPU resources
    context: Option<Arc<gpu::Context>>,
}

impl HelioSkies {
    pub fn new() -> Self {
        Self {
            enabled: true,
            quality: QualityLevel::default(),
            components: ComponentFlags::default(),
            atmosphere: AtmosphereParameters::default(),
            clouds: CloudParameters::default(),
            fog: FogParameters::default(),
            sun_direction: Vec3::new(0.3, 0.7, 0.5).normalize(),
            context: None,
        }
    }
    
    /// Set quality level
    pub fn with_quality(mut self, quality: QualityLevel) -> Self {
        self.quality = quality;
        self
    }
    
    /// Enable/disable sky dome
    pub fn set_sky_enabled(&mut self, enabled: bool) {
        self.components.sky = enabled;
    }
    
    /// Enable/disable volumetric clouds
    pub fn set_clouds_enabled(&mut self, enabled: bool) {
        self.components.clouds = enabled;
    }
    
    /// Enable/disable volumetric fog
    pub fn set_fog_enabled(&mut self, enabled: bool) {
        self.components.fog = enabled;
    }
    
    /// Set sun direction (will be normalized)
    pub fn set_sun_direction(&mut self, direction: Vec3) {
        self.sun_direction = direction.normalize();
    }
    
    /// Get atmosphere parameters
    pub fn atmosphere(&self) -> &AtmosphereParameters {
        &self.atmosphere
    }
    
    /// Get mutable atmosphere parameters
    pub fn atmosphere_mut(&mut self) -> &mut AtmosphereParameters {
        &mut self.atmosphere
    }
}

impl Default for HelioSkies {
    fn default() -> Self {
        Self::new()
    }
}

impl Feature for HelioSkies {
    fn name(&self) -> &str {
        "helio_skies"
    }
    
    fn init(&mut self, context: &FeatureContext) {
        log::info!("Initializing Helio Skies atmosphere feature");
        log::info!("  Quality: {:?}", self.quality);
        log::info!("  Sky enabled: {}", self.components.sky);
        log::info!("  Clouds enabled: {}", self.components.clouds);
        log::info!("  Fog enabled: {}", self.components.fog);
        
        self.context = Some(context.gpu.clone());
        
        // TODO: Initialize GPU resources (textures, buffers) here
        
        log::info!("Helio Skies initialized successfully");
    }
    
    fn is_enabled(&self) -> bool {
        self.enabled
    }
    
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    
    fn shader_injections(&self) -> Vec<ShaderInjection> {
        let mut injections = Vec::new();
        
        if !self.enabled {
            return injections;
        }
        
        // Add atmosphere shader functions to fragment preamble
        injections.push(ShaderInjection::with_priority(
            ShaderInjectionPoint::FragmentPreamble,
            include_str!("../shaders/atmosphere_functions.wgsl"),
            10,
        ));
        
        // Apply sky dome rendering if enabled
        if self.components.sky {
            injections.push(ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentPostProcess,
                "    // Helio Skies: Blend sky color based on view direction\n    final_color = apply_sky_color(final_color, world_position, camera_position);",
                5,
            ));
        }
        
        // Apply aerial perspective if enabled
        if self.components.aerial_perspective {
            injections.push(ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentPostProcess,
                "    // Helio Skies: Apply aerial perspective\n    final_color = apply_aerial_perspective(final_color, world_position, camera_position);",
                15,
            ));
        }
        
        // Apply volumetric fog if enabled
        if self.components.fog {
            injections.push(ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentPostProcess,
                "    // Helio Skies: Apply volumetric fog\n    final_color = apply_volumetric_fog(final_color, world_position, camera_position);",
                18,
            ));
        }
        
        injections
    }
}
