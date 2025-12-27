use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, Icon, IconName,
};
use std::process::Command;

/// Embedded setup scripts (bundled at compile time)
/// Note: These will only work if build.rs has run. For now, using inline scripts.
const SETUP_SCRIPT_PS1: &str = r#"
Write-Host "Pulsar Engine - Dependency Setup (PowerShell)"
Write-Host "This is a placeholder. Run script/setup-dev-environment.ps1 manually."
"#;

const SETUP_SCRIPT_SH: &str = r#"#!/usr/bin/env bash
echo "Pulsar Engine - Dependency Setup (Bash)"
echo "This is a placeholder. Run script/setup-dev-environment.sh manually."
"#;

pub struct DependencySetupWindow {
    setup_tasks: Vec<SetupTask>,
    current_step: usize,
    progress: f32,
    is_running: bool,
    setup_complete: bool,
    setup_error: Option<String>,
}

#[derive(Clone, Debug)]
struct SetupTask {
    name: String,
    description: String,
    status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq)]
enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
}

pub struct SetupComplete;
impl EventEmitter<SetupComplete> for DependencySetupWindow {}

impl DependencySetupWindow {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let tasks = vec![
            SetupTask {
                name: "Checking Rust Installation".to_string(),
                description: "Verifying Rust toolchain is installed".to_string(),
                status: TaskStatus::Pending,
            },
            SetupTask {
                name: "Checking Build Tools".to_string(),
                description: "Verifying C++ compiler and build tools".to_string(),
                status: TaskStatus::Pending,
            },
            SetupTask {
                name: "Checking Platform SDKs".to_string(),
                description: if cfg!(windows) {
                    "Verifying Windows SDK and Visual Studio".to_string()
                } else if cfg!(target_os = "macos") {
                    "Verifying Xcode Command Line Tools".to_string()
                } else {
                    "Verifying system development libraries".to_string()
                },
                status: TaskStatus::Pending,
            },
            SetupTask {
                name: "Installing Missing Dependencies".to_string(),
                description: "Running automated dependency installer".to_string(),
                status: TaskStatus::Pending,
            },
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

            let rust_ok = Self::check_rust();
            
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

            let build_tools_ok = Self::check_build_tools();
            
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

            let sdk_ok = Self::check_platform_sdk();
            
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

            let install_ok = Self::run_setup_script();

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

    fn update_task_status(&mut self, index: usize, status: TaskStatus) {
        if let Some(task) = self.setup_tasks.get_mut(index) {
            task.status = status;
        }
    }

    // Dependency check functions
    fn check_rust() -> bool {
        Command::new("rustc")
            .arg("--version")
            .output()
            .is_ok()
    }

    fn check_build_tools() -> bool {
        #[cfg(target_os = "windows")]
        {
            Command::new("cl")
                .arg("/?")
                .output()
                .is_ok()
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("gcc")
                .arg("--version")
                .output()
                .is_ok()
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("clang")
                .arg("--version")
                .output()
                .is_ok()
        }
    }

    fn check_platform_sdk() -> bool {
        #[cfg(target_os = "windows")]
        {
            Command::new("reg")
                .args(&["query", "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Microsoft SDKs\\Windows\\v10.0"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("xcode-select")
                .arg("-p")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("pkg-config")
                .arg("--version")
                .output()
                .is_ok()
        }
    }

    fn run_setup_script() -> bool {
        #[cfg(target_os = "windows")]
        {
            use std::fs;
            let temp_dir = std::env::temp_dir();
            let script_path = temp_dir.join("pulsar-setup.ps1");
            
            if fs::write(&script_path, SETUP_SCRIPT_PS1).is_err() {
                return false;
            }
            
            let result = Command::new("powershell")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&script_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            
            let _ = fs::remove_file(&script_path);
            result
        }

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::fs;
            use std::os::unix::fs::PermissionsExt;
            
            let temp_dir = std::env::temp_dir();
            let script_path = temp_dir.join("pulsar-setup.sh");
            
            if fs::write(&script_path, SETUP_SCRIPT_SH).is_err() {
                return false;
            }
            
            if let Ok(metadata) = fs::metadata(&script_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&script_path, perms);
            }
            
            let result = Command::new("bash")
                .arg(&script_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            
            let _ = fs::remove_file(&script_path);
            result
        }
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

impl DependencySetupWindow {
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
