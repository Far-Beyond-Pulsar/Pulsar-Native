//! # Process Module
//!
//! Process control and shell command execution for the Pulsar visual programming system.
//!
//! This module provides process and shell operations including:
//! - Process control (get ID, exit, abort)
//! - Shell command execution (synchronous and asynchronous)
//! - Command piping and chaining
//! - Command timeout management
//! - Exit code handling
//! - Executable path resolution

use crate::blueprint;

// =============================================================================
// Process Control Operations
// =============================================================================

/// Get the current process ID.
///
/// # Returns
/// The process ID as a u32
///
/// # Example
/// If the current process ID is 12345, the output will be 12345.
///
/// # Notes
/// The process ID is assigned by the operating system and is unique among running processes.
/// # Process ID
/// Returns the current process ID (PID).
#[blueprint(type: NodeTypes::pure, category: "Process", color: "#E74C3C")]
pub fn process_id() -> u32 {
    std::process::id()
}

/// Exit the current process with a specified exit code.
///
/// # Inputs
/// - `code`: The exit code to return (integer)
///
/// # Returns
/// This function does not return; it terminates the process
///
/// # Example
/// If `code` is 0, the process exits with code 0 (success).
/// If `code` is 1, the process exits with code 1 (error).
///
/// # Notes
/// Use with caution: this will immediately terminate the process and no further nodes will execute.
/// # Process Exit
/// Exits the current process with the specified exit code.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn process_exit(code: i64) -> ! {
    std::process::exit(code as i32)
}

/// Abort the current process immediately.
///
/// # Returns
/// This function does not return; the process is terminated immediately
///
/// # Example
/// When this function is executed, the process will exit with an abort signal (usually exit code 134 or similar).
///
/// # Notes
/// This is a hard abort: no destructors are run, files may not be flushed, and resources may be leaked.
/// Use only for unrecoverable errors or debugging.
/// # Process Abort
/// Aborts the current process immediately without cleanup.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn process_abort() -> ! {
    std::process::abort()
}

// =============================================================================
// Shell Command Execution
// =============================================================================

/// Execute a shell command and return the output.
///
/// # Inputs
/// - `command`: The shell command to execute (string)
///
/// # Returns
/// The standard output of the command if successful, or an error message if the command fails
///
/// # Example
/// If `command` is "echo Hello", the output will be Ok("Hello\n").
///
/// # Notes
/// On Windows, the command is executed using `cmd /C`. On Unix-like systems, it uses `sh -c`.
/// Use with caution, as executing arbitrary shell commands can be a security risk.
/// # Shell Execute
/// Executes a shell command and returns the standard output.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_execute(command: String) -> Result<String, String> {
    use std::process::Command;

    let output = if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", &command]).output()
    } else {
        Command::new("sh").args(["-c", &command]).output()
    };

    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).to_string())
            }
        }
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}

/// Execute a shell command asynchronously and return the process ID.
///
/// # Inputs
/// - `command`: The shell command to execute (string)
///
/// # Returns
/// The process ID of the spawned process if successful, or an error message if spawning fails
///
/// # Example
/// If `command` is "sleep 10", the function will start the sleep command in the background and return its PID.
///
/// # Notes
/// The command runs asynchronously; you are responsible for managing the process lifecycle.
/// # Shell Execute Async
/// Executes a shell command asynchronously and returns the process ID.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_execute_async(command: String) -> Result<u32, String> {
    use std::process::Command;

    let child = if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", &command]).spawn()
    } else {
        Command::new("sh").args(["-c", &command]).spawn()
    };

    match child {
        Ok(child) => Ok(child.id()),
        Err(e) => Err(format!("Failed to spawn process: {}", e)),
    }
}

/// Pipe the output of one shell command to another.
///
/// # Inputs
/// - `command1`: The first shell command to execute (string)
/// - `command2`: The second shell command to execute, which receives the output of the first as input (string)
///
/// # Returns
/// The standard output of the second command if successful, or an error message if either command fails
///
/// # Example
/// If `command1` is "echo hello" and `command2` is "tr a-z A-Z", the output will be "HELLO".
///
/// # Notes
/// This implementation uses the system shell to pipe commands together.
/// # Shell Pipe
/// Pipes the output of one command to another command.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_pipe(command1: String, command2: String) -> Result<String, String> {
    use std::process::{Command, Stdio};

    // First command
    let cmd1 = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", &command1])
            .stdout(Stdio::piped())
            .spawn()
    } else {
        Command::new("sh")
            .args(["-c", &command1])
            .stdout(Stdio::piped())
            .spawn()
    };

    let child1 = match cmd1 {
        Ok(child) => child,
        Err(e) => return Err(format!("Failed to execute first command: {}", e)),
    };

    // Second command
    let cmd2 = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", &command2])
            .stdin(child1.stdout.unwrap())
            .output()
    } else {
        Command::new("sh")
            .args(["-c", &command2])
            .stdin(child1.stdout.unwrap())
            .output()
    };

    match cmd2 {
        Ok(output) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).to_string())
            }
        }
        Err(e) => Err(format!("Failed to execute second command: {}", e)),
    }
}

