//! DXC for Windows on ARM.
//!
//! wgpu compiles DX12 shaders with DXC when it can and falls back to FXC
//! otherwise. Windows on ARM has no statically linked DXC (wgpu: "Not available
//! on `windows-aarch64-pc-*` targets") and nothing ships `dxcompiler.dll` with
//! us, so those machines silently got FXC. FXC is old, unmaintained and rejects
//! valid shaders (barriers it cannot prove uniform, loops it cannot unroll), so
//! the editor panicked at startup on shaders that compile everywhere else.
//!
//! The engine therefore never runs on FXC there. [`require_or_exit`] makes sure
//! `dxcompiler.dll` and `dxil.dll` are loadable, installing them when they are
//! not, then selects the dynamic DXC compiler explicitly: if it cannot load,
//! wgpu fails instead of quietly using FXC.
//!
//! Installing = download Microsoft's pinned DXC release, check the zip against
//! its published SHA-256, extract the two ARM64 DLLs into a per-user cache
//! (`%LOCALAPPDATA%\Pulsar\dxc\<tag>\arm64`) and put that directory on the DLL
//! search path. A copy next to the executable wins and is never replaced.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The pinned release: <https://github.com/microsoft/DirectXShaderCompiler/releases/tag/v1.9.2609>.
pub const DXC_TAG: &str = "v1.9.2609";
const DXC_URL: &str = "https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2609/dxc_2026_09_29.zip";
/// SHA-256 GitHub publishes for that asset.
const DXC_ZIP_SHA256: &str = "ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1";
const DXC_ZIP_BYTES: u64 = 32_287_592;
/// The release's `bin/<arch>/` folder for this build.
const ARCH_DIR: &str = "arm64";
const DLLS: [&str; 2] = ["dxcompiler.dll", "dxil.dll"];
/// Written last; a cache folder without it is a half install.
const MARKER: &str = "installed.ok";
/// The variable wgpu reads (`Dx12Compiler::from_env`) to choose the compiler.
const COMPILER_ENV: &str = "WGPU_DX12_COMPILER";

/// Make DXC available and select it, or tell the user why not and exit.
///
/// Call once at startup, before any wgpu instance exists and before other
/// threads run (it sets an environment variable). A `WGPU_DX12_COMPILER` the
/// user set themselves is left alone.
pub fn require_or_exit() {
    if std::env::var_os(COMPILER_ENV).is_some() {
        return;
    }
    match ensure() {
        Ok(dir) => {
            if let Err(e) = activate(&dir) {
                fail(&format!(
                    "DXC was found at {} but could not be loaded: {e:#}",
                    dir.display()
                ));
            }
        }
        Err(e) => fail(&format!("{e:#}")),
    }
}

fn fail(reason: &str) -> ! {
    let message = format!(
        "Pulsar Engine needs Microsoft's DirectX Shader Compiler (DXC) on this PC and could not set it up.\n\n\
         {reason}\n\n\
         Connect to the internet and start Pulsar again, or copy dxcompiler.dll and dxil.dll \
         ({DXC_TAG}, arm64) next to pulsar_engine.exe.",
    );
    eprintln!("{message}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Pulsar Engine")
        .set_description(&message)
        .show();
    std::process::exit(1);
}

// std::fs allowed: the Windows-on-ARM DXC install is the engine's own per-user tool cache, outside any project
#[allow(clippy::disallowed_methods)]
/// The directory holding both DLLs: beside the executable, else the cache
/// (installed first if needed).
fn ensure() -> Result<PathBuf> {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        if has_dlls(&dir) {
            return Ok(dir);
        }
    }
    let cache = cache_dir()?;
    if has_dlls(&cache) && cache.join(MARKER).is_file() {
        return Ok(cache);
    }
    if !confirm_download() {
        bail!("The download was declined, and Pulsar will not run on the legacy shader compiler.");
    }
    println!(
        "Installing DirectX Shader Compiler {DXC_TAG} (one time, {} MB)...",
        DXC_ZIP_BYTES / 1_000_000
    );
    install(&cache).with_context(|| format!("installing DXC {DXC_TAG}"))?;
    println!("DirectX Shader Compiler installed to {}", cache.display());
    Ok(cache)
}

