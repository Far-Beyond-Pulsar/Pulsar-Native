//! The keys an input mapping can bind: every control of every device.
//!
//! A *key* here is anything that produces a value: a keyboard key, a mouse
//! button or axis, a gamepad button, trigger or stick. Files store a key by
//! its [`KeyDef::id`] (`Keyboard.KeyW`, `Mouse.Left`, `Gamepad.LeftStick`),
//! which never changes, so the table can grow and be reordered freely.
//!
//! Every key delivers a value of one [`KeyKind`]: a button is 0 or 1, an
//! axis is a signed number, and a stick or mouse motion is two axes together.
//! Which raw input drives which key is [`Source`]; the runtime uses it to
//! route events, the editor to group and label.

use std::collections::HashMap;
use std::sync::LazyLock;

use pulsar_events::keycodes;

/// The kind of device a key belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Device {
    Keyboard,
    Mouse,
    Gamepad,
}

impl Device {
    pub const ALL: [Device; 3] = [Device::Keyboard, Device::Mouse, Device::Gamepad];

    pub fn label(self) -> &'static str {
        match self {
            Device::Keyboard => "Keyboard",
            Device::Mouse => "Mouse",
            Device::Gamepad => "Gamepad",
        }
    }
}

/// What a key reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyKind {
    /// Down (1) or up (0).
    Button,
    /// One signed value.
    Axis1D,
    /// Two signed values, X and Y.
    Axis2D,
}

impl KeyKind {
    pub fn label(self) -> &'static str {
        match self {
            KeyKind::Button => "Button",
            KeyKind::Axis1D => "1D axis",
            KeyKind::Axis2D => "2D axis",
        }
    }

    /// How many components carry a value.
    pub fn width(self) -> usize {
        match self {
            KeyKind::Button | KeyKind::Axis1D => 1,
            KeyKind::Axis2D => 2,
        }
    }
}

/// A gamepad control, whatever the pad calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pad {
    FaceBottom,
    FaceRight,
    FaceLeft,
    FaceTop,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
}

impl Pad {
    /// Every control.
    pub const ALL: [Pad; 21] = [
        Pad::FaceBottom,
        Pad::FaceRight,
        Pad::FaceLeft,
        Pad::FaceTop,
        Pad::LeftShoulder,
        Pad::RightShoulder,
        Pad::LeftTrigger,
        Pad::RightTrigger,
        Pad::Select,
        Pad::Start,
        Pad::Mode,
        Pad::LeftThumb,
        Pad::RightThumb,
        Pad::DpadUp,
        Pad::DpadDown,
        Pad::DpadLeft,
        Pad::DpadRight,
        Pad::LeftStickX,
        Pad::LeftStickY,
        Pad::RightStickX,
        Pad::RightStickY,
    ];
}

/// Mouse motion axes, per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseAxis {
    X,
    Y,
    WheelX,
    WheelY,
}

/// What produces a key's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A physical keyboard key (`pulsar_events::keycodes`).
    Keyboard(i64),
    /// A mouse button, numbered as `MouseButtonDown` does.
    MouseButton(u8),
    /// Mouse motion or wheel, as the change since the last frame.
    MouseAxis(MouseAxis),
    /// A gamepad control, read straight from the pad.
    Pad(Pad),
    /// Worked out from other keys (a stick as a pair, a stick pushed in a
    /// direction, the D-pad as a pair, the wheel as buttons).
    Derived,
}

/// One bindable key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyDef {
    /// The stable id files store: `Device.Name`.
    pub id: String,
    pub label: String,
    pub device: Device,
    /// The heading the picker lists it under.
    pub group: &'static str,
    pub kind: KeyKind,
    pub source: Source,
}

/// Every key, in the order a picker lists them.
pub struct Catalog {
    keys: Vec<KeyDef>,
    by_id: HashMap<String, usize>,
    by_code: HashMap<i64, usize>,
}