/// Execute a command with a timeout in seconds.
///
/// # Inputs
/// - `command`: The shell command to execute (string)
/// - `timeout`: The timeout duration in seconds (integer)
///
/// # Returns
/// The standard output of the command if successful and completed within the timeout,
/// or an error message if the command fails or times out
///
/// # Example
/// If `command` is "sleep 2" and `timeout` is 1, the command will be killed after 1 second
/// and the output will be an error message ("Command timed out").
///
/// # Notes
/// The timeout is enforced by sleeping and checking process status.
/// # Shell Timeout
/// Executes a command with a timeout (in seconds).
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_timeout(command: String, timeout: i64) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let child = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", &command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    } else {
        Command::new("sh")
            .args(["-c", &command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };

    let mut child = match child {
        Ok(child) => child,
        Err(e) => return Err(format!("Failed to spawn process: {}", e)),
    };

    // Simple timeout simulation
    std::thread::sleep(Duration::from_millis(100)); // Small delay to let process start

    match child.try_wait() {
        Ok(Some(status)) => {
            let output = child.wait_with_output().unwrap();
            if status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).to_string())
            }
        }
        Ok(None) => {
            // Process still running - wait for timeout
            let timeout_secs = timeout as u64;
            std::thread::sleep(Duration::from_secs(timeout_secs));

            match child.try_wait() {
                Ok(Some(status)) => {
                    let output = child.wait_with_output().unwrap();
                    if status.success() {
                        Ok(String::from_utf8_lossy(&output.stdout).to_string())
                    } else {
                        Err(String::from_utf8_lossy(&output.stderr).to_string())
                    }
                }
                _ => {
                    let _ = child.kill();
                    Err("Command timed out".to_string())
                }
            }
        }
        Err(e) => Err(format!("Error checking process status: {}", e)),
    }
}

/// Execute a command and return just the exit code.
///
/// # Inputs
/// - `command`: The shell command to execute (string)
///
/// # Returns
/// The exit code of the command if successful, or an error message if the command fails to execute
///
/// # Example
/// If `command` is "exit 42", the output will be Ok(42).
///
/// # Notes
/// The exit code is returned as an integer. If the command does not return an exit code, -1 is used.
/// # Shell Exit Code
/// Executes a command and returns only the exit code.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_exit_code(command: String) -> Result<i32, String> {
    use std::process::Command;

    let status = if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", &command]).status()
    } else {
        Command::new("sh").args(["-c", &command]).status()
    };

    match status {
        Ok(status) => Ok(status.code().unwrap_or(-1)),
        Err(e) => Err(format!("Failed to execute command: {}", e)),
    }
}

/// Find the path to an executable command.
///
/// # Inputs
/// - `command`: The name of the command to search for (string)
///
/// # Returns
/// The full path to the executable if found, or an error message if not found
///
/// # Example
/// If `command` is "python", the output will be the path to the Python executable,
/// such as "/usr/bin/python" or "C:\\Python39\\python.exe".
///
/// # Notes
/// On Windows, this function uses the `where` command; on Unix-like systems, it uses `which`.
/// # Shell Which
/// Finds the path to an executable command in the system PATH.
#[blueprint(type: NodeTypes::fn_, category: "Process", color: "#E74C3C")]
pub fn shell_which(command: String) -> Result<String, String> {
    use std::process::Command;

    let output = if cfg!(target_os = "windows") {
        Command::new("where").arg(&command).output()
    } else {
        Command::new("which").arg(&command).output()
    };

    match output {
        Ok(output) => {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if path.is_empty() {
                    Err(format!("Command '{}' not found", command))
                } else {
                    Ok(path)
                }
            } else {
                Err(format!("Command '{}' not found", command))
            }
        }
        Err(e) => Err(format!("Failed to search for command: {}", e)),
    }
}
