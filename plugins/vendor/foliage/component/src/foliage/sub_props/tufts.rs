use engine_class_derive::engine_class;

/// How a type's blades gather into tufts and patches (meadows, flower
/// clusters). The defaults are a lone upright blade and no patches.
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Tufts", category_color = "#9BD45A")]
#[serde(default)]
pub struct TuftFoliageProps {
    /// Blades a tuft gathers, at least and at most (1 and 1: a lone blade).
    #[property(min = 1.0, max = 64.0, step = 1.0, category = "Tufts")]
    pub blades_min: u32,
    #[property(min = 1.0, max = 64.0, step = 1.0, category = "Tufts")]
    pub blades_max: u32,
    /// How far a tuft's blades lean out from its centre (run over rise): at
    /// the centre blade and at the rim. Blocky blades step out a cube per
    /// `1 / lean` cubes up.
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Tufts")]
    pub lean_center: f32,
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Tufts")]
    pub lean_rim: f32,
    /// Extra lean at the tip over the base.
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Tufts")]
    pub curve: f32,
    /// Share of blades lying over.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Tufts")]
    pub fall_chance: f32,
    /// Whether the blades' tops take `tip_color` (flower heads).
    #[property(category = "Tufts")]
    pub tip_colored: bool,
    /// The tops' linear colour.
    #[property(category = "Tufts")]
    pub tip_color: [f32; 3],
    /// Metres across the type's bare and dense patches (0: none), and how
    /// far patches thin it (1: bare between them).
    #[property(min = 0.0, max = 1000.0, step = 0.5, category = "Tufts")]
    pub patch_scale: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Tufts")]
    pub patch_strength: f32,
}

impl Default for TuftFoliageProps {
    fn default() -> Self {
        Self {
            blades_min: 1,
            blades_max: 1,
            lean_center: 0.0,
            lean_rim: 0.0,
            curve: 0.0,
            fall_chance: 0.0,
            tip_colored: false,
            tip_color: [1.0; 3],
            patch_scale: 0.0,
            patch_strength: 0.0,
        }
    }
}
