//! What an action does with the raw value of its keys: its value type, the
//! modifiers that reshape the value, and the triggers that decide when the
//! action fires.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::id::ActionRef;

/// A value of up to three components. Buttons use `x`.
pub type Value3 = [f32; 3];

/// What an action reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    /// On or off.
    #[default]
    Bool,
    /// One number: a trigger pull, throttle, zoom.
    #[serde(rename = "axis1d")]
    Axis1D,
    /// Two numbers: movement, look.
    #[serde(rename = "axis2d")]
    Axis2D,
    /// Three numbers: flying, a direction in space.
    #[serde(rename = "axis3d")]
    Axis3D,
}

impl ValueType {
    pub const ALL: [ValueType; 4] = [ValueType::Bool, ValueType::Axis1D, ValueType::Axis2D, ValueType::Axis3D];

    pub fn label(self) -> &'static str {
        match self {
            ValueType::Bool => "Digital (on / off)",
            ValueType::Axis1D => "Axis 1D (number)",
            ValueType::Axis2D => "Axis 2D (X, Y)",
            ValueType::Axis3D => "Axis 3D (X, Y, Z)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            ValueType::Bool => "Bool",
            ValueType::Axis1D => "1D",
            ValueType::Axis2D => "2D",
            ValueType::Axis3D => "3D",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ValueType::Bool => "Pressed or not. Jump, fire, interact.",
            ValueType::Axis1D => "A single number. A trigger pull, zoom, throttle.",
            ValueType::Axis2D => "Two numbers. Movement and look.",
            ValueType::Axis3D => "Three numbers. Flying, a direction in space.",
        }
    }

    /// Components that carry a value.
    pub fn width(self) -> usize {
        match self {
            ValueType::Bool | ValueType::Axis1D => 1,
            ValueType::Axis2D => 2,
            ValueType::Axis3D => 3,
        }
    }

    /// `value` cut down to what this type can hold (a Bool is 0 or 1).
    pub fn fit(self, value: Value3, threshold: f32) -> Value3 {
        match self {
            ValueType::Bool => [if magnitude(value) >= threshold && magnitude(value) > 0.0 { 1.0 } else { 0.0 }, 0.0, 0.0],
            ValueType::Axis1D => [value[0], 0.0, 0.0],
            ValueType::Axis2D => [value[0], value[1], 0.0],
            ValueType::Axis3D => value,
        }
    }
}

/// How large `value` is.
pub fn magnitude(value: Value3) -> f32 {
    (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt()
}

// ── Tolerant ────────────────────────────────────────────────────────────────

/// A trigger or modifier read leniently. One written by a newer version, or
/// by hand with a typo, is kept as the JSON it was and written back unchanged,
/// instead of failing the whole file.
#[derive(Clone, Debug, PartialEq)]
pub enum Tolerant<T> {
    Known(T),
    Unknown(Value),
}

impl<T> Tolerant<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Tolerant::Known(t) => Some(t),
            Tolerant::Unknown(_) => None,
        }
    }

    pub fn known_mut(&mut self) -> Option<&mut T> {
        match self {
            Tolerant::Known(t) => Some(t),
            Tolerant::Unknown(_) => None,
        }
    }
}

impl<T: Serialize> Serialize for Tolerant<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Tolerant::Known(t) => t.serialize(serializer),
            Tolerant::Unknown(v) => v.serialize(serializer),
        }
    }
}

impl<'de, T: for<'a> Deserialize<'a>> Deserialize<'de> for Tolerant<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match serde_json::from_value::<T>(value.clone()) {
            Ok(t) => Tolerant::Known(t),
            Err(_) => Tolerant::Unknown(value),
        })
    }
}

// ── Triggers ────────────────────────────────────────────────────────────────

