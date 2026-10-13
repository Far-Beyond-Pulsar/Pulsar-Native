//! The details panel: everything about what is selected, built from the
//! property schema. With nothing selected it shows the widget itself.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::OpenAsset;
use serde_json::{json, Value as Json};
use ui::button::{Button, ButtonVariants};
use ui::input::{InputEvent, InputState, NumberInput, TextInput};
use ui::menu::DropdownMenu;
use ui::scroll::ScrollbarAxis;
use ui::switch::Switch;
use ui::tag::Tag;
use ui::{h_flex, v_flex, ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, StyledExt};

use crate::model::node::{Arg, Node};
use crate::model::schema::{Children, PropDef, PropType, SlotKind};
use crate::model::validate::WidgetSource;
use crate::model::ParamId;

use super::canvas::geometry::{AnchorPreset, Pin};
use super::commands::Ctx;
use super::common::chrome::{empty_message, panel_header};
use super::common::panel::dock_panel;
use super::fields::{Fields, NumberSpec};
use super::icons::kind_icon;
use super::value_editor::{type_for_class, value_editor};

pub struct DetailsPanel {
    ctx: Ctx,
    fields: Fields<Self>,
    filter: Entity<InputState>,
    collapsed: HashSet<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(DetailsPanel, "widget-details", "Details");

impl DetailsPanel {
    pub fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search properties"));
        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sel, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let fields = Fields::new(ctx.lib.read(cx).root().to_path_buf());
        Self { ctx, fields, filter, collapsed: HashSet::new(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    // ── Reading the model ──────────────────────────────────────────────────

    fn prop_json(&self, key: &str, cx: &App) -> Option<Json> {
        let node = self.ctx.widget(cx).node(self.ctx.primary(cx)?)?;
        Some(node.view().json(key).into_owned())
    }

    fn slot_json(&self, key: &str, cx: &App) -> Option<Json> {
        let id = self.ctx.primary(cx)?;
        let widget = self.ctx.widget(cx);
        let node = widget.node(id)?;
        Some(node.slot_view(widget.slot_kind_of(id)).json(key).into_owned())
    }

    // ── Pieces ─────────────────────────────────────────────────────────────

    fn section(&mut self, id: &str, title: impl Into<SharedString>, body: Vec<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let open = !self.collapsed.contains(id);
        let key = id.to_string();
        v_flex()
            .w_full()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                h_flex()
                    .id(SharedString::from(format!("section-{id}")))
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.list_hover))
                    .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.0)).text_color(theme.muted_foreground))
                    .child(div().text_xs().font_semibold().text_color(theme.foreground).child(title.into()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&key) {
                            this.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    })),
            )
            .children(open.then(|| v_flex().w_full().px_3().pb_2().gap_1p5().children(body)))
            .into_any_element()
    }

    fn labelled(&self, label: impl Into<SharedString>, tip: &str, control: AnyElement, trailing: Option<AnyElement>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let tip = tip.to_string();
        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            .child({
                let label = label.into();
                div()
                    .id(SharedString::from(format!("label-{label}")))
                    .w(px(96.0))
                    .flex_shrink_0()
                    .pt(px(5.0))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(label)
                    .when(!tip.is_empty(), |d| d.tooltip(move |window, cx| ui::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
            })
            .child(control)
            .children(trailing.map(|t| div().flex_shrink_0().child(t)))
            .into_any_element()
    }

    // ── A property ─────────────────────────────────────────────────────────

    fn prop_row(&mut self, node: &Node, def: &'static PropDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = def.key;
        let widget = self.ctx.widget(cx);
        let driver = widget.driver_of(node.id, key).map(|p| (p.id, p.name.clone(), p.default.clone()));
        let set_here = node.props.get(key).is_some_and(|v| !v.is_null());
        let current = match &driver {
            Some((_, _, default)) => default.clone(),
            None => node.view().json(key).into_owned(),
        };
        let compatible: Vec<(ParamId, String)> = widget.parameters.iter().filter(|p| p.class == def.ty.class()).map(|p| (p.id, p.name.clone())).collect();

        let is_image = matches!(def.ty, PropType::Image);
        let ctx = self.ctx.clone();
        let control = if def.optional && !set_here && driver.is_none() {
            // An optional property that is off: just the switch to turn it on.
            let ctx = ctx.clone();
            let default_on = default_for_optional(&def.ty);
            Switch::new(SharedString::from(format!("opt-{key}")))
                .checked(false)
                .on_click(move |_, _, cx| ctx.set_prop(key, default_on.clone(), cx))
                .into_any_element()
        } else {
            let get = move |this: &Self, cx: &App| this.prop_json(key, cx);
            let set = move |this: &mut Self, value: Json, cx: &mut Context<Self>| {
                let value = match (&value, is_image) {
                    (Json::String(s), true) => json!(this.ctx.project_relative(s, cx)),
                    _ => value,
                };
                this.ctx.clone().set_prop(key, value, cx);
            };
            let editor = value_editor(&mut self.fields, &format!("p:{key}"), def.ty, &current, driver.is_some(), get, set, window, cx);
            if def.optional {
                let ctx = ctx.clone();
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .items_start()
                    .child(Switch::new(SharedString::from(format!("opt-{key}"))).checked(true).on_click(move |_, _, cx| ctx.reset_prop(key, cx)))
                    .child(editor)
                    .into_any_element()
            } else {
                editor
            }
        };

        let menu = self.prop_menu(node.id, def, set_here, driver.clone().map(|(id, name, _)| (id, name)), compatible);
        let tip = match &driver {
            Some((_, name, _)) => format!("Driven by the parameter {name}. Change its default in the Parameters panel."),
            None => def.tip.to_string(),
        };
        self.labelled(
            if driver.is_some() { SharedString::from(format!("\u{1f517} {}", def.label)) } else { SharedString::from(def.label) },
            &tip,
            control,
            Some(menu),
            cx,
        )
    }

    fn prop_menu(&self, node: crate::model::NodeId, def: &'static PropDef, set_here: bool, driver: Option<(ParamId, String)>, compatible: Vec<(ParamId, String)>) -> AnyElement {
        let ctx = self.ctx.clone();
        let key = def.key;
        Button::new(SharedString::from(format!("menu-{key}")))
            .icon(IconName::EllipsisVertical)
            .ghost()
            .xsmall()
            .popup_menu(move |menu, window, cx| {
                let mut menu = menu;
                let c = ctx.clone();
                menu = menu.menu_handler_with_icon_and_disabled("Reset to default", IconName::Undo, !set_here, move |_, cx| c.reset_prop(key, cx));
                menu = menu.separator();
                match &driver {
                    Some((_, name)) => {
                        let c = ctx.clone();
                        menu = menu.menu_handler(format!("Unlink from {name}"), move |_, cx| c.unlink_property(node, key, cx));
                    }
                    None => {
                        let c = ctx.clone();
                        menu = menu.menu_handler("Make a parameter from this", move |_, cx| {
                            c.parameterize(node, key, cx);
                        });
                        if !compatible.is_empty() {
                            let (c, list) = (ctx.clone(), compatible.clone());
                            menu = menu.submenu("Link to parameter", window, cx, move |mut m, _, _| {
                                for (id, name) in &list {
                                    let (c, id) = (c.clone(), *id);
                                    m = m.menu_handler(name.clone(), move |_, cx| c.link_parameter(id, node, key, cx));
                                }
                                m
                            });
                        }
                    }
                }
                menu
            })
            .into_any_element()
    }

    // ── Sections for a selected widget ─────────────────────────────────────

    fn identity(&mut self, node: &Node, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = node.id;
        let ctx = self.ctx.clone();
        let name = self.fields.text(
            "name".into(),
            false,
            "Name",
            &node.name,
            |this, cx| this.ctx.primary(cx).and_then(|id| this.ctx.widget(cx).node(id)).map(|n| n.name.clone()),
            |this, text, cx| {
                if let Some(id) = this.ctx.primary(cx) {
                    this.ctx.rename(id, &text, cx);
                }
            },
            window,
            cx,
        );
        let multiple = self.ctx.sel.read(cx).len();
        let info = node.info();
        v_flex()
            .w_full()
            .p_2()
            .gap_1p5()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(kind_icon(node.kind)).size(px(18.0)).text_color(theme.info))
                    .child(div().flex_1().min_w_0().child(TextInput::new(&name).small()))
                    .child({
                        let ctx = ctx.clone();
                        Button::new("details-hidden").icon(if node.editor.hidden { IconName::EyeOff } else { IconName::Eye }).ghost().small().selected(node.editor.hidden).tooltip("Hide in the designer").on_click(move |_, _, cx| ctx.toggle_hidden(id, cx))
                    })
                    .child({
                        let ctx = ctx.clone();
                        Button::new("details-locked").icon(if node.editor.locked { IconName::Lock } else { IconName::LockSlash }).ghost().small().selected(node.editor.locked).tooltip("Lock in the designer").on_click(move |_, _, cx| ctx.toggle_locked(id, cx))
                    }),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Tag::secondary().small().child(info.label))
                    .children((multiple > 1).then(|| Tag::info().small().child(format!("{multiple} selected: edits apply to all")))),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(info.description))
            .into_any_element()
    }

    fn property_sections(&mut self, node: &Node, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let info = node.info();
        // The kind's own categories first, then the ones every widget has.
        let common: HashSet<&str> = crate::model::schema::COMMON.iter().map(|d| d.category).collect();
        let mut order: Vec<&'static str> = Vec::new();
        for def in &info.props {
            if !common.contains(def.category) && !order.contains(&def.category) {
                order.push(def.category);
            }
        }
        for def in &info.props {
            if common.contains(def.category) && !order.contains(&def.category) {
                order.push(def.category);
            }
        }
        let mut sections = Vec::new();
        for category in order {
            let defs: Vec<&'static PropDef> = info
                .props
                .iter()
                .filter(|d| d.category == category)
                .filter(|d| query.is_empty() || d.label.to_lowercase().contains(&query) || category.to_lowercase().contains(&query))
                .collect();
            if defs.is_empty() {
                continue;
            }
            let rows: Vec<AnyElement> = defs.into_iter().map(|d| self.prop_row(node, d, window, cx)).collect();
            sections.push(self.section(&format!("cat-{category}"), category, rows, cx));
        }
        sections
    }

    // ── Layout in the parent ───────────────────────────────────────────────

    fn slot_section(&mut self, node: &Node, parent_kind: SlotKind, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if parent_kind == SlotKind::None {
            return None;
        }
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let mut rows: Vec<AnyElement> = Vec::new();
        if parent_kind == SlotKind::Canvas {
            rows.push(self.anchor_picker(node, cx));
            rows.push(self.offsets_editor(node, window, cx));
        }
        for def in parent_kind.props() {
            if parent_kind == SlotKind::Canvas && def.key == "offsets" || def.key == "anchor_min" || def.key == "anchor_max" {
                continue;
            }
            if !query.is_empty() && !def.label.to_lowercase().contains(&query) && !"layout slot".contains(&query) {
                continue;
            }
            let key = def.key;
            let current = node.slot_view(parent_kind).json(key).into_owned();
            let is_set = node.slot.get(key).is_some();
            let get = move |this: &Self, cx: &App| this.slot_json(key, cx);
            let set = move |this: &mut Self, value: Json, cx: &mut Context<Self>| this.ctx.clone().set_slot_prop(key, value, cx);
            let editor = value_editor(&mut self.fields, &format!("s:{key}"), def.ty, &current, false, get, set, window, cx);
            let ctx = self.ctx.clone();
            let reset = Button::new(SharedString::from(format!("reset-slot-{key}")))
                .icon(IconName::Undo)
                .ghost()
                .xsmall()
                .disabled(!is_set)
                .tooltip("Reset to default")
                .on_click(move |_, _, cx| ctx.reset_slot_prop(key, cx))
                .into_any_element();
            rows.push(self.labelled(def.label, def.tip, editor, Some(reset), cx));
        }
        if rows.is_empty() {
            return None;
        }
        Some(self.section("slot", "Layout", rows, cx))
    }

    fn anchor_picker(&self, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let v = node.slot_view(SlotKind::Canvas);
        let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
        let current = AnchorPreset::matching(amin, amax);
        let mut grid = v_flex().gap(px(2.0));
        for row in 0..4 {
            let mut line = h_flex().gap(px(2.0));
            for col in 0..4 {
                let preset = AnchorPreset::at(row, col);
                let ctx = self.ctx.clone();
                let selected = current == Some(preset);
                let tip = preset.label();
                line = line.child(
                    div()
                        .id(SharedString::from(format!("anchor-{row}-{col}")))
                        .relative()
                        .size(px(26.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(if selected { theme.info } else { theme.border })
                        .bg(if selected { theme.info.opacity(0.18) } else { theme.secondary })
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(anchor_glyph(preset, selected, theme.info, theme.muted_foreground))
                        .tooltip(move |window, cx| ui::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(move |event, _, cx| ctx.set_anchors(preset, !event.modifiers().shift, cx)),
                );
            }
            grid = grid.child(line);
        }
        h_flex()
            .w_full()
            .gap_3()
            .items_start()
            .child(grid)
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(div().text_xs().font_semibold().text_color(theme.foreground).child("Anchors"))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(match current {
                        Some(p) => p.label(),
                        None => "Custom anchors".to_string(),
                    }))
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Click to pin the widget to the panel. Shift keeps its alignment.")),
            )
            .into_any_element()
    }

    /// The canvas offsets, labelled for what they mean given the anchors.
    fn offsets_editor(&mut self, node: &Node, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let v = node.slot_view(SlotKind::Canvas);
        let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
        let offsets = v.quad("offsets");
        let (stretch_x, stretch_y) = ((amin.x - amax.x).abs() > 1e-6, (amin.y - amax.y).abs() > 1e-6);
        let auto = v.bool("auto_size");
        // Position and size, or margins, per axis.
        let cells: [(&str, usize, bool); 4] = [
            (if stretch_x { "Left" } else { "X" }, 0, false),
            (if stretch_y { "Top" } else { "Y" }, 1, false),
            (if stretch_x { "Right" } else { "Width" }, 2, !stretch_x && auto),
            (if stretch_y { "Bottom" } else { "Height" }, 3, !stretch_y && auto),
        ];
        let theme = cx.theme().clone();
        let spec = NumberSpec { step: 1.0, min: -100_000.0, max: 100_000.0, decimals: 1 };
        let mut inputs = Vec::new();
        for (label, i, disabled) in cells {
            let input = self.fields.number(
                format!("s:offsets:{i}"),
                spec,
                Some(offsets[i]),
                move |this, cx| this.slot_json("offsets", cx).and_then(|j| crate::model::geom::json_array4(&j)).map(|a| a[i]),
                move |this, v, cx| {
                    let mut o = this.slot_json("offsets", cx).and_then(|j| crate::model::geom::json_array4(&j)).unwrap_or([0.0; 4]);
                    o[i] = v;
                    this.ctx.clone().set_slot_prop("offsets", json!(o), cx);
                },
                window,
                cx,
            );
            inputs.push(
                div()
                    .w(px(1.0))
                    .flex_1()
                    .min_w_0()
                    .child(NumberInput::new(&input).small().disabled(disabled).prefix(div().text_xs().text_color(theme.muted_foreground).child(label)))
                    .into_any_element(),
            );
        }
        let mut it = inputs.into_iter();
        v_flex()
            .w_full()
            .gap_1()
            .child(h_flex().gap_1().children(it.by_ref().take(2)))
            .child(h_flex().gap_1().children(it))
            .into_any_element()
    }

    // ── Instances ──────────────────────────────────────────────────────────

    fn instance_section(&mut self, node: &Node, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let instance = node.instance.as_ref()?;
        let theme = cx.theme().clone();
        let library = self.ctx.lib.read(cx).library();
        let target = library.widget(&instance.widget);
        let path = library.path_of(&instance.widget).map(|p| p.to_path_buf());
        let ambiguous = library.is_ambiguous(&instance.widget);
        let name = library.name(&instance.widget).unwrap_or_else(|| instance.name_hint.clone());
        let id = node.id;
        let this_widget = self.ctx.widget(cx);
        let this_params: Vec<(ParamId, String, crate::model::ValueClass)> = this_widget.parameters.iter().map(|p| (p.id, p.name.clone(), p.class)).collect();

        let mut rows: Vec<AnyElement> = Vec::new();
        let status = if ambiguous {
            Tag::warning().small().child("Ambiguous id")
        } else if target.is_none() {
            Tag::danger().small().child("Missing")
        } else {
            Tag::success().small().child("Found")
        };

        // Which widget, with Open and Replace.
        let others: Vec<(crate::model::WidgetId, String)> = library.entries().iter().map(|e| (e.id.clone(), e.name.clone())).collect();
        let this_id = this_widget.id.clone();
        let current_library = self.ctx.library_now(cx);
        let others: Vec<(crate::model::WidgetId, String)> =
            others.into_iter().filter(|(w, _)| !crate::model::validate::would_cycle(&current_library, &this_id, w)).collect();
        let ctx = self.ctx.clone();
        let replace = Button::new("instance-replace")
            .label("Replace")
            .small()
            .outline()
            .popup_menu(move |menu, _, _| {
                let mut menu = menu;
                for (target, name) in &others {
                    let (c, t, n) = (ctx.clone(), target.clone(), name.clone());
                    menu = menu.menu_handler(name.clone(), move |_, cx| c.retarget_instance(id, &t, &n, cx));
                }
                menu
            });
        let open = Button::new("instance-open").label("Open").small().outline().disabled(path.is_none()).on_click(move |_, window, cx| {
            if let Some(path) = &path {
                window.dispatch_action(Box::new(OpenAsset { path: path.clone() }), cx);
            }
        });
        rows.push(
            v_flex()
                .w_full()
                .gap_1p5()
                .child(h_flex().gap_2().items_center().child(Icon::new(IconName::Component).size(px(15.0)).text_color(theme.info)).child(div().flex_1().min_w_0().text_sm().text_color(theme.foreground).truncate().child(if name.is_empty() { "(unnamed)".to_string() } else { name })).child(status))
                .child(h_flex().gap_1().child(open).child(replace))
                .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Refers to id {}", instance.widget.short())))
                .into_any_element(),
        );

        // Its parameters.
        if let Some(target) = &target {
            for param in &target.parameters {
                let pid = param.id;
                let arg = instance.overrides.get(&pid).cloned();
                let ty = type_for_class(param.class);
                let bound = matches!(arg, Some(Arg::Bind(_)));
                let current = match &arg {
                    Some(Arg::Value(v)) => v.clone(),
                    _ => param.default.clone(),
                };
                let control = if let Some(Arg::Bind(outer)) = &arg {
                    let outer_name = this_params.iter().find(|(i, _, _)| i == outer).map_or("(missing parameter)".to_string(), |(_, n, _)| n.clone());
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .bg(theme.info.opacity(0.12))
                        .child(Icon::new(IconName::Link).size(px(12.0)).text_color(theme.info))
                        .child(div().text_xs().text_color(theme.foreground).truncate().child(outer_name))
                        .into_any_element()
                } else {
                    let get = move |this: &Self, cx: &App| {
                        let node = this.ctx.widget(cx).node(this.ctx.primary(cx)?)?;
                        match node.instance.as_ref()?.overrides.get(&pid) {
                            Some(Arg::Value(v)) => Some(v.clone()),
                            _ => None,
                        }
                    };
                    let set = move |this: &mut Self, value: Json, cx: &mut Context<Self>| {
                        if let Some(id) = this.ctx.primary(cx) {
                            this.ctx.clone().set_override(id, pid, Some(Arg::Value(value)), cx);
                        }
                    };
                    value_editor(&mut self.fields, &format!("o:{}", pid.0), ty, &current, false, get, set, window, cx)
                };

                let ctx = self.ctx.clone();
                let compatible: Vec<(ParamId, String)> = this_params.iter().filter(|(_, _, c)| *c == param.class).map(|(i, n, _)| (*i, n.clone())).collect();
                let overridden = arg.is_some();
                let menu = Button::new(SharedString::from(format!("override-menu-{}", pid.0)))
                    .icon(IconName::EllipsisVertical)
                    .ghost()
                    .xsmall()
                    .popup_menu(move |menu, window, cx| {
                        let mut menu = menu;
                        let c = ctx.clone();
                        menu = menu.menu_handler_with_icon_and_disabled("Use the default", IconName::Undo, !overridden, move |_, cx| c.set_override(id, pid, None, cx));
                        if bound {
                            let c = ctx.clone();
                            menu = menu.menu_handler("Stop passing a parameter", move |_, cx| c.set_override(id, pid, None, cx));
                        } else if !compatible.is_empty() {
                            let (c, list) = (ctx.clone(), compatible.clone());
                            menu = menu.submenu("Pass one of this widget's parameters", window, cx, move |mut m, _, _| {
                                for (outer, name) in &list {
                                    let (c, outer) = (c.clone(), *outer);
                                    m = m.menu_handler(name.clone(), move |_, cx| c.set_override(id, pid, Some(Arg::Bind(outer)), cx));
                                }
                                m
                            });
                        }
                        menu
                    })
                    .into_any_element();
                rows.push(self.labelled(param.name.clone(), &param.description, control, Some(menu), cx));
            }
            if target.parameters.is_empty() {
                rows.push(div().text_xs().text_color(theme.muted_foreground).child("This widget has no parameters.").into_any_element());
            }
            let slots = target.slot_names();
            if !slots.is_empty() {
                rows.push(div().text_xs().text_color(theme.muted_foreground).child(format!("Slots you can fill: {}. Drop widgets onto it, then choose the slot in their details.", slots.join(", "))).into_any_element());
            }
        }
        Some(self.section("instance", "Widget", rows, cx))
    }

    /// For a child of an instance: which of the instance's slots it fills.
    fn slot_assignment(&mut self, node: &Node, cx: &mut Context<Self>) -> Option<AnyElement> {
        let parent = self.ctx.widget(cx).parent_of(node.id).and_then(|p| self.ctx.widget(cx).node(p))?;
        let instance = parent.instance.as_ref()?;
        let target = self.ctx.lib.read(cx).library().widget(&instance.widget)?;
        let slots = target.slot_names();
        let ctx = self.ctx.clone();
        let id = node.id;
        let current = node.for_slot.clone().unwrap_or_else(|| "(none)".to_string());
        let control = Button::new("fills-slot")
            .label(current)
            .small()
            .outline()
            .w_full()
            .popup_menu(move |menu, _, _| {
                let mut menu = menu;
                for slot in &slots {
                    let (c, slot) = (ctx.clone(), slot.clone());
                    menu = menu.menu_handler(slot.clone(), move |_, cx| {
                        let slot = slot.clone();
                        c.doc.update(cx, |d, cx| d.edit("Fill slot", None, cx, |w| {
                            if let Some(n) = w.node_mut(id) {
                                n.for_slot = Some(slot);
                            }
                        }));
                    });
                }
                menu
            })
            .into_any_element();
        let row = self.labelled("Fills slot", "Which of the parent widget's named slots this goes in", control, None, cx);
        Some(self.section("fills", "Slot", vec![row], cx))
    }

    // ── With nothing selected: the widget itself ───────────────────────────

    fn widget_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let widget = self.ctx.widget(cx).clone();
        let library = self.ctx.lib.read(cx).library();
        let ctx = self.ctx.clone();

        let description = self.fields.text(
            "widget-description".into(),
            true,
            "What is this widget for?",
            &widget.description,
            |this, cx| Some(this.ctx.widget(cx).description.clone()),
            |this, text, cx| {
                this.ctx.doc.update(cx, |d, cx| d.edit("Edit description", Some("description"), cx, |w| w.description = text));
            },
            window,
            cx,
        );
        let spec = NumberSpec { step: 10.0, min: 1.0, max: 16384.0, decimals: 0 };
        let size_input = |this: &mut Self, axis: usize, window: &mut Window, cx: &mut Context<Self>| {
            let value = if axis == 0 { widget.design_size.x } else { widget.design_size.y };
            this.fields.number(
                format!("design-size:{axis}"),
                spec,
                Some(value),
                move |this, cx| {
                    let s = this.ctx.widget(cx).design_size;
                    Some(if axis == 0 { s.x } else { s.y })
                },
                move |this, v, cx| {
                    this.ctx.doc.update(cx, |d, cx| {
                        d.edit("Change design size", Some("design-size"), cx, |w| {
                            if axis == 0 {
                                w.design_size.x = v;
                            } else {
                                w.design_size.y = v;
                            }
                        })
                    });
                },
                window,
                cx,
            )
        };
        let width = size_input(self, 0, window, cx);
        let height = size_input(self, 1, window, cx);

        let id_text = widget.id.to_string();
        let copy_ctx = id_text.clone();
        let identity = vec![
            self.labelled(
                "Id",
                "The stable id other widgets refer to this one by. It never changes when the widget is renamed or moved.",
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .child(div().flex_1().min_w_0().text_xs().text_color(theme.foreground).truncate().child(id_text))
                    .child(Button::new("copy-widget-id").icon(IconName::Copy).ghost().xsmall().tooltip("Copy the id").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_ctx.clone()))))
                    .into_any_element(),
                None,
                cx,
            ),
            self.labelled("Description", "", div().flex_1().min_w_0().child(TextInput::new(&description).small()).into_any_element(), None, cx),
            self.labelled(
                "Design size",
                "The screen size this widget is laid out for",
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().w(px(1.0)).flex_1().child(NumberInput::new(&width).small().prefix(div().text_xs().text_color(theme.muted_foreground).child("W"))))
                    .child(div().w(px(1.0)).flex_1().child(NumberInput::new(&height).small().prefix(div().text_xs().text_color(theme.muted_foreground).child("H"))))
                    .into_any_element(),
                None,
                cx,
            ),
        ];

        let used_by: Vec<_> = library.used_by(&widget.id).into_iter().map(|e| (e.name.clone(), e.path.clone())).collect();
        let uses: Vec<(String, Option<std::path::PathBuf>, bool)> = widget
            .dependencies()
            .iter()
            .map(|dep| match library.entry(dep) {
                Some(e) => (e.name.clone(), Some(e.path.clone()), true),
                None => {
                    let hint = widget_hint(&widget, dep).unwrap_or_else(|| dep.short().to_string());
                    (hint, None, false)
                }
            })
            .collect();

        let link_row = |label: String, path: Option<std::path::PathBuf>, ok: bool, n: usize| {
            let theme = theme.clone();
            h_flex()
                .id(SharedString::from(format!("ref-{n}-{label}")))
                .w_full()
                .gap_2()
                .items_center()
                .px_1()
                .py_0p5()
                .rounded(px(4.0))
                .when(path.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(theme.list_hover)))
                .child(Icon::new(if ok { IconName::Component } else { IconName::TriangleAlert }).size(px(13.0)).text_color(if ok { theme.info } else { theme.danger }))
                .child(div().flex_1().text_sm().text_color(theme.foreground).truncate().child(label))
                .on_click(move |_, window, cx| {
                    if let Some(path) = &path {
                        window.dispatch_action(Box::new(OpenAsset { path: path.clone() }), cx);
                    }
                })
                .into_any_element()
        };
        let used_rows: Vec<AnyElement> = if used_by.is_empty() {
            vec![div().text_xs().text_color(theme.muted_foreground).child("No other widget uses this one yet.").into_any_element()]
        } else {
            used_by.into_iter().enumerate().map(|(i, (name, path))| link_row(name, Some(path), true, i)).collect()
        };
        let uses_rows: Vec<AnyElement> = if uses.is_empty() {
            vec![div().text_xs().text_color(theme.muted_foreground).child("This widget doesn't use other widgets.").into_any_element()]
        } else {
            uses.into_iter().enumerate().map(|(i, (name, path, ok))| link_row(name, path, ok, 100 + i)).collect()
        };

        let slots = widget.slot_names();
        let stats = vec![
            stat_row("Widgets", widget.root.count().to_string(), cx),
            stat_row("Nesting depth", widget.root.depth().to_string(), cx),
            stat_row("Parameters", widget.parameters.len().to_string(), cx),
            stat_row("Named slots", if slots.is_empty() { "none".to_string() } else { slots.join(", ") }, cx),
        ];

        let _ = ctx;
        vec![
            self.section("widget", "Widget", identity, cx),
            self.section("used-by", "Used by", used_rows, cx),
            self.section("uses", "Uses", uses_rows, cx),
            self.section("stats", "Statistics", stats, cx),
        ]
    }
}

