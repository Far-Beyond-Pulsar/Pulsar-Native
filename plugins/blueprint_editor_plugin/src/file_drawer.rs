use gpui::*;
use ui::{
    h_flex, v_flex,
    ActiveTheme as _, StyledExt,
};

use super::panel::BlueprintEditorPanel;

/// File drawer showing virtual mount points for Engine libraries, plugins, etc.
pub struct FileDrawerRenderer;

impl FileDrawerRenderer {
    pub fn render(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                // Header
                h_flex()
                    .w_full()
                    .px_4()
                    .py_3()
                    .bg(cx.theme().secondary)
                    .border_b_2()
                    .border_color(cx.theme().border)
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(6.0))
                                    .bg(cx.theme().accent.opacity(0.15))
                                    .border_1()
                                    .border_color(cx.theme().accent.opacity(0.3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_lg()
                                            .child("📁")
                                    )
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_bold()
                                    .text_color(cx.theme().foreground)
                                    .child("File Drawer")
                            )
                    )
            )
            .child(
                // Content area - virtual mount points
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .p_3()
                    .gap_2()
                    .scrollable(Axis::Vertical)
                    .child(Self::render_mount_points(panel, cx))
            )
    }

    fn render_mount_points(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                // Engine mount point
                Self::render_mount_point(
                    "Engine",
                    "⚙️",
                    &panel.library_manager,
                    cx
                )
            )
            .child(
                // Plugins mount point (future)
                Self::render_mount_point_placeholder(
                    "Plugins",
                    "🔌",
                    cx
                )
            )
            .child(
                // Project mount point (future)
                Self::render_mount_point_placeholder(
                    "Project",
                    "📦",
                    cx
                )
            )
            .child(
                // Types mount point
                Self::render_mount_point_placeholder(
                    "Types",
                    "📐",
                    cx
                )
            )
    }

    fn render_mount_point(
        name: &'static str,
        icon: &'static str,
        library_manager: &ui::graph::LibraryManager,
        cx: &mut Context<BlueprintEditorPanel>
    ) -> impl IntoElement {
        let libraries = library_manager.get_libraries();
        let library_count = libraries.len();

        v_flex()
            .w_full()
            .gap_1()
            .child(
                // Mount point header (collapsible)
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.4))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(cx.theme().accent.opacity(0.05))
                            .border_color(cx.theme().accent.opacity(0.4))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("▼") // Expand/collapse arrow
                    )
                    .child(
                        div()
                            .text_lg()
                            .child(icon)
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(name)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} libraries", library_count))
                            )
                    )
            )
            .child(
                // Libraries list (indented)
                v_flex()
                    .pl_6()
                    .gap_1()
                    .children(
                        libraries.values().map(|library| {
                            let name = library.name.clone();
                            let macro_count = library.subgraphs.len();
                            
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_2()
                                .gap_2()
                                .bg(cx.theme().background.opacity(0.5))
                                .border_1()
                                .border_color(cx.theme().border.opacity(0.2))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|style| {
                                    style
                                        .bg(cx.theme().accent.opacity(0.08))
                                        .border_color(cx.theme().accent.opacity(0.4))
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .child("📚")
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .text_color(cx.theme().foreground)
                                                .child(name)
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("{} macros", macro_count))
                                        )
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.4))
                                        .child("›")
                                )
                        }).collect::<Vec<_>>()
                    )
            )
    }

    fn render_library_entry(
        library: &ui::graph::SubGraphLibrary,
        cx: &mut Context<BlueprintEditorPanel>
    ) -> AnyElement {
        let name = library.name.clone();
        let macro_count = library.subgraphs.len();
        let library_id = library.id.clone();
        let library_name_for_click = library.name.clone();
        
        div()
            .w_full()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .bg(cx.theme().background.opacity(0.5))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.2))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(cx.theme().accent.opacity(0.08))
                            .border_color(cx.theme().accent.opacity(0.4))
                    })
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |panel, _, _, cx| {
                        // Emit event to open this library in main tabs
                        panel.request_open_engine_library(library_id.clone(), library_name_for_click.clone(), None, None, cx);
                    }))
                    .child(
                        div()
                            .text_sm()
                            .child("📚")
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child(name)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} macros", macro_count))
                            )
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.4))
                            .child("›")
                    )
            )
            .into_any_element()
    }

    fn render_mount_point_placeholder(
        name: &'static str,
        icon: &'static str,
        cx: &mut Context<BlueprintEditorPanel>
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .bg(cx.theme().background.opacity(0.3))
            .border_1()
            .border_color(cx.theme().border.opacity(0.2))
            .rounded(px(6.0))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("▶") // Collapsed arrow
            )
            .child(
                div()
                    .text_lg()
                    .opacity(0.5)
                    .child(icon)
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(name)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                            .child("Coming soon...")
                    )
            )
    }
}
