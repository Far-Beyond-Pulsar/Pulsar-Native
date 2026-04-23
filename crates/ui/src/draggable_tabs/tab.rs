use gpui::ParentElement;
use gpui::Styled;
use gpui::{
    div, px, AnyElement, AnyView, App, ClickEvent, Context, ElementId, IntoElement, Pixels, Point,
    Render, SharedString, Window,
};
use std::rc::Rc;

use crate::{h_flex, v_flex, ActiveTheme, Icon, Sizable, Size, StyledExt};

/// Data carried during tab drag operations
#[derive(Clone, Debug)]
pub struct DraggedTab {
    pub tab_id: ElementId,
    pub content: AnyView,
    pub label: SharedString,
    pub tab_bar_id: ElementId,
    pub source_index: usize,
    pub drag_start_position: Option<Point<Pixels>>,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .child(self.label.clone())
    }
}

/// A single draggable tab with Chrome-like behavior
/// Note: This is a data structure, not meant to be rendered directly
/// Rendering is handled by DraggableTabBar
pub struct DraggableTab {
    pub id: ElementId,
    pub label: SharedString,
    pub icon: Option<Icon>,
    pub prefix: Option<AnyElement>,
    pub suffix: Option<AnyElement>,
    pub content: AnyView,
    pub size: Size,
    pub selected: bool,
    pub closable: bool,
    pub on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    pub on_close: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl DraggableTab {
    /// Create a new draggable tab
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, content: AnyView) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            prefix: None,
            suffix: None,
            content,
            size: Size::default(),
            selected: false,
            closable: true,
            on_click: None,
            on_close: None,
        }
    }

    /// Set the icon for the tab
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set a prefix element (shown before label)
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// Set a suffix element (shown after label, before close button)
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Set whether this tab is selected
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Set whether this tab can be closed
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// Set on_click handler
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Set on_close handler
    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    pub fn content(&self) -> &AnyView {
        &self.content
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

impl Sizable for DraggableTab {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}
