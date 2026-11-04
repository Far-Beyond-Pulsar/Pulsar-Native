//! The shadows' host settings ([`SHADOWS`], as `helio_settings` describes
//! the renderer's): engines build their settings UI from it and read the
//! values back into [`ShadowSettings`] ([`ShadowSettings::from_settings`]).

use helio_settings::{choice, integer, number, pick, toggle, Choice, Group, Resolved};

use crate::shadows::{ShadowQuality, ShadowSettings};

const QUALITY_TIERS: &[Choice] = &[
    choice("low", "Low"),
    choice("medium", "Medium"),
    choice("high", "High"),
    choice("ultra", "Ultra"),
];

/// The shadows' host settings: a host lists it with its other settings
/// groups.
pub static SHADOWS: Group = Group {
    key: "shadows",
    title: "Shadows",
    description: "The lights' shadow maps: their filtering and their atlas budget",
    settings: &[
        pick(
            "quality",
            "Shadow Quality",
            "Shadows",
            QUALITY_TIERS,
            "medium",
            "Shadow filtering quality",
        ),
        number(
            "memory_mb",
            "Shadow Memory (MB)",
            "Shadows",
            (16.0, 2048.0, 16.0),
            160.0,
            "GPU memory of the shadow atlases; sets their size",
        ),
        pick(
            "max_resolution",
            "Shadow Max Resolution",
            "Shadows",
            &[
                choice("512", "512"),
                choice("1024", "1024"),
                choice("2048", "2048"),
                choice("4096", "4096"),
                choice("8192", "8192"),
            ],
            "2048",
            "Largest shadow tile a light gets",
        ),
        integer(
            "updates_per_frame",
            "Shadow Updates per Frame",
            "Shadows",
            (1, 1024),
            32,
            "Shadow faces redrawn a frame at most",
        ),
        number(
            "update_megatexels",
            "Shadow Update Texels (M)",
            "Shadows",
            (0.25, 64.0, 0.25),
            4.0,
            "Millions of shadow texels redrawn a frame at most",
        ),
        number(
            "max_distance",
            "Shadow Distance",
            "Shadows",
            (10.0, 100_000.0, 10.0),
            500.0,
            "Metres from the eye that local lights cast shadows",
        ),
        integer(
            "fade_frames",
            "Shadow Fade Frames",
            "Shadows",
            (0, 240),
            12,
            "Frames a shadow fades in or out over",
        ),
        number(
            "hysteresis",
            "Shadow Hysteresis",
            "Shadows",
            (0.0, 1.0, 0.05),
            0.2,
            "How much a light's shadow resolution must change before its tile is reallocated",
        ),
        toggle(
            "page_invalidation",
            "Shadow Page Invalidation",
            "Shadows",
            true,
            "Redraw only the shadow pages a moving caster touches",
        ),
    ],
};

impl ShadowSettings {
    /// The settings `settings` (of [`SHADOWS`]) hold.
    pub fn from_settings(settings: &Resolved) -> Self {
        debug_assert!(std::ptr::eq(settings.group(), &SHADOWS));
        let mut shadows = Self::default();
        shadows.quality = match settings.text("quality") {
            "low" => ShadowQuality::Low,
            "high" => ShadowQuality::High,
            "ultra" => ShadowQuality::Ultra,
            _ => ShadowQuality::Medium,
        };
        let f = |key| settings.number(key) as f32;
        let budget = &mut shadows.budget;
        budget.memory_bytes = (settings.number("memory_mb") * 1024.0 * 1024.0) as u64;
        budget.max_resolution = settings
            .text("max_resolution")
            .parse()
            .unwrap_or(budget.max_resolution);
        budget.updates_per_frame = settings.number("updates_per_frame") as u32;
        budget.update_texels_per_frame =
            (settings.number("update_megatexels") * 1024.0 * 1024.0) as u32;
        budget.max_distance = f("max_distance");
        budget.fade_frames = settings.number("fade_frames") as u32;
        budget.hysteresis = f("hysteresis");
        budget.page_invalidation = settings.bool("page_invalidation");
        shadows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_defaults_are_the_default_settings() {
        assert_eq!(
            format!("{:?}", ShadowSettings::from_settings(&SHADOWS.defaults())),
            format!("{:?}", ShadowSettings::default())
        );
    }

    #[test]
    fn every_quality_maps() {
        let helio_settings::Kind::Choice(options) = SHADOWS.setting("quality").unwrap().kind else {
            panic!("quality is a choice");
        };
        let mut seen = Vec::new();
        for option in options {
            let settings = SHADOWS.resolve(|key| {
                (key == "quality").then(|| helio_settings::Value::Text(option.value.into()))
            });
            seen.push(ShadowSettings::from_settings(&settings).quality);
        }
        seen.dedup();
        assert_eq!(seen.len(), options.len());
    }
}
