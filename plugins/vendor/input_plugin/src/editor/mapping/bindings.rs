//! The bindings: every entry of the mapping, in its category.
//!
//! A row reads like a sentence: this key triggers that action. Click the key
//! or the action to change it, drag a row to move it, drop an action from the
//! list on the left to bind it.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::input::{InputEvent, InputState, TextInput};
use ui::menu::DropdownMenu;
use ui::scroll::ScrollbarAxis;
use ui::switch::Switch;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::keys::{catalog, Device};
use crate::library::Library;
use crate::model::behavior::ValueType;
use crate::model::validate::ActionSource;
use crate::model::{Entry, Severity};

use super::super::chooser::{ChoiceItem, Chooser, ChosenEvent};
use super::super::common::chrome::{empty_message, panel_header, severity_color, severity_icon};
use super::super::common::panel::dock_panel;
use super::super::drag::{ActionDrag, DragGhost, EntryDrag, Side};
use super::super::form::chip;
use super::super::keypicker::KeyPicker;
use super::super::action_choices;
use super::{action_ref, MapCtx};

pub fn device_icon(device: Device) -> IconName {
    match device {
        Device::Keyboard => IconName::Key,
        Device::Mouse => IconName::PcMouse,
        Device::Gamepad => IconName::Gamepad,
    }
}

const ROW_HEIGHT: f32 = 38.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickKind {
    Key,
    Action,
}

#[derive(Clone, Copy, Debug)]
struct OpenPicker {
    entry: u32,
    kind: PickKind,
    at: Point<Pixels>,
}

/// Where a drop would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hint {
    Row(u32, Side),
    Category(u32),
}

/// Everything a row shows, worked out before drawing.
struct RowView {
    entry: Entry,
    key_label: String,
    key_device: Option<Device>,
    action_name: Option<String>,
    action_type: Option<ValueType>,
    action_missing: bool,
    chips: Vec<String>,
    worst: Option<(Severity, String)>,
}

pub struct BindingsPanel {
    ctx: MapCtx,
    focus_handle: FocusHandle,
    collapsed: HashSet<u32>,
    renaming: Option<u32>,
    rename: Entity<InputState>,
    search: Entity<InputState>,
    hint: Option<Hint>,
    picker: Option<OpenPicker>,
    key_picker: Entity<KeyPicker>,
    action_chooser: Entity<Chooser>,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(BindingsPanel, "input-mapping-bindings", "Bindings");

impl BindingsPanel {
    pub fn new(ctx: MapCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rename = cx.new(|cx| InputState::new(window, cx).placeholder("Category name"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search bindings…"));
        let key_picker = cx.new(|cx| KeyPicker::new("", window, cx));
        let action_chooser = cx.new(|cx| {
            Chooser::new(Vec::new(), "", window, cx).with_empty_text("No actions in the project yet").with_pinned(
                vec![ChoiceItem::new("", "No action").detail("Leave this key unassigned"), ChoiceItem::new("+new", "New action…").detail("Make an action file beside this mapping")],
                cx,
            )
        });

        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sel, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&rename, window, |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(id) = this.renaming {
                        let name = input.read(cx).value().to_string();
                        this.ctx.rename_category(id, &name, cx);
                    }
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.renaming = None;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&key_picker, |this: &mut Self, _, event: &ChosenEvent, cx| {
                if let Some(open) = this.picker.take().filter(|o| o.kind == PickKind::Key) {
                    this.ctx.set_key(open.entry, &event.0, cx);
                }
                cx.notify();
            }),
            cx.subscribe(&key_picker, |this: &mut Self, _, _: &DismissEvent, cx| {
                this.picker = None;
                cx.notify();
            }),
            cx.subscribe(&action_chooser, |this: &mut Self, _, event: &ChosenEvent, cx| {
                let Some(open) = this.picker.take().filter(|o| o.kind == PickKind::Action) else { return };
                this.choose_action(open.entry, &event.0, cx);
                cx.notify();
            }),
            cx.subscribe(&action_chooser, |this: &mut Self, _, _: &DismissEvent, cx| {
                this.picker = None;
                cx.notify();
            }),
        ];
        Self {
            ctx,
            focus_handle: cx.focus_handle(),
            collapsed: HashSet::new(),
            renaming: None,
            rename,
            search,
            hint: None,
            picker: None,
            key_picker,
            action_chooser,
            _subscriptions: subscriptions,
        }
    }

    fn choose_action(&mut self, entry: u32, id: &str, cx: &mut Context<Self>) {
        self.ctx.assign_action(entry, id, cx);
    }

