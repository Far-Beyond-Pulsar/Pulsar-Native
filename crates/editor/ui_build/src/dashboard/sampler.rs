//! Sampling the build's processes and the machine: the running cargo
//! command and everything it started (rustc per crate, build scripts,
//! linkers), each with its CPU and memory, and the machine's CPU and memory.
//!
//! A [`Sampler`] keeps its `sysinfo::System` between samples: a process's CPU
//! use is measured between two refreshes, so the first sample of a process
//! reads 0 %.

use std::collections::{HashMap, HashSet};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// What a process in the build's tree is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessKind {
    /// Compiling one crate.
    Rustc,
    /// A crate's build script.
    BuildScript,
    Linker,
    Cargo,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRow {
    pub pid: u32,
    pub kind: ProcessKind,
    /// The crate for rustc and build scripts, else the program's name.
    pub label: String,
    /// Percent of one core (above 100 when it uses several).
    pub cpu: f32,
    pub memory: u64,
    /// Seconds it has been running.
    pub run_time: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    /// The build's processes, busiest first.
    pub processes: Vec<ProcessRow>,
    /// The build's processes together, in percent of the whole machine.
    pub build_cpu: f32,
    pub build_memory: u64,
    /// The machine's CPU use, in percent.
    pub system_cpu: f32,
    pub system_memory_used: u64,
    pub system_memory_total: u64,
}

pub struct Sampler {
    system: System,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    /// One sample: the process tree under `root` (the running cargo
    /// command; `None` between commands), and the machine.
    pub fn sample(&mut self, root: Option<u32>) -> Sample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let cores = self.system.cpus().len().max(1) as f32;
        let mut sample = Sample {
            system_cpu: self.system.global_cpu_usage(),
            system_memory_used: self.system.used_memory(),
            system_memory_total: self.system.total_memory(),
            ..Sample::default()
        };
        let Some(root) = root else {
            return sample;
        };

        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
        let parents: Vec<(u32, Option<u32>)> = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| (pid.as_u32(), process.parent().map(Pid::as_u32)))
            .collect();

        for pid in descendants(root, &parents) {
            let Some(process) = self.system.process(Pid::from_u32(pid)) else {
                continue;
            };
            let cmd: Vec<String> = process
                .cmd()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let exe = process.exe().map(|p| p.to_string_lossy().into_owned());
            let (kind, label) = classify(&process.name().to_string_lossy(), &cmd, exe.as_deref());
            sample.build_cpu += process.cpu_usage() / cores;
            sample.build_memory += process.memory();
            sample.processes.push(ProcessRow {
                pid,
                kind,
                label,
                cpu: process.cpu_usage(),
                memory: process.memory(),
                run_time: process.run_time(),
            });
        }
        sample.processes.sort_by(|a, b| {
            b.cpu
                .total_cmp(&a.cpu)
                .then(b.memory.cmp(&a.memory))
                .then(a.pid.cmp(&b.pid))
        });
        sample
    }
}

/// `root` and every process descended from it, from `(pid, parent)` pairs,
/// `root` first. A pid recycled into a loop is visited once.
pub fn descendants(root: u32, parents: &[(u32, Option<u32>)]) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, parent) in parents {
        if let Some(parent) = parent.filter(|parent| *parent != pid) {
            children.entry(parent).or_default().push(pid);
        }
    }
    let alive: HashSet<u32> = parents.iter().map(|(pid, _)| *pid).collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if alive.contains(&pid) {
            out.push(pid);
        }
        if let Some(kids) = children.get(&pid) {
            let mut kids = kids.clone();
            kids.sort_unstable_by(|a, b| b.cmp(a));
            queue.extend(kids);
        }
    }
    out
}

