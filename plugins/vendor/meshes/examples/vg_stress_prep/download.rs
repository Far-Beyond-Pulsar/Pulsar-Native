//! Resumable HTTP(S) download with retries, mirrors, and size + SHA-256 checks.

use super::md5;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_ATTEMPTS: u32 = 6;

/// Identifies this engine to servers (Poly Haven asks API clients to do so).
pub const USER_AGENT: &str =
    "PulsarEngine-Helio-vg_stress/1.0 (+https://github.com/Far-Beyond-Pulsar)";

/// Which digest a download is checked against. Stanford archives are pinned by
/// SHA-256; Poly Haven publishes MD5 (and sizes) for every file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checksum<'a> {
    Sha256(&'a str),
    Md5(&'a str),
}

impl Checksum<'_> {
    fn expected(&self) -> &str {
        match self {
            Checksum::Sha256(h) | Checksum::Md5(h) => h,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Checksum::Sha256(_) => "SHA-256",
            Checksum::Md5(_) => "MD5",
        }
    }

    fn marker_ext(&self) -> &'static str {
        match self {
            Checksum::Sha256(_) => ".sha256",
            Checksum::Md5(_) => ".md5",
        }
    }
}

fn hash_file(path: &Path, kind: Checksum, progress: &dyn Fn(&str)) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sha = Sha256::new();
    let mut md = md5::Md5::new();
    let use_md5 = matches!(kind, Checksum::Md5(_));
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let mut last = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if use_md5 {
            md.update(&buf[..n]);
        } else {
            sha.update(&buf[..n]);
        }
        done += n as u64;
        if done - last > 64 << 20 {
            last = done;
            progress(&format!(
                "verifying {} ({}%)",
                file_name(path),
                done * 100 / total
            ));
        }
    }
    Ok(if use_md5 {
        md.finalize().iter().map(|b| format!("{b:02x}")).collect()
    } else {
        sha.finalize().iter().map(|b| format!("{b:02x}")).collect()
    })
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

fn marker(dest: &Path, kind: Checksum) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(kind.marker_ext());
    PathBuf::from(s)
}

fn part(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// True when `dest` is complete and verified (verifying and recording the
/// hash of a file that was placed there by hand).
pub fn is_verified(dest: &Path, size: u64, kind: Checksum, progress: &dyn Fn(&str)) -> bool {
    let expected = kind.expected();
    let Ok(meta) = std::fs::metadata(dest) else {
        return false;
    };
    if meta.len() != size {
        return false;
    }
    let m = marker(dest, kind);
    if std::fs::read_to_string(&m).is_ok_and(|s| s.trim() == expected) {
        return true;
    }
    match hash_file(dest, kind, progress) {
        Ok(h) if h == expected => {
            let _ = std::fs::write(&m, expected);
            true
        }
        _ => false,
    }
}

enum Outcome {
    Complete,
    /// Connection dropped or server error: try again (resuming).
    Retry(String),
}

fn attempt(
    url: &str,
    part_path: &Path,
    size: u64,
    progress: &dyn Fn(&str),
    label: &str,
) -> Result<Outcome, String> {
    let have = std::fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
    if have > size {
        let _ = std::fs::remove_file(part_path);
        return Ok(Outcome::Retry("partial file larger than expected".into()));
    }
    if have == size {
        return Ok(Outcome::Complete);
    }
    let agent = ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .build();
    let mut req = agent.get(url);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(416, _)) => {
            let _ = std::fs::remove_file(part_path);
            return Ok(Outcome::Retry("range not satisfiable".into()));
        }
        Err(ureq::Error::Status(code, _)) if code >= 500 || code == 429 => {
            return Ok(Outcome::Retry(format!("HTTP {code}")));
        }
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code} for {url}")),
        Err(e) => return Ok(Outcome::Retry(e.to_string())),
    };
    let mut file = if resp.status() == 206 {
        OpenOptions::new().append(true).create(true).open(part_path)
    } else {
        // The server ignored the range (or this is a fresh start): restart.
        File::create(part_path)
    }
    .map_err(|e| format!("{}: {e}", part_path.display()))?;
    let mut done = if resp.status() == 206 { have } else { 0 };
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 1 << 18];
    let mut last = std::time::Instant::now() - Duration::from_secs(10);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Ok(Outcome::Retry(e.to_string())),
        };
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done > size {
            drop(file);
            let _ = std::fs::remove_file(part_path);
            return Ok(Outcome::Retry(
                "server sent more than the expected size".into(),
            ));
        }
        if last.elapsed() >= Duration::from_secs(2) {
            last = std::time::Instant::now();
            progress(&format!(
                "downloading {label}: {}/{} MB",
                done >> 20,
                size >> 20
            ));
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    if done == size {
        Ok(Outcome::Complete)
    } else {
        Ok(Outcome::Retry(format!(
            "connection ended at {done}/{size} bytes"
        )))
    }
}

/// Download one of `urls` to `dest` unless it is already present and valid.
/// `backoff` is the base delay between retries (tests use a tiny one).
pub fn fetch(
    urls: &[&str],
    dest: &Path,
    size: u64,
    sha256: &str,
    backoff: Duration,
    progress: &dyn Fn(&str),
) -> Result<(), String> {
    fetch_checked(
        urls,
        dest,
        size,
        Checksum::Sha256(sha256),
        backoff,
        progress,
    )
}

