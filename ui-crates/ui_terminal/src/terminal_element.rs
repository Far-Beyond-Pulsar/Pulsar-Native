//! Complete terminal element with full interactivity
//! Adapted from Zed's terminal_element.rs

use super::terminal_core::{Terminal, TerminalBounds};
use super::rendering::{layout_grid, BatchedTextRun, LayoutRect};
use gpui::*;
use ui::ActiveTheme;
use alacritty_terminal::vte::ansi::CursorShape as AlacCursorShape;

// For cursor rendering - these types should exist in your editor crate
// If not, we'll need to define them inline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Bar,
    Block,
    Underscore,
    Hollow,
}

pub struct CursorLayout {
    origin: Point<Pixels>,
    block_width: Pixels,
    line_height: Pixels,
    color: Hsla,
    shape: CursorShape,
    text: Option<ShapedLine>,
}

impl CursorLayout {
    pub fn new(
        origin: Point<Pixels>,
        block_width: Pixels,
        line_height: Pixels,
        color: Hsla,
        shape: CursorShape,
        text: Option<ShapedLine>,
    ) -> Self {
        Self {
            origin,
            block_width,
            line_height,
            color,
            shape,
            text,
        }
    }

    pub fn bounding_rect(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: point(origin.x + self.origin.x, origin.y + self.origin.y),
            size: size(self.block_width, self.line_height),
        }
    }

    pub fn paint(&mut self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let position = point(origin.x + self.origin.x, origin.y + self.origin.y);
        
        match self.shape {
            CursorShape::Block => {
                // Filled block cursor
                window.paint_quad(fill(
                    Bounds {
                        origin: position,
                        size: size(self.block_width, self.line_height),
                    },
                    self.color,
                ));
                
                // Paint the text on top if available
                if let Some(text) = &self.text {
                    let _ = text.paint(position, self.line_height, window, cx);
                }
            }
            CursorShape::Hollow => {
                // Hollow block cursor (just outline)
                let bounds = Bounds {
                    origin: position,
                    size: size(self.block_width, self.line_height),
                };
                
                // Draw outline by painting 4 rectangles
                let thickness = px(1.0);
                
                // Top
                window.paint_quad(fill(
                    Bounds {
                        origin: bounds.origin,
                        size: size(bounds.size.width, thickness),
                    },
                    self.color,
                ));
                
                // Bottom
                window.paint_quad(fill(
                    Bounds {
                        origin: point(bounds.origin.x, bounds.origin.y + bounds.size.height - thickness),
                        size: size(bounds.size.width, thickness),
                    },
                    self.color,
                ));
                
                // Left
                window.paint_quad(fill(
                    Bounds {
                        origin: bounds.origin,
                        size: size(thickness, bounds.size.height),
                    },
                    self.color,
                ));
                
                // Right
                window.paint_quad(fill(
                    Bounds {
                        origin: point(bounds.origin.x + bounds.size.width - thickness, bounds.origin.y),
                        size: size(thickness, bounds.size.height),
                    },
                    self.color,
                ));
            }
            CursorShape::Bar => {
                // Vertical bar cursor
                window.paint_quad(fill(
                    Bounds {
                        origin: position,
                        size: size(px(2.0), self.line_height),
                    },
                    self.color,
                ));
            }
            CursorShape::Underscore => {
                // Underscore cursor at the bottom
                window.paint_quad(fill(
                    Bounds {
                        origin: point(position.x, position.y + self.line_height - px(2.0)),
                        size: size(self.block_width, px(2.0)),
                    },
                    self.color,
                ));
            }
        }
    }
}