    fn open_picker(&mut self, entry: u32, kind: PickKind, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let asset = self.ctx.asset(cx);
        let current = asset.entry(entry);
        match kind {
            PickKind::Key => {
                let key = current.map(|e| e.key.clone()).unwrap_or_default();
                self.key_picker.update(cx, |p, cx| p.sync(&key, cx));
                let handle = self.key_picker.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            PickKind::Action => {
                let library = self.ctx.library(cx);
                let selected = current.and_then(|e| e.action_id()).map(|a| a.to_string()).unwrap_or_default();
                let items = action_choices(&library, None);
                self.action_chooser.update(cx, |c, cx| c.sync(items, &selected, cx));
                let handle = self.action_chooser.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        }
        self.picker = Some(OpenPicker { entry, kind, at });
        self.ctx.select_entry(entry, cx);
        cx.notify();
    }

    fn start_rename(&mut self, category: u32, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.ctx.asset(cx).category(category).map(|c| c.name.clone()).unwrap_or_default();
        self.renaming = Some(category);
        self.rename.update(cx, |i, cx| i.set_value(name, window, cx));
        let handle = self.rename.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    // ── Drops ──────────────────────────────────────────────────────────────

    fn set_hint(&mut self, hint: Hint, cx: &mut Context<Self>) {
        if self.hint != Some(hint) {
            self.hint = Some(hint);
            cx.notify();
        }
    }

    /// The category and position a drop at the current hint lands on.
    fn drop_spot(&self, moving: Option<u32>, cx: &App) -> Option<(u32, usize)> {
        let asset = self.ctx.doc.read(cx).asset();
        match self.hint? {
            Hint::Category(c) => Some((c, usize::MAX)),
            Hint::Row(entry, side) => {
                let at = asset.find(entry)?;
                let category = asset.categories[at.category].id;
                let mut index = at.index + usize::from(side == Side::After);
                if let Some(from) = moving.and_then(|m| asset.find(m)) {
                    // The moved row leaves before it is put back.
                    if from.category == at.category && from.index < index {
                        index -= 1;
                    }
                }
                Some((category, index))
            }
        }
    }

    fn drop_entry(&mut self, drag: &EntryDrag, cx: &mut Context<Self>) {
        if let Some((category, index)) = self.drop_spot(Some(drag.id), cx) {
            self.ctx.move_binding(drag.id, category, index, cx);
        }
        self.hint = None;
        cx.notify();
    }

    fn drop_action(&mut self, drag: &ActionDrag, cx: &mut Context<Self>) {
        if let Some((category, index)) = self.drop_spot(None, cx) {
            let library = self.ctx.library(cx);
            if let Some(reference) = action_ref(&library, &drag.id) {
                let index = (index != usize::MAX).then_some(index);
                self.ctx.add_binding(category, index, "", Some(reference), cx);
            }
        }
        self.hint = None;
        cx.notify();
    }

    // ── Building rows ──────────────────────────────────────────────────────

    fn row_views(&self, library: &Library, cx: &App) -> (Vec<(u32, String, Vec<RowView>)>, usize) {
        let asset = self.ctx.doc.read(cx).asset();
        let issues = asset.validate(library);
        let words: Vec<String> = self.search.read(cx).value().split_whitespace().map(str::to_lowercase).collect();
        let keys = catalog();
        let mut total = 0;
        let mut out = Vec::new();
        for category in &asset.categories {
            let mut rows = Vec::new();
            for entry in &category.entries {
                let key = keys.get(&entry.key);
                let info = entry.action_id().and_then(|id| library.action(id));
                let missing = entry.action.is_some() && info.is_none();
                let chips: Vec<String> = entry
                    .triggers
                    .iter()
                    .filter_map(|t| t.known().map(|t| t.summary()))
                    .chain(entry.modifiers.iter().filter_map(|m| m.known().map(|m| m.summary())))
                    .collect();
                let worst = issues
                    .iter()
                    .filter(|i| i.entry == Some(entry.id))
                    .max_by_key(|i| i.severity)
                    .map(|i| (i.severity, i.message.clone()));
                let view = RowView {
                    entry: entry.clone(),
                    key_label: key.map_or_else(|| if entry.key.is_empty() { "Choose a key".to_string() } else { entry.key.clone() }, |k| k.label.clone()),
                    key_device: key.map(|k| k.device),
                    action_name: info.as_ref().map(|a| a.name.clone()),
                    action_type: info.map(|a| a.value_type),
                    action_missing: missing,
                    chips,
                    worst,
                };
                let hay = format!("{} {} {} {}", view.key_label, view.action_name.as_deref().unwrap_or(""), entry.note, view.chips.join(" ")).to_lowercase();
                if words.iter().all(|w| hay.contains(w)) {
                    rows.push(view);
                }
                total += 1;
            }
            let name_matches = !words.is_empty() && words.iter().all(|w| category.name.to_lowercase().contains(w));
            if words.is_empty() || !rows.is_empty() || name_matches {
                out.push((category.id, category.name.clone(), rows));
            }
        }
        (out, total)
    }

    fn render_row(&self, view: RowView, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = view.entry.id;
        let hint = self.hint;
        let weak = cx.weak_entity();

        let key_button = {
            let label = view.key_label.clone();
            let missing = view.entry.key.is_empty();
            let unknown = !missing && view.key_device.is_none();
            let color = if unknown { theme.danger } else if missing { theme.warning } else { theme.foreground };
            h_flex()
                .id(("key", id))
                .h(px(26.0))
                .w(px(190.0))
                .flex_shrink_0()
                .px_2()
                .gap_1p5()
                .items_center()
                .rounded(px(5.0))
                .border_1()
                .border_color(if missing { theme.warning.opacity(0.6) } else { theme.border })
                .bg(theme.background)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.info))
                .child(Icon::new(view.key_device.map_or(IconName::Key, device_icon)).size(px(13.0)).text_color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().text_color(color).child(label))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let at = event.position();
                    this.open_picker(id, PickKind::Key, at, window, cx);
                    cx.stop_propagation();
                }))
        };

