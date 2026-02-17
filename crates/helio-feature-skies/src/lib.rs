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
            // VERY early in FragmentMain - check if sky and return immediately
            ShaderInjection::with_priority(
                ShaderInjectionPoint::FragmentMain,
                r#"    // Helio Skies: Detect sky sphere and return emissive color immediately
    let distance_to_camera = length(input.world_position - camera.position);
    if (distance_to_camera > 400.0) {
        let view_dir = normalize(input.world_position - camera.position);
        return vec4<f32>(calculate_sky_color(view_dir), 1.0);
    }"#,
                1000, // VERY high priority - execute first before anything else
            ),
        ]
    }
}
