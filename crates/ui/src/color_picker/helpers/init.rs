use super::*;

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Cancel, Some(CONTEXT))])
}

struct ColorPickerInit;
impl crate::registry::UiComponentInit for ColorPickerInit {
    fn init(&self, cx: &mut App) {
        init(cx);
    }
}
crate::register_ui_component!(ColorPickerInit);
