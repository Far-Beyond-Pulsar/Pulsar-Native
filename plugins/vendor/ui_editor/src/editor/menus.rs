//! The context menu for widgets, shared by the canvas and the hierarchy.

use gpui::*;
use plugin_editor_api::OpenAsset;
use ui::menu::PopupMenu;
use ui::IconName;

use crate::model::Kind;

use super::canvas::geometry::{AlignKind, AnchorPreset, Pin};
use super::commands::Ctx;

/// What to wrap a selection in, in menu order.
const WRAPPERS: [Kind; 8] = [
    Kind::Border,
    Kind::Overlay,
    Kind::VerticalBox,
    Kind::HorizontalBox,
    Kind::Canvas,
    Kind::SizeBox,
    Kind::ScaleBox,
    Kind::Button,
];

/// The menu for the current selection.
pub fn node_menu(menu: PopupMenu, ctx: &Ctx, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let selection = ctx.selected(cx);
    let primary = ctx.primary(cx);
    let widget = ctx.widget(cx);
    let canvas_children = selection.iter().filter(|&&id| widget.slot_kind_of(id) == crate::model::SlotKind::Canvas).count();
    let can_unwrap = primary.and_then(|id| widget.node(id)).is_some_and(|n| !n.children.is_empty() && n.id != widget.root.id);
    let can_paste = ctx.can_paste(cx);
    let has_selection = !selection.is_empty();
    let instance_path = primary.and_then(|id| widget.node(id)).and_then(|n| n.instance.as_ref()).and_then(|i| ctx.lib.read(cx).library().path_of(&i.widget).map(|p| p.to_path_buf()));

    let (cut, copy, paste, dup, del) = (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone());
    let mut menu = menu
        .menu_handler_with_icon_and_disabled("Cut", IconName::Cut, !has_selection, move |_, cx| cut.cut_selected(cx))
        .menu_handler_with_icon_and_disabled("Copy", IconName::Copy, !has_selection, move |_, cx| {
            copy.copy_selected(cx);
        })
        .menu_handler_with_icon_and_disabled("Paste", IconName::PasteClipboard, !can_paste, move |_, cx| paste.paste(cx))
        .menu_handler_with_icon_and_disabled("Duplicate", IconName::Copy, !has_selection, move |_, cx| dup.duplicate_selected(cx))
        .menu_handler_with_icon_and_disabled("Delete", IconName::Trash, !has_selection, move |_, cx| del.delete_selected(cx))
        .separator();

    let wrap_ctx = ctx.clone();
    menu = menu.submenu("Wrap with", window, cx, move |mut m, _, _| {
        for kind in WRAPPERS {
            let c = wrap_ctx.clone();
            m = m.menu_handler(kind.info().label, move |_, cx| c.wrap_selected(kind, cx));
        }
        m
    });
    let unwrap = ctx.clone();
    menu = menu.menu_handler_with_icon_and_disabled("Replace with its contents", IconName::Expand, !can_unwrap, move |_, cx| unwrap.unwrap_selected(cx));

    let order = ctx.clone();
    menu = menu.submenu("Arrange", window, cx, move |m, _, _| {
        let (a, b, c, d) = (order.clone(), order.clone(), order.clone(), order.clone());
        m.menu_handler("Bring to front", move |_, cx| a.reorder_selected(1, true, cx))
            .menu_handler("Bring forward", move |_, cx| b.reorder_selected(1, false, cx))
            .menu_handler("Send backward", move |_, cx| c.reorder_selected(-1, false, cx))
            .menu_handler("Send to back", move |_, cx| d.reorder_selected(-1, true, cx))
    });

    if canvas_children > 0 {
        let anchors = ctx.clone();
        menu = menu.separator().submenu("Anchors", window, cx, move |mut m, _, _| {
            for row in 0..4 {
                for col in 0..4 {
                    let preset = AnchorPreset::at(row, col);
                    let c = anchors.clone();
                    m = m.menu_handler(preset.label(), move |_, cx| c.set_anchors(preset, true, cx));
                }
                if row < 3 {
                    m = m.separator();
                }
            }
            m
        });
        let align = ctx.clone();
        menu = menu.submenu("Align", window, cx, move |mut m, _, _| {
            for kind in AlignKind::ALL {
                let c = align.clone();
                m = m.menu_handler(kind.label(), move |_, cx| c.align_selected(kind, cx));
            }
            let (h, v) = (align.clone(), align.clone());
            m.separator()
                .menu_handler("Distribute horizontally", move |_, cx| h.distribute_selected(true, cx))
                .menu_handler("Distribute vertically", move |_, cx| v.distribute_selected(false, cx))
        });
    }

    if let Some(path) = instance_path {
        menu = menu.separator().menu_handler_with_icon("Open widget", IconName::ExternalLink, move |window, cx| {
            window.dispatch_action(Box::new(OpenAsset { path: path.clone() }), cx);
        });
    }
    let _ = Pin::Start;
    menu
}
