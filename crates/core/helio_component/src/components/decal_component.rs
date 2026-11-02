//! A decal: an image or a tint projected onto the surfaces inside a box
//! (Pulsar-Native #1058).
//!
//! The box is centred on the owner and oriented by it; `size` is its full
//! extent in the owner's space before the owner's scale. The decal projects
//! along the box's local Z: local X and Y span the image, local Z its depth.
//!
//! An authored decal is permanent: its row's fade time is zero, so it never
//! disappears on its own. How it disappears is up to scripts, through its
//! opacity: the `opacity` property (`DecalComponent::get_opacity` /
//! `set_opacity`) and the `fade_by` method (`DecalComponent::fade_by(decal,
//! amount)`, which lowers it and returns what is left), e.g. a little every
//! tick.
//!
//! SceneDB derives a `DecalSourceRow` (`environment_rows`) from the authored
//! value; the environment join places it as the decal pass's
//! `helio_pass_decal::DecalComponent` row in `"decals"`, its world-to-decal
//! transform built from the owner's transform on the GPU.

use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

/// The class name scripts and level files use.
pub const DECAL_CLASS_NAME: &str = "DecalComponent";

/// How a decal combines with the surface under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum DecalBlend {
    /// Over the surface, by its opacity.
    AlphaBlend,
    /// Added to the surface.
    Additive,
    /// Multiplied with the surface (darkens: dirt, burns).
    Multiply,
}

impl Default for DecalBlend {
    fn default() -> Self {
        Self::AlphaBlend
    }
}

/// What a decal changes on the surface under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum DecalLayers {
    /// The surface colour (and normal, from a normal map).
    Albedo,
    /// Emitted light only: the decal glows in its tint.
    Emissive,
    /// Colour, emission and roughness.
    All,
}

impl Default for DecalLayers {
    fn default() -> Self {
        Self::Albedo
    }
}

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Decal", category_color = "#C7853B")]
#[serde(default)]
pub struct DecalComponent {
    #[property(category = "Decal")]
    pub enabled: bool,
    /// Full extent of the projection box in the owner's space: X and Y span
    /// the image, Z is how deep it reaches.
    #[property(min = 0.0, max = 100000.0, step = 0.1, category = "Decal")]
    pub size: [f32; 3],
    /// Tint, multiplied with the image (or the colour without one).
    #[property(category = "Decal")]
    pub color: [f32; 3],
    /// 0 invisible, 1 fully applied. Scripts fade a decal out with it.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Decal")]
    pub opacity: f32,
    /// Image to project (project-relative path); its alpha is the decal's
    /// shape. Empty: the tint alone, over the whole box.
    #[property(category = "Decal")]
    pub albedo_texture: String,
    #[property(category = "Decal")]
    pub blend: DecalBlend,
    #[property(category = "Decal")]
    pub layers: DecalLayers,
    /// The image `albedo_texture` names, as the world loaded it (a texture
    /// asset, [`crate::texture_assets`]). Never authored.
    #[serde(skip)]
    pub albedo_image: crate::texture_assets::BoundTexture,
}

impl Default for DecalComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            size: [1.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0],
            opacity: 1.0,
            albedo_texture: String::new(),
            blend: DecalBlend::AlphaBlend,
            layers: DecalLayers::Albedo,
            albedo_image: Default::default(),
        }
    }
}

