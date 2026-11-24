//! What a renderer reports to the editor's Problems panel
//! (`pulsar_findings`), as it publishes its details: the render passes'
//! warnings and a GPU frame that stays over its frame budget, naming the
//! pass that costs most (Performance); a pipeline that does not build and a
//! rejected shader edit (Project).
//!
//! Each renderer publishes under its own source (several viewports render at
//! once) and withdraws it when dropped; the panel lists a finding two
//! renderers share once.

use std::sync::atomic::{AtomicU64, Ordering};

use pulsar_findings::{Category, Finding, Severity};

use crate::subsystems::render::GpuProfilerData;

/// The panel's name for these findings.
const LABEL: &str = "Renderer";
/// Frame budget when the output is unpaced: 60 frames a second.
const DEFAULT_BUDGET_MS: f32 = 1000.0 / 60.0;
/// How quickly the smoothed GPU time follows a change (per published detail,
/// about 4 a second): a few seconds, so one slow frame reports nothing.
const SMOOTHING: f32 = 0.15;
/// Over-budget starts above this fraction of the budget and ends below
/// [`CLEAR`], so a frame hovering at the budget does not flicker.
const REPORT: f32 = 1.10;
const CLEAR: f32 = 0.95;
/// How many details in a row (about 4 a second) must be over before it is
/// reported: a load hitch or one spike says nothing about the steady cost.
const SUSTAINED: u32 = 8;

/// One renderer's findings and the state they are judged from.
pub struct RendererFindings {
    source: String,
    gpu_ms: Option<f32>,
    over_budget: bool,
    /// Consecutive details over the report threshold.
    over_samples: u32,
    /// The passes' warnings by key: a warning is listed once it has been
    /// there for [`SUSTAINED`] details in a row, and until it has been gone
    /// as long (a condition that comes and goes frame to frame is not one
    /// to act on, and the list must not flicker).
    warnings: std::collections::BTreeMap<String, Debounced>,
    published: Vec<Finding>,
}

/// One warning and how long it has been there, or gone.
struct Debounced {
    latest: Finding,
    present: u32,
    absent: u32,
    shown: bool,
}

impl Default for RendererFindings {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            source: format!("{LABEL}#{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            gpu_ms: None,
            over_budget: false,
            over_samples: 0,
            warnings: Default::default(),
            published: Vec::new(),
        }
    }
}

