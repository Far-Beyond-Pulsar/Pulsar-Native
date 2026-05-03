use gpui::{
    anchored, canvas, deferred, div, fill, point, prelude::FluentBuilder as _, px, relative, size,
    App, AppContext, Axis, Bounds, ClickEvent, Context, Corner, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, RenderOnce,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Subscription, Window,
};

use crate::{
    actions::{Cancel, Confirm},
    button::{Button, ButtonVariants},
    divider::Divider,
    h_flex,
    input::{InputEvent, InputState, Input},
    styled::PixelsExt,
    tooltip::Tooltip,
    v_flex, ActiveTheme as _, Colorize as _, FocusableExt as _, Icon, IconName, Selectable as _,
    Sizable, Size, StyleSized, StyledExt,
};

const CONTEXT: &'static str = "ColorPicker";
const PICKER_SIZE: f32 = 224.0;
const HUE_RING_THICKNESS: f32 = 20.0;
const SLIDER_HEIGHT: f32 = 18.0;
const CHECKER_CELL_SIZE: f32 = 8.0;
/// Columns in every row of the All Colors grid.
const ALL_COLORS_COLS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerDragTarget {
    HueRing,
    Triangle,
    R,
    G,
    B,
    A,
}

#[derive(Clone, Copy)]
struct PickerGeometry {
    cx: f32,
    cy: f32,
    outer_r: f32,
    inner_r: f32,
}

mod components;
mod helpers;
use components::*;
pub use helpers::ColorPickerEvent;
use helpers::*;

mod picker;
mod state;

pub use picker::ColorPicker;
pub use state::ColorPickerState;
