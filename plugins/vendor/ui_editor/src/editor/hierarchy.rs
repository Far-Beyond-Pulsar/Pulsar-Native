//! The hierarchy: the widget's tree as an outline, for selecting, renaming,
//! reordering and nesting widgets, and hiding or locking them in the designer.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::input::{InputEvent, InputState, TextInput};
use ui::menu::context_menu::ContextMenuExt;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use crate::model::{Kind, NodeId};

use super::commands::{Ctx, DropSpot};
use super::common::chrome::{empty_message, panel_header};
use super::common::panel::dock_panel;
use super::icons::{kind_icon, DragGhost, LibraryDrag, NodeDrag, PaletteDrag};

const ROW_HEIGHT: f32 = 26.0;

/// One visible line of the outline.
#[derive(Clone)]
struct Row {
    id: NodeId,
    depth: usize,
    name: String,
    kind: Kind,
    has_children: bool,
    expanded: bool,
    hidden: bool,
    locked: bool,
    /// An instance that can't be shown, with the reason.
    problem: Option<String>,
    /// For an instance, what it is an instance of.
    detail: Option<String>,
}

/// Where a dragged item lands relative to the row it is over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Where {
    Before,
    Inside,
    After,
}

pub struct HierarchyPanel {
    ctx: Ctx,
    search: Entity<InputState>,
    rename: Option<(NodeId, Entity<InputState>)>,
    scroll: UniformListScrollHandle,
    hint: Option<(NodeId, Where)>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(HierarchyPanel, "widget-hierarchy", "Hierarchy");

impl HierarchyPanel {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search widgets"));
        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sel, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self { ctx, search, rename: None, scroll: UniformListScrollHandle::new(), hint: None, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let widget = self.ctx.widget(cx);
        let library = self.ctx.lib.read(cx).library();
        let query = self.search.read(cx).value().trim().to_lowercase();

        // With a search, show matches and the path to them, everything open.
        let mut keep: Option<HashSet<NodeId>> = None;
        if !query.is_empty() {
            let mut set = HashSet::new();
            widget.root.walk(&mut |n| {
                if n.name.to_lowercase().contains(&query) || n.info().label.to_lowercase().contains(&query) {
                    if let Some(path) = widget.path_to(n.id) {
                        set.extend(path);
                    }
                }
            });
            keep = Some(set);
        }

        let mut rows = Vec::new();
        fn visit(node: &crate::model::Node, depth: usize, keep: &Option<HashSet<NodeId>>, library: &crate::library::Library, rows: &mut Vec<Row>) {
            if keep.as_ref().is_some_and(|k| !k.contains(&node.id)) {
                return;
            }
            let (mut problem, mut detail) = (None, None);
            if let Some(instance) = &node.instance {
                use crate::model::validate::WidgetSource;
                detail = library.name(&instance.widget).or_else(|| (!instance.name_hint.is_empty()).then(|| instance.name_hint.clone()));
                if library.is_ambiguous(&instance.widget) {
                    problem = Some("More than one file has this widget's id".to_string());
                } else if library.widget(&instance.widget).is_none() {
                    problem = Some("This widget can't be found".to_string());
                }
            }
            let expanded = keep.is_some() || node.editor.expanded;
            rows.push(Row {
                id: node.id,
                depth,
                name: node.name.clone(),
                kind: node.kind,
                has_children: !node.children.is_empty(),
                expanded,
                hidden: node.editor.hidden,
                locked: node.editor.locked,
                problem,
                detail,
            });
            if expanded {
                for child in &node.children {
                    visit(child, depth + 1, keep, library, rows);
                }
            }
        }
        visit(&widget.root, 0, &keep, &library, &mut rows);
        rows
    }

    pub fn begin_rename(&mut self, id: NodeId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ctx.widget(cx).node(id).map(|n| n.name.clone()) else { return };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, move |this: &mut Self, input, event: &InputEvent, _window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().to_string();
                this.ctx.rename(id, &text, cx);
                this.rename = None;
                cx.notify();
            }
            InputEvent::Blur => {
                this.rename = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        self.rename = Some((id, input));
        cx.notify();
    }

    fn click(&mut self, id: NodeId, modifiers: Modifiers, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        if clicks >= 2 && !modifiers.shift && !modifiers.secondary() {
            self.begin_rename(id, window, cx);
            return;
        }
        if modifiers.secondary() || modifiers.shift {
            self.ctx.sel.update(cx, |s, cx| s.toggle(id, cx));
        } else {
            self.ctx.select(vec![id], cx);
        }
    }