fn widget_hint(widget: &crate::model::WidgetAsset, id: &crate::model::WidgetId) -> Option<String> {
    let mut hint = None;
    widget.root.walk(&mut |n| {
        if let Some(i) = &n.instance {
            if &i.widget == id && !i.name_hint.is_empty() && hint.is_none() {
                hint = Some(format!("{} (missing)", i.name_hint));
            }
        }
    });
    hint
}

fn stat_row(label: &str, value: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .gap_2()
        .child(div().w(px(96.0)).text_xs().text_color(theme.muted_foreground).child(label.to_string()))
        .child(div().flex_1().text_xs().text_color(theme.foreground).child(value))
        .into_any_element()
}

/// A tiny picture of an anchor preset: the panel, with the pinned part filled.
fn anchor_glyph(preset: AnchorPreset, selected: bool, accent: Hsla, muted: Hsla) -> impl IntoElement {
    let colour = if selected { accent } else { muted };
    let span = |pin: Pin| -> (f32, f32) {
        // (start, length) across the 26px cell, inside a 4px margin.
        let (lo, len) = (5.0, 16.0);
        match pin {
            Pin::Start => (lo, 5.0),
            Pin::Center => (lo + (len - 5.0) * 0.5, 5.0),
            Pin::End => (lo + len - 5.0, 5.0),
            Pin::Stretch => (lo, len),
        }
    };
    let (x, w) = span(preset.h);
    let (y, h) = span(preset.v);
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded(px(1.5)).bg(colour)
}

