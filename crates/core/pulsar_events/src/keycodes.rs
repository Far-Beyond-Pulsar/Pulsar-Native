//! Physical key codes, and the names that go with them.
//!
//! The `key` of [`KeyDown`](crate::builtin::KeyDown) and
//! [`KeyUp`](crate::builtin::KeyUp) is one number space no matter who sent
//! it: winit's `KeyCode` discriminant, which is what a standalone game window
//! forwards. The editor's Play-in-Editor viewport maps GPUI key names into the
//! same space with [`code_for_gpui_name`], so a script or an input mapping
//! sees the same key either way.
//!
//! The table is data, not a dependency on winit: `pulsar_game` (which does
//! link winit) has a test that every row still equals the enum's value.

/// One physical key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCodeInfo {
    /// winit's `KeyCode` discriminant.
    pub code: i64,
    /// winit's variant name (`KeyW`, `ArrowUp`): stable, used by files.
    pub name: &'static str,
    /// What a person calls it.
    pub label: &'static str,
    /// GPUI's `Keystroke::key` for it; empty when GPUI has no name for it
    /// or reports it without a side.
    pub gpui: &'static str,
}

const fn k(code: i64, name: &'static str, label: &'static str, gpui: &'static str) -> KeyCodeInfo {
    KeyCodeInfo {
        code,
        name,
        label,
        gpui,
    }
}

/// Every key the engine names, in a stable order.
pub const KEYS: &[KeyCodeInfo] = &[
    k(19, "KeyA", "A", "a"),
    k(20, "KeyB", "B", "b"),
    k(21, "KeyC", "C", "c"),
    k(22, "KeyD", "D", "d"),
    k(23, "KeyE", "E", "e"),
    k(24, "KeyF", "F", "f"),
    k(25, "KeyG", "G", "g"),
    k(26, "KeyH", "H", "h"),
    k(27, "KeyI", "I", "i"),
    k(28, "KeyJ", "J", "j"),
    k(29, "KeyK", "K", "k"),
    k(30, "KeyL", "L", "l"),
    k(31, "KeyM", "M", "m"),
    k(32, "KeyN", "N", "n"),
    k(33, "KeyO", "O", "o"),
    k(34, "KeyP", "P", "p"),
    k(35, "KeyQ", "Q", "q"),
    k(36, "KeyR", "R", "r"),
    k(37, "KeyS", "S", "s"),
    k(38, "KeyT", "T", "t"),
    k(39, "KeyU", "U", "u"),
    k(40, "KeyV", "V", "v"),
    k(41, "KeyW", "W", "w"),
    k(42, "KeyX", "X", "x"),
    k(43, "KeyY", "Y", "y"),
    k(44, "KeyZ", "Z", "z"),
    k(5, "Digit0", "0", "0"),
    k(6, "Digit1", "1", "1"),
    k(7, "Digit2", "2", "2"),
    k(8, "Digit3", "3", "3"),
    k(9, "Digit4", "4", "4"),
    k(10, "Digit5", "5", "5"),
    k(11, "Digit6", "6", "6"),
    k(12, "Digit7", "7", "7"),
    k(13, "Digit8", "8", "8"),
    k(14, "Digit9", "9", "9"),
    k(159, "F1", "F1", "f1"),
    k(160, "F2", "F2", "f2"),
    k(161, "F3", "F3", "f3"),
    k(162, "F4", "F4", "f4"),
    k(163, "F5", "F5", "f5"),
    k(164, "F6", "F6", "f6"),
    k(165, "F7", "F7", "f7"),
    k(166, "F8", "F8", "f8"),
    k(167, "F9", "F9", "f9"),
    k(168, "F10", "F10", "f10"),
    k(169, "F11", "F11", "f11"),
    k(170, "F12", "F12", "f12"),
    k(82, "ArrowUp", "Up Arrow", "up"),
    k(79, "ArrowDown", "Down Arrow", "down"),
    k(80, "ArrowLeft", "Left Arrow", "left"),
    k(81, "ArrowRight", "Right Arrow", "right"),
    k(60, "ShiftLeft", "Left Shift", "shift"),
    k(61, "ShiftRight", "Right Shift", ""),
    k(55, "ControlLeft", "Left Ctrl", "control"),
    k(56, "ControlRight", "Right Ctrl", ""),
    k(50, "AltLeft", "Left Alt", "alt"),
    k(51, "AltRight", "Right Alt", ""),
    k(58, "SuperLeft", "Left Super", "platform"),
    k(59, "SuperRight", "Right Super", ""),
    k(62, "Space", "Space", "space"),
    k(57, "Enter", "Enter", "enter"),
    k(114, "Escape", "Escape", "escape"),
    k(63, "Tab", "Tab", "tab"),
    k(52, "Backspace", "Backspace", "backspace"),
    k(72, "Delete", "Delete", "delete"),
    k(76, "Insert", "Insert", "insert"),
    k(75, "Home", "Home", "home"),
    k(73, "End", "End", "end"),
    k(78, "PageUp", "Page Up", "pageup"),
    k(77, "PageDown", "Page Down", "pagedown"),
    k(53, "CapsLock", "Caps Lock", "capslock"),
    k(0, "Backquote", "`", "`"),
    k(45, "Minus", "-", "-"),
    k(15, "Equal", "=", "="),
    k(2, "BracketLeft", "[", "["),
    k(3, "BracketRight", "]", "]"),
    k(1, "Backslash", "\\", "\\"),
    k(48, "Semicolon", ";", ";"),
    k(47, "Quote", "'", "'"),
    k(4, "Comma", ",", ","),
    k(46, "Period", ".", "."),
    k(49, "Slash", "/", "/"),
    k(54, "ContextMenu", "Menu", ""),
    k(83, "NumLock", "Num Lock", ""),
    k(117, "PrintScreen", "Print Screen", ""),
    k(118, "ScrollLock", "Scroll Lock", ""),
    k(119, "Pause", "Pause", ""),
    k(84, "Numpad0", "Numpad 0", ""),
    k(85, "Numpad1", "Numpad 1", ""),
    k(86, "Numpad2", "Numpad 2", ""),
    k(87, "Numpad3", "Numpad 3", ""),
    k(88, "Numpad4", "Numpad 4", ""),
    k(89, "Numpad5", "Numpad 5", ""),
    k(90, "Numpad6", "Numpad 6", ""),
    k(91, "Numpad7", "Numpad 7", ""),
    k(92, "Numpad8", "Numpad 8", ""),
    k(93, "Numpad9", "Numpad 9", ""),
    k(94, "NumpadAdd", "Numpad +", ""),
    k(113, "NumpadSubtract", "Numpad -", ""),
    k(109, "NumpadMultiply", "Numpad *", ""),
    k(100, "NumpadDivide", "Numpad /", ""),
    k(99, "NumpadDecimal", "Numpad .", ""),
    k(101, "NumpadEnter", "Numpad Enter", ""),
];

