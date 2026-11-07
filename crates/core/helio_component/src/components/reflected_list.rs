//! Inspector editing for ordered lists of reflected values.
//!
//! The reflected properties panel finds an editor by the property's exact
//! type, and nothing registers one for a `Vec<T>`, so a list field shows as
//! unimplemented. This module holds the pieces a list editor is built from:
//! [`FieldRows`] (every field of one value, each edited by the editor
//! registered for its type), [`ListEdit`] (move, remove, add) and
//! [`item_card`] (one item's header and rows). [`ReflectedListEditor`]
//! puts them together for any [`ReflectedListItem`]; register it for a
//! `Vec<T>` with [`register_reflected_list_editor!`].

use std::any::Any;
use std::sync::{Arc, Mutex};

use gpui::{AnyElement, App, SharedString, WeakEntity, Window, prelude::*};
use pulsar_reflection::{
    BoundPropertyEditor, EngineClass, PropertyEditorArgs, PropertyEditorFactory, PropertyMetadata,
    PropertyWriteBack, VirtualRows,
};
use ui::button::{Button, ButtonVariants as _};
use ui::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};

/// The editor registered for a property type, if any.
pub(crate) fn editor_factory(type_id: std::any::TypeId) -> Option<PropertyEditorFactory> {
    pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint>
        .into_iter()
        .find(|hint| hint.type_id == type_id)
        // SAFETY: `erase_property_editor_fn_ptr` only accepts a
        // `PropertyEditorFactory`, so every submitted pointer has that type
        // (the same contract the inspector's registry relies on).
        .map(|hint| unsafe { std::mem::transmute::<fn(), PropertyEditorFactory>(hint.fn_ptr) })
}

/// Editors for the reflected properties of one value (a layer, a rule, a
/// lens element...), each writing the whole value back through `put`.
/// Properties without a registered editor (lists, nested structs) and those
/// `shows` hides are skipped.
pub struct FieldRows {
    pub editors: Vec<(usize, BoundPropertyEditor)>,
    /// Pushes the current value into every editor.
    refresh: Arc<dyn Fn(&[(usize, BoundPropertyEditor)], &mut Window, &mut App) + Send + Sync>,
}

impl FieldRows {
    #[allow(clippy::too_many_arguments)]
    pub fn new<T: EngineClass + Clone>(
        value: &T,
        id: &str,
        class_name: &str,
        shows: impl Fn(&T, &PropertyMetadata) -> bool,
        get: impl Fn() -> Option<T> + Send + Sync + 'static,
        put: impl Fn(T, &mut Window, &mut App) + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let props = Arc::new(value.get_properties());
        let get = Arc::new(get);
        let put = Arc::new(put);
        let mut editors = Vec::new();
        for (index, prop) in props.iter().enumerate() {
            if !shows(value, prop) {
                continue;
            }
            let Some(factory) = editor_factory(prop.type_info.type_id) else {
                continue;
            };
            let current = (prop.getter)(value);
            let write_back: PropertyWriteBack = {
                let (props, get, put) = (props.clone(), get.clone(), put.clone());
                Arc::new(
                    move |new: Box<dyn Any + Send>, window: &mut Window, cx: &mut App| {
                        let Some(mut value) = get() else { return };
                        (props[index].setter)(&mut value, new);
                        put(value, window, cx);
                    },
                )
            };
            let args = PropertyEditorArgs {
                id_prefix: id,
                class_name,
                display_name: &prop.display_name,
                prop_name: prop.name,
                type_info: prop.type_info,
                current_value: current.as_ref(),
                write_back,
            };
            editors.push((index, factory(&args, window, cx)));
        }
        let refresh: Arc<
            dyn Fn(&[(usize, BoundPropertyEditor)], &mut Window, &mut App) + Send + Sync,
        > = Arc::new(
            move |editors: &[(usize, BoundPropertyEditor)], window: &mut Window, cx: &mut App| {
                let Some(value) = get() else { return };
                for (index, editor) in editors {
                    (editor.set_value)((props[*index].getter)(&value).as_ref(), window, cx);
                }
            },
        );
        Self { editors, refresh }
    }

    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        (self.refresh)(&self.editors, window, cx);
    }

    pub fn views(&self) -> impl Iterator<Item = AnyElement> + '_ {
        self.editors
            .iter()
            .map(|(_, editor)| editor.view.clone().into_any_element())
    }
}

/// Moves, removals and additions of a list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEdit {
    Up(usize),
    Down(usize),
    Remove(usize),
    Add,
}