/// Simple terminal input handler (simplified from Zed)
struct TerminalInputHandler {
    terminal: Entity<Terminal>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(&mut self, _ignore_disabled_input: bool, _window: &mut Window, _cx: &mut App) -> Option<UTF16Selection> {
        // Return empty selection
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<std::ops::Range<usize>> {
        None
    }

    fn text_for_range(&mut self, _range: std::ops::Range<usize>, _actual_range: &mut Option<std::ops::Range<usize>>, _window: &mut Window, _cx: &mut App) -> Option<String> {
        None
    }

    fn replace_text_in_range(&mut self, _replacement_range: Option<std::ops::Range<usize>>, text: &str, window: &mut Window, cx: &mut App) {
        // This is the key method - send text to terminal!
        self.terminal.update(cx, |terminal, cx| {
            terminal.handle_input(text, window, cx);
        });
    }

    fn replace_and_mark_text_in_range(&mut self, _range_utf16: Option<std::ops::Range<usize>>, _new_text: &str, _new_marked_range: Option<std::ops::Range<usize>>, _window: &mut Window, _cx: &mut App) {
        // Not needed for basic terminal
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {
        // Not needed for basic terminal
    }

    fn bounds_for_range(&mut self, _range_utf16: std::ops::Range<usize>, _window: &mut Window, _cx: &mut App) -> Option<Bounds<Pixels>> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _window: &mut Window, _cx: &mut App) -> Option<usize> {
        None
    }
}

/// Layout state for terminal rendering (from Zed)
pub struct LayoutState {
    pub hitbox: Hitbox,
    pub batched_text_runs: Vec<BatchedTextRun>,
    pub rects: Vec<LayoutRect>,
    pub cursor: Option<CursorLayout>,
    pub background_color: Hsla,
    pub dimensions: TerminalBounds,
    pub text_style: TextStyle,
}

/// Helper struct for converting data between Alacritty's cursor points, and displayed cursor points (from Zed)
struct DisplayCursor {
    line: i32,
    col: usize,
}

impl DisplayCursor {
    fn from(cursor_point: alacritty_terminal::index::Point, display_offset: usize) -> Self {
        Self {
            line: cursor_point.line.0 + display_offset as i32,
            col: cursor_point.column.0,
        }
    }

    pub fn line(&self) -> i32 {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }
}

/// Terminal element with full interactivity (adapted from Zed)
pub struct TerminalElement {
    terminal: Entity<Terminal>,
    focus: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    interactivity: Interactivity,
}

impl InteractiveElement for TerminalElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for TerminalElement {}

impl TerminalElement {
    pub fn new(terminal: Entity<Terminal>, focus: FocusHandle, focused: bool) -> Self {
        TerminalElement {
            terminal,
            focus: focus.clone(),
            focused,
            cursor_visible: true,
            interactivity: Interactivity::default(),
        }
        .track_focus(&focus)
        .key_context(super::TERMINAL_CONTEXT)  // Set Terminal key context for Tab handling
    }

    /// Computes the cursor position and expected block width (from Zed)
    /// May return a zero width if x_for_index returns the same position for sequential indexes. Use em_width instead
    fn shape_cursor(
        cursor_point: DisplayCursor,
        size: &TerminalBounds,
        text_fragment: &ShapedLine,
    ) -> Option<(Point<Pixels>, Pixels)> {
        if cursor_point.line() < size.num_lines() as i32 {
            let cursor_width = if text_fragment.width == Pixels::ZERO {
                size.cell_width
            } else {
                text_fragment.width
            };

            // Cursor should always surround as much of the text as possible,
            // hence when on pixel boundaries round the origin down and the width up
            Some((
                point(
                    (cursor_point.col() as f32 * size.cell_width).floor(),
                    (cursor_point.line() as f32 * size.line_height).floor(),
                ),
                cursor_width.ceil(),
            ))
        } else {
            None
        }
    }

    // Adapted from Zed's register_mouse_listeners
    fn register_mouse_listeners(&mut self, hitbox: &Hitbox, window: &mut Window) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let origin = hitbox.bounds.origin;

