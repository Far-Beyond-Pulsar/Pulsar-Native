//! Editing a list of triggers or modifiers: add from a menu, reorder, remove,
//! and set each one's values.
//!
//! The same list appears on an action and on a single binding, so it is
//! written once over [`Behavior`], which both [`Trigger`] and [`Modifier`]
//! implement: what they are called, what the user can set on each, and what
//! can be added.

use std::rc::Rc;

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::input::NumberInput;
use ui::menu::DropdownMenu;
use ui::switch::Switch;
use ui::{h_flex, v_flex, ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt};

use crate::model::behavior::{Curve, Modifier, ModifierKind, Swizzle, Tolerant, Trigger, TriggerKind};
use crate::model::{ActionRef, AssetId};

use super::chooser::ChoiceItem;
use super::fields::{Fields, NumberSpec};

/// One setting of a trigger or modifier.
pub enum Control<T> {
    Number { label: &'static str, spec: NumberSpec, get: fn(&T) -> f32, set: fn(&mut T, f32) },
    Flag { label: &'static str, get: fn(&T) -> bool, set: fn(&mut T, bool) },
    /// `options` are `(value, label)`.
    Choice { label: &'static str, options: &'static [(&'static str, &'static str)], get: fn(&T) -> &'static str, set: fn(&mut T, &str) },
    /// Another action, picked from the project's.
    Action { label: &'static str, get: fn(&T) -> Option<ActionRef>, set: fn(&mut T, ActionRef) },
}

/// Something that goes in a list on an action or a binding.
pub trait Behavior: Clone + PartialEq + 'static {
    /// The word for one of these ("trigger"), for labels.
    const NOUN: &'static str;
    const ICON: IconName;

    fn title(&self) -> &'static str;
    fn summary(&self) -> String;
    fn description(&self) -> &'static str;
    fn controls(&self) -> Vec<Control<Self>>;
    /// Everything that can be added: `(title, description, a default one)`.
    fn catalog() -> Vec<(&'static str, &'static str, Self)>;
}

const SECONDS: NumberSpec = NumberSpec { step: 0.05, min: 0.0, max: 60.0, decimals: 2 };
const UNIT: NumberSpec = NumberSpec { step: 0.05, min: -1_000.0, max: 1_000.0, decimals: 2 };

impl Behavior for Trigger {
    const NOUN: &'static str = "trigger";
    const ICON: IconName = IconName::Flash;

    fn title(&self) -> &'static str {
        self.kind().label()
    }

    fn summary(&self) -> String {
        Trigger::summary(self)
    }

    fn description(&self) -> &'static str {
        self.kind().description()
    }

    fn controls(&self) -> Vec<Control<Self>> {
        match self {
            Trigger::Hold { .. } => vec![
                Control::Number {
                    label: "Seconds",
                    spec: SECONDS,
                    get: |t| if let Trigger::Hold { seconds, .. } = t { *seconds } else { 0.0 },
                    set: |t, v| {
                        if let Trigger::Hold { seconds, .. } = t {
                            *seconds = v
                        }
                    },
                },
                Control::Flag {
                    label: "Only once",
                    get: |t| matches!(t, Trigger::Hold { once: true, .. }),
                    set: |t, v| {
                        if let Trigger::Hold { once, .. } = t {
                            *once = v
                        }
                    },
                },
            ],
            Trigger::HoldAndRelease { .. } => vec![Control::Number {
                label: "Seconds",
                spec: SECONDS,
                get: |t| if let Trigger::HoldAndRelease { seconds } = t { *seconds } else { 0.0 },
                set: |t, v| {
                    if let Trigger::HoldAndRelease { seconds } = t {
                        *seconds = v
                    }
                },
            }],
            Trigger::Tap { .. } => vec![Control::Number {
                label: "Longest tap (s)",
                spec: SECONDS,
                get: |t| if let Trigger::Tap { max_seconds } = t { *max_seconds } else { 0.0 },
                set: |t, v| {
                    if let Trigger::Tap { max_seconds } = t {
                        *max_seconds = v
                    }
                },
            }],
            Trigger::Pulse { .. } => vec![
                Control::Number {
                    label: "Interval (s)",
                    spec: NumberSpec { min: 0.01, ..SECONDS },
                    get: |t| if let Trigger::Pulse { interval, .. } = t { *interval } else { 0.0 },
                    set: |t, v| {
                        if let Trigger::Pulse { interval, .. } = t {
                            *interval = v
                        }
                    },
                },
                Control::Number {
                    label: "Limit (0: none)",
                    spec: NumberSpec { step: 1.0, min: 0.0, max: 100_000.0, decimals: 0 },
                    get: |t| if let Trigger::Pulse { limit, .. } = t { *limit as f32 } else { 0.0 },
                    set: |t, v| {
                        if let Trigger::Pulse { limit, .. } = t {
                            *limit = v.round().max(0.0) as u32
                        }
                    },
                },
                Control::Flag {
                    label: "Fire on press",
                    get: |t| matches!(t, Trigger::Pulse { on_start: true, .. }),
                    set: |t, v| {
                        if let Trigger::Pulse { on_start, .. } = t {
                            *on_start = v
                        }
                    },
                },
            ],
            Trigger::DoubleTap { .. } => vec![Control::Number {
                label: "Window (s)",
                spec: SECONDS,
                get: |t| if let Trigger::DoubleTap { window } = t { *window } else { 0.0 },
                set: |t, v| {
                    if let Trigger::DoubleTap { window } = t {
                        *window = v
                    }
                },
            }],
            Trigger::Chorded { .. } => vec![Control::Action {
                label: "While",
                get: |t| if let Trigger::Chorded { action } = t { Some(action.clone()) } else { None },
                set: |t, a| {
                    if let Trigger::Chorded { action } = t {
                        *action = a
                    }
                },
            }],
            Trigger::Down | Trigger::Pressed | Trigger::Released => Vec::new(),
        }
    }

    fn catalog() -> Vec<(&'static str, &'static str, Self)> {
        TriggerKind::ALL.iter().map(|k| (k.label(), k.description(), k.default_trigger())).collect()
    }
}

const SWIZZLES: &[(&str, &str)] = &[
    ("yxz", "YXZ (swap X and Y)"),
    ("xzy", "XZY (swap Y and Z)"),
    ("zyx", "ZYX (swap X and Z)"),
    ("yzx", "YZX"),
    ("zxy", "ZXY"),
];

const CURVES: &[(&str, &str)] = &[("exponential", "Exponential"), ("quadratic", "Quadratic"), ("cubic", "Cubic"), ("smooth_step", "Smooth step")];

fn swizzle_value(s: Swizzle) -> &'static str {
    match s {
        Swizzle::Yxz => "yxz",
        Swizzle::Xzy => "xzy",
        Swizzle::Zyx => "zyx",
        Swizzle::Yzx => "yzx",
        Swizzle::Zxy => "zxy",
    }
}