/// [`fetch`] with a choice of digest.
pub fn fetch_checked(
    urls: &[&str],
    dest: &Path,
    size: u64,
    kind: Checksum,
    backoff: Duration,
    progress: &dyn Fn(&str),
) -> Result<(), String> {
    let sha256 = kind.expected();
    if is_verified(dest, size, kind, progress) {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part_path = part(dest);
    let label = file_name(dest);
    let mut last_error = String::from("no URL");
    let mut hash_failures = 0;
    for url in urls {
        let mut attempts = 0;
        while attempts < MAX_ATTEMPTS {
            attempts += 1;
            match attempt(url, &part_path, size, progress, &label) {
                Err(e) => {
                    // Permanent for this URL (404 ...): next mirror.
                    last_error = e;
                    break;
                }
                Ok(Outcome::Retry(why)) => {
                    last_error = format!("{url}: {why}");
                    progress(&format!("{label}: {why}; retry {attempts}/{MAX_ATTEMPTS}"));
                    std::thread::sleep(backoff * attempts);
                }
                Ok(Outcome::Complete) => {
                    let hash = hash_file(&part_path, kind, progress)?;
                    if hash == sha256 {
                        std::fs::rename(&part_path, dest).map_err(|e| e.to_string())?;
                        let _ = std::fs::write(marker(dest, kind), sha256);
                        return Ok(());
                    }
                    let _ = std::fs::remove_file(&part_path);
                    hash_failures += 1;
                    last_error = format!(
                        "{label}: {} mismatch (got {hash}, expected {sha256})",
                        kind.name()
                    );
                    if hash_failures >= 2 {
                        return Err(last_error);
                    }
                }
            }
        }
    }
    Err(format!("could not download {label}: {last_error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    /// Serves `body` for any path. `cut_first`: the first request sends only
    /// half the body then closes. `fail_first`: first request gets a 503.
    fn serve(
        body: Vec<u8>,
        cut_first: bool,
        fail_first: bool,
    ) -> (String, Arc<AtomicUsize>, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (h, r) = (hits.clone(), ranges.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let n = h.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                }
                r.lock().unwrap().push(format!("{range:?}"));
                if fail_first && n == 0 {
                    let _ = s.write_all(
                        b"HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let start = range.unwrap_or(0).min(body.len());
                let part = &body[start..];
                let head = if range.is_some() {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        part.len(),
                        start,
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        part.len()
                    )
                };
                let _ = s.write_all(head.as_bytes());
                if cut_first && n == 0 {
                    let _ = s.write_all(&part[..part.len() / 2]);
                    let _ = s.flush();
                    continue; // drop: truncated body
                }
                let _ = s.write_all(part);
            }
        });
        (url, hits, ranges)
    }

    fn hash(b: &[u8]) -> String {
        Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vgprep-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn downloads_and_verifies() {
        let b = body();
        let (url, _, _) = serve(b.clone(), false, false);
        let d = tmp("ok");
        let dest = d.join("f.bin");
        fetch(
            &[&url],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b);
        // Second call is a verified no-op (server not contacted again).
        fetch(
            &[&url],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
    }

    #[test]
    fn resumes_a_dropped_connection_and_retries_server_errors() {
        let b = body();
        let (url, hits, ranges) = serve(b.clone(), true, false);
        let d = tmp("resume");
        let dest = d.join("f.bin");
        fetch(
            &[&url],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b);
        assert!(hits.load(Ordering::SeqCst) >= 2);
        let r = ranges.lock().unwrap();
        assert_eq!(r[0], "None");
        assert!(r[1].starts_with("Some("), "second request resumes: {r:?}");

        let (url, hits, _) = serve(b.clone(), false, true);
        let dest = d.join("g.bin");
        fetch(
            &[&url],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2, "503 then success");
    }

    #[test]
    fn hash_mismatch_is_rejected_and_part_file_removed() {
        let b = body();
        let (url, _, _) = serve(b.clone(), false, false);
        let d = tmp("bad");
        let dest = d.join("f.bin");
        let err = fetch(
            &[&url],
            &dest,
            b.len() as u64,
            &hash(b"other"),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap_err();
        assert!(err.contains("SHA-256 mismatch"), "{err}");
        assert!(!dest.exists() && !part(&dest).exists());
    }

    #[test]
    fn falls_through_to_the_next_mirror_on_404() {
        let b = body();
        let (good, _, _) = serve(b.clone(), false, false);
        // A server that answers every request with 404.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let bad = format!("http://{}/x", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = s.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let d = tmp("mirror");
        let dest = d.join("f.bin");
        fetch(
            &[&bad, &good],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b);
    }

    #[test]
    fn existing_file_is_verified_not_redownloaded() {
        let b = body();
        let d = tmp("existing");
        let dest = d.join("f.bin");
        std::fs::write(&dest, &b).unwrap();
        // No reachable URL: succeeds purely on the local file's hash.
        fetch(
            &["http://127.0.0.1:1/none"],
            &dest,
            b.len() as u64,
            &hash(&b),
            Duration::from_millis(1),
            &|_| {},
        )
        .unwrap();
        assert!(marker(&dest, Checksum::Sha256("")).exists());
    }
}
