//! What is dragged around the mapping editor.

use gpui::*;
use ui::{h_flex, ActiveTheme, Icon, IconName};

/// A binding being moved.
#[derive(Clone)]
pub struct EntryDrag {
    pub id: u32,
    pub label: String,
}

/// An action being dragged out of the actions list.
#[derive(Clone)]
pub struct ActionDrag {
    pub id: String,
}

/// What follows the pointer during a drag.
pub struct DragGhost {
    pub label: SharedString,
    pub icon: IconName,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1p5()
            .items_center()
            .px_2()
            .py_1()
            .rounded(px(6.0))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.info)
            .shadow_md()
            .child(Icon::new(self.icon.clone()).size(px(14.0)).text_color(theme.info))
            .child(div().text_sm().text_color(theme.foreground).child(self.label.clone()))
    }
}

/// Which side of a row a drop lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

impl Side {
    /// The half of `bounds` the pointer is in.
    pub fn at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Self {
        if position.y - bounds.origin.y < bounds.size.height / 2.0 {
            Side::Before
        } else {
            Side::After
        }
    }
}
