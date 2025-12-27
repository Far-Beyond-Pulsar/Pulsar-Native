use gpui::*;
use gpui::prelude::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, Icon, IconName,
};
use crate::entry_screen::{EntryScreen, InstallProgress, InstallStatus};
use std::process::Command;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
const RUSTUP_URL: &str = "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe";

#[cfg(any(target_os = "linux", target_os = "macos"))]
const RUSTUP_URL: &str = "https://sh.rustup.rs";

pub fn render_dependency_setup(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    
    // Use cached dependency status (checked on background thread)
    let (rust_installed, build_tools_installed) = if let Some(ref status) = screen.dependency_status {
        (status.rust_installed, status.build_tools_installed)
    } else {
        // Still checking in background, show as not installed
        (false, false)
    };
    
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.7))
        .child(
            v_flex()
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
                    v_flex()
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
                                        .child("Development Dependencies")
                                )
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Manage required development tools and dependencies")
                        )
                )
                // Status section
                .child(
                    v_flex()
                        .gap_3()
                        .child(render_dependency_item("Rust Toolchain".to_string(), rust_installed, None, theme))
                        .child(render_dependency_item(
                            "C/C++ Compiler".to_string(), 
                            build_tools_installed,
                            screen.dependency_status.as_ref().and_then(|s| s.compiler_info.clone()),
                            theme
                        ))
                )
                // Progress section
                .children(screen.install_progress.clone().map(|progress| {
                    render_install_progress(progress, theme)
                }))
                // Actions
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child("Actions")
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    Button::new("install-deps")
                                        .label("Install Missing Dependencies")
                                        .primary()
                                        .when(screen.install_progress.as_ref().map(|p| {
                                            matches!(p.status, InstallStatus::Downloading | InstallStatus::Installing)
                                        }).unwrap_or(false), |btn| btn.ghost())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            run_setup_script(this, cx);
                                            cx.notify();
                                        }))
                                )
                                .children(rust_installed.then(|| {
                                    Button::new("uninstall-rust")
                                        .label("Uninstall Rust (Testing)")
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            uninstall_rust();
                                            this.check_dependencies_async(cx);
                                            cx.notify();
                                        }))
                                }))
                        )
                )
                // Close button
                .child(
                    h_flex()
                        .justify_end()
                        .child(
                            Button::new("close")
                                .label("Close")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_dependency_setup = false;
                                    cx.notify();
                                }))
                        )
                )
        )
}

fn render_install_progress(progress: InstallProgress, theme: &ui::Theme) -> impl IntoElement {
    let (icon, color, status_text) = match &progress.status {
        InstallStatus::Idle => (IconName::Circle, theme.accent, "Ready".to_string()),
        InstallStatus::Downloading => (IconName::Download, theme.accent, "Downloading installer...".to_string()),
        InstallStatus::Installing => (IconName::Settings, theme.accent, "Installing dependencies...".to_string()),
        InstallStatus::Complete => (IconName::Check, theme.success_foreground, "Installation complete!".to_string()),
        InstallStatus::Error(e) => (IconName::WarningTriangle, gpui::red(), e.clone()),
    };
    
    let progress_val = progress.progress;
    let logs = progress.logs.clone();
    
    v_flex()
        .gap_3()
        .p_4()
        .bg(theme.secondary.opacity(0.2))
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(icon)
                        .size_4()
                        .text_color(color)
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(status_text)
                )
        )
        // Progress bar
        .child(
            div()
                .w_full()
                .h(px(8.))
                .bg(theme.secondary.opacity(0.3))
                .rounded_sm()
                .child(
                    div()
                        .h_full()
                        .rounded_sm()
                        .bg(if matches!(progress.status, InstallStatus::Error(_)) {
                            gpui::red()
                        } else {
                            theme.accent
                        })

                        .w(relative(progress.progress.max(0.0).min(1.0)))
                )
        )
        // Log output
        .child(
            div()
                .w_full()
                .max_h(px(200.))
                .p_2()
                .bg(gpui::black().opacity(0.3))
                .rounded_sm()
                .text_xs()
                .children(
                    progress.logs.iter().rev().take(20).rev().map(|log| {
                        div()
                            .text_color(theme.muted_foreground)
                            .child(log.clone())
                    })
                )
        )
}

fn render_dependency_item(name: String, installed: bool, info: Option<String>, theme: &ui::Theme) -> impl IntoElement {
    let (icon, color, status_text) = if installed {
        (IconName::Check, theme.success_foreground, info.unwrap_or_else(|| "Installed".to_string()))
    } else {
        (IconName::WarningTriangle, gpui::yellow(), "Missing".to_string())
    };
    
    h_flex()
        .gap_3()
        .items_center()
        .p_3()
        .bg(theme.secondary.opacity(0.3))
        .rounded_md()
        .child(
            Icon::new(icon)
                .size_5()
                .text_color(color)
        )
        .child(
            v_flex()
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child(name)
                )
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(status_text)
        )
}

