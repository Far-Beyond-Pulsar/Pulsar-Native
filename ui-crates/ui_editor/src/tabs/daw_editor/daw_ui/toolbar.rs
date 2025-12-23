/// DAW Toolbar Component
/// Top toolbar with file operations, edit tools, and view options

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, Disableable, divider::Divider,
};

pub fn render_toolbar(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(48.0))
        .px_4()
        .gap_3()
        .items_center()
        .bg(cx.theme().muted.opacity(0.3))
        .border_b_1()
        .border_color(cx.theme().border)
        // File operations
        .child(render_file_section(state, cx))
        .child(Divider::vertical().h(px(24.0)).bg(cx.theme().border))
        // Edit tools
        .child(render_tools_section(state, cx))
        .child(Divider::vertical().h(px(24.0)).bg(cx.theme().border))
        // View options
        .child(render_view_section(state, cx))
        .child(Divider::vertical().h(px(24.0)).bg(cx.theme().border))
        // Snap settings
        .child(render_snap_section(state, cx))
        // Spacer
        .child(div().flex_1())
        // Project info
        .child(render_project_info(state, cx))
}

fn render_file_section(state: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut DawUiState, cx: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut Context<super::panel::DawPanel>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            Button::new("toolbar-new")
                .icon(Icon::new(IconName::Plus))
                .ghost()
                .small()
                .tooltip("New Project")
                .on_click(cx.listener(|this, _, window, cx| {
                    handle_new_project(&mut this.state, window, cx);
                }))
        )
        // Open button removed - DAW opens via engine asset selection
        .child(
            Button::new("toolbar-save")
                .icon(Icon::new(IconName::Download))
                .ghost()
                .small()
                .disabled(state.project.is_none())
                .tooltip("Save Project")
                .on_click(cx.listener(|this, _, _window, cx| {
                    if let Err(e) = this.state.save_project() {
                        eprintln!("❌ Save failed: {}", e);
                    } else {
                        eprintln!("✅ Project saved");
                    }
                    cx.notify();
                }))
        )
        .child(
            Button::new("toolbar-export")
                .icon(Icon::new(IconName::Download))
                .ghost()
                .small()
                .disabled(state.project.is_none())
                .tooltip("Export Audio")
        )
}

fn render_tools_section(state: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut DawUiState, cx: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut Context<super::panel::DawPanel>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            Button::new("tool-select")
                .icon(Icon::new(IconName::CursorPointer))
                .when(state.current_tool == EditTool::Select, |b| b.primary())
                .when(state.current_tool != EditTool::Select, |b| b.ghost())
                .small()
                .tooltip("Select Tool")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.current_tool = EditTool::Select;
                    cx.notify();
                }))
        )
        .child(
            Button::new("tool-draw")
                .icon(Icon::new(IconName::EditPencil))
                .when(state.current_tool == EditTool::Draw, |b| b.primary())
                .when(state.current_tool != EditTool::Draw, |b| b.ghost())
                .small()
                .tooltip("Draw Tool")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.current_tool = EditTool::Draw;
                    cx.notify();
                }))
        )
        .child(
            Button::new("tool-cut")
                .icon(Icon::new(IconName::Scissor))
                .when(state.current_tool == EditTool::Cut, |b| b.primary())
                .when(state.current_tool != EditTool::Cut, |b| b.ghost())
                .small()
                .tooltip("Cut Tool")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.current_tool = EditTool::Cut;
                    cx.notify();
                }))
        )
        .child(
            Button::new("tool-erase")
                .icon(Icon::new(IconName::Trash))
                .when(state.current_tool == EditTool::Erase, |b| b.primary())
                .when(state.current_tool != EditTool::Erase, |b| b.ghost())
                .small()
                .tooltip("Erase Tool")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.current_tool = EditTool::Erase;
                    cx.notify();
                }))
        )
}

fn render_view_section(state: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut DawUiState, cx: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut Context<super::panel::DawPanel>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            Button::new("view-arrange")
                .label("Arrange")
                .when(state.view_mode == ViewMode::Arrange, |b| b.primary())
                .when(state.view_mode != ViewMode::Arrange, |b| b.ghost())
                .small()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.view_mode = ViewMode::Arrange;
                    cx.notify();
                }))
        )
        .child(
            Button::new("view-mix")
                .label("Mix")
                .when(state.view_mode == ViewMode::Mix, |b| b.primary())
                .when(state.view_mode != ViewMode::Mix, |b| b.ghost())
                .small()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.view_mode = ViewMode::Mix;
                    cx.notify();
                }))
        )
        .child(
            Button::new("view-edit")
                .label("Edit")
                .when(state.view_mode == ViewMode::Edit, |b| b.primary())
                .when(state.view_mode != ViewMode::Edit, |b| b.ghost())
                .small()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.view_mode = ViewMode::Edit;
                    cx.notify();
                }))
        )
}

fn render_snap_section(state: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut DawUiState, cx: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut Context<super::panel::DawPanel>) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            Button::new("snap-toggle")
                .icon(Icon::new(IconName::Menu))
                .when(state.snap_mode != SnapMode::Off, |b| b.primary())
                .when(state.snap_mode == SnapMode::Off, |b| b.ghost())
                .small()
                .tooltip("Toggle Snap")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.state.snap_mode = match this.state.snap_mode {
                        SnapMode::Off => SnapMode::Grid,
                        _ => SnapMode::Off,
                    };
                    cx.notify();
                }))
        )
        .child(
            Button::new("snap-value")
                .label(state.snap_value.label())
                .ghost()
                .small()
                .tooltip("Snap Value")
                .on_click(cx.listener(|this, _, _window, cx| {
                    // Cycle through snap values
                    this.state.snap_value = match this.state.snap_value {
                        SnapValue::Bar => SnapValue::Half,
                        SnapValue::Half => SnapValue::Quarter,
                        SnapValue::Quarter => SnapValue::Eighth,
                        SnapValue::Eighth => SnapValue::Sixteenth,
                        SnapValue::Sixteenth => SnapValue::ThirtySecond,
                        SnapValue::ThirtySecond => SnapValue::Bar,
                    };
                    cx.notify();
                }))
        )
}

fn render_project_info(state: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut DawUiState, cx: <V: 'static>(state: &mut DawUiState, cx: &mut Context<super::panel::DawPanel>)mut Context<super::panel::DawPanel>) -> impl IntoElement {
    let project_name = state.project.as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "No Project".to_string());
    
    let track_count = state.project.as_ref()
        .map(|p| p.tracks.len())
        .unwrap_or(0);
    
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(project_name)
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{} tracks", track_count))
        )
}

// Event handlers

fn handle_new_project(state: &mut DawUiState, window: &mut Window, cx: &mut Context<DawPanel>) {
    // In a real implementation, this would open a dialog
    // For now, create a default project
    use std::env;
    let default_dir = env::temp_dir().join("pulsar_projects");
    state.new_project("New Project".to_string(), default_dir);
    cx.notify();
}

