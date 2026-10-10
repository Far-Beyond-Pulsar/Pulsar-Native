//! Stopping a cargo command and everything it started.
//!
//! cargo runs rustc, build scripts and linkers as its own children, and
//! `cargo run` starts the game as one. Killing only cargo leaves them running
//! (#1004): a cancelled build keeps compiling, and a stopped game keeps
//! playing. [`kill_tree`] kills the whole tree.
//!
//! The commands stay in the editor's process group, so a Ctrl+C in the
//! editor's terminal still reaches them.

use std::process::{Child, Command, Stdio};

/// Kills `child` and every process it started. The caller still reaps
/// `child` (`Child::wait`).
pub fn kill_tree(child: &mut Child) {
    kill_descendants_and(child.id());
    let _ = child.kill();
}

/// `pid` and all its descendants, `pid` first.
#[cfg(unix)]
fn tree(pid: u32) -> Vec<u32> {
    let mut all = vec![pid];
    let mut next = 0;
    while next < all.len() {
        // `pgrep -P` lists direct children on both Linux and macOS.
        let children = Command::new("pgrep")
            .arg("-P")
            .arg(all[next].to_string())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = children {
            all.extend(
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .filter_map(|pid| pid.parse::<u32>().ok()),
            );
        }
        next += 1;
    }
    all
}

#[cfg(unix)]
fn kill_descendants_and(pid: u32) {
    // Collected before anything dies: a killed process's children are
    // re-parented and can no longer be found from `pid`.
    let pids: Vec<String> = tree(pid).iter().map(u32::to_string).collect();
    let _ = Command::new("kill")
        .arg("-KILL")
        .args(&pids)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
fn kill_descendants_and(pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(any(unix, windows)))]
fn kill_descendants_and(_pid: u32) {}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn alive(pid: u32) -> bool {
        // A zombie still has a /proc entry; a reaped or killed-and-gone one
        // doesn't, and a zombie's state is 'Z'.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| !stat.split_whitespace().nth(2).is_some_and(|state| state == "Z"))
            .unwrap_or(false)
    }

    #[test]
    fn kills_grandchildren_too() {
        // A shell that starts a long sleep and waits on it: the sleep is the
        // grandchild a plain `Child::kill` would leave behind.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh");
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let grandchild: u32 = line.trim().parse().expect("sleep pid");
        assert!(alive(grandchild));

        kill_tree(&mut child);
        let _ = child.wait();

        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(grandchild) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(grandchild), "the grandchild survived kill_tree");
    }
}
