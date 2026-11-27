//! Moving around: selecting, following references, history, the browser's
//! keyboard.

use gpui::{ClipboardItem, Context, KeyDownEvent, ScrollStrategy, Window};
use pulsar_scenedb::Entity;

use crate::screen::SceneInspectorView;
use crate::utils::detail::ComponentRef;
use crate::utils::format;
use crate::utils::index::EntityKind;
use crate::utils::outline::{Mode, Node, Row};

impl SceneInspectorView {
    /// Select `entity` by the user's own action, showing `component` if
    /// given, else the same component type as now where it has one.
    pub(crate) fn select(
        &mut self,
        entity: Entity,
        component: Option<ComponentRef>,
        cx: &mut Context<Self>,
    ) {
        let wanted = component.or(self.wanted);
        if self.selected == Some(entity) && self.wanted == wanted {
            return;
        }
        if self.selected != Some(entity) {
            self.record_history();
        }
        self.show(entity, wanted, cx);
    }

    fn show(&mut self, entity: Entity, wanted: Option<ComponentRef>, cx: &mut Context<Self>) {
        self.selected = Some(entity);
        self.wanted = wanted;
        self.refresh.selection_epoch += 1;
        self.request_refresh(false, cx);
        cx.notify();
    }

    /// Show another component of what is selected.
    pub(crate) fn select_component(&mut self, target: ComponentRef, cx: &mut Context<Self>) {
        if self.wanted != Some(target) {
            self.wanted = Some(target);
            self.refresh.selection_epoch += 1;
            self.request_refresh(false, cx);
            cx.notify();
        }
    }

    /// Follow an entity reference: select it and bring its row into view.
    pub(crate) fn follow_link(&mut self, entity: Entity, cx: &mut Context<Self>) {
        let alive = self
            .index
            .as_ref()
            .is_some_and(|index| index.entity(entity).is_some());
        self.select(entity, None, cx);
        if alive {
            self.reveal_selected();
        } else {
            self.notify_user(format!(
                "{} is not in the last scan; it may have been despawned",
                format::entity(entity)
            ));
        }
        cx.notify();
    }

    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) {
        let Some((entity, wanted)) = self.back.pop() else {
            return;
        };
        if let Some(current) = self.selected {
            self.forward.push((current, self.wanted));
        }
        self.show(entity, wanted, cx);
        self.reveal_selected();
    }

    pub(crate) fn go_forward(&mut self, cx: &mut Context<Self>) {
        let Some((entity, wanted)) = self.forward.pop() else {
            return;
        };
        if let Some(current) = self.selected {
            self.back.push((current, self.wanted));
        }
        self.show(entity, wanted, cx);
        self.reveal_selected();
    }

    pub(crate) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild_rows();
            self.reveal_selected();
            cx.notify();
        }
    }

    /// Scroll the browser to the selection, opening the tree to it.
    pub(crate) fn reveal_selected(&mut self) {
        let (Some(selected), Some(index)) = (self.selected, self.index.clone()) else {
            return;
        };
        if self.mode == Mode::Objects {
            // An instance is shown by its owner's row.
            let shown = match index.entity(selected).map(|row| row.kind) {
                Some(EntityKind::Instance { owner, .. }) => owner,
                _ => selected,
            };
            let mut parent = index.entity(shown).and_then(|row| row.parent);
            let mut opened = false;
            // Bounded, so a parent cycle cannot hang the walk.
            for _ in 0..index.entities.len() {
                let Some(row) = parent.and_then(|parent| index.entity(parent)) else {
                    break;
                };
                if !self.expansion.is_open(Node::Object(row.entity)) {
                    self.expansion.set(Node::Object(row.entity), true);
                    opened = true;
                }
                parent = row.parent;
            }
            if opened {
                self.rebuild_rows();
            }
        }
        if let Some(position) = self.row_of_selection() {
            self.browser_scroll
                .handle
                .scroll_to_item(position, ScrollStrategy::Center);
        }
    }

    fn row_of_selection(&self) -> Option<usize> {
        let index = self.index.as_ref()?;
        let selected = self.selected?;
        let target = match (self.mode, index.entity(selected).map(|row| row.kind)) {
            (Mode::Objects, Some(EntityKind::Instance { owner, .. })) => owner,
            _ => selected,
        };
        let row_ix = *index.by_entity.get(&target)?;
        let shows = |row: &Row| matches!(*row, Row::Entity { index, .. } if index == row_ix);
        // In the type view an entity is listed once per type: stay under the
        // type being looked at.
        let under = self.wanted.map(|wanted| wanted.id);
        self.rows
            .iter()
            .position(|row| {
                shows(row) && matches!(*row, Row::Entity { under: listed, .. } if listed == under)
            })
            .or_else(|| self.rows.iter().position(shows))
    }

    /// The browser row at `position` was clicked.
    pub(crate) fn activate_row(&mut self, position: usize, cx: &mut Context<Self>) {
        let (Some(index), Some(&row)) = (self.index.clone(), self.rows.get(position)) else {
            return;
        };
        match row {
            Row::Entity {
                index: ix, under, ..
            } => {
                let entity = index.entities[ix as usize].entity;
                let component = under.map(|id| ComponentRef { entity, id });
                self.select(entity, component, cx);
            }
            _ => self.toggle_row(position, cx),
        }
    }

    pub(crate) fn toggle_row(&mut self, position: usize, cx: &mut Context<Self>) {
        let node = match (&self.index, self.rows.get(position)) {
            (Some(index), Some(row)) => row.node(index),
            _ => None,
        };
        if let Some(node) = node {
            self.expansion.toggle(node);
            self.rebuild_rows();
            cx.notify();
        }
    }

    pub(crate) fn on_browser_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let current = self.row_of_selection();
        let step = |by: isize| match current {
            Some(at) => at.saturating_add_signed(by).min(count - 1),
            None => 0,
        };
        let key = event.keystroke.key.as_str();
        let next = match key {
            "up" => step(-1),
            "down" => step(1),
            "pageup" => step(-20),
            "pagedown" => step(20),
            "home" => 0,
            "end" => count - 1,
            "left" | "right" => {
                let Some(at) = current else {
                    return;
                };
                if let Some(Row::Entity {
                    open: Some(open), ..
                }) = self.rows.get(at)
                {
                    if *open == (key == "left") {
                        self.toggle_row(at, cx);
                    }
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        // Headings are not selectable: go on in the same direction to the
        // nearest entity.
        let backwards = matches!(key, "up" | "pageup" | "end");
        let entity_at = |at: &usize| matches!(self.rows.get(*at), Some(Row::Entity { .. }));
        let found = if backwards {
            (0..=next).rev().find(entity_at)
        } else {
            (next..count).find(entity_at)
        };
        if let Some(at) = found {
            self.activate_row(at, cx);
            self.browser_scroll
                .handle
                .scroll_to_item(at, ScrollStrategy::Center);
        }
    }

    pub(crate) fn copy(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notify_user(format!("Copied {what}"));
        cx.notify();
    }

    pub(crate) fn notify_user(&mut self, text: String) {
        self.notice = Some((text, std::time::Instant::now()));
    }
}