fn swizzle_from(value: &str) -> Swizzle {
    Swizzle::ALL.iter().copied().find(|s| swizzle_value(*s) == value).unwrap_or_default()
}

fn curve_value(c: Curve) -> &'static str {
    match c {
        Curve::Exponential => "exponential",
        Curve::Quadratic => "quadratic",
        Curve::Cubic => "cubic",
        Curve::SmoothStep => "smooth_step",
    }
}

fn curve_from(value: &str) -> Curve {
    Curve::ALL.iter().copied().find(|c| curve_value(*c) == value).unwrap_or_default()
}

macro_rules! scale_axis {
    ($label:literal, $field:ident) => {
        Control::Number {
            label: $label,
            spec: UNIT,
            get: |m| if let Modifier::Scale { $field, .. } = m { *$field } else { 1.0 },
            set: |m, v| {
                if let Modifier::Scale { $field, .. } = m {
                    *$field = v
                }
            },
        }
    };
}

macro_rules! negate_axis {
    ($label:literal, $field:ident) => {
        Control::Flag {
            label: $label,
            get: |m| matches!(m, Modifier::Negate { $field: true, .. }),
            set: |m, v| {
                if let Modifier::Negate { $field, .. } = m {
                    *$field = v
                }
            },
        }
    };
}

impl Behavior for Modifier {
    const NOUN: &'static str = "modifier";
    const ICON: IconName = IconName::Settings;

