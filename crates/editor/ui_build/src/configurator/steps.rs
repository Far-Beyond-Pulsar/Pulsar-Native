//! The configurator's build-steps section: which steps to go through, and a
//! live preview of the exact commands that will run.

use engine_state::build_config::{BuildConfiguration, BuildSteps, TargetPlatform};
use gpui::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use ui::button::Button;
use ui::switch::Switch;
use ui::{Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex};

use super::form::section;
use super::{BuildConfiguratorWindow, Palette};
use crate::runner::plan::{self, Step};

struct StepRow {
    title: &'static str,
    description: &'static str,
    get: fn(&BuildSteps) -> bool,
    set: fn(&mut BuildSteps, bool),
}

/// In the order they run.
const STEPS: [StepRow; 5] = [
    StepRow {
        title: "Update dependencies",
        description: "cargo update: pull the newest compatible versions first.",
        get: |s| s.update,
        set: |s, v| s.update = v,
    },
    StepRow {
        title: "Clean first",
        description: "cargo clean: throw away old build output and start from scratch.",
        get: |s| s.clean,
        set: |s, v| s.clean = v,
    },
    StepRow {
        title: "Check",
        description: "cargo check: type-check quickly without producing binaries.",
        get: |s| s.check,
        set: |s, v| s.check = v,
    },
    StepRow {
        title: "Build",
        description: "cargo build: compile for every selected platform.",
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
        title: "Run",
        description: "Launch the game when the build finishes (turns Build on).",
        get: |s| s.run,
        set: |s, v| s.run = v,
    },
];

const PRESETS: [(&str, BuildSteps); 5] = [
    (
        "Quick check",
        BuildSteps {
            update: false,
            clean: false,
            check: true,
            build: false,
            run: false,
        },
    ),
    ("Build", BuildSteps::just_build()),
    (
        "Clean build",
        BuildSteps {
            update: false,
            clean: true,
            check: false,
            build: true,
            run: false,
        },
    ),
    (
        "Build & run",
        BuildSteps {
            update: false,
            clean: false,
            check: false,
            build: true,
            run: true,
        },
    ),
    (
        "Full refresh",
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

        let presets =
            h_flex()
                .gap_2()
                .flex_wrap()
                .children(PRESETS.iter().map(|(label, preset)| {
                    let preset = *preset;
                    Button::new(SharedString::from(format!("bc-preset-{label}")))
                        .small()
                        .label(*label)
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
                                .child(row.title),
                        )
                        .child(div().text_xs().text_color(p.muted).child(row.description)),
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
            "Build steps",
            "They always run in this order. Unused steps are skipped.",
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
                        .unwrap_or_else(|| "generate or refresh project files".to_owned());
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
                        .child(format!("environment: {env}"))
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
                .child("PIPELINE PREVIEW"),
        )
        .child(body)
        .into_any_element()
}