impl Catalog {
    fn build() -> Self {
        let mut keys = Vec::new();
        let mut add = |id: &str, label: &str, device, group, kind, source| {
            keys.push(KeyDef { id: id.to_string(), label: label.to_string(), device, group, kind, source });
        };

        for k in keyboard_keys() {
            add(&format!("Keyboard.{}", k.name), k.label, Device::Keyboard, k.group, KeyKind::Button, Source::Keyboard(k.code));
        }

        use KeyKind::*;
        let mouse = Device::Mouse;
        add("Mouse.Left", "Left Button", mouse, "Buttons", Button, Source::MouseButton(0));
        add("Mouse.Right", "Right Button", mouse, "Buttons", Button, Source::MouseButton(1));
        add("Mouse.Middle", "Middle Button", mouse, "Buttons", Button, Source::MouseButton(2));
        add("Mouse.Back", "Back Button", mouse, "Buttons", Button, Source::MouseButton(3));
        add("Mouse.Forward", "Forward Button", mouse, "Buttons", Button, Source::MouseButton(4));
        add("Mouse.XY", "Mouse Movement (X, Y)", mouse, "Motion", Axis2D, Source::Derived);
        add("Mouse.X", "Mouse Movement X", mouse, "Motion", Axis1D, Source::MouseAxis(MouseAxis::X));
        add("Mouse.Y", "Mouse Movement Y", mouse, "Motion", Axis1D, Source::MouseAxis(MouseAxis::Y));
        add("Mouse.Wheel", "Wheel", mouse, "Wheel", Axis1D, Source::MouseAxis(MouseAxis::WheelY));
        add("Mouse.WheelX", "Wheel (Sideways)", mouse, "Wheel", Axis1D, Source::MouseAxis(MouseAxis::WheelX));
        add("Mouse.WheelUp", "Wheel Up", mouse, "Wheel", Button, Source::Derived);
        add("Mouse.WheelDown", "Wheel Down", mouse, "Wheel", Button, Source::Derived);

        let pad = Device::Gamepad;
        add("Gamepad.FaceBottom", "Face Bottom (A / Cross)", pad, "Face buttons", Button, Source::Pad(Pad::FaceBottom));
        add("Gamepad.FaceRight", "Face Right (B / Circle)", pad, "Face buttons", Button, Source::Pad(Pad::FaceRight));
        add("Gamepad.FaceLeft", "Face Left (X / Square)", pad, "Face buttons", Button, Source::Pad(Pad::FaceLeft));
        add("Gamepad.FaceTop", "Face Top (Y / Triangle)", pad, "Face buttons", Button, Source::Pad(Pad::FaceTop));
        add("Gamepad.LeftShoulder", "Left Shoulder (LB / L1)", pad, "Shoulders", Button, Source::Pad(Pad::LeftShoulder));
        add("Gamepad.RightShoulder", "Right Shoulder (RB / R1)", pad, "Shoulders", Button, Source::Pad(Pad::RightShoulder));
        add("Gamepad.LeftTrigger", "Left Trigger (LT / L2)", pad, "Shoulders", Axis1D, Source::Pad(Pad::LeftTrigger));
        add("Gamepad.RightTrigger", "Right Trigger (RT / R2)", pad, "Shoulders", Axis1D, Source::Pad(Pad::RightTrigger));
        add("Gamepad.Select", "Select (Back / Share)", pad, "Menu", Button, Source::Pad(Pad::Select));
        add("Gamepad.Start", "Start (Menu / Options)", pad, "Menu", Button, Source::Pad(Pad::Start));
        add("Gamepad.Mode", "Guide (Home / PS)", pad, "Menu", Button, Source::Pad(Pad::Mode));
        add("Gamepad.DpadUp", "D-pad Up", pad, "D-pad", Button, Source::Pad(Pad::DpadUp));
        add("Gamepad.DpadDown", "D-pad Down", pad, "D-pad", Button, Source::Pad(Pad::DpadDown));
        add("Gamepad.DpadLeft", "D-pad Left", pad, "D-pad", Button, Source::Pad(Pad::DpadLeft));
        add("Gamepad.DpadRight", "D-pad Right", pad, "D-pad", Button, Source::Pad(Pad::DpadRight));
        add("Gamepad.Dpad", "D-pad (X, Y)", pad, "D-pad", Axis2D, Source::Derived);
        add("Gamepad.LeftStick", "Left Stick (X, Y)", pad, "Left stick", Axis2D, Source::Derived);
        add("Gamepad.LeftStickX", "Left Stick X", pad, "Left stick", Axis1D, Source::Pad(Pad::LeftStickX));
        add("Gamepad.LeftStickY", "Left Stick Y", pad, "Left stick", Axis1D, Source::Pad(Pad::LeftStickY));
        add("Gamepad.LeftStickUp", "Left Stick Up", pad, "Left stick", Button, Source::Derived);
        add("Gamepad.LeftStickDown", "Left Stick Down", pad, "Left stick", Button, Source::Derived);
        add("Gamepad.LeftStickLeft", "Left Stick Left", pad, "Left stick", Button, Source::Derived);
        add("Gamepad.LeftStickRight", "Left Stick Right", pad, "Left stick", Button, Source::Derived);
        add("Gamepad.LeftThumb", "Left Stick Click (L3)", pad, "Left stick", Button, Source::Pad(Pad::LeftThumb));
        add("Gamepad.RightStick", "Right Stick (X, Y)", pad, "Right stick", Axis2D, Source::Derived);
        add("Gamepad.RightStickX", "Right Stick X", pad, "Right stick", Axis1D, Source::Pad(Pad::RightStickX));
        add("Gamepad.RightStickY", "Right Stick Y", pad, "Right stick", Axis1D, Source::Pad(Pad::RightStickY));
        add("Gamepad.RightStickUp", "Right Stick Up", pad, "Right stick", Button, Source::Derived);
        add("Gamepad.RightStickDown", "Right Stick Down", pad, "Right stick", Button, Source::Derived);
        add("Gamepad.RightStickLeft", "Right Stick Left", pad, "Right stick", Button, Source::Derived);
        add("Gamepad.RightStickRight", "Right Stick Right", pad, "Right stick", Button, Source::Derived);
        add("Gamepad.RightThumb", "Right Stick Click (R3)", pad, "Right stick", Button, Source::Pad(Pad::RightThumb));

        let by_id = keys.iter().enumerate().map(|(i, k)| (k.id.clone(), i)).collect();
        let by_code = keys
            .iter()
            .enumerate()
            .filter_map(|(i, k)| match k.source {
                Source::Keyboard(code) => Some((code, i)),
                _ => None,
            })
            .collect();
        Self { keys, by_id, by_code }
    }