    fn title(&self) -> &'static str {
        self.kind().label()
    }

    fn summary(&self) -> String {
        Modifier::summary(self)
    }

    fn description(&self) -> &'static str {
        self.kind().description()
    }

    fn controls(&self) -> Vec<Control<Self>> {
        match self {
            Modifier::DeadZone { .. } => vec![
                Control::Number {
                    label: "Lower",
                    spec: NumberSpec { step: 0.01, min: 0.0, max: 1.0, decimals: 2 },
                    get: |m| if let Modifier::DeadZone { lower, .. } = m { *lower } else { 0.0 },
                    set: |m, v| {
                        if let Modifier::DeadZone { lower, .. } = m {
                            *lower = v
                        }
                    },
                },
                Control::Number {
                    label: "Upper",
                    spec: NumberSpec { step: 0.01, min: 0.0, max: 1.0, decimals: 2 },
                    get: |m| if let Modifier::DeadZone { upper, .. } = m { *upper } else { 1.0 },
                    set: |m, v| {
                        if let Modifier::DeadZone { upper, .. } = m {
                            *upper = v
                        }
                    },
                },
                Control::Flag {
                    label: "Radial (judge the stick as a whole)",
                    get: |m| matches!(m, Modifier::DeadZone { radial: true, .. }),
                    set: |m, v| {
                        if let Modifier::DeadZone { radial, .. } = m {
                            *radial = v
                        }
                    },
                },
            ],
            Modifier::Scale { .. } => vec![scale_axis!("X", x), scale_axis!("Y", y), scale_axis!("Z", z)],
            Modifier::Negate { .. } => vec![negate_axis!("Negate X", x), negate_axis!("Negate Y", y), negate_axis!("Negate Z", z)],
            Modifier::Swizzle { .. } => vec![Control::Choice {
                label: "Order",
                options: SWIZZLES,
                get: |m| if let Modifier::Swizzle { order } = m { swizzle_value(*order) } else { "yxz" },
                set: |m, v| {
                    if let Modifier::Swizzle { order } = m {
                        *order = swizzle_from(v)
                    }
                },
            }],
            Modifier::Smooth { .. } => vec![Control::Number {
                label: "Speed",
                spec: NumberSpec { step: 0.5, min: 0.0, max: 1_000.0, decimals: 1 },
                get: |m| if let Modifier::Smooth { speed } = m { *speed } else { 0.0 },
                set: |m, v| {
                    if let Modifier::Smooth { speed } = m {
                        *speed = v
                    }
                },
            }],
            Modifier::ResponseCurve { curve, .. } => {
                let mut controls = vec![Control::Choice {
                    label: "Curve",
                    options: CURVES,
                    get: |m| if let Modifier::ResponseCurve { curve, .. } = m { curve_value(*curve) } else { "exponential" },
                    set: |m, v| {
                        if let Modifier::ResponseCurve { curve, .. } = m {
                            *curve = curve_from(v)
                        }
                    },
                }];
                if *curve == Curve::Exponential {
                    controls.push(Control::Number {
                        label: "Exponent",
                        spec: NumberSpec { step: 0.1, min: 0.0, max: 16.0, decimals: 2 },
                        get: |m| if let Modifier::ResponseCurve { exponent, .. } = m { *exponent } else { 2.0 },
                        set: |m, v| {
                            if let Modifier::ResponseCurve { exponent, .. } = m {
                                *exponent = v
                            }
                        },
                    });
                }
                controls
            }
            Modifier::Clamp { .. } => vec![
                Control::Number {
                    label: "Minimum",
                    spec: UNIT,
                    get: |m| if let Modifier::Clamp { min, .. } = m { *min } else { 0.0 },
                    set: |m, v| {
                        if let Modifier::Clamp { min, .. } = m {
                            *min = v
                        }
                    },
                },
                Control::Number {
                    label: "Maximum",
                    spec: UNIT,
                    get: |m| if let Modifier::Clamp { max, .. } = m { *max } else { 0.0 },
                    set: |m, v| {
                        if let Modifier::Clamp { max, .. } = m {
                            *max = v
                        }
                    },
                },
            ],
        }
    }

    fn catalog() -> Vec<(&'static str, &'static str, Self)> {
        ModifierKind::ALL.iter().map(|k| (k.label(), k.description(), k.default_modifier())).collect()
    }
}

/// How the list reads and writes its model. Both run when a control fires,
/// so they must look at the model as it is then.
pub struct ListModel<V: 'static, T: Behavior> {
    pub get: Rc<dyn Fn(&V, &App) -> Vec<Tolerant<T>>>,
    /// Store a new list; the `bool` is true when the change is a typed value
    /// that should merge into one undo step.
    pub set: Rc<dyn Fn(&mut V, Vec<Tolerant<T>>, bool, &mut Context<V>)>,
}

