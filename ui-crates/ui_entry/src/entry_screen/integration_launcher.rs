/// Integration launcher module for opening projects in external tools
///
/// This module provides a unified, robust way to launch external applications like:
/// - Code editors (VS Code, Visual Studio, etc.)
/// - Git GUI tools (GitHub Desktop, GitKraken, etc.)
/// - Terminals (Windows Terminal, PowerShell, etc.)
/// - File managers (Explorer, Finder, etc.)
///
/// All launches use proper Windows APIs to avoid unwanted terminal windows.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationType {
    Editor,
    GitTool,
    Terminal,
    FileManager,
}

#[derive(Debug, Clone)]
pub struct LaunchResult {
    pub success: bool,
    pub error: Option<String>,
}

impl LaunchResult {
    pub fn success() -> Self {
        Self {
            success: true,
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            error: Some(message),
        }
    }
}

/// Launch an editor with the given project path
pub fn launch_editor(command: &str, path: impl AsRef<Path>) -> LaunchResult {
    let path = path.as_ref();

    // Special handling for known editors
    match command {
        "code" => launch_vscode(path),
        "devenv" => launch_visual_studio(path),
        "subl" => launch_sublime(path),
        "idea" | "clion" => launch_jetbrains(command, path),
        "nvim" | "vim" | "emacs" => launch_terminal_editor(command, path),
        "notepad++" => launch_notepadpp(path),
        _ => launch_gui_app(command, path),
    }
}

/// Launch a git GUI tool with the given repository path
pub fn launch_git_tool(command: &str, path: impl AsRef<Path>) -> LaunchResult {
    let path = path.as_ref();

    // Special handling for known git tools
    match command {
        "git" => launch_git_gui(path),
        "github" => launch_github_desktop(path),
        "gitkraken" => launch_gitkraken(path),
        "sourcetree" => launch_sourcetree(path),
        "git-cola" => launch_git_cola(path),
        "lazygit" => launch_lazygit(path),
        "emacs" => launch_magit(path),
        _ => launch_gui_app(command, path),
    }
}

/// Launch a terminal at the given path
pub fn launch_terminal(command: &str, path: impl AsRef<Path>) -> LaunchResult {
    let path = path.as_ref();

    #[cfg(windows)]
    {
        match command {
            "wt" | "default" => launch_windows_terminal(path),
            "pwsh" => launch_powershell(path),
            "cmd" => launch_cmd(path),
            "bash" => launch_git_bash(path),
            "alacritty" => launch_alacritty(path),
            "kitty" => launch_kitty(path),
            _ => launch_windows_terminal(path), // Fallback to Windows Terminal
        }
    }

    #[cfg(target_os = "macos")]
    {
        match command {
            "Terminal" | "default" => launch_macos_terminal(path),
            "iTerm" => launch_iterm(path),
            "alacritty" => launch_alacritty(path),
            "kitty" => launch_kitty(path),
            _ => launch_macos_terminal(path),
        }
    }

    #[cfg(target_os = "linux")]
    {
        match command {
            "gnome-terminal" | "default" => launch_gnome_terminal(path),
            "konsole" => launch_konsole(path),
            "xterm" => launch_xterm(path),
            "alacritty" => launch_alacritty(path),
            "kitty" => launch_kitty(path),
            _ => launch_gnome_terminal(path),
        }
    }
}

/// Launch a file manager at the given path
pub fn launch_file_manager(path: impl AsRef<Path>) -> LaunchResult {
    let path = path.as_ref();

    #[cfg(windows)]
    {
        launch_windows_explorer(path)
    }

    #[cfg(target_os = "macos")]
    {
        launch_finder(path)
    }

    #[cfg(target_os = "linux")]
    {
        launch_nautilus(path)
    }
}

// ============================================================================
// Windows-specific robust launcher
// ============================================================================

#[cfg(windows)]
fn launch_gui_app(command: &str, path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    // Try direct launch first (best for GUI apps that are in PATH)
    match Command::new(command)
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(_) => {
            // Fallback: use PowerShell Start-Process for robustness
            // This handles apps not in PATH and properly detaches them
            let ps_command = format!(
                "Start-Process -FilePath '{}' -ArgumentList '{}' -WindowStyle Hidden",
                command,
                path.to_string_lossy().replace("'", "''")
            );
            
            match Command::new("powershell")
                .args(&["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps_command])
                .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                .spawn()
            {
                Ok(_) => LaunchResult::success(),
                Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
            }
        }
    }
}

#[cfg(not(windows))]
fn launch_gui_app(command: &str, path: &Path) -> LaunchResult {
    match Command::new(command)
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
    }
}

// ============================================================================
// Editor Implementations
// ============================================================================

fn launch_vscode(path: &Path) -> LaunchResult {
    launch_gui_app("code", path)
}

fn launch_visual_studio(path: &Path) -> LaunchResult {
    // Try to find a solution file first
    let sln_file = std::fs::read_dir(path)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "sln")
                        .unwrap_or(false)
                })
                .map(|e| e.path())
        });

    if let Some(sln) = sln_file {
        launch_gui_app("devenv", &sln)
    } else {
        launch_gui_app("devenv", path)
    }
}

fn launch_sublime(path: &Path) -> LaunchResult {
    launch_gui_app("subl", path)
}

fn launch_jetbrains(command: &str, path: &Path) -> LaunchResult {
    launch_gui_app(command, path)
}

fn launch_notepadpp(path: &Path) -> LaunchResult {
    launch_gui_app("notepad++", path)
}

