//! The build as MCP tools: `build_configurations`, `build_set_mode`,
//! `build_start`, `build_stop`, `build_status` and `build_output`.
//!
//! They act through the seams the toolbar's Build button uses: the project's
//! build configurations (`engine_state::build_config`), [`run_configuration`]
//! (which plans and runs the steps as an editor task and opens the dashboard),
//! [`cancel_build`] and [`stop_game`]. Starting a build needs the UI thread (it
//! opens a window and pushes notifications), so [`build_start`] hops there with
//! `ui_common::ui_call`; everything else reads shared state from the worker.
//!
//! A build's output is the build task's log (cargo's lines, the packer's, the
//! compiler errors one row each); the game's is the `Game: Run` task's log. Both
//! come back from `build_output` with a cursor, and from `logs_query` filtered by
//! task.

use std::time::Duration;

use engine_state::build_config::{
    BuildConfiguration, BuildProfile, build_configurations, ensure_loaded, persist,
};
use engine_state::playback::playback;
use gpui::AppContext as _;
use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use serde_json::{Value, json};

use crate::runner::session::{self, Outcome};
use crate::runner::{cancel_build, run_configuration, stop_game};

fn args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn config_json(config: &BuildConfiguration, selected: bool) -> Value {
    json!({
        "id": config.id,
        "name": config.name,
        "description": config.description,
        "selected": selected,
        "profile": format!("{:?}", config.profile).to_lowercase(),
        "platforms": config.platforms_summary(),
        "features": config.features,
        "extra_args": config.extra_args,
        "steps": {
            "update": config.steps.update,
            "clean": config.steps.clean,
            "regenerate_core_project_files": config.steps.regenerate_core_project_files,
            "pack_content": config.steps.pack_content,
            "check": config.steps.check,
            "build": config.steps.build,
            "run": config.steps.run,
        },
        "summary": config.subtitle(),
    })
}

/// The configuration a name or id picks.
fn find_config(wanted: &str) -> Result<BuildConfiguration, String> {
    ensure_loaded();
    let configs = build_configurations();
    let configs = configs.read();
    let lower = wanted.trim().to_lowercase();
    configs
        .configs()
        .iter()
        .find(|c| c.id == wanted || c.name.trim().to_lowercase() == lower)
        .cloned()
        .ok_or_else(|| {
            let names: Vec<&str> = configs.configs().iter().map(|c| c.name.as_str()).collect();
            format!(
                "no build configuration named `{wanted}`; the configurations are: {}",
                names.join(", ")
            )
        })
}

fn configurations(_: &str) -> ToolResult {
    ensure_loaded();
    let configs = build_configurations();
    let configs = configs.read();
    let selected = configs.selected_id().map(str::to_owned);
    let list: Vec<Value> = configs
        .configs()
        .iter()
        .map(|c| config_json(c, selected.as_deref() == Some(c.id.as_str())))
        .collect();
    ToolResult::json(&json!({
        "configurations": list,
        "profiles": ["debug", "release", "shipping"],
        "note": "build_start runs the selected configuration unless it names another. 'Build & Run' builds the game and launches it; 'Build' only builds.",
    }))
}

fn set_mode(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    ensure_loaded();
    if let Some(wanted) = arguments.get("configuration").and_then(Value::as_str) {
        match find_config(wanted) {
            Ok(config) => build_configurations().update(|c| c.select(&config.id)),
            Err(message) => return ToolResult::error(message),
        }
    }
    if let Some(profile) = arguments.get("profile").and_then(Value::as_str) {
        let profile = match profile.to_ascii_lowercase().as_str() {
            "debug" => BuildProfile::Debug,
            "release" => BuildProfile::Release,
            "shipping" => BuildProfile::Shipping,
            other => {
                return ToolResult::error(format!(
                    "`profile` is debug, release or shipping, not `{other}`"
                ));
            }
        };
        let handle = build_configurations();
        let Some(id) = handle.read().selected_id().map(str::to_owned) else {
            return ToolResult::error("no configuration is selected; pass `configuration`");
        };
        handle.update(|c| c.edit(&id, |config| config.profile = profile));
    }
    persist();
    let handle = build_configurations();
    let configs = handle.read();
    match configs.selected() {
        Some(config) => ToolResult::json(&json!({ "selected": config_json(config, true) })),
        None => ToolResult::error("no build configuration is selected"),
    }
}

