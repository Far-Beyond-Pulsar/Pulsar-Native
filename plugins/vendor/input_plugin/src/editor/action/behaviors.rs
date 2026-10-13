//! The action's triggers and modifiers.

use std::rc::Rc;

use gpui::*;
use ui::scroll::ScrollbarAxis;
use ui::{v_flex, ActiveTheme, IconName, StyledExt};

use crate::model::behavior::{Modifier, Tolerant, Trigger};

use super::super::action_choices;
use super::super::behavior_ui::{behavior_list, ListModel};
use super::super::common::chrome::panel_header;
use super::super::common::panel::dock_panel;
use super::super::fields::Fields;
use super::ActionCtx;

pub struct TriggersPanel {
    ctx: ActionCtx,
    fields: Fields<Self>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(TriggersPanel, "input-action-triggers", "Triggers");

impl TriggersPanel {
    pub fn new(ctx: ActionCtx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify()), cx.observe(&ctx.lib, |_, _, cx| cx.notify())];
        Self { ctx, fields: Fields::default(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

impl Render for TriggersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let asset = self.ctx.asset(cx);
        let actions = action_choices(&self.ctx.library_now(cx), Some(&asset.id));
        let model: ListModel<Self, Trigger> = ListModel {
            get: Rc::new(|this: &Self, cx: &App| this.ctx.doc.read(cx).asset().triggers.clone()),
            set: Rc::new(|this: &mut Self, list: Vec<Tolerant<Trigger>>, typed: bool, cx: &mut Context<Self>| {
                this.ctx.edit("Change triggers", typed.then_some("triggers"), cx, |a| a.triggers = list)
            }),
        };
        let list = behavior_list(
            "action-trigger",
            &mut self.fields,
            &asset.triggers,
            model,
            &actions,
            "No triggers: the action fires every frame its input is held. Add one to fire on press, on release, after a hold, and so on.",
            window,
            cx,
        );
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Flash, "Triggers", None, cx))
            .child(v_flex().id("input-action-triggers-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(v_flex().w_full().p_3().child(list)))
    }
}

pub struct ModifiersPanel {
    ctx: ActionCtx,
    fields: Fields<Self>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(ModifiersPanel, "input-action-modifiers", "Modifiers");

impl ModifiersPanel {
    pub fn new(ctx: ActionCtx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify())];
        Self { ctx, fields: Fields::default(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

impl Render for ModifiersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let asset = self.ctx.asset(cx);
        let model: ListModel<Self, Modifier> = ListModel {
            get: Rc::new(|this: &Self, cx: &App| this.ctx.doc.read(cx).asset().modifiers.clone()),
            set: Rc::new(|this: &mut Self, list: Vec<Tolerant<Modifier>>, typed: bool, cx: &mut Context<Self>| {
                this.ctx.edit("Change modifiers", typed.then_some("modifiers"), cx, |a| a.modifiers = list)
            }),
        };
        let list = behavior_list(
            "action-modifier",
            &mut self.fields,
            &asset.modifiers,
            model,
            &[],
            "No modifiers: the value passes through as the keys report it. Add a dead zone, a scale or a curve to reshape it.",
            window,
            cx,
        );
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Settings, "Modifiers", None, cx))
            .child(v_flex().id("input-action-modifiers-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(v_flex().w_full().p_3().child(list)))
    }
}
