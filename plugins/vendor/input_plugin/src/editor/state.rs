//! State the editors' panels share: the project's input library, and what
//! is selected in the mapping editor.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;

use crate::library::Library;

use super::common::document::SaveBus;

pub type Lib = Entity<LibraryModel>;
pub type Sel = Entity<Selection>;

// ── Selection ──────────────────────────────────────────────────────────────

/// What is selected in the mapping editor: a binding, or a category.
#[derive(Default)]
pub struct Selection {
    entry: Option<u32>,
    category: Option<u32>,
}

impl Selection {
    pub fn entry(&self) -> Option<u32> {
        self.entry
    }

    pub fn category(&self) -> Option<u32> {
        self.category
    }

    pub fn select_entry(&mut self, id: u32, category: u32, cx: &mut Context<Self>) {
        if self.entry != Some(id) || self.category != Some(category) {
            self.entry = Some(id);
            self.category = Some(category);
            cx.notify();
        }
    }

    pub fn select_category(&mut self, id: u32, cx: &mut Context<Self>) {
        if self.entry.is_some() || self.category != Some(id) {
            self.entry = None;
            self.category = Some(id);
            cx.notify();
        }
    }

    /// Forget what no longer exists (after a delete or undo).
    pub fn retain_existing(&mut self, mapping: &crate::model::MappingAsset, cx: &mut Context<Self>) {
        let before = (self.entry, self.category);
        if self.entry.is_some_and(|e| mapping.entry(e).is_none()) {
            self.entry = None;
        }
        if self.category.is_some_and(|c| mapping.category(c).is_none()) {
            self.category = None;
        }
        if before != (self.entry, self.category) {
            cx.notify();
        }
    }
}

// ── The project's library ──────────────────────────────────────────────────

/// The project's actions and mappings, kept up to date as files are saved.
pub struct LibraryModel {
    root: PathBuf,
    library: Arc<Library>,
    scanning: bool,
    /// Counts scans that changed the library, so caches can tell.
    revision: u64,
    _task: Option<Task<()>>,
    _subscription: Subscription,
}

impl LibraryModel {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let bus = SaveBus::get(cx);
        let subscription = cx.observe(&bus, |this: &mut Self, _, cx| this.refresh(cx));
        let mut model = Self {
            library: Arc::new(Library::empty(root.clone())),
            root,
            scanning: false,
            revision: 0,
            _task: None,
            _subscription: subscription,
        };
        model.refresh(cx);
        model
    }

    pub fn library(&self) -> Arc<Library> {
        self.library.clone()
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Scan the project again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let scan = cx.background_executor().spawn(async move { Library::scan(&root) });
        self.scanning = true;
        self._task = Some(cx.spawn(async move |this, cx| {
            let library = scan.await;
            this.update(cx, |this, cx| {
                this.scanning = false;
                this.library = Arc::new(library);
                this.revision += 1;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}
