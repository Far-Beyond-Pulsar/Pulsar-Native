//! Virtual geometry project settings (plan:
//! `.agents/plans/virtual-geometry.md` sections 3 item 5 and 7).
//!
//! [`register`] adds the `project.virtual_geometry` namespace. It is purely
//! additive: a settings file written before it existed has no such keys, and
//! every key reads as its schema default. [`VirtualGeometrySettings`] is the
//! typed view of the namespace that the renderer's integration reads and
//! pushes to the virtual-geometry passes; the renderer maps
//! [`VirtualGeometryMode`] onto `helio_vg_format::VgMode`.

use pulsar_config::{
    ConfigManager, ConfigValue, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "virtual_geometry";

/// Largest visible-cluster ceiling the visibility id layout can address
/// (`helio_vg_format::MAX_VISIBLE_CLUSTERS`, 25 bits).
pub const MAX_VISIBLE_CLUSTERS_LIMIT: u32 = 1 << 25;
const MIN_VISIBLE_CLUSTERS: u32 = 4096;

/// Which virtual-geometry pipeline runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VirtualGeometryMode {
    /// Best path the device supports (never ray tracing for dense tiles
    /// unless the hardware is available).
    #[default]
    Auto,
    /// Dense tiles use the non-ray-traced fallback even where ray tracing
    /// exists.
    ForceNonRtFallback,
    /// Everything through hardware raster (a debugging and comparison mode).
    ForceHwRaster,
    /// Virtual geometry off: every mesh draws through the classic path.
    Off,
}

impl VirtualGeometryMode {
    pub const ALL: [Self; 4] = [
        Self::Auto,
        Self::ForceNonRtFallback,
        Self::ForceHwRaster,
        Self::Off,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ForceNonRtFallback => "force_non_rt_fallback",
            Self::ForceHwRaster => "force_hw_raster",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }

    /// Whether any instance may be virtual-geometry active.
    pub fn is_on(self) -> bool {
        self != Self::Off
    }
}

/// Which debug visualisation the virtual-geometry passes show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VirtualGeometryDebugView {
    #[default]
    None,
    Cluster,
    Lod,
    Tier,
    OverlapHeatmap,
    Overdraw,
}

impl VirtualGeometryDebugView {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Cluster,
        Self::Lod,
        Self::Tier,
        Self::OverlapHeatmap,
        Self::Overdraw,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cluster => "cluster",
            Self::Lod => "lod",
            Self::Tier => "tier",
            Self::OverlapHeatmap => "overlap_heatmap",
            Self::Overdraw => "overdraw",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.as_str() == value)
    }
}

/// Typed view of the `project.virtual_geometry` namespace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualGeometrySettings {
    pub mode: VirtualGeometryMode,
    /// Screen-space error threshold in pixels.
    pub tau_pixels: f32,
    /// Budget for resident cluster pages, in MB.
    pub page_budget_mb: u32,
    /// Ceiling on visible clusters per frame (a ceiling, not an allocation).
    pub max_visible_clusters: u32,
    /// Two-phase (previous-frame then current-frame Hi-Z) occlusion culling.
    pub two_phase_occlusion: bool,
    /// Tier 3: dense tiles (ray-traced where proxies are resident, else the
    /// depth peel only on the no-int64 Tier 2 fallback). Off by default: on an
    /// RTX 4090 Tier 2 handles up to ~9 triangles/pixel in ~1 ms, so Tier 3 does
    /// not pay there (see VG_STRESS.md, "Tier 3 measurements"). Enabled but idle
    /// it costs under 0.02 ms.
    pub tier3: bool,
    pub debug_view: VirtualGeometryDebugView,
}

impl Default for VirtualGeometrySettings {
    fn default() -> Self {
        Self {
            mode: VirtualGeometryMode::Auto,
            tau_pixels: 1.0,
            page_budget_mb: 512,
            max_visible_clusters: 4_194_304,
            two_phase_occlusion: true,
            tier3: false,
            debug_view: VirtualGeometryDebugView::None,
        }
    }
}

impl VirtualGeometrySettings {
    pub const TAU_RANGE: (f32, f32) = (0.1, 16.0);
    pub const PAGE_BUDGET_RANGE_MB: (u32, u32) = (64, 65_536);

    /// Read the namespace from `cfg`. Any key that is missing, of the wrong
    /// type or out of range reads as its default (settings files written by
    /// older builds, hand edits).
    pub fn from_config(cfg: &ConfigManager) -> Self {
        let get = |key: &str| cfg.get(NS, OWNER, key).ok();
        Self::from_lookup(&get)
    }

