//! Editor for composite property values: arrays, maps and structs.
//!
//! Registered nowhere: [`crate::render_property_row_runtime`] falls back to it
//! for any type that has no editor of its own but is structurally a container.
//!
//! The value is kept as JSON and shown as a tree ([`crate::composite_model`]).
//! Only expanded branches contribute rows, and the rows are drawn by a
//! `gpui::list` when it has a viewport, which measures just the rows in view and remembers their
//! heights, so a 100,000-element array or a deeply nested struct costs what is
//! on screen.  By default the rows flow with the surrounding panel, 50 at a
//! time with a "Show more" button, so there is no nested scrolling.  A type that
//! explicitly asks for a sub-scroll area ([`request_subscroll`]) gets a windowed
//! viewport instead, where every row is virtualized.
//!
//! Elements are edited by the editor registered for their type.  An edit
//! replaces that element in the JSON, rebuilds the whole value from it and
//! writes it back like any other property.

use crate::composite_model::{self, get, get_mut, Path, RowKind, Seg, TreeRow};
use crate::property_editor_registry::PROPERTY_EDITOR_REGISTRY;
use gpui::{prelude::*, *};
use pulsar_reflection::{
    VirtualRows,
    BoundPropertyEditor, PropertyEditorArgs, PropertyWriteBack, RuntimeTypeInfo,
    RUNTIME_TYPE_REGISTRY,
};
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ui::button::{Button, ButtonVariants as _};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable as _};

/// Where the panel's wake-up callback is kept, shared by the editor and
/// its [`VirtualRows`] handle.
type WakeSlot = Arc<std::sync::Mutex<Option<Arc<dyn Fn(&mut App) + Send + Sync>>>>;

/// Types whose composite editor must scroll inside a viewport of its own.
///
/// Empty unless the reflection side asks: nested scrolling is awkward, so the
/// editor hands its rows to the panel's own list ([`VirtualRows`]) by default.
/// A type that
/// really wants a sub-scroll area of its own calls [`request_subscroll`].
static SUBSCROLL: std::sync::LazyLock<std::sync::RwLock<HashMap<TypeId, f32>>> =
    std::sync::LazyLock::new(Default::default);

/// Explicitly ask for the composite editor of `type_id` to scroll within a
/// viewport `height` pixels tall.
pub fn request_subscroll(type_id: TypeId, height: f32) {
    if let Ok(mut map) = SUBSCROLL.write() {
        map.insert(type_id, height);
    }
}

fn requested_subscroll(type_id: TypeId) -> Option<f32> {
    SUBSCROLL.read().ok()?.get(&type_id).copied()
}

/// Leaf editors kept alive for rows that scrolled away.  Past this the cache
/// is dropped; visible rows rebuild theirs on the next frame.
const LEAF_CACHE_LIMIT: usize = 256;
const INDENT: f32 = 12.0;

fn has_editor(id: TypeId) -> bool {
    PROPERTY_EDITOR_REGISTRY.has(id)
}

/// Whether `ty` is edited by [`CompositeEditor`] (a container with no editor
/// of its own).
pub fn is_composite(ty: &'static RuntimeTypeInfo) -> bool {
    !matches!(
        composite_model::resolve(ty, &has_editor).1,
        composite_model::Shape::Leaf
    )
}

/// Treat a registry-produced box as `Send`.
///
/// Every type in the runtime registry is `Reflectable`, which requires
/// `Send + Sync`; the registry only erases that to `dyn Any`.
fn into_send(value: Box<dyn Any>) -> Box<dyn Any + Send> {
    // SAFETY: `Reflectable: Any + Send + Sync`, and only `Reflectable` types
    // are registered, so the concrete value is `Send`.  The transmute changes only
    // the auto-trait part of the trait-object type.
    unsafe { std::mem::transmute::<Box<dyn Any>, Box<dyn Any + Send>>(value) }
}

