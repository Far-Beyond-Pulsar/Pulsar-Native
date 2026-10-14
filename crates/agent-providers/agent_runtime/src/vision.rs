//! Getting tool-result images to the model that can use them.
//!
//! Tools such as `capture_viewport` and `run_and_observe` write PNGs to
//! `.pulsar/agent_captures/` and name them by ref in their JSON result. The
//! ledger stores only that text, so history stays small and persists as is.
//! Just before a request is sent, [`attach_tool_images`] resolves the refs in
//! the newest tool results:
//!
//! - a vision model gets the files as [`ImagePart`]s on the tool message
//!   (only the newest [`MAX_IMAGES_IN_REQUEST`], since images are expensive
//!   and a stale screenshot is worth little);
//! - any other model keeps the text and gets a one-line statement that the
//!   image was not shown, so it cannot pretend to have seen it.

use std::path::Path;

use agent_chat_core::{ChatMessage, ChatRole, ImagePart};
use agent_chat_tools::visual::{scan_capture_refs, CaptureStore};
use base64::Engine as _;

/// Images sent per request, newest first.
pub const MAX_IMAGES_IN_REQUEST: usize = 4;
/// A larger file is not sent.
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

const NOT_SHOWN: &str = "[image not shown: this model cannot see images. Use the log, errors and \
                         metrics above; do not describe what the image contains.]";
const DROPPED: &str = "[older image omitted to save context; capture again for a fresh view.]";
const UNREADABLE: &str = "[image file is missing or too large to attach.]";

/// What [`attach_tool_images`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttachReport {
    pub attached: usize,
    /// Tool results that mention captures a text-only model cannot see.
    pub described_as_text: usize,
}

fn load(store: &CaptureStore, id: &str) -> Option<ImagePart> {
    let path = store.path_for(id)?;
    if std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    Some(ImagePart::png(
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

/// Attach capture images to `messages` (a projected request) for a vision
/// model, or annotate their absence for a text-only one.
pub fn attach_tool_images(
    messages: &mut [ChatMessage],
    vision: bool,
    workspace_root: &Path,
) -> AttachReport {
    let store = CaptureStore::new(workspace_root);
    let mut report = AttachReport::default();
    // Newest first, so the budget is spent on the freshest captures.
    for message in messages.iter_mut().rev() {
        if message.role != ChatRole::Tool {
            continue;
        }
        let refs = scan_capture_refs(&message.content);
        if refs.is_empty() {
            continue;
        }
        if !vision {
            message.content.push('\n');
            message.content.push_str(NOT_SHOWN);
            report.described_as_text += 1;
            continue;
        }
        let mut missing = false;
        let mut over_budget = false;
        for id in &refs {
            if report.attached >= MAX_IMAGES_IN_REQUEST {
                over_budget = true;
                break;
            }
            match load(&store, id) {
                Some(image) => {
                    message.images.push(image);
                    report.attached += 1;
                }
                None => missing = true,
            }
        }
        if over_budget {
            message.content.push('\n');
            message.content.push_str(DROPPED);
        } else if missing {
            message.content.push('\n');
            message.content.push_str(UNREADABLE);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Tool,
            content: content.to_string(),
            tool_call_id: Some("c".into()),
            tool_calls: vec![],
            images: vec![],
        }
    }

    fn capture(store: &CaptureStore, bytes: &[u8]) -> String {
        let (id, path) = store.allocate().unwrap();
        std::fs::write(path, bytes).unwrap();
        id
    }

    fn result_json(ids: &[&str]) -> String {
        let captures: Vec<_> = ids.iter().map(|id| serde_json::json!({ "ref": id })).collect();
        serde_json::json!({ "ok": true, "captures": captures }).to_string()
    }

    #[test]
    fn vision_models_get_the_file_as_base64() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaptureStore::new(dir.path());
        let id = capture(&store, b"PNGDATA");
        let mut messages = vec![tool_message(&result_json(&[&id]))];
        let report = attach_tool_images(&mut messages, true, dir.path());
        assert_eq!(report.attached, 1);
        assert_eq!(messages[0].images.len(), 1);
        assert_eq!(messages[0].images[0].mime_type, "image/png");
        assert_eq!(messages[0].images[0].data_base64, "UE5HREFUQQ==");
        assert!(!messages[0].content.contains("not shown"));
    }

    #[test]
    fn text_only_models_are_told_the_image_was_not_shown() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaptureStore::new(dir.path());
        let id = capture(&store, b"x");
        let original = result_json(&[&id]);
        let mut messages = vec![tool_message(&original)];
        let report = attach_tool_images(&mut messages, false, dir.path());
        assert_eq!(report.attached, 0);
        assert_eq!(report.described_as_text, 1);
        assert!(messages[0].images.is_empty());
        assert!(messages[0].content.starts_with(&original));
        assert!(messages[0].content.contains("cannot see images"));
    }

    #[test]
    fn only_the_newest_images_are_sent() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaptureStore::new(dir.path());
        let ids: Vec<String> = (0..6).map(|_| capture(&store, b"i")).collect();
        let mut messages: Vec<ChatMessage> =
            ids.iter().map(|id| tool_message(&result_json(&[id]))).collect();
        let report = attach_tool_images(&mut messages, true, dir.path());
        assert_eq!(report.attached, MAX_IMAGES_IN_REQUEST);
        // The two oldest lost their image and say so.
        assert!(messages[0].images.is_empty() && messages[0].content.contains("older image"));
        assert!(messages[1].images.is_empty());
        assert_eq!(messages[5].images.len(), 1);
    }

    #[test]
    fn missing_oversized_and_forged_refs_attach_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaptureStore::new(dir.path());
        let big = capture(&store, &vec![0u8; (MAX_IMAGE_BYTES + 1) as usize]);
        let mut messages = vec![
            tool_message(&result_json(&[&big])),
            tool_message(&result_json(&["cap_1_1"])),
            // A path in the arguments is not a ref; nothing to resolve.
            tool_message(r#"{"path":"../../secrets.png"}"#),
        ];
        let report = attach_tool_images(&mut messages, true, dir.path());
        assert_eq!(report.attached, 0);
        assert!(messages[0].content.contains("missing or too large"));
        assert!(messages[1].content.contains("missing or too large"));
        assert!(messages[2].images.is_empty() && !messages[2].content.contains('['));
    }

    #[test]
    fn launching_the_game_is_gated_and_captures_are_not_untrusted() {
        use crate::taint::{ToolClass, ToolClassifier};
        use std::path::Path;
        let classifier = ToolClassifier::new();
        // Launches a process: a Write tool, so Phase 5 policy confirms it.
        assert_eq!(classifier.classify("run_and_observe"), ToolClass::Write);
        assert_eq!(classifier.classify("capture_viewport"), ToolClass::Read);
        for name in ["run_and_observe", "capture_viewport"] {
            assert!(
                !classifier.taints_on_result(name, &serde_json::json!({}), Path::new("/p")),
                "{name} must not taint the run"
            );
        }
    }

    #[test]
    fn other_roles_and_plain_results_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaptureStore::new(dir.path());
        let id = capture(&store, b"x");
        let mut user = tool_message(&result_json(&[&id]));
        user.role = ChatRole::User;
        let plain = tool_message("{\"ok\":true}");
        let mut messages = vec![user.clone(), plain.clone()];
        let report = attach_tool_images(&mut messages, true, dir.path());
        assert_eq!(report, AttachReport::default());
        assert_eq!(messages[0].content, user.content);
        assert_eq!(messages[1].content, plain.content);
    }
}
