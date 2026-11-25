//! The server over its real transport: a client on a TCP socket initializes,
//! lists tools, calls one, and is refused without the token or from a foreign
//! origin or host.

use std::io::{Read, Write};
use std::net::TcpStream;

use pulsar_mcp::{Annotations, McpConfig, McpServer, McpToolRegistration, ToolResult};
use serde_json::{Value, json};

fn echo(arguments: &str) -> ToolResult {
    let arguments: Value = serde_json::from_str(arguments).unwrap();
    ToolResult::json(&json!({ "echo": arguments["text"] }))
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "echo_test",
        title: "Echo",
        description: "Returns its `text` argument.",
        input_schema: r#"{"type":"object","properties":{"text":{"type":"string","description":"what to echo"}},"required":["text"]}"#,
        annotations: Annotations::READ_ONLY,
        call: echo,
    }
}

struct Reply {
    status: u16,
    body: String,
}

/// One raw HTTP exchange.
fn exchange(port: u16, method: &str, headers: &[(&str, String)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut request = format!("{method} /mcp HTTP/1.1\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    Reply { status, body }
}

fn authed(port: u16, token: &str, body: &Value) -> Reply {
    exchange(
        port,
        "POST",
        &[
            ("Host", format!("127.0.0.1:{port}")),
            ("Authorization", format!("Bearer {token}")),
            ("Content-Type", "application/json".into()),
        ],
        &body.to_string(),
    )
}

#[test]
fn a_client_initializes_lists_and_calls_a_registered_tool() {
    let server = McpServer::start(McpConfig::default()).unwrap();
    let port = server.local_addr().port();
    assert!(server.local_addr().ip().is_loopback(), "bound to loopback only");
    let token = server.token().to_owned();

    let init = authed(
        port,
        &token,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
    );
    assert_eq!(init.status, 200);
    let init: Value = serde_json::from_str(&init.body).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "pulsar-engine");

    let note = authed(port, &token, &json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    assert_eq!(note.status, 202);

    let list = authed(port, &token, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list: Value = serde_json::from_str(&list.body).unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "echo_test")
        .expect("the registered tool is listed");
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["inputSchema"]["required"][0], "text");

    let call = authed(
        port,
        &token,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo_test","arguments":{"text":"hi"}}}),
    );
    let call: Value = serde_json::from_str(&call.body).unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["structuredContent"]["echo"], "hi");

    // A missing required argument is a tool error that says which.
    let call = authed(
        port,
        &token,
        &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo_test","arguments":{}}}),
    );
    let call: Value = serde_json::from_str(&call.body).unwrap();
    assert_eq!(call["result"]["isError"], true);
    assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("`text`"));
    server.stop();
}

#[test]
fn requests_without_the_token_or_from_elsewhere_are_refused() {
    let server = McpServer::start(McpConfig::default()).unwrap();
    let port = server.local_addr().port();
    let token = server.token().to_owned();
    let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string();
    let host = ("Host", format!("127.0.0.1:{port}"));

    // No token, a wrong token, a token of the wrong scheme.
    assert_eq!(exchange(port, "POST", &[host.clone()], &ping).status, 401);
    let wrong = ("Authorization", "Bearer nope".to_owned());
    assert_eq!(exchange(port, "POST", &[host.clone(), wrong], &ping).status, 401);
    let basic = ("Authorization", format!("Basic {token}"));
    assert_eq!(exchange(port, "POST", &[host.clone(), basic], &ping).status, 401);

    let bearer = ("Authorization", format!("Bearer {token}"));
    // DNS rebinding: a foreign Host or Origin, even with the right token.
    let foreign_host = ("Host", format!("evil.example:{port}"));
    assert_eq!(
        exchange(port, "POST", &[foreign_host, bearer.clone()], &ping).status,
        403
    );
    let origin = ("Origin", "https://evil.example".to_owned());
    assert_eq!(
        exchange(port, "POST", &[host.clone(), bearer.clone(), origin], &ping).status,
        403
    );
    let local_origin = ("Origin", "http://localhost:5173".to_owned());
    assert_eq!(
        exchange(port, "POST", &[host.clone(), bearer.clone(), local_origin], &ping).status,
        200
    );

    // There is no stream or session to offer.
    assert_eq!(exchange(port, "GET", &[host, bearer], "").status, 405);
    server.stop();
}

#[test]
fn the_endpoint_file_names_the_server_and_goes_when_it_stops() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mcp").join("endpoint.json");
    let server = McpServer::start(McpConfig {
        endpoint_file: Some(file.clone()),
        ..McpConfig::default()
    })
    .unwrap();
    let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(written["url"], server.url());
    assert_eq!(written["token"], server.token());
    assert_eq!(
        written["client_config"]["mcpServers"]["pulsar"]["headers"]["Authorization"],
        format!("Bearer {}", server.token())
    );
    server.stop();
    assert!(!file.exists(), "the endpoint file is removed on stop");
}