/// When an action fires.
///
/// An action with no triggers fires every frame its value is actuated
/// ([`Trigger::Down`]). With triggers, it fires when any of them does; a
/// [`Trigger::Chorded`] is the exception, which must also be satisfied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// Every frame the value is actuated.
    Down,
    /// Once, the frame the value becomes actuated.
    Pressed,
    /// Once, the frame the value stops being actuated.
    Released,
    /// After the value has been held for `seconds`; with `once`, only the
    /// first frame after that, otherwise every frame until released.
    Hold {
        #[serde(default = "d_hold")]
        seconds: f32,
        #[serde(default)]
        once: bool,
    },
    /// On release, if the value had been held for at least `seconds`.
    HoldAndRelease {
        #[serde(default = "d_hold")]
        seconds: f32,
    },
    /// On release, if the value was held for no more than `max_seconds`.
    Tap {
        #[serde(default = "d_tap")]
        max_seconds: f32,
    },
    /// While held, every `interval` seconds, up to `limit` times (0: no
    /// limit); with `on_start`, also the moment it is pressed.
    Pulse {
        #[serde(default = "d_interval")]
        interval: f32,
        #[serde(default)]
        limit: u32,
        #[serde(default = "d_true")]
        on_start: bool,
    },
    /// On the second press within `window` seconds of the first.
    DoubleTap {
        #[serde(default = "d_window")]
        window: f32,
    },
    /// Only while another action is firing: hold Shift to sprint, Ctrl+S to
    /// save. Every other trigger on this action is held back until it is.
    Chorded {
        #[serde(default)]
        action: ActionRef,
    },
}

fn d_hold() -> f32 {
    0.5
}
fn d_tap() -> f32 {
    0.2
}
fn d_interval() -> f32 {
    0.25
}
fn d_window() -> f32 {
    0.3
}
fn d_true() -> bool {
    true
}

/// A trigger without its settings, for menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    Down,
    Pressed,
    Released,
    Hold,
    HoldAndRelease,
    Tap,
    Pulse,
    DoubleTap,
    Chorded,
}

impl TriggerKind {
    pub const ALL: [TriggerKind; 9] = [
        TriggerKind::Down,
        TriggerKind::Pressed,
        TriggerKind::Released,
        TriggerKind::Hold,
        TriggerKind::HoldAndRelease,
        TriggerKind::Tap,
        TriggerKind::Pulse,
        TriggerKind::DoubleTap,
        TriggerKind::Chorded,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerKind::Down => "Down",
            TriggerKind::Pressed => "Pressed",
            TriggerKind::Released => "Released",
            TriggerKind::Hold => "Hold",
            TriggerKind::HoldAndRelease => "Hold and release",
            TriggerKind::Tap => "Tap",
            TriggerKind::Pulse => "Pulse",
            TriggerKind::DoubleTap => "Double tap",
            TriggerKind::Chorded => "Chorded",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            TriggerKind::Down => "Fires every frame the input is held.",
            TriggerKind::Pressed => "Fires once when the input goes down.",
            TriggerKind::Released => "Fires once when the input comes up.",
            TriggerKind::Hold => "Fires once the input has been held long enough.",
            TriggerKind::HoldAndRelease => "Fires on release, if it was held long enough.",
            TriggerKind::Tap => "Fires on release, if it was a quick press.",
            TriggerKind::Pulse => "Fires repeatedly at an interval while held.",
            TriggerKind::DoubleTap => "Fires on a quick second press.",
            TriggerKind::Chorded => "Only fires while another action is firing.",
        }
    }

    pub fn default_trigger(self) -> Trigger {
        match self {
            TriggerKind::Down => Trigger::Down,
            TriggerKind::Pressed => Trigger::Pressed,
            TriggerKind::Released => Trigger::Released,
            TriggerKind::Hold => Trigger::Hold { seconds: d_hold(), once: false },
            TriggerKind::HoldAndRelease => Trigger::HoldAndRelease { seconds: d_hold() },
            TriggerKind::Tap => Trigger::Tap { max_seconds: d_tap() },
            TriggerKind::Pulse => Trigger::Pulse { interval: d_interval(), limit: 0, on_start: true },
            TriggerKind::DoubleTap => Trigger::DoubleTap { window: d_window() },
            TriggerKind::Chorded => Trigger::Chorded { action: ActionRef::default() },
        }
    }
}

