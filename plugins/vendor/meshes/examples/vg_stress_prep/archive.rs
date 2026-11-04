//! Streaming `.gz` / `.tar.gz` access without any external tool: a gzip
//! decoder plus a minimal ustar/GNU/pax reader that yields one member.

use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

/// Open `path` as gzip. With `member`, treat the payload as a tar archive and
/// return the first regular file whose path equals `member` or ends with
/// `/member`. Without it, return the decompressed stream itself.
pub fn open_gz(path: &Path, member: Option<&str>) -> Result<Box<dyn Read + Send>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let gz = flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 20, file));
    match member {
        None => Ok(Box::new(gz)),
        Some(m) => Ok(Box::new(find_tar_member(gz, m)?)),
    }
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<bool, String> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..]).map_err(|e| e.to_string())?;
        if k == 0 {
            return if n == 0 {
                Ok(false)
            } else {
                Err("tar: truncated header".into())
            };
        }
        n += k;
    }
    Ok(true)
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).to_string()
}

fn parse_size(b: &[u8]) -> Result<u64, String> {
    if b[0] & 0x80 != 0 {
        // GNU base-256 extension.
        let mut v = (b[0] & 0x7f) as u64;
        for &c in &b[1..] {
            v = v.checked_shl(8).ok_or("tar: size overflow")? | c as u64;
        }
        return Ok(v);
    }
    let s = cstr(b);
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).map_err(|_| format!("tar: bad size field '{s}'"))
}

fn skip<R: Read>(r: &mut R, mut n: u64) -> Result<(), String> {
    let mut sink = vec![0u8; 1 << 16];
    while n > 0 {
        let k = (n as usize).min(sink.len());
        r.read_exact(&mut sink[..k])
            .map_err(|e| format!("tar: {e}"))?;
        n -= k as u64;
    }
    Ok(())
}

pub fn find_tar_member<R: Read>(mut r: R, member: &str) -> Result<std::io::Take<R>, String> {
    let suffix = format!("/{member}");
    let mut long_name: Option<String> = None;
    let mut header = [0u8; 512];
    loop {
        if !read_full(&mut r, &mut header)? || header.iter().all(|&b| b == 0) {
            return Err(format!("tar: member '{member}' not found"));
        }
        let size = parse_size(&header[124..136])?;
        let kind = header[156];
        let padded = size.div_ceil(512) * 512;
        match kind {
            b'L' | b'x' | b'g' => {
                let mut data = vec![0u8; size as usize];
                r.read_exact(&mut data).map_err(|e| format!("tar: {e}"))?;
                skip(&mut r, padded - size)?;
                let text = String::from_utf8_lossy(&data).to_string();
                if kind == b'L' {
                    long_name = Some(text.trim_end_matches('\0').to_string());
                } else if kind == b'x' {
                    // "<len> path=<name>\n" records.
                    for rec in text.split('\n') {
                        if let Some(p) = rec
                            .split_once(' ')
                            .and_then(|(_, kv)| kv.strip_prefix("path="))
                        {
                            long_name = Some(p.to_string());
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        let name = long_name.take().unwrap_or_else(|| {
            let base = cstr(&header[0..100]);
            let prefix = cstr(&header[345..500]);
            if &header[257..262] == b"ustar" && !prefix.is_empty() {
                format!("{prefix}/{base}")
            } else {
                base
            }
        });
        let is_file = kind == b'0' || kind == 0;
        if is_file && (name == member || name.ends_with(&suffix)) {
            return Ok(r.take(size));
        }
        skip(&mut r, padded)?;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    pub fn tar_entry(name: &str, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[156] = kind;
        let size = format!("{:011o}\0", data.len());
        h[124..136].copy_from_slice(size.as_bytes());
        h[257..262].copy_from_slice(b"ustar");
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len() + (512 - data.len() % 512) % 512, 0);
        out
    }

    pub fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    fn sample_tar() -> Vec<u8> {
        let mut t = Vec::new();
        t.extend(tar_entry("pkg/", b'5', b""));
        t.extend(tar_entry("pkg/other.ply", b'0', &[1u8; 700]));
        let long = format!("pkg/{}/deep.ply", "d".repeat(120));
        t.extend(tar_entry("././@LongLink", b'L', long.as_bytes()));
        t.extend(tar_entry("truncated-name", b'0', b"long-member"));
        t.extend(tar_entry("pkg/wanted.ply", b'0', b"wanted-bytes"));
        t.extend([0u8; 1024]);
        t
    }

    #[test]
    fn finds_member_by_suffix_after_skipping_others() {
        let mut out = Vec::new();
        find_tar_member(&sample_tar()[..], "wanted.ply")
            .map(|mut r| r.read_to_end(&mut out))
            .unwrap()
            .unwrap();
        assert_eq!(out, b"wanted-bytes");
        // Exact, full path through a GNU long-name record.
        let long = format!("pkg/{}/deep.ply", "d".repeat(120));
        let mut out = Vec::new();
        let tar = sample_tar();
        let mut r = find_tar_member(&tar[..], &long).unwrap();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"long-member");
        assert!(find_tar_member(&sample_tar()[..], "nope.ply").is_err());
    }

    #[test]
    fn gz_and_tar_gz_round_trip_from_disk() {
        let dir = std::env::temp_dir().join(format!("vgprep-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("a.gz");
        std::fs::write(&plain, gzip(b"hello world")).unwrap();
        let mut s = String::new();
        open_gz(&plain, None)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "hello world");
        let tgz = dir.join("b.tar.gz");
        std::fs::write(&tgz, gzip(&sample_tar())).unwrap();
        let mut s = String::new();
        open_gz(&tgz, Some("pkg/wanted.ply"))
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "wanted-bytes");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