    /// Where a drop on `row` goes, from the pointer's height in the row.
    fn spot(&self, row: NodeId, at: Where, cx: &App) -> Option<DropSpot> {
        let widget = self.ctx.widget(cx);
        match at {
            Where::Inside if widget.can_accept(row).is_ok() => Some(DropSpot { container: row, index: None, at: None }),
            Where::Inside | Where::After | Where::Before => {
                let parent = widget.parent_of(row)?;
                let index = widget.index_in_parent(row)?;
                Some(DropSpot { container: parent, index: Some(if at == Where::Before { index } else { index + 1 }), at: None })
            }
        }
    }

    fn render_row(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.ctx.sel.read(cx).contains(row.id);
        let id = row.id;
        let ctx = self.ctx.clone();
        let renaming = self.rename.as_ref().filter(|(r, _)| *r == id).map(|(_, input)| input.clone());
        let hint = self.hint.filter(|(h, _)| *h == id).map(|(_, w)| w);

        let color = match row.kind {
            Kind::Instance => theme.info,
            Kind::NamedSlot => theme.warning,
            k if k.info().children != crate::model::schema::Children::None => theme.muted_foreground,
            _ => theme.foreground.opacity(0.85),
        };
        let dim = if row.hidden { 0.45 } else { 1.0 };

        let label = if let Some(input) = renaming {
            div().flex_1().min_w_0().child(TextInput::new(&input).xsmall()).into_any_element()
        } else {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .items_baseline()
                .child(div().text_sm().text_color(theme.foreground.opacity(dim)).truncate().child(row.name.clone()))
                .children(row.detail.as_ref().map(|d| div().text_xs().text_color(theme.muted_foreground).truncate().child(format!("\u{2192} {d}"))))
                .into_any_element()
        };

        let ctx_menu = self.ctx.clone();
        let drag_label = row.name.clone();
        let drag_ids = {
            let sel = self.ctx.sel.read(cx);
            if sel.contains(id) {
                sel.ids().to_vec()
            } else {
                vec![id]
            }
        };

        let weak = cx.weak_entity();
        let base = h_flex()
            .id(("hierarchy-row", id.0 as usize))
            .relative()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_1()
            .gap_1()
            .items_center()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.info.opacity(0.22)))
            .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
            .child(div().w(px(row.depth as f32 * 14.0)).flex_shrink_0())
            .child(if row.has_children {
                let ctx = ctx.clone();
                let expanded = row.expanded;
                div()
                    .id(("hierarchy-twist", id.0 as usize))
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0)).text_color(theme.muted_foreground))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        ctx.set_expanded(id, !expanded, cx);
                    })
                    .into_any_element()
            } else {
                div().size(px(16.0)).flex_shrink_0().into_any_element()
            })
            .child(Icon::new(kind_icon(row.kind)).size(px(14.0)).text_color(color.opacity(dim)))
            .child(label)
            .children(row.problem.as_ref().map(|p| {
                let text = p.clone();
                div()
                    .id(("hierarchy-problem", id.0 as usize))
                    .child(Icon::new(IconName::TriangleAlert).size(px(13.0)).text_color(theme.danger))
                    .tooltip(move |window, cx| ui::tooltip::Tooltip::new(text.clone()).build(window, cx))
            }))
            .child(self.visibility_button(id, row.hidden, cx))
            .child(self.lock_button(id, row.locked, cx))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.click(id, event.modifiers(), event.click_count(), window, cx);
            }))
            .on_mouse_down(MouseButton::Right, {
                let ctx = self.ctx.clone();
                move |_, _, cx| {
                    if !ctx.sel.read(cx).contains(id) {
                        ctx.select(vec![id], cx);
                    }
                }
            })
            .on_drag(NodeDrag { ids: drag_ids, label: drag_label.clone() }, {
                let icon = kind_icon(row.kind);
                move |drag, _, _, cx| {
                    cx.new(|_| DragGhost { label: drag.label.clone().into(), icon: icon.clone() })
                }
            })
            .on_drag_move::<NodeDrag>({
                let weak = weak.clone();
                move |event, _, cx| set_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drag_move::<PaletteDrag>({
                let weak = weak.clone();
                move |event, _, cx| set_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drag_move::<LibraryDrag>({
                let weak = weak.clone();
                move |event, _, cx| set_hint(&weak, id, event.bounds, event.event.position, cx)
            })
            .on_drop(cx.listener(move |this, drag: &NodeDrag, _, cx| {
                if let Some((row, at)) = this.hint.take() {
                    if let Some(spot) = this.spot(row, at, cx) {
                        this.ctx.move_nodes(&drag.ids, spot, cx);
                    }
                }
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &PaletteDrag, _, cx| {
                if let Some((row, at)) = this.hint.take() {
                    if let Some(spot) = this.spot(row, at, cx) {
                        this.ctx.add_node(drag.kind, Some(spot), cx);
                    }
                }
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &LibraryDrag, _, cx| {
                if let Some((row, at)) = this.hint.take() {
                    if let Some(spot) = this.spot(row, at, cx) {
                        this.ctx.add_instance(&drag.id, &drag.name, Some(spot), cx);
                    }
                }
                cx.notify();
            }));

        // A line where the drop will land, or a box around the new parent.
        let base = match hint {
            Some(Where::Before) => base.border_t_2().border_color(theme.info),
            Some(Where::After) => base.border_b_2().border_color(theme.info),
            Some(Where::Inside) => base.bg(theme.info.opacity(0.18)).border_1().border_color(theme.info),
            None => base,
        };

        base.context_menu(move |menu, window, cx| super::menus::node_menu(menu, &ctx_menu, window, cx)).into_any_element()
    }

    fn visibility_button(&self, id: NodeId, hidden: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ctx = self.ctx.clone();
        div()
            .id(("hierarchy-eye", id.0 as usize))
            .size(px(18.0))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(px(3.0))
            .hover(|s| s.bg(theme.foreground.opacity(0.1)))
            .child(
                Icon::new(if hidden { IconName::EyeOff } else { IconName::Eye })
                    .size(px(13.0))
                    .text_color(if hidden { theme.warning } else { theme.muted_foreground.opacity(0.5) }),
            )
            .tooltip(move |window, cx| ui::tooltip::Tooltip::new(if hidden { "Hidden in the designer" } else { "Hide in the designer" }).build(window, cx))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                ctx.toggle_hidden(id, cx);
            })
    }

    fn lock_button(&self, id: NodeId, locked: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ctx = self.ctx.clone();
        div()
            .id(("hierarchy-lock", id.0 as usize))
            .size(px(18.0))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(px(3.0))
            .hover(|s| s.bg(theme.foreground.opacity(0.1)))
            .child(
                Icon::new(if locked { IconName::Lock } else { IconName::LockSlash })
                    .size(px(13.0))
                    .text_color(if locked { theme.warning } else { theme.muted_foreground.opacity(0.35) }),
            )
            .tooltip(move |window, cx| ui::tooltip::Tooltip::new(if locked { "Locked: can't be picked in the designer" } else { "Lock in the designer" }).build(window, cx))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                ctx.toggle_locked(id, cx);
            })
    }
}

