//! Parsing LSP edits and code actions into the UI's neutral types
//! (`ui::diagnostics::{CodeAction, TextEdit}`: 1-based line and column).

use serde_json::Value;
use ui::diagnostics::{CodeAction, TextEdit};

use crate::path_utils::uri_to_path;

/// One LSP `TextEdit` as a [`TextEdit`] of `file_path`.
pub fn parse_text_edit(text_edit: &Value, file_path: &str) -> Option<TextEdit> {
    let range = text_edit.get("range")?;
    let new_text = text_edit.get("newText")?.as_str()?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(TextEdit {
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
pub fn edits_in_workspace_edit(edit: &Value) -> Vec<TextEdit> {
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
            for text_edit in change.get("edits").and_then(Value::as_array).into_iter().flatten() {
                edits.extend(parse_text_edit(text_edit, &file));
            }
        }
    }
    edits
}

/// A code action with its edits (`None`: it carries none, so it is either
/// unresolved or a command).
pub fn parse_single_code_action(action: &Value) -> Option<CodeAction> {
    let title = action
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Unknown action")
        .to_string();
    let edits = edits_in_workspace_edit(action.get("edit")?);
    (!edits.is_empty()).then_some(CodeAction { title, edits })
}

/// The code actions of a `textDocument/codeAction` result that already carry
/// their edits.
pub fn parse_code_actions(result: &Value) -> Vec<CodeAction> {
    result
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_single_code_action)
        .collect()
}

/// The code actions of a result that have `data` but no `edit`: they need
/// `codeAction/resolve` to produce one.
pub fn get_unresolved_actions(result: &Value) -> Vec<Value> {
    result
        .as_array()
        .into_iter()
        .flatten()
        .filter(|action| action.get("data").is_some() && action.get("edit").is_none())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edits_name_decoded_paths_and_one_based_positions() {
        let edit = json!({ "changes": { "file:///c%3A/a%20b/x.rs": [{
            "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 1, "character": 0 } },
            "newText": "y"
        }]}});
        let edits = edits_in_workspace_edit(&edit);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].file_path, "c:/a b/x.rs");
        assert_eq!((edits[0].start_line, edits[0].start_column), (1, 3));
        assert_eq!((edits[0].end_line, edits[0].end_column), (2, 1));
    }

    #[test]
    fn document_changes_and_resource_operations() {
        let edit = json!({ "documentChanges": [
            { "kind": "create", "uri": "file:///a/new.rs" },
            { "textDocument": { "uri": "file:///a/x.rs", "version": 3 }, "edits": [{
                "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 1 } },
                "newText": ""
            }]}
        ]});
        assert_eq!(edits_in_workspace_edit(&edit).len(), 1);
    }

    #[test]
    fn unresolved_actions_have_data_but_no_edit() {
        let result = json!([
            { "title": "a", "data": 1 },
            { "title": "b", "edit": { "changes": {} } },
            { "title": "c", "data": 2, "edit": { "changes": {} } }
        ]);
        assert_eq!(get_unresolved_actions(&result).len(), 1);
        assert!(parse_code_actions(&result).is_empty(), "no edits in any of them");
    }
}

/// A spec's [`QuickFix`](pulsar_language::QuickFix) as the UI's code action.
pub fn code_action_of(fix: pulsar_language::QuickFix) -> CodeAction {
    CodeAction {
        title: fix.title,
        edits: fix
            .edits
            .into_iter()
            .map(|edit| TextEdit {
                file_path: edit.file_path,
                start_line: edit.start_line,
                start_column: edit.start_column,
                end_line: edit.end_line,
                end_column: edit.end_column,
                new_text: edit.new_text,
            })
            .collect(),
    }
}

/// A [`TextEdit`] as a quick fix's edit.
pub fn quick_edit_of(edit: TextEdit) -> pulsar_language::QuickFixEdit {
    pulsar_language::QuickFixEdit {
        file_path: edit.file_path,
        start_line: edit.start_line,
        start_column: edit.start_column,
        end_line: edit.end_line,
        end_column: edit.end_column,
        new_text: edit.new_text,
    }
}
