//! The action's own settings: its name, what it reports, and how input is
//! shared.

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::input::{NumberInput, TextInput};
use ui::scroll::ScrollbarAxis;
use ui::switch::Switch;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt};

use crate::model::action::to_identifier;
use crate::model::behavior::ValueType;
use crate::runtime::events;
use crate::runtime::system::Phase;

use super::super::common::chrome::panel_header;
use super::super::common::panel::dock_panel;
use super::super::fields::{Fields, NumberSpec};
use super::super::form::{chip, note, section, setting};
use super::ActionCtx;

pub struct SettingsPanel {
    ctx: ActionCtx,
    fields: Fields<Self>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(SettingsPanel, "input-action-settings", "Action");

impl SettingsPanel {
    pub fn new(ctx: ActionCtx, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify())];
        Self { ctx, fields: Fields::default(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn value_type_card(&self, value_type: ValueType, current: ValueType, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = value_type == current;
        let button = Button::new(SharedString::from(format!("value-type-{}", value_type.short())))
            .label(value_type.short())
            .small()
            .tooltip(format!("{}\n{}", value_type.label(), value_type.description()))
            .on_click(cx.listener(move |this, _, _, cx| this.ctx.edit("Change value type", None, cx, |a| a.value_type = value_type)));
        div().flex_1().child(if selected { button.primary().w_full() } else { button.outline().w_full() })
    }

    fn events_card(&self, name: &str, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let rows = Phase::ALL.iter().map(|&phase| {
            let qualified = events::qualified(name, phase);
            let copy = qualified.clone();
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().text_sm().font_family("monospace").text_color(theme.foreground).child(qualified))
                .child(
                    Button::new(SharedString::from(format!("copy-event-{}", phase.word())))
                        .icon(IconName::Copy)
                        .ghost()
                        .xsmall()
                        .tooltip("Copy the event name")
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
                )
        });
        v_flex()
            .w_full()
            .p_2()
            .gap_1()
            .rounded(px(6.0))
            .bg(theme.secondary.opacity(0.5))
            .border_1()
            .border_color(theme.border)
            .children(rows.collect::<Vec<_>>())
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let asset = self.ctx.asset(cx);
        let stem = self.ctx.stem(cx);
        let name = asset.event_name(&stem);

        let identifier = self.fields.text(
            "identifier".into(),
            false,
            &to_identifier(&stem),
            &asset.identifier,
            |this, cx| Some(this.ctx.asset(cx).identifier),
            |this, text, cx| this.ctx.edit("Rename script name", Some("identifier"), cx, |a| a.identifier = text.trim().to_string()),
            window,
            cx,
        );
        let description = self.fields.text(
            "description".into(),
            true,
            "What this action is for",
            &asset.description,
            |this, cx| Some(this.ctx.asset(cx).description),
            |this, text, cx| this.ctx.edit("Change description", Some("description"), cx, |a| a.description = text),
            window,
            cx,
        );
        let threshold = self.fields.number(
            "threshold".into(),
            NumberSpec { step: 0.05, min: 0.0, max: 1.0, decimals: 2 },
            Some(asset.actuation_threshold),
            |this, cx| Some(this.ctx.asset(cx).actuation_threshold),
            |this, v, cx| this.ctx.edit("Change threshold", Some("threshold"), cx, |a| a.actuation_threshold = v),
            window,
            cx,
        );

        let identity = section("Identity", IconName::Component, cx)
            .child(setting("Script name", "What scripts call this action. Empty uses the file name.", TextInput::new(&identifier).small(), cx))
            .child(note("Scripts and plugins hear it as these events:", cx))
            .child(self.events_card(&name, cx))
            .child(note(
                "Each carries entity, value, x, y, z, elapsed and triggered_for. On an object's own channel when the mapping is on that object, otherwise global.",
                cx,
            ))
            .child(setting("Description", "", TextInput::new(&description).small(), cx));

        let current = asset.value_type;
        let value = section("Value", IconName::DashboardSpeed, cx)
            .child(h_flex().w_full().gap_1().children(ValueType::ALL.map(|t| self.value_type_card(t, current, cx))))
            .child(note(current.description(), cx))
            .child(h_flex().gap_1().items_center().child(note("Reports", cx)).child(chip(current.label(), cx)));

        let consume = Switch::new("consume-input").checked(asset.consume_input).on_click(cx.listener(|this, checked: &bool, _, cx| {
            let checked = *checked;
            this.ctx.edit("Change consume input", None, cx, |a| a.consume_input = checked)
        }));
        let paused = Switch::new("when-paused").checked(asset.trigger_when_paused).on_click(cx.listener(|this, checked: &bool, _, cx| {
            let checked = *checked;
            this.ctx.edit("Change trigger when paused", None, cx, |a| a.trigger_when_paused = checked)
        }));
        let behavior = section("Behavior", IconName::Settings, cx)
            .child(setting(
                "Consume input",
                "While this action is active, keys it uses are hidden from mappings with a lower priority.",
                consume,
                cx,
            ))
            .child(setting("Trigger when paused", "Keep working while the game is paused, for menus.", paused, cx))
            .child(setting(
                "Actuation threshold",
                "How far from rest a value must be to count as pressed.",
                NumberInput::new(&threshold).small(),
                cx,
            ));

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Flash, "Action", None, cx))
            .child(
                v_flex()
                    .id("input-action-settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .scrollable(ScrollbarAxis::Vertical)
                    .child(v_flex().w_full().max_w(px(760.0)).mx_auto().p_4().gap_5().child(identity).child(value).child(behavior)),
            )
    }
}
