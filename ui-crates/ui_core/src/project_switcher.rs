//! Project Switcher Dropdown Component
//!
//! Provides a searchable dropdown UI for switching between recent projects.
//! Similar to GenericPalette but optimized for the statusbar.

use directories::ProjectDirs;
use gpui::prelude::*;
use gpui::{
    div, px, Axis, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, Render, Window, rgba,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ui::{
    h_flex, input::{Escape, InputEvent, InputState, TextInput}, v_flex, ActiveTheme as _, Icon,
    IconName, StyledExt,
};

/// Recent project metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

/// Recent projects list from disk
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl RecentProjectsList {
    pub fn load(path: &Path) -> Self {
        use ui_common::file_utils;
        file_utils::read_json(path).unwrap_or_default()
    }

    pub fn get_recent_projects_path() -> Option<PathBuf> {
        ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|proj| proj.data_dir().join("recent_projects.json"))
    }

    pub fn load_from_default_location() -> Self {
        Self::get_recent_projects_path()
            .as_ref()
            .map(|p| Self::load(p.as_path()))
            .unwrap_or_default()
    }
}

/// Event emitted when a project is selected
#[derive(Clone, Debug)]
pub struct ProjectSelected {
    pub project: RecentProject,
}

/// Project Switcher Dropdown UI Component
pub struct ProjectSwitcherDropdown {
    pub focus_handle: FocusHandle,
    pub search_input: Entity<InputState>,
    all_projects: Vec<RecentProject>,
    filtered_projects: Vec<RecentProject>,
    selected_index: usize,
    current_project_path: Option<PathBuf>,
}

impl EventEmitter<DismissEvent> for ProjectSwitcherDropdown {}
impl EventEmitter<ProjectSelected> for ProjectSwitcherDropdown {}

impl ProjectSwitcherDropdown {
    pub fn new(
        current_project_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search projects...", window, cx);
            state
        });

        let all_projects = RecentProjectsList::load_from_default_location().projects;
        let filtered_projects = all_projects.clone();

        // Subscribe to search input changes
        cx.subscribe(&search_input, |this, _input, event: &InputEvent, cx| {
            if event == &InputEvent::Change {
                let query = this.search_input.read(cx).text().to_string();
                this.update_filter(&query);
                cx.notify();
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            all_projects,
            filtered_projects,
            selected_index: 0,
            current_project_path,
        }
    }

    fn update_filter(&mut self, query: &str) {
        if query.is_empty() {
            self.filtered_projects = self.all_projects.clone();
        } else {
            let query_lower = query.to_lowercase();
            self.filtered_projects = self
                .all_projects
                .iter()
                .filter(|proj| {
                    proj.name.to_lowercase().contains(&query_lower)
                        || proj.path.to_lowercase().contains(&query_lower)
                })
                .cloned()
                .collect();
        }
        self.selected_index = 0;
    }

    fn select_project(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = self.filtered_projects.get(self.selected_index) {
            cx.emit(ProjectSelected {
                project: project.clone(),
            });
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered_projects.is_empty() {
            return;
        }
        let new_index = ((self.selected_index as isize) + delta)
            .rem_euclid(self.filtered_projects.len() as isize) as usize;
        self.selected_index = new_index;
        cx.notify();
    }
}

impl Render for ProjectSwitcherDropdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_current_project = |proj: &RecentProject| {
            self.current_project_path
                .as_ref()
                .map(|p| p.to_string_lossy() == proj.path)
                .unwrap_or(false)
        };

        // Full-screen overlay with darkened background
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000099))
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            .on_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.emit(DismissEvent);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "down" | "arrowdown" => {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "up" | "arrowup" => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "enter" | "return" => {
                        this.select_project(cx);
                        cx.stop_propagation();
                    }
                    "escape" => {
                        cx.emit(DismissEvent);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                v_flex()
                    .w(px(500.0))
                    .max_h(px(400.0))
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "down" | "arrowdown" => {
                                this.move_selection(1, cx);
                                cx.stop_propagation();
                            }
                            "up" | "arrowup" => {
                                this.move_selection(-1, cx);
                                cx.stop_propagation();
                            }
                            "enter" | "return" => {
                                this.select_project(cx);
                                cx.stop_propagation();
                            }
                            "escape" => {
                                cx.emit(DismissEvent);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
                    // Search input
                    .child(
                        h_flex()
                            .p_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                TextInput::new(&self.search_input)
                                    .appearance(false)
                                    .bordered(false)
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size(px(18.0))
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .w_full(),
                            ),
                    )
                    // Project list
                    .child(
                        if self.filtered_projects.is_empty() {
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No projects found"),
                                )
                                .into_any_element()
                        } else {
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .p_2()
                                        .id("projects-list")
                                        .scrollable(Axis::Vertical)
                                        .children(self.filtered_projects.iter().enumerate().map(
                                            |(idx, proj)| {
                                                let is_selected = idx == self.selected_index;
                                                let is_current = is_current_project(proj);

                                                h_flex()
                                                    .w_full()
                                                    .px_3()
                                                    .py_2p5()
                                                    .rounded(px(6.0))
                                                    .gap_3()
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .when(is_selected, |this| {
                                                        this.bg(cx.theme().primary.opacity(0.15))
                                                    })
                                                    .hover(|s| {
                                                        s.bg(cx.theme().muted.opacity(0.2))
                                                    })
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.selected_index = idx;
                                                            this.select_project(cx);
                                                        }),
                                                    )
                                                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                                        if this.selected_index != idx {
                                                            this.selected_index = idx;
                                                            cx.notify();
                                                        }
                                                    }))
                                                    .child(
                                                        Icon::new(if is_current {
                                                            IconName::CheckCircle
                                                        } else {
                                                            IconName::Folder
                                                        })
                                                        .size(px(18.0))
                                                        .text_color(if is_selected {
                                                            cx.theme().primary
                                                        } else if is_current {
                                                            cx.theme().success
                                                        } else {
                                                            cx.theme().muted_foreground
                                                        }),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .flex_1()
                                                            .gap_0p5()
                                                            .overflow_hidden()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .font_semibold()
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .whitespace_nowrap()
                                                                    .text_color(if is_selected {
                                                                        cx.theme().foreground
                                                                    } else {
                                                                        cx.theme().foreground.opacity(0.9)
                                                                    })
                                                                    .child(proj.name.clone()),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .whitespace_nowrap()
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(proj.path.clone()),
                                                            ),
                                                    )
                                                    .into_any_element()
                                            },
                                        )),
                                )
                                .into_any_element()
                        },
                    )
            )
    }
}

impl Focusable for ProjectSwitcherDropdown {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
