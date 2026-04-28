use super::*;

#[derive(IntoElement)]
pub struct ColorPicker {
    id: ElementId,
    style: StyleRefinement,
    state: Entity<ColorPickerState>,
    featured_colors: Option<Vec<Hsla>>,
    label: Option<SharedString>,
    icon: Option<Icon>,
    size: Size,
    anchor: Corner,
}

impl ColorPicker {
    pub fn new(state: &Entity<ColorPickerState>) -> Self {
        Self {
            id: ("color-picker", state.entity_id()).into(),
            style: StyleRefinement::default(),
            state: state.clone(),
            featured_colors: None,
            size: Size::Medium,
            label: None,
            icon: None,
            anchor: Corner::TopLeft,
        }
    }

    /// Set the featured colors to be displayed in the color picker.
    ///
    /// This is used to display a set of colors that the user can quickly select from,
    /// for example provided user's last used colors.
    pub fn featured_colors(mut self, colors: Vec<Hsla>) -> Self {
        self.featured_colors = Some(colors);
        self
    }

    /// Set the size of the color picker, default is `Size::Medium`.
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Set the icon to the color picker button.
    ///
    /// If this is set the color picker button will display the icon.
    /// Else it will display the square color of the current value.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set the label to be displayed above the color picker.
    ///
    /// Default is `None`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the anchor corner of the color picker.
    ///
    /// Default is `Corner::TopLeft`.
    pub fn anchor(mut self, anchor: Corner) -> Self {
        self.anchor = anchor;
        self
    }

    fn render_item(
        &self,
        color: Hsla,
        clickable: bool,
        window: &mut Window,
        _: &mut App,
    ) -> impl IntoElement {
        render_color_swatch("color", color, clickable, self.state.clone(), window)
    }

