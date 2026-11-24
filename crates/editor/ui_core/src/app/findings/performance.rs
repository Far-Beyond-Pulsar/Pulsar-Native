//! Performance the editor can see without a renderer: textures drawn below
//! full detail to fit the texture VRAM budget, and an asset cache too small
//! for what the scene keeps using (it evicts values it then rebuilds).

use pulsar_findings::{Action, Category, Finding, Severity, SettingsWindow};

pub(super) const SOURCE: &str = "Streaming";

/// The cache counts as full above this share of its budget.
const FULL: f64 = 0.95;
/// Rebuilds of evicted values in one tick (a second) that make it thrash.
const THRASH_REBUILDS: u64 = 4;
/// Ticks of thrashing in a row before it is reported, and calm ticks in a
/// row before it clears.
const SUSTAINED: u32 = 5;

fn streaming_settings() -> Action {
    Action::OpenSettings {
        window: SettingsWindow::Project,
        page: Some("Streaming".to_string()),
    }
}

/// What the collector remembers between ticks.
#[derive(Default)]
pub(super) struct Performance {
    last_rebuilds: Option<u64>,
    thrash_ticks: u32,
    calm_ticks: u32,
    thrashing: bool,
}

impl Performance {
    pub(super) fn collect(&mut self) -> Vec<Finding> {
        let mut out = Vec::new();
        let bias = helio_component::texture_detail::detail_bias();
        if bias > 0 {
            out.push(texture_detail(bias, helio_component::texture_detail::vram_budget_bytes()));
        }
        let stats = pulsar_scenedb::residency_stats();
        if self.judge_cache(stats.resident_bytes as u64, stats.budget as u64, stats.rebuilds) {
            out.push(
                Finding::new("cache-thrash", Category::Performance, Severity::Warning, "The asset cache keeps rebuilding what it evicted")
                    .detail("Decoded asset data does not fit the CPU asset cache, so values are dropped and rebuilt from their source again and again (each rebuild decodes the file). Raise CPU Asset Cache in Project Settings > Streaming.")
                    .subject("Asset cache")
                    .metric(format!("{} / {} MB", stats.resident_bytes / (1024 * 1024), stats.budget / (1024 * 1024)))
                    .action("Open Streaming settings", streaming_settings()),
            );
        }
        out
    }

    /// Whether the cache is thrashing: full, and rebuilding evicted values
    /// tick after tick. Reported after [`SUSTAINED`] such ticks, cleared after
    /// as many calm ones.
    fn judge_cache(&mut self, resident: u64, budget: u64, rebuilds: u64) -> bool {
        let new_rebuilds = self.last_rebuilds.map_or(0, |last| rebuilds.saturating_sub(last));
        self.last_rebuilds = Some(rebuilds);
        let full = budget > 0 && resident as f64 >= budget as f64 * FULL;
        if full && new_rebuilds >= THRASH_REBUILDS {
            self.thrash_ticks += 1;
            self.calm_ticks = 0;
            if self.thrash_ticks >= SUSTAINED {
                self.thrashing = true;
            }
        } else {
            self.thrash_ticks = 0;
            self.calm_ticks += 1;
            if self.calm_ticks >= SUSTAINED {
                self.thrashing = false;
            }
        }
        self.thrashing
    }
}

fn texture_detail(bias: u32, budget_bytes: u64) -> Finding {
    let levels = if bias == 1 { "1 level" } else { "levels" };
    Finding::new("texture-detail", Category::Performance, Severity::Warning, "Textures are drawn below full detail")
        .detail(format!(
            "The scene's textures do not fit the {} MB texture VRAM budget, so every texture is drawn {bias} mip {levels} lower (each level a quarter of the texels). Raise Texture VRAM Budget, lower Max Texture Size, or use fewer or smaller textures.",
            budget_bytes / (1024 * 1024)
        ))
        .subject("Textures")
        .metric(format!("-{bias} mip"))
        .action("Open Streaming settings", streaming_settings())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_cache_that_rebuilds_tick_after_tick_is_thrashing_until_it_calms() {
        let mut perf = Performance::default();
        let mut rebuilds = 0;
        let mut tick = |perf: &mut Performance, resident: u64, more: u64| {
            rebuilds += more;
            perf.judge_cache(resident, 100, rebuilds)
        };
        assert!(!tick(&mut perf, 99, 0));
        for _ in 0..(SUSTAINED - 1) {
            assert!(!tick(&mut perf, 99, 10), "not yet sustained");
        }
        assert!(tick(&mut perf, 99, 10));
        assert!(tick(&mut perf, 50, 0), "one calm tick does not clear it");
        for _ in 0..SUSTAINED {
            tick(&mut perf, 50, 0);
        }
        assert!(!tick(&mut perf, 50, 0));
        assert!(!tick(&mut perf, 50, 100), "rebuilding with room to spare is not thrashing");
    }
}
