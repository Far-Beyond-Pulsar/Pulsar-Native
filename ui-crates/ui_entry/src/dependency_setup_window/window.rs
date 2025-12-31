//! Main dependency setup window implementation.
//!
//! This module contains the primary window component that orchestrates
//! the dependency checking and installation workflow, including UI rendering.

use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, Icon, IconName,
};

use super::checks::{check_rust, check_build_tools, check_platform_sdk};
use super::installer::run_setup_script;
use super::task::{SetupTask, TaskStatus};

/// Main window for dependency setup process.
///
/// This component provides an interactive UI that guides users through
/// checking and installing required development dependencies. It runs
/// validation checks asynchronously and provides real-time progress feedback.
///
/// # Workflow
///
/// 1. User opens the dependency setup window
/// 2. Click "Start Setup" to begin validation
/// 3. System checks for Rust, build tools, and platform SDKs
/// 4. If all checks pass, setup completes immediately
/// 5. If checks fail, automated installer attempts to fix issues
/// 6. User is notified of success or failure
///
/// # Events
///
/// - [`SetupComplete`] - Emitted when all dependencies are successfully installed
pub struct DependencySetupWindow {
    /// List of setup tasks to execute.
    setup_tasks: Vec<SetupTask>,
    
    /// Index of the currently executing task.
    current_step: usize,
    
    /// Overall progress (0.0 to 1.0).
    progress: f32,
    
    /// Whether setup is currently running.
    is_running: bool,
    
    /// Whether setup completed successfully.
    setup_complete: bool,
    
    /// Error message if setup failed.
    setup_error: Option<String>,
}

/// Event emitted when dependency setup completes successfully.
pub struct SetupComplete;

impl EventEmitter<SetupComplete> for DependencySetupWindow {}

impl DependencySetupWindow {
    /// Creates a new dependency setup window.
    ///
    /// Initializes the window with a list of platform-specific setup tasks
    /// that need to be validated and potentially installed.
    ///
    /// # Arguments
    ///
    /// * `_window` - The GPUI window context (unused in initialization)
    /// * `_cx` - The GPUI context (unused in initialization)
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let tasks = vec![
            SetupTask::new(
                "Checking Rust Installation",
                "Verifying Rust toolchain is installed",
            ),
            SetupTask::new(
                "Checking Build Tools",
                "Verifying C++ compiler and build tools",
            ),
            SetupTask::new(
                "Checking Platform SDKs",
                if cfg!(windows) {
                    "Verifying Windows SDK and Visual Studio"
                } else if cfg!(target_os = "macos") {
                    "Verifying Xcode Command Line Tools"
                } else {
                    "Verifying system development libraries"
                },
            ),
            SetupTask::new(
                "Installing Missing Dependencies",
                "Running automated dependency installer",
            ),
        ];

