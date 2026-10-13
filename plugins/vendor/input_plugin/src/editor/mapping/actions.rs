//! The project's actions, ready to bind.
//!
//! Drag one onto a category, double-click it to bind it in the selected
//! category, or open its menu for ready-made sets of keys (WASD for a 2D move,
//! the triggers for a 1D throttle...).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::input::{InputEvent, InputState, TextInput};
use ui::menu::DropdownMenu;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use crate::library::{ActionEntry, Library};
use crate::model::behavior::ValueType;
use crate::model::presets::presets_for;
use crate::model::{ActionRef, AssetId};

use super::super::common::chrome::{empty_message, panel_header};
use super::super::common::panel::dock_panel;
use super::super::drag::{ActionDrag, DragGhost};
use super::super::form::chip;
use super::super::open_asset;
use super::MapCtx;

const ROW_HEIGHT: f32 = 44.0;

pub struct ActionsPanel {
    ctx: MapCtx,
    search: Entity<InputState>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(ActionsPanel, "input-mapping-actions", "Actions");

impl ActionsPanel {
    pub fn new(ctx: MapCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search actions…"));
        let subscriptions = vec![
            cx.observe(&ctx.doc, |_, _, cx| cx.notify()),
            cx.observe(&ctx.lib, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self { ctx, search, scroll: UniformListScrollHandle::new(), focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    /// The actions that match the search, with how many times this mapping binds each.
    fn visible(&self, library: &Library, cx: &App) -> Vec<(ActionEntry, usize)> {
        let words: Vec<String> = self.search.read(cx).value().split_whitespace().map(str::to_lowercase).collect();
        let used: HashMap<AssetId, usize> = {
            let mut counts = HashMap::new();
            for (_, entry) in self.ctx.doc.read(cx).asset().entries() {
                if let Some(id) = entry.action_id() {
                    *counts.entry(id.clone()).or_insert(0) += 1;
                }
            }
            counts
        };
        library
            .actions()
            .into_iter()
            .filter(|a| {
                let hay = format!("{} {} {}", a.event_name(), a.relative, a.asset.value_type.short()).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .map(|a| (a.clone(), used.get(&a.id).copied().unwrap_or(0)))
            .collect()
    }

    fn bind(&self, action: &ActionEntry, cx: &mut App) {
        let category = self.ctx.target_category(cx);
        self.ctx.add_binding(category, None, "", Some(ActionRef::new(action.id.clone(), action.relative.clone())), cx);
    }

    fn render_rows(&mut self, rows: Arc<Vec<(ActionEntry, usize)>>, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let entity = cx.entity();
        range
            .filter_map(|at| {
                let (action, count) = rows.get(at)?.clone();
                let name = action.event_name();
                let reference = ActionRef::new(action.id.clone(), action.relative.clone());
                let path = action.path.clone();
                let value_type = action.asset.value_type;
                let bound = count > 0;
                let drag = ActionDrag { id: action.id.to_string() };
                let menu_entity = entity.clone();
                let menu_reference = reference.clone();
                let bind_action = action.clone();
                Some(
                    h_flex()
                        .id(("action", at))
                        .group("action-row")
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.accent.opacity(0.12)))
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            if event.click_count() >= 2 {
                                this.bind(&bind_action, cx);
                            }
                        }))
                        .on_drag(drag, {
                            let name = name.clone();
                            move |_, _, _, cx| cx.new(|_| DragGhost { label: name.clone().into(), icon: IconName::Flash })
                        })
                        .child(Icon::new(IconName::Flash).size(px(15.0)).text_color(if bound { theme.success } else { theme.info }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().text_color(theme.foreground).overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                                .child(div().text_xs().text_color(theme.muted_foreground).overflow_hidden().text_ellipsis().whitespace_nowrap().child(action.relative.clone())),
                        )
                        .child(chip(value_type.short(), cx))
                        .children(bound.then(|| chip(format!("{count} bound"), cx)))
                        .child(
                            Button::new(("more", at)).icon(IconName::Ellipsis).ghost().xsmall().tooltip("Bind with a set of keys").popup_menu(move |menu, _, _| {
                                let mut menu = menu.label("Bind with…");
                                for preset in presets_for(value_type) {
                                    let (entity, reference) = (menu_entity.clone(), menu_reference.clone());
                                    menu = menu.menu_handler(preset.label, move |_, cx| {
                                        let (preset, reference) = (preset.clone(), reference.clone());
                                        entity.update(cx, |this, cx| this.ctx.bind_preset(&reference, &preset, cx));
                                    });
                                }
                                let path = path.clone();
                                menu.menu_handler("Open action", move |window, cx| open_asset(path.clone(), window, cx))
                            }),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for ActionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library(cx);
        let rows = Arc::new(self.visible(&library, cx));
        let total = library.actions().len();
        let count = rows.len();
        let ctx = self.ctx.clone();

        let new_action = Button::new("new-action").icon(IconName::Plus).label("New").small().outline().tooltip("Make an action beside this mapping").popup_menu({
            let ctx = ctx.clone();
            move |menu, _, _| {
                let mut menu = menu.label("New action that reports…");
                for value_type in ValueType::ALL {
                    let ctx = ctx.clone();
                    menu = menu.menu_handler(value_type.label(), move |_, cx| {
                        match ctx.create_action("NewAction", value_type, cx) {
                            Ok(reference) => {
                                let category = ctx.target_category(cx);
                                ctx.add_binding(category, None, "", Some(reference), cx);
                            }
                            Err(error) => tracing::warn!("input: couldn't make an action: {error}"),
                        }
                    });
                }
                menu
            }
        });

        let body = if total == 0 {
            empty_message(
                IconName::Flash,
                "No actions in this project",
                "Make one with New above, or create an Input Action from the file browser.",
                cx,
            )
            .into_any_element()
        } else if count == 0 {
            empty_message(IconName::Search, "No action matches", "Try fewer words.", cx).into_any_element()
        } else {
            let list_rows = rows.clone();
            uniform_list(
                "input-actions-list",
                count,
                cx.processor(move |this, range: Range<usize>, _window, cx| this.render_rows(list_rows.clone(), range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(IconName::Flash, "Actions", Some(new_action.into_any_element()), cx))
            .child(h_flex().w_full().flex_shrink_0().p_2().child(div().w_full().child(TextInput::new(&self.search).small().cleanable())))
            .child(div().flex_1().min_h_0().px_1().child(body))
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{count} of {total} actions · double-click to bind, drag onto a category")),
            )
    }
}
