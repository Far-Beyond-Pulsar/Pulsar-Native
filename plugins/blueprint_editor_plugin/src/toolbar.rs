use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    ActiveTheme as _, StyledExt, Colorize,
    IconName,
};

use super::panel::BlueprintEditorPanel;

pub struct ToolbarRenderer;

impl ToolbarRenderer {
    pub fn render(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        // Professional studio-quality toolbar with visual hierarchy and compilation status
        v_flex()
            .w_full()
            .child(
                // Main toolbar
                h_flex()
                    .w_full()
                    .h(px(60.0)) // Taller for more professional look
                    .px_4()
                    .py_2()
                    .bg(cx.theme().secondary)
                    .border_b_2()
                    .border_color(cx.theme().border)
                    .items_center()
                    .gap_3()
                    // Left section - Primary actions
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                // Blueprint title/breadcrumb
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .px_3()
                                    .py_2()
                                    .rounded(px(4.0))
                                    .bg(cx.theme().background.opacity(0.3))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(
                                                panel.tab_title.clone()
                                                    .unwrap_or_else(|| "Blueprint".to_string())
                                            )
                                    )
                            )
                            .child(Self::render_separator(cx))
                            .child(
                                Button::new("compile")
                                    .icon(IconName::Play)
                                    .label("Compile")
                                    .primary()
                                    .tooltip("Compile Blueprint (Ctrl+B)")
                                    .on_click(cx.listener(|panel, _, _window, cx| {
                                        panel.start_compilation(cx);
                                    }))
                            )
                            .child(
                                Button::new("save")
                                    .icon(IconName::FloppyDisk)
                                    .ghost()
                                    .tooltip("Save Blueprint (Ctrl+S)")
                                    .on_click(cx.listener(|panel, _, _window, _cx| {
                                        if let Some(class_path) = &panel.current_class_path {
                                            let save_path = class_path.join("graph_save.json");
                                            if let Err(e) = panel.save_blueprint(save_path.to_str().unwrap()) {
                                                eprintln!("Failed to save blueprint: {}", e);
                                            } else {
                                                println!("Blueprint saved to {}", save_path.display());
                                            }
                                        } else {
                                            eprintln!("No class loaded - cannot save");
                                        }
                                    }))
                            )
                    )
                    // Middle section - Editing tools
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Self::render_separator(cx))
                            .child(
                                Button::new("add_comment")
                                    .icon(IconName::Plus)
                                    .tooltip("Add Comment (C)")
                                    .ghost()
                                    .on_click(cx.listener(|panel, _, window, cx| {
                                        panel.create_comment_at_center(window, cx);
                                    }))
                            )
                            .child(
                                Button::new("delete")
                                    .icon(IconName::Delete)
                                    .tooltip("Delete Selected (Del)")
                                    .ghost()
                                    .on_click(cx.listener(|panel, _, _window, cx| {
                                        panel.delete_selected_nodes(cx);
                                    }))
                            )
                    )
                    // Center section - View controls
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Self::render_separator(cx))
                            .child(
                                Button::new("zoom_in")
                                    .icon(IconName::Plus)
                                    .tooltip("Zoom In (+)")
                                    .ghost()
                                    .on_click(cx.listener(|panel, _, _window, cx| {
                                        let graph = panel.get_graph_mut();
                                        graph.zoom_level = (graph.zoom_level * 1.2).min(3.0);
                                        cx.notify();
                                    }))
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{:.0}%", panel.get_graph().zoom_level * 100.0))
                            )
                            .child(
                                Button::new("zoom_out")
                                    .icon(IconName::Minus)
                                    .tooltip("Zoom Out (-)")
                                    .ghost()
                                    .on_click(cx.listener(|panel, _, _window, cx| {
                                        let graph = panel.get_graph_mut();
                                        graph.zoom_level = (graph.zoom_level / 1.2).max(0.2);
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("fit_view")
                                    .icon(IconName::BadgeCheck)
                                    .tooltip("Fit to View (F)")
                                    .ghost()
                                    .on_click(cx.listener(|panel, _, _window, cx| {
                                        let graph = panel.get_graph_mut();
                                        graph.zoom_level = 1.0;
                                        graph.pan_offset = Point::new(0.0, 0.0);
                                        cx.notify();
                                    }))
                            )
                    )
                    // Spacer
                    .child(div().flex_1())
                    // Right section - Status and info
                    .child(Self::render_graph_stats(panel, cx))
                    .child(Self::render_compilation_status(panel, cx))
            )
            // Compilation progress bar (when compiling)
            .children(if panel.compilation_status.is_compiling {
                vec![Self::render_compilation_progress(panel, cx)]
            } else {
                vec![]
            })
    }

    fn render_separator(cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        div()
            .w_px()
            .h(px(24.0))
            .bg(cx.theme().border.opacity(0.5))
            .mx_2()
    }

    fn render_graph_stats(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        let stats = &panel.graph.virtualization_stats;
        
        h_flex()
            .gap_3()
            .items_center()
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .bg(cx.theme().background.opacity(0.5))
            .border_1()
            .border_color(cx.theme().border.opacity(0.3))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Nodes:")
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().accent)
                            .child(format!("{}", stats.total_nodes))
                    )
            )
            .child(
                div()
                    .w_px()
                    .h(px(16.0))
                    .bg(cx.theme().border.opacity(0.3))
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Connections:")
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().info)
                            .child(format!("{}", stats.total_connections))
                    )
            )
            .child(
                div()
                    .w_px()
                    .h(px(16.0))
                    .bg(cx.theme().border.opacity(0.3))
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Variables:")
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().success)
                            .child(format!("{}", panel.class_variables.len()))
                    )
            )
    }

    fn render_compilation_status(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        let status = &panel.compilation_status;
        
        let (icon, text, color) = match status.state {
            super::CompilationState::Idle => ("⚪", "Ready", cx.theme().muted_foreground),
            super::CompilationState::Compiling => ("🔵", "Compiling...", cx.theme().info),
            super::CompilationState::Success => ("✅", "Success", cx.theme().success),
            super::CompilationState::Error => ("❌", "Error", cx.theme().danger),
        };

        h_flex()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .rounded(px(6.0))
            .bg(cx.theme().background.opacity(0.7))
            .border_1()
            .border_color(color.opacity(0.5))
            .child(
                div()
                    .text_sm()
                    .child(icon)
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
                    .child(text)
            )
    }

    fn render_compilation_progress(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        let status = &panel.compilation_status;
        
        v_flex()
            .w_full()
            .child(
                // Progress bar
                div()
                    .w_full()
                    .h(px(3.0))
                    .bg(cx.theme().muted.opacity(0.2))
                    .child(
                        div()
                            .h_full()
                            .w(relative(status.progress))
                            .bg(cx.theme().info)
                    )
            )
            .child(
                // Progress message
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2()
                    .bg(cx.theme().secondary.darken(0.02))
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status.message.clone())
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("JetBrainsMono-Regular")
                            .text_color(cx.theme().accent)
                            .child(format!("{:.0}%", status.progress * 100.0))
                    )
            )
    }
}
