//! Tools that describe the editor itself: its tasks and threads (the Tasks window),
//! its problems (the Problems panel) and its plugins, with the exact reason one did
//! not load (the Extensions window's data and the load report).

use std::time::UNIX_EPOCH;

use plugin_manager::LibraryOutcome;
use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use serde_json::{Value, json};

fn args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn millis(time: std::time::SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

// ---- tasks and threads ---------------------------------------------------------------

fn tasks_list(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    let status = arguments
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("all")
        .to_ascii_lowercase();
    let category = arguments.get("category").and_then(Value::as_str);
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(50, |n| n as usize)
        .clamp(1, 500);
    let mut tasks = editor_task_queue::global().snapshots();
    tasks.retain(|task| {
        let state = format!("{:?}", task.status).to_ascii_lowercase();
        let wanted = match status.as_str() {
            "all" => true,
            "active" => !task.status.is_finished(),
            other => state == other,
        };
        wanted
            && category.is_none_or(|c| task.category.eq_ignore_ascii_case(c))
            && title
                .as_ref()
                .is_none_or(|t| task.title.to_ascii_lowercase().contains(t))
    });
    // Newest first.
    tasks.sort_by_key(|task| std::cmp::Reverse(task.id.get()));
    let total = tasks.len();
    tasks.truncate(limit);
    let rows: Vec<Value> = tasks
        .iter()
        .map(|task| {
            json!({
                "id": task.id.get(),
                "title": task.title,
                "category": task.category,
                "status": format!("{:?}", task.status).to_lowercase(),
                "progress": task.progress,
                "detail": task.detail,
                "phase": task.phase,
                "error": task.error,
                "origin": task.origin,
                "worker": task.worker,
                "log_lines": task.log_lines,
                "idle_seconds": task.idle_for.as_secs_f64(),
                "stalled": task.stall.as_ref().map(|stall| stall.message.clone()),
                "submitted_ms": millis(task.submitted_at),
                "finished_ms": task.finished_at.map(millis),
            })
        })
        .collect();
    ToolResult::json(&json!({
        "tasks": rows,
        "matched": total,
        "note": "Read a task's log with logs_query (task_id) or logs_wait.",
    }))
}

fn threads_list(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    let owner = arguments.get("owner").and_then(Value::as_str);
    let threads: Vec<Value> = editor_task_queue::thread_snapshots()
        .iter()
        .filter(|thread| owner.is_none_or(|o| thread.owner.eq_ignore_ascii_case(o)))
        .map(|thread| {
            json!({
                "id": thread.id,
                "name": thread.name,
                "role": format!("{:?}", thread.role).to_lowercase(),
                "pool": thread.pool,
                "owner": thread.owner,
                "state": format!("{:?}", thread.state).to_lowercase(),
                "state_seconds": thread.state_for.as_secs_f64(),
                "alive_seconds": thread.alive_for.as_secs_f64(),
                "phase": thread.phase,
                "task_id": thread.task.map(|task| task.get()),
            })
        })
        .collect();
    ToolResult::json(&json!({ "threads": threads }))
}

// ---- problems ------------------------------------------------------------------------

fn problems_list(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    let category = arguments
        .get("category")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let min = arguments
        .get("min_severity")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let mut rows = Vec::new();
    for (source, set) in pulsar_findings::snapshot().iter() {
        for finding in set.iter() {
            if category
                .as_ref()
                .is_some_and(|c| finding.category.label().to_ascii_lowercase() != *c)
            {
                continue;
            }
            if let Some(min) = &min {
                let rank = |label: &str| match label {
                    "error" => 0,
                    "warning" | "warn" => 1,
                    _ => 2,
                };
                if rank(&finding.severity.label().to_ascii_lowercase()) > rank(min) {
                    continue;
                }
            }
            rows.push(json!({
                "source": source,
                "key": finding.key,
                "category": finding.category.label(),
                "severity": finding.severity.label(),
                "title": finding.title,
                "detail": finding.detail,
                "subject": finding.subject,
                "metric": finding.metric,
                "location": format!("{:?}", finding.location),
                "actions": finding.actions.iter().map(|a| a.label.clone()).collect::<Vec<_>>(),
            }));
        }
    }
    ToolResult::json(&json!({
        "findings": rows,
        "note": "The Problems panel's findings (project, performance, editor). Code diagnostics (compiler and language-server errors) are per file and not listed here.",
    }))
}

// ---- plugins -------------------------------------------------------------------------

fn plugins_list(arguments: &str) -> ToolResult {
    let arguments = args(arguments);
    let only_problems = arguments
        .get("only_problems")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let contributions = arguments
        .get("contributions")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let layout = pulsar_install_layout::InstallLayout::of_running_engine();
    let report = plugin_manager::load_report();
    let Some(manager) = plugin_manager::global() else {
        return ToolResult::error("the plugin manager is not running yet");
    };
    let catalog = manager.read().catalog();

    let libraries: Vec<Value> = report
        .libraries
        .iter()
        .map(|library| {
            let (status, reason, plugin_id, stage) = match &library.outcome {
                LibraryOutcome::Prepared => ("prepared", None, None, None),
                LibraryOutcome::Loaded { plugin_id } => {
                    ("loaded", None, Some(plugin_id.clone()), None)
                }
                LibraryOutcome::Failed { stage, reason } => {
                    ("failed", Some(reason.clone()), None, Some(*stage))
                }
            };
            json!({
                "file": library.file,
                "path": library.path.display().to_string(),
                "status": status,
                "plugin_id": plugin_id,
                "failed_at": stage,
                "reason": reason,
                "renamed_imports": library.renamed_imports,
            })
        })
        .collect();
    let failed = report
        .libraries
        .iter()
        .filter(|l| matches!(l.outcome, LibraryOutcome::Failed { .. }))
        .count();
    let renamed = report
        .libraries
        .iter()
        .filter(|l| !l.renamed_imports.is_empty())
        .count();

    let mut plugins: Vec<Value> = catalog
        .plugins
        .iter()
        .map(|plugin| {
            let library = catalog
                .libraries
                .iter()
                .find(|details| details.metadata.id.as_str() == plugin.descriptor.id);
            let mut by_category = serde_json::Map::new();
            for summary in &plugin.contributions {
                let entry = by_category
                    .entry(summary.category.to_owned())
                    .or_insert_with(|| json!({ "count": 0, "ids": [] }));
                entry["count"] = json!(entry["count"].as_u64().unwrap_or(0) + 1);
                if contributions {
                    if let Some(ids) = entry["ids"].as_array_mut() {
                        ids.push(json!(summary.id));
                    }
                }
            }
            json!({
                "id": plugin.descriptor.id,
                "name": plugin.descriptor.name,
                "root_crate": plugin.descriptor.root,
                "version": library.map(|l| l.metadata.version.to_string()),
                "library_path": library.map(|l| l.library_path.display().to_string()),
                "library_loaded": library.is_some(),
                "enabled_in_project": plugin.enabled,
                "editor_only": plugin.descriptor.editor_only,
                "requires": plugin.requirements.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>(),
                "contributes": by_category,
                "editors": library.map(|l| l.editors.clone()),
                "components": library.map(|l| l.components.clone()),
                "ai_tools": library.map(|l| l.ai_tools.clone()),
            })
        })
        .collect();
    if only_problems {
        plugins.retain(|p| {
            !p["library_loaded"].as_bool().unwrap_or(false)
                || !p["enabled_in_project"].as_bool().unwrap_or(true)
        });
    }
    let activation: Vec<String> = catalog
        .activation
        .as_ref()
        .map(|resolution| format!("{resolution:?}"))
        .into_iter()
        .collect();
    let problems: Vec<String> = catalog.problems.iter().map(|p| format!("{p:?}")).collect();

    ToolResult::json(&json!({
        "plugin_directory": layout.plugins_dir().display().to_string(),
        "directory_used_for_load": report.directory.as_ref().map(|d| d.display().to_string()),
        "directory_exists": report.directory_exists,
        "libraries_found": report.libraries.len(),
        "libraries_loaded": report.libraries.len() - failed,
        "libraries_failed": failed,
        "libraries_with_renamed_imports": renamed,
        "libraries": libraries,
        "plugins": plugins,
        "engine_contributions": catalog.engine.len(),
        "catalog_problems": problems,
        "activation": activation,
        "note": "`libraries` is what loading each library file did, with the exact reason for a failure (the OS loader's error with the imports the library is missing, an integrity or version refusal, a dependency or constructor failure) and the imports renamed for a shared library of another build (the plugin_compat copies). `plugins` is every installed plugin with whether its library loaded and whether the project enables it.",
    }))
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "tasks_list",
        title: "List editor tasks",
        description: "The editor tasks the Tasks window lists: every background job (builds, imports, loads, services) with id, title, category, status (queued/running/succeeded/failed/cancelled), progress, phase, error, origin (file:line that created it), log line count and a stall report if it is stuck. Newest first. Read one task's log with logs_query (task_id).",
        input_schema: r#"{"type":"object","properties":{"status":{"type":"string","enum":["all","active","queued","running","succeeded","failed","cancelled"],"description":"Which tasks (default all; active = not finished)."},"category":{"type":"string","description":"Only this category (Build, Game, fs, mcp, ui, ...)."},"title":{"type":"string","description":"Only tasks whose title contains this text."},"limit":{"type":"integer","description":"At most this many (default 50, at most 500)."}}}"#,
        annotations: Annotations::READ_ONLY,
        call: tasks_list,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "threads_list",
        title: "List editor threads",
        description: "The threads the editor registered (the Tasks window's Threads view): name, role (main, render, service, editor worker, tokio, rayon, io), pool, owner, state and how long, the phase it last reported and the task it is polling.",
        input_schema: r#"{"type":"object","properties":{"owner":{"type":"string","description":"Only threads of this owner (a crate or subsystem name)."}}}"#,
        annotations: Annotations::READ_ONLY,
        call: threads_list,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "problems_list",
        title: "List problems",
        description: "The Problems panel's findings: failed builds and game crashes, plugin resolution problems, performance findings (streaming, frame budget, renderer warnings) and editor findings (stalled tasks, UI freezes). Each has a source, category, severity, title, detail (what to do) and subject.",
        input_schema: r#"{"type":"object","properties":{"category":{"type":"string","enum":["code","performance","project","editor"],"description":"Only this tab."},"min_severity":{"type":"string","enum":["error","warning","info"],"description":"This severity and worse."}}}"#,
        annotations: Annotations::READ_ONLY,
        call: problems_list,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "plugins_list",
        title: "List plugins and why one did not load",
        description: "Every installed plugin (id, name, version, library path, whether its library loaded, whether the open project enables it, what it contributes by category) and what loading each library file did: the plugin directory in use and whether it exists, per library loaded or failed with the exact reason (the OS loader's error with the imports the library is missing, an integrity or version refusal, a dependency or constructor failure) and which libraries needed imports renamed for a shared library of another build (the plugin_compat copies). Use only_problems to see just what is missing.",
        input_schema: r#"{"type":"object","properties":{"only_problems":{"type":"boolean","description":"List only plugins whose library did not load or that the project disables."},"contributions":{"type":"boolean","description":"Also list every contributed id per category (long)."}}}"#,
        annotations: Annotations::READ_ONLY,
        call: plugins_list,
    }
}
