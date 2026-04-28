use super::*;

#[derive(Clone)]
pub enum ColorPickerEvent {
    Change(Option<Hsla>),
}