    pub fn all(&self) -> &[KeyDef] {
        &self.keys
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    pub fn get(&self, id: &str) -> Option<&KeyDef> {
        self.index_of(id).map(|i| &self.keys[i])
    }

    pub fn at(&self, index: usize) -> &KeyDef {
        &self.keys[index]
    }

    /// The key for a winit key code, as `KeyDown` reports it.
    pub fn index_for_code(&self, code: i64) -> Option<usize> {
        self.by_code.get(&code).copied()
    }

    /// Keys whose label, group or id contain every word of `query`.
    pub fn search<'a>(&'a self, query: &str, device: Option<Device>) -> Vec<&'a KeyDef> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.keys
            .iter()
            .filter(|k| device.is_none_or(|d| k.device == d))
            .filter(|k| {
                let hay = format!("{} {} {} {}", k.label, k.group, k.id, k.device.label()).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }
}

struct Board {
    code: i64,
    name: &'static str,
    label: &'static str,
    group: &'static str,
}

fn keyboard_keys() -> Vec<Board> {
    keycodes::KEYS
        .iter()
        .map(|k| {
            let group = match k.name {
                n if n.starts_with("Key") => "Letters",
                n if n.starts_with("Digit") => "Numbers",
                n if n.starts_with("Numpad") => "Numpad",
                n if n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit()) => "Function",
                "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => "Arrows",
                "ShiftLeft" | "ShiftRight" | "ControlLeft" | "ControlRight" | "AltLeft" | "AltRight" | "SuperLeft" | "SuperRight" => "Modifiers",
                "Space" | "Enter" | "Escape" | "Tab" | "Backspace" | "Delete" | "Insert" | "Home" | "End" | "PageUp" | "PageDown" | "CapsLock" => "Editing",
                _ => "Symbols",
            };
            Board { code: k.code, name: k.name, label: k.label, group }
        })
        .collect()
}

