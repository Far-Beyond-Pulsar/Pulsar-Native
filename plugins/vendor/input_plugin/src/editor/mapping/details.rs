//! Details of what is selected: a binding's key, action, triggers and
//! modifiers; a category's name; or, with nothing selected, the mapping itself.

use std::rc::Rc;

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::input::{NumberInput, TextInput};
use ui::popover::Popover;
use ui::scroll::ScrollbarAxis;
use ui::switch::Switch;
use ui::{h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt};

use crate::model::behavior::{Modifier, Tolerant, Trigger};
use crate::model::Entry;

use super::super::action_choices;
use super::super::behavior_ui::{behavior_list, ListModel};
use super::super::chooser::{ChoiceItem, Chooser};
use super::super::common::chrome::{empty_message, panel_header};
use super::super::common::panel::dock_panel;
use super::super::fields::{Fields, NumberSpec};
use super::super::form::{chip, note, section, setting};
use super::super::keypicker::KeyPicker;
use super::super::open_asset;
use super::bindings::device_icon;
use super::MapCtx;

pub struct DetailsPanel {
    ctx: MapCtx,
    fields: Fields<Self>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(DetailsPanel, "input-mapping-details", "Details");

impl DetailsPanel {
    pub fn new(ctx: MapCtx, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.sel, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
        ];
        Self { ctx, fields: Fields::default(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn selected_entry(&self, cx: &App) -> Option<Entry> {
        let id = self.ctx.selected_entry(cx)?;
        self.ctx.doc.read(cx).asset().entry(id).cloned()
    }

    fn binding(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library(cx);
        let key = crate::keys::catalog().get(&entry.key);
        let info = entry.action_id().and_then(|a| crate::model::validate::ActionSource::action(library.as_ref(), a));
        let choices = action_choices(&library, None);

        // Key.
        let key_picker = self.fields.key_picker(
            "details-key".into(),
            &entry.key,
            |this, picked, cx| {
                if let Some(id) = this.ctx.selected_entry(cx) {
                    this.ctx.set_key(id, &picked, cx);
                }
            },
            window,
            cx,
        );
        let key_button = Button::new("details-key-button")
            .icon(key.map_or(IconName::Key, |k| device_icon(k.device)))
            .label(key.map_or_else(|| if entry.key.is_empty() { "Choose a key".to_string() } else { entry.key.clone() }, |k| k.label.clone()))
            .small()
            .outline()
            .w_full()
            .dropdown_caret(true);
        let key_control = Popover::<KeyPicker>::new("details-key-popover").anchor(Corner::BottomLeft).trigger(key_button).content(move |_, _| key_picker.clone());

        // Action.
        let selected = entry.action_id().map(|a| a.to_string()).unwrap_or_default();
        let pinned = vec![ChoiceItem::new("", "No action").detail("Leave this key unassigned"), ChoiceItem::new("+new", "New action…").detail("Make an action file beside this mapping")];
        let action_chooser = self.fields.chooser(
            "details-action".into(),
            choices,
            pinned,
            &selected,
            |this, picked, cx| {
                if let Some(id) = this.ctx.selected_entry(cx) {
                    this.ctx.assign_action(id, &picked, cx);
                }
            },
            window,
            cx,
        );
        let action_label = match (&info, entry.action.is_some()) {
            (Some(a), _) => a.name.clone(),
            (None, true) => "Missing action".to_string(),
            _ => "Choose an action".to_string(),
        };
        let action_button = Button::new("details-action-button").icon(IconName::Flash).label(action_label).small().outline().w_full().dropdown_caret(true);
        let action_control = Popover::<Chooser>::new("details-action-popover").anchor(Corner::BottomLeft).trigger(action_button).content(move |_, _| action_chooser.clone());

        let enabled = Switch::new("details-enabled").checked(entry.enabled).on_click(cx.listener(|this, checked: &bool, _, cx| {
            if let Some(id) = this.ctx.selected_entry(cx) {
                this.ctx.set_enabled(id, *checked, cx);
            }
        }));
        let note_field = self.fields.text(
            "details-note".into(),
            true,
            "Why this binding exists",
            &entry.note,
            |this, cx| this.selected_entry(cx).map(|e| e.note),
            |this, text, cx| {
                if let Some(id) = this.ctx.selected_entry(cx) {
                    this.ctx.edit("Change note", Some("note"), cx, |m| {
                        if let Some(e) = m.entry_mut(id) {
                            e.note = text;
                        }
                    });
                }
            },
            window,
            cx,
        );

        let mut binding = section("Binding", IconName::Key, cx)
            .child(setting("Key", "Search the list, or listen for the key.", key_control, cx))
            .child(setting("Action", "What the key triggers.", action_control, cx))
            .child(setting("Enabled", "A disabled binding is kept but does nothing.", enabled, cx))
            .child(setting("Note", "", TextInput::new(&note_field).small(), cx));
        if let Some(path) = entry.action_id().and_then(|a| library.action_entry(a)).map(|a| a.path.clone()) {
            binding = binding.child(
                h_flex().w_full().gap_2().items_center().child(chip(info.as_ref().map_or("", |i| i.value_type.label()), cx)).child(div().flex_1()).child(
                    Button::new("open-action").icon(IconName::ArrowUpRight).label("Open action").small().ghost().on_click(move |_, window, cx| open_asset(path.clone(), window, cx)),
                ),
            );
        }
        if let (Some(key), Some(info)) = (key, &info) {
            if let Some(text) = lossy_note(key.kind, info.value_type, entry.modifiers.is_empty()) {
                binding = binding.child(div().w_full().p_2().rounded(px(6.0)).bg(theme.info.opacity(0.08)).border_1().border_color(theme.info.opacity(0.3)).text_xs().text_color(theme.foreground.opacity(0.9)).child(text));
            }
        }

        // Triggers and modifiers for this binding alone.
        let actions_for_chords = action_choices(&library, None);
        let triggers: ListModel<Self, Trigger> = ListModel {
            get: Rc::new(|this: &Self, cx: &App| this.selected_entry(cx).map(|e| e.triggers).unwrap_or_default()),
            set: Rc::new(|this: &mut Self, list: Vec<Tolerant<Trigger>>, typed: bool, cx: &mut Context<Self>| {
                if let Some(id) = this.ctx.selected_entry(cx) {
                    this.ctx.edit("Change binding triggers", typed.then_some("entry-triggers"), cx, |m| {
                        if let Some(e) = m.entry_mut(id) {
                            e.triggers = list;
                        }
                    });
                }
            }),
        };
        let trigger_list = behavior_list(
            "entry-trigger",
            &mut self.fields,
            &entry.triggers,
            triggers,
            &actions_for_chords,
            "None: the action's own triggers decide. Add one to make just this key behave differently, such as a double tap, or only while another action is held.",
            window,
            cx,
        );
        let modifiers: ListModel<Self, Modifier> = ListModel {
            get: Rc::new(|this: &Self, cx: &App| this.selected_entry(cx).map(|e| e.modifiers).unwrap_or_default()),
            set: Rc::new(|this: &mut Self, list: Vec<Tolerant<Modifier>>, typed: bool, cx: &mut Context<Self>| {
                if let Some(id) = this.ctx.selected_entry(cx) {
                    this.ctx.edit("Change binding modifiers", typed.then_some("entry-modifiers"), cx, |m| {
                        if let Some(e) = m.entry_mut(id) {
                            e.modifiers = list;
                        }
                    });
                }
            }),
        };
        let modifier_list = behavior_list(
            "entry-modifier",
            &mut self.fields,
            &entry.modifiers,
            modifiers,
            &[],
            "None: the key's value goes to the action as it is. Add a swizzle to send a key to Y, or a negate to make it push the other way.",
            window,
            cx,
        );

        v_flex()
            .w_full()
            .p_3()
            .gap_4()
            .child(binding)
            .child(section("Triggers for this key", IconName::Flash, cx).child(trigger_list))
            .child(section("Modifiers for this key", IconName::Settings, cx).child(modifier_list))
            .into_any_element()
    }

    fn category(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let asset = self.ctx.asset(cx);
        let Some(category) = asset.category(id).cloned() else {
            return empty_message(IconName::Key, "Nothing selected", "", cx).into_any_element();
        };
        let name = self.fields.text(
            "category-name".into(),
            false,
            "Category name",
            &category.name,
            |this, cx| {
                let id = this.ctx.selected_category(cx)?;
                this.ctx.doc.read(cx).asset().category(id).map(|c| c.name.clone())
            },
            |this, text, cx| {
                if let Some(id) = this.ctx.selected_category(cx) {
                    this.ctx.rename_category(id, &text, cx);
                }
            },
            window,
            cx,
        );
        let ctx = self.ctx.clone();
        let remove_ctx = self.ctx.clone();
        let only = asset.categories.len() <= 1;
        v_flex()
            .w_full()
            .p_3()
            .gap_4()
            .child(
                section("Category", IconName::Component, cx)
                    .child(setting("Name", "Categories only organize the list; they mean nothing at runtime.", TextInput::new(&name).small(), cx))
                    .child(note(format!("{} bindings", category.entries.len()), cx))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(Button::new("category-add").icon(IconName::Plus).label("Add binding").small().outline().on_click(move |_, _, cx| {
                                ctx.add_binding(id, None, "", None, cx);
                            }))
                            .child(
                                Button::new("category-remove")
                                    .icon(IconName::Trash)
                                    .label(if only { "Clear" } else { "Delete category" })
                                    .small()
                                    .ghost()
                                    .on_click(move |_, _, cx| remove_ctx.remove_category(id, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn mapping(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let asset = self.ctx.asset(cx);
        let priority = self.fields.number(
            "priority".into(),
            NumberSpec { step: 1.0, min: -1000.0, max: 1000.0, decimals: 0 },
            Some(asset.priority as f32),
            |this, cx| Some(this.ctx.doc.read(cx).asset().priority as f32),
            |this, v, cx| this.ctx.edit("Change priority", Some("priority"), cx, |m| m.priority = v.round() as i32),
            window,
            cx,
        );
        let description = self.fields.text(
            "mapping-description".into(),
            true,
            "What this mapping is for",
            &asset.description,
            |this, cx| Some(this.ctx.doc.read(cx).asset().description.clone()),
            |this, text, cx| this.ctx.edit("Change description", Some("description"), cx, |m| m.description = text),
            window,
            cx,
        );
        let actions = asset.used_actions().len();
        v_flex()
            .w_full()
            .p_3()
            .gap_4()
            .child(
                section("Mapping", IconName::Key, cx)
                    .child(setting("Priority", "When mappings overlap, the higher one sees input first.", NumberInput::new(&priority).small(), cx))
                    .child(setting("Description", "", TextInput::new(&description).small(), cx))
                    .child(note(format!("{} bindings of {} actions in {} categories.", asset.entry_count(), actions, asset.categories.len()), cx)),
            )
            .child(note("Select a binding or a category to edit it. A mapping becomes active on an object with a Player Input component, or when a script publishes input::AddMapping.", cx))
            .into_any_element()
    }
}

/// What gets lost when `key` drives an action of `value`, if worth saying.
fn lossy_note(key: crate::keys::KeyKind, value: crate::model::ValueType, plain: bool) -> Option<&'static str> {
    use crate::keys::KeyKind;
    use crate::model::ValueType;
    match (key, value) {
        (KeyKind::Axis2D, ValueType::Axis1D) => Some("A 2D key drives a 1D action: only X is used. Add a Swizzle to use Y instead."),
        (KeyKind::Axis2D, ValueType::Bool) => Some("The action counts as pressed when the stick or mouse moves past its threshold, in any direction."),
        (KeyKind::Button | KeyKind::Axis1D, ValueType::Axis2D | ValueType::Axis3D) if plain => {
            Some("This key only drives X. Use a swizzle to point it at Y, or bind the action with a ready-made set of keys from the actions list.")
        }
        _ => None,
    }
}

impl Render for DetailsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (icon, title, body) = if let Some(entry) = self.selected_entry(cx) {
            (IconName::Key, "Binding", self.binding(entry, window, cx))
        } else if let Some(category) = self.ctx.selected_category(cx).filter(|c| self.ctx.doc.read(cx).asset().category(*c).is_some()) {
            (IconName::Component, "Category", self.category(category, window, cx))
        } else {
            (IconName::Settings, "Mapping", self.mapping(window, cx))
        };
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(icon, title, None, cx))
            .child(v_flex().id("input-details-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(body))
    }
}