/// Apply `edit` to `items` (out-of-range moves do nothing); `new` makes an
/// added item.
pub fn apply_list_edit<T>(items: &mut Vec<T>, edit: ListEdit, new: impl FnOnce() -> T) {
    match edit {
        ListEdit::Up(i) if i > 0 && i < items.len() => items.swap(i - 1, i),
        ListEdit::Down(i) if i + 1 < items.len() => items.swap(i, i + 1),
        ListEdit::Remove(i) if i < items.len() => {
            items.remove(i);
        }
        ListEdit::Add => items.push(new()),
        _ => {}
    }
}

/// One list item: its title with move and remove buttons, then its rows.
/// `edit` applies a button's [`ListEdit`] to the owning view.
#[allow(dead_code)]
pub(crate) fn item_card<V: 'static>(
    id: &str,
    index: usize,
    title: String,
    rows: impl IntoIterator<Item = AnyElement>,
    edit: impl Fn(&mut V, ListEdit, &mut Window, &mut gpui::Context<V>) + Clone + 'static,
    cx: &mut gpui::Context<V>,
) -> AnyElement {
    let mut button = |name: &str, icon: IconName, tooltip: &str, list_edit: ListEdit| {
        let edit = edit.clone();
        Button::new(SharedString::from(format!("{id}-{index}-{name}")))
            .icon(icon)
            .xsmall()
            .ghost()
            .tooltip(tooltip.to_string())
            .on_click(cx.listener(move |this, _, window, cx| edit(this, list_edit, window, cx)))
    };
    let buttons = h_flex()
        .gap_1()
        .child(button(
            "up",
            IconName::ArrowUp,
            "Move up",
            ListEdit::Up(index),
        ))
        .child(button(
            "down",
            IconName::ArrowDown,
            "Move down",
            ListEdit::Down(index),
        ))
        .child(button(
            "remove",
            IconName::Trash,
            "Remove",
            ListEdit::Remove(index),
        ));
    v_flex()
        .w_full()
        .gap_1()
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(gpui::div().text_sm().child(title))
                .child(buttons),
        )
        .children(rows)
        .into_any_element()
}

/// A value edited as an item of a `Vec` property by [`ReflectedListEditor`].
pub trait ReflectedListItem:
    EngineClass + Clone + Default + serde::Serialize + Send + Sync + 'static
{
    /// Class name handed to the field editors.
    const CLASS_NAME: &'static str;
    /// What one item is called on the add button ("element").
    const NOUN: &'static str;
    /// Most items the list holds; the add button hides at this length.
    const MAX_ITEMS: usize = usize::MAX;

    /// The item's card title; `index` is its position.
    fn title(&self, index: usize) -> String;

    /// Whether the item's card shows `prop`. An item whose fields mean
    /// different things by kind shows only those its kind reads; its rows
    /// rebuild when the answer changes.
    fn shows(&self, _prop: &PropertyMetadata) -> bool {
        true
    }

    /// Store an edited item. Returns true when more than the edited field
    /// changed (a new kind bringing its defaults), so every row refreshes.
    fn apply_edit(&mut self, edited: Self) -> bool {
        *self = edited;
        false
    }
}

/// Which of an item's properties its card shows.
fn shown<T: ReflectedListItem>(item: &T) -> Vec<bool> {
    item.get_properties()
        .iter()
        .map(|prop| item.shows(prop))
        .collect()
}