pub struct CompositeEditor {
    label: String,
    id: SharedString,
    ty: &'static RuntimeTypeInfo,
    id_prefix: String,
    class_name: String,
    root: Value,
    /// Header open/closed.
    open: bool,
    expanded: HashSet<Path>,
    rows: Vec<TreeRow>,
    list_state: ListState,
    scroll_state: ScrollbarState,
    /// Viewport height when a sub-scroll area was explicitly requested.
    viewport: Option<f32>,
    leaves: HashMap<Path, BoundPropertyEditor>,
    write_back: PropertyWriteBack,
    /// Set by the host when it knows the value changed behind our back.
    stale: Arc<AtomicBool>,
    /// Address of the last value pushed in; an unchanged address with no
    /// `stale` flag means nothing to re-read.
    last_ptr: usize,
    /// The panel's wake-up, registered through [`VirtualRows::on_change`].
    wake: WakeSlot,
}

impl CompositeEditor {
    fn new(
        args: &PropertyEditorArgs<'_>,
        stale: Arc<AtomicBool>,
        wake: WakeSlot,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(args.current_value)
            .unwrap_or(Value::Null);
        let mut this = Self {
            label: args.display_name.to_string(),
            id: SharedString::from(format!(
                "composite-{}-{}-{}",
                args.id_prefix, args.class_name, args.prop_name
            )),
            ty: args.type_info,
            id_prefix: args.id_prefix.to_string(),
            class_name: args.class_name.to_string(),
            root,
            open: false,
            expanded: HashSet::new(),
            rows: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(240.0)),
            scroll_state: ScrollbarState::default(),
            viewport: requested_subscroll(args.type_info.type_id),
            leaves: HashMap::new(),
            write_back: args.write_back.clone(),
            stale,
            last_ptr: args.current_value as *const dyn Any as *const () as usize,
            wake,
        };
        this.relayout();
        let _ = cx;
        this
    }

    /// Rows offered to the panel's list: all of them while open, none when the
    /// header is closed or the rows scroll in a viewport of their own.
    fn rows_offered(&self) -> usize {
        if self.open && self.viewport.is_none() {
            self.rows.len()
        } else {
            0
        }
    }

    /// Something that shapes the rows changed: repaint, and wake the panel so
    /// it re-reads [`Self::rows_offered`].  The wake is deferred, because the
    /// change can arrive while the panel itself is mid-render.
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let wake = self.wake.lock().ok().and_then(|slot| slot.clone());
        if let Some(wake) = wake {
            cx.defer(move |cx| wake(cx));
        }
    }

    /// Recompute the visible rows and tell the list which range changed.
    fn relayout(&mut self) {
        let rows =
            composite_model::flatten(&self.root, self.ty, &self.expanded, &has_editor);
        let same = |a: &TreeRow, b: &TreeRow| {
            a.path == b.path && a.depth == b.depth && a.kind == b.kind
        };
        let old = &self.rows;
        let prefix = old.iter().zip(&rows).take_while(|(a, b)| same(a, b)).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(a, b)| same(a, b))
            .count();
        if prefix != old.len() || prefix != rows.len() {
            self.list_state.splice(
                prefix..old.len() - suffix,
                rows.len() - prefix - suffix,
            );
        }
        self.rows = rows;
    }

    /// Accept a value that changed elsewhere.
    fn set_value(&mut self, value: &dyn Any, cx: &mut Context<Self>) {
        let ptr = value as *const dyn Any as *const () as usize;
        let stale = self.stale.swap(false, Ordering::AcqRel);
        if ptr == self.last_ptr && !stale {
            return;
        }
        self.last_ptr = ptr;
        let Ok(json) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value) else {
            return;
        };
        if json == self.root {
            return;
        }
        self.root = json;
        self.relayout();
        // Rows keep their heights, but their editors must re-read: the
        // per-row `set_value` in `render_row` does that when they draw.
        self.changed(cx);
    }

    /// Replace the element at `path` and write the rebuilt value back.
    fn apply(&mut self, path: &[Seg], json: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = get_mut(&mut self.root, path) else {
            return;
        };
        if *slot == json {
            return;
        }
        *slot = json;
        self.commit(window, cx);
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(self.ty, self.root.clone()) {
            Ok(value) => (self.write_back)(into_send(value), window, cx),
            Err(error) => tracing::warn!("composite edit rejected: {error}"),
        }
        self.changed(cx);
    }

    fn toggle(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.clone());
        }
        self.relayout();
        self.changed(cx);
    }

    /// Remove the element at `path` from its array.
    fn remove(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some((Seg::Index(index), parent)) = path.split_last() else {
            return;
        };
        let Some(Value::Array(items)) = get_mut(&mut self.root, parent) else {
            return;
        };
        if (*index as usize) < items.len() {
            items.remove(*index as usize);
        }
        // Indices after the removed one shifted: cached editors and expansion
        // are addressed by index, so start those afresh.
        self.leaves.clear();
        self.expanded.retain(|p| !p.starts_with(parent));
        self.relayout();
        self.commit(window, cx);
    }

    /// Append a copy of the last element (or of the type's JSON default when
    /// empty is not derivable, nothing).
    fn push(&mut self, parent: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Value::Array(items)) = get_mut(&mut self.root, parent) else {
            return;
        };
        let Some(template) = items.last().cloned() else {
            return;
        };
        items.push(template);
        self.relayout();
        self.commit(window, cx);
    }

    fn leaf_editor(
        &mut self,
        row: &TreeRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<BoundPropertyEditor> {
        if let Some(existing) = self.leaves.get(&row.path) {
            return Some(existing.clone());
        }
        let factory = PROPERTY_EDITOR_REGISTRY.get(row.ty.type_id)?;
        let json = get(&self.root, &row.path)?.clone();
        let current = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(row.ty, json)
            .ok()?;
        let this = cx.entity().downgrade();
        let path = row.path.clone();
        let write_back: PropertyWriteBack = Arc::new(move |value, window, cx| {
            let Ok(json) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value.as_ref()) else {
                return;
            };
            let path = path.clone();
            this.update(cx, |editor, cx| editor.apply(&path, json, window, cx))
                .ok();
        });
        let label = row.label();
        let args = PropertyEditorArgs {
            id_prefix: &self.id_prefix,
            class_name: &self.class_name,
            display_name: &label,
            prop_name: &label,
            type_info: row.ty,
            current_value: current.as_ref(),
            write_back,
        };
        if self.leaves.len() >= LEAF_CACHE_LIMIT {
            self.leaves.clear();
        }
        let editor = factory(&args, window, cx);
        self.leaves.insert(row.path.clone(), editor.clone());
        Some(editor)
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let indent = px(INDENT * f32::from(row.depth));
        let remove = row.removable.then(|| {
            let path = row.path.clone();
            Button::new(SharedString::from(format!("{}-rm-{ix}", self.id)))
                .icon(IconName::Close)
                .xsmall()
                .ghost()
                .tooltip("Remove")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.remove(&path, window, cx);
                }))
                .into_any_element()
        });

        let body: AnyElement = match &row.kind {
            RowKind::Leaf => match self.leaf_editor(&row, window, cx) {
                Some(editor) => {
                    if let Some(json) = get(&self.root, &row.path).cloned() {
                        if let Ok(value) =
                            RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(row.ty, json)
                        {
                            (editor.set_value)(value.as_ref(), window, cx);
                        }
                    }
                    // A cached view: a row that did not change replays instead of
                    // rebuilding every frame while the list scrolls.
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            editor
                                .view
                                .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0()),
                        )
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            RowKind::ReadOnly => {
                let text = get(&self.root, &row.path)
                    .map(|v| match v {
                        Value::Null => "None".to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_between()
                    .gap_2()
                    .child(div().text_sm().text_color(muted).child(row.label()))
                    .child(
                        div()
                            .text_xs()
                            .max_w(px(160.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(text),
                    )
                    .into_any_element()
            }
            RowKind::Branch { len, expanded } => {
                let path = row.path.clone();
                let push_path = row.path.clone();
                let is_seq = matches!(
                    composite_model::resolve(row.ty, &has_editor).1,
                    composite_model::Shape::Seq(_)
                );
                h_flex()
                    .id(SharedString::from(format!("{}-row-{ix}", self.id)))
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&path, cx)))
                    .child(
                        Icon::new(if *expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.label()),
                    )
                    .child(div().text_xs().text_color(muted).child(len.to_string()))
                    .when(is_seq && *len > 0, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("{}-add-{ix}", self.id)))
                                .icon(IconName::Plus)
                                .xsmall()
                                .ghost()
                                .tooltip("Add a copy of the last element")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.push(&push_path, window, cx);
                                })),
                        )
                    })
                    .into_any_element()
            }
        };

        h_flex()
            .w_full()
            .pl(indent)
            .py(px(1.))
            .gap_1()
            .items_center()
            .child(body)
            .children(remove)
            .into_any_element()
    }
}

