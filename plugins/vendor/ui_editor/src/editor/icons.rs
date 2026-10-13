//! Icons and drag payloads for the widget kinds.

use gpui::*;
use ui::{h_flex, ActiveTheme, Icon, IconName};

use crate::model::{Kind, NodeId, WidgetId};

pub fn kind_icon(kind: Kind) -> IconName {
    match kind {
        Kind::Canvas => IconName::Frame,
        Kind::VerticalBox => IconName::TableRows,
        Kind::HorizontalBox => IconName::ViewColumns2,
        Kind::Overlay => IconName::Combine,
        Kind::Grid => IconName::ViewGrid,
        Kind::WrapBox => IconName::ListSelect,
        Kind::ScrollBox => IconName::TextArrowsUpDown,
        Kind::SizeBox => IconName::Ruler,
        Kind::ScaleBox => IconName::ScaleFrameEnlarge,
        Kind::Border => IconName::FrameAltEmpty,
        Kind::Text => IconName::Text,
        Kind::Image => IconName::Image,
        Kind::Button => IconName::CursorPointer,
        Kind::CheckBox => IconName::CheckSquare,
        Kind::Slider => IconName::ControlSlider,
        Kind::ProgressBar => IconName::Activity,
        Kind::TextInput => IconName::InputField,
        Kind::ComboBox => IconName::ChevronsUpDown,
        Kind::Spacer => IconName::ResizeCorner,
        Kind::NamedSlot => IconName::PlusSquareDashed,
        Kind::Instance => IconName::Component,
        Kind::Unknown => IconName::HelpCircle,
    }
}

/// The palette group of a kind, in the order the palette lists them.
pub const PALETTE_GROUPS: [&str; 5] = ["Common", "Input", "Display", "Layout", "Special"];

/// A kind of widget dragged out of the palette.
#[derive(Clone, Copy, Debug)]
pub struct PaletteDrag {
    pub kind: Kind,
}

/// A widget from the project dragged out of the library.
#[derive(Clone, Debug)]
pub struct LibraryDrag {
    pub id: WidgetId,
    pub name: String,
}

/// Nodes of the widget being edited, dragged by the hierarchy or the canvas.
#[derive(Clone, Debug)]
pub struct NodeDrag {
    pub ids: Vec<NodeId>,
    pub label: String,
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
