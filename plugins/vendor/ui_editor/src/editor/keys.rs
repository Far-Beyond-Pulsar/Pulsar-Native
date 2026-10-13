//! Keyboard shortcuts for editing, shared by the canvas and the hierarchy.

use gpui::*;

use crate::model::{Kind, Vec2};

use super::commands::Ctx;
use super::frame::Frame;

/// Handle an editing shortcut. Returns whether the key was one.
pub fn handle(ctx: &Ctx, event: &KeyDownEvent, frame: Option<&Frame>, cx: &mut App) -> bool {
    let m = event.keystroke.modifiers;
    let key = event.keystroke.key.as_str();
    let command = m.secondary();

    match (key, command, m.shift) {
        ("delete" | "backspace", false, _) => ctx.delete_selected(cx),
        ("d", true, false) => ctx.duplicate_selected(cx),
        ("c", true, false) => {
            ctx.copy_selected(cx);
        }
        ("x", true, false) => ctx.cut_selected(cx),
        ("v", true, false) => ctx.paste(cx),
        ("a", true, false) => ctx.select_all(cx),
        ("g", true, false) => ctx.wrap_selected(Kind::Overlay, cx),
        ("g", true, true) => ctx.unwrap_selected(cx),
        ("]", false, _) => ctx.reorder_selected(1, command, cx),
        ("[", false, _) => ctx.reorder_selected(-1, command, cx),
        ("]", true, _) => ctx.reorder_selected(1, true, cx),
        ("[", true, _) => ctx.reorder_selected(-1, true, cx),
        ("escape", false, _) => {
            ctx.clear_notice(cx);
            ctx.select(Vec::new(), cx);
        }
        ("left" | "right" | "up" | "down", false, _) => {
            let step = if m.shift { 10.0 } else { 1.0 };
            let delta = match key {
                "left" => Vec2::new(-step, 0.0),
                "right" => Vec2::new(step, 0.0),
                "up" => Vec2::new(0.0, -step),
                _ => Vec2::new(0.0, step),
            };
            ctx.nudge_selected(delta, cx);
        }
        _ => {
            let _ = frame;
            return false;
        }
    }
    true
}
