//! Helio Skies - Volumetric Atmosphere Feature
//! 
//! Provides volumetric sky rendering via shader injection.
//! The sky color is applied based on distance - distant geometry (sky sphere) gets full sky color.

use helio_features::{Feature, FeatureContext, ShaderInjection, ShaderInjectionPoint};

pub struct HelioSkies {
    enabled: bool,
}

impl HelioSkies {
    pub fn new() -> Self {
        Self {
            enabled: true,
        }
    }
    
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

impl Feature for HelioSkies {
    fn name(&self) -> &str {
        "helio_skies"
    }
    
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    
    fn init(&mut self, _context: &FeatureContext) {
        // No GPU resources needed - pure shader injection
    }
    
    fn shader_injections(&self) -> Vec<ShaderInjection> {
        if !self.enabled {
            return Vec::new();
        }
        
        vec![
            // Add atmosphere functions to fragment preamble
            ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentPreamble,
                include_str!("../shaders/sky_atmosphere.wgsl"),
                10,
            ),
            // Apply sky rendering in post-process
            ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentPostProcess,
                "    final_color = apply_volumetric_sky(final_color, world_position, camera_position);",
                5,
            ),
        ]
    }
}
