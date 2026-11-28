//! How a component wants the images it shows: its own choice, per component.
//!
//! A component keeps the choice as two properties and passes it when it asks
//! for a slot ([`crate::texture_assets::texture_slot`]). Components that make
//! the same choice for one image share one GPU texture; different choices are
//! different textures, each taking its own video memory.
//!
//! The project settings a plugin registers with [`register_settings`] are the
//! values a **new** component starts with (see
//! [`crate::asset_component::NewComponentRegistration`]). Changing them
//! changes no existing component, and a saved component without the
//! properties gets [`TextureOptions::default`], whatever the project says.
//! The `streaming` limits (largest size, video memory budget) are not
//! defaults: they apply to every texture at once.

use engine_state::settings::{
    ConfigManager, ConfigValue, FieldType, NamespaceSchema, SchemaEntry, Validator, NS_PROJECT,
};

/// The largest size a component can ask for.
const LARGEST_SIZE: i32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureOptions {
    /// Hold the image block-compressed where the device can sample it.
    pub compressed: bool,
    /// The largest side, in texels, the GPU holds; 0 for no limit of the
    /// component's own. Applied by leaving off the largest mip levels, so
    /// the result is the next level that fits.
    pub max_size: u32,
}

impl Default for TextureOptions {
    /// What a component saved before it could choose has.
    fn default() -> Self {
        Self {
            compressed: true,
            max_size: 0,
        }
    }
}

impl TextureOptions {
    /// From a component's two properties.
    pub fn new(compressed: bool, max_size: i32) -> Self {
        Self {
            compressed,
            max_size: max_size.clamp(0, LARGEST_SIZE) as u32,
        }
    }

    /// The number that tells this choice apart from others.
    pub(crate) fn variant_bits(self) -> u64 {
        u64::from(!self.compressed) | (u64::from(self.max_size) << 1)
    }

    /// How many of the largest mip levels an image of `width` by `height`
    /// leaves off to fit [`Self::max_size`].
    pub(crate) fn levels_to_drop(self, width: u32, height: u32) -> u32 {
        let mut drop = 0;
        let longest = width.max(height);
        while self.max_size > 0 && (longest >> drop) > self.max_size && (longest >> drop) > 1 {
            drop += 1;
        }
        drop
    }

    /// The values new components start with, from the project settings
    /// registered under `owner`.
    pub fn from_project(owner: &str) -> Self {
        let get = |key: &str| {
            engine_state::settings::global_config()
                .get(NS_PROJECT, owner, key)
                .ok()
        };
        let default = Self::default();
        let compressed = match get("compress_textures") {
            Some(ConfigValue::Bool(value)) => value,
            _ => default.compressed,
        };
        let max_size = match get("max_texture_size") {
            Some(ConfigValue::Int(value)) => value.clamp(0, i64::from(LARGEST_SIZE)) as i32,
            _ => default.max_size as i32,
        };
        Self::new(compressed, max_size)
    }
}

/// Register the project settings `owner` holds: what a new component of the
/// kind `things` names ("sprites") starts with.
pub fn register_settings(cfg: &'static ConfigManager, owner: &str, title: &str, things: &str) {
    let schema = NamespaceSchema::new(title, "What new components start with")
        .setting(
            "compress_textures",
            SchemaEntry::new(
                format!(
                    "Whether new {things} hold their image compressed in video memory. \
                     Existing ones keep their own setting"
                ),
                true,
            )
            .label("Compress Textures (new components)")
            .page("Textures")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "max_texture_size",
            SchemaEntry::new(
                format!(
                    "The largest side, in texels, new {things} hold their image at; 0 for no \
                     limit of their own. Existing ones keep their own setting"
                ),
                0_i64,
            )
            .label("Max Texture Size (new components)")
            .page("Textures")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(f64::from(LARGEST_SIZE)),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, i64::from(LARGEST_SIZE))),
        );
    let _ = cfg.register(NS_PROJECT, owner, schema);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_limit_drops_to_the_next_level_that_fits() {
        let limit = |max_size| TextureOptions {
            compressed: true,
            max_size,
        };
        assert_eq!(limit(0).levels_to_drop(4096, 4096), 0);
        assert_eq!(limit(2048).levels_to_drop(2048, 1024), 0);
        assert_eq!(limit(2048).levels_to_drop(4096, 256), 1);
        assert_eq!(limit(1000).levels_to_drop(4096, 4096), 3);
        assert_eq!(limit(1).levels_to_drop(8, 8), 3);
    }

    #[test]
    fn each_choice_has_its_own_number_and_the_old_one_is_zero() {
        assert_eq!(TextureOptions::default().variant_bits(), 0);
        let choices = [
            TextureOptions::new(true, 0),
            TextureOptions::new(false, 0),
            TextureOptions::new(true, 512),
            TextureOptions::new(false, 512),
        ];
        let numbers: std::collections::HashSet<u64> =
            choices.iter().map(|c| c.variant_bits()).collect();
        assert_eq!(numbers.len(), choices.len());
        assert_eq!(TextureOptions::new(true, -5).max_size, 0);
    }
}