        let action_button = {
            let (label, color) = match (&view.action_name, view.action_missing, view.entry.action.is_some()) {
                (Some(name), _, _) => (name.clone(), theme.foreground),
                (None, true, _) => ("Missing action".to_string(), theme.danger),
                _ => ("Choose an action".to_string(), theme.warning),
            };
            let unassigned = view.entry.action.is_none();
            h_flex()
                .id(("action", id))
                .h(px(26.0))
                .min_w(px(150.0))
                .flex_1()
                .max_w(px(300.0))
                .px_2()
                .gap_1p5()
                .items_center()
                .rounded(px(5.0))
                .border_1()
                .border_color(if unassigned { theme.warning.opacity(0.6) } else { theme.border })
                .bg(theme.background)
                .cursor_pointer()
                .hover(|s| s.border_color(theme.info))
                .child(Icon::new(IconName::Flash).size(px(13.0)).text_color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().text_color(color).child(label))
                .children(view.action_type.map(|t| chip(t.short(), cx)))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let at = event.position();
                    this.open_picker(id, PickKind::Action, at, window, cx);
                    cx.stop_propagation();
                }))
        };

        let chips: Vec<AnyElement> = view.chips.iter().take(2).map(|c| chip(c.clone(), cx).into_any_element()).collect();
        let extra = view.chips.len().saturating_sub(2);
        let label = format!("{} → {}", view.key_label, view.action_name.clone().unwrap_or_default());

        let row = h_flex()
            .id(("binding", id))
            .group("binding-row")
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_2()
            .gap_2()
            .items_center()
            .rounded(px(6.0))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(selected, |r| r.bg(theme.accent.opacity(0.22)).border_color(theme.info.opacity(0.6)))
            .when(!selected, |r| r.hover(|s| s.bg(theme.accent.opacity(0.10))))
            .when(!view.entry.enabled, |r| r.opacity(0.55))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.ctx.select_entry(id, cx);
                window.focus(&this.focus_handle, cx);
            }))
            .on_drag(EntryDrag { id, label: label.clone() }, move |drag, _, _, cx| cx.new(|_| DragGhost { label: drag.label.clone().into(), icon: IconName::Key }))
            .on_drag_move::<EntryDrag>({
                let weak = weak.clone();
                move |event, _, cx| row_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drag_move::<ActionDrag>({
                let weak = weak.clone();
                move |event, _, cx| row_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drop(cx.listener(|this, drag: &EntryDrag, _, cx| this.drop_entry(drag, cx)))
            .on_drop(cx.listener(|this, drag: &ActionDrag, _, cx| this.drop_action(drag, cx)))
            .child(Icon::new(IconName::Drag).size(px(14.0)).text_color(theme.muted_foreground.opacity(0.7)).cursor_grab())
            .child(
                Switch::new(("enabled", id))
                    .checked(view.entry.enabled)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| this.ctx.set_enabled(id, *checked, cx))),
            )
            .child(key_button)
            .child(Icon::new(IconName::ArrowRight).size(px(13.0)).text_color(theme.muted_foreground))
            .child(action_button)
            .child(h_flex().flex_1().min_w_0().gap_1().overflow_hidden().children(chips).children((extra > 0).then(|| chip(format!("+{extra}"), cx))))
            .children(view.worst.map(|(severity, message)| {
                let color = severity_color(severity, cx);
                div().id(("issue", id)).flex_shrink_0().tooltip(move |window, cx| ui::tooltip::Tooltip::new(message.clone()).build(window, cx)).child(Icon::new(severity_icon(severity)).size(px(14.0)).text_color(color))
            }))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .opacity(0.0)
                    .group_hover("binding-row", |s| s.opacity(1.0))
                    .child(
                        Button::new(("dup", id))
                            .icon(IconName::Copy)
                            .ghost()
                            .xsmall()
                            .tooltip("Duplicate (Ctrl+D)")
                            .on_click(cx.listener(move |this, _, _, cx| this.ctx.duplicate_binding(id, cx))),
                    )
                    .child(
                        Button::new(("del", id))
                            .icon(IconName::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove (Delete)")
                            .on_click(cx.listener(move |this, _, _, cx| this.ctx.remove_binding(id, cx))),
                    ),
            );

        // A line where the drop will land.
        let row = match hint {
            Some(Hint::Row(h, Side::Before)) if h == id => row.border_t_2().border_color(theme.info),
            Some(Hint::Row(h, Side::After)) if h == id => row.border_b_2().border_color(theme.info),
            _ => row,
        };
        row.into_any_element()
    }

    fn render_category(&self, id: u32, name: String, rows: Vec<RowView>, searching: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let collapsed = !searching && self.collapsed.contains(&id);
        let selected_entry = self.ctx.selected_entry(cx);
        let selected_category = self.ctx.selected_category(cx) == Some(id) && selected_entry.is_none();
        let count = rows.len();
        let weak = cx.weak_entity();
        let into = self.hint == Some(Hint::Category(id));
        let renaming = self.renaming == Some(id);
        let entity = cx.entity();
        let category_count = self.ctx.doc.read(cx).asset().categories.len();
        let position = self.ctx.doc.read(cx).asset().categories.iter().position(|c| c.id == id).unwrap_or(0);

        let title: AnyElement = if renaming {
            div().w(px(220.0)).child(TextInput::new(&self.rename).small()).into_any_element()
        } else {
            div().text_sm().font_semibold().text_color(theme.foreground).child(name.clone()).into_any_element()
        };

        let menu_entity = entity.clone();
        let header = h_flex()
            .id(("category", id))
            .w_full()
            .h(px(32.0))
            .px_2()
            .gap_2()
            .items_center()
            .rounded(px(6.0))
            .border_1()
            .border_color(gpui::transparent_black())
            .bg(if selected_category { theme.accent.opacity(0.22) } else { theme.secondary })
            .when(into, |h| h.border_color(theme.info).bg(theme.info.opacity(0.18)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.ctx.select_category(id, cx);
                if event.click_count() >= 2 {
                    this.start_rename(id, window, cx);
                }
            }))
            .on_drag_move::<EntryDrag>({
                let weak = weak.clone();
                move |event, _, cx| category_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drag_move::<ActionDrag>({
                let weak = weak.clone();
                move |event, _, cx| category_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drop(cx.listener(|this, drag: &EntryDrag, _, cx| this.drop_entry(drag, cx)))
            .on_drop(cx.listener(|this, drag: &ActionDrag, _, cx| this.drop_action(drag, cx)))
            .child(
                Button::new(("fold", id))
                    .icon(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&id) {
                            this.collapsed.insert(id);
                        }
                        cx.notify();
                    })),
            )
            .child(title)
            .child(chip(format!("{count}"), cx))
            .child(div().flex_1())
            .child(
                Button::new(("add", id))
                    .icon(IconName::Plus)
                    .label("Binding")
                    .ghost()
                    .xsmall()
                    .tooltip("Add a binding to this category")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ctx.add_binding(id, None, "", None, cx);
                        this.collapsed.remove(&id);
                    })),
            )
            .child(
                Button::new(("more", id)).icon(IconName::Ellipsis).ghost().xsmall().popup_menu(move |menu, _, _| {
                    let (e1, e2, e3, e4) = (menu_entity.clone(), menu_entity.clone(), menu_entity.clone(), menu_entity.clone());
                    menu.menu_handler("Rename", move |window, cx| {
                        e1.update(cx, |this, cx| this.start_rename(id, window, cx));
                    })
                    .menu_handler("Move up", move |_, cx| {
                        e2.update(cx, |this, cx| this.ctx.move_category(id, position.saturating_sub(1), cx));
                    })
                    .menu_handler("Move down", move |_, cx| {
                        e3.update(cx, |this, cx| this.ctx.move_category(id, (position + 1).min(category_count.saturating_sub(1)), cx));
                    })
                    .menu_handler("Delete category", move |_, cx| {
                        e4.update(cx, |this, cx| this.ctx.remove_category(id, cx));
                    })
                }),
            );

        let body: Vec<AnyElement> = if collapsed {
            Vec::new()
        } else if rows.is_empty() {
            vec![div()
                .w_full()
                .h(px(44.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .border_1()
                .border_dashed()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if searching { "Nothing here matches" } else { "Drop an action here, or add a binding" })
                .into_any_element()]
        } else {
            rows.into_iter()
                .map(|row| {
                    let selected = selected_entry == Some(row.entry.id);
                    self.render_row(row, selected, cx)
                })
                .collect()
        };

        v_flex().w_full().gap_1().child(header).child(v_flex().w_full().gap_0p5().pl_3().children(body)).into_any_element()
    }
}