        Self {
            setup_tasks: tasks,
            current_step: 0,
            progress: 0.0,
            is_running: false,
            setup_complete: false,
            setup_error: None,
        }
    }

    /// Starts the dependency setup workflow.
    ///
    /// Executes all validation and installation tasks asynchronously,
    /// updating the UI with progress as each step completes.
    ///
    /// # Behavior
    ///
    /// - Does nothing if setup is already running
    /// - Resets progress and state before starting
    /// - Executes tasks sequentially with UI feedback
    /// - Emits [`SetupComplete`] event on success
    /// - Updates task statuses in real-time
    ///
    /// # Arguments
    ///
    /// * `cx` - The GPUI context for spawning background tasks
    pub fn start_setup(&mut self, cx: &mut Context<Self>) {
        if self.is_running {
            return;
        }

        self.is_running = true;
        self.current_step = 0;
        self.progress = 0.0;
        cx.notify();

        // Run checks and setup in background
        let view = cx.entity().downgrade();

        cx.spawn(async move |_this, mut cx| {
            // Step 1: Check Rust
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        this.update_task_status(0, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let rust_ok = check_rust();
            
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        let status = if rust_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("Rust not found".to_string())
                        };
                        this.update_task_status(0, status);
                        this.current_step = 1;
                        this.progress = 0.25;
                        cx.notify();
                    });
                }
            });

            // Small delay for UI feedback
            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;

            // Step 2: Check Build Tools
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        this.update_task_status(1, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let build_tools_ok = check_build_tools();
            
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        let status = if build_tools_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("Build tools not found".to_string())
                        };
                        this.update_task_status(1, status);
                        this.current_step = 2;
                        this.progress = 0.5;
                        cx.notify();
                    });
                }
            });

            cx.background_executor().timer(std::time::Duration::from_millis(300)).await;

            // Step 3: Check Platform SDKs
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        this.update_task_status(2, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let sdk_ok = check_platform_sdk();
            
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        let status = if sdk_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("SDK not found".to_string())
                        };
                        this.update_task_status(2, status);
                        this.current_step = 3;
                        this.progress = 0.75;
                        cx.notify();
                    });
                }
            });

            // If everything passed, we're done
            if rust_ok && build_tools_ok && sdk_ok {
                cx.update(|cx| {
                    if let Some(view) = view.upgrade() {
                        let _ = view.update(cx, |this, cx| {
                            this.update_task_status(3, TaskStatus::Completed);
                            this.progress = 1.0;
                            this.setup_complete = true;
                            this.is_running = false;
                            cx.emit(SetupComplete);
                            cx.notify();
                        });
                    }
                });
                return;
            }

            // Step 4: Install missing dependencies
            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        this.update_task_status(3, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let install_ok = run_setup_script();

            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(cx, |this, cx| {
                        let status = if install_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("Setup script failed".to_string())
                        };
                        this.update_task_status(3, status);
                        this.progress = 1.0;
                        this.setup_complete = install_ok;
                        this.is_running = false;
                        
                        if install_ok {
                            cx.emit(SetupComplete);
                        }
                        cx.notify();
                    });
                }
            });
        }).detach();
    }

    /// Updates the status of a specific task by index.
    ///
    /// # Arguments
    ///
    /// * `index` - The zero-based index of the task to update
    /// * `status` - The new status to set
    fn update_task_status(&mut self, index: usize, status: TaskStatus) {
        if let Some(task) = self.setup_tasks.get_mut(index) {
            task.status = status;
        }
    }

    /// Renders a single setup task as a UI element.
    ///
    /// Creates a visual representation of the task showing its name,
    /// description, current status icon, and any error messages.
    ///
    /// # Arguments
    ///
    /// * `task` - The task to render
    /// * `theme` - The current UI theme for styling
    ///
    /// # Returns
    ///
    /// A GPUI element representing the task's current state.
    fn render_task(&self, task: &SetupTask, theme: &ui::Theme) -> impl IntoElement {
        let (icon_name, icon_color) = match &task.status {
            TaskStatus::Pending => (IconName::Circle, theme.muted_foreground),
            TaskStatus::InProgress => (IconName::Loader, theme.accent),
            TaskStatus::Completed => (IconName::Check, theme.success_foreground),
            TaskStatus::Failed(_) => (IconName::WarningTriangle, gpui::red()),
        };

        h_flex()
            .gap_3()
            .items_start()
            .p_3()
            .bg(theme.secondary.opacity(0.3))
            .rounded_md()
            .child(
                Icon::new(icon_name)
                    .size_5()
                    .text_color(icon_color)
            )
            .child(
                v_flex()
                    .gap_1()
                    .flex_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(task.name.clone())
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(task.description.clone())
                    )
                    .children(
                        if let TaskStatus::Failed(ref err) = task.status {
                            Some(div()
                                .text_xs()
                                .text_color(gpui::red())
                                .child(format!("Error: {}", err)))
                        } else {
                            None
                        }
                    )
            )
    }
}

impl Render for DependencySetupWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        // Calculate relative width for progress bar
        let relative_w = relative(match self.progress {
            v if v < 0.0 => 0.0,
            v if v > 1.0 => 1.0,
            v => v,
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(600.))
                    .gap_6()
                    .p_8()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .shadow_lg()
                    // Header
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        Icon::new(IconName::Settings)
                                            .size_6()
                                            .text_color(theme.accent)
                                    )
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .child("Dependency Setup")
                                    )
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Checking and installing required development dependencies")
                            )
                    )
                    // Progress bar
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .h(px(8.))
                                    .w_full()
                                    .bg(theme.secondary)
                                    .rounded(px(4.))
                                    .relative()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(4.))
                                            .bg(theme.accent)
                                            .w(relative_w)
                                    )
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}% Complete", (self.progress * 100.0) as u32))
                            )
                    )
                    // Task list
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(self.setup_tasks.iter().map(|task| {
                                self.render_task(task, theme)
                            }))
                    )
                    // Error message
                    .children(self.setup_error.as_ref().map(|error| {
                        div()
                            .p_3()
                            .bg(gpui::red().opacity(0.1))
                            .border_1()
                            .border_color(gpui::red())
                            .rounded_md()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(gpui::red())
                                    .child(error.clone())
                            )
                    }))
                    // Action buttons
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_3()
                            .children((!self.is_running).then(|| {
                                Button::new("cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .on_click(cx.listener(|_, _, _, _| {
                                        // TODO: Cancel setup  
                                    }))
                            }))
                            .children((!self.is_running && !self.setup_complete).then(|| {
                                    Button::new("start")
                                        .label("Start Setup")
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_setup(cx);
                                        }))
                            }))
                            .children(self.is_running.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Installing...")
                            }))
                            .children(self.setup_complete.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(theme.success_foreground)
                                    .child("✅ Complete")
                            }))
                    )
            )
    }
}
