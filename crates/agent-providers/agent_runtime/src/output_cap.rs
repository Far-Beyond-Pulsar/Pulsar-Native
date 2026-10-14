//! Hard caps on tool output. An oversized result is cut to head + tail and
//! the full text is kept under a short `ref`; the model reads more of it with
//! the `fetch_output` meta tool.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use agent_chat_core::ToolDefinition;
use serde_json::{json, Value};

pub const FETCH_OUTPUT: &str = "fetch_output";

/// Entries and bytes the process-wide store keeps before dropping the oldest.
const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 16 * 1024 * 1024;

static NEXT_REF: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub struct OutputStore {
    entries: Mutex<VecDeque<(String, Arc<str>)>>,
}

impl OutputStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The store the run loop uses; refs stay valid across runs of a session.
    pub fn global() -> &'static OutputStore {
        static GLOBAL: std::sync::OnceLock<OutputStore> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(OutputStore::new)
    }

    pub fn put(&self, content: &str) -> String {
        let reference = format!("out-{}", NEXT_REF.fetch_add(1, Ordering::Relaxed));
        let Ok(mut entries) = self.entries.lock() else {
            return reference;
        };
        entries.push_back((reference.clone(), Arc::from(content)));
        let mut bytes: usize = entries.iter().map(|(_, text)| text.len()).sum();
        while entries.len() > MAX_ENTRIES || (bytes > MAX_BYTES && entries.len() > 1) {
            if let Some((_, dropped)) = entries.pop_front() {
                bytes -= dropped.len();
            }
        }
        reference
    }

    pub fn get(&self, reference: &str) -> Option<Arc<str>> {
        self.entries
            .lock()
            .ok()?
            .iter()
            .find(|(r, _)| r == reference)
            .map(|(_, text)| text.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().map_or(true, |e| e.is_empty())
    }

    /// Read `length` characters from `offset` of a stored output.
    pub fn fetch(&self, reference: &str, offset: usize, length: usize) -> Result<String, String> {
        let text = self
            .get(reference)
            .ok_or_else(|| format!("unknown or expired ref \"{reference}\""))?;
        let total = text.chars().count();
        if offset >= total {
            return Err(format!("offset {offset} is past the end ({total} chars)"));
        }
        let window: String = text.chars().skip(offset).take(length).collect();
        let end = offset + window.chars().count();
        let more = if end < total {
            format!("\n[{} more chars: fetch_output ref=\"{reference}\" offset={end}]", total - end)
        } else {
            "\n[end of output]".to_string()
        };
        Ok(format!("{window}{more}"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Capped {
    pub text: String,
    /// Set when the text was cut.
    pub reference: Option<String>,
}

/// Cut `content` to at most `max_chars` characters (marker included),
/// keeping the start and the end, which is where errors and totals live.
pub fn cap_output(content: &str, max_chars: usize, store: &OutputStore) -> Capped {
    let total = content.chars().count();
    if total <= max_chars {
        return Capped {
            text: content.to_string(),
            reference: None,
        };
    }
    let reference = store.put(content);
    let marker = format!(
        "\n[... {} of {} chars omitted; fetch_output ref=\"{}\" offset=N to read more ...]\n",
        "0000000", total, reference
    );
    let marker_len = marker.chars().count();
    let room = max_chars.saturating_sub(marker_len).max(16);
    let head_len = room * 2 / 3;
    let tail_len = room - head_len;
    let omitted = total.saturating_sub(head_len + tail_len);
    let marker = format!(
        "\n[... {omitted} of {total} chars omitted; fetch_output ref=\"{reference}\" offset=N to read more ...]\n"
    );
    let head: String = content.chars().take(head_len).collect();
    let tail: String = content.chars().skip(total - tail_len).collect();
    Capped {
        text: format!("{head}{marker}{tail}"),
        reference: Some(reference),
    }
}

/// Definition of the `fetch_output` meta tool.
pub fn fetch_output_definition() -> ToolDefinition {
    ToolDefinition {
        name: FETCH_OUTPUT.to_string(),
        description: Some("Read more of a cut-off tool result by its ref.".to_string()),
        parameters_json_schema: json!({
            "type": "object",
            "properties": {
                "ref": {"type": "string"},
                "offset": {"type": "integer"},
                "length": {"type": "integer"}
            },
            "required": ["ref"]
        }),
    }
}

/// Run `fetch_output`. `max_chars` bounds the window so the answer itself
/// never needs capping.
pub fn run_fetch_output(args: &Value, store: &OutputStore, max_chars: usize) -> Result<String, String> {
    let reference = args
        .get("ref")
        .and_then(Value::as_str)
        .ok_or("fetch_output needs a \"ref\"")?;
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let length = args
        .get("length")
        .and_then(Value::as_u64)
        .map_or(max_chars, |l| (l as usize).min(max_chars));
    store.fetch(reference, offset, length.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_passes_through() {
        let store = OutputStore::new();
        let capped = cap_output("short", 100, &store);
        assert_eq!(capped.text, "short");
        assert!(capped.reference.is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn large_output_keeps_head_and_tail_with_a_ref() {
        let store = OutputStore::new();
        let content = format!("HEAD{}TAIL", "x".repeat(10_000));
        let capped = cap_output(&content, 400, &store);
        assert!(capped.text.chars().count() <= 400);
        assert!(capped.text.starts_with("HEAD"));
        assert!(capped.text.ends_with("TAIL"));
        let reference = capped.reference.expect("ref");
        assert!(capped.text.contains(&reference));
        assert!(capped.text.contains("omitted"));
        assert_eq!(&*store.get(&reference).unwrap(), content.as_str());
    }

    #[test]
    fn fetch_pages_through_the_original() {
        let store = OutputStore::new();
        let content: String = (0..2_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let capped = cap_output(&content, 300, &store);
        let reference = capped.reference.unwrap();

        let first = run_fetch_output(&json!({"ref": reference, "offset": 0, "length": 100}), &store, 300)
            .unwrap();
        assert!(first.starts_with(&content[..100]));
        assert!(first.contains("offset=100"));

        let last = run_fetch_output(&json!({"ref": reference, "offset": 1_950}), &store, 300).unwrap();
        assert!(last.starts_with(&content[1_950..]));
        assert!(last.contains("end of output"));

        assert!(run_fetch_output(&json!({"ref": reference, "offset": 5_000}), &store, 300).is_err());
        assert!(run_fetch_output(&json!({"ref": "out-nope"}), &store, 300).is_err());
        assert!(run_fetch_output(&json!({}), &store, 300).is_err());
    }

    #[test]
    fn capping_respects_multibyte_text() {
        let store = OutputStore::new();
        let content = "漢字".repeat(1_000);
        let capped = cap_output(&content, 200, &store);
        assert!(capped.text.chars().count() <= 200);
    }

    #[test]
    fn store_evicts_the_oldest() {
        let store = OutputStore::new();
        let first = store.put("one");
        for _ in 0..MAX_ENTRIES {
            store.put("filler");
        }
        assert!(store.get(&first).is_none());
    }
}
