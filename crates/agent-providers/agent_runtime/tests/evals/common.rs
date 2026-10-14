//! Helpers shared by the evals.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{ChatMessage, ChatProvider};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{EventSink, ModelProfile, RunConfig, RunEvent};

pub fn init_engine() {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
}

pub fn config(provider: Arc<dyn ChatProvider>, profile: ModelProfile) -> RunConfig {
    init_engine();
    RunConfig::new(provider, "eval-model", ToolRegistry::new(), PathBuf::from("."))
        .with_profile(profile)
}

pub fn sink() -> (EventSink, Arc<Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    (sink, events)
}

/// A request as plain text: role, tool-call plumbing and content, one block
/// per message. This is the form golden files store.
pub fn render(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for (i, m) in messages.iter().enumerate() {
        out.push_str(&format!("--- [{i}] {:?}", m.role));
        if let Some(id) = &m.tool_call_id {
            out.push_str(&format!(" tool_call_id={id}"));
        }
        out.push('\n');
        for call in &m.tool_calls {
            out.push_str(&format!(
                "  call {} {} {}\n",
                call.id, call.name, call.arguments_json
            ));
        }
        out.push_str(&m.content);
        out.push('\n');
    }
    out
}

/// Compare `actual` to `tests/evals/golden/<name>.txt`. Run with
/// `UPDATE_GOLDEN=1` to (re)write the file after an intended change.
pub fn assert_golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/evals/golden")
        .join(format!("{name}.txt"));
    let actual = actual.replace("\r\n", "\n");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with UPDATE_GOLDEN=1 to create it",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert_eq!(
        expected,
        actual,
        "golden mismatch for {name}; if intended, rerun with UPDATE_GOLDEN=1"
    );
}
