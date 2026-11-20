//! `WorkspaceEdit`: parsing it and applying it, language-neutral.
//!
//! Rename, code actions, refactors and `workspace/applyEdit` all end in a
//! `WorkspaceEdit`. [`apply_workspace_edit`] plans every step first (text
//! edits are computed against the current text of the file, the open buffer
//! when an editor has it, so a bad edit refuses the whole change before
//! anything is written), then commits in order:
//!
//! * a file an editor has open is edited **in the buffer**, as one undo group
//!   per file (the editor implements [`BufferEdits`]);
//! * any other file is read and written through the filesystem library;
//! * create, rename and delete operations go through it too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Global};
use serde_json::Value;

use crate::path_utils::{path_key, uri_to_path};

/// One text edit: zero-based line, UTF-16 column (LSP positions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspTextEdit {
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub new_text: String,
}

impl LspTextEdit {
    fn parse(value: &Value) -> Option<Self> {
        let range = value.get("range")?;
        let position = |value: &Value| -> Option<(u32, u32)> {
            Some((
                value.get("line")?.as_u64()? as u32,
                value.get("character")?.as_u64()? as u32,
            ))
        };
        Some(Self {
            start: position(range.get("start")?)?,
            end: position(range.get("end")?)?,
            new_text: value.get("newText")?.as_str()?.to_owned(),
        })
    }
}

/// One step of a `WorkspaceEdit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    Edit {
        path: PathBuf,
        edits: Vec<LspTextEdit>,
    },
    Create {
        path: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        path: PathBuf,
        ignore_if_not_exists: bool,
    },
}

/// A `WorkspaceEdit` as ordered steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceEdit {
    pub changes: Vec<FileChange>,
}

impl WorkspaceEdit {
    /// Parse the LSP value: `documentChanges` (text-document edits and
    /// resource operations, in order) when present, else `changes`.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let mut changes = Vec::new();
        let option = |op: &Value, name: &str| {
            op.get("options")
                .and_then(|options| options.get(name))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        if let Some(document_changes) = value.get("documentChanges").and_then(Value::as_array) {
            for change in document_changes {
                match change.get("kind").and_then(Value::as_str) {
                    Some("create") => changes.push(FileChange::Create {
                        path: uri_of(change, "uri")?,
                        overwrite: option(change, "overwrite"),
                        ignore_if_exists: option(change, "ignoreIfExists"),
                    }),
                    Some("rename") => changes.push(FileChange::Rename {
                        from: uri_of(change, "oldUri")?,
                        to: uri_of(change, "newUri")?,
                        overwrite: option(change, "overwrite"),
                        ignore_if_exists: option(change, "ignoreIfExists"),
                    }),
                    Some("delete") => changes.push(FileChange::Delete {
                        path: uri_of(change, "uri")?,
                        ignore_if_not_exists: option(change, "ignoreIfNotExists"),
                    }),
                    Some(other) => return Err(format!("unknown resource operation `{other}`")),
                    None => {
                        let uri = change
                            .get("textDocument")
                            .and_then(|document| document.get("uri"))
                            .and_then(Value::as_str)
                            .ok_or("a document change without a textDocument.uri")?;
                        changes.push(FileChange::Edit {
                            path: uri_to_path(uri),
                            edits: parse_edits(change.get("edits"))?,
                        });
                    }
                }
            }
        } else if let Some(map) = value.get("changes").and_then(Value::as_object) {
            for (uri, edits) in map {
                changes.push(FileChange::Edit {
                    path: uri_to_path(uri),
                    edits: parse_edits(Some(edits))?,
                });
            }
        }
        Ok(Self { changes })
    }

    /// The files this edit touches, once each.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        let mut files = Vec::new();
        for change in &self.changes {
            let paths: Vec<&PathBuf> = match change {
                FileChange::Edit { path, .. }
                | FileChange::Create { path, .. }
                | FileChange::Delete { path, .. } => vec![path],
                FileChange::Rename { from, to, .. } => vec![from, to],
            };
            for path in paths {
                if seen.insert(path_key(path)) {
                    files.push(path.clone());
                }
            }
        }
        files
    }
}

