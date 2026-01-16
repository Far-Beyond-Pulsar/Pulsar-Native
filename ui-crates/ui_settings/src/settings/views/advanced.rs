use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    switch::Switch,
};
use crate::settings::SettingsScreen;
use super::components::*;

impl SettingsScreen {
    pub fn render_advanced_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_4()
            .child(render_section_header("Advanced", cx))
            .child(self.render_performance_card(cx))
            .child(self.render_debugging_card(cx))
            .child(self.render_extensions_card(cx))
    }

    fn render_performance_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let accent = cx.theme().accent;
        let foreground = cx.theme().foreground;
        let fps_options = vec![30u32, 60, 120, 144, 240, 0];
        let current_fps = self.settings.advanced.max_viewport_fps;
        let performance_level = self.settings.advanced.performance_level;

        SettingCard::new("Performance Settings")
            .icon(IconName::Rocket)
            .description("Configure performance and rendering options")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border_color)
                    .child(
                        SettingRow::new("Performance Level")
                            .description("Higher levels use more resources but may improve performance")
                            .control(
                                render_value_display(
                                    format!("Level {}", performance_level),
                                    cx
                                )
                            )
                            .render(cx)
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .bg(border_color)
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(foreground)
                                            .child("Viewport Max FPS (Frame Pacing)")
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(foreground.opacity(0.7))
                                            .child("Controls viewport refresh rate for consistent frame pacing")
                                    )
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .flex_wrap()
                                    .children(fps_options.iter().map(|&fps| {
                                        let label = if fps == 0 { "Unlimited".to_string() } else { format!("{}", fps) };
                                        let is_selected = current_fps == fps;

                                        let mut btn = Button::new(SharedString::from(format!("fps-{}", fps)))
                                            .label(label);

                                        if is_selected {
                                            btn = btn.primary();
                                        } else {
                                            btn = btn.ghost();
                                        }

                                        btn.on_click(cx.listener(move |screen, _, _window, cx| {
                                            screen.settings.advanced.max_viewport_fps = fps;
                                            screen.settings.save(&screen.config_path);
                                            cx.notify();
                                        }))
                                    }))
                            )
                            .child(
                                div()
                                    .p_4()
                                    .rounded_lg()
                                    .bg(hsla(accent.h, accent.s, accent.l, 0.1))
                                    .border_1()
                                    .border_color(hsla(accent.h, accent.s, accent.l, 0.2))
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .items_start()
                                            .child(
                                                Icon::new(IconName::Info)
                                                    .size(px(20.0))
                                                    .text_color(accent)
                                            )
                                            .child(
                                                v_flex()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(foreground)
                                                            .child("Frame Pacing Tips")
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(foreground.opacity(0.9))
                                                            .child("• 60 FPS: Best for most users, balances smoothness and performance")
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(foreground.opacity(0.9))
                                                            .child("• 120/144 FPS: For high refresh rate monitors")
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(foreground.opacity(0.9))
                                                            .child("• Unlimited: Maximum frame rate, may increase GPU usage")
                                                    )
                                            )
                                    )
                            )
                    )
            )
            .render(cx)
    }

    fn render_debugging_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let primary_color = cx.theme().primary;
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let debug_logging = self.settings.advanced.debug_logging;
        let experimental_features = self.settings.advanced.experimental_features;

        SettingCard::new("Debugging & Development")
            .icon(IconName::Bug)
            .description("Developer tools and debugging features")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border_color)
                    .child(
                        SettingRow::new("Debug Logging")
                            .description("Enable detailed logging for troubleshooting")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Switch::new("debug-logging-switch")
                                            .checked(debug_logging)
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.advanced.debug_logging = !screen.settings.advanced.debug_logging;
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if debug_logging {
                                                primary_color
                                            } else {
                                                muted_foreground
                                            })
                                            .child(if debug_logging { "Enabled" } else { "Disabled" })
                                    )
                                    .child(
                                        Button::new("save-debug-logging")
                                            .primary()
                                            .label("Save")
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.save(&screen.config_path);
                                                cx.notify();
                                            }))
                                    )
                            )
                            .render(cx)
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .bg(border_color)
                    )
                    .child(
                        SettingRow::new("Experimental Features")
                            .description("Enable cutting-edge features that may be unstable")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Switch::new("experimental-features-switch")
                                            .checked(experimental_features)
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.advanced.experimental_features = !screen.settings.advanced.experimental_features;
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if experimental_features {
                                                primary_color
                                            } else {
                                                muted_foreground
                                            })
                                            .child(if experimental_features { "Enabled" } else { "Disabled" })
                                    )
                                    .child(
                                        Button::new("save-experimental-features")
                                            .primary()
                                            .label("Save")
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.save(&screen.config_path);
                                                cx.notify();
                                            }))
                                    )
                            )
                            .render(cx)
                    )
                    .when(experimental_features, |this| {
                        let warning_color = hsla(0.05, 0.8, 0.5, 1.0); // Orange warning color
                        this.child(
                            div()
                                .p_4()
                                .rounded_lg()
                                .bg(hsla(0.05, 0.8, 0.5, 0.1))
                                .border_1()
                                .border_color(hsla(0.05, 0.8, 0.5, 0.3))
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_start()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(20.0))
                                                .text_color(warning_color)
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(foreground)
                                                .child("Warning: Experimental features may cause instability or data loss. Use at your own risk.")
                                        )
                                )
                        )
                    })
            )
            .render(cx)
    }

    fn render_extensions_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Extensions & Plugins")
            .icon(IconName::Package)
            .description("Manage installed extensions and plugins")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .p_5()
                            .rounded_lg()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                Icon::new(IconName::Package)
                                                    .size(px(24.0))
                                                    .text_color(theme.primary)
                                            )
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Extension Manager")
                                            )
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Browse, install, and manage extensions to enhance your workflow. Coming in a future update.")
                                    )
                                    .child(
                                        Button::new("browse-extensions")
                                            .ghost()
                                            .label("Browse Extensions")
                                            .icon(IconName::ExternalLink)
                                            .on_click(cx.listener(|_screen, _, _window, cx| {
                                                // TODO: Open extensions marketplace
                                                cx.notify();
                                            }))
                                    )
                            )
                    )
            )
            .render(cx)
    }
}