        // Left mouse button down - focus terminal and send to terminal
        self.interactivity.on_mouse_down(MouseButton::Left, {
            let focus = focus.clone();
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, window, cx| {
                window.focus(&focus);
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(&e, origin, cx);
                });
            }
        });
        
        // Left mouse button up
        self.interactivity.on_mouse_up(MouseButton::Left, {
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, _window, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_up(&e, origin, cx);
                });
            }
        });

        // Right mouse button (for mouse mode programs)
        self.interactivity.on_mouse_down(MouseButton::Right, {
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, _window, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(&e, origin, cx);
                });
            }
        });
        
        self.interactivity.on_mouse_up(MouseButton::Right, {
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, _window, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_up(&e, origin, cx);
                });
            }
        });
        
        // Middle mouse button
        self.interactivity.on_mouse_down(MouseButton::Middle, {
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, _window, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(&e, origin, cx);
                });
            }
        });
        
        self.interactivity.on_mouse_up(MouseButton::Middle, {
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e, _window, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_up(&e, origin, cx);
                });
            }
        });

        // Mouse move for hover effects and mouse mode
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            let terminal = terminal.clone();
            let origin = origin.clone();

            move |e: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                // Check if mouse is in bounds
                if hitbox.bounds.contains(&e.position) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_move(&e, origin);
                        cx.notify();
                    });
                }
            }
        });

        // Scroll wheel - from Zed (enhanced with mouse mode support)
        self.interactivity.on_scroll_wheel({
            let terminal = terminal.clone();
            let origin = origin.clone();
            
            move |event: &ScrollWheelEvent, _phase, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_scroll(event, origin, cx);
                });
            }
        });
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    // Adapted from Zed's request_layout
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                style.size.width = relative(1.).into();
                style.size.height = relative(1.).into();
                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    // Adapted from Zed's prepaint - this is where the magic happens
    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                let hitbox = hitbox.unwrap();
                
                // Clone theme colors we need before any borrows
                let (background_color, foreground_color) = {
                    let theme = cx.theme();
                    (theme.background, theme.foreground)
                };
                
                // Create text style exactly like script editor (JetBrains Mono)
                let text_style = TextStyle {
                    font_family: "JetBrains Mono".into(),
                    font_features: FontFeatures::default(),
                    font_weight: FontWeight::NORMAL,
                    font_fallbacks: Some(FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                    font_size: px(13.0).into(),
                    font_style: FontStyle::Normal,
                    line_height: px(20.0).into(),
                    background_color: Some(background_color),
                    white_space: WhiteSpace::Normal,
                    color: foreground_color,
                    ..Default::default()
                };

                // Use JetBrains Mono font like script editor - EXACT match
                let font = Font {
                    family: "JetBrains Mono".to_string().into(),
                    weight: FontWeight::NORMAL,
                    style: FontStyle::Normal,
                    features: FontFeatures::default(),
                    fallbacks: Some(FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                };

                // Calculate terminal dimensions exactly like Zed does
                let font_id = cx.text_system().resolve_font(&font);
                let rem_size = window.rem_size();
                let font_pixels = text_style.font_size.to_pixels(rem_size);
                
                // Use lowercase 'm' for cell width (proper monospace calculation like Zed)
                let cell_width = cx
                    .text_system()
                    .advance(font_id, font_pixels, 'm')
                    .unwrap()
                    .width;
                    
                // Line height - use font_pixels * 1.3 for better spacing
                let line_height = font_pixels * 1.3;

                let dimensions = TerminalBounds::new(line_height, cell_width, bounds);

                // Update terminal size and sync content (release borrow immediately)
                // IMPORTANT: Check if size actually changed before updating to avoid reentrancy
                {
                    let needs_resize = self.terminal.read(cx).active_session()
                        .map(|s| s.last_content.terminal_bounds != dimensions)
                        .unwrap_or(false);

                    if needs_resize {
                        // Only update if size actually changed
                        self.terminal.update(cx, |terminal, cx| {
                            if let Some(session) = terminal.active_session_mut() {
                                session.set_size(dimensions);
                                // Sync to process any new events and update content
                                session.sync(window, cx);
                            }
                        });
                    }
                }

                // Get terminal content - read only in prepaint (Zed approach)
                // Need to get theme again for layout_grid
                let theme = cx.theme();
                let (rects, batched_text_runs) = {
                    let terminal_read = self.terminal.read(cx);
                    if let Some(session) = terminal_read.active_session() {
                        // Use cached last_content (like Zed)
                        layout_grid(
                            session.last_content.cells.iter().cloned(),
                            session.last_content.display_offset,
                            &text_style,
                            &font,
                            &theme,
                        )
                    } else {
                        (Vec::new(), Vec::new())
                    }
                };

                // Layout cursor (from Zed) - Rectangle is used for IME, so we should lay it out even if we don't end up showing it
                let cursor = {
                    let terminal_read = self.terminal.read(cx);
                    if let Some(session) = terminal_read.active_session() {
                        let cursor = &session.last_content.cursor;
                        let cursor_char = session.last_content.cursor_char;
                        let display_offset = session.last_content.display_offset;
                        
                        if let AlacCursorShape::Hidden = cursor.shape {
                            None
                        } else {
                            let cursor_point = DisplayCursor::from(cursor.point, display_offset);
                            let cursor_text = {
                                let str_text = cursor_char.to_string();
                                let len = str_text.len();
                                window.text_system().shape_line(
                                    str_text.into(),
                                    text_style.font_size.to_pixels(window.rem_size()),
                                    &[TextRun {
                                        len,
                                        font: text_style.font(),
                                        color: background_color,
                                        background_color: None,
                                        underline: Default::default(),
                                        strikethrough: None,
                                    }],
                                    None,
                                )
                            };

                            let focused = self.focused;
                            Self::shape_cursor(cursor_point, &dimensions, &cursor_text).map(
                                move |(cursor_position, block_width)| {
                                    let (shape, text) = match cursor.shape {
                                        AlacCursorShape::Block if !focused => (CursorShape::Hollow, None),
                                        AlacCursorShape::Block => (CursorShape::Block, Some(cursor_text)),
                                        AlacCursorShape::Underline => (CursorShape::Underscore, None),
                                        AlacCursorShape::Beam => (CursorShape::Bar, None),
                                        AlacCursorShape::HollowBlock => (CursorShape::Hollow, None),
                                        AlacCursorShape::Hidden => unreachable!(),
                                    };

                                    CursorLayout::new(
                                        cursor_position,
                                        block_width,
                                        dimensions.line_height,
                                        foreground_color,  // Use theme foreground as cursor color
                                        shape,
                                        text,
                                    )
                                },
                            )
                        }
                    } else {
                        None
                    }
                };

                LayoutState {
                    hitbox,
                    batched_text_runs,
                    rects,
                    cursor,
                    background_color,
                    dimensions,
                    text_style,
                }
            },
        )
    }

    // Adapted from Zed's paint method
    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.register_mouse_listeners(&layout.hitbox, window);
        
        // Set cursor style like Zed does
        if self.focused {
            window.set_cursor_style(CursorStyle::IBeam, &layout.hitbox);
        }

        // Paint using interactivity (Zed's approach)
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            Some(&layout.hitbox),
            window,
            cx,
            |_, window, cx| {
                // Register input handler - THIS IS KEY!
                let input_handler = TerminalInputHandler {
                    terminal: self.terminal.clone(),
                };
                window.handle_input(&self.focus, input_handler, cx);

                // Register key handler for special keys (from Zed)
                window.on_key_event({
                    let terminal = self.terminal.clone();
                    move |event: &KeyDownEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        
                        // Try to handle as keystroke (special keys)
                        terminal.update(cx, |term, cx| {
                            term.try_keystroke(&event.keystroke, false, cx);
                        });
                    }
                });

                // Paint background
                window.paint_quad(fill(bounds, layout.background_color));

                let origin = bounds.origin;

                // Paint background rectangles (Zed does this)
                for rect in &layout.rects {
                    rect.paint(origin, &layout.dimensions, window);
                }

                // Paint batched text runs (Zed's optimization)
                for batch in &layout.batched_text_runs {
                    batch.paint(origin, &layout.dimensions, window, cx);
                }

                // Paint cursor (from Zed)
                if self.cursor_visible {
                    if let Some(mut cursor) = layout.cursor.take() {
                        cursor.paint(origin, window, cx);
                    }
                }
            },
        );
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

