//! Startup compatibility notices for known hardware/driver issues.

use std::{path::PathBuf, sync::Arc};

use gpui::{prelude::*, *};
use serde::Deserialize;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _,
};
use ui_common::PulsarWindowExt as _;
use window_manager::{PulsarWindow, WindowConfig};

use crate::LoadingScreen;

const ISSUE_DATABASE: &str = include_str!("../data/known_issues.json");

/// Machine details used to match the compiled compatibility database.
#[derive(Clone, Debug)]
pub struct HardwareProfile {
    /// Graphics API selected for wgpu (for example `Vulkan` or `DX12`).
    pub backend: String,
    /// GPU vendor, e.g. `NVIDIA` or `AMD`.
    pub manufacturer: String,
    /// GPU model name reported by wgpu.
    pub gpu_brand: String,
    /// GPU driver version reported by wgpu or the platform.
    pub driver: String,
    /// OS name and version, joined for matching (for example `Windows 11`).
    pub os: String,
}

#[derive(Clone, Debug, Deserialize)]
struct IssueDatabase {
    #[serde(default)]
    issues: Vec<KnownIssue>,
}

#[derive(Clone, Debug, Deserialize)]
struct KnownIssue {
    id: String,
    title: String,
    description: String,
    #[serde(default)]
    manufacturer: String,
    #[serde(default)]
    driver: String,
    #[serde(default)]
    gpu_brand: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    backend: String,
    engine_version: VersionRange,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VersionRange {
    /// Inclusive lower bound. Omit to match all earlier versions.
    min_inclusive: Option<String>,
    /// Exclusive upper bound. Omit to match all later versions.
    max_exclusive: Option<String>,
}

impl KnownIssue {
    fn matches(&self, profile: &HardwareProfile, engine_version: &str) -> bool {
        wildcard_matches(&self.manufacturer, &profile.manufacturer)
            && wildcard_matches(&self.driver, &profile.driver)
            && wildcard_matches(&self.gpu_brand, &profile.gpu_brand)
            && wildcard_matches(&self.os, &profile.os)
            && wildcard_matches(&self.backend, &profile.backend)
            && self.engine_version.matches(engine_version)
    }
}

impl VersionRange {
    fn matches(&self, version: &str) -> bool {
        let Some(version) = numeric_version(version) else {
            return false;
        };
        if self.min_inclusive.as_deref().is_some_and(|min| {
            numeric_version(min).map_or(true, |min| compare_versions(&version, &min).is_lt())
        }) {
            return false;
        }
        if self.max_exclusive.as_deref().is_some_and(|max| {
            numeric_version(max).map_or(true, |max| !compare_versions(&version, &max).is_lt())
        }) {
            return false;
        }
        true
    }
}

fn numeric_version(version: &str) -> Option<Vec<u64>> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);
    let mut numbers = Vec::new();
    for part in &mut parts {
        numbers.push(part.ok()?);
    }
    if numbers.is_empty() {
        None
    } else {
        Some(numbers)
    }
}

fn compare_versions(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    (0..len)
        .map(|index| {
            a.get(index)
                .copied()
                .unwrap_or(0)
                .cmp(&b.get(index).copied().unwrap_or(0))
        })
        .find(|ordering| !ordering.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let parts: Vec<_> = pattern.split('*').collect();
    let mut rest = value.as_str();
    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');

    for (index, part) in parts
        .iter()
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
    {
        let Some(position) = rest.find(part) else {
            return false;
        };
        if index == 0 && anchored_start && position != 0 {
            return false;
        }
        rest = &rest[position + part.len()..];
    }

    if anchored_end {
        parts.last().is_some_and(|last| value.ends_with(last))
    } else {
        true
    }
}

fn matching_issues(profile: &HardwareProfile, engine_version: &str) -> Vec<KnownIssue> {
    let database: IssueDatabase = match serde_json::from_str(ISSUE_DATABASE) {
        Ok(database) => database,
        Err(error) => {
            tracing::error!("Failed to parse compiled hardware issue database: {error}");
            return Vec::new();
        }
    };
    database
        .issues
        .into_iter()
        .filter(|issue| issue.matches(profile, engine_version))
        .collect()
}

/// Checks the compiled issue database and shows a notice screen before the
/// loading splash when the current machine and engine version are affected.
pub fn open_before_loading(
    path: PathBuf,
    on_complete: Arc<dyn Fn(PathBuf, &mut gpui::App) + Send + Sync>,
    profile: HardwareProfile,
    engine_version: &'static str,
    cx: &mut gpui::App,
) {
    let issues = matching_issues(&profile, engine_version);
    if issues.is_empty() {
        LoadingScreen::open((path, on_complete), cx);
    } else {
        CompatibilityNotice::open((path, on_complete, issues), cx);
    }
}

struct CompatibilityNotice {
    project_path: PathBuf,
    on_complete: Arc<dyn Fn(PathBuf, &mut gpui::App) + Send + Sync>,
    issues: Vec<KnownIssue>,
}

#[window_manager::register_window]
impl PulsarWindow for CompatibilityNotice {
    type Params = (
        PathBuf,
        Arc<dyn Fn(PathBuf, &mut gpui::App) + Send + Sync>,
        Vec<KnownIssue>,
    );

    fn window_name() -> &'static str {
        "CompatibilityNotice"
    }

    fn window_options(_: &Self::Params) -> WindowOptions {
        WindowConfig::dialog(620.0, 440.0)
    }

    fn build(params: Self::Params, _: &mut Window, cx: &mut gpui::App) -> Entity<Self> {
        let (project_path, on_complete, issues) = params;
        cx.new(|_| Self {
            project_path,
            on_complete,
            issues,
        })
    }
}

impl Render for CompatibilityNotice {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let issue_cards = self.issues.iter().map(|issue| {
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .rounded_md()
                .bg(cx.theme().background)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::BOLD)
                        .child(issue.title.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(issue.description.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Issue {}", issue.id)),
                )
        });

        v_flex()
            .size_full()
            .gap_4()
            .p_5()
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Known compatibility issue"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Pulsar found a known issue for this graphics setup and engine version. Review the notice before continuing."))
            .child(v_flex().flex_1().w_full().gap_3().children(issue_cards))
            .child(h_flex().w_full().justify_end().child(
                Button::new("compatibility-continue")
                    .primary()
                    .label("Continue to loading screen")
                    .on_click(move |_, window, cx| {
                        let (path, callback) = entity.update(cx, |this, _| {
                            (this.project_path.clone(), this.on_complete.clone())
                        });
                        LoadingScreen::open((path, callback), cx);
                        window.remove_window();
                    }),
            ))
    }
}
