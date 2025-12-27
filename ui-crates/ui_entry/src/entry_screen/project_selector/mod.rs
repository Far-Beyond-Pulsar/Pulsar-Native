use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _}, v_flex, ActiveTheme as _, StyledExt,
};
use std::path::PathBuf;

pub struct ProjectSelector {
    focus_handle: FocusHandle,
    selected_path: Option<PathBuf>,
}

impl ProjectSelector {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            selected_path: None,
        }
    }

    fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Use native file dialog to select project folder
        let file_dialog = rfd::AsyncFileDialog::new()
            .set_title("Select Pulsar Project Folder")
            .set_directory(std::env::current_dir().unwrap_or_default());

        cx.spawn(async move |this, cx| {
            if let Some(folder) = file_dialog.pick_folder().await {
                let path = folder.path().to_path_buf();

                // Validate that Pulsar.toml exists
                let toml_path = path.join("Pulsar.toml");
                if !toml_path.exists() {
                    // Show error - not a valid Pulsar project
                    tracing::error!("Invalid project: Pulsar.toml not found in selected folder");
                    return;
                }

                cx.update(|cx| {
                    let _ = this.update(cx, |selector, cx| {
                        selector.selected_path = Some(path.clone());
                        cx.notify();
                    });
                }).ok();
            }
        }).detach();
    }

    fn confirm_project(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.selected_path {
            // Only emit the event - the window creation is handled by the event subscriber
            cx.emit(ProjectSelected { path: path.clone() });
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProjectSelected {
    pub path: PathBuf,
}

impl EventEmitter<ProjectSelected> for ProjectSelector {}

impl Focusable for ProjectSelector {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProjectSelector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .gap_6()
                    .items_center()
                    .p_8()
                    .max_w_96()
                    .child(
                        // Logo/Icon
                        div()
                            .size_24()
                            .rounded_full()
                            .bg(cx.theme().primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(cx.theme().primary_foreground)
                            .text_2xl()
                            .font_bold()
                            .child("P")
                    )
                    .child(
                        // Title
                        div()
                            .text_2xl()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child("Pulsar Engine")
                    )
                    .child(
                        // Subtitle
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .text_center()
                            .child("Open a project to get started")
                    )
                    .children(self.selected_path.as_ref().map(|path| {
                        div()
                            .p_3()
                            .bg(cx.theme().muted.opacity(0.3))
                            .rounded(cx.theme().radius)
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(path.display().to_string())
                    }))
                    .child(
                        // Action buttons
                        v_flex()
                            .gap_3()
                            .w_full()
                            .child(
                                Button::new("open-project")
                                    .primary()
                                    .label("Open Project Folder")
                                    .w_full()
                                    .on_click(cx.listener(|selector, _, window, cx| {
                                        selector.open_folder_dialog(window, cx);
                                    }))
                            )
                            .children(if self.selected_path.is_some() {
                                Some(
                                    Button::new("confirm-project")
                                        .primary()
                                        .label("Continue")
                                        .w_full()
                                        .on_click(cx.listener(|selector, _, _, cx| {
                                            selector.confirm_project(cx);
                                        }))
                                )
                            } else {
                                None
                            })
                    )
                    .child(
                        //TODO: Recent projects (placeholder for future)
                        div()
                            .mt_4()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Recent projects will appear here")
                    )
            )
    }
}