/// Record which part of row `id` the pointer is over during a drag.
fn set_hint(weak: &WeakEntity<HierarchyPanel>, id: NodeId, bounds: Bounds<Pixels>, position: Point<Pixels>, cx: &mut App) {
    // Every row hears every drag movement; only the one under the pointer may answer.
    if !bounds.contains(&position) {
        return;
    }
    let t = ((position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0);
    let at = if t < 0.25 {
        Where::Before
    } else if t > 0.75 {
        Where::After
    } else {
        Where::Inside
    };
    weak.update(cx, |this, cx| {
        if this.hint != Some((id, at)) {
            this.hint = Some((id, at));
            cx.notify();
        }
    })
    .ok();
}

impl Render for HierarchyPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.rows(cx);
        let count = rows.len();
        let total = self.ctx.widget(cx).root.count();
        let searching = !self.search.read(cx).value().trim().is_empty();
        let _ = window;

        let body = if count == 0 {
            div().size_full().child(empty_message(IconName::Search, "Nothing matches", "No widget has that name", cx)).into_any_element()
        } else {
            uniform_list(
                "hierarchy-list",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let rows = this.rows(cx);
                    range.filter_map(|ix| rows.get(ix).cloned()).map(|row| this.render_row(&row, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };

        let ctx = self.ctx.clone();
        v_flex()
            .id("hierarchy-panel")
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "f2" {
                    if let Some(id) = this.ctx.primary(cx) {
                        this.begin_rename(id, window, cx);
                        cx.stop_propagation();
                    }
                } else if super::keys::handle(&this.ctx, event, None, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(panel_header(
                IconName::ListSelect,
                "Hierarchy",
                Some(div().text_xs().text_color(theme.muted_foreground).child(if searching { format!("{count} of {total}") } else { format!("{total} widgets") }).into_any_element()),
                cx,
            ))
            .child(div().px_2().py_1p5().border_b_1().border_color(theme.border.opacity(0.6)).child(TextInput::new(&self.search).small().cleanable()))
            .child(div().flex_1().min_h_0().on_mouse_down(MouseButton::Left, move |_, _, cx| {
                // Clicking the gaps below the rows clears the selection.
                let _ = &ctx;
                let _ = cx;
            }).child(body))
    }
}