/// Streaming fingerprint of a value's JSON: no allocation, so checking whether
/// a list changed costs a pass over it, not a copy of it.
fn fingerprint<T: serde::Serialize>(value: &[T]) -> u64 {
    use std::hash::Hasher as _;
    struct HashWriter(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for HashWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(std::collections::hash_map::DefaultHasher::new());
    let _ = serde_json::to_writer(&mut writer, value);
    writer.0.finish()
}

/// A collapsible list item: a header (chevron, title, move and remove
/// buttons) and, when `expanded`, its rows.
///
/// `edit` applies a button's [`ListEdit`] to the owning view; `toggle` flips
/// the item's expansion.
pub fn collapsible_item_card<V: 'static>(
    id: &str,
    index: usize,
    title: String,
    expanded: bool,
    rows: impl IntoIterator<Item = AnyElement>,
    edit: impl Fn(&mut V, ListEdit, &mut Window, &mut gpui::Context<V>) + Clone + 'static,
    toggle: impl Fn(&mut V, usize, &mut Window, &mut gpui::Context<V>) + Clone + 'static,
    cx: &mut gpui::Context<V>,
) -> AnyElement {
    let mut button = |name: &str, icon: IconName, tooltip: &str, list_edit: ListEdit| {
        let edit = edit.clone();
        Button::new(SharedString::from(format!("{id}-{index}-{name}")))
            .icon(icon)
            .xsmall()
            .ghost()
            .tooltip(tooltip.to_string())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                edit(this, list_edit, window, cx)
            }))
    };
    let buttons = h_flex()
        .gap_1()
        .child(button(
            "up",
            IconName::ArrowUp,
            "Move up",
            ListEdit::Up(index),
        ))
        .child(button(
            "down",
            IconName::ArrowDown,
            "Move down",
            ListEdit::Down(index),
        ))
        .child(button(
            "remove",
            IconName::Trash,
            "Remove",
            ListEdit::Remove(index),
        ));
    let toggle = toggle.clone();
    let header = h_flex()
        .id(SharedString::from(format!("{id}-{index}-header")))
        .w_full()
        .gap_1()
        .items_center()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| toggle(this, index, window, cx)))
        .child(
            ui::Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(cx.theme().muted_foreground),
        )
        .child(gpui::div().flex_1().text_sm().child(title))
        .child(buttons);
    v_flex()
        .w_full()
        .gap_1()
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .child(header)
        .when(expanded, |card| card.children(rows))
        .into_any_element()
}

/// One row of the list's flattened layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// The card header of item `0`.
    Header(usize),
    /// Field editor `k` of item `i`.
    Field(usize, usize),
    /// Row `ix` of the rows offered by field editor `k` of item `i` (a nested
    /// list).
    Nested(usize, usize, usize),
}

/// A `Vec<T>` property as an ordered list of cards: move up and down,
/// remove, add, and every shown field of each item edited in place. The
/// whole list writes back as one value, so undo is one step.
///
/// Built to stay cheap on long lists of heavy items:
/// - the editor's own row is only the title and the add button; the item
///   cards, their fields and any nested lists are offered to the panel as
///   [`VirtualRows`], so the panel's single list draws only what is in view,
///   with no scroll area of its own;
/// - cards start collapsed, and an item's field editors are created only when
///   its card is first opened;
/// - telling whether the list changed is a streaming hash, not a JSON copy.
pub struct ReflectedListEditor<T: ReflectedListItem> {
    label: String,
    id: String,
    current: Arc<Mutex<Vec<T>>>,
    write_back: PropertyWriteBack,
    /// Per item: its rows and the shown mask they were built for. `None`
    /// until the item's card is opened.
    items: Vec<Option<(FieldRows, Vec<bool>)>>,
    /// Opened cards, by index.
    expanded: std::collections::HashSet<usize>,
    /// Fingerprint of `current`, shared with the rows' write path so the
    /// value pushed back after a local edit is recognised as already shown.
    fingerprint: Arc<std::sync::atomic::AtomicU64>,
    /// Set by the host (via `VirtualRows::mark_stale`) when it re-read the
    /// value; with the address check it keeps `set_value` from hashing a
    /// huge list every frame.
    stale: Arc<std::sync::atomic::AtomicBool>,
    last_ptr: usize,
    /// The rows offered to the panel, in order.
    layout: Vec<Entry>,
    /// The panel's wake-up (see [`VirtualRows::on_change`]).
    wake: ListWake,
    this: WeakEntity<Self>,
}

/// Where the panel's wake-up callback is kept, shared by the editor and its
/// [`VirtualRows`] handle.
pub type ListWake = Arc<Mutex<Option<Arc<dyn Fn(&mut App) + Send + Sync>>>>;

impl<T: ReflectedListItem> ReflectedListEditor<T> {
    pub fn new(
        args: &PropertyEditorArgs<'_>,
        wake: ListWake,
        stale: Arc<std::sync::atomic::AtomicBool>,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let value = args
            .current_value
            .downcast_ref::<Vec<T>>()
            .cloned()
            .unwrap_or_default();
        let hash = fingerprint(&value);
        let len = value.len();
        let mut editor = Self {
            label: args.display_name.to_string(),
            id: format!(
                "list-{}-{}-{}",
                args.id_prefix, args.class_name, args.prop_name
            ),
            current: Arc::new(Mutex::new(value)),
            write_back: args.write_back.clone(),
            items: (0..len).map(|_| None).collect(),
            expanded: Default::default(),
            fingerprint: Arc::new(std::sync::atomic::AtomicU64::new(hash)),
            stale,
            last_ptr: args.current_value as *const dyn Any as *const () as usize,
            layout: Vec::new(),
            wake,
            this: cx.weak_entity(),
        };
        editor.relayout(cx);
        editor
    }

