//! Choosing a key: search the list, or press the key to bind it.
//!
//! "Listen" is the quick way: press a keyboard key, click a mouse button,
//! turn the wheel or push a controller control, and that is the key. It
//! reads the controllers on a thread of its own for as long as it listens.

use std::time::Duration;

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::keys::{catalog, id_for_gpui_key, id_for_pad, Device};
use crate::runtime::gamepad::Gamepads;

use super::chooser::{ChoiceItem, Chooser, ChosenEvent, HEIGHT, WIDTH};

/// How often a listening picker looks at the controllers.
const POLL: Duration = Duration::from_millis(40);
/// How far a control must move to count as the one meant.
const PUSHED: f32 = 0.6;

/// The key table as chooser items, grouped by device and area.
pub fn key_items() -> Vec<ChoiceItem> {
    let mut items = Vec::new();
    for device in Device::ALL {
        let mut groups: Vec<&'static str> = Vec::new();
        for key in catalog().all().iter().filter(|k| k.device == device) {
            if !groups.contains(&key.group) {
                groups.push(key.group);
            }
        }
        for group in groups {
            for key in catalog().all().iter().filter(|k| k.device == device && k.group == group) {
                items.push(
                    ChoiceItem::new(key.id.clone(), key.label.clone())
                        .group(format!("{} · {}", device.label(), group))
                        .tag(match key.kind {
                            crate::keys::KeyKind::Button => String::new(),
                            other => other.label().to_string(),
                        }),
                );
            }
        }
    }
    items
}

pub struct KeyPicker {
    chooser: Entity<Chooser>,
    listening: bool,
    pads: Option<Gamepads>,
    poll: Option<Task<()>>,
    capture: FocusHandle,
    modifiers: Modifiers,
    _chosen: Subscription,
}

impl EventEmitter<ChosenEvent> for KeyPicker {}
impl EventEmitter<DismissEvent> for KeyPicker {}

impl Focusable for KeyPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.listening {
            self.capture.clone()
        } else {
            self.chooser.read(cx).focus_handle(cx)
        }
    }
}

impl KeyPicker {
    pub fn new(selected: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chooser = cx.new(|cx| Chooser::new(key_items(), selected, window, cx).with_empty_text("No key matches"));
        let chosen = cx.subscribe(&chooser, |this: &mut Self, _chooser, event: &ChosenEvent, cx| {
            this.finish(event.0.clone(), cx);
        });
        Self {
            chooser,
            listening: false,
            pads: None,
            poll: None,
            capture: cx.focus_handle(),
            modifiers: Modifiers::default(),
            _chosen: chosen,
        }
    }

    pub fn sync(&mut self, selected: &str, cx: &mut Context<Self>) {
        self.chooser.update(cx, |c, cx| c.sync(key_items(), selected, cx));
    }

    fn finish(&mut self, key: String, cx: &mut Context<Self>) {
        self.stop_listening();
        cx.emit(ChosenEvent(key));
        cx.emit(DismissEvent);
    }

    fn stop_listening(&mut self) {
        self.listening = false;
        self.pads = None;
        self.poll = None;
    }

    fn start_listening(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.listening = true;
        self.modifiers = window.modifiers();
        window.focus(&self.capture, cx);
        self.pads = Some(Gamepads::spawn());
        self.poll = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let alive = this.update(cx, |this, cx| this.poll_pads(cx));
            if !alive.unwrap_or(false) {
                break;
            }
        }));
        cx.notify();
    }

    /// Look at the controllers; `false` once listening has ended.
    fn poll_pads(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.listening {
            return false;
        }
        let Some(pads) = &self.pads else { return false };
        let pushed = pads.take().into_iter().find(|(_, value)| *value >= PUSHED || *value <= -PUSHED);
        if let Some((pad, _)) = pushed {
            if let Some(key) = id_for_pad(pad) {
                self.finish(key, cx);
                return false;
            }
        }
        true
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let name = event.keystroke.key.as_str();
        if name == "escape" {
            self.stop_listening();
            cx.notify();
        } else if let Some(key) = id_for_gpui_key(name) {
            self.finish(key, cx);
        }
        cx.stop_propagation();
    }

    /// Modifier keys arrive as a change of state, not as key presses.
    fn on_modifiers(&mut self, event: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        let (was, now) = (self.modifiers, event.modifiers);
        self.modifiers = now;
        let pressed = [
            (!was.shift && now.shift, "Keyboard.ShiftLeft"),
            (!was.control && now.control, "Keyboard.ControlLeft"),
            (!was.alt && now.alt, "Keyboard.AltLeft"),
            (!was.platform && now.platform, "Keyboard.SuperLeft"),
        ];
        if let Some((_, key)) = pressed.into_iter().find(|(on, _)| *on) {
            self.finish(key.to_string(), cx);
        }
    }
}

impl Render for KeyPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.listening {
            return v_flex()
                .id("key-listen")
                .track_focus(&self.capture)
                .w(px(WIDTH))
                .h(px(HEIGHT))
                .items_center()
                .justify_center()
                .gap_3()
                .bg(theme.popover)
                .cursor_pointer()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
                .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| this.on_modifiers(event, cx)))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish("Mouse.Left".into(), cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.finish("Mouse.Right".into(), cx)))
                .on_mouse_down(MouseButton::Middle, cx.listener(|this, _, _, cx| this.finish("Mouse.Middle".into(), cx)))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                    let dy = match event.delta {
                        ScrollDelta::Lines(l) => l.y,
                        ScrollDelta::Pixels(p) => f32::from(p.y),
                    };
                    if dy != 0.0 {
                        this.finish(if dy > 0.0 { "Mouse.WheelUp" } else { "Mouse.WheelDown" }.into(), cx);
                    }
                }))
                .child(Icon::new(IconName::Key).size(px(36.0)).text_color(theme.info))
                .child(div().text_base().font_semibold().text_color(theme.foreground).child("Press a key, click, or move a control"))
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .px_6()
                        .text_color(theme.muted_foreground)
                        .child("Keyboard, mouse buttons, the wheel and game controllers are all heard. Escape stops listening."),
                )
                .child(
                    Button::new("key-listen-cancel")
                        .label("Stop listening")
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stop_listening();
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        v_flex()
            .child(
                h_flex()
                    .w(px(WIDTH))
                    .flex_shrink_0()
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .bg(theme.popover)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("key-listen")
                            .icon(IconName::Key)
                            .label("Listen for input")
                            .small()
                            .tooltip("Press the key, click the button or push the control to bind it")
                            .on_click(cx.listener(|this, _, window, cx| this.start_listening(window, cx))),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child("or search the list")),
            )
            .child(self.chooser.clone())
            .into_any_element()
    }
}
