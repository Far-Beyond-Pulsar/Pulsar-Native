//! "Build Core" split-button.
//!
//! Left side  — triggers the currently selected build mode immediately.
//! Right side — chevron opens a popup menu to switch mode.
//!
//! Modes:
//!   Build        — regenerate + cargo build
//!   Build + Run  — regenerate + cargo build, then `cargo run` (process tracked)
//!   Check        — cargo check only (fast type-check, no codegen)
//!
//! While a Build+Run process is alive the dropdown is disabled and a Stop
//! button appears next to it.  Killing the process re-enables everything.

use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _, DropdownButton};
use ui::notification::Notification;
use ui::{h_flex, ContextModal as _, Disableable as _, IconName, Sizable as _};

use super::super::actions::SetBuildMode;
use crate::level_editor::state::{BuildMode, EditorMode, LevelEditorState};

pub(super) struct BuildCoreNotification;

pub struct BuildCoreButton;

impl BuildCoreButton {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let is_playing = state.scene.editor_mode == EditorMode::Play;
        let game_running = state.build.game_running;
        let build_mode = state.build.mode;
        let entity_id = cx.entity().entity_id();

        let (label, icon, tooltip) = mode_label_icon_tooltip(build_mode);

        // ── Primary button ────────────────────────────────────────────────────
        let state_for_click = state_arc.clone();
        let primary = Button::new("build_core_primary")
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .when(is_playing || game_running, |b| b.disabled(true))
            .on_click(move |_, window, cx| {
                let mode = state_for_click.read().build.mode;
                trigger_build(mode, state_for_click.clone(), entity_id, window, cx);
            });

        // ── Dropdown (chevron) ────────────────────────────────────────────────
        let state_for_menu = state_arc.clone();
        let dropdown = DropdownButton::new("build_core_dropdown")
            .button(primary)
            .when(!is_playing && !game_running, |d| {
                d.popup_menu(move |menu, _, _| {
                    let current = state_for_menu.read().build.mode;
                    menu.label("Build Mode")
                        .separator()
                        .menu_with_check(
                            "Build",
                            current == BuildMode::Build,
                            Box::new(SetBuildMode(BuildMode::Build)),
                        )
                        .menu_with_check(
                            "Build + Run",
                            current == BuildMode::BuildAndRun,
                            Box::new(SetBuildMode(BuildMode::BuildAndRun)),
                        )
                        .menu_with_check(
                            "Check",
                            current == BuildMode::Check,
                            Box::new(SetBuildMode(BuildMode::Check)),
                        )
                        .menu_with_check(
                            "Update",
                            current == BuildMode::Update,
                            Box::new(SetBuildMode(BuildMode::Update)),
                        )
                        .menu_with_check(
                            "Update + Build + Run",
                            current == BuildMode::UpdateBuildAndRun,
                            Box::new(SetBuildMode(BuildMode::UpdateBuildAndRun)),
                        )
                        .separator()
                        .label("Scratch (clean first)")
                        .menu_with_check(
                            "Build (Scratch)",
                            current == BuildMode::BuildScratch,
                            Box::new(SetBuildMode(BuildMode::BuildScratch)),
                        )
                        .menu_with_check(
                            "Build + Run (Scratch)",
                            current == BuildMode::BuildAndRunScratch,
                            Box::new(SetBuildMode(BuildMode::BuildAndRunScratch)),
                        )
                        .menu_with_check(
                            "Check (Scratch)",
                            current == BuildMode::CheckScratch,
                            Box::new(SetBuildMode(BuildMode::CheckScratch)),
                        )
                })
            });

        // ── Stop button (only while game is running) ──────────────────────────
        let state_for_stop = state_arc.clone();
        let stop_btn = Button::new("build_core_stop")
            .icon(IconName::Square)
            .label("Stop")
            .tooltip("Stop the running game")
            .on_click(move |_, _, cx| {
                let mut state = state_for_stop.write();
                if let Some(mut child) = state.build.game_process.lock().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                state.build.game_running = false;
                cx.notify(entity_id);
            });

        h_flex()
            .gap_1()
            .items_center()
            .child(dropdown)
            .when(game_running, |el| el.child(stop_btn))
    }
}

fn mode_label_icon_tooltip(mode: BuildMode) -> (&'static str, IconName, &'static str) {
    match mode {
        BuildMode::Build => (
            "Build",
            IconName::Hammer,
            "Compile all blueprints and generate a runnable Pulsar game crate",
        ),
        BuildMode::BuildAndRun => (
            "Build + Run",
            IconName::Play,
            "Compile and immediately launch the game",
        ),
        BuildMode::Check => (
            "Check",
            IconName::Check,
            "Run cargo check — fast type-check with no codegen",
        ),
        BuildMode::Update => (
            "Update",
            IconName::Refresh,
            "Run cargo update — refresh all git and registry dependencies",
        ),
        BuildMode::UpdateBuildAndRun => (
            "Update + Build + Run",
            IconName::Refresh,
            "Refresh git/registry dependencies (picks up newly-pushed engine commits), then compile and launch the game",
        ),
        BuildMode::BuildScratch => (
            "Build (Scratch)",
            IconName::Hammer,
            "Clean then build from scratch — cargo clean + cargo build --release",
        ),
        BuildMode::BuildAndRunScratch => (
            "Build + Run (Scratch)",
            IconName::Play,
            "Clean, build from scratch, then launch — cargo clean + cargo build + cargo run",
        ),
        BuildMode::CheckScratch => (
            "Check (Scratch)",
            IconName::Check,
            "Clean then check from scratch — cargo clean + cargo check",
        ),
    }
}

fn project_root() -> Option<PathBuf> {
    engine_state::get_project_path().map(PathBuf::from)
}

fn trigger_build(
    mode: BuildMode,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    entity_id: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(root) = project_root() else {
        window.push_notification(
            Notification::warning(t!("Notification.Message.NoProjectOpen").to_string()),
            cx,
        );
        return;
    };

    match mode {
        BuildMode::Check => run_check(root, window, cx),
        BuildMode::Update => run_update(root, window, cx),
        BuildMode::UpdateBuildAndRun => {
            run_update_build_and_run(root, state_arc, entity_id, window, cx)
        }
        BuildMode::Build => run_build_pipeline(root, mode, None, entity_id, window, cx),
        BuildMode::BuildAndRun => {
            run_build_pipeline(root, mode, Some(state_arc), entity_id, window, cx)
        }
        BuildMode::BuildScratch => run_scratch(root, BuildMode::Build, None, entity_id, window, cx),
        BuildMode::BuildAndRunScratch => run_scratch(
            root,
            BuildMode::BuildAndRun,
            Some(state_arc),
            entity_id,
            window,
            cx,
        ),
        BuildMode::CheckScratch => run_scratch(root, BuildMode::Check, None, entity_id, window, cx),
    }
}

mod crash;
mod pipeline;
mod quick;

use pipeline::run_build_pipeline;
use quick::{run_check, run_scratch, run_update, run_update_build_and_run};
