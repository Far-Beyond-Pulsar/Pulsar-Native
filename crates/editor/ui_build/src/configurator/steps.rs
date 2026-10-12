//! The configurator's build-steps section: which steps to go through, and a
//! live preview of the exact commands that will run.

use engine_state::build_config::{BuildConfiguration, BuildSteps, TargetPlatform};
use gpui::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use rust_i18n::t;
use ui::button::Button;
use ui::switch::Switch;
use ui::{Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex};

use super::form::section;
use super::{BuildConfiguratorWindow, Palette};
use crate::runner::plan::{self, Step};

/// `title` and `description` are locale keys.
struct StepRow {
    title: &'static str,
    description: &'static str,
    get: fn(&BuildSteps) -> bool,
    set: fn(&mut BuildSteps, bool),
}

/// In the order they run.
const STEPS: [StepRow; 5] = [
    StepRow {
        title: "Build.Steps.Update.Title",
        description: "Build.Steps.Update.Description",
        get: |s| s.update,
        set: |s, v| s.update = v,
    },
    StepRow {
        title: "Build.Steps.Clean.Title",
        description: "Build.Steps.Clean.Description",
        get: |s| s.clean,
        set: |s, v| s.clean = v,
    },
    StepRow {
        title: "Build.Steps.Check.Title",
        description: "Build.Steps.Check.Description",
        get: |s| s.check,
        set: |s, v| s.check = v,
    },
    StepRow {
        title: "Build.Steps.Build.Title",
        description: "Build.Steps.Build.Description",
        get: |s| s.build,
        // No build, nothing to run.
        set: |s, v| {
            s.build = v;
            if !v {
                s.run = false;
            }
        },
    },
    StepRow {
        title: "Build.Steps.Run.Title",
        description: "Build.Steps.Run.Description",
        get: |s| s.run,
        set: |s, v| s.run = v,
    },
];

/// Locale key and steps.
const PRESETS: [(&str, BuildSteps); 5] = [
    (
        "Build.Steps.Preset.QuickCheck",
        BuildSteps {
            update: false,
            clean: false,
            check: true,
            build: false,
            run: false,
        },
    ),
    ("Build.Steps.Preset.Build", BuildSteps::just_build()),
    (
        "Build.Steps.Preset.CleanBuild",
        BuildSteps {
            update: false,
            clean: true,
            check: false,
            build: true,
            run: false,
        },
    ),
    (
        "Build.Steps.Preset.BuildAndRun",
        BuildSteps {
            update: false,
            clean: false,
            check: false,
            build: true,
            run: true,
        },
    ),
    (
        "Build.Steps.Preset.FullRefresh",
        BuildSteps {
            update: true,
            clean: true,
            check: false,
            build: true,
            run: true,
        },
    ),
];

impl BuildConfiguratorWindow {
    pub(super) fn render_steps(
        &mut self,
        config: &BuildConfiguration,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let steps = config.steps;

        let presets = h_flex()
            .gap_2()
            .flex_wrap()
            .children(PRESETS.iter().map(|(key, preset)| {
                let preset = *preset;
                Button::new(SharedString::from(format!("bc-preset-{key}")))
                    .small()
                    .label(t!(*key).to_string())
                    .selected(steps == preset)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.edit(cx, |c| c.steps = preset)),
                    )
            }));

        let rows = STEPS.iter().enumerate().map(|(ix, row)| {
            let on = (row.get)(&steps);
            let set = row.set;
            h_flex()
                .gap_3()
                .items_center()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(if on { p.primary.opacity(0.5) } else { p.border })
                .child(
                    div()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_xs()
                        .bg(if on { p.primary } else { p.hover })
                        .child(format!("{}", ix + 1)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(t!(row.title).to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted)
                                .child(t!(row.description).to_string()),
                        ),
                )
                .child(
                    Switch::new(SharedString::from(format!("bc-step-{ix}")))
                        .checked(on)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let value = *checked;
                            this.edit(cx, move |c| set(&mut c.steps, value));
                        })),
                )
        });

        section(
            t!("Build.Steps.Title").to_string(),
            t!("Build.Steps.Description").to_string(),
            p,
            v_flex()
                .gap_3()
                .child(presets)
                .child(v_flex().gap_2().children(rows))
                .child(preview(config, p)),
        )
    }
}

/// The commands the configuration will run, exactly as the runner will.
fn preview(config: &BuildConfiguration, p: Palette) -> AnyElement {
    let body: AnyElement = match plan::plan(config, TargetPlatform::host()) {
        Err(error) => h_flex()
            .gap_2()
            .items_center()
            .child(
                Icon::new(IconName::WarningTriangle)
                    .size(px(14.))
                    .text_color(p.danger),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(p.danger)
                    .child(error.to_string()),
            )
            .into_any_element(),
        Ok(plan) => {
            let env = plan
                .steps
                .iter()
                .filter_map(Step::invocation)
                .find(|i| !i.envs.is_empty())
                .map(|i| {
                    i.envs
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("  ")
                });
            v_flex()
                .gap_1()
                .children(plan.steps.iter().enumerate().map(|(ix, step)| {
                    let command = step
                        .invocation()
                        .map(|i| i.display())
                        .unwrap_or_else(|| t!("Build.Preview.Bootstrap").to_string());
                    h_flex()
                        .gap_3()
                        .items_baseline()
                        .child(
                            div()
                                .w(px(18.))
                                .text_xs()
                                .text_color(p.muted)
                                .child(format!("{}.", ix + 1)),
                        )
                        .child(div().w(px(190.)).text_sm().child(step.title()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(p.muted)
                                .child(command),
                        )
                }))
                .children(env.map(|env| {
                    div()
                        .pt_1()
                        .font_family("monospace")
                        .text_xs()
                        .text_color(p.muted)
                        .child(t!("Build.Preview.Environment", env = env).to_string())
                }))
                .children(plan.warnings.iter().map(|warning| {
                    h_flex()
                        .pt_1()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::WarningTriangle)
                                .size(px(14.))
                                .text_color(p.warning),
                        )
                        .child(div().text_sm().text_color(p.warning).child(warning.clone()))
                }))
                .into_any_element()
        }
    };

    v_flex()
        .gap_2()
        .p_3()
        .rounded_md()
        .bg(p.bg.opacity(0.6))
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .text_xs()
                .text_color(p.muted)
                .child(t!("Build.Preview.Title").to_string()),
        )
        .child(body)
        .into_any_element()
}