fn run_setup_script(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) {
    // Initialize progress
    screen.install_progress = Some(InstallProgress {
        logs: vec!["Starting installation...".to_string()],
        progress: 0.0,
        status: InstallStatus::Downloading,
    });
    
    let progress = Arc::new(Mutex::new(screen.install_progress.clone().unwrap()));
    let progress_clone = Arc::clone(&progress);
    
    // Spawn directly on background executor instead of using std::thread
    cx.spawn(async move |this, cx| {
        // Run installation on background thread pool
        let result = cx.background_executor().spawn(async move {
            install_rust_with_progress(progress_clone)
        }).await;
        
        if let Err(e) = result {
            let mut prog = progress.lock().unwrap();
            prog.status = InstallStatus::Error(format!("Installation failed: {}", e));
            prog.logs.push(format!("Error: {}", e));
        }
        
        // Poll progress updates
        loop {
            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
            
            let should_break = cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    if let Ok(prog) = progress.lock() {
                        screen.install_progress = Some(prog.clone());
                        cx.notify();
                        
                        matches!(prog.status, InstallStatus::Complete | InstallStatus::Error(_))
                    } else {
                        false
                    }
                }).unwrap_or(false)
            }).unwrap_or(false);
            
            if should_break {
                // Re-check dependencies after install
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.check_dependencies_async(cx);
                    }).ok();
                }).ok();
                break;
            }
        }
    }).detach();
}

fn install_rust_with_progress(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        install_rust_windows(progress)
    }
    
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        install_rust_unix(progress)
    }
}

#[cfg(target_os = "windows")]
fn install_rust_windows(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    
    let exe_path = std::env::temp_dir().join("rustup-init.exe");
    
    // Check if rustup already exists
    let rustup_exists = Command::new("rustup")
        .arg("--version")
        .output()
        .is_ok();
    
    if rustup_exists {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Existing Rust installation detected".to_string());
        prog.logs.push("Stopping all Rust processes...".to_string());
        prog.progress = 0.02;
        drop(prog);
        
        // Kill all rust-related processes
        let rust_processes = [
            "rustc", "cargo", "rustup", "rust-analyzer", "rls",
            "rustfmt", "cargo-clippy", "cargo-fmt", "rustdoc"
        ];
        
        for process in &rust_processes {
            let _ = Command::new("taskkill")
                .args(&["/F", "/IM", &format!("{}.exe", process)])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output();
        }
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Waiting for processes to terminate...".to_string());
            prog.progress = 0.04;
        }
        
        std::thread::sleep(std::time::Duration::from_secs(3));
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Uninstalling old Rust version...".to_string());
            prog.progress = 0.05;
        }
        
        // Try to uninstall existing rustup
        let _ = Command::new("rustup")
            .args(&["self", "uninstall", "-y"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Cleaning up installation directories...".to_string());
            prog.progress = 0.07;
        }
        
        std::thread::sleep(std::time::Duration::from_secs(3));
        
        // Force delete rust directories if they still exist
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let cargo_home = format!("{}/.cargo", home);
        let rustup_home = format!("{}/.rustup", home);
        
        let _ = std::fs::remove_dir_all(&cargo_home);
        let _ = std::fs::remove_dir_all(&rustup_home);
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Old installation cleaned up".to_string());
            prog.progress = 0.09;
        }
    }
    
    // Update progress: Downloading
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Downloading rustup installer...".to_string());
        prog.progress = 0.1;
        prog.status = InstallStatus::Downloading;
    }
    
    // Download using reqwest
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    
    let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push(format!("Downloaded {} bytes", bytes.len()));
        prog.progress = 0.3;
    }
    
    // Write to temp file
    let mut file = std::fs::File::create(&exe_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Running rustup installer with elevated privileges...".to_string());
        prog.logs.push("Please accept the UAC prompt if it appears".to_string());
        prog.progress = 0.4;
        prog.status = InstallStatus::Installing;
    }
    
    // Run installer with admin privileges using runas verb
    // Set environment to reduce thread spawning
    let status = runas::Command::new(&exe_path)
        .args(&["-y", "--default-toolchain", "stable", "--profile", "minimal"])
        .show(false) // Don't show console window
        .status()
        .map_err(|e| e.to_string())?;
    
    if status.success() {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("✅ Rust installed successfully!".to_string());
        prog.logs.push("Adding Windows Defender exclusions...".to_string());
        drop(prog);
        
        // Add Windows Defender exclusions for Rust toolchain
        add_windows_defender_exclusions(&progress);
        
        let mut prog = progress.lock().unwrap();
        prog.progress = 1.0;
        prog.status = InstallStatus::Complete;
    } else {
        return Err(format!("Rustup installer exited with status: {:?}", status));
    }
    
    // Clean up
    let _ = std::fs::remove_file(&exe_path);
    
    Ok(())
}

