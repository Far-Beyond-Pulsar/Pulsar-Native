use std::{ops::Range, rc::Rc};

use gpui::{
    canvas, deferred, div, prelude::FluentBuilder, px, relative, Action, AnyElement, App,
    AppContext, Bounds, Context, DismissEvent, Empty, Entity, EventEmitter, FontWeight,
    HighlightStyle, InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, Render,
    RenderOnce, SharedString, Styled, StyledText, Subscription, Window,
};
use lsp_types::{CompletionItem, CompletionTextEdit};

const MAX_MENU_WIDTH: Pixels = px(320.);
const MAX_MENU_HEIGHT: Pixels = px(240.);
const POPOVER_GAP: Pixels = px(4.);

use crate::{
    actions, h_flex,
    input::{
        self,
        popovers::{editor_popover, render_markdown},
        InputState, RopeExt,
    },
    label::Label,
    list::{List, ListDelegate, ListEvent},
    ActiveTheme, Icon, IconName, IndexPath, Selectable, Sizable as _,
};

struct ContextMenuDelegate {
    query: SharedString,
    menu: Entity<CompletionMenu>,
    items: Vec<Rc<CompletionItem>>,
    /// Indices into `items` that pass the current query filter, in sorted order.
    filtered_indices: Vec<usize>,
    /// Per-entry highlight byte-ranges for the current query (parallel to `filtered_indices`).
    filter_highlights: Vec<Vec<(Range<usize>, HighlightStyle)>>,
    selected_ix: usize,
}

// ---------------------------------------------------------------------------
// Fuzzy / prefix matching helpers
// ---------------------------------------------------------------------------

/// Match score (higher = better).
fn match_score(label: &str, filter_text: &str, query_lower: &str) -> Option<i32> {
    if query_lower.is_empty() {
        return Some(0);
    }
    let label_lower = label.to_lowercase();
    let filter_lower = filter_text.to_lowercase();

    // Exact
    if filter_lower == query_lower {
        return Some(1000);
    }
    // Prefix on filterText
    if filter_lower.starts_with(query_lower) {
        return Some(900);
    }
    // Prefix on label
    if label_lower.starts_with(query_lower) {
        return Some(800);
    }
    // Word-boundary prefix: each word in the label starts with consecutive query chars
    if word_boundary_match(&label_lower, query_lower) {
        return Some(700);
    }
    // Substring in filterText
    if filter_lower.contains(query_lower) {
        return Some(600);
    }
    // Substring in label
    if label_lower.contains(query_lower) {
        return Some(500);
    }
    // Fuzzy subsequence
    if is_subsequence(query_lower, &filter_lower) {
        return Some(300);
    }
    None
}

/// Returns true if the letters of `needle` each appear (in order) at the start
/// of successive words in `haystack` (e.g. "hcf" matches "hash_code_function").
fn word_boundary_match(haystack: &str, needle: &str) -> bool {
    let mut nchars = needle.chars();
    let Some(mut nc) = nchars.next() else { return true };
    let mut at_word_start = true;
    for hc in haystack.chars() {
        let is_delim = hc == '_' || hc == '-' || hc == ':' || hc == '.';
        if is_delim {
            at_word_start = true;
            continue;
        }
        if at_word_start && hc == nc {
            match nchars.next() {
                Some(next) => nc = next,
                None => return true,
            }
        }
        at_word_start = false;
    }
    false
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut hchars = haystack.chars();
    for nc in needle.chars() {
        if !hchars.any(|h| h == nc) {
            return false;
        }
    }
    true
}

/// Compute highlight ranges (byte offsets in `label`) for the matched characters.
fn compute_highlights(label: &str, query_lower: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    if query_lower.is_empty() {
        return vec![];
    }
    let label_lower = label.to_lowercase();
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };

    // Prefix: highlight the matching prefix
    if label_lower.starts_with(query_lower) {
        let byte_len = label
            .char_indices()
            .nth(query_lower.chars().count())
            .map(|(b, _)| b)
            .unwrap_or(label.len());
        return vec![(0..byte_len, bold)];
    }

    // Substring: highlight the matching span
    if let Some(pos) = label_lower.find(query_lower) {
        let end = pos + query_lower.len();
        return vec![(pos..end, bold)];
    }

    // Fuzzy: highlight each matched character individually
    let mut ranges: Vec<(Range<usize>, HighlightStyle)> = vec![];
    let mut qchars = query_lower.chars().peekable();
    let mut byte_offset = 0usize;
    for (_, lc) in label_lower.char_indices() {
        let qc = match qchars.peek() {
            Some(&c) => c,
            None => break,
        };
        let char_len = lc.len_utf8();
        if lc == qc {
            ranges.push((byte_offset..byte_offset + char_len, bold));
            qchars.next();
        }
        byte_offset += char_len;
    }
    ranges
}