fn launch_terminal_editor(command: &str, path: &Path) -> LaunchResult {
    // Terminal editors need to be launched in a terminal
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;

        match Command::new("cmd")
            .args(&["/K", "cd", "/D", &path.to_string_lossy(), "&&", command])
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
        }
    }

    #[cfg(not(windows))]
    {
        launch_terminal("default", path)
    }
}

// ============================================================================
// Git Tool Implementations
// ============================================================================

fn launch_git_gui(path: &Path) -> LaunchResult {
    #[cfg(windows)]
    {
        // Use start to launch git-gui without console window
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;

        // Use PowerShell to launch git gui without terminal
        let ps_command = format!(
            "Set-Location '{}'; Start-Process git -ArgumentList 'gui' -WindowStyle Hidden",
            path.to_string_lossy().replace("'", "''")
        );
        
        match Command::new("powershell")
            .args(&["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps_command])
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch Git GUI: {}", e)),
        }
    }

    #[cfg(not(windows))]
    {
        match Command::new("git")
            .args(&["gui"])
            .current_dir(path)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch Git GUI: {}", e)),
        }
    }
}

fn launch_github_desktop(path: &Path) -> LaunchResult {
    #[cfg(windows)]
    {
        launch_gui_app("github", path)
    }

    #[cfg(target_os = "macos")]
    {
        match Command::new("open")
            .arg("-a")
            .arg("GitHub Desktop")
            .arg(path)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch GitHub Desktop: {}", e)),
        }
    }

    #[cfg(target_os = "linux")]
    {
        launch_gui_app("github-desktop", path)
    }
}

fn launch_gitkraken(path: &Path) -> LaunchResult {
    launch_gui_app("gitkraken", path)
}

fn launch_sourcetree(path: &Path) -> LaunchResult {
    #[cfg(target_os = "macos")]
    {
        match Command::new("open")
            .arg("-a")
            .arg("Sourcetree")
            .arg(path)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch Sourcetree: {}", e)),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        launch_gui_app("sourcetree", path)
    }
}

fn launch_git_cola(path: &Path) -> LaunchResult {
    launch_gui_app("git-cola", path)
}

fn launch_lazygit(path: &Path) -> LaunchResult {
    // Lazygit is a terminal app
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;

        match Command::new("cmd")
            .args(&["/K", "cd", "/D", &path.to_string_lossy(), "&&", "lazygit"])
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch Lazygit: {}", e)),
        }
    }

    #[cfg(not(windows))]
    {
        match Command::new("lazygit")
            .current_dir(path)
            .spawn()
        {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch Lazygit: {}", e)),
        }
    }
}

fn launch_magit(path: &Path) -> LaunchResult {
    // Magit needs Emacs
    launch_gui_app("emacs", path)
}

// ============================================================================
// Terminal Implementations
// ============================================================================

#[cfg(windows)]
fn launch_windows_terminal(path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;

    match Command::new("wt")
        .args(&["-d", &path.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Windows Terminal: {}", e)),
    }
}

#[cfg(windows)]
fn launch_powershell(path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    match Command::new("pwsh")
        .args(&["-NoExit", "-Command", &format!("cd '{}'", path.display())])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch PowerShell: {}", e)),
    }
}

#[cfg(windows)]
fn launch_cmd(path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    match Command::new("cmd")
        .args(&["/K", "cd", "/D", &path.to_string_lossy()])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Command Prompt: {}", e)),
    }
}

#[cfg(windows)]
fn launch_git_bash(path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    // Git Bash needs a console window
    match Command::new("bash")
        .arg("--login")
        .arg("-i")
        .current_dir(path)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Git Bash: {}", e)),
        }
}

#[cfg(target_os = "macos")]
fn launch_macos_terminal(path: &Path) -> LaunchResult {
    match Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Terminal: {}", e)),
    }
}

#[cfg(target_os = "macos")]
fn launch_iterm(path: &Path) -> LaunchResult {
    match Command::new("open")
        .arg("-a")
        .arg("iTerm")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch iTerm: {}", e)),
    }
}

#[cfg(target_os = "linux")]
fn launch_gnome_terminal(path: &Path) -> LaunchResult {
    match Command::new("gnome-terminal")
        .arg("--working-directory")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch GNOME Terminal: {}", e)),
    }
}

#[cfg(target_os = "linux")]
fn launch_konsole(path: &Path) -> LaunchResult {
    match Command::new("konsole")
        .arg("--workdir")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Konsole: {}", e)),
    }
}

#[cfg(target_os = "linux")]
fn launch_xterm(path: &Path) -> LaunchResult {
    match Command::new("xterm")
        .arg("-e")
        .arg(format!("cd '{}' && bash", path.display()))
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch xterm: {}", e)),
    }
}

fn launch_alacritty(path: &Path) -> LaunchResult {
    match Command::new("alacritty")
        .arg("--working-directory")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Alacritty: {}", e)),
    }
}

fn launch_kitty(path: &Path) -> LaunchResult {
    match Command::new("kitty")
        .arg("--directory")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Kitty: {}", e)),
    }
}

// ============================================================================
// File Manager Implementations
// ============================================================================

#[cfg(windows)]
fn launch_windows_explorer(path: &Path) -> LaunchResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;

    match Command::new("explorer")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Explorer: {}", e)),
    }
}

#[cfg(target_os = "macos")]
fn launch_finder(path: &Path) -> LaunchResult {
    match Command::new("open")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Finder: {}", e)),
    }
}

#[cfg(target_os = "linux")]
fn launch_nautilus(path: &Path) -> LaunchResult {
    match Command::new("nautilus")
        .arg(path)
        .spawn()
    {
        Ok(_) => LaunchResult::success(),
        Err(e) => LaunchResult::error(format!("Failed to launch Nautilus: {}", e)),
    }
}