    fn value(&self) -> Vec<T> {
        self.current.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn len(&self) -> usize {
        self.current.lock().map(|v| v.len()).unwrap_or(0)
    }

    /// Rows for item `i`, writing the whole list back on every edit.
    fn rows(&self, i: usize, item: &T, window: &mut Window, cx: &mut App) -> FieldRows {
        let (read, write) = (self.current.clone(), self.current.clone());
        let write_back = self.write_back.clone();
        let this = self.this.clone();
        let fingerprint_slot = self.fingerprint.clone();
        let rows = FieldRows::new(
            item,
            &format!("{}-{i}", self.id),
            T::CLASS_NAME,
            T::shows,
            move || read.lock().ok().and_then(|items| items.get(i).cloned()),
            move |edited: T, window, cx| {
                let (items, reshaped) = {
                    let Ok(mut items) = write.lock() else { return };
                    let Some(item) = items.get_mut(i) else { return };
                    let before = shown(item);
                    let refreshed = item.apply_edit(edited);
                    let reshaped = refreshed || shown(item) != before;
                    (items.clone(), reshaped)
                };
                fingerprint_slot.store(fingerprint(&items), std::sync::atomic::Ordering::Release);
                write_back(Box::new(items), window, cx);
                if reshaped {
                    // After the editor that wrote returns: it is mid-event,
                    // and its own row may be the one rebuilt.
                    let this = this.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.refresh(window, cx);
                            this.changed(cx);
                        });
                    });
                }
            },
            window,
            cx,
        );
        // A nested list changing its own rows reshapes ours.
        let this = self.this.clone();
        let nested_changed: Arc<dyn Fn(&mut App) + Send + Sync> = Arc::new(move |cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                this.relayout(cx);
                this.changed(cx);
            });
        });
        for (_, editor) in &rows.editors {
            if let Some(nested) = &editor.rows {
                nested.on_change(nested_changed.clone());
            }
        }
        rows
    }

    /// Build the rows of item `i` if it is open and they are missing or were
    /// built for a different shown mask.
    fn ensure_rows(&mut self, i: usize, window: &mut Window, cx: &mut App) {
        if !self.expanded.contains(&i) {
            return;
        }
        let Some(item) = self.current.lock().ok().and_then(|v| v.get(i).cloned()) else {
            return;
        };
        let mask = shown(&item);
        if self
            .items
            .get(i)
            .and_then(Option::as_ref)
            .is_some_and(|(_, built)| *built == mask)
        {
            return;
        }
        let rows = self.rows(i, &item, window, cx);
        if let Some(slot) = self.items.get_mut(i) {
            *slot = Some((rows, mask));
        }
    }

    /// Recompute the rows offered to the panel from the open cards and the
    /// row counts of their nested editors.  Cheap enough to run on every
    /// structural change; the panel only reads the result.
    fn relayout(&mut self, cx: &App) {
        let len = self.len();
        if self.items.len() != len {
            self.items = (0..len).map(|_| None).collect();
        }
        let mut layout = Vec::new();
        for i in 0..len {
            layout.push(Entry::Header(i));
            if !self.expanded.contains(&i) {
                continue;
            }
            let Some(Some((rows, _))) = self.items.get(i) else {
                continue;
            };
            for (k, (_, editor)) in rows.editors.iter().enumerate() {
                layout.push(Entry::Field(i, k));
                if let Some(nested) = &editor.rows {
                    layout.extend((0..nested.row_count(cx)).map(|ix| Entry::Nested(i, k, ix)));
                }
            }
        }
        self.layout = layout;
    }

    /// Repaint, and wake the panel so it re-reads the row count.  The wake is
    /// deferred: a change can arrive while the panel is mid-render.
    fn changed(&mut self, cx: &mut gpui::Context<Self>) {
        cx.notify();
        let wake = self.wake.lock().ok().and_then(|slot| slot.clone());
        if let Some(wake) = wake {
            cx.defer(move |cx| wake(cx));
        }
    }

    /// Push the current value into the rows that exist, and drop those whose
    /// shown fields changed (they rebuild straight away if their card is open).
    fn refresh(&mut self, window: &mut Window, cx: &mut App) {
        let value = self.value();
        if self.items.len() != value.len() {
            self.items = (0..value.len()).map(|_| None).collect();
        }
        for (i, item) in value.iter().enumerate() {
            let mask = shown(item);
            let stale = self
                .items
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(|(_, built)| *built != mask);
            if stale {
                self.items[i] = None;
            }
            if let Some(Some((rows, _))) = self.items.get(i) {
                rows.refresh(window, cx);
            } else {
                self.ensure_rows(i, window, cx);
            }
        }
        self.relayout(cx);
    }

    fn edit_list(&mut self, edit: ListEdit, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let mut items = self.value();
        if edit == ListEdit::Add && items.len() >= T::MAX_ITEMS {
            return;
        }
        let before = items.len();
        apply_list_edit(&mut items, edit, T::default);
        self.expanded = remap_expanded(&self.expanded, edit, before);
        if edit == ListEdit::Add && items.len() > before {
            self.expanded.insert(items.len() - 1);
        }
        self.fingerprint
            .store(fingerprint(&items), std::sync::atomic::Ordering::Release);
        if let Ok(mut current) = self.current.lock() {
            *current = items.clone();
        }
        // Rows follow indices: every item's rows start again.
        self.items = (0..items.len()).map(|_| None).collect();
        for i in 0..items.len() {
            self.ensure_rows(i, window, cx);
        }
        self.relayout(cx);
        (self.write_back)(Box::new(items), window, cx);
        self.changed(cx);
    }

    /// Open or close item `index`'s card.
    pub fn toggle(&mut self, index: usize, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.expanded.remove(&index) {
            self.expanded.insert(index);
            self.ensure_rows(index, window, cx);
        }
        self.relayout(cx);
        self.changed(cx);
    }

    /// Accept a value that changed elsewhere (undo, a preset, a blueprint).
    fn set_value(&mut self, value: &Vec<T>, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ptr = value as *const Vec<T> as usize;
        let stale = self.stale.swap(false, std::sync::atomic::Ordering::AcqRel);
        if ptr == self.last_ptr && !stale {
            return;
        }
        self.last_ptr = ptr;
        let hash = fingerprint(value);
        if hash == self.fingerprint.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        self.fingerprint
            .store(hash, std::sync::atomic::Ordering::Release);
        if let Ok(mut current) = self.current.lock() {
            *current = value.clone();
        }
        self.refresh(window, cx);
        self.changed(cx);
    }

    /// Row `ix` of the layout.
    fn render_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.layout.get(ix).copied() else {
            return gpui::div().into_any_element();
        };
        let field_view =
            |view: gpui::AnyView| {
                gpui::div()
                    .w_full()
                    .pl(gpui::px(12.))
                    .child(view.cached_auto_height(
                        gpui::StyleRefinement::default().w_full().flex_shrink_0(),
                    ))
                    .into_any_element()
            };
        match entry {
            Entry::Header(i) => {
                // The title is read in place: cloning the item to read it would
                // copy every nested list for each header drawn.
                let title = self
                    .current
                    .lock()
                    .ok()
                    .and_then(|v| v.get(i).map(|item| item.title(i)));
                let Some(title) = title else {
                    return gpui::div().into_any_element();
                };
                collapsible_item_card(
                    &self.id,
                    i,
                    title,
                    self.expanded.contains(&i),
                    std::iter::empty(),
                    |this: &mut Self, edit, window, cx| this.edit_list(edit, window, cx),
                    |this: &mut Self, index, window, cx| this.toggle(index, window, cx),
                    cx,
                )
            }
            Entry::Field(i, k) => match self.items.get(i) {
                Some(Some((rows, _))) => match rows.editors.get(k) {
                    Some((_, editor)) => field_view(editor.view.clone()),
                    None => gpui::div().into_any_element(),
                },
                _ => gpui::div().into_any_element(),
            },
            Entry::Nested(i, k, n) => {
                let nested = match self.items.get(i) {
                    Some(Some((rows, _))) => rows
                        .editors
                        .get(k)
                        .and_then(|(_, editor)| editor.rows.clone()),
                    _ => None,
                };
                match nested {
                    Some(nested) => gpui::div()
                        .w_full()
                        .pl(gpui::px(12.))
                        .child(nested.render_row(n, window, cx))
                        .into_any_element(),
                    None => gpui::div().into_any_element(),
                }
            }
        }
    }
}