impl Trigger {
    pub fn kind(&self) -> TriggerKind {
        match self {
            Trigger::Down => TriggerKind::Down,
            Trigger::Pressed => TriggerKind::Pressed,
            Trigger::Released => TriggerKind::Released,
            Trigger::Hold { .. } => TriggerKind::Hold,
            Trigger::HoldAndRelease { .. } => TriggerKind::HoldAndRelease,
            Trigger::Tap { .. } => TriggerKind::Tap,
            Trigger::Pulse { .. } => TriggerKind::Pulse,
            Trigger::DoubleTap { .. } => TriggerKind::DoubleTap,
            Trigger::Chorded { .. } => TriggerKind::Chorded,
        }
    }

    /// A one-line summary with the settings in it.
    pub fn summary(&self) -> String {
        match self {
            Trigger::Hold { seconds, once } => format!("Hold {seconds:.2}s{}", if *once { ", once" } else { "" }),
            Trigger::HoldAndRelease { seconds } => format!("Hold {seconds:.2}s, then release"),
            Trigger::Tap { max_seconds } => format!("Tap within {max_seconds:.2}s"),
            Trigger::Pulse { interval, limit, .. } => {
                if *limit == 0 {
                    format!("Pulse every {interval:.2}s")
                } else {
                    format!("Pulse every {interval:.2}s, {limit} times")
                }
            }
            Trigger::DoubleTap { window } => format!("Double tap within {window:.2}s"),
            other => other.kind().label().to_string(),
        }
    }
}

// ── Modifiers ───────────────────────────────────────────────────────────────

/// The order components are taken in by a swizzle: the output's X, Y, Z come
/// from these inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Swizzle {
    #[default]
    Yxz,
    Xzy,
    Zyx,
    Yzx,
    Zxy,
}

impl Swizzle {
    pub const ALL: [Swizzle; 5] = [Swizzle::Yxz, Swizzle::Xzy, Swizzle::Zyx, Swizzle::Yzx, Swizzle::Zxy];

    pub fn label(self) -> &'static str {
        match self {
            Swizzle::Yxz => "YXZ (swap X and Y)",
            Swizzle::Xzy => "XZY (swap Y and Z)",
            Swizzle::Zyx => "ZYX (swap X and Z)",
            Swizzle::Yzx => "YZX",
            Swizzle::Zxy => "ZXY",
        }
    }

    /// Which input component feeds each output component.
    pub fn order(self) -> [usize; 3] {
        match self {
            Swizzle::Yxz => [1, 0, 2],
            Swizzle::Xzy => [0, 2, 1],
            Swizzle::Zyx => [2, 1, 0],
            Swizzle::Yzx => [1, 2, 0],
            Swizzle::Zxy => [2, 0, 1],
        }
    }
}

/// The shape of a response curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    /// `|v|^exponent`: gentle near the middle, full at the edge.
    #[default]
    Exponential,
    /// `v^2`
    Quadratic,
    /// `v^3`
    Cubic,
    /// Eases in and out.
    SmoothStep,
}

impl Curve {
    pub const ALL: [Curve; 4] = [Curve::Exponential, Curve::Quadratic, Curve::Cubic, Curve::SmoothStep];

    pub fn label(self) -> &'static str {
        match self {
            Curve::Exponential => "Exponential",
            Curve::Quadratic => "Quadratic",
            Curve::Cubic => "Cubic",
            Curve::SmoothStep => "Smooth step",
        }
    }
}

