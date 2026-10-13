//! Ready-made sets of bindings for an action, by what the action reports:
//! WASD for a 2D move, the triggers for a 1D throttle, Space and A for a jump.
//!
//! Picking one adds the entries (with the modifiers that make each key point
//! the right way) to a mapping in one go.

use super::behavior::{Modifier, Swizzle, Tolerant, ValueType};
use super::id::ActionRef;
use super::mapping::MappingAsset;

/// One key of a preset and the modifiers it needs.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetKey {
    pub key: &'static str,
    pub modifiers: Vec<Modifier>,
}

/// A set of bindings.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub label: &'static str,
    pub description: &'static str,
    pub keys: Vec<PresetKey>,
}

fn key(key: &'static str) -> PresetKey {
    PresetKey { key, modifiers: Vec::new() }
}

fn with(key: &'static str, modifiers: Vec<Modifier>) -> PresetKey {
    PresetKey { key, modifiers }
}

const X_TO_Y: Modifier = Modifier::Swizzle { order: Swizzle::Yxz };
const X_TO_Z: Modifier = Modifier::Swizzle { order: Swizzle::Zyx };
const NEGATE_X: Modifier = Modifier::Negate { x: true, y: false, z: false };
const NEGATE_Y: Modifier = Modifier::Negate { x: false, y: true, z: false };
const NEGATE_Z: Modifier = Modifier::Negate { x: false, y: false, z: true };

fn stick_dead_zone() -> Vec<Modifier> {
    vec![Modifier::DeadZone { lower: 0.2, upper: 1.0, radial: true }]
}

fn four_keys(up: &'static str, down: &'static str, left: &'static str, right: &'static str) -> Vec<PresetKey> {
    vec![
        with(up, vec![X_TO_Y]),
        with(down, vec![X_TO_Y, NEGATE_Y]),
        with(left, vec![NEGATE_X]),
        key(right),
    ]
}

/// The presets that make sense for an action reporting `value_type`.
pub fn presets_for(value_type: ValueType) -> Vec<Preset> {
    match value_type {
        ValueType::Bool => vec![
            Preset { label: "Jump: Space / Gamepad A", description: "Space and the bottom face button", keys: vec![key("Keyboard.Space"), key("Gamepad.FaceBottom")] },
            Preset { label: "Interact: E / Gamepad X", description: "E and the left face button", keys: vec![key("Keyboard.KeyE"), key("Gamepad.FaceLeft")] },
            Preset { label: "Fire: Left click / Right trigger", description: "The left mouse button and the right trigger", keys: vec![key("Mouse.Left"), key("Gamepad.RightTrigger")] },
            Preset { label: "Aim: Right click / Left trigger", description: "The right mouse button and the left trigger", keys: vec![key("Mouse.Right"), key("Gamepad.LeftTrigger")] },
            Preset { label: "Confirm: Enter / Gamepad A", description: "Enter and the bottom face button", keys: vec![key("Keyboard.Enter"), key("Gamepad.FaceBottom")] },
            Preset { label: "Cancel: Escape / Gamepad B", description: "Escape and the right face button", keys: vec![key("Keyboard.Escape"), key("Gamepad.FaceRight")] },
            Preset { label: "Pause: Escape / Start", description: "Escape and the start button", keys: vec![key("Keyboard.Escape"), key("Gamepad.Start")] },
        ],
        ValueType::Axis1D => vec![
            Preset { label: "Triggers: right +, left −", description: "Both analog triggers", keys: vec![key("Gamepad.RightTrigger"), with("Gamepad.LeftTrigger", vec![NEGATE_X])] },
            Preset { label: "Mouse wheel", description: "Scrolling up is positive", keys: vec![key("Mouse.Wheel")] },
            Preset { label: "E +, Q −", description: "Two keys", keys: vec![key("Keyboard.KeyE"), with("Keyboard.KeyQ", vec![NEGATE_X])] },
            Preset { label: "Up arrow +, Down arrow −", description: "Two keys", keys: vec![key("Keyboard.ArrowUp"), with("Keyboard.ArrowDown", vec![NEGATE_X])] },
            Preset { label: "Right stick up and down", description: "The vertical axis of the right stick", keys: vec![with("Gamepad.RightStickY", stick_dead_zone())] },
        ],
        ValueType::Axis2D => vec![
            Preset { label: "WASD", description: "Four keys; W is up", keys: four_keys("Keyboard.KeyW", "Keyboard.KeyS", "Keyboard.KeyA", "Keyboard.KeyD") },
            Preset { label: "Arrow keys", description: "Four keys", keys: four_keys("Keyboard.ArrowUp", "Keyboard.ArrowDown", "Keyboard.ArrowLeft", "Keyboard.ArrowRight") },
            Preset { label: "Left stick", description: "With a dead zone for drift", keys: vec![with("Gamepad.LeftStick", stick_dead_zone())] },
            Preset { label: "Right stick", description: "With a dead zone for drift", keys: vec![with("Gamepad.RightStick", stick_dead_zone())] },
            Preset { label: "D-pad", description: "The four directional buttons", keys: vec![key("Gamepad.Dpad")] },
            Preset { label: "Mouse movement", description: "How far the mouse moved this frame", keys: vec![key("Mouse.XY")] },
            Preset { label: "WASD + Left stick", description: "Keyboard and controller together", keys: {
                let mut keys = four_keys("Keyboard.KeyW", "Keyboard.KeyS", "Keyboard.KeyA", "Keyboard.KeyD");
                keys.push(with("Gamepad.LeftStick", stick_dead_zone()));
                keys
            } },
            Preset { label: "Mouse + Right stick (look)", description: "Look around with either", keys: vec![key("Mouse.XY"), with("Gamepad.RightStick", stick_dead_zone())] },
        ],
        ValueType::Axis3D => vec![
            Preset { label: "Fly: WASD, Space up, Ctrl down", description: "Move on the ground plane and up and down", keys: {
                let mut keys = four_keys("Keyboard.KeyW", "Keyboard.KeyS", "Keyboard.KeyA", "Keyboard.KeyD");
                keys.push(with("Keyboard.Space", vec![X_TO_Z]));
                keys.push(with("Keyboard.ControlLeft", vec![X_TO_Z, NEGATE_Z]));
                keys
            } },
        ],
    }
}

