//! What the git tool adds to the editor shell (`ui_common::editor_tools`):
//! its status-bar button, its window's startup, the project's sync state in
//! the title bar (with auto-fetch while the project is open), the Git
//! Settings entry of the account menu, and the project's configured hooks
//! synced while the project loads. The shell names none of it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Context, IntoElement, Render, Task, UpdateGlobal as _, Window, div, prelude::*, px,
};
use ui::{ActiveTheme as _, Icon, IconName, h_flex};
use ui_common::editor_tools::{
    AccountMenuItemRegistration, EditorToolRegistration, ProjectOpenedRegistration,
    TitleBarItemRegistration,
};
use window_manager::WindowRegistry;

use crate::{GitManagerWindow, RepositoryProbe};

inventory::submit! {
    EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.git",
        label: || "Git Manager".to_string(),
        icon: IconName::GitBranch,
        order: 30,
        open: |_, cx| {
            WindowRegistry::update_global(cx, |registry, cx| registry.open("GitManagerWindow", cx))
        },
        is_open: None,
        init: Some(GitManagerWindow::init),
    }
}

inventory::submit! {
    TitleBarItemRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "git.sync_state",
        order: 0,
        build: |project, window, cx| {
            let project = project.map(Path::to_path_buf);
            cx.new(|cx| GitSyncIndicator::new(project, window, cx)).into()
        },
    }
}

inventory::submit! {
    AccountMenuItemRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "git.settings",
        section: || "GIT".to_string(),
        label: || "Git Settings".to_string(),
        icon: IconName::GitCommit,
        open: crate::open_git_settings_modal,
    }
}

inventory::submit! {
    ProjectOpenedRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "git.hooks",
        label: || "Synchronizing Git hooks".to_string(),
        run: sync_hooks,
    }
}

/// The loading step: sync the project's configured Git hooks, and say what
/// happened.
fn sync_hooks(project: &Path) -> String {
    match crate::sync_configured_project_hooks(project) {
        Ok(report) => {
            let crate::GitHookSyncReport {
                status,
                created,
                preserved,
            } = report;
            match status {
                crate::GitHookSyncStatus::Synced => {
                    if created.is_empty() && preserved.is_empty() {
                        "no hooks configured".to_string()
                    } else {
                        format!("{} installed, {} preserved", created.len(), preserved.len())
                    }
                }
                crate::GitHookSyncStatus::Skipped(reason) => match reason {
                    crate::GitHookSyncSkipReason::RemoteProvider { label } => {
                        format!("skipped: {label} provider")
                    }
                    crate::GitHookSyncSkipReason::MissingGitDirectory => {
                        "skipped: no Git repository".to_string()
                    }
                    crate::GitHookSyncSkipReason::CustomHooksPath { path } => {
                        format!("skipped: custom hooks path {}", path.display())
                    }
                },
            }
        }
        Err(error) => {
            tracing::warn!(
                project = %project.display(),
                error = %error,
                "Failed to synchronize configured Git hooks; continuing startup"
            );
            "skipped: hook sync failed".to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitSyncStatus {
    ahead: usize,
    behind: usize,
}

/// The project's commits ahead of and behind its upstream, in the title bar;
/// clicking opens the Git Manager on the project. While it exists the
/// project's repository is fetched in the background
/// ([`crate::start_auto_fetch_task`]).
struct GitSyncIndicator {
    project: Option<PathBuf>,
    status: Option<GitSyncStatus>,
    _auto_fetch: Option<Task<()>>,
}

impl GitSyncIndicator {
    fn new(project: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let auto_fetch = project.as_ref().map(|project| {
            crate::start_auto_fetch_task(project.clone(), window.window_handle(), cx)
        });
        if let Some(poll_path) = project.clone() {
            cx.spawn(async move |this, cx| {
                loop {
                    let path = poll_path.clone();
                    let status = cx
                        .background_executor()
                        .spawn(async move {
                            match crate::probe_repository(&path) {
                                Ok(RepositoryProbe::Repository(state)) => Some(GitSyncStatus {
                                    ahead: state.ahead,
                                    behind: state.behind,
                                }),
                                Ok(RepositoryProbe::NotARepository) | Err(_) => None,
                            }
                        })
                        .await;

                    if this
                        .update(cx, |this, cx| {
                            if this.status != status {
                                this.status = status;
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }

                    cx.background_executor().timer(Duration::from_secs(5)).await;
                }
            })
            .detach();
        }
        Self {
            project,
            status: None,
            _auto_fetch: auto_fetch,
        }
    }

    fn open_manager(&self, cx: &mut App) {
        let project = self.project.clone();
        WindowRegistry::update_global(cx, |_, cx| {
            GitManagerWindow::open_scoped(project, cx);
        });
    }
}

impl Render for GitSyncIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(status) = self.status.as_ref() else {
            return div().into_any_element();
        };
        let behind = status.behind;
        let ahead = status.ahead;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        h_flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("git-sync-behind")
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .bg(danger.opacity(0.18))
                    .text_color(danger)
                    .child(Icon::new(IconName::ArrowDown).size(px(14.)))
                    .child(behind.to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.open_manager(cx))),
            )
            .child(
                div()
                    .id("git-sync-ahead")
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .bg(success.opacity(0.18))
                    .text_color(success)
                    .child(Icon::new(IconName::ArrowUp).size(px(14.)))
                    .child(ahead.to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.open_manager(cx))),
            )
            .into_any_element()
    }
}