fn uri_of(change: &Value, field: &str) -> Result<PathBuf, String> {
    change
        .get(field)
        .and_then(Value::as_str)
        .map(uri_to_path)
        .ok_or_else(|| format!("a resource operation without `{field}`"))
}

fn parse_edits(value: Option<&Value>) -> Result<Vec<LspTextEdit>, String> {
    value
        .and_then(Value::as_array)
        .ok_or("text edits are not a list")?
        .iter()
        .map(|edit| LspTextEdit::parse(edit).ok_or_else(|| format!("malformed text edit: {edit}")))
        .collect()
}

/// Byte offset of `(line, utf16 column)` in `text`, or an error when the
/// position is outside it. A column past the end of its line clamps to the
/// line's end, as the protocol asks of servers' stale positions.
fn offset_of(text: &str, line_starts: &[usize], (line, column): (u32, u32)) -> Result<usize, String> {
    let line = line as usize;
    let Some(&start) = line_starts.get(line) else {
        // The position just past the last line terminator is a (virtual)
        // empty line; anything further is out of range.
        return if line == line_starts.len() {
            Ok(text.len())
        } else {
            Err(format!("line {line} is past the end of the file"))
        };
    };
    let end = line_starts.get(line + 1).copied().unwrap_or(text.len());
    let content = &text[start..end];
    let mut remaining = column as usize;
    let mut offset = 0;
    for ch in content.chars() {
        if ch == '\n' || ch == '\r' {
            break;
        }
        let units = ch.len_utf16();
        if remaining < units {
            break;
        }
        remaining -= units;
        offset += ch.len_utf8();
    }
    Ok(start + offset)
}

fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => starts.push(i + 1),
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                starts.push(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    starts
}

/// `text` with `edits` applied. Edits are against the original text, must
/// not overlap, and inserts at one position keep their order.
pub fn apply_text_edits(text: &str, edits: &[LspTextEdit]) -> Result<String, String> {
    let starts = line_starts(text);
    let mut resolved: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = offset_of(text, &starts, edit.start)?;
        let end = offset_of(text, &starts, edit.end)?;
        if end < start {
            return Err(format!("an edit ends before it starts ({:?} to {:?})", edit.start, edit.end));
        }
        resolved.push((start, end, edit.new_text.as_str()));
    }
    // Stable: inserts at the same position stay in array order.
    resolved.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, new_text) in resolved {
        if start < at {
            return Err("two edits overlap".to_string());
        }
        out.push_str(&text[at..start]);
        out.push_str(new_text);
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// The editor's side: the buffers it has open. `C` is whatever context the
/// host passes (GPUI's `App`).
pub trait BufferEdits<C> {
    /// The text of `path` when an editor has it open.
    fn text(&self, path: &Path, cx: &mut C) -> Option<String>;
    /// Apply `edits` to the open buffer of `path` as one undo group.
    fn apply(&self, path: &Path, edits: &[LspTextEdit], cx: &mut C) -> Result<(), String>;
}

/// The files a plan changes, for the caller's report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub edited_buffers: Vec<PathBuf>,
    pub written_files: Vec<PathBuf>,
    pub resource_operations: usize,
}

enum Step {
    Buffer { path: PathBuf, edits: Vec<LspTextEdit> },
    Write { path: PathBuf, text: String },
    Create { path: PathBuf },
    Rename { from: PathBuf, to: PathBuf, overwrite: bool },
    Delete { path: PathBuf },
}