fn row_hint(weak: &WeakEntity<BindingsPanel>, id: u32, bounds: Bounds<Pixels>, position: Point<Pixels>, cx: &mut App) {
    // Every row hears every drag movement; only the one under the pointer may answer.
    if !bounds.contains(&position) {
        return;
    }
    let side = Side::at(bounds, position);
    weak.update(cx, |this, cx| this.set_hint(Hint::Row(id, side), cx)).ok();
}

fn category_hint(weak: &WeakEntity<BindingsPanel>, id: u32, bounds: Bounds<Pixels>, position: Point<Pixels>, cx: &mut App) {
    if !bounds.contains(&position) {
        return;
    }
    weak.update(cx, |this, cx| this.set_hint(Hint::Category(id), cx)).ok();
}

impl Render for BindingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library(cx);
        let (categories, total) = self.row_views(&library, cx);
        let searching = !self.search.read(cx).value().trim().is_empty();

        let blocks: Vec<AnyElement> = categories.into_iter().map(|(id, name, rows)| self.render_category(id, name, rows, searching, cx)).collect();

        let ctx = self.ctx.clone();
        let header = panel_header(
            IconName::Key,
            "Bindings",
            Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(200.0)).child(TextInput::new(&self.search).small().cleanable()))
                    .child(Button::new("add-category").icon(IconName::Plus).label("Category").small().outline().tooltip("Add a category").on_click(move |_, _, cx| {
                        ctx.add_category(cx);
                    }))
                    .into_any_element(),
            ),
            cx,
        );

        let body = if total == 0 && !searching {
            empty_message(
                IconName::Key,
                "Nothing is bound yet",
                "Add a binding to a category, or drag an action in from the list on the left. Pick an action there for ready-made sets of keys such as WASD.",
                cx,
            )
            .into_any_element()
        } else {
            v_flex().w_full().p_3().gap_3().children(blocks).into_any_element()
        };

        // The picker open over a row, if any.
        let overlay = self.picker.map(|open| {
            let content: AnyElement = match open.kind {
                PickKind::Key => self.key_picker.clone().into_any_element(),
                PickKind::Action => self.action_chooser.clone().into_any_element(),
            };
            deferred(
                anchored().position(open.at).anchor(Corner::TopLeft).snap_to_window_with_margin(px(8.0)).child(
                    div()
                        .occlude()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.picker = None;
                            cx.notify();
                        }))
                        .child(content),
                ),
            )
            .with_priority(2)
        });

        v_flex()
            .id("input-bindings")
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Only when the list itself has focus, not while typing in a field.
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let Some(id) = this.ctx.selected_entry(cx) else { return };
                match event.keystroke.key.as_str() {
                    "delete" | "backspace" => {
                        this.ctx.remove_binding(id, cx);
                        cx.stop_propagation();
                    }
                    "d" if event.keystroke.modifiers.secondary() => {
                        this.ctx.duplicate_binding(id, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_drop(cx.listener(|this, _: &EntryDrag, _, cx| {
                this.hint = None;
                cx.notify();
            }))
            .child(header)
            .child(v_flex().id("input-bindings-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(body))
            .children(overlay)
    }
}

