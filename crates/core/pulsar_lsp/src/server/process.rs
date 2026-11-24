//! The server's service thread: locate, spawn, handshake and read.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::capabilities;
use crate::path_utils::path_to_uri;
use crate::protocol::{self, Connection};
use pulsar_language::spec::{LanguageServerSpec, LocateContext, ServerCommand, ServerContext};

use super::*;

/// The body of a server's service thread: locate, spawn, handshake, read.
pub(super) fn run_server(
    spec: &'static LanguageServerSpec,
    context: Arc<ServerContext>,
    child_slot: Arc<Mutex<Option<Child>>>,
    tx: flume::Sender<Msg>,
    task: &editor_task_queue::TaskContext,
    transport: Transport,
) {
    let label = format!("{}:{}", spec.id, context.root.display());

    task.set_phase("Preparing the workspace");
    if let Some(prepare) = spec.prepare_workspace {
        if let Err(error) = prepare(&context) {
            let _ = tx.send(Msg::Failed(format!(
                "could not prepare the workspace: {error}"
            )));
            return;
        }
    }

    if let Transport::Attached(reader, writer) = transport {
        // Already connected: no locating, no spawning, the same conversation.
        let connection = Connection::new(writer, label);
        connection.set_trace(context.settings.trace);
        let _ = tx.send(Msg::Spawned(connection.clone()));
        serve(
            spec,
            context,
            &tx,
            task,
            connection,
            std::io::BufReader::new(reader),
        );
        task.set_phase("Exited");
        let _ = tx.send(Msg::Exited("closed its connection".to_string()));
        return;
    }

    task.set_phase("Locating the server");
    let detail_tx = tx.clone();
    let progress = move |text: &str| {
        let _ = detail_tx.send(Msg::Detail(text.to_string()));
    };
    let locate = LocateContext::new(
        &context.project_root,
        context.settings.path.clone(),
        &progress,
    );
    let mut command: ServerCommand = match (spec.locate)(&locate) {
        Ok(command) => command,
        Err(error) => {
            let _ = tx.send(if error.missing {
                Msg::Missing(error.message)
            } else {
                Msg::Failed(error.message)
            });
            return;
        }
    };
    command
        .args
        .extend(context.settings.args.iter().map(std::ffi::OsString::from));
    tracing::info!(
        "[{label}] starting {:?} {:?}",
        command.program,
        command.args
    );

    task.set_phase("Spawning the process");
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&context.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // A console window must never flash for a server.
        process.creation_flags(0x0800_0000);
    }
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = tx.send(Msg::Failed(format!(
                "could not start {} ({}): {error}",
                spec.display_name,
                command.program.display()
            )));
            return;
        }
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = tx.send(Msg::Failed(
            "the server's pipes are not available".to_string(),
        ));
        return;
    };
    if let Ok(mut slot) = child_slot.lock() {
        *slot = Some(child);
    }

    // The server's stderr is its log: it lands in this task's log.
    let span = task.span();
    let stderr_label = label.clone();
    let _ = editor_task_queue::spawn_thread(
        editor_task_queue::ThreadSpec::new(
            format!("lsp stderr: {}", spec.id),
            editor_task_queue::ThreadRole::Io,
        )
        .owner("pulsar_lsp"),
        move || {
            use std::io::BufRead as _;
            span.in_scope(|| {
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(|l| l.ok())
                {
                    tracing::info!("[{stderr_label}] {line}");
                }
            });
        },
    );

    let connection = Connection::new(stdin, label.clone());
    connection.set_trace(context.settings.trace);
    let _ = tx.send(Msg::Spawned(connection.clone()));
    serve(
        spec,
        context,
        &tx,
        task,
        connection,
        std::io::BufReader::new(stdout),
    );

    // The stream ended: the process is exiting (or was killed).
    task.set_phase("Exited");
    let how = match child_slot.lock().ok().and_then(|mut slot| slot.take()) {
        Some(mut child) => {
            let status = wait_briefly(&mut child);
            match status {
                Some(status) if status.success() => "exited normally".to_string(),
                Some(status) => format!("exited with {status}"),
                None => "closed its output and was stopped".to_string(),
            }
        }
        // Stopped on purpose: the entity already took the child.
        None => "was stopped".to_string(),
    };
    tracing::info!("[{label}] {how}");
    let _ = tx.send(Msg::Exited(how));
}

/// The conversation with a connected server: the handshake, then reading
/// until the stream ends, answering what the server asks.
pub(super) fn serve(
    spec: &'static LanguageServerSpec,
    context: Arc<ServerContext>,
    tx: &flume::Sender<Msg>,
    task: &editor_task_queue::TaskContext,
    connection: Arc<Connection>,
    reader: impl std::io::BufRead,
) {
    let label = format!("{}:{}", spec.id, context.root.display());
    task.set_phase("Handshake");
    let init_tx = tx.clone();
    let init_connection = connection.clone();
    let initialize = initialize_params(spec, &context);
    let sent = connection.request_with("initialize", initialize, move |response| {
        if let Some(error) = response.get("error") {
            let _ = init_tx.send(Msg::Failed(format!("initialize failed: {error}")));
            return;
        }
        // `initialized` goes out before anything the entity sends next.
        let _ = init_connection.notify("initialized", json!({}));
        let capabilities = response
            .pointer("/result/capabilities")
            .cloned()
            .unwrap_or(Value::Null);
        let _ = init_tx.send(Msg::Initialized { capabilities });
    });
    if let Err(error) = sent {
        let _ = tx.send(Msg::Failed(format!(
            "could not write to the server: {error}"
        )));
        return;
    }

    task.set_phase("Serving");
    task.set_idle(true);
    let mut handler = ServerHandler {
        spec,
        context,
        tx: tx.clone(),
        connection: connection.clone(),
        task: task.clone(),
        label,
    };
    protocol::run_reader(reader, &connection, &mut handler);
}

/// Wait for a child whose output closed; stop it if it lingers.
fn wait_briefly(child: &mut Child) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn initialize_params(spec: &LanguageServerSpec, context: &ServerContext) -> Value {
    let root_uri = path_to_uri(&context.root);
    let name = context
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "Pulsar", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": root_uri,
        "workspaceFolders": [{ "uri": root_uri, "name": name }],
        "capabilities": capabilities::with_spec((spec.client_capabilities)()),
        "initializationOptions": spec.effective_initialization_options(context),
        "trace": context.settings.trace.as_str(),
    })
}