impl ContextMenuDelegate {
    fn set_items(&mut self, items: Vec<CompletionItem>) {
        let mut items: Vec<Rc<CompletionItem>> = items.into_iter().map(Rc::new).collect();
        items.sort_by(|a, b| {
            let sort_a = a.sort_text.as_ref().unwrap_or(&a.label);
            let sort_b = b.sort_text.as_ref().unwrap_or(&b.label);
            sort_a.cmp(sort_b)
        });
        self.items = items;
            self.filtered_indices = (0..self.items.len()).collect();
            self.filter_highlights = self.filtered_indices.iter().map(|_| vec![]).collect();
            self.selected_ix = 0;

            tracing::info!("📋 Set {} completions", self.items.len());
    }

    fn selected_item(&self) -> Option<&Rc<CompletionItem>> {
        let filtered_ix = *self.filtered_indices.get(self.selected_ix)?;
        self.items.get(filtered_ix)
    }

    /// Apply `query` as a case-insensitive fuzzy filter over the current items.
    /// Immediately updates `filtered_indices`, `filter_highlights`, and `selected_ix`.
    fn apply_filter(&mut self, query: &str) {
        println!("[FILTER] apply_filter: query='{}', items.len()={}", query, self.items.len());
        tracing::info!("🔍 apply_filter called: query='{}', items.len()={}", query, self.items.len());
        self.query = SharedString::from(query.to_string());

        if query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
            self.filter_highlights = self.filtered_indices.iter().map(|_| vec![]).collect();
            self.selected_ix = 0;
            println!("[FILTER] empty query → showing all {} items", self.items.len());
            tracing::info!("📝 Empty query: showing all {} items", self.items.len());
            return;
        }

        let query_lower = query.to_lowercase();

        // Print first few items so we can see what the filter is working with.
        for (i, item) in self.items.iter().enumerate().take(5) {
            let ft = item.filter_text.as_deref().unwrap_or("<none>");
            println!("[FILTER]   item[{}] label={:?} filter_text={:?}", i, item.label, ft);
        }
        if self.items.len() > 5 {
            println!("[FILTER]   … {} more items", self.items.len() - 5);
        }

        let mut scored: Vec<(usize, i32)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| {
                let filter_text = item.filter_text.as_deref().unwrap_or(&item.label);
                let score = match_score(&item.label, filter_text, &query_lower);
                if ix < 5 {
                    println!("[FILTER]   score item[{}] ({:?}) = {:?}", ix, item.label, score);
                }
                score.map(|s| (ix, s))
            })
            .collect();

        println!("[FILTER] → {} / {} items matched query='{}'", scored.len(), self.items.len(), query);
        tracing::info!("✨ After scoring: {} items match query '{}'", scored.len(), query);

        // Higher score first, ties broken by server's sortText.
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                let sa = self.items[a.0].sort_text.as_ref().unwrap_or(&self.items[a.0].label);
                let sb = self.items[b.0].sort_text.as_ref().unwrap_or(&self.items[b.0].label);
                sa.cmp(sb)
            })
        });

        self.filter_highlights = scored
            .iter()
            .map(|(ix, _)| compute_highlights(&self.items[*ix].label, &query_lower))
            .collect();
        self.filtered_indices = scored.into_iter().map(|(ix, _)| ix).collect();
        self.selected_ix = 0;
        println!("[FILTER] filtered_indices.len()={}", self.filtered_indices.len());
        tracing::info!("✅ apply_filter done: filtered_indices.len()={}", self.filtered_indices.len());
    }
}

#[derive(IntoElement)]
struct CompletionMenuItem {
    ix: usize,
    item: Rc<CompletionItem>,
    children: Vec<AnyElement>,
    selected: bool,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

impl CompletionMenuItem {
    fn new(ix: usize, item: Rc<CompletionItem>) -> Self {
        Self {
            ix,
            item,
            children: vec![],
            selected: false,
            highlights: vec![],
        }
    }