/// What a process is and how to name it, from its program name, command
/// line and executable path.
pub fn classify(name: &str, cmd: &[String], exe: Option<&str>) -> (ProcessKind, String) {
    let program = program_name(name);
    if program == "cargo" {
        let sub = cmd.iter().skip(1).find(|arg| !arg.starts_with('-'));
        return (
            ProcessKind::Cargo,
            sub.map_or_else(|| "cargo".to_owned(), |sub| format!("cargo {sub}")),
        );
    }
    if let Some(krate) = flag_value(cmd, "--crate-name") {
        return (ProcessKind::Rustc, krate.to_owned());
    }
    if program == "rustc" {
        return (ProcessKind::Rustc, "rustc".to_owned());
    }
    if program.starts_with("build-script-") {
        // target/<profile>/build/<crate>-<hash>/build-script-build
        let krate = exe
            .and_then(|exe| exe.replace('\\', "/").rsplit('/').nth(1).map(str::to_owned))
            .map(|dir| strip_hash(&dir).to_owned())
            .unwrap_or_else(|| program.clone());
        return (ProcessKind::BuildScript, krate);
    }
    const LINKERS: &[&str] = &[
        "link", "lld-link", "rust-lld", "ld", "ld.lld", "ld64", "ld64.lld", "mold", "cc", "c++",
        "gcc", "g++", "clang", "clang++", "cl", "lib", "ar",
    ];
    if LINKERS.contains(&program.as_str()) {
        return (ProcessKind::Linker, program);
    }
    (ProcessKind::Other, program)
}

/// The program's name, lowercase, without `.exe`.
fn program_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

fn flag_value<'a>(cmd: &'a [String], flag: &str) -> Option<&'a str> {
    let at = cmd.iter().position(|arg| arg == flag)?;
    cmd.get(at + 1).map(String::as_str)
}

/// `serde-1a2b3c4d5e6f7a8b` → `serde`: Cargo names build directories
/// `<crate>-<16 hex digits>`.
fn strip_hash(dir: &str) -> &str {
    match dir.rsplit_once('-') {
        Some((krate, hash)) if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) => krate,
        _ => dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_tree_holds_only_the_roots_descendants() {
        // 10 cargo -> 11 rustc -> 13 link; 10 -> 12 build script; 20 is
        // unrelated; 30 is a dead pid's child; 40 is its own parent.
        let parents = [
            (10, Some(1)),
            (11, Some(10)),
            (12, Some(10)),
            (13, Some(11)),
            (20, Some(1)),
            (31, Some(30)),
            (40, Some(40)),
        ];
        assert_eq!(descendants(10, &parents), [10, 11, 13, 12]);
        assert_eq!(descendants(30, &parents), [31], "a gone root still finds its children");
        assert_eq!(descendants(40, &parents), [40]);
        assert!(descendants(99, &parents).is_empty());
    }

    #[test]
    fn processes_are_named_by_what_they_build() {
        assert_eq!(
            classify(
                "rustc.exe",
                &args(&["rustc", "--crate-name", "pulsar_game", "--edition=2021"]),
                None
            ),
            (ProcessKind::Rustc, "pulsar_game".to_owned())
        );
        assert_eq!(
            classify(
                "build-script-build.exe",
                &args(&["build-script-build"]),
                Some(r"D:\p\target\release\build\ring-0123456789abcdef\build-script-build.exe")
            ),
            (ProcessKind::BuildScript, "ring".to_owned())
        );
        assert_eq!(
            classify("cargo", &args(&["cargo", "build", "--release"]), None),
            (ProcessKind::Cargo, "cargo build".to_owned())
        );
        assert_eq!(classify("link.exe", &[], None), (ProcessKind::Linker, "link".to_owned()));
        assert_eq!(classify("python3", &[], None), (ProcessKind::Other, "python3".to_owned()));
        // A rustc wrapper (sccache) still names the crate.
        assert_eq!(
            classify("sccache", &args(&["sccache", "rustc", "--crate-name", "serde"]), None).1,
            "serde"
        );
    }

    #[test]
    fn build_directory_hashes_are_stripped() {
        assert_eq!(strip_hash("serde-1a2b3c4d5e6f7a8b"), "serde");
        assert_eq!(strip_hash("my-crate-1a2b3c4d5e6f7a8b"), "my-crate");
        assert_eq!(strip_hash("my-crate"), "my-crate");
    }
}
