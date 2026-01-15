use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::Button,
    switch::Switch,
    label::Label,
};

/// A beautiful setting card component that matches the launcher quality
pub struct SettingCard {
    title: String,
    description: Option<String>,
    icon: Option<IconName>,
    children: Vec<AnyElement>,
}

impl SettingCard {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: None,
            icon: None,
            children: Vec::new(),
        }
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn render(self, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .w_full()
            .gap_5()
            .p_6()
            .bg(theme.sidebar)
            .border_1()
            .border_color(theme.border)
            .rounded_xl()
            .shadow_sm()
            .child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .when_some(self.icon, |this, icon| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .w(px(48.0))
                                .h(px(48.0))
                                .rounded_lg()
                                .bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                                .border_1()
                                .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.3))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(icon)
                                        .size(px(24.0))
                                        .text_color(theme.primary)
                                )
                        )
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child(self.title)
                            )
                            .when_some(self.description, |this, desc| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .line_height(rems(1.5))
                                        .text_color(theme.muted_foreground)
                                        .child(desc)
                                )
                            })
                    )
            )
            .children(self.children)
    }
}

/// A setting row with label, description, and control
pub struct SettingRow {
    label: String,
    description: Option<String>,
    control: Option<AnyElement>,
}

impl SettingRow {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            control: None,
        }
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    pub fn render(self, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_6()
            .child(
                v_flex()
                    .flex_1()
                    .gap_1p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(self.label)
                    )
                    .when_some(self.description, |this, desc| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(desc)
                        )
                    })
            )
            .when_some(self.control, |this, control| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .child(control)
                )
            })
    }
}

/// Value display component
pub fn render_value_display(value: impl Into<String>, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();

    div()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_sm()
                .font_family("monospace")
                .text_color(theme.foreground)
                .child(value.into())
        )
}

/// Section header component
pub fn render_section_header(title: impl Into<String>, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();

    v_flex()
        .w_full()
        .gap_2()
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child(title.into())
        )
        .child(
            div()
                .w_full()
                .h(px(1.0))
                .bg(theme.border)
        )
}
