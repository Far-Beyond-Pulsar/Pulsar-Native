//! The documents one server has open, refcounted: any number of editors can
//! show a file; the server sees one `didOpen` for the first and one
//! `didClose` after the last. A document opened before the server has
//! answered `initialize` is held back (with its latest text) and announced
//! when [`Documents::flush`] runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::path_utils::{path_key, path_to_uri};

struct Doc {
    path: PathBuf,
    language_id: String,
    version: i32,
    holders: usize,
    /// `Some(text)` until the server has been told about the document.
    unsent_text: Option<String>,
}

/// What the caller must send for a document operation.
#[derive(Debug, Default, PartialEq)]
pub struct Notifications(pub Vec<(&'static str, Value)>);

#[derive(Default)]
pub struct Documents {
    docs: HashMap<String, Doc>,
}

impl Documents {
    /// An editor opened `path` with `text`. `connected`: the server can take
    /// notifications now (it answered `initialize`).
    pub fn open(
        &mut self,
        path: &Path,
        text: &str,
        language_id: &str,
        connected: bool,
    ) -> Notifications {
        let key = path_key(path);
        let doc = self.docs.entry(key).or_insert_with(|| Doc {
            path: path.to_path_buf(),
            language_id: language_id.to_string(),
            version: 1,
            holders: 0,
            unsent_text: None,
        });
        doc.holders += 1;
        if doc.holders > 1 {
            // Another editor has it open: the server has (or will get) the text.
            return Notifications::default();
        }
        if !connected {
            doc.unsent_text = Some(text.to_string());
            return Notifications::default();
        }
        Notifications(vec![("textDocument/didOpen", open_params(doc, text))])
    }

    /// The server is initialized: announce every document it has not seen.
    pub fn flush(&mut self) -> Notifications {
        let mut out = Vec::new();
        for doc in self.docs.values_mut() {
            if let Some(text) = doc.unsent_text.take() {
                out.push(("textDocument/didOpen", open_params(doc, &text)));
            }
        }
        Notifications(out)
    }

    /// Whether `path` is open (held by at least one editor).
    pub fn is_open(&self, path: &Path) -> bool {
        self.docs.contains_key(&path_key(path))
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// A full-text change. Before the server knows the document the text is
    /// simply the new text to open with.
    pub fn change_full(&mut self, path: &Path, text: &str, version: i32) -> Notifications {
        let Some(doc) = self.docs.get_mut(&path_key(path)) else {
            return Notifications::default();
        };
        doc.version = version;
        if doc.unsent_text.is_some() {
            doc.unsent_text = Some(text.to_string());
            return Notifications::default();
        }
        Notifications(vec![(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": [{ "text": text }],
            }),
        )])
    }

    /// Incremental changes (`range` + `text`, applied in order). `None`: the
    /// document is not yet known to the server, so the caller must send the
    /// full text instead (`change_full`).
    pub fn change_incremental(
        &mut self,
        path: &Path,
        version: i32,
        changes: Vec<Value>,
    ) -> Option<Notifications> {
        let doc = self.docs.get_mut(&path_key(path))?;
        doc.version = version;
        if doc.unsent_text.is_some() {
            return None;
        }
        if changes.is_empty() {
            return Some(Notifications::default());
        }
        Some(Notifications(vec![(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": changes,
            }),
        )]))
    }

    /// Whether the server has been told about `path`.
    pub fn is_announced(&self, path: &Path) -> bool {
        self.docs
            .get(&path_key(path))
            .is_some_and(|doc| doc.unsent_text.is_none())
    }

    /// An editor saved `path`.
    pub fn saved(&self, path: &Path, text: &str) -> Notifications {
        if !self.is_announced(path) {
            return Notifications::default();
        }
        Notifications(vec![(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": path_to_uri(path) }, "text": text }),
        )])
    }

    /// An editor closed `path`: `didClose` once the last one has.
    pub fn close(&mut self, path: &Path) -> Notifications {
        let key = path_key(path);
        let Some(doc) = self.docs.get_mut(&key) else {
            return Notifications::default();
        };
        doc.holders = doc.holders.saturating_sub(1);
        if doc.holders > 0 {
            return Notifications::default();
        }
        let doc = self.docs.remove(&key).expect("just found");
        if doc.unsent_text.is_some() {
            // The server never saw it.
            return Notifications::default();
        }
        Notifications(vec![(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(&doc.path) } }),
        )])
    }

    /// The process is gone: it knows no documents. Editors' holds are dropped
    /// too (they re-announce when the server is back).
    pub fn forget_all(&mut self) {
        self.docs.clear();
    }
}

fn open_params(doc: &Doc, text: &str) -> Value {
    json!({
        "textDocument": {
            "uri": path_to_uri(&doc.path),
            "languageId": doc.language_id,
            "version": doc.version,
            "text": text,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(n: &Notifications) -> Vec<&'static str> {
        n.0.iter().map(|(method, _)| *method).collect()
    }

    #[test]
    fn two_editors_one_didopen_and_one_didclose() {
        let mut docs = Documents::default();
        let path = Path::new("/p/a.rs");
        assert_eq!(methods(&docs.open(path, "x", "lang", true)), ["textDocument/didOpen"]);
        assert!(docs.open(path, "x", "lang", true).0.is_empty());
        assert!(docs.close(path).0.is_empty(), "one editor still has it");
        assert_eq!(methods(&docs.close(path)), ["textDocument/didClose"]);
        assert!(!docs.is_open(path));
    }

    #[test]
    fn a_document_opened_early_is_announced_with_its_latest_text() {
        let mut docs = Documents::default();
        let path = Path::new("/p/a.ts");
        assert!(docs.open(path, "old", "typescript", false).0.is_empty());
        assert!(docs.change_full(path, "newer", 2).0.is_empty());
        assert_eq!(docs.change_incremental(path, 3, vec![json!({})]), None);
        let flushed = docs.flush();
        assert_eq!(methods(&flushed), ["textDocument/didOpen"]);
        assert_eq!(flushed.0[0].1["textDocument"]["text"], "newer");
        assert_eq!(flushed.0[0].1["textDocument"]["languageId"], "typescript");
        assert!(docs.flush().0.is_empty(), "announced once");
        assert!(docs.change_incremental(path, 4, vec![json!({ "text": "x" })]).is_some());
    }

    #[test]
    fn closing_a_never_announced_document_sends_nothing() {
        let mut docs = Documents::default();
        let path = Path::new("/p/a.rs");
        docs.open(path, "x", "lang", false);
        assert!(docs.close(path).0.is_empty());
        assert!(docs.is_empty());
    }

    #[test]
    fn every_spelling_of_a_path_is_one_document() {
        let mut docs = Documents::default();
        docs.open(Path::new("/p/A.rs"), "x", "lang", true);
        assert!(docs.is_open(Path::new("/p/A.rs")));
        assert_eq!(docs.len(), 1);
        docs.forget_all();
        assert!(docs.is_empty());
    }
}