/// Where the open cards sit after `edit` of a list that held `len` items.
pub fn remap_expanded(
    expanded: &std::collections::HashSet<usize>,
    edit: ListEdit,
    len: usize,
) -> std::collections::HashSet<usize> {
    expanded
        .iter()
        .filter_map(|&i| match edit {
            ListEdit::Up(m) if m > 0 && m < len => Some(if i == m {
                m - 1
            } else if i == m - 1 {
                m
            } else {
                i
            }),
            ListEdit::Down(m) if m + 1 < len => Some(if i == m {
                m + 1
            } else if i == m + 1 {
                m
            } else {
                i
            }),
            ListEdit::Remove(m) if m < len => match i.cmp(&m) {
                std::cmp::Ordering::Less => Some(i),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(i - 1),
            },
            _ => Some(i),
        })
        .collect()
}

/// The editor's own row: the title with its count, and the add button.  The
/// item cards are the panel's rows ([`VirtualRows`]).
impl<T: ReflectedListItem> gpui::Render for ReflectedListEditor<T> {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let len = self.len();
        let add = (len < T::MAX_ITEMS).then(|| {
            Button::new(SharedString::from(format!("{}-add", self.id)))
                .icon(IconName::Plus)
                .tooltip(format!("Add {}", T::NOUN))
                .xsmall()
                .ghost()
                .on_click(
                    cx.listener(|this, _, window, cx| this.edit_list(ListEdit::Add, window, cx)),
                )
        });
        let count = if T::MAX_ITEMS == usize::MAX {
            format!("{} ({})", self.label, len)
        } else {
            format!("{} ({}/{})", self.label, len, T::MAX_ITEMS)
        };
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_1()
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(count),
            )
            .children(add)
    }
}

