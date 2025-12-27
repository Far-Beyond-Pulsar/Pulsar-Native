use gpui::*;
use ui::{
    button::Button,
    h_flex, v_flex,
    ActiveTheme as _, StyledExt, Selectable,
    IconName,
};

pub struct ToolbarButton {
    icon: IconName,
    label: String,
    tooltip: String,
    active: bool,
    onclick: Option<Box<dyn Fn(&mut Window, &mut App) + Send + Sync>>,
}

impl ToolbarButton {
    pub fn new(icon: IconName, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            tooltip: String::new(),
            active: false,
            onclick: None,
        }
    }

    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = tooltip.into();
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn on_click<F>(mut self, onclick: F) -> Self
    where
        F: Fn(&mut Window, &mut App) + Send + Sync + 'static,
    {
        self.onclick = Some(Box::new(onclick));
        self
    }

    pub fn render(self) -> Button {
    let mut button = Button::new("toolbar-btn")
            .label(self.label.clone())
            .icon(self.icon)
            .tooltip(self.tooltip);

        if self.active {
            button = button.selected(true);
        }

        if let Some(onclick) = self.onclick {
            button = button.on_click(move |_, window, cx| {
                onclick(window, cx);
            });
        }

        button
    }
}

pub struct Toolbar {
    buttons: Vec<ToolbarButton>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            buttons: Vec::new(),
        }
    }

    pub fn add_button(mut self, button: ToolbarButton) -> Self {
        self.buttons.push(button);
        self
    }

    pub fn add_separator(self) -> Self {
        self
    }

    pub fn render(self, cx: &mut App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .p_2()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .children(
                self.buttons
                    .into_iter()
                    .map(|button| button.render().into_any_element())
            )
    }
}

pub struct StatusBar {
    left_items: Vec<String>,
    right_items: Vec<String>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            left_items: Vec::new(),
            right_items: Vec::new(),
        }
    }

    pub fn add_left_item(mut self, item: impl Into<String>) -> Self {
        self.left_items.push(item.into());
        self
    }

    pub fn add_right_item(mut self, item: impl Into<String>) -> Self {
        self.right_items.push(item.into());
        self
    }

    pub fn render(self, cx: &mut App) -> impl IntoElement {
        h_flex()
            .w_full()
            .h_8()
            .bg(cx.theme().background)
            .border_t_1()
            .border_color(cx.theme().border)
            .px_4()
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_4()
                    .children(
                        self.left_items
                            .into_iter()
                            .map(|item| {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(item)
                                    .into_any_element()
                            })
                    )
            )
            .child(
                h_flex()
                    .gap_4()
                    .children(
                        self.right_items
                            .into_iter()
                            .map(|item| {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(item)
                                    .into_any_element()
                            })
                    )
            )
    }
}

pub struct ViewportControls {
    show_grid: bool,
    show_axes: bool,
    perspective_mode: bool,
}

impl ViewportControls {
    pub fn new() -> Self {
        Self {
            show_grid: true,
            show_axes: true,
            perspective_mode: true,
        }
    }

    pub fn render(&self, cx: &mut App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .p_2()
            .w(px(200.0)) // Hardcoded width to prevent inheritance issues
            .bg(cx.theme().background.opacity(0.9))
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("grid")
                            .icon(IconName::LayoutDashboard)
                            .tooltip("Toggle Grid")
                            .selected(self.show_grid)
                    )
                    .child(
                        Button::new("axes")
                            .icon(IconName::Axes)
                            .tooltip("Toggle Axes")
                            .selected(self.show_axes)
                    )
                    .child(
                        Button::new("perspective")
                            .icon(IconName::Cube)
                            .tooltip("Toggle Perspective")
                            .selected(self.perspective_mode)
                    )
            )
    }
}

pub struct PropertyField {
    label: String,
    value: String,
    readonly: bool,
}

impl PropertyField {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            readonly: false,
        }
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn render(self, cx: &mut App) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().foreground)
                    .child(self.label)
            )
            .child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(cx.theme().input)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .text_sm()
                    .text_color(if self.readonly { cx.theme().muted_foreground } else { cx.theme().foreground })
                    .child(self.value)
            )
    }
}
