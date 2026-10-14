//! Run the engine's agent tools as an MCP stdio server.
//!
//! ```text
//! pulsar_mcp_server [--workspace <dir>] [--allow-write] [--allow-tool <name>]... [--list]
//! ```
//!
//! Register it with an MCP client, for example in `.mcp.json`:
//! `{"mcpServers":{"pulsar":{"command":"pulsar_mcp_server","args":["--workspace","."]}}}`
//!
//! This is a headless host: tools that need the live editor (open tabs, scene
//! edits) report that the editor is unavailable. Logs go to stderr; stdout is
//! protocol only.

use std::{collections::BTreeSet, io, path::PathBuf};

use agent_chat_tools::{build_default_registry, make_tool_context, PulsarToolExtras};
use agent_mcp::{exposed_tools, serve, ServerOptions};
use agent_runtime::ToolClassifier;

fn main() {
    let mut workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut opts = ServerOptions::default();
    let mut list_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => match args.next() {
                Some(dir) => workspace = PathBuf::from(dir),
                None => return usage("--workspace needs a directory"),
            },
            "--allow-write" => opts.allow_write = true,
            "--allow-tool" => match args.next() {
                Some(name) => {
                    opts.extra_allowed.insert(name);
                }
                None => return usage("--allow-tool needs a tool name"),
            },
            "--list" => list_only = true,
            "-h" | "--help" => return usage(""),
            other => return usage(&format!("unknown argument: {other}")),
        }
    }

    // The tools read engine state through the global context.
    engine_state::EngineContext::new().set_global();
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let ctx = make_tool_context(workspace, None, PulsarToolExtras::default());
    let registry = build_default_registry();
    let classifier = ToolClassifier::new();

    if list_only {
        let names: BTreeSet<String> = exposed_tools(&registry, &classifier, &opts).into_iter().collect();
        for name in names {
            println!("{name}");
        }
        return;
    }

    let stdin = io::stdin();
    if let Err(err) = serve(&registry, &ctx, &classifier, &opts, stdin.lock(), io::stdout().lock()) {
        eprintln!("pulsar_mcp_server: {err}");
        std::process::exit(1);
    }
}

fn usage(problem: &str) {
    if !problem.is_empty() {
        eprintln!("error: {problem}");
    }
    eprintln!(
        "usage: pulsar_mcp_server [--workspace <dir>] [--allow-write] [--allow-tool <name>]... [--list]"
    );
    if !problem.is_empty() {
        std::process::exit(2);
    }
}
