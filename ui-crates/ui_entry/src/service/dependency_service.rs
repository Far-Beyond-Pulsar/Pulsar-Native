use crate::core::types::{DependencyStatus, InstallProgress, InstallStatus};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
const RUSTUP_URL: &str = "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const RUSTUP_URL: &str = "https://sh.rustup.rs";

pub struct DependencyService;

impl DependencyService {
    /// Synchronously check Rust and build tool availability
    pub fn check() -> DependencyStatus {
        use std::process::Command;
        let rust_installed = Command::new("rustc").arg("--version").output().is_ok();
        #[cfg(target_os = "windows")]
        let (build_tools_installed, compiler_info) = {
            if Command::new("cl").arg("/?").output().is_ok() { (true, Some("MSVC".to_string())) }
            else if Command::new("gcc").arg("--version").output().is_ok() { (true, Some("GCC (MinGW)".to_string())) }
            else if Command::new("clang").arg("--version").output().is_ok() { (true, Some("Clang".to_string())) }
            else { (false, None) }
        };
        #[cfg(target_os = "linux")]
        let (build_tools_installed, compiler_info) = {
            if Command::new("gcc").arg("--version").output().is_ok() { (true, Some("GCC".to_string())) }
            else if Command::new("clang").arg("--version").output().is_ok() { (true, Some("Clang".to_string())) }
            else { (false, None) }
        };
        #[cfg(target_os = "macos")]
        let (build_tools_installed, compiler_info) = {
            if Command::new("clang").arg("--version").output().is_ok() { (true, Some("Clang".to_string())) }
            else if Command::new("gcc").arg("--version").output().is_ok() { (true, Some("GCC".to_string())) }
            else { (false, None) }
        };
        DependencyStatus { rust_installed, build_tools_installed, compiler_info }
    }

    pub fn install_rust(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
        #[cfg(target_os = "windows")] { Self::install_windows(progress) }
        #[cfg(any(target_os = "linux", target_os = "macos"))] { Self::install_unix(progress) }
    }

    #[cfg(target_os = "windows")]
    fn install_windows(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        let exe_path = std::env::temp_dir().join("rustup-init.exe");
        let rustup_exists = Command::new("rustup").arg("--version").output().is_ok();

        if rustup_exists {
            {
                let mut p = progress.lock().unwrap();
                p.logs.push("Existing Rust installation detected".to_string());
                p.logs.push("Stopping all Rust processes...".to_string());
                p.progress = 0.02;
            }
            for process in &["rustc","cargo","rustup","rust-analyzer","rls","rustfmt","cargo-clippy","cargo-fmt","rustdoc"] {
                let _ = Command::new("taskkill").args(["/F", "/IM", &format!("{}.exe", process)]).creation_flags(0x08000000).output();
            }
            { let mut p = progress.lock().unwrap(); p.progress = 0.04; }
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = Command::new("rustup").args(["self", "uninstall", "-y"]).creation_flags(0x08000000).output();
            { let mut p = progress.lock().unwrap(); p.progress = 0.07; }
            std::thread::sleep(std::time::Duration::from_secs(3));
            let home = std::env::var("USERPROFILE").unwrap_or_default();
            let _ = std::fs::remove_dir_all(format!("{}/.cargo", home));
            let _ = std::fs::remove_dir_all(format!("{}/.rustup", home));
            { let mut p = progress.lock().unwrap(); p.logs.push("Old installation cleaned up".to_string()); p.progress = 0.09; }
        }

        {
            let mut p = progress.lock().unwrap();
            p.logs.push("Downloading rustup installer...".to_string());
            p.progress = 0.1;
            p.status = InstallStatus::Downloading;
        }

        let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
        let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
        let bytes = response.bytes().map_err(|e| e.to_string())?;
        { let mut p = progress.lock().unwrap(); p.logs.push(format!("Downloaded {} bytes", bytes.len())); p.progress = 0.3; }

        let mut file = std::fs::File::create(&exe_path).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(file);

        {
            let mut p = progress.lock().unwrap();
            p.logs.push("Running rustup installer...".to_string());
            p.progress = 0.4;
            p.status = InstallStatus::Installing;
        }

        let status = runas::Command::new(&exe_path)
            .args(&["-y", "--default-toolchain", "stable", "--profile", "minimal"])
            .show(false).status().map_err(|e| e.to_string())?;

        if status.success() {
            { let mut p = progress.lock().unwrap(); p.logs.push("Rust installed successfully!".to_string()); }
            { let mut p = progress.lock().unwrap(); p.progress = 1.0; p.status = InstallStatus::Complete; }
        } else {
            return Err(format!("Rustup installer exited with status: {:?}", status));
        }
        let _ = std::fs::remove_file(&exe_path);
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn install_unix(progress: Arc<Mutex<InstallProgress>>) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let script_path = std::env::temp_dir().join("rustup-init.sh");
        let rustup_exists = Command::new("rustup").arg("--version").output().is_ok();

        if rustup_exists {
            { let mut p = progress.lock().unwrap(); p.logs.push("Existing Rust installation detected".to_string()); p.progress = 0.02; }
            for process in &["rustc","cargo","rustup","rust-analyzer","rls","rustfmt","cargo-clippy","cargo-fmt","rustdoc"] {
                let _ = Command::new("pkill").arg(process).output();
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = Command::new("rustup").args(&["self", "uninstall", "-y"]).output();
            std::thread::sleep(std::time::Duration::from_secs(3));
            let home = std::env::var("HOME").unwrap_or_default();
            let _ = std::fs::remove_dir_all(format!("{}/.cargo", home));
            let _ = std::fs::remove_dir_all(format!("{}/.rustup", home));
            { let mut p = progress.lock().unwrap(); p.logs.push("Old installation cleaned up".to_string()); p.progress = 0.09; }
        }

        { let mut p = progress.lock().unwrap(); p.logs.push("Downloading rustup installer...".to_string()); p.progress = 0.1; p.status = InstallStatus::Downloading; }
        let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
        let response = client.get(RUSTUP_URL).send().map_err(|e| e.to_string())?;
        let bytes = response.bytes().map_err(|e| e.to_string())?;
        { let mut p = progress.lock().unwrap(); p.logs.push(format!("Downloaded {} bytes", bytes.len())); p.progress = 0.3; }
        let mut file = std::fs::File::create(&script_path).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(file);
        let mut perms = std::fs::metadata(&script_path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;
        { let mut p = progress.lock().unwrap(); p.logs.push("Running rustup installer...".to_string()); p.progress = 0.4; p.status = InstallStatus::Installing; }
        let status = Command::new("sh").args(&[script_path.to_str().unwrap(), "-y", "--default-toolchain", "stable", "--profile", "default"])
            .status().map_err(|e| e.to_string())?;
        if status.success() { let mut p = progress.lock().unwrap(); p.logs.push("Rust installed successfully!".to_string()); p.progress = 1.0; p.status = InstallStatus::Complete; }
        else { return Err(format!("Rustup installer exited with status: {:?}", status)); }
        let _ = std::fs::remove_file(&script_path);
        Ok(())
    }
}