/// Start a build. Returns once it is under way (the work is an editor task).
fn start(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    ensure_loaded();
    let config = match arguments.get("configuration").and_then(Value::as_str) {
        Some(wanted) => match find_config(wanted) {
            Ok(config) => config,
            Err(message) => return ToolResult::error(message),
        },
        None => match build_configurations().read().selected().cloned() {
            Some(config) => config,
            None => return ToolResult::error("no build configuration is selected"),
        },
    };
    let mut config = config;
    // `run` overrides whether the game is launched, so one configuration serves
    // "build only" and "build and run".
    if let Some(run) = arguments.get("run").and_then(Value::as_bool) {
        config.steps.run = run;
        if run {
            config.steps.build = true;
        }
    }
    let state = playback().get();
    if state.build_running {
        return ToolResult::error("a build is already running; build_status shows it, build_stop cancels it");
    }
    if state.game_running {
        return ToolResult::error("the game a previous run launched is still running; build_stop stops it");
    }
    let name = config.name.clone();
    let summary = config.subtitle();
    let started = ui_common::ui_call::call(Duration::from_secs(20), move |cx| {
        let window = cx
            .active_window()
            .or_else(|| cx.windows().into_iter().next())
            .ok_or_else(|| "the editor has no window to run the build from".to_owned())?;
        cx.update_window(window, |_, window, cx| run_configuration(&config, window, cx))
            .map_err(|e| e.to_string())
    });
    match started {
        Ok(Ok(())) => {
            // The runner publishes the session as it begins.
            let task = session::current().and_then(|s| s.task());
            ToolResult::json(&json!({
                "started": name,
                "summary": summary,
                "build_running": playback().get().build_running,
                "task_id": task.map(|t| t.get()),
                "next": "poll build_status; read the output with build_output or logs_wait (task_title \"Build\")",
            }))
        }
        Ok(Err(message)) | Err(message) => ToolResult::error(message),
    }
}

fn stop(_: &str) -> ToolResult {
    let was_building = playback().get().build_running;
    if was_building {
        cancel_build();
    }
    let stopped = stop_game();
    ToolResult::json(&json!({
        "cancelled_build": was_building,
        "stopped_game_instances": stopped,
    }))
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Succeeded => "succeeded",
        Outcome::Failed => "failed",
        Outcome::Cancelled => "cancelled",
    }
}

fn game_task_id() -> Option<u64> {
    editor_task_queue::global()
        .snapshots()
        .into_iter()
        .filter(|task| task.category == "Game")
        .map(|task| task.id.get())
        .max()
}

fn status(_: &str) -> ToolResult {
    let state = playback().get();
    ensure_loaded();
    let selected = build_configurations()
        .read()
        .selected()
        .map(|c| config_json(c, true));
    let build = session::current().map(|session| {
        json!({
            "name": session.name,
            "outcome": outcome_name(session.outcome()),
            "step": session.step.lock().clone(),
            "detail": session.detail.lock().clone(),
            "progress_percent": session.pct.load(std::sync::atomic::Ordering::Relaxed),
            "elapsed_seconds": session.elapsed().as_secs_f64(),
            "task_id": session.task().map(|t| t.get()),
        })
    });
    let findings: Vec<Value> = pulsar_findings::snapshot()
        .iter()
        .filter(|(source, _)| matches!(source.as_str(), "Build" | "Game"))
        .flat_map(|(_, set)| set.iter())
        .map(|finding| {
            json!({ "title": finding.title, "detail": finding.detail, "key": finding.key })
        })
        .collect();
    ToolResult::json(&json!({
        "build_running": state.build_running,
        "game_running": state.game_running,
        "play_phase": format!("{:?}", state.phase),
        "selected_configuration": selected,
        "last_build": build,
        "game_task_id": game_task_id(),
        "problems": findings,
    }))
}

