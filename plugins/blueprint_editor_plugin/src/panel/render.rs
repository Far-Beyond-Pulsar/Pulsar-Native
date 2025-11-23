//! Rendering - GPUI render implementation

use gpui::*;
use gpui::prelude::*;
use ui::{dock::{Panel, PanelEvent, PanelState}, h_flex, v_flex, ActiveTheme, PixelsExt};
use super::core::BlueprintEditorPanel;
use super::super::toolbar::ToolbarRenderer;
use super::super::{DuplicateNode, DeleteNode, CopyNode, PasteNode, DisconnectPin, OpenAddNodeMenu, OpenEngineLibraryRequest, ShowNodePickerRequest};

impl Panel for BlueprintEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Blueprint Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        h_flex()
            .gap_2()
            .items_center()

            .child(div().text_sm().child(if let Some(title) = &self.tab_title {
                title.clone()
            } else {
                "Blueprint Editor".to_string()
            }))
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

impl Focusable for BlueprintEditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for BlueprintEditorPanel {}
impl EventEmitter<OpenEngineLibraryRequest> for BlueprintEditorPanel {}
impl EventEmitter<ShowNodePickerRequest> for BlueprintEditorPanel {}

// EditorInstance trait implementation for plugin system
impl plugin_editor_api::EditorInstance for BlueprintEditorPanel {
    fn file_path(&self) -> &std::path::PathBuf {
        self.current_class_path.as_ref().unwrap_or(&std::path::PathBuf::new())
    }

    fn save(&mut self, window: &mut Window, cx: &mut App) -> Result<(), plugin_editor_api::PluginError> {
        if let Some(path) = &self.current_class_path {
            // Use the existing save_blueprint method from file_io module
            self.save_blueprint(path.to_str().unwrap(), window, cx)
                .map_err(|e| plugin_editor_api::PluginError::FileSaveError {
                    path: path.clone(),
                    message: e.to_string(),
                })
        } else {
            Err(plugin_editor_api::PluginError::Other {
                message: "No file path set".into(),
            })
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut App) -> Result<(), plugin_editor_api::PluginError> {
        if let Some(path) = &self.current_class_path {
            self.load_blueprint(path.to_str().unwrap(), window, cx)
                .map_err(|e| plugin_editor_api::PluginError::FileLoadError {
                    path: path.clone(),
                    message: e.to_string(),
                })
        } else {
            Err(plugin_editor_api::PluginError::Other {
                message: "No file path set".into(),
            })
        }
    }

    fn is_dirty(&self) -> bool {
        // Check if there are unsaved changes
        // For now, we'll use compilation status as a proxy
        // TODO: Implement proper dirty tracking
        false
    }
}

impl Render for BlueprintEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace if needed
        if self.workspace.is_none() {
            self.initialize_workspace(window, cx);
        }

        use ui::{button::{Button, ButtonVariants}, IconName};
        use super::super::{macros, variables, properties, node_graph::NodeGraphRenderer};
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .key_context("BlueprintEditor")
            .on_action(cx.listener(|panel, action: &DuplicateNode, _window, cx| {
                panel.duplicate_node(action.node_id.clone(), cx);
            }))
            .on_action(cx.listener(|panel, action: &DeleteNode, _window, cx| {
                panel.delete_node(action.node_id.clone(), cx);
            }))
            .on_action(cx.listener(|panel, action: &CopyNode, _window, cx| {
                panel.copy_node(action.node_id.clone(), cx);
            }))
            .on_action(cx.listener(|panel, _action: &PasteNode, _window, cx| {
                panel.paste_node(cx);
            }))
            .on_action(cx.listener(|panel, action: &DisconnectPin, _window, cx| {
                panel.disconnect_pin(action.node_id.clone(), action.pin_id.clone(), cx);
            }))
            .on_action(cx.listener(|panel, _action: &OpenAddNodeMenu, window, cx| {
                // Open node menu at center of visible graph area
                if let Some(bounds) = &panel.graph_element_bounds {
                    let screen_center = Point::new(
                        bounds.center().x,
                        bounds.center().y,
                    );
                    let graph_pos = super::super::node_graph::NodeGraphRenderer::screen_to_graph_pos(
                        screen_center, 
                        &panel.graph
                    );
                    panel.show_node_picker(graph_pos, window, cx);
                }
            }))
            .child(ToolbarRenderer::render(self, cx))
            .child(
                // Modular workspace with fully dockable panels
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|el| {
                        if let Some(workspace) = &self.workspace {
                            el.child(workspace.clone())
                        } else {
                            el.child(div().child("Initializing workspace..."))
                        }
                    })
            )
    }
}