impl DecalComponent {
    /// The decal pass row in the owner's space, with its albedo texture at
    /// `albedo_slot` (`u32::MAX`: none): every field but the transform,
    /// which the environment join builds from the owner. Permanent: its fade
    /// time is zero. Opacity is the colour's alpha, zero while disabled.
    pub fn to_row(&self, albedo_slot: u32) -> helio_pass_decal::DecalComponent {
        let opacity = if self.enabled {
            self.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        helio_pass_decal::DecalComponent {
            transform: [0.0; 16],
            color: [self.color[0], self.color[1], self.color[2], opacity],
            albedo_texture_index: albedo_slot,
            normal_texture_index: u32::MAX,
            roughness_texture_index: u32::MAX,
            metalness_texture_index: u32::MAX,
            blend_mode: match self.blend {
                DecalBlend::AlphaBlend => helio_pass_decal::DecalBlendMode::AlphaBlend,
                DecalBlend::Additive => helio_pass_decal::DecalBlendMode::Additive,
                DecalBlend::Multiply => helio_pass_decal::DecalBlendMode::Multiply,
            } as u32,
            decal_type: match self.layers {
                DecalLayers::Albedo => helio_pass_decal::DecalType::AlbedoNormal,
                DecalLayers::Emissive => helio_pass_decal::DecalType::Emissive,
                DecalLayers::All => helio_pass_decal::DecalType::All,
            } as u32,
            fade_time: 0.0,
            fade_start_delay: 0.0,
            age: 0.0,
            normal_adapt: 1,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }

    /// Lower the opacity by `amount` (clamped to 0..1) and return what is
    /// left: a script fading a decal out calls it every tick.
    pub fn fade_by(&mut self, amount: f32) -> f32 {
        self.opacity = (self.opacity - amount).clamp(0.0, 1.0);
        self.opacity
    }
}

#[register_world_component]
impl DecalComponent {}

/// The world counts the image `albedo_texture` names as a reference of the
/// decal, whatever wrote the value, and binds it when it loads.
fn collect_decal_handles(value: *const (), out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>) {
    // SAFETY: SceneDB reaches this only through `DecalComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let decal = unsafe { &*(value as *const DecalComponent) };
    crate::texture_assets::collect_texture(&decal.albedo_texture, out);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::handle_ledger::HandleLedgerRegistration {
        component_id: pulsar_scenedb::component_id::<DecalComponent>,
        collect_from_value: collect_decal_handles,
    }
}

/// How a decal takes in its loaded (or failed) albedo image.
pub(crate) fn bind_decal_texture(decal: &mut DecalComponent, view: &pulsar_scenedb::AssetView) {
    crate::texture_assets::bind_texture(
        &mut decal.albedo_image,
        &decal.albedo_texture,
        view,
        "decal",
    );
}

/// The decal's script methods (beside its property accessors, such as
/// `DecalComponent::set_opacity`).
fn decal_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    use pulsar_reflection::{
        EngineClass, MethodFlags, MethodMetadata, MethodParameter, MethodReturnType,
        RUNTIME_TYPE_REGISTRY,
    };
    let Some(f32_info) = RUNTIME_TYPE_REGISTRY.get::<f32>() else {
        return Vec::new();
    };
    vec![MethodMetadata {
        name: "fade_by",
        display_name: "Fade By".into(),
        category: Some("Decal"),
        params: vec![MethodParameter {
            name: "amount",
            type_info: f32_info,
        }],
        return_type: Some(MethodReturnType {
            type_info: f32_info,
        }),
        flags: MethodFlags::NONE,
        caller: Box::new(
            |instance: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                let amount = *args.first()?.downcast_ref::<f32>()?;
                let decal = instance.as_any_mut().downcast_mut::<DecalComponent>()?;
                Some(Box::new(decal.fade_by(amount)))
            },
        ),
    }]
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ComponentMethodRegistration {
        class_name: DECAL_CLASS_NAME,
        methods: decal_methods,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::EngineClass;

    #[test]
    fn an_authored_decal_is_permanent_and_carries_its_opacity() {
        let mut decal = DecalComponent {
            color: [0.0, 0.0, 1.0],
            opacity: 0.25,
            ..Default::default()
        };
        let row = decal.to_row(u32::MAX);
        assert_eq!(row.fade_time, 0.0);
        assert_eq!(row.color, [0.0, 0.0, 1.0, 0.25]);
        assert_eq!(row.albedo_texture_index, u32::MAX);
        decal.enabled = false;
        assert_eq!(decal.to_row(u32::MAX).color[3], 0.0);
    }

    #[test]
    fn fade_by_is_a_script_method_that_lowers_the_opacity() {
        let methods = <DecalComponent as EngineClass>::get_methods();
        let fade = methods
            .iter()
            .find(|method| method.name == "fade_by")
            .expect("fade_by is registered");
        let mut decal = DecalComponent::default();
        let left = (fade.caller)(&mut decal, vec![Box::new(0.75f32)]).unwrap();
        assert_eq!(*left.downcast_ref::<f32>().unwrap(), 0.25);
        assert_eq!(decal.opacity, 0.25);
        decal.fade_by(1.0);
        assert_eq!(decal.opacity, 0.0);
    }
}