/// Ask before downloading anything. One time: afterwards the cache is used.
fn confirm_download() -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Pulsar Engine: one-time setup")
        .set_description(format!(
            "Pulsar needs Microsoft's DirectX Shader Compiler ({DXC_TAG}) to run on this PC.\n\n\
             It will be downloaded once (about {} MB) from github.com/microsoft/DirectXShaderCompiler, \
             checked against its published SHA-256 and kept in your local app data folder.\n\n\
             Download it now?",
            DXC_ZIP_BYTES / 1_000_000
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

// std::fs allowed: the Windows-on-ARM DXC install is the engine's own per-user tool cache, outside any project
#[allow(clippy::disallowed_methods)]
fn has_dlls(dir: &Path) -> bool {
    DLLS.iter().all(|d| dir.join(d).is_file())
}

fn cache_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("no per-user data directory")?;
    Ok(base.join("Pulsar").join("dxc").join(DXC_TAG).join(ARCH_DIR))
}

/// Put `dir` on the DLL search path (wgpu loads `dxcompiler.dll`, and DXC loads
/// `dxil.dll`, by bare name) and select the dynamic DXC compiler.
fn activate(dir: &Path) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
    unsafe { SetDllDirectoryW(&HSTRING::from(dir.as_os_str()))? };
    // Explicit, so a load failure is an error and never a quiet FXC fallback.
    std::env::set_var(COMPILER_ENV, "dxc");
    Ok(())
}

// std::fs allowed: the Windows-on-ARM DXC install is the engine's own per-user tool cache, outside any project
#[allow(clippy::disallowed_methods)]
fn install(final_dir: &Path) -> Result<()> {
    let tag_dir = final_dir.parent().context("cache path has no parent")?;
    std::fs::create_dir_all(tag_dir)?;
    let work = tag_dir.join(format!(".install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| -> Result<()> {
        let zip = work.join("dxc.zip");
        download(DXC_URL, &zip, DXC_ZIP_SHA256)?;
        let out = work.join("out");
        extract(&zip, &out)?;
        std::fs::write(out.join(MARKER), DXC_ZIP_SHA256)?;
        let _ = std::fs::remove_dir_all(final_dir);
        std::fs::rename(&out, final_dir).context("moving DXC into the cache")?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

// std::fs allowed: the Windows-on-ARM DXC install is the engine's own per-user tool cache, outside any project
#[allow(clippy::disallowed_methods)]
/// Download `url` to `dest` and require its SHA-256 to be `expected`.
fn download(url: &str, dest: &Path, expected: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .user_agent(concat!("pulsar-engine/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client.get(url).send()?.error_for_status()?;
    let mut file = std::fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    file.flush()?;
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(dest);
        bail!(
            "the download does not match the pinned release (sha256 {actual}, expected {expected})"
        );
    }
    Ok(())
}

// std::fs allowed: the Windows-on-ARM DXC install is the engine's own per-user tool cache, outside any project
#[allow(clippy::disallowed_methods)]
/// Copy `bin/<ARCH_DIR>/{dxcompiler,dxil}.dll` out of the release zip into
/// `out`, flat. Entry names are only matched, never used as paths.
fn extract(zip_path: &Path, out: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    std::fs::create_dir_all(out)?;
    let mut found = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().replace('\\', "/").to_ascii_lowercase();
        let mut parts = name.rsplit('/');
        let (Some(file), Some(dir)) = (parts.next(), parts.next()) else {
            continue;
        };
        if dir != ARCH_DIR {
            continue;
        }
        let Some(dll) = DLLS.iter().find(|d| **d == file) else {
            continue;
        };
        let mut dst = std::fs::File::create(out.join(dll))?;
        std::io::copy(&mut entry, &mut dst)?;
        found += 1;
    }
    if found != DLLS.len() {
        bail!(
            "the release zip has no {ARCH_DIR} {} (found {found} of {})",
            DLLS.join(" + "),
            DLLS.len()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        (dir, path)
    }

    #[test]
    fn extract_picks_only_this_architectures_dlls() {
        let (dir, zip) = zip_with(&[
            ("bin/x64/dxcompiler.dll", b"x64c"),
            ("bin/x64/dxil.dll", b"x64i"),
            ("bin/arm64/dxcompiler.dll", b"armc"),
            ("bin/arm64/dxil.dll", b"armi"),
            ("bin/arm64/dxc.exe", b"exe"),
        ]);
        let out = dir.path().join("out");
        extract(&zip, &out).unwrap();
        assert_eq!(std::fs::read(out.join("dxcompiler.dll")).unwrap(), b"armc");
        assert_eq!(std::fs::read(out.join("dxil.dll")).unwrap(), b"armi");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);
    }

    #[test]
    fn extract_rejects_a_zip_missing_a_dll() {
        let (dir, zip) = zip_with(&[
            ("bin/arm64/dxcompiler.dll", b"c"),
            ("bin/x64/dxil.dll", b"i"),
        ]);
        assert!(extract(&zip, &dir.path().join("out")).is_err());
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xab]), "000fab");
    }

    #[test]
    fn the_pin_is_well_formed() {
        assert_eq!(DXC_ZIP_SHA256.len(), 64);
        assert!(DXC_ZIP_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(DXC_URL.contains(DXC_TAG));
    }
}