impl<V: 'static, T: Behavior> Clone for ListModel<V, T> {
    fn clone(&self) -> Self {
        Self { get: self.get.clone(), set: self.set.clone() }
    }
}

fn labelled(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().w(px(110.0)).flex_shrink_0().text_xs().text_color(cx.theme().muted_foreground).child(label))
        .child(div().flex_1().min_w_0().child(control))
}

/// Edit `items`. `id` keeps the list's controls apart from other lists'.
/// `actions` are the project's actions, for the controls that pick one.
#[allow(clippy::too_many_arguments)]
pub fn behavior_list<V: 'static, T: Behavior>(
    id: &str,
    fields: &mut Fields<V>,
    items: &[Tolerant<T>],
    model: ListModel<V, T>,
    actions: &[ChoiceItem],
    empty_hint: &str,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let count = items.len();
    let entity = cx.entity();

    let mut list = v_flex().w_full().gap_2();
    if items.is_empty() {
        list = list.child(div().w_full().p_3().rounded(px(6.0)).border_1().border_dashed().border_color(theme.border).text_xs().text_color(theme.muted_foreground).child(empty_hint.to_string()));
    }
    for (index, item) in items.iter().enumerate() {
        let (title, summary, description, controls, known) = match item {
            Tolerant::Known(t) => (t.title().to_string(), t.summary(), t.description(), t.controls(), true),
            Tolerant::Unknown(_) => (format!("Unknown {}", T::NOUN), "Made by a newer version".to_string(), "It is kept as it is and not evaluated.", Vec::new(), false),
        };
        let current = item.known().cloned();

        let mut body = v_flex().gap_1p5().pl_2().pt_1();
        if controls.is_empty() {
            body = body.child(div().text_xs().text_color(theme.muted_foreground).child(description));
        }
        for (n, control) in controls.into_iter().enumerate() {
            let key = format!("{id}:{index}:{n}");
            let element: AnyElement = match control {
                Control::Number { label, spec, get, set } => {
                    let value = current.as_ref().map(get);
                    let (m_get, m_set) = (model.clone(), model.clone());
                    let input = fields.number(
                        key,
                        spec,
                        value,
                        move |this, cx| (m_get.get)(this, cx).get(index).and_then(Tolerant::known).map(get),
                        move |this, v, cx| {
                            let mut list = (m_set.get)(this, cx);
                            if let Some(t) = list.get_mut(index).and_then(Tolerant::known_mut) {
                                set(t, v);
                            }
                            (m_set.set)(this, list, true, cx);
                        },
                        window,
                        cx,
                    );
                    labelled(label, NumberInput::new(&input).small(), cx).into_any_element()
                }
                Control::Flag { label, get, set } => {
                    let on = current.as_ref().is_some_and(get);
                    let m = model.clone();
                    labelled(
                        label,
                        Switch::new(SharedString::from(key)).checked(on).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let mut list = (m.get)(this, cx);
                            if let Some(t) = list.get_mut(index).and_then(Tolerant::known_mut) {
                                set(t, *checked);
                            }
                            (m.set)(this, list, false, cx);
                        })),
                        cx,
                    )
                    .into_any_element()
                }
                Control::Choice { label, options, get, set } => {
                    let shown = current.as_ref().map(get).and_then(|v| options.iter().find(|(value, _)| *value == v)).map_or("", |(_, l)| *l);
                    let (m, entity) = (model.clone(), entity.clone());
                    labelled(
                        label,
                        Button::new(SharedString::from(key))
                            .label(shown)
                            .small()
                            .outline()
                            .w_full()
                            .dropdown_caret(true)
                            .popup_menu(move |menu, _, _| {
                                let mut menu = menu;
                                for (value, label) in options {
                                    let (m, entity, value) = (m.clone(), entity.clone(), *value);
                                    menu = menu.menu_handler(*label, move |_, cx| {
                                        entity.update(cx, |this, cx| {
                                            let mut list = (m.get)(this, cx);
                                            if let Some(t) = list.get_mut(index).and_then(Tolerant::known_mut) {
                                                set(t, value);
                                            }
                                            (m.set)(this, list, false, cx);
                                        });
                                    });
                                }
                                menu
                            }),
                        cx,
                    )
                    .into_any_element()
                }
                Control::Action { label, get, set } => {
                    let chosen = current.as_ref().and_then(get);
                    let name = chosen
                        .as_ref()
                        .map(|a| actions.iter().find(|c| c.id == a.id.as_str()).map_or_else(|| "Missing action".to_string(), |c| c.title.clone()))
                        .unwrap_or_else(|| "Choose an action".to_string());
                    let selected = chosen.as_ref().map_or(String::new(), |a| a.id.to_string());
                    let m = model.clone();
                    let known = Rc::new(actions.to_vec());
                    let chooser = fields.chooser(key.clone(), actions.to_vec(), Vec::new(), &selected, move |this, picked, cx| {
                        let Some(found) = known.iter().find(|c| c.id == picked) else { return };
                        let reference = ActionRef::new(AssetId::parse(&picked).unwrap_or_default(), found.detail.clone());
                        let mut list = (m.get)(this, cx);
                        if let Some(t) = list.get_mut(index).and_then(Tolerant::known_mut) {
                            set(t, reference);
                        }
                        (m.set)(this, list, false, cx);
                    }, window, cx);
                    labelled(
                        label,
                        ui::popover::Popover::<super::chooser::Chooser>::new(SharedString::from(format!("{key}-pop")))
                            .anchor(Corner::BottomLeft)
                            .trigger(Button::new(SharedString::from(key)).label(name).small().outline().w_full().dropdown_caret(true))
                            .content(move |_, _| chooser.clone()),
                        cx,
                    )
                    .into_any_element()
                }
            };
            body = body.child(element);
        }

        // The row's own buttons.
        let (up, down, remove) = (model.clone(), model.clone(), model.clone());
        let button = |name: &str, icon: IconName, tip: &'static str| Button::new(SharedString::from(format!("{id}:{index}:{name}"))).icon(icon).ghost().xsmall().tooltip(tip);
        list = list.child(
            v_flex()
                .w_full()
                .p_2()
                .gap_1()
                .rounded(px(6.0))
                .border_1()
                .border_color(if known { theme.border } else { theme.warning.opacity(0.5) })
                .bg(theme.secondary.opacity(0.5))
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .child(Icon::new(T::ICON).size(px(13.0)).text_color(theme.info))
                        .child(div().text_sm().font_semibold().text_color(theme.foreground).child(title))
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().text_color(theme.muted_foreground).child(summary))
                        .child(button("up", IconName::ArrowUp, "Move earlier").disabled(index == 0).on_click(cx.listener(move |this, _, _, cx| {
                            let mut list = (up.get)(this, cx);
                            if index > 0 && index < list.len() {
                                list.swap(index, index - 1);
                                (up.set)(this, list, false, cx);
                            }
                        })))
                        .child(button("down", IconName::ArrowDown, "Move later").disabled(index + 1 >= count).on_click(cx.listener(move |this, _, _, cx| {
                            let mut list = (down.get)(this, cx);
                            if index + 1 < list.len() {
                                list.swap(index, index + 1);
                                (down.set)(this, list, false, cx);
                            }
                        })))
                        .child(button("remove", IconName::Close, "Remove").on_click(cx.listener(move |this, _, _, cx| {
                            let mut list = (remove.get)(this, cx);
                            if index < list.len() {
                                list.remove(index);
                                (remove.set)(this, list, false, cx);
                            }
                        }))),
                )
                .child(body),
        );
    }

    // Add.
    let (m, add_entity) = (model, entity);
    let catalog = T::catalog();
    let add = Button::new(SharedString::from(format!("{id}:add")))
        .icon(IconName::Plus)
        .label(format!("Add {}", T::NOUN))
        .small()
        .outline()
        .popup_menu(move |menu, _, _| {
            let mut menu = menu;
            for (title, _description, item) in catalog.clone() {
                let (m, entity) = (m.clone(), add_entity.clone());
                menu = menu.menu_handler(title, move |_, cx| {
                    let item = item.clone();
                    entity.update(cx, |this, cx| {
                        let mut list = (m.get)(this, cx);
                        list.push(Tolerant::Known(item));
                        (m.set)(this, list, false, cx);
                    });
                });
            }
            menu
        });
    list.child(div().child(add)).into_any_element()
}