/// How the value is reshaped before triggers see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Modifier {
    /// Ignores small values (stick drift) and rescales what is left to 0..1.
    DeadZone {
        #[serde(default = "d_lower")]
        lower: f32,
        #[serde(default = "d_one")]
        upper: f32,
        /// Judge a stick by its distance from the middle rather than each
        /// axis on its own.
        #[serde(default = "d_true")]
        radial: bool,
    },
    /// Multiplies each component.
    Scale {
        #[serde(default = "d_one")]
        x: f32,
        #[serde(default = "d_one")]
        y: f32,
        #[serde(default = "d_one")]
        z: f32,
    },
    /// Flips the sign of the chosen components.
    Negate {
        #[serde(default = "d_true")]
        x: bool,
        #[serde(default)]
        y: bool,
        #[serde(default)]
        z: bool,
    },
    /// Reorders components. Turns a key's X into an action's Y.
    Swizzle {
        #[serde(default)]
        order: Swizzle,
    },
    /// Eases the value toward its target; higher `speed` follows faster.
    Smooth {
        #[serde(default = "d_speed")]
        speed: f32,
    },
    /// Reshapes the magnitude.
    ResponseCurve {
        #[serde(default)]
        curve: Curve,
        #[serde(default = "d_exponent")]
        exponent: f32,
    },
    /// Limits each component to a range.
    Clamp {
        #[serde(default = "d_minus_one")]
        min: f32,
        #[serde(default = "d_one")]
        max: f32,
    },
}

fn d_lower() -> f32 {
    0.2
}
fn d_one() -> f32 {
    1.0
}
fn d_minus_one() -> f32 {
    -1.0
}
fn d_speed() -> f32 {
    10.0
}
fn d_exponent() -> f32 {
    2.0
}

/// A modifier without its settings, for menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    DeadZone,
    Scale,
    Negate,
    Swizzle,
    Smooth,
    ResponseCurve,
    Clamp,
}

impl ModifierKind {
    pub const ALL: [ModifierKind; 7] = [
        ModifierKind::DeadZone,
        ModifierKind::Scale,
        ModifierKind::Negate,
        ModifierKind::Swizzle,
        ModifierKind::Smooth,
        ModifierKind::ResponseCurve,
        ModifierKind::Clamp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ModifierKind::DeadZone => "Dead zone",
            ModifierKind::Scale => "Scale",
            ModifierKind::Negate => "Negate",
            ModifierKind::Swizzle => "Swizzle",
            ModifierKind::Smooth => "Smooth",
            ModifierKind::ResponseCurve => "Response curve",
            ModifierKind::Clamp => "Clamp",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ModifierKind::DeadZone => "Ignore small values such as stick drift.",
            ModifierKind::Scale => "Multiply each component, e.g. for sensitivity.",
            ModifierKind::Negate => "Flip the sign of chosen components.",
            ModifierKind::Swizzle => "Reorder components, e.g. send a key's X to Y.",
            ModifierKind::Smooth => "Ease toward the target instead of jumping.",
            ModifierKind::ResponseCurve => "Make small movements finer.",
            ModifierKind::Clamp => "Keep each component within a range.",
        }
    }

    pub fn default_modifier(self) -> Modifier {
        match self {
            ModifierKind::DeadZone => Modifier::DeadZone { lower: d_lower(), upper: 1.0, radial: true },
            ModifierKind::Scale => Modifier::Scale { x: 1.0, y: 1.0, z: 1.0 },
            ModifierKind::Negate => Modifier::Negate { x: true, y: false, z: false },
            ModifierKind::Swizzle => Modifier::Swizzle { order: Swizzle::Yxz },
            ModifierKind::Smooth => Modifier::Smooth { speed: d_speed() },
            ModifierKind::ResponseCurve => Modifier::ResponseCurve { curve: Curve::Exponential, exponent: d_exponent() },
            ModifierKind::Clamp => Modifier::Clamp { min: -1.0, max: 1.0 },
        }
    }
}