    fn with_highlights(mut self, h: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        self.highlights = h;
        self
    }
}
impl Selectable for CompletionMenuItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for CompletionMenuItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}
impl RenderOnce for CompletionMenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let item = self.item;

        let deprecated = item.deprecated.unwrap_or(false);

        let highlights = self.highlights;

        // Map LSP CompletionItemKind → IconName using LSP metadata on each item.
        // Icons come from assets/icons/*.svg, named by the macro's PascalCase rule.
        let icon: IconName = match item.kind {
            // Functions / methods / constructors
            Some(lsp_types::CompletionItemKind::FUNCTION)
            | Some(lsp_types::CompletionItemKind::METHOD)
            | Some(lsp_types::CompletionItemKind::CONSTRUCTOR) => IconName::SigmaFunction,
            // Nominal types
            Some(lsp_types::CompletionItemKind::STRUCT)
            | Some(lsp_types::CompletionItemKind::CLASS) => IconName::Cube,
            // Enum variants / enum itself
            Some(lsp_types::CompletionItemKind::ENUM)
            | Some(lsp_types::CompletionItemKind::ENUM_MEMBER) => IconName::CodeBracketsSquare,
            // Interfaces / traits
            Some(lsp_types::CompletionItemKind::INTERFACE) => IconName::CodeBrackets,
            // Modules / namespaces / folders
            Some(lsp_types::CompletionItemKind::MODULE)
            | Some(lsp_types::CompletionItemKind::FOLDER) => IconName::FolderOpen,
            // Struct fields / object properties
            Some(lsp_types::CompletionItemKind::FIELD)
            | Some(lsp_types::CompletionItemKind::PROPERTY) => IconName::InputField,
            // Local variables
            Some(lsp_types::CompletionItemKind::VARIABLE) => IconName::Code,
            // Constants / units / plain values
            Some(lsp_types::CompletionItemKind::CONSTANT)
            | Some(lsp_types::CompletionItemKind::VALUE)
            | Some(lsp_types::CompletionItemKind::UNIT) => IconName::FxTag,
            // Language keywords
            Some(lsp_types::CompletionItemKind::KEYWORD) => IconName::Key,
            // Code snippets
            Some(lsp_types::CompletionItemKind::SNIPPET) => IconName::CodeBracketsSquare,
            // Generic type parameters
            Some(lsp_types::CompletionItemKind::TYPE_PARAMETER) => IconName::Type,
            // Colors
            Some(lsp_types::CompletionItemKind::COLOR) => IconName::FillColor,
            // Events
            Some(lsp_types::CompletionItemKind::EVENT) => IconName::Flash,
            // Operators
            Some(lsp_types::CompletionItemKind::OPERATOR) => IconName::Fx,
            // Files / references / plain text
            Some(lsp_types::CompletionItemKind::FILE)
            | Some(lsp_types::CompletionItemKind::REFERENCE)
            | Some(lsp_types::CompletionItemKind::TEXT) => IconName::Notes,
            // Unknown / unset
            _ => IconName::Code,
        };

        let source = "LSP";

        h_flex()
            .id(self.ix)
            .gap_2()
            .p_1()
            .text_xs()
            .line_height(relative(1.))
            .rounded_sm()
            .when(item.deprecated.unwrap_or(false), |this| this.line_through())
            .hover(|this| this.bg(cx.theme().accent.opacity(0.8)))
            .when(self.selected, |this| {
                this.bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
            // Icon — sized to match the text_xs row height
            .child(Icon::new(icon).xsmall())
            // Label
            .child(div().child(StyledText::new(item.label.clone()).with_highlights(highlights)))
            // Detail (type info, etc.)
            .when(item.detail.is_some(), |this| {
                this.child(
                    Label::new(item.detail.as_deref().unwrap_or("").to_string())
                        .text_color(cx.theme().muted_foreground)
                        .when(deprecated, |this| this.line_through())
                        .italic(),
                )
            })
            // Source label (right-aligned)
            .child(
                div().flex_1(), // Push source to the right
            )
            .child(
                Label::new(format!("[{}]", source))
                    .text_color(cx.theme().muted_foreground.opacity(0.6))
                    .italic(),
            )
            .children(self.children)
    }
}

impl EventEmitter<DismissEvent> for ContextMenuDelegate {}

impl ListDelegate for ContextMenuDelegate {
    type Item = CompletionMenuItem;

    fn items_count(&self, _: usize, _: &gpui::App) -> usize {
        let count = self.filtered_indices.len();
        tracing::debug!("📊 items_count: filtered_indices.len()={}, items.len()={}", count, self.items.len());
        count
    }