/// The key a gamepad control drives when something is bound to it by pressing
/// it: sticks bind as the stick (both axes), everything else as itself.
pub fn id_for_pad(pad: Pad) -> Option<String> {
    let id = match pad {
        Pad::LeftStickX | Pad::LeftStickY => "Gamepad.LeftStick".to_string(),
        Pad::RightStickX | Pad::RightStickY => "Gamepad.RightStick".to_string(),
        other => catalog().all().iter().find(|k| k.source == Source::Pad(other))?.id.clone(),
    };
    Some(id)
}

/// The key for a GPUI key name (`"w"`, `"space"`).
pub fn id_for_gpui_key(name: &str) -> Option<String> {
    let code = keycodes::code_for_gpui_name(name)?;
    catalog().index_for_code(code).map(|i| catalog().at(i).id.clone())
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::build);

/// The key table.
pub fn catalog() -> &'static Catalog {
    &CATALOG
}

/// The kind of the key `id`, if there is one.
pub fn kind_of(id: &str) -> Option<KeyKind> {
    catalog().get(id).map(|k| k.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_prefixed_by_their_device() {
        let c = catalog();
        let ids: HashSet<_> = c.all().iter().map(|k| k.id.as_str()).collect();
        assert_eq!(ids.len(), c.all().len());
        for k in c.all() {
            assert!(k.id.starts_with(k.device.label()), "{}", k.id);
        }
    }

    #[test]
    fn keyboard_keys_come_from_the_engine_key_table() {
        let c = catalog();
        let w = c.get("Keyboard.KeyW").unwrap();
        assert_eq!(w.kind, KeyKind::Button);
        assert_eq!(c.index_for_code(41), c.index_of("Keyboard.KeyW"));
        assert_eq!(c.keys.iter().filter(|k| k.device == Device::Keyboard).count(), keycodes::KEYS.len());
    }

    #[test]
    fn every_device_has_buttons_and_the_sticks_are_two_dimensional() {
        let c = catalog();
        for d in Device::ALL {
            assert!(c.all().iter().any(|k| k.device == d && k.kind == KeyKind::Button));
        }
        assert_eq!(kind_of("Gamepad.LeftStick"), Some(KeyKind::Axis2D));
        assert_eq!(kind_of("Mouse.XY"), Some(KeyKind::Axis2D));
        assert_eq!(kind_of("Gamepad.RightTrigger"), Some(KeyKind::Axis1D));
        assert_eq!(kind_of("Nope.None"), None);
    }

    #[test]
    fn search_matches_every_word_in_any_order() {
        let c = catalog();
        let hits = c.search("stick left", None);
        assert!(hits.iter().any(|k| k.id == "Gamepad.LeftStick"));
        assert!(hits.iter().all(|k| k.device == Device::Gamepad));
        assert!(c.search("w", Some(Device::Mouse)).iter().all(|k| k.device == Device::Mouse));
        assert!(c.search("zzzz", None).is_empty());
    }
}
