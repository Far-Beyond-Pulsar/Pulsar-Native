use super::process::serve;
use super::*;
use crate::protocol::fake::*;
use crate::protocol::{Handler, RequestOutcome, METHOD_NOT_FOUND};
use pulsar_language::spec::{section_of, LocateError, RootStrategy, ServerHooks};

fn spec() -> &'static LanguageServerSpec {
    Box::leak(Box::new(LanguageServerSpec {
        id: "fake",
        display_name: "Fake",
        find: |_| None,
        locate: |_| Err(LocateError::new("not used")),
        roots: RootStrategy::ProjectRoot,
        initialization_options: |_| json!({ "section": { "key": 7 } }),
        configuration: |_, section| section_of(&json!({ "section": { "key": 7 } }), section),
        client_capabilities: || Value::Null,
        hooks: ServerHooks::DEFAULT,
        readiness: pulsar_language::spec::ReadinessRule::Immediate,
        max_indexing: Duration::from_secs(5),
        eager: false,
        commands: &[],
        prepare_workspace: None,
        installer: None,
    }))
}

struct Fixture {
    _task: editor_task_queue::PersistentTask,
    handler: ServerHandler,
    rx: flume::Receiver<Msg>,
    written: flume::Receiver<Vec<u8>>,
}

fn fixture() -> Fixture {
    let (tx, rx) = flume::unbounded();
    let (stdin, written) = flume::unbounded();
    let persistent = editor_task_queue::global().persistent_task(
        editor_task_queue::TaskDescription::persistent("test", "test"),
        None,
    );
    let task = persistent.context().clone();
    let spec = spec();
    Fixture {
        _task: persistent,
        handler: ServerHandler {
            spec,
            context: Arc::new(ServerContext {
                root: PathBuf::from("/p"),
                project_root: PathBuf::from("/p"),
                settings: ServerSettings::default(),
            }),
            tx,
            connection: Connection::new(PipeWriter(stdin), "t"),
            task,
            label: "t".into(),
        },
        rx,
        written,
    }
}

#[test]
fn registration_progress_creation_and_refresh_requests_are_acknowledged_with_null() {
    let mut f = fixture();
    for method in [
        "client/registerCapability",
        "window/workDoneProgress/create",
        "workspace/semanticTokens/refresh",
        "workspace/inlayHint/refresh",
    ] {
        assert_eq!(
            f.handler.request(&json!(1), method, &json!({})),
            RequestOutcome::Reply(Value::Null),
            "{method}"
        );
    }
    let _ = &f.written;
}

#[test]
fn workspace_configuration_is_answered_per_item_from_the_spec() {
    let mut f = fixture();
    let reply = f.handler.request(
        &json!(2),
        "workspace/configuration",
        &json!({ "items": [{ "section": "section.key" }, { "section": "missing" }] }),
    );
    assert_eq!(reply, RequestOutcome::Reply(json!([7, null])));
}

#[test]
fn unknown_requests_are_method_not_found_and_apply_edit_is_forwarded_not_answered() {
    let mut f = fixture();
    assert!(matches!(
        f.handler.request(&json!(3), "x/unknown", &json!({})),
        RequestOutcome::Error(METHOD_NOT_FOUND, _)
    ));
    let outcome = f
        .handler
        .request(&json!("e1"), "workspace/applyEdit", &json!({ "edit": {} }));
    assert_eq!(outcome, RequestOutcome::Deferred);
    match f.rx.try_recv().unwrap() {
        Msg::ApplyEdit { id, .. } => assert_eq!(id, json!("e1")),
        _ => panic!("expected the edit to be forwarded to the UI thread"),
    }
}

#[test]
fn progress_and_diagnostics_notifications_are_decoded_for_the_entity() {
    let mut f = fixture();
    f.handler.notification(
        "$/progress",
        json!({ "token": 12, "value": { "kind": "begin", "title": "Indexing", "percentage": 0 } }),
    );
    match f.rx.try_recv().unwrap() {
        Msg::Progress {
            token, kind, title, ..
        } => {
            assert_eq!(token, "12");
            assert_eq!(kind, ProgressKind::Begin);
            assert_eq!(title.as_deref(), Some("Indexing"));
        }
        _ => panic!("expected progress"),
    }
    f.handler.notification(
        "textDocument/publishDiagnostics",
        json!({ "uri": "file:///p/a.fk", "diagnostics": [] }),
    );
    match f.rx.try_recv().unwrap() {
        Msg::Diagnostics { path, diagnostics } => {
            assert_eq!(path, PathBuf::from("/p/a.fk"));
            assert!(
                diagnostics.is_empty(),
                "an empty publish is delivered so it can clear"
            );
        }
        _ => panic!("expected diagnostics"),
    }
}

