//! The parameters panel: the values of this widget that whoever uses it can
//! set, and which properties each one drives.

use gpui::*;
use serde_json::Value as Json;
use ui::button::{Button, ButtonVariants};
use ui::input::TextInput;
use ui::menu::DropdownMenu;
use ui::scroll::ScrollbarAxis;
use ui::tag::Tag;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt};

use crate::model::{ParamId, ValueClass};

use super::commands::Ctx;
use super::common::chrome::{empty_message, panel_header};
use super::common::panel::dock_panel;
use super::fields::Fields;
use super::value_editor::{type_for_class, value_editor};

const CLASSES: [ValueClass; 8] = [
    ValueClass::Text,
    ValueClass::Float,
    ValueClass::Int,
    ValueClass::Bool,
    ValueClass::Color,
    ValueClass::Vec2,
    ValueClass::Quad,
    ValueClass::Image,
];

pub struct ParametersPanel {
    ctx: Ctx,
    fields: Fields<Self>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(ParametersPanel, "widget-parameters", "Parameters");

impl ParametersPanel {
    pub fn new(ctx: Ctx, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify())];
        let fields = Fields::new(ctx.lib.read(cx).root().to_path_buf());
        Self { ctx, fields, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn edit_param(&self, id: ParamId, cx: &mut App, change: impl FnOnce(&mut crate::model::Parameter)) {
        self.ctx.doc.update(cx, |d, cx| {
            d.edit("Edit parameter", Some("parameter"), cx, |w| {
                if let Some(p) = w.parameters.iter_mut().find(|p| p.id == id) {
                    change(p);
                }
            })
        });
    }

    fn row(&mut self, id: ParamId, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let widget = self.ctx.widget(cx).clone();
        let param = widget.parameter(id)?.clone();
        let targets: Vec<(crate::model::NodeId, String, String, bool)> = param
            .targets
            .iter()
            .map(|t| match widget.node(t.node) {
                Some(n) => (t.node, n.name.clone(), n.info().def(&t.prop).map_or(t.prop.clone(), |d| d.label.to_string()), true),
                None => (t.node, "(deleted)".to_string(), t.prop.clone(), false),
            })
            .collect();
        let class = param.class;

        let name = self.fields.text(
            format!("pn:{}", id.0),
            false,
            "Name",
            &param.name,
            move |this, cx| this.ctx.widget(cx).parameter(id).map(|p| p.name.clone()),
            move |this, text, cx| {
                let clean = crate::model::ops::sanitize_name(&text, "Parameter");
                if !text.trim().is_empty() {
                    this.edit_param(id, cx, |p| p.name = clean);
                }
            },
            window,
            cx,
        );
        let description = self.fields.text(
            format!("pd:{}", id.0),
            false,
            "Description (shown to whoever uses this widget)",
            &param.description,
            move |this, cx| this.ctx.widget(cx).parameter(id).map(|p| p.description.clone()),
            move |this, text, cx| this.edit_param(id, cx, |p| p.description = text),
            window,
            cx,
        );

        let get = move |this: &Self, cx: &App| this.ctx.widget(cx).parameter(id).map(|p| p.default.clone());
        let set = move |this: &mut Self, value: Json, cx: &mut Context<Self>| {
            this.edit_param(id, cx, |p| {
                if let Some(v) = p.class.coerce(&value) {
                    p.default = v;
                }
            })
        };
        let editor = value_editor(&mut self.fields, &format!("pv:{}", id.0), type_for_class(class), &param.default, false, get, set, window, cx);

        let chips: Vec<AnyElement> = targets
            .into_iter()
            .map(|(node, node_name, prop, ok)| {
                let ctx = self.ctx.clone();
                let ctx2 = self.ctx.clone();
                let prop_key = param.targets.iter().find(|t| t.node == node && widget.node(t.node).and_then(|n| n.info().def(&t.prop)).is_some_and(|d| d.label == prop || t.prop == prop)).map(|t| t.prop.clone()).unwrap_or_default();
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .pl_1p5()
                    .rounded(px(5.0))
                    .bg(if ok { theme.info.opacity(0.14) } else { theme.danger.opacity(0.18) })
                    .child(
                        div()
                            .id(SharedString::from(format!("target-{}-{}-{prop_key}", id.0, node.0)))
                            .text_xs()
                            .text_color(theme.foreground)
                            .cursor_pointer()
                            .child(format!("{node_name} \u{b7} {prop}"))
                            .on_click(move |_, _, cx| ctx.select(vec![node], cx)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("untarget-{}-{}-{prop_key}", id.0, node.0)))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Stop driving this property")
                            .on_click(move |_, _, cx| ctx2.unlink_property(node, &prop_key, cx)),
                    )
                    .into_any_element()
            })
            .collect();

        let ctx = self.ctx.clone();
        Some(
            h_flex()
                .w_full()
                .gap_3()
                .items_start()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(theme.border.opacity(0.6))
                .child(
                    v_flex()
                        .w(px(220.0))
                        .flex_shrink_0()
                        .gap_1()
                        .child(h_flex().gap_1p5().items_center().child(div().flex_1().child(TextInput::new(&name).small())).child(Tag::secondary().small().child(class.label())))
                        .child(TextInput::new(&description).xsmall()),
                )
                .child(v_flex().w(px(260.0)).flex_shrink_0().gap_1().child(div().text_xs().text_color(theme.muted_foreground).child("Default value")).child(editor))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_xs().text_color(theme.muted_foreground).child(if chips.is_empty() { "Drives nothing yet: use \u{22ef} next to a property in Details to link it".to_string() } else { "Drives".to_string() }))
                        .child(h_flex().flex_wrap().gap_1().children(chips)),
                )
                .child(Button::new(SharedString::from(format!("delete-param-{}", id.0))).icon(IconName::Trash).ghost().small().tooltip("Delete this parameter").on_click(move |_, _, cx| ctx.remove_parameter(id, cx)))
                .into_any_element(),
        )
    }
}

impl Render for ParametersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ids: Vec<ParamId> = self.ctx.widget(cx).parameters.iter().map(|p| p.id).collect();
        let rows: Vec<AnyElement> = ids.into_iter().filter_map(|id| self.row(id, window, cx)).collect();
        let empty = rows.is_empty();

        let ctx = self.ctx.clone();
        let add = Button::new("add-parameter").icon(IconName::Plus).label("Add").small().popup_menu(move |menu, _, _| {
            let mut menu = menu;
            for class in CLASSES {
                let c = ctx.clone();
                menu = menu.menu_handler(class.label(), move |_, cx| {
                    c.add_parameter("Parameter", class, cx);
                });
            }
            menu
        });

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::ControlSlider, "Parameters", Some(add.into_any_element()), cx))
            .child(if empty {
                div()
                    .flex_1()
                    .child(empty_message(
                        IconName::ControlSlider,
                        "No parameters",
                        "A parameter is a value this widget lets its users set: a title, a colour, an icon. Add one, then link it to a property.",
                        cx,
                    ))
                    .into_any_element()
            } else {
                v_flex().id("parameters-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).children(rows).into_any_element()
            })
    }
}
