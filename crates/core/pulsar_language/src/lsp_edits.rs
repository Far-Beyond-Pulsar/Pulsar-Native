//! Reading LSP edits without the editor: `file://` URIs and the text edits of
//! a `WorkspaceEdit`, as [`QuickFixEdit`]s (one-based lines and columns).
//!
//! The one place these are parsed. A language plugin's server hooks use them
//! directly (no GPUI); `pulsar_lsp` builds its UI edits from them (#1945).

use std::path::PathBuf;

use serde_json::Value;

use crate::QuickFixEdit;

/// Decodes `%XX` escapes (servers send `file:///c%3A/Users/…`).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                decoded.push(value);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The filesystem path a `file://` URI names.
pub fn uri_to_path(uri: &str) -> PathBuf {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let decoded = percent_decode(rest);
    // `/C:/Users/…` is a Windows drive path with a leading slash.
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        PathBuf::from(&decoded[1..])
    } else {
        PathBuf::from(decoded)
    }
}

/// One LSP `TextEdit` (zero-based positions) as an edit of `file_path`.
pub fn parse_text_edit(text_edit: &Value, file_path: &str) -> Option<QuickFixEdit> {
    let range = text_edit.get("range")?;
    let new_text = text_edit.get("newText")?.as_str()?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(QuickFixEdit {
        file_path: file_path.to_string(),
        start_line: start.get("line")?.as_u64()? as usize + 1,
        start_column: start.get("character")?.as_u64()? as usize + 1,
        end_line: end.get("line")?.as_u64()? as usize + 1,
        end_column: end.get("character")?.as_u64()? as usize + 1,
        new_text: new_text.to_string(),
    })
}

fn path_string(uri: &str) -> String {
    uri_to_path(uri).to_string_lossy().into_owned()
}

/// Every text edit in a `WorkspaceEdit` (`changes` and the text-document
/// edits of `documentChanges`; resource operations carry none).
pub fn edits_in_workspace_edit(edit: &Value) -> Vec<QuickFixEdit> {
    let mut edits = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, list) in changes {
            let file = path_string(uri);
            for text_edit in list.as_array().into_iter().flatten() {
                edits.extend(parse_text_edit(text_edit, &file));
            }
        }
    }
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            let Some(uri) = change
                .get("textDocument")
                .and_then(|document| document.get("uri"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let file = path_string(uri);
            for text_edit in change
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                edits.extend(parse_text_edit(text_edit, &file));
            }
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_windows_uri_with_escapes_is_a_drive_path() {
        assert_eq!(
            uri_to_path("file:///c%3A/Users/a%20b/x.rs"),
            PathBuf::from("c:/Users/a b/x.rs")
        );
        assert_eq!(uri_to_path("file:///home/a/x.rs"), PathBuf::from("/home/a/x.rs"));
    }

    #[test]
    fn both_shapes_of_a_workspace_edit_give_one_based_edits() {
        let range = json!({ "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 7 } });
        let edit = json!({
            "changes": { "file:///home/a/x.rs": [{ "range": range, "newText": "new" }] },
            "documentChanges": [
                { "textDocument": { "uri": "file:///home/a/y.rs" }, "edits": [{ "range": range, "newText": "y" }] },
                { "kind": "create", "uri": "file:///home/a/z.rs" }
            ]
        });
        let edits = edits_in_workspace_edit(&edit);
        assert_eq!(edits.len(), 2);
        assert_eq!(
            (edits[0].start_line, edits[0].start_column, edits[0].end_column),
            (1, 5, 8)
        );
        assert_eq!(edits[1].new_text, "y");
    }
}