impl Render for CompositeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let count = match &self.root {
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            _ => 0,
        };
        let header = h_flex()
            .id(SharedString::from(format!("{}-header", self.id)))
            .w_full()
            .gap_1()
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.open = !this.open;
                this.changed(cx);
            }))
            .child(
                Icon::new(if self.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(self.label.clone()),
            )
            .child(div().text_xs().text_color(muted).child(count.to_string()));

        let body = (self.open && !self.rows.is_empty() && self.viewport.is_some()).then(|| match self.viewport {
            // Only a type that explicitly asked for a sub-scroll area gets one.
            Some(height) => {
                let rows = list(
                    self.list_state.clone(),
                    cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
                )
                .size_full();
                div()
                    .id(SharedString::from(format!("{}-viewport", self.id)))
                    .relative()
                    .w_full()
                    .h(px(height))
                    .mt_1()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.))
                    .child(rows)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(px(12.))
                            .child(Scrollbar::vertical(&self.scroll_state, &self.list_state)),
                    )
                    .into_any_element()
            }
            // Default: the rows live in the panel's own list ([`VirtualRows`]),
            // so nothing here but the header.
            None => div().into_any_element(),
        });

        v_flex().w_full().child(header).children(body)
    }
}

/// The panel's handle on a composite's rows.
struct CompositeRows {
    editor: Entity<CompositeEditor>,
    wake: WakeSlot,
}