    fn render_item(
        &self,
        ix: crate::IndexPath,
        _: &mut Window,
        _: &mut Context<List<Self>>,
    ) -> Option<Self::Item> {
        let filtered_ix = *self.filtered_indices.get(ix.row)?;
        let item = self.items.get(filtered_ix)?;
        let highlights = self.filter_highlights.get(ix.row).cloned().unwrap_or_default();
        Some(CompletionMenuItem::new(ix.row, item.clone()).with_highlights(highlights))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<crate::IndexPath>,
        _: &mut Window,
        cx: &mut Context<List<Self>>,
    ) {
        self.selected_ix = ix.map(|i| i.row).unwrap_or(0);
        cx.notify();
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<List<Self>>) {
        let Some(item) = self.selected_item() else {
            return;
        };

        self.menu.update(cx, |this, cx| {
            this.select_item(&item, window, cx);
        });
    }
}

/// A context menu for code completions and code actions.
pub struct CompletionMenu {
    offset: usize,
    editor: Entity<InputState>,
    list: Entity<List<ContextMenuDelegate>>,
    open: bool,
    bounds: Bounds<Pixels>,
    loading: bool, // Track if we're loading completions

    /// The offset of the first character that triggered the completion.
    pub(crate) trigger_start_offset: Option<usize>,
    query: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl CompletionMenu {
    /// Creates a new `CompletionMenu` with the given offset and completion items.
    ///
    /// NOTE: This element should not call from InputState::new, unless that will stack overflow.
    pub(crate) fn new(
        editor: Entity<InputState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let view = cx.entity();
            let menu = ContextMenuDelegate {
                query: SharedString::default(),
                menu: view,
                items: vec![],
                filtered_indices: vec![],
                filter_highlights: vec![],
                selected_ix: 0,
            };

            let list = cx.new(|cx| {
                List::new(menu, window, cx)
                    .no_query() // Hide the search input - we filter client-side based on typing
                    .max_h(MAX_MENU_HEIGHT)
            });

            let _subscriptions =
                vec![
                    cx.subscribe(&list, |this: &mut Self, _, ev: &ListEvent, cx| {
                        match ev {
                            ListEvent::Confirm(_) => {
                                this.hide(cx);
                            }
                            _ => {}
                        }
                        cx.notify();
                    }),
                ];

            Self {
                offset: 0,
                editor,
                list,
                open: false,
                loading: false,
                trigger_start_offset: None,
                query: SharedString::default(),
                bounds: Bounds::default(),
                _subscriptions,
            }
        })
    }

    fn select_item(&mut self, item: &CompletionItem, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.offset;
        let item = item.clone();
        let mut range = self.trigger_start_offset.unwrap_or(self.offset)..self.offset;

        let editor = self.editor.clone();

        cx.spawn_in(window, async move |_, cx| {
            editor.update_in(cx, |editor, window, cx| {
                editor.completion_inserting = true;

                let mut new_text = item.label.clone();
                if let Some(text_edit) = item.text_edit.as_ref() {
                    match text_edit {
                        CompletionTextEdit::Edit(edit) => {
                            new_text = edit.new_text.clone();
                            range.start = editor.text.position_to_offset(&edit.range.start);
                            range.end = editor.text.position_to_offset(&edit.range.end);
                        }
                        CompletionTextEdit::InsertAndReplace(edit) => {
                            new_text = edit.new_text.clone();
                            range.start = editor.text.position_to_offset(&edit.replace.start);
                            range.end = editor.text.position_to_offset(&edit.replace.end);
                        }
                    }
                } else if let Some(insert_text) = item.insert_text.clone() {
                    new_text = insert_text;
                    range = offset..offset;
                }

                // Strip LSP snippet syntax (like $0, $1, ${1:default}, etc.)
                new_text = Self::strip_snippet_syntax(&new_text);

                editor.replace_text_in_range_silent(
                    Some(editor.range_to_utf16(&range)),
                    &new_text,
                    window,
                    cx,
                );
                editor.completion_inserting = false;
                // FIXME: Input not get the focus
                editor.focus(window, cx);
            })
        })
        .detach();

        self.hide(cx);
    }

    /// Strip LSP snippet syntax from completion text
    /// Removes $0, $1, ${1:default}, etc.
    fn strip_snippet_syntax(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' {
                // Check if next char is a digit or {
                match chars.peek() {
                    Some('0'..='9') => {
                        // Skip $N syntax
                        chars.next();
                        continue;
                    }
                    Some('{') => {
                        // Skip ${N} or ${N:default} syntax
                        chars.next(); // consume '{'
                        let mut depth = 1;
                        let mut in_default = false;
                        let mut default_text = String::new();

                        while depth > 0 {
                            match chars.next() {
                                Some('}') => {
                                    depth -= 1;
                                    if depth == 0 && in_default {
                                        result.push_str(&default_text);
                                    }
                                }
                                Some('{') => depth += 1,
                                Some(':') if depth == 1 && !in_default => {
                                    in_default = true;
                                }
                                Some(c) if in_default => {
                                    default_text.push(c);
                                }
                                Some(_) => {} // Skip other characters in placeholder
                                None => break,
                            }
                        }
                        continue;
                    }
                    _ => {} // Not a snippet marker, treat as regular $
                }
            }
            result.push(ch);
        }

        result
    }

    pub(crate) fn handle_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open {
            return false;
        }

        // Use Tab to accept completions (like most editors)
        // Both TabComplete and IndentInline should accept the completion
        if action.partial_eq(&super::super::tab_completion::TabComplete)
            || action.partial_eq(&input::IndentInline)
        {
            self.on_action_tab(window, cx);
            return true; // Return immediately to prevent any further action handling
        } else if action.partial_eq(&input::Escape) {
            self.on_action_escape(window, cx);
        } else if action.partial_eq(&input::MoveUp) {
            self.on_action_up(window, cx);
        } else if action.partial_eq(&input::MoveDown) {
            self.on_action_down(window, cx);
        } else {
            return false;
        }

        true
    }

    fn on_action_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.list.read(cx).delegate().selected_item().cloned() else {
            return;
        };
        self.select_item(&item, window, cx);
    }

    fn on_action_escape(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(cx);
    }

    fn on_action_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |this, cx| {
            this.on_action_select_prev(&actions::SelectUp, window, cx)
        });
    }

    fn on_action_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |this, cx| {
            this.on_action_select_next(&actions::SelectDown, window, cx)
        });
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Hide the completion menu and reset the trigger start offset.
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.trigger_start_offset = None;
        cx.notify();
    }

    pub(crate) fn update_query(
        &mut self,
        start_offset: usize,
        query: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.trigger_start_offset.is_none() {
            self.trigger_start_offset = Some(start_offset);
        }
        let q = query.into();
        self.query = q.clone();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().apply_filter(&q);
            cx.notify();
        });
        cx.notify();
    }

    /// Apply a new query instantly (synchronous re-filter of cached items).
    /// Call this on every keystroke before dispatching the async server request.
    pub(crate) fn apply_query(
        &mut self,
        trigger_start: usize,
        query: &str,
        cx: &mut Context<Self>,
    ) {
        println!("[FILTER] apply_query: trigger_start={}, query='{}', menu.open={}, items_in_delegate={}",
            trigger_start, query, self.open,
            self.list.read(cx).delegate().items.len());
        tracing::info!("🎯 CompletionMenu::apply_query called: trigger_start={}, query='{}'", trigger_start, query);
        if self.trigger_start_offset.is_none() {
            self.trigger_start_offset = Some(trigger_start);
        }
        self.query = SharedString::from(query.to_string());
        self.open = true;
        let q = query.to_string();
        self.list.update(cx, |list, cx| {
            println!("[FILTER] apply_query→list update: delegate items.len()={}", list.delegate().items.len());
            tracing::info!("📋 Before apply_filter in list: delegate items.len()={}", list.delegate().items.len());
            list.delegate_mut().apply_filter(&q);
            println!("[FILTER] apply_query→after filter: filtered_indices.len()={}", list.delegate().filtered_indices.len());
            tracing::info!("📋 After apply_filter in list: delegate filtered_indices.len()={}", list.delegate().filtered_indices.len());
            cx.notify();
        });
        cx.notify();
    }

    pub(crate) fn show(
        &mut self,
        offset: usize,
        items: impl Into<Vec<CompletionItem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = items.into();
        self.offset = offset;
        self.open = true;
        self.loading = false;
        let current_query = self.query.to_string();
        println!("[FILTER] show: offset={}, {} new items, will filter with query='{}'",
            offset, items.len(), current_query);
        self.list.update(cx, |this, cx| {
            this.delegate_mut().set_items(items);
            // Re-apply the active query so filter state survives server round-trips.
            this.delegate_mut().apply_filter(&current_query);
            cx.notify();
            this.set_selected_index(Some(IndexPath::new(0)), window, cx);
            // item_to_measure_index must be a POST-FILTER row index (0..filtered_indices.len()).
            // The old code used a raw items index which would be out-of-bounds when the active
            // query filtered the list down, causing render_item to return None, the virtual list
            // to measure a 0px row height, and the entire dropdown to collapse invisibly.
            let longest_filtered_row = {
                let d = this.delegate();
                (0..d.filtered_indices.len())
                    .max_by_key(|&row| {
                        let raw_ix = d.filtered_indices[row];
                        let item = &d.items[raw_ix];
                        item.label.len() + item.detail.as_ref().map(|dl| dl.len()).unwrap_or(0)
                    })
                    .unwrap_or(0)
            };
            this.set_item_to_measure_index(IndexPath::new(longest_filtered_row), window, cx);
        });

        cx.notify();
    }

    /// Show the menu in loading state while waiting for completions.
    /// Keeps any existing items visible (filtered by current query) so the user
    /// doesn't see a blank spinner between keystrokes.
    pub(crate) fn show_loading(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.offset = offset;
        self.open = true;
        self.loading = true;
        // Do NOT clear items or filter — stale results stay visible while we wait.
        cx.notify();
    }

    fn origin(&self, cx: &App) -> Option<Point<Pixels>> {
        let editor = self.editor.read(cx);
        let Some(last_layout) = editor.last_layout.as_ref() else {
            return None;
        };
        let Some(cursor_origin) = last_layout.cursor_bounds.map(|b| b.origin) else {
            return None;
        };

        let scroll_origin = self.editor.read(cx).scroll_handle.offset();

        Some(
            scroll_origin + cursor_origin - editor.input_bounds.origin
                + Point::new(-px(4.), last_layout.line_height + px(4.)),
        )
    }
}

