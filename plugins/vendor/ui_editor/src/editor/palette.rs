//! The palette (kinds of widget to add) and the library (the project's
//! widgets to nest inside this one).

use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::OpenAsset;
use ui::button::{Button, ButtonVariants};
use ui::input::{InputEvent, InputState, TextInput};
use ui::menu::context_menu::ContextMenuExt;
use ui::scroll::ScrollbarAxis;
use ui::tag::Tag;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::library::ProblemKind;
use crate::model::validate::{would_cycle, WidgetSource};
use crate::model::Kind;

use super::commands::Ctx;
use super::common::chrome::{empty_message, panel_header};
use super::common::panel::dock_panel;
use super::icons::{kind_icon, DragGhost, LibraryDrag, PaletteDrag, PALETTE_GROUPS};

const ROW_HEIGHT: f32 = 30.0;

// ── Palette ────────────────────────────────────────────────────────────────

pub struct PalettePanel {
    ctx: Ctx,
    search: Entity<InputState>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(PalettePanel, "widget-palette", "Palette");

impl PalettePanel {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search the palette"));
        let subscriptions = vec![cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        Self { ctx, search, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn item(&self, kind: Kind, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let info = kind.info();
        let ctx = self.ctx.clone();
        let description = info.description;
        h_flex()
            .id(SharedString::from(format!("palette-{}", kind.key())))
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_2()
            .gap_2()
            .items_center()
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover))
            .child(Icon::new(kind_icon(kind)).size(px(15.0)).text_color(theme.info))
            .child(div().flex_1().text_sm().text_color(theme.foreground).child(info.label))
            .tooltip(move |window, cx| ui::tooltip::Tooltip::new(description).build(window, cx))
            .on_click(move |event, _, cx| {
                if event.click_count() >= 2 {
                    ctx.add_node(kind, None, cx);
                }
            })
            .on_drag(PaletteDrag { kind }, move |drag, _, _, cx| {
                cx.new(|_| DragGhost { label: drag.kind.info().label.into(), icon: kind_icon(drag.kind) })
            })
    }
}

impl Render for PalettePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.search.read(cx).value().trim().to_lowercase();

        let mut groups = Vec::new();
        for group in PALETTE_GROUPS {
            let kinds: Vec<Kind> = Kind::ALL
                .into_iter()
                .filter(|k| *k != Kind::Instance && k.info().category == group)
                .filter(|k| query.is_empty() || k.info().label.to_lowercase().contains(&query) || k.info().description.to_lowercase().contains(&query))
                .collect();
            if kinds.is_empty() {
                continue;
            }
            groups.push(
                v_flex()
                    .w_full()
                    .px_1p5()
                    .pb_2()
                    .gap_0p5()
                    .child(div().px_2().py_1().text_xs().font_semibold().text_color(theme.muted_foreground).child(group.to_uppercase()))
                    .children(kinds.into_iter().map(|k| self.item(k, cx).into_any_element())),
            );
        }
        let empty = groups.is_empty();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Component, "Palette", None, cx))
            .child(div().px_2().py_1p5().border_b_1().border_color(theme.border.opacity(0.6)).child(TextInput::new(&self.search).small().cleanable()))
            .child(if empty {
                div().flex_1().child(empty_message(IconName::Search, "Nothing matches", "No kind of widget has that name", cx)).into_any_element()
            } else {
                v_flex().id("palette-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).py_1().children(groups).into_any_element()
            })
            .child(
                div()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(theme.border.opacity(0.6))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Drag onto the canvas or the hierarchy, or double-click to add."),
            )
    }
}

// ── Library ────────────────────────────────────────────────────────────────