    /// As [`Self::from_config`], over any key lookup.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<ConfigValue>) -> Self {
        let defaults = Self::default();
        let text = |key: &str| {
            get(key).and_then(|value| value.as_str().ok().map(str::to_owned))
        };
        let int = |key: &str| get(key).and_then(|value| value.as_int().ok());
        Self {
            mode: text("mode")
                .and_then(|value| VirtualGeometryMode::parse(&value))
                .unwrap_or(defaults.mode),
            tau_pixels: get("tau_pixels")
                .and_then(|value| value.as_float().ok())
                .map_or(defaults.tau_pixels, |value| value as f32),
            page_budget_mb: int("page_budget_mb")
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(defaults.page_budget_mb),
            max_visible_clusters: int("max_visible_clusters")
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(defaults.max_visible_clusters),
            two_phase_occlusion: get("two_phase_occlusion")
                .and_then(|value| value.as_bool().ok())
                .unwrap_or(defaults.two_phase_occlusion),
            tier3: get("tier3")
                .and_then(|value| value.as_bool().ok())
                .unwrap_or(defaults.tier3),
            debug_view: text("debug_view")
                .and_then(|value| VirtualGeometryDebugView::parse(&value))
                .unwrap_or(defaults.debug_view),
        }
        .sanitized()
    }

    /// Clamp every number into the range the passes support; a non-finite
    /// threshold becomes the default.
    pub fn sanitized(mut self) -> Self {
        let (tau_min, tau_max) = Self::TAU_RANGE;
        self.tau_pixels = if self.tau_pixels.is_finite() {
            self.tau_pixels.clamp(tau_min, tau_max)
        } else {
            Self::default().tau_pixels
        };
        let (budget_min, budget_max) = Self::PAGE_BUDGET_RANGE_MB;
        self.page_budget_mb = self.page_budget_mb.clamp(budget_min, budget_max);
        self.max_visible_clusters = self
            .max_visible_clusters
            .clamp(MIN_VISIBLE_CLUSTERS, MAX_VISIBLE_CLUSTERS_LIMIT);
        self
    }
}

fn choices(values: impl IntoIterator<Item = (&'static str, &'static str)>) -> Vec<DropdownOption> {
    values
        .into_iter()
        .map(|(label, value)| DropdownOption::new(label, value))
        .collect()
}

/// The `project.virtual_geometry` schema.
pub fn schema() -> NamespaceSchema {
    let defaults = VirtualGeometrySettings::default();
    let (tau_min, tau_max) = VirtualGeometrySettings::TAU_RANGE;
    let (budget_min, budget_max) = VirtualGeometrySettings::PAGE_BUDGET_RANGE_MB;
    NamespaceSchema::new(
        "Virtual Geometry",
        "Cluster-based mesh rendering for static meshes",
    )
    .setting(
        "mode",
        SchemaEntry::new(
            "Which virtual geometry pipeline runs; Off draws every mesh classically",
            defaults.mode.as_str(),
        )
        .label("Mode")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::Dropdown {
            options: choices([
                ("Auto", "auto"),
                ("Force non-ray-traced fallback", "force_non_rt_fallback"),
                ("Force hardware raster", "force_hw_raster"),
                ("Off", "off"),
            ]),
        }),
    )
    .setting(
        "tau_pixels",
        SchemaEntry::new(
            "Largest screen-space geometric error, in pixels, a cluster may have before it is refined",
            f64::from(defaults.tau_pixels),
        )
        .label("Pixel Error")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::Slider {
            min: f64::from(tau_min),
            max: f64::from(tau_max),
            step: 0.1,
        })
        .validator(Validator::float_range(f64::from(tau_min), f64::from(tau_max))),
    )
    .setting(
        "page_budget_mb",
        SchemaEntry::new(
            "Video memory for resident cluster pages, in MB",
            i64::from(defaults.page_budget_mb),
        )
        .label("Page Budget (MB)")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::NumberInput {
            min: Some(f64::from(budget_min)),
            max: Some(f64::from(budget_max)),
            step: Some(64.0),
        })
        .validator(Validator::int_range(
            i64::from(budget_min),
            i64::from(budget_max),
        )),
    )
    .setting(
        "max_visible_clusters",
        SchemaEntry::new(
            "Most clusters drawn in one frame; a ceiling, memory grows only as the scene needs",
            i64::from(defaults.max_visible_clusters),
        )
        .label("Max Visible Clusters")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::NumberInput {
            min: Some(f64::from(MIN_VISIBLE_CLUSTERS)),
            max: Some(f64::from(MAX_VISIBLE_CLUSTERS_LIMIT)),
            step: Some(65_536.0),
        })
        .validator(Validator::int_range(
            i64::from(MIN_VISIBLE_CLUSTERS),
            i64::from(MAX_VISIBLE_CLUSTERS_LIMIT),
        )),
    )
    .setting(
        "two_phase_occlusion",
        SchemaEntry::new(
            "Occlusion cull in two phases, against the previous frame then the current one",
            defaults.two_phase_occlusion,
        )
        .label("Two-Phase Occlusion")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "tier3",
        SchemaEntry::new(
            "Resolve dense overlapping tiles with the ray-traced / depth-peel Tier 3 (experimental; no measured gain on int64-atomic GPUs)",
            defaults.tier3,
        )
        .label("Dense-Tile Tier 3 (experimental)")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "debug_view",
        SchemaEntry::new(
            "Visualise virtual geometry internals",
            defaults.debug_view.as_str(),
        )
        .label("Debug View")
        .page("Rendering / Virtual Geometry")
        .field_type(FieldType::Dropdown {
            options: choices([
                ("None", "none"),
                ("Clusters", "cluster"),
                ("Level of detail", "lod"),
                ("Raster tier", "tier"),
                ("Overlap heatmap", "overlap_heatmap"),
                ("Overdraw", "overdraw"),
            ]),
        }),
    )
}