impl RendererFindings {
    /// Judge the newest details and publish what changed. `budget_ms`: how
    /// long a frame may take (`None`: unpaced). `health`: whether the
    /// pipeline built and the last rejected shader edit.
    pub fn update(
        &mut self,
        profiler: Option<&GpuProfilerData>,
        warnings: &[String],
        budget_ms: Option<f32>,
        health: Option<&(helio::PipelineStatus, Option<String>)>,
    ) {
        let budget = budget_ms.filter(|ms| *ms > 0.0).unwrap_or(DEFAULT_BUDGET_MS);
        if let Some(total) = profiler.and_then(|p| p.total_gpu_ms).filter(|ms| ms.is_finite()) {
            let smoothed = match self.gpu_ms {
                Some(previous) => previous + (total - previous) * SMOOTHING,
                None => total,
            };
            self.gpu_ms = Some(smoothed);
            if smoothed > budget * REPORT {
                self.over_samples = self.over_samples.saturating_add(1);
                if self.over_samples >= SUSTAINED {
                    self.over_budget = true;
                }
            } else {
                self.over_samples = 0;
                if smoothed < budget * CLEAR {
                    self.over_budget = false;
                }
            }
        }
        let mut findings = self.debounce(warnings.iter().filter_map(|line| warning(line)));
        if self.over_budget {
            if let Some(gpu_ms) = self.gpu_ms {
                findings.push(frame_budget(gpu_ms, budget, profiler));
            }
        }
        if let Some((pipeline, shader_error)) = health {
            if let helio::PipelineStatus::Invalid { id, reason, details } = pipeline {
                findings.push(
                    Finding::new(
                        format!("pipeline:{id}"),
                        Category::Project,
                        Severity::Error,
                        format!("The render pipeline {id} does not build"),
                    )
                    .detail(
                        std::iter::once(reason.as_str())
                            .chain(details.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                    .subject("Render pipeline"),
                );
            }
            if let Some(error) = shader_error {
                let mut lines = error.lines();
                let first = lines.next().unwrap_or_default().trim();
                findings.push(
                    Finding::new("shader-edit", Category::Project, Severity::Error, "A shader edit was rejected")
                        .detail(
                            std::iter::once(first)
                                .chain(lines)
                                .take(24)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )
                        .subject("Shaders"),
                );
            }
        }
        if findings != self.published {
            pulsar_findings::publish(&self.source, findings.clone());
            self.published = findings;
        }
    }
}

impl RendererFindings {
    /// The warnings to list now, given the ones the passes report this time.
    fn debounce(&mut self, current: impl Iterator<Item = Finding>) -> Vec<Finding> {
        let mut seen = std::collections::BTreeSet::new();
        for finding in current {
            seen.insert(finding.key.clone());
            let entry = self.warnings.entry(finding.key.clone()).or_insert(Debounced {
                latest: finding.clone(),
                present: 0,
                absent: 0,
                shown: false,
            });
            entry.latest = finding;
            entry.present = entry.present.saturating_add(1);
            entry.absent = 0;
            if entry.present >= SUSTAINED {
                entry.shown = true;
            }
        }
        self.warnings.retain(|key, entry| {
            if seen.contains(key) {
                return true;
            }
            entry.present = 0;
            entry.absent = entry.absent.saturating_add(1);
            entry.shown && entry.absent < SUSTAINED
        });
        self.warnings
            .values()
            .filter(|entry| entry.shown)
            .map(|entry| entry.latest.clone())
            .collect()
    }
}

impl Drop for RendererFindings {
    fn drop(&mut self) {
        pulsar_findings::publish(&self.source, Vec::new());
    }
}

/// A pass's `WARNING:` line as a finding: its first sentence is the title,
/// the rest the explanation; the key is the title without its numbers, so a
/// count that changes is still the same finding.
fn warning(line: &str) -> Option<Finding> {
    let text = line.trim().strip_prefix("WARNING:")?.trim();
    if text.is_empty() {
        return None;
    }
    let (title, detail) = match text.find(". ") {
        Some(end) => (&text[..end], text[end + 2..].trim()),
        None => (text.trim_end_matches('.'), ""),
    };
    let key: String = title.chars().filter(|c| !c.is_ascii_digit()).collect();
    let mut title_chars = title.chars();
    let title = match title_chars.next() {
        Some(first) => first.to_uppercase().chain(title_chars).collect::<String>(),
        None => String::new(),
    };
    Some(
        Finding::new(format!("warning:{key}"), Category::Performance, Severity::Warning, title)
            .detail(detail)
            .subject(LABEL),
    )
}

fn frame_budget(gpu_ms: f32, budget: f32, profiler: Option<&GpuProfilerData>) -> Finding {
    let severity = if gpu_ms > budget * 2.0 {
        Severity::Error
    } else {
        Severity::Warning
    };
    let slowest = profiler
        .and_then(GpuProfilerData::slowest_gpu_pass)
        .map(|(pass, ms)| {
            format!(" The most expensive pass is {pass} at {ms:.1} ms: start there (the GPU pipeline overlay lists every pass).")
        })
        .unwrap_or_default();
    Finding::new(
        "frame-budget",
        Category::Performance,
        severity,
        "GPU frame time is over the frame budget",
    )
    .detail(format!(
        "The GPU takes {gpu_ms:.1} ms a frame on average; the output allows {budget:.1} ms, so frames are dropped or the display waits.{slowest}"
    ))
    .subject(LABEL)
    .metric(format!("{gpu_ms:.1} / {budget:.1} ms"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiler(total_gpu_ms: f32) -> GpuProfilerData {
        GpuProfilerData {
            total_gpu_ms: Some(total_gpu_ms),
            ..Default::default()
        }
    }

    #[test]
    fn a_warning_line_becomes_a_finding_whose_key_ignores_counts() {
        let a = warning("WARNING: a screen tile has 300 lights. Select the tile view.").unwrap();
        let b = warning("WARNING: a screen tile has 412 lights. Select the tile view.").unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(a.title, "A screen tile has 300 lights");
        assert_eq!(a.detail, "Select the tile view.");
        assert!(warning("stats: 3 chunks").is_none(), "only WARNING lines");
    }

    #[test]
    fn the_frame_budget_finding_needs_a_sustained_overrun_and_clears_with_hysteresis() {
        let mut findings = RendererFindings::default();
        let budget = Some(10.0);
        findings.update(Some(&profiler(8.0)), &[], budget, None);
        findings.update(Some(&profiler(30.0)), &[], budget, None);
        assert!(findings.published.is_empty(), "one slow frame is not reported");
        for _ in 0..30 {
            findings.update(Some(&profiler(13.0)), &[], budget, None);
        }
        assert_eq!(findings.published.len(), 1);
        for _ in 0..30 {
            findings.update(Some(&profiler(10.0)), &[], budget, None);
        }
        assert_eq!(findings.published.len(), 1, "at the budget it stays (hysteresis)");
        for _ in 0..30 {
            findings.update(Some(&profiler(8.0)), &[], budget, None);
        }
        assert!(findings.published.is_empty(), "well under the budget it clears");
    }

    #[test]
    fn a_warning_is_listed_only_while_it_persists() {
        let mut findings = RendererFindings::default();
        let line = vec!["WARNING: a tile is crowded. Inspect it.".to_string()];
        findings.update(None, &line, None, None);
        findings.update(None, &[], None, None);
        assert!(findings.published.is_empty(), "a one-off warning is not listed");
        for _ in 0..SUSTAINED {
            findings.update(None, &line, None, None);
        }
        assert_eq!(findings.published.len(), 1);
        for _ in 0..(SUSTAINED - 1) {
            findings.update(None, &[], None, None);
        }
        assert_eq!(findings.published.len(), 1, "a brief absence keeps it");
        findings.update(None, &[], None, None);
        assert!(findings.published.is_empty(), "gone long enough, it clears");
    }
}
