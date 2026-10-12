//! Sidecar mode for rebuilding the engine executable that launched the editor.
//!
//! The menu action starts a copy of the current executable from the system
//! temp directory. That copy waits for the editor to exit, builds the engine,
//! then starts the newly built executable with the original arguments.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const SIDECAR_ARG: &str = "--pulsar-bootstrap-sidecar-run";

/// Handle the internal sidecar invocation before normal engine startup.
/// Returns `true` when the process should exit instead of starting the UI.
pub fn run_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != SIDECAR_ARG) {
        return false;
    }

    if let Err(error) = run_sidecar(&args) {
        append_log(&format!("Bootstrap failed: {error}\n"));
    }
    true
}

fn run_sidecar(args: &[OsString]) -> Result<(), String> {
    let parent_pid = arg(args, 2)?.to_string_lossy().parse::<u32>()
        .map_err(|error| format!("Invalid parent process id: {error}"))?;
    let workspace = PathBuf::from(arg(args, 3)?);
    let executable = PathBuf::from(arg(args, 4)?);
    let working_directory = PathBuf::from(arg(args, 5)?);
    let restart_args = args[6..].to_vec();

    wait_for_process_exit(parent_pid)?;

    let profile = executable
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("debug");
    let mut cargo = Command::new("cargo");
    cargo.current_dir(&workspace).args(["build", "-p", "pulsar_engine"]);
    if profile == "release" {
        cargo.arg("--release");
    }

    let log_path = bootstrap_log_path();
    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)
        .map_err(|error| format!("Could not create {}: {error}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .map_err(|error| format!("Could not open bootstrap log: {error}"))?;
    cargo.stdout(Stdio::from(log)).stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cargo.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let status = cargo
        .status()
        .map_err(|error| format!("Could not run cargo build: {error}"))?;
    if !status.success() {
        append_log(&format!("\nCargo build failed with {status}.\n"));
        return Err(format!("Cargo build failed with {status}"));
    }

    let mut restart = Command::new(&executable);
    restart.current_dir(working_directory).args(restart_args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        restart.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    restart
        .spawn()
        .map_err(|error| format!("Could not relaunch {}: {error}", executable.display()))?;

    append_log(&format!("\nEngine rebuilt and relaunched from {}.\n", executable.display()));
    schedule_sidecar_cleanup(std::env::current_exe().ok());
    Ok(())
}

fn arg(args: &[OsString], index: usize) -> Result<&OsString, String> {
    args.get(index)
        .ok_or_else(|| format!("Missing sidecar argument at position {index}"))
}

fn wait_for_process_exit(pid: u32) -> Result<(), String> {
    loop {
        #[cfg(windows)]
        let alive = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
            .map_err(|error| format!("Could not inspect editor process: {error}"))?;

        #[cfg(unix)]
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .map_err(|error| format!("Could not inspect editor process: {error}"))?;

        if !alive {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn bootstrap_log_path() -> PathBuf {
    std::env::temp_dir().join("pulsar-engine-bootstrap.log")
}

fn append_log(message: &str) {
    if let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(bootstrap_log_path())
    {
        let _ = log.write_all(message.as_bytes());
    }
}

fn schedule_sidecar_cleanup(sidecar: Option<PathBuf>) {
    let Some(sidecar) = sidecar else { return };
    #[cfg(windows)]
    {
        let command = format!(
            "timeout /t 2 /nobreak >NUL & del /F /Q \"{}\"",
            sidecar.display()
        );
        let _ = Command::new("cmd")
            .args(["/C", &command])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = fs::remove_file(sidecar);
    }
}
