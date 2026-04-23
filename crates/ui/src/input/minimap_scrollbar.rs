//! VSCode-style minimap scrollbar component.
//!
//! This component integrates the minimap directly into a custom scrollbar,
//! providing both scrolling functionality and document overview.

use gpui::*;
use ropey::{LineType, Rope};
use std::ops::Range;

use super::minimap::{
    calculate_viewport_indicator, minimap_click_to_line, render_viewport_indicator, MinimapConfig,
    MinimapState,
};
use crate::{ActiveTheme, Colorize};

/// A scrollbar with an integrated VSCode-style minimap.
pub struct MinimapScrollbar {
    /// The text content to display in the minimap.
    text: Rope,

    /// Currently visible line range in the editor.
    visible_range: Range<usize>,

    /// Total number of lines in the document.
    total_lines: usize,

    /// Minimap configuration.
    config: MinimapConfig,

    /// Minimap interaction state.
    state: MinimapState,

    /// Callback when user clicks/drags to navigate.
    on_navigate: Option<Box<dyn Fn(usize) + 'static>>,
}

impl MinimapScrollbar {
    /// Create a new minimap scrollbar.
    pub fn new(text: Rope, visible_range: Range<usize>, total_lines: usize) -> Self {
        Self {
            text,
            visible_range,
            total_lines,
            config: MinimapConfig::default(),
            state: MinimapState::new(),
            on_navigate: None,
        }
    }

    /// Set the minimap configuration.
    pub fn config(mut self, config: MinimapConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the navigation callback.
    pub fn on_navigate(mut self, callback: impl Fn(usize) + 'static) -> Self {
        self.on_navigate = Some(Box::new(callback));
        self
    }

    /// Render the minimap content lines.
    fn render_content(&self, minimap_bounds: Bounds<Pixels>, _cx: &App) -> Vec<AnyElement> {
        let mut elements = vec![];

        // Sample rate based on file size for performance
        let sample_rate = match self.total_lines {
            0..=1000 => 1,
            1001..=10000 => 5,
            10001..=50000 => 20,
            _ => 50,
        };

        // Render density bars for sampled lines
        for line_idx in (0..self.total_lines).step_by(sample_rate) {
            // Safety check - use len_lines() method
            if line_idx >= self.text.len_lines(LineType::LF) {
                break;
            }

            // Get line content - provide LineType parameter
            let line_text = self.text.line(line_idx, LineType::LF).to_string();
            let trimmed = line_text.trim();

            // Calculate density (how full is this line)
            let density = (trimmed.len() as f32 / self.config.max_chars_per_line as f32).min(1.0);

            // Calculate Y position (proportional to line number)
            let y_ratio = line_idx as f32 / self.total_lines as f32;
            let y_pos = minimap_bounds.origin.y + (y_ratio * minimap_bounds.size.height);

            // Only render if within bounds and non-empty
            if y_pos < minimap_bounds.origin.y + minimap_bounds.size.height && density > 0.05 {
                let bar_width = self.config.width * density;
                let bar_height = self.config.line_height.max(px(1.0)); // Minimum 1px

                elements.push(
                    div()
                        .absolute()
                        .left(minimap_bounds.origin.x + px(4.0))
                        .top(y_pos)
                        .w(bar_width - px(8.0))
                        .h(bar_height)
                        .bg(rgb(0x606060)) // Subtle gray for code
                        .into_any_element(),
                );
            }
        }

        elements
    }
}

impl IntoElement for MinimapScrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MinimapScrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Bounds<Pixels>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = self.config.width.into();
        style.size.height = relative(1.0).into();
        style.position = Position::Absolute;
        style.inset.right = px(0.0).into();
        style.inset.top = px(0.0).into();

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        bounds
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Self::PrepaintState,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let minimap_bounds = bounds;

        // Background
        window.paint_quad(
            fill(minimap_bounds, cx.theme().secondary).corner_radii(Corners::all(px(4.0))),
        );

        // Viewport indicator
        let indicator_bounds = calculate_viewport_indicator(
            &minimap_bounds,
            &self.visible_range,
            self.total_lines,
            &self.config,
        );

        window.paint_quad(PaintQuad {
            bounds: indicator_bounds,
            corner_radii: Corners::all(px(2.0)),
            background: cx.theme().accent.into(),
            border_widths: Edges::all(px(1.0)),
            border_color: cx.theme().accent,
            border_style: BorderStyle::Solid,
        });

        // Mouse interaction - create hitbox using correct API
        window.insert_hitbox(minimap_bounds, HitboxBehavior::default());

        // TODO: Add mouse event handlers for click and drag
        // This would require integration with the Element trait's event system
    }
}

/// Helper to create a minimap scrollbar div with proper event handling.
///
/// This is the recommended way to use the minimap in your code.
pub fn minimap_scrollbar(
    text: &Rope,
    visible_range: Range<usize>,
    total_lines: usize,
    _on_scroll: impl Fn(usize) + 'static,
) -> impl IntoElement {
    let config = MinimapConfig::default();

    div()
        .absolute()
        .right_0()
        .top_0()
        .w(config.width)
        .h_full()
        .bg(rgb(0x1e1e1e))
        .rounded_md()
        .child({
            // Viewport indicator
            let indicator_bounds = calculate_viewport_indicator(
                &Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(config.width, px(600.0)), // Placeholder, would be actual height
                ),
                &visible_range,
                total_lines,
                &config,
            );

            div()
                .absolute()
                .left(px(0.0))
                .top(indicator_bounds.origin.y)
                .w_full()
                .h(indicator_bounds.size.height)
                .bg(rgb(0x007acc))
                .border_2()
                .border_color(rgb(0x007acc))
                .rounded_sm()
        })
        .on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
            // Calculate which line was clicked
            // TODO: Implement proper click-to-scroll logic
        })
}