fn output(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    let which = arguments
        .get("which")
        .and_then(Value::as_str)
        .unwrap_or("build");
    let task = match which {
        "build" => session::current().and_then(|s| s.task()).map(|t| t.get()),
        "game" => game_task_id(),
        other => return ToolResult::error(format!("`which` is build or game, not `{other}`")),
    };
    let Some(task) = task else {
        return ToolResult::error(format!(
            "no {which} has run in this editor session yet; build_start starts one"
        ));
    };
    let Some(log) = editor_task_queue::log_of_task(task) else {
        return ToolResult::error(format!("the {which} task's log is gone (task {task})"));
    };
    let cursor = arguments
        .get("after_cursor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tail = arguments
        .get("tail")
        .and_then(Value::as_u64)
        .map_or(200, |n| n as usize)
        .clamp(1, 5_000);
    let read = log.read_from(cursor);
    let skip = read.lines.len().saturating_sub(tail);
    let lines: Vec<&String> = read.lines.iter().skip(skip).collect();
    let mut result = ToolResult::json(&json!({
        "task_id": task,
        "lines": lines,
        "next_cursor": read.next_cursor,
        "dropped": read.dropped,
    }));
    if let Some(pulsar_mcp::Content::Text(text)) = result.content.first_mut() {
        *text = format!(
            "{} {which} line(s) of task {task}; next_cursor {}\n{}",
            lines.len(),
            read.next_cursor,
            lines.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n")
        );
    }
    result
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_configurations",
        title: "List build configurations",
        description: "The project's build configurations (the entries of the toolbar's Build dropdown): id, name, profile (debug, release or shipping), platforms, and the steps each runs (update, clean, regenerate project files, pack content, check, build, run). Exactly one is selected. 'Build & Run' builds the game and launches it; 'Build' only builds.",
        input_schema: r#"{"type":"object","properties":{}}"#,
        annotations: Annotations::READ_ONLY,
        call: configurations,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_set_mode",
        title: "Select a build configuration or profile",
        description: "Select which build configuration the Build button (and build_start) runs, and/or set the selected configuration's profile: debug (fast, unoptimised), release (optimised) or shipping (fat LTO, for distribution). Saved to the project's .pulsar/build_configurations.toml. Does not start a build.",
        input_schema: r#"{"type":"object","properties":{"configuration":{"type":"string","description":"The name or id of a configuration to select (build_configurations lists them), e.g. \"Build & Run\"."},"profile":{"type":"string","enum":["debug","release","shipping"],"description":"Set the (selected) configuration's profile."}}}"#,
        annotations: Annotations::WRITES_IDEMPOTENT,
        call: set_mode,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_start",
        title: "Start a build (and run the game)",
        description: "Start a build as the toolbar's Build button does: regenerate the project's game crate, cook and pack its content, `cargo build` it, and (when the configuration runs, or `run` is true) launch the game. Returns once the build is under way; it runs as an editor task and opens the Build Dashboard. Then poll build_status and read build_output (or logs_wait with task_title \"Build\"). Refused while a build or the launched game is running. Starts compiling the whole game on the first run: minutes.",
        input_schema: r#"{"type":"object","properties":{"configuration":{"type":"string","description":"Name or id of the configuration to run; the selected one by default."},"run":{"type":"boolean","description":"true: build and launch the game; false: build only. Overrides the configuration."}}}"#,
        annotations: Annotations::WRITES,
        call: start,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_stop",
        title: "Cancel the build and stop the game",
        description: "Cancel a running build (the current cargo command is killed and no further step runs) and stop the game a run launched, every instance and the processes it started. Does nothing when neither is running.",
        input_schema: r#"{"type":"object","properties":{}}"#,
        annotations: Annotations::DESTRUCTIVE,
        call: stop,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_status",
        title: "Build status",
        description: "Whether a build or the launched game is running, the selected configuration, the latest build (name, outcome running/succeeded/failed/cancelled, current step and detail, progress percent, elapsed seconds, its task id) and the failures the Problems panel holds for the build and the game (a failed build's compiler errors, a game crash).",
        input_schema: r#"{"type":"object","properties":{}}"#,
        annotations: Annotations::READ_ONLY,
        call: status,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "build_output",
        title: "Build or game output",
        description: "The output of the latest build (cargo's lines, the content packer's, each compiler error and script error on its own line) or of the game it launched (everything the game prints on stdout and stderr, instance-tagged). Returns the lines and next_cursor; pass it back as after_cursor to read only what is new.",
        input_schema: r#"{"type":"object","properties":{"which":{"type":"string","enum":["build","game"],"description":"build (default) or game."},"after_cursor":{"type":"integer","description":"Only lines after this cursor (the next_cursor of an earlier call)."},"tail":{"type":"integer","description":"At most this many lines, the newest (default 200, at most 5000)."}}}"#,
        annotations: Annotations::READ_ONLY,
        call: output,
    }
}