impl Render for CompletionMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        // Show spinner only when loading AND no items cached yet (first request).
        if self.loading && self.list.read(cx).delegate().items.is_empty() {
            let view = cx.entity();
            let Some(pos) = self.origin(cx) else {
                return Empty.into_any_element();
            };

            return deferred(
                div().absolute().left(pos.x).top(pos.y).child(
                    editor_popover("completion-loading", cx)
                        .max_w(MAX_MENU_WIDTH)
                        .child(
                            div()
                                .p_2()
                                .text_color(cx.theme().muted_foreground)
                                .child("Loading completions..."),
                        ),
                ),
            )
            .into_any_element();
        }

        if self.list.read(cx).delegate().items.is_empty() {
            self.open = false;
            return Empty.into_any_element();
        }

        let view = cx.entity();

        let Some(pos) = self.origin(cx) else {
            return Empty.into_any_element();
        };

        let selected_documentation = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .and_then(|item| item.documentation.clone());

        let max_width = MAX_MENU_WIDTH.min(window.bounds().size.width - pos.x);
        let abs_pos = self.editor.read(cx).input_bounds.origin + pos;
        let vertical_layout =
            abs_pos.x + MAX_MENU_WIDTH + POPOVER_GAP + MAX_MENU_WIDTH + POPOVER_GAP
                > window.bounds().size.width;

        deferred(
            div()
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .flex()
                .flex_row()
                .gap(POPOVER_GAP)
                .items_start()
                .when(vertical_layout, |this| this.flex_col())
                .child(
                    editor_popover("completion-menu", cx)
                        .max_w(max_width)
                        .min_w(px(120.))
                        .child(self.list.clone())
                        .child(
                            canvas(
                                move |bounds, _, cx| view.update(cx, |r, _| r.bounds = bounds),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        ),
                )
                .when_some(selected_documentation, |this, documentation| {
                    let mut doc = match documentation {
                        lsp_types::Documentation::String(s) => s.clone(),
                        lsp_types::Documentation::MarkupContent(mc) => mc.value.clone(),
                    };
                    if vertical_layout {
                        doc = doc.split("\n").next().unwrap_or_default().to_string();
                    }

                    this.child(
                        div().child(
                            editor_popover("completion-menu", cx)
                                .w(MAX_MENU_WIDTH)
                                .px_2()
                                .child(render_markdown("doc", doc, window, cx)),
                        ),
                    )
                })
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.hide(cx);
                })),
        )
        .into_any_element()
    }
}