impl Modifier {
    pub fn kind(&self) -> ModifierKind {
        match self {
            Modifier::DeadZone { .. } => ModifierKind::DeadZone,
            Modifier::Scale { .. } => ModifierKind::Scale,
            Modifier::Negate { .. } => ModifierKind::Negate,
            Modifier::Swizzle { .. } => ModifierKind::Swizzle,
            Modifier::Smooth { .. } => ModifierKind::Smooth,
            Modifier::ResponseCurve { .. } => ModifierKind::ResponseCurve,
            Modifier::Clamp { .. } => ModifierKind::Clamp,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Modifier::DeadZone { lower, upper, .. } => format!("Dead zone {lower:.2} to {upper:.2}"),
            Modifier::Scale { x, y, z } => format!("Scale {x:.2}, {y:.2}, {z:.2}"),
            Modifier::Negate { x, y, z } => {
                let axes: String = [(*x, 'X'), (*y, 'Y'), (*z, 'Z')].iter().filter(|(on, _)| *on).map(|(_, c)| *c).collect();
                if axes.is_empty() {
                    "Negate nothing".to_string()
                } else {
                    format!("Negate {axes}")
                }
            }
            Modifier::Swizzle { order } => format!("Swizzle {}", order.label().split(' ').next().unwrap_or("")),
            Modifier::Smooth { speed } => format!("Smooth, speed {speed:.1}"),
            Modifier::ResponseCurve { curve, exponent } => match curve {
                Curve::Exponential => format!("Curve, exponent {exponent:.2}"),
                other => format!("Curve, {}", other.label().to_lowercase()),
            },
            Modifier::Clamp { min, max } => format!("Clamp {min:.2} to {max:.2}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_and_modifiers_round_trip_with_defaults_filled_in() {
        let t: Trigger = serde_json::from_str(r#"{"type":"hold"}"#).unwrap();
        assert_eq!(t, Trigger::Hold { seconds: 0.5, once: false });
        let m: Modifier = serde_json::from_str(r#"{"type":"dead_zone"}"#).unwrap();
        assert_eq!(m, Modifier::DeadZone { lower: 0.2, upper: 1.0, radial: true });
        for kind in TriggerKind::ALL {
            let t = kind.default_trigger();
            assert_eq!(t.kind(), kind);
            assert_eq!(serde_json::from_value::<Trigger>(serde_json::to_value(&t).unwrap()).unwrap(), t);
        }
        for kind in ModifierKind::ALL {
            let m = kind.default_modifier();
            assert_eq!(m.kind(), kind);
            assert_eq!(serde_json::from_value::<Modifier>(serde_json::to_value(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn an_unrecognised_trigger_is_kept_not_rejected() {
        let list: Vec<Tolerant<Trigger>> = serde_json::from_str(r#"[{"type":"pressed"},{"type":"from_the_future","x":3}]"#).unwrap();
        assert_eq!(list[0], Tolerant::Known(Trigger::Pressed));
        assert!(matches!(list[1], Tolerant::Unknown(_)));
        let back = serde_json::to_value(&list).unwrap();
        assert_eq!(back[1], serde_json::json!({"type":"from_the_future","x":3}));
    }

    #[test]
    fn a_value_type_fits_a_value_to_what_it_can_hold() {
        assert_eq!(ValueType::Bool.fit([0.6, 0.0, 0.0], 0.5), [1.0, 0.0, 0.0]);
        assert_eq!(ValueType::Bool.fit([0.2, 0.0, 0.0], 0.5), [0.0, 0.0, 0.0]);
        assert_eq!(ValueType::Axis1D.fit([0.3, 0.9, 0.9], 0.5), [0.3, 0.0, 0.0]);
        assert_eq!(ValueType::Axis2D.fit([0.3, 0.9, 0.9], 0.5), [0.3, 0.9, 0.0]);
    }

    #[test]
    fn swizzle_orders_send_the_named_input_to_each_output() {
        assert_eq!(Swizzle::Yxz.order(), [1, 0, 2]);
        assert_eq!(Swizzle::Zyx.order(), [2, 1, 0]);
    }
}
