use std::sync::{Arc, RwLock};

use rfd::FileDialog;
use gpui::{
    Application, Bounds, MouseButton, Size, WindowBounds, WindowOptions, div,
    prelude::*, rgb, SharedString, px,
};

/// TopBar: displays title, icon, and window controls.
pub fn top_bar(title: SharedString) -> gpui::Div {
    div()
        .flex()
        .h_10()
        .items_center()
        .justify_between()
        .px_4()
        .bg(rgb(0x0A0A0A))
        .child(
            // Title and icon.
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_color(rgb(0x2F80ED))
                .child("◆")
                .child(
                    div()
                        .text_color(rgb(0x2F80ED))
                        .text_sm()
                        .child(title)
                )
        )
        .child(
            // Window controls.
            div()
                .flex()
                .gap_4()
                .child(div().text_lg().text_color(rgb(0x666666)).child("−"))
                .child(div().text_lg().text_color(rgb(0x666666)).child("□"))
                .child(div().text_lg().text_color(rgb(0x666666)).child("×"))
        )
}

/// MenuBar: displays menu items.
pub fn menu_bar() -> gpui::Div {
    div()
        .flex()
        .h_8()
        .items_center()
        .px_4()
        .gap_6()
        .bg(rgb(0x0A0A0A))
        .text_color(rgb(0xCCCCCC))
        .text_sm()
        .child("File")
        .child("Edit")
        .child("View")
        .child("Project")
        .child("Build")
        .child("Tools")
        .child("Settings")
        .child("Help")
        .child(
            div()
                .flex_grow()
                .flex()
                .justify_end()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("⚙")
                        .child("Settings")
                )
        )
}

/// TabBar: renders a button for each tab, highlighting the active one.
pub fn tab_bar(active_tab: usize, tabs: &[&str]) -> gpui::Div {
    let mut tab_elements: Vec<gpui::Div> = Vec::new();
    for (i, &tab_name) in tabs.iter().enumerate() {
        let is_active = i == active_tab;
        let tab = if is_active {
            div().px_3().py_1().bg(rgb(0x1A1A1A)).text_color(rgb(0xFFFFFF))
        } else {
            div().px_3().py_1().text_color(rgb(0x666666))
        };
        tab_elements.push(tab.child(format!("{} ×", tab_name)));
    }
    // Add a "+" button for creating new tabs.
    tab_elements.push(
        div()
            .flex_grow()
            .flex()
            .justify_end()
            .child(div().px_2().child("+ "))
    );
    div()
        .flex()
        .h_8()
        .items_center()
        .px_2()
        .gap_1()
        .bg(rgb(0x0A0A0A))
        .text_color(rgb(0x666666))
        .text_sm()
        .children(tab_elements)
}

/// Tab enum representing available tabs.
pub enum Tab {
    LevelEditor,
    ScriptEditor,
}

/// Level editor view.
pub fn level_editor_view() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_4()
        .text_color(rgb(0x666666))
        .child("↳")
        .child("Welcome to the Level Editor")
        .child("Open a file to start editing")
        .child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0x2F80ED))
                        .text_color(rgb(0xFFFFFF))
                        .rounded_sm()
                        .child("Open Folder")
                        .on_mouse_down(MouseButton::Left, |_, _, _| {
                            println!("Open Folder clicked");
                            FileDialog::new()
                                .set_directory("/")
                                .pick_folder()
                                .map(|path| println!("Selected folder: {:?}", path));
                        })
                )
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0x1A1A1A))
                        .text_color(rgb(0xFFFFFF))
                        .rounded_sm()
                        .child("New File")
                )
        )
}

/// Script editor view.
pub fn script_editor_view() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_4()
        .text_color(rgb(0x666666))
        .child("Welcome to the Script Editor")
        .child("Edit your scripts here")
        .child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0x2F80ED))
                        .text_color(rgb(0xFFFFFF))
                        .rounded_sm()
                        .child("Open Script")
                )
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0x1A1A1A))
                        .text_color(rgb(0xFFFFFF))
                        .rounded_sm()
                        .child("New Script")
                )
        )
}

/// Main content: displays sidebar and editor view based on active tab.
pub fn main_content(active_tab: &Tab) -> gpui::Div {
    let sidebar = div()
        .w_64()
        .bg(rgb(0x0A0A0A))
        .border_color(rgb(0x222222))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .py_2()
                .text_color(rgb(0xCCCCCC))
                .text_sm()
                .child("EXPLORER")
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child("+")
                        .child("□")
                        .child("↻")
                )
        );
    let editor_view = match active_tab {
        Tab::LevelEditor => level_editor_view(),
        Tab::ScriptEditor => script_editor_view(),
    };
    div()
        .flex()
        .flex_grow()
        .child(sidebar)
        .child(
            div()
                .flex_grow()
                .flex()
                .justify_center()
                .items_center()
                .child(editor_view)
        )
}

/// Status bar: displays FPS, memory usage, time, and branch info.
pub fn status_bar(fps: Arc<RwLock<SharedString>>, memory: Arc<RwLock<SharedString>>, time: Arc<RwLock<SharedString>>, branch: Arc<RwLock<SharedString>>) -> gpui::Div {
    div()
        .flex()
        .h_6()
        .items_center()
        .justify_between()
        .px_4()
        .bg(rgb(0x0A0A0A))
        .text_color(rgb(0xCCCCCC))
        .text_xs()
        .child(
            div()
                .flex()
                .gap_4()
                .items_center()
                .child("⚠ Rust Analyzer: 2 issues")
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(rgb(0xFFA500))
                        .child("⌥")
                        .child(branch)
                )
        )
        .child(
            div()
                .flex()
                .gap_4()
                .items_center()
                .child(format!("FPS: {}", fps))
                .child(format!("{} MB", memory))
                .child(time)
        )
}