/// The key with this winit code.
pub fn info_for_code(code: i64) -> Option<&'static KeyCodeInfo> {
    KEYS.iter().find(|k| k.code == code)
}

/// The key with this winit variant name.
pub fn info_for_name(name: &str) -> Option<&'static KeyCodeInfo> {
    KEYS.iter().find(|k| k.name == name)
}

/// The code for the key GPUI calls `name` (`"w"`, `"space"`, `"up"`).
/// Matched ignoring case; modifier keys, which GPUI reports without a side,
/// map to the left-hand key.
pub fn code_for_gpui_name(name: &str) -> Option<i64> {
    let lower = name.to_ascii_lowercase();
    KEYS.iter()
        .find(|k| !k.gpui.is_empty() && k.gpui == lower)
        .map(|k| k.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_and_names_are_unique() {
        let codes: HashSet<_> = KEYS.iter().map(|k| k.code).collect();
        let names: HashSet<_> = KEYS.iter().map(|k| k.name).collect();
        assert_eq!(codes.len(), KEYS.len());
        assert_eq!(names.len(), KEYS.len());
    }

    #[test]
    fn gpui_names_map_to_the_physical_key() {
        assert_eq!(code_for_gpui_name("w"), Some(41));
        assert_eq!(code_for_gpui_name("W"), Some(41));
        assert_eq!(code_for_gpui_name("space"), Some(62));
        assert_eq!(code_for_gpui_name("up"), Some(82));
        assert_eq!(code_for_gpui_name("nonsense"), None);
        assert_eq!(info_for_code(41).map(|k| k.name), Some("KeyW"));
    }
}