impl VirtualRows for CompositeRows {
    fn row_count(&self, cx: &App) -> usize {
        self.editor.read(cx).rows_offered()
    }

    fn render_row(&self, index: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        self.editor
            .update(cx, |editor, cx| editor.render_row(index, window, cx))
    }

    fn on_change(&self, wake: Arc<dyn Fn(&mut App) + Send + Sync>) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }
}

/// Build the editor for a composite property.  The returned flag is set by the
/// host (via [`crate::PropertyStateManager::mark_stale`]) when it knows the
/// value changed outside the editor.
///
/// The editor itself is only the header; its content is offered to the panel
/// as [`VirtualRows`], to be drawn by the panel's one virtualized list.
pub fn build(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> (BoundPropertyEditor, Arc<AtomicBool>) {
    let _ = window;
    let stale = Arc::new(AtomicBool::new(false));
    let wake: WakeSlot = Default::default();
    let entity = cx.new(|cx| CompositeEditor::new(args, stale.clone(), wake.clone(), cx));
    let for_set = entity.clone();
    let editor = BoundPropertyEditor {
        view: entity.clone().into(),
        set_value: Arc::new(move |value, _window, cx| {
            for_set.update(cx, |editor, cx| editor.set_value(value, cx));
        }),
        rows: None,
    }
    .with_rows(Arc::new(CompositeRows {
        editor: entity,
        wake,
    }));
    (editor, stale)
}

impl CompositeEditor {
    /// Open or close the header (closed by default).
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        self.changed(cx);
    }

    /// Scroll the row viewport of a long composite.
    pub fn scroll_by(&self, distance: Pixels) {
        self.list_state.scroll_by(distance);
    }

    /// Number of rows currently in the tree (expanded branches included).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Expand every top-level branch.
    pub fn expand_top_level(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<Path> = self
            .rows
            .iter()
            .filter(|r| r.path.len() == 1 && matches!(r.kind, RowKind::Branch { .. }))
            .map(|r| r.path.clone())
            .collect();
        self.expanded.extend(paths);
        self.relayout();
        self.changed(cx);
    }
}

impl CompositeEditor {
    /// Replace the element at the index path `indices` (array positions from
    /// the root) with `json` and write the rebuilt value back, exactly as an
    /// edit through a row's editor would.
    pub fn edit_element(
        &mut self,
        indices: &[u32],
        json: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path: Path = indices.iter().map(|i| Seg::Index(*i)).collect();
        self.apply(&path, json, window, cx);
    }
}