impl MappingAsset {
    /// Add every key of `preset` for `action` at the end of `category`.
    /// Returns the new entries' ids.
    pub fn add_preset(&mut self, category: u32, action: &ActionRef, preset: &Preset) -> Vec<u32> {
        let mut ids = Vec::new();
        for key in &preset.keys {
            if let Some(id) = self.add_entry(category, key.key, Some(action.clone())) {
                if let Some(entry) = self.entry_mut(id) {
                    entry.modifiers = key.modifiers.iter().cloned().map(Tolerant::Known).collect();
                }
                ids.push(id);
            }
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{catalog, KeyKind};
    use crate::model::AssetId;

    #[test]
    fn every_preset_uses_keys_that_exist() {
        for value_type in ValueType::ALL {
            let presets = presets_for(value_type);
            assert!(!presets.is_empty(), "{value_type:?}");
            for preset in presets {
                assert!(!preset.keys.is_empty(), "{}", preset.label);
                for k in &preset.keys {
                    assert!(catalog().get(k.key).is_some(), "`{}` in {}", k.key, preset.label);
                }
            }
        }
    }

    #[test]
    fn two_dimensional_presets_drive_both_axes() {
        let wasd = presets_for(ValueType::Axis2D).remove(0);
        assert_eq!(wasd.label, "WASD");
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let ids = m.add_preset(cat, &ActionRef::new(AssetId::new(), ""), &wasd);
        assert_eq!(ids.len(), 4);
        // W reaches +Y, S −Y, A −X, D +X, applying the modifiers to a lone key press.
        let reach = |id: u32| {
            let mut v = [1.0f32, 0.0, 0.0];
            for m in &m.entry(id).unwrap().modifiers {
                let m = m.known().unwrap();
                v = apply(m, v);
            }
            v
        };
        assert_eq!(reach(ids[0]), [0.0, 1.0, 0.0]);
        assert_eq!(reach(ids[1]), [0.0, -1.0, 0.0]);
        assert_eq!(reach(ids[2]), [-1.0, 0.0, 0.0]);
        assert_eq!(reach(ids[3]), [1.0, 0.0, 0.0]);
    }

    fn apply(m: &Modifier, v: [f32; 3]) -> [f32; 3] {
        let mut chain = crate::runtime::modify::Chain::new(vec![m.clone()]);
        chain.apply(v, 0.016)
    }

    #[test]
    fn fly_reaches_all_three_axes() {
        let fly = presets_for(ValueType::Axis3D).remove(0);
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let ids = m.add_preset(cat, &ActionRef::new(AssetId::new(), ""), &fly);
        let up = m.entry(ids[4]).unwrap();
        let mut chain = crate::runtime::modify::Chain::new(up.modifiers.iter().filter_map(|m| m.known().cloned()).collect());
        assert_eq!(chain.apply([1.0, 0.0, 0.0], 0.016), [0.0, 0.0, 1.0]);
        let down = m.entry(ids[5]).unwrap();
        let mut chain = crate::runtime::modify::Chain::new(down.modifiers.iter().filter_map(|m| m.known().cloned()).collect());
        assert_eq!(chain.apply([1.0, 0.0, 0.0], 0.016), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn a_preset_key_kind_matches_what_it_is_for() {
        // Stick presets bind a 2D key to a 2D action; the pair presets bind buttons.
        for preset in presets_for(ValueType::Axis2D) {
            for k in preset.keys {
                let kind = catalog().get(k.key).unwrap().kind;
                assert!(matches!(kind, KeyKind::Button | KeyKind::Axis2D), "{}", preset.label);
            }
        }
        for preset in presets_for(ValueType::Bool) {
            assert!(preset.keys.iter().all(|k| catalog().get(k.key).unwrap().kind != KeyKind::Axis2D));
        }
    }
}