pub fn register(cfg: &'static ConfigManager) {
    let _ = cfg.register(NS, OWNER, schema());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(values: HashMap<&'static str, ConfigValue>) -> VirtualGeometrySettings {
        VirtualGeometrySettings::from_lookup(&|key| values.get(key).cloned())
    }

    #[test]
    fn an_empty_namespace_reads_as_the_defaults() {
        // A settings file from before this namespace existed.
        let settings = lookup(HashMap::new());
        assert_eq!(settings, VirtualGeometrySettings::default());
        assert_eq!(settings.mode, VirtualGeometryMode::Auto);
        assert_eq!(settings.tau_pixels, 1.0);
        assert_eq!(settings.max_visible_clusters, 4_194_304);
        assert!(settings.two_phase_occlusion);
    }

    #[test]
    fn values_parse_and_wrong_types_fall_back() {
        let settings = lookup(HashMap::from([
            ("mode", ConfigValue::from("force_hw_raster")),
            ("tau_pixels", ConfigValue::from(2.5_f64)),
            ("page_budget_mb", ConfigValue::from(1024_i64)),
            ("two_phase_occlusion", ConfigValue::from(false)),
            ("debug_view", ConfigValue::from("tier")),
            ("max_visible_clusters", ConfigValue::from("many")),
        ]));
        assert_eq!(settings.mode, VirtualGeometryMode::ForceHwRaster);
        assert_eq!(settings.tau_pixels, 2.5);
        assert_eq!(settings.page_budget_mb, 1024);
        assert!(!settings.two_phase_occlusion);
        assert_eq!(settings.debug_view, VirtualGeometryDebugView::Tier);
        assert_eq!(settings.max_visible_clusters, 4_194_304);

        let unknown = lookup(HashMap::from([("mode", ConfigValue::from("turbo"))]));
        assert_eq!(unknown.mode, VirtualGeometryMode::Auto);
    }

    #[test]
    fn out_of_range_numbers_are_clamped() {
        let settings = lookup(HashMap::from([
            ("tau_pixels", ConfigValue::from(0.0_f64)),
            ("page_budget_mb", ConfigValue::from(1_i64)),
            ("max_visible_clusters", ConfigValue::from(i64::MAX)),
        ]));
        assert_eq!(settings.tau_pixels, VirtualGeometrySettings::TAU_RANGE.0);
        assert_eq!(settings.page_budget_mb, VirtualGeometrySettings::PAGE_BUDGET_RANGE_MB.0);
        // i64::MAX does not fit a u32, so the default applies, not a clamp.
        assert_eq!(settings.max_visible_clusters, 4_194_304);
        let huge = lookup(HashMap::from([(
            "max_visible_clusters",
            ConfigValue::from(1_i64 << 30),
        )]));
        assert_eq!(huge.max_visible_clusters, MAX_VISIBLE_CLUSTERS_LIMIT);
        assert!(VirtualGeometrySettings { tau_pixels: f32::NAN, ..Default::default() }
            .sanitized()
            .tau_pixels
            .is_finite());
    }

    #[test]
    fn names_round_trip() {
        for mode in VirtualGeometryMode::ALL {
            assert_eq!(VirtualGeometryMode::parse(mode.as_str()), Some(mode));
        }
        for view in VirtualGeometryDebugView::ALL {
            assert_eq!(VirtualGeometryDebugView::parse(view.as_str()), Some(view));
        }
        assert!(!VirtualGeometryMode::Off.is_on());
    }

    #[test]
    fn registered_schema_defaults_match_the_typed_defaults() {
        let cfg: &'static ConfigManager = Box::leak(Box::new(ConfigManager::new()));
        let handle = cfg.register(NS, OWNER, schema()).expect("register");
        assert_eq!(
            VirtualGeometrySettings::from_config(cfg),
            VirtualGeometrySettings::default()
        );
        handle.set("mode", "off").expect("set mode");
        handle.set("tau_pixels", 2.0_f64).expect("set tau");
        handle.set("debug_view", "overdraw").expect("set view");
        let settings = VirtualGeometrySettings::from_config(cfg);
        assert_eq!(settings.mode, VirtualGeometryMode::Off);
        assert_eq!(settings.tau_pixels, 2.0);
        assert_eq!(settings.debug_view, VirtualGeometryDebugView::Overdraw);
    }
}