/// Apply `edit`. Every step is planned (and so validated) before the first
/// is committed.
pub fn apply_workspace_edit<C>(
    edit: &WorkspaceEdit,
    buffers: Option<&dyn BufferEdits<C>>,
    cx: &mut C,
) -> Result<Applied, String> {
    // The text each file will hold after the steps planned so far; `None`
    // marks a file the plan deletes or moves away.
    let mut staged: HashMap<String, Option<String>> = HashMap::new();
    let mut steps = Vec::new();

    for change in &edit.changes {
        match change {
            FileChange::Edit { path, edits } => {
                let key = path_key(path);
                if let Some(entry) = staged.get(&key) {
                    let text = entry
                        .as_deref()
                        .ok_or_else(|| format!("{} is deleted by this edit", path.display()))?;
                    let updated = apply_text_edits(text, edits)?;
                    staged.insert(key, Some(updated.clone()));
                    steps.push(Step::Write { path: path.clone(), text: updated });
                    continue;
                }
                let open = buffers.and_then(|host| host.text(path, cx));
                match open {
                    Some(text) => {
                        // Validate against the buffer now; the buffer applies
                        // the edits itself.
                        apply_text_edits(&text, edits).map_err(|e| format!("{}: {e}", path.display()))?;
                        steps.push(Step::Buffer { path: path.clone(), edits: edits.clone() });
                    }
                    None => {
                        let text = engine_fs::virtual_fs::read_to_string(path)
                            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                        let updated =
                            apply_text_edits(&text, edits).map_err(|e| format!("{}: {e}", path.display()))?;
                        staged.insert(key, Some(updated.clone()));
                        steps.push(Step::Write { path: path.clone(), text: updated });
                    }
                }
            }
            FileChange::Create { path, overwrite, ignore_if_exists } => {
                let exists = staged
                    .get(&path_key(path))
                    .map(|entry| entry.is_some())
                    .unwrap_or_else(|| engine_fs::virtual_fs::exists(path).unwrap_or(false));
                if exists && !overwrite {
                    if *ignore_if_exists {
                        continue;
                    }
                    return Err(format!("{} already exists", path.display()));
                }
                staged.insert(path_key(path), Some(String::new()));
                steps.push(Step::Create { path: path.clone() });
            }
            FileChange::Rename { from, to, overwrite, ignore_if_exists } => {
                let target_exists = staged
                    .get(&path_key(to))
                    .map(|entry| entry.is_some())
                    .unwrap_or_else(|| engine_fs::virtual_fs::exists(to).unwrap_or(false));
                if target_exists && !overwrite {
                    if *ignore_if_exists {
                        continue;
                    }
                    return Err(format!("{} already exists", to.display()));
                }
                let moved = match staged.remove(&path_key(from)) {
                    Some(text) => text,
                    None => {
                        if !engine_fs::virtual_fs::exists(from).unwrap_or(false) {
                            return Err(format!("{} does not exist", from.display()));
                        }
                        // Content of an unstaged file is read at commit; edits
                        // after a rename of it are planned against the disk.
                        staged.insert(path_key(from), None);
                        engine_fs::virtual_fs::read_to_string(from).ok()
                    }
                };
                staged.insert(path_key(to), moved);
                steps.push(Step::Rename { from: from.clone(), to: to.clone(), overwrite: *overwrite });
            }
            FileChange::Delete { path, ignore_if_not_exists } => {
                let exists = staged
                    .get(&path_key(path))
                    .map(|entry| entry.is_some())
                    .unwrap_or_else(|| engine_fs::virtual_fs::exists(path).unwrap_or(false));
                if !exists {
                    if *ignore_if_not_exists {
                        continue;
                    }
                    return Err(format!("{} does not exist", path.display()));
                }
                staged.insert(path_key(path), None);
                steps.push(Step::Delete { path: path.clone() });
            }
        }
    }

    let mut applied = Applied::default();
    for step in steps {
        match step {
            Step::Buffer { path, edits } => {
                let host = buffers.ok_or("no editor to edit the open buffer")?;
                host.apply(&path, &edits, cx)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                applied.edited_buffers.push(path);
            }
            Step::Write { path, text } => {
                engine_fs::virtual_fs::write_file_atomically(&path, text.as_bytes())
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                if !applied.written_files.contains(&path) {
                    applied.written_files.push(path);
                }
            }
            Step::Create { path } => {
                if let Some(parent) = path.parent() {
                    engine_fs::virtual_fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
                engine_fs::virtual_fs::write_file_atomically(&path, b"")
                    .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
                applied.resource_operations += 1;
            }
            Step::Rename { from, to, overwrite } => {
                if overwrite && engine_fs::virtual_fs::exists(&to).unwrap_or(false) {
                    engine_fs::virtual_fs::delete_path(&to)
                        .map_err(|e| format!("cannot replace {}: {e}", to.display()))?;
                }
                if let Some(parent) = to.parent() {
                    engine_fs::virtual_fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
                engine_fs::virtual_fs::rename(&from, &to)
                    .map_err(|e| format!("cannot rename {}: {e}", from.display()))?;
                applied.resource_operations += 1;
            }
            Step::Delete { path } => {
                engine_fs::virtual_fs::delete_path(&path)
                    .map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
                applied.resource_operations += 1;
            }
        }
    }
    Ok(applied)
}

/// The editor's buffers as a GPUI global, set by whoever owns the editors
/// (the code editor). Without one, edits go to disk.
#[derive(Clone)]
pub struct EditHost(pub Arc<dyn BufferEdits<App>>);

impl Global for EditHost {}

/// Apply `edit` against the editor buffers registered as the [`EditHost`].
pub fn apply_workspace_edit_in_app(edit: &WorkspaceEdit, cx: &mut App) -> Result<Applied, String> {
    let host = cx.try_global::<EditHost>().map(|host| host.0.clone());
    apply_workspace_edit(edit, host.as_deref(), cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> LspTextEdit {
        LspTextEdit { start, end, new_text: text.to_string() }
    }

    #[test]
    fn edits_apply_against_the_original_text_in_any_order() {
        let text = "let a = 1;\nlet b = 2;\n";
        let result = apply_text_edits(
            text,
            &[edit((1, 4), (1, 5), "bee"), edit((0, 4), (0, 5), "ay")],
        )
        .unwrap();
        assert_eq!(result, "let ay = 1;\nlet bee = 2;\n");
    }

    #[test]
    fn columns_count_utf16_units() {
        // 'é' is one unit, '😀' is two.
        let text = "é😀x\n";
        let result = apply_text_edits(text, &[edit((0, 3), (0, 4), "Y")]).unwrap();
        assert_eq!(result, "é😀Y\n");
    }

    #[test]
    fn inserts_at_one_position_keep_their_order_and_overlaps_are_refused() {
        let result = apply_text_edits(
            "ab",
            &[edit((0, 1), (0, 1), "1"), edit((0, 1), (0, 1), "2")],
        )
        .unwrap();
        assert_eq!(result, "a12b");
        assert!(apply_text_edits("abcd", &[edit((0, 0), (0, 3), ""), edit((0, 2), (0, 4), "")]).is_err());
        assert!(apply_text_edits("abc", &[edit((5, 0), (5, 1), "")]).is_err());
    }

    #[test]
    fn crlf_files_and_the_position_after_the_last_line() {
        let text = "a\r\nb\r\n";
        assert_eq!(apply_text_edits(text, &[edit((1, 0), (1, 1), "B")]).unwrap(), "a\r\nB\r\n");
        assert_eq!(apply_text_edits(text, &[edit((2, 0), (2, 0), "c")]).unwrap(), "a\r\nb\r\nc");
    }

    #[test]
    fn a_workspace_edit_parses_changes_and_document_changes_with_resource_operations() {
        let simple = WorkspaceEdit::from_value(&json!({ "changes": { "file:///p/a.rs": [{
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
            "newText": "x"
        }]}}))
        .unwrap();
        assert_eq!(simple.changes.len(), 1);
        let rich = WorkspaceEdit::from_value(&json!({ "documentChanges": [
            { "kind": "create", "uri": "file:///p/n.rs", "options": { "ignoreIfExists": true } },
            { "textDocument": { "uri": "file:///p/n.rs", "version": null }, "edits": [] },
            { "kind": "rename", "oldUri": "file:///p/n.rs", "newUri": "file:///p/m.rs" },
            { "kind": "delete", "uri": "file:///p/old.rs" }
        ]}))
        .unwrap();
        assert_eq!(rich.changes.len(), 4);
        assert!(matches!(rich.changes[0], FileChange::Create { ignore_if_exists: true, .. }));
        assert_eq!(rich.files().len(), 3);
        assert!(WorkspaceEdit::from_value(&json!({ "documentChanges": [{ "kind": "explode", "uri": "x" }] })).is_err());
    }

    struct FakeBuffers {
        open: RefCell<HashMap<String, String>>,
        applied: RefCell<Vec<(PathBuf, usize)>>,
    }

    impl BufferEdits<()> for FakeBuffers {
        fn text(&self, path: &Path, _: &mut ()) -> Option<String> {
            self.open.borrow().get(&path_key(path)).cloned()
        }

        fn apply(&self, path: &Path, edits: &[LspTextEdit], _: &mut ()) -> Result<(), String> {
            let key = path_key(path);
            let text = self.open.borrow().get(&key).cloned().ok_or("not open")?;
            let updated = apply_text_edits(&text, edits)?;
            self.open.borrow_mut().insert(key, updated);
            self.applied.borrow_mut().push((path.to_path_buf(), edits.len()));
            Ok(())
        }
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulsar_lsp_edit_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Open buffers take the edit in the buffer (one call per file); closed
    /// files change on disk; resource operations run; a bad edit anywhere
    /// refuses everything before any file is touched.
    #[test]
    fn open_buffers_closed_files_and_resource_operations_in_one_edit() {
        let dir = temp("apply");
        let on_disk = dir.join("disk.rs");
        let in_buffer = dir.join("buffer.rs");
        std::fs::write(&on_disk, "let x = 1;\n").unwrap();
        std::fs::write(&in_buffer, "stale on disk\n").unwrap();
        let buffers = FakeBuffers {
            open: RefCell::new(HashMap::from([(path_key(&in_buffer), "let y = 2;\n".to_string())])),
            applied: RefCell::new(Vec::new()),
        };
        let renamed = dir.join("renamed.rs");
        let workspace = WorkspaceEdit {
            changes: vec![
                FileChange::Edit { path: on_disk.clone(), edits: vec![edit((0, 4), (0, 5), "xx")] },
                FileChange::Edit {
                    path: in_buffer.clone(),
                    edits: vec![edit((0, 4), (0, 5), "yy"), edit((0, 8), (0, 9), "3")],
                },
                FileChange::Create { path: dir.join("new.rs"), overwrite: false, ignore_if_exists: false },
                FileChange::Rename { from: on_disk.clone(), to: renamed.clone(), overwrite: false, ignore_if_exists: false },
            ],
        };
        let applied = apply_workspace_edit(&workspace, Some(&buffers), &mut ()).unwrap();
        assert_eq!(applied.edited_buffers, [in_buffer.clone()]);
        assert_eq!(
            buffers.applied.borrow().as_slice(),
            [(in_buffer.clone(), 2)],
            "both edits of the file are one buffer call (one undo group)"
        );
        assert_eq!(buffers.open.borrow()[&path_key(&in_buffer)], "let yy = 3;\n");
        assert_eq!(std::fs::read_to_string(&in_buffer).unwrap(), "stale on disk\n", "the buffer was not written behind the editor's back");
        assert!(dir.join("new.rs").exists());
        assert!(!on_disk.exists());
        assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "let xx = 1;\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_edit_refuses_the_whole_change_before_anything_is_written() {
        let dir = temp("refuse");
        let good = dir.join("good.rs");
        let bad = dir.join("bad.rs");
        std::fs::write(&good, "abc\n").unwrap();
        std::fs::write(&bad, "abc\n").unwrap();
        let workspace = WorkspaceEdit {
            changes: vec![
                FileChange::Edit { path: good.clone(), edits: vec![edit((0, 0), (0, 1), "Z")] },
                FileChange::Edit { path: bad, edits: vec![edit((9, 0), (9, 1), "Z")] },
            ],
        };
        assert!(apply_workspace_edit::<()>(&workspace, None, &mut ()).is_err());
        assert_eq!(std::fs::read_to_string(&good).unwrap(), "abc\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_steps_on_one_file_compose() {
        let dir = temp("compose");
        let file = dir.join("f.rs");
        std::fs::write(&file, "abc").unwrap();
        let workspace = WorkspaceEdit {
            changes: vec![
                FileChange::Edit { path: file.clone(), edits: vec![edit((0, 0), (0, 1), "A")] },
                FileChange::Edit { path: file.clone(), edits: vec![edit((0, 3), (0, 3), "!")] },
            ],
        };
        apply_workspace_edit::<()>(&workspace, None, &mut ()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Abc!");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