    fn render_palette_switcher_popout(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (selected_palette_index, palette_switcher_open, palette_header_bounds) = {
            let state = self.state.read(cx);
            (state.selected_palette_index, state.palette_switcher_open, state.palette_header_bounds)
        };
        let named_palettes = named_color_palettes();
        let safe_palette_index = selected_palette_index.min(named_palettes.len().saturating_sub(1));

        div()
            .when(palette_switcher_open, |this| {
                this.child(
                    deferred(
                        anchored()
                            .position(palette_header_bounds.corner(Corner::BottomLeft))
                            .snap_to_window_with_margin(px(8.))
                            .child(
                                div()
                                    .occlude()
                                    .mt_1p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .shadow_lg()
                                    .bg(cx.theme().background)
                                    .w(px(300.0))
                                    .child(
                                        v_flex()
                                            .max_h(px(300.0))
                                            .scrollable(Axis::Vertical)
                                            .child(
                                                v_flex().gap_px().children(
                                                    named_palettes
                                                        .iter()
                                                        .enumerate()
                                                        .map(|(ix, (name, colors))| {
                                                            let swatches = colors.iter().copied().take(9).collect::<Vec<_>>();
                                                            h_flex()
                                                                .w_full()
                                                                .items_center()
                                                                .justify_between()
                                                                .gap_2()
                                                                .px_3()
                                                                .py_2()
                                                                .when(ix == safe_palette_index, |this| {
                                                                    this.bg(cx.theme().accent.opacity(0.16))
                                                                })
                                                                .hover(|this| this.bg(cx.theme().muted.opacity(0.45)))
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_semibold()
                                                                        .text_color(cx.theme().foreground)
                                                                        .child((*name).to_string()),
                                                                )
                                                                .child(
                                                                    h_flex().gap_1().children(swatches.into_iter().map(|color| {
                                                                        div()
                                                                            .h_4()
                                                                            .w_4()
                                                                            .bg(color)
                                                                            .border_1()
                                                                            .border_color(color.darken(0.2))
                                                                    })),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    window.listener_for(
                                                                        &self.state,
                                                                        move |state, _, window, cx| {
                                                                            state.selected_palette_index = ix;
                                                                            state.palette_switcher_open = false;
                                                                            cx.notify();
                                                                        },
                                                                    ),
                                                                )
                                                        }),
                                                ),
                                            ),
                                    )
                                    .on_mouse_down_out(
                                        window.listener_for(&self.state, |state, _, _window, cx| {
                                            state.palette_switcher_open = false;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    )
                    .with_priority(2),
                )
            })
    }

    fn render_all_colors_grid(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Each named palette is one row, sorted dark → light (value ascending).
        let color_rows = named_color_palettes()
            .into_iter()
            .map(|(_, mut palette_colors)| {
                palette_colors.sort_by(|a, b| {
                    let (_, _, v_a, _) = hsla_to_hsva(*a);
                    let (_, _, v_b, _) = hsla_to_hsva(*b);
                    v_a.partial_cmp(&v_b).unwrap_or(std::cmp::Ordering::Equal)
                });
                let row_colors: Vec<Hsla> = palette_colors.into_iter().take(ALL_COLORS_COLS).collect();
                h_flex().gap_1().children(row_colors.into_iter().map(|color| {
                    render_color_swatch("all-color", color, true, self.state.clone(), window)
                }))
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_px()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .pb_1()
                    .text_color(cx.theme().muted_foreground)
                    .child("All Colors"),
            )
            .children(color_rows)
    }

    fn render_rgba_slider(
        &self,
        channel: usize,
        label: &'static str,
        value_255: u8,
        alpha_value: f32,
        rgba: gpui::Rgba,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let numeric_input_state = {
            let picker = state.read(cx);
            picker.rgba_input_states[channel].clone()
        };
        let value_01 = value_255 as f32 / 255.0;

        h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(18.0))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .relative()
                    .h(px(SLIDER_HEIGHT))
                    .flex_1()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .child(
                        canvas(
                            {
                                let state = state.clone();
                                move |bounds, _, cx| {
                                    state.update(cx, |picker, _| picker.slider_bounds[channel] = bounds);
                                    bounds
                                }
                            },
                            move |bounds, _, window, _| {
                                paint_slider_gradient(window, bounds, channel, rgba, value_01);
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        window.listener_for(&state, move |picker, event, window, cx| {
                            picker.active_drag = Some(match channel {
                                0 => PickerDragTarget::R,
                                1 => PickerDragTarget::G,
                                2 => PickerDragTarget::B,
                                _ => PickerDragTarget::A,
                            });
                            picker.start_drag(event, window, cx);
                        }),
                    )
                    .on_mouse_move(window.listener_for(&state, move |picker, event: &MouseMoveEvent, window, cx| {
                        if picker.active_drag.is_some() {
                            picker.drag_move(event.position, window, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, window.listener_for(&state, ColorPickerState::stop_drag_mouse))
                    .on_mouse_up_out(MouseButton::Left, window.listener_for(&state, ColorPickerState::stop_drag_mouse)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        TextInput::new(&numeric_input_state)
                            .xsmall()
                            .w(px(52.0))
                            .font_family("JetBrainsMono-Regular")
                            .text_xs(),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                            .child(if channel == 3 { "0-1" } else { "0-255" }),
                    ),
            )
    }

    fn render_relation_row(
        &self,
        title: &'static str,
        colors: Vec<Hsla>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        render_relation_row_component(title, colors, self.state.clone(), window, cx)
    }

    fn render_advanced_picker(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (current, hue_value, sat_value, val_value, alpha_value, recent_colors, selected_palette_index, palette_switcher_open, code_input_state) = {
            let state = self.state.read(cx);
            (
                state
                    .value
                    .unwrap_or_else(|| hsva_to_hsla(state.hue, state.saturation, state.value_channel, state.alpha)),
                state.hue,
                state.saturation,
                state.value_channel,
                state.alpha,
                state.recent_colors.clone(),
                state.selected_palette_index,
                state.palette_switcher_open,
                state.state.clone(),
            )
        };
        let named_palettes = named_color_palettes();
        let safe_palette_index = selected_palette_index.min(named_palettes.len().saturating_sub(1));
        let (selected_palette_name, selected_palette_colors) = named_palettes
            .get(safe_palette_index)
            .cloned()
            .unwrap_or(("Palette", Vec::new()));

        let rgba: gpui::Rgba = current.into();
        let r_u8 = (rgba.r * 255.0).round() as u8;
        let g_u8 = (rgba.g * 255.0).round() as u8;
        let b_u8 = (rgba.b * 255.0).round() as u8;
        let a_u8 = (rgba.a * 255.0).round() as u8;

        let complementary = hsva_to_hsla(
            (hue_value + 0.5).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let triad_a = hsva_to_hsla(
            (hue_value + (1.0 / 3.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let triad_b = hsva_to_hsla(
            (hue_value + (2.0 / 3.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );

        let state_entity = self.state.clone();
        let hue = hue_value;
        let sat = sat_value;
        let val = val_value;

        let analogous_l = hsva_to_hsla(
            (hue_value - (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let analogous_r = hsva_to_hsla(
            (hue_value + (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let split_l = hsva_to_hsla(
            (hue_value + 0.5 - (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let split_r = hsva_to_hsla(
            (hue_value + 0.5 + (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .size(px(PICKER_SIZE))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.65))
                            .child(
                                canvas(
                                    {
                                        let state = state_entity.clone();
                                        move |bounds, _, cx| {
                                            state.update(cx, |picker, _| picker.picker_bounds = bounds);
                                            bounds
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        let Some(geometry) = picker_geometry(bounds) else {
                                            return;
                                        };

                                        paint_hue_wheel(window, geometry);
                                        paint_sv_triangle(window, geometry, hue);

                                        let ring_angle =
                                            hue * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                                        let ring_radius = (geometry.outer_r + geometry.inner_r) * 0.5;
                                        let ring_x = geometry.cx + ring_angle.cos() * ring_radius;
                                        let ring_y = geometry.cy + ring_angle.sin() * ring_radius;

                                        let ring_marker = Bounds {
                                            origin: point(px(ring_x - 4.0), px(ring_y - 4.0)),
                                            size: size(px(8.0), px(8.0)),
                                        };
                                        window.paint_quad(fill(ring_marker, gpui::white()));

                                        let [a, b, c] = triangle_vertices(geometry, hue);
                                        let w_h = sat * val;
                                        let w_w = (1.0 - sat) * val;
                                        let w_b = 1.0 - val;

                                        let tri_x = w_h * a.0 + w_w * b.0 + w_b * c.0;
                                        let tri_y = w_h * a.1 + w_w * b.1 + w_b * c.1;

                                        let tri_marker = Bounds {
                                            origin: point(px(tri_x - 5.0), px(tri_y - 5.0)),
                                            size: size(px(10.0), px(10.0)),
                                        };
                                        window.paint_quad(fill(tri_marker, gpui::black().opacity(0.65)));
                                        let inner = Bounds {
                                            origin: point(px(tri_x - 3.0), px(tri_y - 3.0)),
                                            size: size(px(6.0), px(6.0)),
                                        };
                                        window.paint_quad(fill(inner, gpui::white()));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                window.listener_for(&state_entity, |picker, event, window, cx| {
                                    picker.start_drag(event, window, cx);
                                }),
                            )
                            .on_mouse_move(window.listener_for(&state_entity, move |picker, event: &MouseMoveEvent, window, cx| {
                                if picker.active_drag.is_some() {
                                    picker.drag_move(event.position, window, cx);
                                }
                            }))
                            .on_mouse_up(MouseButton::Left, window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse))
                            .on_mouse_up_out(MouseButton::Left, window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .h(px(52.0))
                                    .rounded_md()
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(current.darken(0.35))
                                    .child(
                                        canvas(
                                            |bounds, _, _| bounds,
                                            |bounds, _, window, _| {
                                                paint_alpha_checkerboard(window, bounds);
                                            },
                                        )
                                        .size_full()
                                        .absolute()
                                        .inset_0(),
                                    )
                                    .child(div().absolute().inset_0().bg(current)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.55))
                                    .bg(cx.theme().muted.opacity(0.25))
                                    .child(
                                        v_flex()
                                            .gap_px()
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("HEX  {}", current.to_hex()))
                                            .child(format!("RGBA {}, {}, {}, {}", r_u8, g_u8, b_u8, a_u8)),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(34.0))
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Code"),
                                            )
                                            .child(
                                                TextInput::new(&code_input_state)
                                                    .xsmall()
                                                    .w_full()
                                                    .cleanable()
                                                    .font_family("JetBrainsMono-Regular"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                                            .child("HEX, RGB(A), HSL(A)"),
                                    ),
                            )
                            .child(self.render_rgba_slider(0, "R", r_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(1, "G", g_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(2, "B", b_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(3, "A", a_u8, alpha_value, rgba, window, cx)),
                    )
                    .on_mouse_move(window.listener_for(&state_entity, move |picker, event: &MouseMoveEvent, window, cx| {
                        if picker.active_drag.is_some() {
                            picker.drag_move(event.position, window, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Color Relations"),
                    )
                    .child(self.render_relation_row("Complementary", vec![current, complementary], window, cx))
                    .child(self.render_relation_row("Analogous", vec![analogous_l, current, analogous_r], window, cx))
                    .child(self.render_relation_row("Split-Comp", vec![current, split_l, split_r], window, cx))
                    .child(self.render_relation_row("Triadic", vec![current, triad_a, triad_b], window, cx)),
            )
            .when(!recent_colors.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Recent"),
                        )
                        .child(
                            h_flex().gap_1().children(
                                recent_colors
                                    .iter()
                                    .copied()
                                    .map(|color| self.render_item(color, true, window, cx)),
                            ),
                        ),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .relative()
                            .child(
                                canvas(
                                    {
                                        let state = self.state.clone();
                                        move |bounds, _, cx| state.update(cx, |r, _| r.palette_header_bounds = bounds)
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Palette: {}", selected_palette_name)),
                            )
                            .child(
                                Button::new("palette-switcher")
                                    .ghost()
                                    .xsmall()
                                    .icon(if palette_switcher_open {
                                        Icon::new(IconName::ChevronUp)
                                    } else {
                                        Icon::new(IconName::ChevronDown)
                                    })
                                    .on_click(window.listener_for(
                                        &self.state,
                                        ColorPickerState::toggle_palette_switcher,
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap_1()
                            .children(
                                selected_palette_colors
                                    .iter()
                                    .copied()
                                    .map(|color| self.render_item(color, true, window, cx)),
                            ),
                    )
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Divider::horizontal())
                    .child(self.render_all_colors_grid(window, cx)),
            )
    }

    fn resolved_corner(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        bounds.corner(match self.anchor {
            Corner::TopLeft => Corner::BottomLeft,
            Corner::TopRight => Corner::BottomRight,
            Corner::BottomLeft => Corner::TopLeft,
            Corner::BottomRight => Corner::TopRight,
        })
    }
}

impl Sizable for ColorPicker {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Focusable for ColorPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle.clone()
    }
}

impl Styled for ColorPicker {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ColorPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (bounds, current_value, is_open, is_dragging, is_focused, focus_handle, palette_switcher_open) = {
            let state = self.state.read(cx);
            (
                state.bounds,
                state.value,
                state.open,
                state.active_drag.is_some(),
                state.focus_handle.is_focused(window),
                state.focus_handle.clone().tab_stop(true),
                state.palette_switcher_open,
            )
        };
        let display_title: SharedString = if let Some(value) = current_value {
            value.to_hex()
        } else {
            "".to_string()
        }
        .into();

        div()
            .id(self.id.clone())
            .key_context(CONTEXT)
            .track_focus(&focus_handle)
            .on_action(window.listener_for(&self.state, ColorPickerState::on_escape))
            .on_action(window.listener_for(&self.state, ColorPickerState::on_confirm))
            .child(
                h_flex()
                    .id("color-picker-input")
                    .gap_2()
                    .items_center()
                    .input_text_size(self.size)
                    .line_height(relative(1.))
                    .refine_style(&self.style)
                    .when_some(self.icon.clone(), |this, icon| {
                        this.child(
                            Button::new("btn")
                                .track_focus(&focus_handle)
                                .ghost()
                                .selected(is_open)
                                .with_size(self.size)
                                .icon(icon.clone()),
                        )
                    })
                    .when_none(&self.icon, |this| {
                        this.child(
                            div()
                                .id("color-picker-square")
                                .bg(cx.theme().background)
                                .border_1()
                                .m_1()
                                .border_color(cx.theme().input)
                                .rounded(cx.theme().radius)
                                .shadow_xs()
                                .rounded(cx.theme().radius)
                                .overflow_hidden()
                                .size_with(self.size)
                                .when_some(current_value, |this, value| {
                                    this.bg(value)
                                        .border_color(value.darken(0.3))
                                        .when(is_open, |this| this.border_2())
                                })
                                .when(!display_title.is_empty(), |this| {
                                    this.tooltip(move |_, cx| {
                                        cx.new(|_| Tooltip::new(display_title.clone())).into()
                                    })
                                }),
                        )
                        .focus_ring(is_focused, px(0.), window, cx)
                    })
                    .when_some(self.label.clone(), |this, label| this.child(label))
                    .on_click(window.listener_for(&self.state, ColorPickerState::toggle_picker))
                    .child(
                        canvas(
                            {
                                let state = self.state.clone();
                                move |bounds, _, cx| state.update(cx, |r, _| r.bounds = bounds)
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(is_open, |this| {
                this.child(
                    deferred(
                        anchored()
                            .anchor(self.anchor)
                            .snap_to_window_with_margin(px(8.))
                            .position(self.resolved_corner(bounds))
                            .child(
                                div()
                                    .occlude()
                                    .map(|this| match self.anchor {
                                        Corner::TopLeft | Corner::TopRight => this.mt_1p5(),
                                        Corner::BottomLeft | Corner::BottomRight => this.mb_1p5(),
                                    })
                                    .w(px(480.0))
                                    .rounded(cx.theme().radius)
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .shadow_lg()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().background)
                                    .relative()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .gap_3()
                                            .child(self.render_advanced_picker(window, cx))
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        window.listener_for(&self.state, |state, _, window, cx| {
                                            if state.active_drag.is_some() {
                                                state.active_drag = None;
                                                cx.notify();
                                            } else {
                                                state.on_escape(&Cancel, window, cx);
                                            }
                                        }),
                                    ),
                            ),
                    )
                    .with_priority(1),
                )
                .when(palette_switcher_open, |this| {
                    this.child(self.render_palette_switcher_popout(window, cx))
                })
                .when(is_dragging, |this| {
                    this.child(
                        deferred(
                            anchored().snap_to_window_with_margin(px(0.)).child(
                                div()
                                    .size_full()
                                    .on_mouse_move(window.listener_for(
                                        &self.state,
                                        |picker, event: &MouseMoveEvent, window, cx| {
                                            if picker.active_drag.is_some() {
                                                picker.drag_move(event.position, window, cx);
                                            }
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        window.listener_for(
                                            &self.state,
                                            ColorPickerState::stop_drag_mouse,
                                        ),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        window.listener_for(
                                            &self.state,
                                            ColorPickerState::stop_drag_mouse,
                                        ),
                                    ),
                            ),
                        )
                        .with_priority(3),
                    )
                })
            })
    }
}