#[cfg(target_os = "windows")]
fn add_windows_defender_exclusions(progress: &Arc<Mutex<InstallProgress>>) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    
    let home = match std::env::var("USERPROFILE") {
        Ok(h) => h,
        Err(_) => return,
    };
    
    let exclusions = vec![
        format!("{}/.cargo", home),
        format!("{}/.rustup", home),
    ];
    
    let mut prog = progress.lock().unwrap();
    prog.logs.push("Requesting admin privileges to add exclusions...".to_string());
    drop(prog);
    
    // Build PowerShell command to add all exclusions
    let mut ps_commands = Vec::new();
    for path in &exclusions {
        ps_commands.push(format!("Add-MpPreference -ExclusionPath '{}'", path));
    }
    ps_commands.push("Add-MpPreference -ExclusionProcess 'rustc.exe'".to_string());
    ps_commands.push("Add-MpPreference -ExclusionProcess 'cargo.exe'".to_string());
    
    let full_command = ps_commands.join("; ");
    
    // Run PowerShell with elevation to add exclusions
    let result = runas::Command::new("powershell")
        .args(&["-NoProfile", "-Command", &full_command])
        .show(false)
        .status();
    
    let mut prog = progress.lock().unwrap();
    match result {
        Ok(status) if status.success() => {
            prog.logs.push("✅ Windows Defender exclusions added successfully!".to_string());
            prog.logs.push("Cargo builds will no longer be blocked".to_string());
        }
        Ok(_) => {
            prog.logs.push("⚠️ Failed to add Windows Defender exclusions".to_string());
            prog.logs.push("You may need to add them manually in Windows Security".to_string());
        }
        Err(e) => {
            prog.logs.push(format!("⚠️ Could not add exclusions: {}", e));
            prog.logs.push("Builds may be slower due to antivirus scanning".to_string());
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn install_rust_unix(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    
    let script_path = std::env::temp_dir().join("rustup-init.sh");
    
    // Check if rustup already exists
    let rustup_exists = Command::new("rustup")
        .arg("--version")
        .output()
        .is_ok();
    
    if rustup_exists {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Existing Rust installation detected".to_string());
        prog.logs.push("Stopping all Rust processes...".to_string());
        prog.progress = 0.02;
        drop(prog);
        
        // Kill all rust-related processes
        let rust_processes = [
            "rustc", "cargo", "rustup", "rust-analyzer", "rls",
            "rustfmt", "cargo-clippy", "cargo-fmt", "rustdoc"
        ];
        
        for process in &rust_processes {
            let _ = Command::new("pkill")
                .arg(process)
                .output();
        }
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Waiting for processes to terminate...".to_string());
            prog.progress = 0.04;
        }
        
        std::thread::sleep(std::time::Duration::from_secs(3));
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Uninstalling old Rust version...".to_string());
            prog.progress = 0.05;
        }
        
        // Try to uninstall existing rustup
        let _ = Command::new("rustup")
            .args(&["self", "uninstall", "-y"])
            .output();
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Cleaning up installation directories...".to_string());
            prog.progress = 0.07;
        }
        
        std::thread::sleep(std::time::Duration::from_secs(3));
        
        // Force delete rust directories if they still exist
        let home = std::env::var("HOME").unwrap_or_default();
        let cargo_home = format!("{}/.cargo", home);
        let rustup_home = format!("{}/.rustup", home);
        
        let _ = std::fs::remove_dir_all(&cargo_home);
        let _ = std::fs::remove_dir_all(&rustup_home);
        
        {
            let mut prog = progress.lock().unwrap();
            prog.logs.push("Old installation cleaned up".to_string());
            prog.progress = 0.09;
        }
    }
    
    // Update progress: Downloading
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Downloading rustup installer...".to_string());
        prog.progress = 0.1;
        prog.status = InstallStatus::Downloading;
    }
    
    // Download using reqwest
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    
    let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push(format!("Downloaded {} bytes", bytes.len()));
        prog.progress = 0.3;
    }
    
    // Write to temp file
    let mut file = std::fs::File::create(&script_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    
    // Make executable
    let mut perms = std::fs::metadata(&script_path).map_err(|e| e.to_string())?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;
    
    {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("Running rustup installer...".to_string());
        prog.logs.push("May require sudo password".to_string());
        prog.progress = 0.4;
        prog.status = InstallStatus::Installing;
    }
    
    // Run installer silently with -y
    let status = Command::new("sh")
        .args(&[script_path.to_str().unwrap(), "-y", "--default-toolchain", "stable", "--profile", "default"])
        .status()
        .map_err(|e| e.to_string())?;
    
    if status.success() {
        let mut prog = progress.lock().unwrap();
        prog.logs.push("✅ Rust installed successfully!".to_string());
        prog.progress = 1.0;
        prog.status = InstallStatus::Complete;
    } else {
        return Err(format!("Rustup installer exited with status: {:?}", status));
    }
    
    // Clean up
    let _ = std::fs::remove_file(&script_path);
    
    Ok(())
}

fn uninstall_rust() {
    #[cfg(target_os = "windows")]
    {
        // Run rustup self uninstall
        let _ = Command::new("rustup")
            .args(&["self", "uninstall", "-y"])
            .spawn();
    }
    
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // Run rustup self uninstall
        let _ = Command::new("rustup")
            .args(&["self", "uninstall", "-y"])
            .spawn();
    }
}
