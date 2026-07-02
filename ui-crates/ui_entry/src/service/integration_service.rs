use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationType { Editor, GitTool, Terminal, FileManager }

pub struct LaunchResult { pub success: bool, pub error: Option<String> }

impl LaunchResult {
    pub fn success() -> Self { Self { success: true, error: None } }
    pub fn error(message: String) -> Self { Self { success: false, error: Some(message) } }
}

pub struct IntegrationService;

impl IntegrationService {
    pub fn launch_editor(command: &str, path: impl AsRef<Path>) -> LaunchResult {
        match command {
            "code" => Self::launch_gui("code", path.as_ref()),
            "devenv" => Self::launch_visual_studio(path.as_ref()),
            "subl" => Self::launch_gui("subl", path.as_ref()),
            "nvim" | "vim" | "emacs" => Self::launch_terminal_editor(command, path.as_ref()),
            "notepad++" => Self::launch_gui("notepad++", path.as_ref()),
            _ => Self::launch_gui(command, path.as_ref()),
        }
    }

    pub fn launch_git_tool(command: &str, path: impl AsRef<Path>) -> LaunchResult {
        match command {
            "github" => Self::launch_gui("github", path.as_ref()),
            "gitkraken" => Self::launch_gui("gitkraken", path.as_ref()),
            "sourcetree" => Self::launch_gui("sourcetree", path.as_ref()),
            "git-cola" => Self::launch_gui("git-cola", path.as_ref()),
            "lazygit" => Self::launch_terminal_editor("lazygit", path.as_ref()),
            "emacs" => Self::launch_gui("emacs", path.as_ref()),
            _ => Self::launch_gui("git", path.as_ref()),
        }
    }

    pub fn launch_file_manager(path: impl AsRef<Path>) -> LaunchResult {
        #[cfg(windows)] { Self::launch_gui("explorer", path.as_ref()) }
        #[cfg(target_os = "macos")] { Self::launch_gui("open", path.as_ref()) }
        #[cfg(target_os = "linux")] { Self::launch_gui("nautilus", path.as_ref()) }
    }

    #[cfg(windows)]
    fn launch_gui(command: &str, path: &Path) -> LaunchResult {
        use std::os::windows::process::CommandExt;
        const FLAGS: u32 = 0x08000000 | 0x00000008 | 0x00000200;
        match Command::new(command).arg(path).creation_flags(FLAGS).spawn() {
            Ok(_) => LaunchResult::success(),
            Err(_) => {
                let ps = format!("Start-Process -FilePath '{}' -ArgumentList '{}' -WindowStyle Hidden", command, path.to_string_lossy().replace("'", "''"));
                match Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps])
                    .creation_flags(0x08000000 | 0x00000008).spawn() {
                    Ok(_) => LaunchResult::success(),
                    Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
                }
            }
        }
    }

    #[cfg(not(windows))]
    fn launch_gui(command: &str, path: &Path) -> LaunchResult {
        match Command::new(command).arg(path).spawn() {
            Ok(_) => LaunchResult::success(),
            Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
        }
    }

    fn launch_visual_studio(path: &Path) -> LaunchResult {
        let sln = std::fs::read_dir(path).ok().and_then(|e| e.filter_map(|e| e.ok())
            .find(|e| e.path().extension().and_then(|e| e.to_str()) == Some("sln")).map(|e| e.path()));
        Self::launch_gui("devenv", sln.as_deref().unwrap_or(path))
    }

    fn launch_terminal_editor(command: &str, path: &Path) -> LaunchResult {
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            match Command::new("cmd").args(["/K", "cd", "/D", &path.to_string_lossy(), "&&", command])
                .creation_flags(0x00000010).spawn() {
                Ok(_) => LaunchResult::success(),
                Err(e) => LaunchResult::error(format!("Failed to launch {}: {}", command, e)),
            }
        }
        #[cfg(not(windows))] { Self::launch_gui(command, path) }
    }
}
