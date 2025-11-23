//! Workspace-based rendering for Blueprint Editor
//!
//! This uses the modular workspace system for fully dockable panels

use gpui::*;
use ui::{
    workspace::{Workspace, WorkspacePanel},
    dock::{DockItem, DockPlacement, PanelView},
    v_flex, h_flex, ActiveTheme, IconName, StyledExt,
};
use super::core::BlueprintEditorPanel;
use super::super::toolbar::ToolbarRenderer;
use super::super::node_graph::NodeGraphRenderer;
use super::workspace_panels::*;
use std::sync::Arc;
use gpui::prelude::*;

impl BlueprintEditorPanel {
    /// Initialize the workspace with all dockable panels
    pub(super) fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return; // Already initialized
        }

        let editor_weak = cx.entity().downgrade();
        let workspace = cx.new(|cx| {
            // Use channel 1 for blueprint editor to isolate from main app dock (channel 0)
            Workspace::new_with_channel(
                "blueprint-editor-workspace",
                ui::dock::DockChannel(1),
                window,
                cx
            )
        });

        // Initialize workspace AFTER creation to avoid entity borrow issues
        workspace.update(cx, |workspace, cx| {
            // Get the dock area weak reference
            let dock_area_weak = workspace.dock_area().downgrade();

            // Create all dockable panels
            let variables_panel = Self::create_variables_panel(editor_weak.clone(), window, cx);
            let macros_panel = Self::create_macros_panel(editor_weak.clone(), window, cx);
            let compiler_panel = Self::create_compiler_panel(editor_weak.clone(), window, cx);
            let find_panel = Self::create_find_panel(editor_weak.clone(), window, cx);
            let properties_panel = Self::create_properties_panel(editor_weak.clone(), window, cx);
            let palette_panel = Self::create_palette_panel(editor_weak.clone(), window, cx);

            // Center is the editor itself (which is already a Panel)
            // We can't use it directly in the workspace init, so create a simple graph view panel
            let center_panel = Self::create_center_panel(editor_weak.clone(), window, cx);
            let center = DockItem::tabs(
                vec![Arc::new(center_panel)],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            // Create left dock (My Blueprint section)
            let left = DockItem::tabs(
                vec![Arc::new(variables_panel), Arc::new(macros_panel)],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            // Create bottom dock (Utilities section)
            let bottom = DockItem::tabs(
                vec![Arc::new(compiler_panel), Arc::new(find_panel)],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            // Create right dock (Inspector section)
            let right = DockItem::tabs(
                vec![Arc::new(properties_panel), Arc::new(palette_panel)],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            workspace.initialize(center, Some(left), Some(right), Some(bottom), window, cx);
        });

        self.workspace = Some(workspace);
    }

    /// Create Variables panel
    fn create_variables_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<VariablesPanel> {
        cx.new(|cx| VariablesPanel::new(editor_weak, cx))
    }

    /// Create Macros panel
    fn create_macros_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<MacrosPanel> {
        cx.new(|cx| MacrosPanel::new(editor_weak, cx))
    }

    /// Create Compiler panel
    fn create_compiler_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<CompilerPanel> {
        cx.new(|cx| CompilerPanel::new(editor_weak, cx))
    }

    /// Create Find panel
    fn create_find_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<FindPanel> {
        cx.new(|cx| FindPanel::new(editor_weak, cx))
    }

    /// Create Properties panel
    fn create_properties_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<PropertiesPanel> {
        cx.new(|cx| PropertiesPanel::new(editor_weak, cx))
    }

    /// Create Palette panel
    fn create_palette_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<PalettePanel> {
        cx.new(|cx| PalettePanel::new(editor_weak, cx))
    }

    /// Create Center panel (main graph canvas with tabs)
    fn create_center_panel(
        editor_weak: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<GraphCanvasPanel> {
        cx.new(|cx| GraphCanvasPanel::new(editor_weak, cx))
    }

    // Helper methods to render panel content

    pub(super) fn render_variables_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        super::super::variables::VariablesRenderer::render(self, cx)
    }

    pub(super) fn render_macros_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        super::super::macros::MacrosRenderer::render(self, cx)
    }

    pub(super) fn render_properties_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        super::super::properties::PropertiesRenderer::render(self, cx)
    }

    pub(super) fn render_node_library(&self, cx: &mut Context<Self>) -> impl IntoElement {
        super::super::node_library::NodeLibraryRenderer::render(self, cx)
    }

    pub(super) fn render_compiler_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use super::super::CompilationState;
        use ui::{button::{Button, ButtonVariants}, IconName};
        
        v_flex()
            .size_full()
            .child(
                // Header with current status and clear button
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(match self.compilation_status.state {
                                CompilationState::Success => gpui::green(),
                                CompilationState::Error => gpui::red(),
                                CompilationState::Compiling => gpui::yellow(),
                                _ => cx.theme().foreground,
                            })
                            .child(match self.compilation_status.state {
                                CompilationState::Idle => "Compiler Output",
                                CompilationState::Compiling => "⟳ Compiling...",
                                CompilationState::Success => "✓ Build Succeeded",
                                CompilationState::Error => "✗ Build Failed",
                            })
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} messages", self.compilation_history.len()))
                    )
            )
            .child(
                // Scrollable history list
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w_full()
                            .gap_0p5()
                            .children(
                                self.compilation_history.iter().rev().map(|entry| {
                                    h_flex()
                                        .w_full()
                                        .px_2()
                                        .py_1()
                                        .gap_2()
                                        .border_b_1()
                                        .border_color(cx.theme().border.opacity(0.1))
                                        .hover(|s| s.bg(cx.theme().muted.opacity(0.05)))
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_xs()
                                                .font_family("JetBrainsMono-Regular")
                                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                                .child(entry.timestamp.clone())
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .w(px(12.0))
                                                .text_xs()
                                                .text_color(match entry.state {
                                                    CompilationState::Success => gpui::green(),
                                                    CompilationState::Error => gpui::red(),
                                                    _ => cx.theme().muted_foreground,
                                                })
                                                .child(match entry.state {
                                                    CompilationState::Success => "✓",
                                                    CompilationState::Error => "✗",
                                                    _ => "•",
                                                })
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_xs()
                                                .text_color(cx.theme().foreground)
                                                .child(entry.message.clone())
                                        )
                                })
                            )
                            .when(self.compilation_history.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .h(px(100.0))
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No compilation history yet")
                                )
                            })
                    )
            )
    }

    pub(super) fn render_find_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_2()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Find in Blueprint - Coming soon")
            )
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{button::{Button, ButtonVariants}, IconName};
        
        h_flex()
            .w_full()
            .h(px(32.0))
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .items_center()
            .overflow_x_hidden()
            .child(
                // Tabs container
                h_flex()
                    .items_center()
                    .children(
                        self.open_tabs.iter().enumerate().map(|(index, tab)| {
                            let is_active = index == self.active_tab_index;
                            
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .px_3()
                                .h_full()
                                .bg(if is_active {
                                    cx.theme().background
                                } else {
                                    gpui::transparent_black()
                                })
                                .when(is_active, |this| {
                                    this.border_t_2()
                                        .border_color(cx.theme().accent)
                                })
                                .when(!is_active, |this| {
                                    this.hover(|s| s.bg(cx.theme().muted.opacity(0.1)))
                                })
                                .cursor_pointer()
                                .child(
                                    // Tab icon
                                    ui::Icon::new(if tab.is_main {
                                        IconName::Play
                                    } else {
                                        IconName::Component
                                    })
                                    .size(px(14.0))
                                    .text_color(if is_active {
                                        cx.theme().accent
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                )
                                .child(
                                    // Tab name
                                    div()
                                        .text_sm()
                                        .when(is_active, |s| s.font_weight(gpui::FontWeight::SEMIBOLD))
                                        .text_color(if is_active {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(tab.name.clone())
                                )
                                .when(tab.is_dirty, |this| {
                                    this.child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(cx.theme().accent)
                                    )
                                })
                        })
                    )
            )
            .child(
                // Spacer
                div().flex_1()
            )
            .child(
                // Graph utilities
                h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .child(
                        Button::new("find-in-graph")
                            .icon(IconName::Search)
                            .ghost()
                            .compact()
                            .tooltip("Find in Graph (Ctrl+F)")
                    )
                    .child(
                        Button::new("graph-settings")
                            .icon(IconName::Settings)
                            .ghost()
                            .compact()
                            .tooltip("Graph Settings")
                    )
            )
    }
}