#[test]
fn a_location_answer_decodes_to_a_path_and_position() {
    let result = json!([{ "targetUri": "file:///p/a.rs", "targetSelectionRange": {
        "start": { "line": 4, "character": 2 }, "end": { "line": 4, "character": 5 }
    }}]);
    assert_eq!(
        location_target(&result),
        Some((PathBuf::from("/p/a.rs"), 4, 2))
    );
}

/// A scripted server on a pipe: the handshake, then the server's own
/// requests (one with an id that collides with a pending client id),
/// progress, an edit request, and the end of the stream.
#[test]
fn a_scripted_server_handshakes_is_answered_and_forwards_what_the_ui_needs() {
    use crate::protocol::read_message;
    let mut wire = wire();
    let (tx, rx) = flume::unbounded();
    let persistent = editor_task_queue::global().persistent_task(
        editor_task_queue::TaskDescription::persistent("scripted", "test"),
        None,
    );
    let task = persistent.context().clone();
    let context = Arc::new(ServerContext {
        root: PathBuf::from("/p"),
        project_root: PathBuf::from("/p"),
        settings: ServerSettings::default(),
    });
    let connection = wire.connection.clone();
    let reader = wire.client_reader.take().unwrap();
    let serving = std::thread::spawn(move || {
        serve(spec(), context, &tx, &task, connection, reader);
    });

    // 1. The client's `initialize` (id 1).
    let initialize = read_message(&mut wire.from_client).unwrap().unwrap();
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(initialize["params"]["rootUri"], "file:///p");
    assert_eq!(
        initialize["params"]["capabilities"]["workspace"]["applyEdit"],
        true
    );
    send(
        &wire.to_client,
        json!({
            "jsonrpc": "2.0", "id": initialize["id"],
            "result": { "capabilities": { "completionProvider": { "triggerCharacters": ["."] } } }
        }),
    );
    // 2. `initialized` follows, then the entity hears of the capabilities.
    let initialized = read_message(&mut wire.from_client).unwrap().unwrap();
    assert_eq!(initialized["method"], "initialized");
    match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
        Msg::Initialized { capabilities } => {
            assert_eq!(
                capabilities["completionProvider"]["triggerCharacters"][0],
                "."
            );
        }
        _ => panic!("expected the capabilities"),
    }

    // 3. The server asks for things with ids that collide with ours: both
    //    are answered, and neither is mistaken for a response.
    let request = wire
        .connection
        .request("textDocument/hover", json!({}))
        .unwrap();
    let ours = read_message(&mut wire.from_client).unwrap().unwrap();
    send(
        &wire.to_client,
        json!({
            "jsonrpc": "2.0", "id": ours["id"], "method": "client/registerCapability", "params": {}
        }),
    );
    let reply = read_message(&mut wire.from_client).unwrap().unwrap();
    assert_eq!(reply["id"], ours["id"]);
    assert_eq!(reply["result"], Value::Null);
    send(
        &wire.to_client,
        json!({
            "jsonrpc": "2.0", "id": 77, "method": "workspace/configuration",
            "params": { "items": [{ "section": "section.key" }] }
        }),
    );
    let reply = read_message(&mut wire.from_client).unwrap().unwrap();
    assert_eq!(reply["id"], 77);
    assert_eq!(reply["result"], json!([7]));
    assert!(
        request.try_recv().is_err(),
        "still waiting for its own response"
    );
    send(
        &wire.to_client,
        json!({ "jsonrpc": "2.0", "id": ours["id"], "result": "hovered" }),
    );
    assert_eq!(
        request
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()["result"],
        "hovered"
    );

    // 4. Progress and an edit request reach the UI side; the edit request
    //    is answered later, by the UI.
    send(
        &wire.to_client,
        json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "end" } }
        }),
    );
    assert!(matches!(
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
        Msg::Progress {
            kind: ProgressKind::End,
            ..
        }
    ));
    send(
        &wire.to_client,
        json!({
            "jsonrpc": "2.0", "id": "edit-1", "method": "workspace/applyEdit", "params": { "edit": {} }
        }),
    );
    let Msg::ApplyEdit { id, .. } = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()
    else {
        panic!("expected the edit request");
    };
    wire.connection
        .respond(&id, Ok(json!({ "applied": true })))
        .unwrap();
    let reply = read_message(&mut wire.from_client).unwrap().unwrap();
    assert_eq!(reply["id"], "edit-1");
    assert_eq!(reply["result"]["applied"], true);

    // 5. The stream ends: the reader returns and the connection is closed.
    drop(wire.to_client);
    serving.join().unwrap();
    assert!(wire.connection.is_closed());
    drop(persistent);
}