pub struct LibraryPanel {
    ctx: Ctx,
    search: Entity<InputState>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(LibraryPanel, "widget-library", "Widgets");

impl LibraryPanel {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search the project's widgets"));
        let subscriptions = vec![
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self { ctx, search, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.lib.read(cx).library();
        let scanning = self.ctx.lib.read(cx).is_scanning();
        let this_id = self.ctx.widget(cx).id.clone();
        let this_path = self.ctx.doc.read(cx).path().map(|p| p.to_path_buf());
        let current = self.ctx.library_now(cx);
        let usage = library.usage_counts();
        let query = self.search.read(cx).value().trim().to_lowercase();

        let entries: Vec<_> = library.entries().into_iter().filter(|e| query.is_empty() || e.name.to_lowercase().contains(&query)).collect();
        let conflicts = library.conflicts().len();
        let needs_id: Vec<std::path::PathBuf> = library.problems().iter().filter(|p| p.kind == ProblemKind::NoId).map(|p| p.path.clone()).collect();
        let root = library.root().to_path_buf();

        let rows: Vec<AnyElement> = entries
            .iter()
            .map(|entry| {
                let id = entry.id.clone();
                let name = entry.name.clone();
                let is_self = entry.id == this_id;
                let cycles = !is_self && would_cycle(&current, &this_id, &entry.id);
                let ambiguous = library.is_ambiguous(&entry.id);
                let blocked = is_self || cycles || ambiguous;
                let used = usage.get(&entry.id).copied().unwrap_or(0);
                let relative = entry.path.strip_prefix(&root).unwrap_or(&entry.path).to_string_lossy().replace('\\', "/");
                let path = entry.path.clone();
                let ctx = self.ctx.clone();
                let (open_path, copy_id) = (entry.path.clone(), entry.id.to_string());
                let tip = if is_self {
                    "This is the widget you are editing".to_string()
                } else if cycles {
                    "It contains this widget, so it can't go inside it".to_string()
                } else if ambiguous {
                    "More than one file has this widget's id".to_string()
                } else {
                    format!("{relative}\nDrag onto the canvas, or double-click to add an instance")
                };

                h_flex()
                    .id(SharedString::from(format!("library-{}", entry.id)))
                    .w_full()
                    .h(px(ROW_HEIGHT + 4.0))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(px(5.0))
                    .when(!blocked, |d| d.cursor_pointer().hover(|s| s.bg(theme.list_hover)))
                    .opacity(if blocked { 0.5 } else { 1.0 })
                    .child(Icon::new(IconName::Component).size(px(15.0)).text_color(if is_self { theme.warning } else { theme.info }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().text_color(theme.foreground).truncate().child(name.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(relative)),
                    )
                    .children((used > 0).then(|| Tag::secondary().small().child(format!("{used}"))))
                    .tooltip(move |window, cx| ui::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .on_click({
                        let id = id.clone();
                        let name = name.clone();
                        move |event, _, cx| {
                            if event.click_count() >= 2 && !blocked {
                                ctx.add_instance(&id, &name, None, cx);
                            }
                        }
                    })
                    .when(!blocked, |d| {
                        let (id, name) = (id.clone(), name.clone());
                        d.on_drag(LibraryDrag { id: id.clone(), name: name.clone() }, move |drag, _, _, cx| {
                            cx.new(|_| DragGhost { label: drag.name.clone().into(), icon: IconName::Component })
                        })
                    })
                    .context_menu(move |menu, _, _| {
                        let (p, id_text) = (open_path.clone(), copy_id.clone());
                        let _ = &path;
                        menu.menu_handler("Open", move |window, cx| window.dispatch_action(Box::new(OpenAsset { path: p.clone() }), cx))
                            .menu_handler("Copy id", move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(id_text.clone())))
                    })
                    .into_any_element()
            })
            .collect();

        let refresh = {
            let lib = self.ctx.lib.clone();
            Button::new("library-refresh").icon(IconName::Refresh).ghost().small().tooltip("Look for widgets again").on_click(move |_, _, cx| lib.update(cx, |l, cx| l.refresh(cx)))
        };

        let stamp = (!needs_id.is_empty()).then(|| {
            let lib = self.ctx.lib.clone();
            let paths = needs_id.clone();
            v_flex()
                .w_full()
                .px_3()
                .py_2()
                .gap_1p5()
                .border_t_1()
                .border_color(theme.border.opacity(0.6))
                .child(div().text_xs().text_color(theme.foreground.opacity(0.9)).child(format!("{} widget file(s) have no id yet, so nothing can refer to them.", paths.len())))
                .child(Button::new("library-assign-ids").label("Give them ids").small().on_click(move |_, _, cx| {
                    for p in &paths {
                        if let Err(error) = crate::library::stamp_file(p) {
                            tracing::warn!("couldn't give {} an id: {error}", p.display());
                        }
                    }
                    lib.update(cx, |l, cx| l.refresh(cx));
                }))
        });

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(
                IconName::Component,
                "Widgets",
                Some(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .children(scanning.then(|| Tag::info().small().child("Scanning")))
                        .children((conflicts > 0).then(|| Tag::warning().small().child(format!("{conflicts} id conflict(s)"))))
                        .child(refresh)
                        .into_any_element(),
                ),
                cx,
            ))
            .child(div().px_2().py_1p5().border_b_1().border_color(theme.border.opacity(0.6)).child(TextInput::new(&self.search).small().cleanable()))
            .child(if rows.is_empty() {
                div()
                    .flex_1()
                    .child(empty_message(
                        IconName::Component,
                        if query.is_empty() { "No other widgets yet" } else { "Nothing matches" },
                        if query.is_empty() { "Create more .widget files and they appear here, ready to nest" } else { "No widget has that name" },
                        cx,
                    ))
                    .into_any_element()
            } else {
                v_flex().id("library-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).p_1p5().gap_0p5().children(rows).into_any_element()
            })
            .children(stamp)
    }
}