/// What an optional property starts at when it is switched on.
fn default_for_optional(ty: &PropType) -> Json {
    match ty {
        PropType::Float { min, .. } => json!(if *min > 0.0 { *min } else { 100.0 }),
        PropType::Color => json!("#ffffff"),
        PropType::Bool => json!(true),
        PropType::Int { .. } => json!(0),
        _ => json!(0),
    }
}

impl Render for DetailsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let primary = self.ctx.primary(cx);
        let node = primary.and_then(|id| self.ctx.widget(cx).node(id).cloned());
        let parent_slot = primary.map(|id| self.ctx.widget(cx).slot_kind_of(id)).unwrap_or(SlotKind::None);

        let mut body: Vec<AnyElement> = Vec::new();
        let mut header = None;
        match &node {
            Some(node) => {
                header = Some(self.identity(node, window, cx));
                body.extend(self.instance_section(node, window, cx));
                body.extend(self.slot_assignment(node, cx));
                body.extend(self.slot_section(node, parent_slot, window, cx));
                body.extend(self.property_sections(node, window, cx));
                if node.info().children == Children::None && body.is_empty() {
                    body.push(div().p_3().text_xs().text_color(theme.muted_foreground).child("Nothing to edit.").into_any_element());
                }
            }
            None => body = self.widget_settings(window, cx),
        }

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Settings, "Details", None, cx))
            .children(header)
            .children(node.is_some().then(|| div().px_2().py_1p5().border_b_1().border_color(theme.border.opacity(0.6)).child(TextInput::new(&self.filter).small().cleanable())))
            .child(if body.is_empty() {
                div().flex_1().child(empty_message(IconName::Search, "No matching properties", "Try another search", cx)).into_any_element()
            } else {
                v_flex().id("details-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).children(body).into_any_element()
            })
    }
}