/// The panel's handle on a list's rows.
struct ListRows<T: ReflectedListItem> {
    stale: Arc<std::sync::atomic::AtomicBool>,
    editor: gpui::Entity<ReflectedListEditor<T>>,
    wake: ListWake,
}

impl<T: ReflectedListItem> VirtualRows for ListRows<T> {
    fn row_count(&self, cx: &App) -> usize {
        self.editor.read(cx).layout.len()
    }

    fn render_row(&self, index: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        self.editor
            .update(cx, |editor, cx| editor.render_entry(index, window, cx))
    }

    fn on_change(&self, wake: Arc<dyn Fn(&mut App) + Send + Sync>) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(wake);
        }
    }

    fn mark_stale(&self) {
        self.stale.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// The editor factory for a `Vec<T>` property.
pub fn reflected_list_editor<T: ReflectedListItem>(
    args: &PropertyEditorArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> BoundPropertyEditor {
    let _ = window;
    let wake: ListWake = Default::default();
    let stale = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entity = cx.new(|cx| ReflectedListEditor::<T>::new(args, wake.clone(), stale.clone(), cx));
    BoundPropertyEditor::new(
        entity.clone(),
        |editor: &mut ReflectedListEditor<T>, value: &Vec<T>, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
    .with_rows(Arc::new(ListRows {
        stale,
        editor: entity,
        wake,
    }))
}

/// Register [`ReflectedListEditor`] as the inspector editor of `Vec<$item>`.
#[macro_export]
macro_rules! register_reflected_list_editor {
    ($item:ty) => {
        pulsar_reflection::inventory::submit! {
            pulsar_reflection::UiPropertyEditorHint {
                type_id: std::any::TypeId::of::<Vec<$item>>(),
                fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(
                    $crate::components::reflected_list::reflected_list_editor::<$item>,
                ),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_edits_move_remove_and_add() {
        let mut items = vec![1, 2, 3];
        apply_list_edit(&mut items, ListEdit::Up(1), || 0);
        assert_eq!(items, [2, 1, 3]);
        apply_list_edit(&mut items, ListEdit::Down(1), || 0);
        assert_eq!(items, [2, 3, 1]);
        apply_list_edit(&mut items, ListEdit::Up(0), || 0);
        apply_list_edit(&mut items, ListEdit::Down(2), || 0);
        assert_eq!(items, [2, 3, 1], "moves past the ends do nothing");
        apply_list_edit(&mut items, ListEdit::Remove(0), || 0);
        apply_list_edit(&mut items, ListEdit::Remove(9), || 0);
        assert_eq!(items, [3, 1]);
        apply_list_edit(&mut items, ListEdit::Add, || 7);
        assert_eq!(items, [3, 1, 7]);
    }
}