impl BlueprintEditorPanel {
    /// Render professional tab bar for graph navigation (Unreal-style) - OLD LAYOUT - DEPRECATED
    fn _old_render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                .when(!tab.is_main, |this| {
                                    this.child(
                                        Button::new(("close-tab", index))
                                            .icon(IconName::Close)
                                            .ghost()
                                            .compact()
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.close_tab(index, cx);
                                            }))
                                    )
                                })
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                    this.switch_to_tab(index, cx);
                                }))
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

    /// Render "My Blueprint" tabbed panel
    fn render_my_blueprint_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{button::{Button, ButtonVariants}, IconName};
        
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                // Tab bar with drag support
                h_flex()
                    .w_full()
                    .h(px(28.0))
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        h_flex()
                            .items_center()
                            .child(self.render_sidebar_tab("Variables", IconName::Code, 0, 0, cx))
                            .child(self.render_sidebar_tab("Macros", IconName::Component, 1, 0, cx))
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("my-bp-menu")
                            .icon(IconName::Menu)
                            .ghost()
                            .compact()
                            .tooltip("Panel Options")
                    )

            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|el| {
                        match self.left_top_tab {
                            0 => el.child(super::super::variables::VariablesRenderer::render(self, cx)),
                            1 => el.child(super::super::macros::MacrosRenderer::render(self, cx)),
                            _ => el.child(div())
                        }
                    })
            )
    }

    /// Render bottom left panel
    fn render_left_bottom_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{button::{Button, ButtonVariants}, IconName};
        
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.0))
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        h_flex()
                            .items_center()
                            .child(self.render_sidebar_tab("Compiler", IconName::Terminal, 0, 1, cx))
                            .child(self.render_sidebar_tab("Find", IconName::Search, 1, 1, cx))
                    )
                    .child(div().flex_1())

            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|el| {
                        match self.left_bottom_tab {
                            0 => el.child(self.render_compiler_results(cx)),
                            1 => el.child(self.render_find_panel(cx)),
                            _ => el.child(div())
                        }
                    })
            )
    }

    /// Render right sidebar Details panel
    fn render_details_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{button::{Button, ButtonVariants}, IconName};
        
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.0))
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .child(
                        h_flex()
                            .items_center()
                            .child(self.render_sidebar_tab("Details", IconName::Settings, 0, 2, cx))
                            .child(self.render_sidebar_tab("Palette", IconName::Palette, 1, 2, cx))
                    )
                    .child(div().flex_1())

            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|el| {
                        match self.right_tab {
                            0 => el.child(super::super::properties::PropertiesRenderer::render(self, cx)),
                            1 => el.child(super::super::node_library::NodeLibraryRenderer::render(self, cx)),
                            _ => el.child(div())
                        }
                    })
            )
    }

    /// Render a sidebar tab (compact Unreal-style)
    fn render_sidebar_tab(&self, label: &'static str, icon: ui::IconName, tab_index: usize, panel_id: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = match panel_id {
            0 => self.left_top_tab == tab_index,
            1 => self.left_bottom_tab == tab_index,
            2 => self.right_tab == tab_index,
            _ => false,
        };
        let label_str = label.to_string();
        
        h_flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .h(px(28.0))
            .bg(if is_active {
                cx.theme().background
            } else {
                gpui::transparent_black()
            })
            .when(is_active, |this| {
                this.border_b_2()
                    .border_color(cx.theme().accent)
            })
            .when(!is_active, |this| {
                this.hover(|s| s.bg(cx.theme().muted.opacity(0.1)))
            })
            .cursor_pointer()
            .child(
                ui::Icon::new(icon)
                    .size(px(12.0))
                    .text_color(if is_active {
                        cx.theme().accent
                    } else {
                        cx.theme().muted_foreground
                    })
            )
            .child(
                div()
                    .text_xs()
                    .when(is_active, |s| s.font_weight(gpui::FontWeight::SEMIBOLD))
                    .text_color(if is_active {
                        cx.theme().foreground
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(label_str)
            )
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                // TODO: Add drag and drop support when GPUI provides the API
                match panel_id {
                    0 => this.left_top_tab = tab_index,
                    1 => this.left_bottom_tab = tab_index,
                    2 => this.right_tab = tab_index,
                    _ => {}
                }
                cx.notify();
            }))
    }
    
    /// OLD RENDER METHODS - DEPRECATED - NOW IN render_workspace.rs
    fn _old_render_compiler_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(
                        Button::new("clear-compiler")
                            .icon(IconName::Close)
                            .ghost()
                            .compact()
                            .tooltip("Clear History")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.compilation_history.clear();
                                cx.notify();
                            }))
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
    
    fn _old_render_find_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
}
