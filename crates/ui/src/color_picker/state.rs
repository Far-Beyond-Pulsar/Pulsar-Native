use super::*;

/// State of the [`ColorPicker`].
pub struct ColorPickerState {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) value: Option<Hsla>,
    pub(crate) hovered_color: Option<Hsla>,
    pub(crate) state: Entity<InputState>,
    pub(crate) syncing_inputs: bool,
    pub(crate) open: bool,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) picker_bounds: Bounds<Pixels>,
    pub(crate) slider_bounds: [Bounds<Pixels>; 4],
    pub(crate) active_drag: Option<PickerDragTarget>,
    pub(crate) triangle_drag_hue_lock: Option<f32>,
    pub(crate) selected_palette_index: usize,
    pub(crate) palette_switcher_open: bool,
    pub(crate) palette_header_bounds: Bounds<Pixels>,
    pub(crate) rgba_input_states: [Entity<InputState>; 4],
    pub(crate) hue: f32,
    pub(crate) saturation: f32,
    pub(crate) value_channel: f32,
    pub(crate) alpha: f32,
    pub(crate) recent_colors: Vec<Hsla>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl ColorPickerState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("#RRGGBB, rgba(...), hsl(...)")
        });
        let rgba_input_states = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));

        let mut _subscriptions = vec![cx.subscribe_in(
            &state,
            window,
            |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    if this.syncing_inputs {
                        return;
                    }
                    let value = state.read(cx).value();
                    if let Some(color) = parse_color_code(value.as_str()) {
                        this.apply_external_color(color, true, window, cx);
                    }
                }
                InputEvent::PressEnter { .. } => {
                    let val = this.state.read(cx).value();
                    if let Some(color) = parse_color_code(&val) {
                        this.open = false;
                        this.apply_external_color(color, true, window, cx);
                    }
                }
                _ => {}
            },
        )];

        for channel in 0..4 {
            let input_state = rgba_input_states[channel].clone();
            _subscriptions.push(cx.subscribe_in(
                &input_state,
                window,
                move |this, _state, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => this.apply_numeric_input(channel, false, window, cx),
                    InputEvent::PressEnter { .. } => {
                        this.apply_numeric_input(channel, true, window, cx)
                    }
                    _ => {}
                },
            ));
        }

        Self {
            focus_handle: cx.focus_handle(),
            value: None,
            hovered_color: None,
            state,
            syncing_inputs: false,
            open: false,
            bounds: Bounds::default(),
            picker_bounds: Bounds::default(),
            slider_bounds: [
                Bounds::default(),
                Bounds::default(),
                Bounds::default(),
                Bounds::default(),
            ],
            active_drag: None,
            triangle_drag_hue_lock: None,
            selected_palette_index: 0,
            palette_switcher_open: false,
            palette_header_bounds: Bounds::default(),
            rgba_input_states,
            hue: 0.0,
            saturation: 0.0,
            value_channel: 1.0,
            alpha: 1.0,
            recent_colors: Vec::new(),
            _subscriptions,
        }
    }

    /// Set default color value.
    pub fn default_value(mut self, value: Hsla) -> Self {
        self.value = Some(value);
        self.sync_hsva_from_color(value);
        self
    }

    /// Set current color value.
    pub fn set_value(&mut self, value: Hsla, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_external_color(value, false, window, cx);
    }

    /// Get current color value.
    pub fn value(&self) -> Option<Hsla> {
        self.value
    }

    pub(crate) fn on_escape(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            cx.propagate();
        }

        self.open = false;
        self.palette_switcher_open = false;
        cx.notify();
    }

    pub(crate) fn on_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.palette_switcher_open = false;
        }
        cx.notify();
    }

    pub(crate) fn toggle_picker(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.palette_switcher_open = false;
        }
        cx.notify();
    }

    pub(crate) fn toggle_palette_switcher(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_switcher_open = !self.palette_switcher_open;
        cx.notify();
    }

    fn select_palette(
        &mut self,
        palette_index: usize,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_palette_index = palette_index;
        self.palette_switcher_open = false;
        cx.notify();
    }

    fn sync_hsva_from_color(&mut self, color: Hsla) {
        let (h, s, v, a) = hsla_to_hsva(color);
        self.hue = h;
        self.saturation = s;
        self.value_channel = v;
        self.alpha = a;
    }

    fn push_recent_color(&mut self, color: Hsla) {
        let hex = color.to_hex();
        self.recent_colors.retain(|c| c.to_hex() != hex);
        self.recent_colors.insert(0, color);
        self.recent_colors.truncate(12);
    }

    fn drag_target_for_point(&self, position: Point<Pixels>) -> Option<PickerDragTarget> {
        if let Some(geometry) = picker_geometry(self.picker_bounds) {
            if self.picker_bounds.contains(&position) {
                let dx = position.x.as_f32() - geometry.cx;
                let dy = position.y.as_f32() - geometry.cy;
                let distance = (dx * dx + dy * dy).sqrt();

                if distance >= geometry.inner_r && distance <= geometry.outer_r {
                    return Some(PickerDragTarget::HueRing);
                }

                // The entire inner disc is the SV/triangle zone — no gap between
                // the triangle vertex hull and the ring's inner edge.
                if distance < geometry.inner_r {
                    return Some(PickerDragTarget::Triangle);
                }
            }
        }

        for (index, bounds) in self.slider_bounds.iter().enumerate() {
            if bounds.contains(&position) {
                return Some(match index {
                    0 => PickerDragTarget::R,
                    1 => PickerDragTarget::G,
                    2 => PickerDragTarget::B,
                    _ => PickerDragTarget::A,
                });
            }
        }

        None
    }

    fn apply_picker_point(
        &mut self,
        target: PickerDragTarget,
        position: Point<Pixels>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(target, PickerDragTarget::HueRing | PickerDragTarget::Triangle) {
            return;
        }

        let Some(geometry) = picker_geometry(self.picker_bounds) else {
            return;
        };

        let x = position.x.as_f32();
        let y = position.y.as_f32();

        let dx = x - geometry.cx;
        let dy = y - geometry.cy;
        let distance = (dx * dx + dy * dy).sqrt();

        match target {
            PickerDragTarget::HueRing => {
                // No distance guard here — during drag we only need the angle.
                let angle = dy.atan2(dx);
                self.hue = ((angle + std::f32::consts::FRAC_PI_2) / std::f32::consts::TAU)
                    .rem_euclid(1.0);
                let color = hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha);
                self.update_value(Some(color), emit, window, cx);
            }
            PickerDragTarget::Triangle => {
                // No distance guard — clamp_point_to_triangle handles out-of-bounds.
                let drag_hue = self.triangle_drag_hue_lock.unwrap_or(self.hue);
                let [a, b, c] = triangle_vertices(geometry, drag_hue);
                let p = clamp_point_to_triangle((x, y), a, b, c);
                let (w_h, w_w, _w_b) = barycentric(p, a, b, c);

                let v = clamp01(w_h + w_w);
                let s = if v <= 0.0001 { 0.0 } else { clamp01(w_h / v) };

                // Set HSV directly — update_value will not touch these.
                self.hue = drag_hue;
                self.saturation = s;
                self.value_channel = v;
                let color = hsva_to_hsla(drag_hue, s, v, self.alpha);
                self.update_value(Some(color), emit, window, cx);
            }
            _ => {}
        }
    }

    fn apply_slider_point(
        &mut self,
        channel: PickerDragTarget,
        position: Point<Pixels>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slider_index = match channel {
            PickerDragTarget::R => 0,
            PickerDragTarget::G => 1,
            PickerDragTarget::B => 2,
            PickerDragTarget::A => 3,
            PickerDragTarget::HueRing | PickerDragTarget::Triangle => return,
        };

        let bounds = self.slider_bounds[slider_index];
        if bounds.size.width <= px(0.0) {
            return;
        }

        let t = clamp01(
            (position.x.as_f32() - bounds.origin.x.as_f32()) / bounds.size.width.as_f32(),
        );

        let color = self.value.unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let mut rgba: gpui::Rgba = color.into();

        match channel {
            PickerDragTarget::R => rgba.r = t,
            PickerDragTarget::G => rgba.g = t,
            PickerDragTarget::B => rgba.b = t,
            PickerDragTarget::A => rgba.a = t,
            PickerDragTarget::HueRing | PickerDragTarget::Triangle => {}
        }

        let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
        if s > 0.0001 {
            self.hue = h;
        }
        self.saturation = s;
        self.value_channel = v;
        self.alpha = rgba.a;

        self.update_value(Some(rgba.into()), emit, window, cx);
    }

    pub(crate) fn start_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let position = event.position;
        self.active_drag = self.drag_target_for_point(position);
        self.triangle_drag_hue_lock = match self.active_drag {
            Some(PickerDragTarget::Triangle) => Some(self.hue),
            _ => None,
        };
        if let Some(target) = self.active_drag {
            match target {
                PickerDragTarget::HueRing | PickerDragTarget::Triangle => {
                    self.apply_picker_point(target, position, true, window, cx)
                }
                _ => self.apply_slider_point(target, position, true, window, cx),
            }
        }
    }

    pub(crate) fn drag_move(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.active_drag {
            match target {
                PickerDragTarget::HueRing | PickerDragTarget::Triangle => {
                    self.apply_picker_point(target, position, true, window, cx)
                }
                _ => self.apply_slider_point(target, position, true, window, cx),
            }
        }
    }

    pub(crate) fn stop_drag_mouse(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.active_drag = None;
        self.triangle_drag_hue_lock = None;
        cx.notify();
    }

    fn sync_numeric_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let color = self
            .value
            .unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let rgba: gpui::Rgba = color.into();

        let texts = [
            (rgba.r * 255.0).round().clamp(0.0, 255.0).to_string(),
            (rgba.g * 255.0).round().clamp(0.0, 255.0).to_string(),
            (rgba.b * 255.0).round().clamp(0.0, 255.0).to_string(),
            alpha_to_text(rgba.a),
        ];

        for (index, text) in texts.iter().enumerate() {
            self.rgba_input_states[index].update(cx, |input, cx| {
                if input.value() != *text {
                    input.set_value(text, window, cx);
                }
            });
        }
    }

    fn apply_numeric_input(
        &mut self,
        channel: usize,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.syncing_inputs {
            return;
        }

        let raw = self.rgba_input_states[channel].read(cx).value();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }

        let color = self
            .value
            .unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let mut rgba: gpui::Rgba = color.into();

        let parsed_ok = if channel <= 2 {
            match trimmed.parse::<i32>() {
                Ok(v) => {
                    let clamped = v.clamp(0, 255) as f32 / 255.0;
                    match channel {
                        0 => rgba.r = clamped,
                        1 => rgba.g = clamped,
                        _ => rgba.b = clamped,
                    }
                    true
                }
                Err(_) => false,
            }
        } else {
            match trimmed.parse::<f32>() {
                Ok(v) => {
                    rgba.a = clamp01(v);
                    true
                }
                Err(_) => false,
            }
        };

        if parsed_ok {
            let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
            if s > 0.0001 {
                self.hue = h;
            }
            self.saturation = s;
            self.value_channel = v;
            self.alpha = rgba.a;

            self.update_value(Some(rgba.into()), emit, window, cx);
        }
    }

    /// Apply a color that came from outside the picker's own HSV state
    /// (palette swatch, hex field, public set_value). Syncs HSV first, then
    /// records the value. Internal drag/slider code must NOT use this.
    pub(crate) fn apply_external_color(
        &mut self,
        color: Hsla,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_hsva_from_color(color);
        self.update_value(Some(color), emit, window, cx);
    }

    /// Record the final color, sync text inputs, and emit. Never touches
    /// hue/saturation/value_channel/alpha — callers own those fields.
    fn update_value(
        &mut self,
        value: Option<Hsla>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.value = value;
        self.hovered_color = value;
        if let Some(color) = value {
            self.push_recent_color(color);
        }
        self.syncing_inputs = true;
        self.state.update(cx, |view, cx| {
            if let Some(value) = value {
                let hex = value.to_hex();
                if view.value() != hex {
                    view.set_value(hex, window, cx);
                }
            } else {
                if !view.value().is_empty() {
                    view.set_value("", window, cx);
                }
            }
        });
        self.sync_numeric_inputs(window, cx);
        self.syncing_inputs = false;
        if emit {
            cx.emit(ColorPickerEvent::Change(value));
        }
        cx.notify();
    }
}
impl EventEmitter<ColorPickerEvent> for ColorPickerState {}
impl Render for ColorPickerState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.state.clone()
    }
}
impl Focusable for ColorPickerState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
