use gpui::ParentElement;
use gpui::{
    div, px, size, AnyElement, AnyView, App, AppContext, Bounds, Context, DragMoveEvent, ElementId,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement, Window, WindowBounds, WindowKind, WindowOptions,
};

use super::{DraggableTab, DraggedTab};
use crate::{
    button::{Button, ButtonVariants},
    h_flex,
    tab::Tab,
    v_flex, ActiveTheme, IconName, Selectable, Sizable, StyledExt,
};
use gpui::prelude::FluentBuilder;
use gpui::Styled;

/// Events emitted by DraggableTabBar
#[derive(Clone, Debug)]
pub enum TabBarEvent {
    /// Tab was selected
    TabSelected(usize),
    /// Tab was closed
    TabClosed(usize),
    /// Tab was reordered within this bar
    TabReordered { from: usize, to: usize },
    /// Tab was dropped from another window
    TabDropped { tab: DraggedTab, at_index: usize },
}

/// A Chrome-like draggable tab bar
pub struct DraggableTabBar {
    _id: ElementId,
    focus_handle: FocusHandle,
    tabs: Vec<(ElementId, SharedString, AnyView, bool)>, // (id, label, content, closable)
    selected_index: usize,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    /// Track if currently dragging outside window bounds
    dragging_outside: bool,
}

impl DraggableTabBar {
    pub fn new(id: impl Into<ElementId>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            _id: id.into(),
            focus_handle: cx.focus_handle(),
            tabs: Vec::new(),
            selected_index: 0,
            prefix: None,
            suffix: None,
            dragging_outside: false,
        }
    }

    /// Add a tab to the bar
    pub fn add_tab(
        &mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        content: AnyView,
        closable: bool,
    ) {
        self.tabs.push((id.into(), label.into(), content, closable));
    }

    /// Remove a tab by index
    pub fn remove_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.selected_index >= self.tabs.len() && !self.tabs.is_empty() {
                self.selected_index = self.tabs.len() - 1;
            }
        }
    }

    /// Set the selected tab index
    pub fn set_selected(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.selected_index = index;
        }
    }

    /// Get the currently selected tab content
    pub fn selected_content(&self) -> Option<&AnyView> {
        self.tabs
            .get(self.selected_index)
            .map(|(_, _, content, _)| content)
    }

    /// Set prefix element (shown before tabs)
    pub fn set_prefix(&mut self, prefix: impl IntoElement) {
        self.prefix = Some(prefix.into_any_element());
    }

    /// Set suffix element (shown after tabs)
    pub fn set_suffix(&mut self, suffix: impl IntoElement) {
        self.suffix = Some(suffix.into_any_element());
    }

    /// Check if drag position is outside window bounds
    fn check_drag_outside(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let bounds = window.bounds();
        let margin = px(20.0);

        let is_outside = position.x < bounds.left() - margin
            || position.x > bounds.right() + margin
            || position.y < bounds.top() - margin
            || position.y > bounds.bottom() + margin;

        if is_outside != self.dragging_outside {
            self.dragging_outside = is_outside;
            cx.notify();
        }

        is_outside
    }

    fn reorder_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from != to && from < self.tabs.len() && to < self.tabs.len() {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);

            // Update selected index
            if self.selected_index == from {
                self.selected_index = to;
            } else if from < self.selected_index && to >= self.selected_index {
                self.selected_index -= 1;
            } else if from > self.selected_index && to <= self.selected_index {
                self.selected_index += 1;
            }

            cx.emit(TabBarEvent::TabReordered { from, to });
            cx.notify();
        }
    }
}

impl EventEmitter<TabBarEvent> for DraggableTabBar {}
impl Focusable for DraggableTabBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DraggableTabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().clone();
        let view_entity_id = view.entity_id();
        let tab_bar_element_id = ElementId::Name(SharedString::from(format!(
            "tab-bar-{}",
            view_entity_id.as_u64()
        )));

        v_flex()
            .size_full()
            .child(
                // Tab bar container
                h_flex()
                    .h(px(36.))
                    .bg(cx.theme().tab_bar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when_some(self.prefix.take(), |this, prefix| this.child(prefix))
                    .child(
                        // Tabs container
                        h_flex().flex_1().overflow_x_hidden().items_end().children(
                            self.tabs.iter().enumerate().map(
                                |(ix, (tab_id, label, content, closable))| {
                                    let is_selected = ix == self.selected_index;
                                    let tab_id = tab_id.clone();
                                    let label = label.clone();
                                    let content = content.clone();
                                    let closable = *closable;
                                    let view_clone = view.clone();
                                    let tab_bar_id = tab_bar_element_id.clone();

                                    Tab::new(format!("tab-{}", ix))
                                        .child(label.clone())
                                        .selected(is_selected)
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            this.set_selected(ix);
                                            cx.emit(TabBarEvent::TabSelected(ix));
                                            cx.notify();
                                        }))
                                        .on_drag(
                                            DraggedTab {
                                                tab_id: tab_id.clone(),
                                                content: content.clone(),
                                                label: label.clone(),
                                                tab_bar_id: tab_bar_id.clone(),
                                                source_index: ix,
                                                drag_start_position: None,
                                            },
                                            move |drag, position, _, cx| {
                                                let mut drag_data = drag.clone();
                                                drag_data.drag_start_position = Some(position);
                                                cx.stop_propagation();
                                                cx.new(|_| drag_data)
                                            },
                                        )
                                        .on_drag_move(cx.listener(
                                            move |this,
                                                  event: &DragMoveEvent<DraggedTab>,
                                                  window,
                                                  cx| {
                                                this.check_drag_outside(
                                                    event.event.position,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        ))
                                        .drag_over::<DraggedTab>(|this, _, _, cx| {
                                            this.rounded_l_none()
                                                .border_l_2()
                                                .border_r_0()
                                                .border_color(cx.theme().drag_border)
                                        })
                                        .on_drop(cx.listener(
                                            move |this, drag: &DraggedTab, _window, cx| {
                                                if drag.tab_bar_id == tab_bar_id {
                                                    // Reorder within same bar
                                                    this.reorder_tab(drag.source_index, ix, cx);
                                                } else {
                                                    // Drop from another window
                                                    cx.emit(TabBarEvent::TabDropped {
                                                        tab: drag.clone(),
                                                        at_index: ix,
                                                    });
                                                }
                                            },
                                        ))
                                        .when(closable, |tab| {
                                            tab.suffix(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Button::new(SharedString::from(format!(
                                                            "tab-close-{}",
                                                            ix
                                                        )))
                                                        .icon(IconName::Close)
                                                        .ghost()
                                                        .with_size(crate::Size::XSmall)
                                                        .on_click(cx.listener(
                                                            move |this, _event, _window, cx| {
                                                                this.remove_tab(ix);
                                                                cx.emit(TabBarEvent::TabClosed(ix));
                                                                cx.notify();
                                                            },
                                                        )),
                                                    )
                                                    .into_any_element(),
                                            )
                                        })
                                },
                            ),
                        ),
                    )
                    .when_some(self.suffix.take(), |this, suffix| this.child(suffix)),
            )
            .child(
                // Content area
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(self.selected_content().cloned(), |this, content| {
                        this.child(content)
                    }),
            )
    }
}
