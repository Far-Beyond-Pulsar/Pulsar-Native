//! Fetching a language server's managed install over HTTPS, in process
//! (#1940): no system `curl`, `wget` or PowerShell. The transport is not
//! trusted: the caller verifies what it gets against a digest pinned in the
//! plugin before anything is unpacked or run.

use std::time::Duration;

/// The most a download may be: a language server's archive is tens of MB.
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// The body of `url` (redirects followed, HTTPS only, at most
/// [`MAX_DOWNLOAD_BYTES`]), or why it could not be fetched.
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("refusing to download {url}: not HTTPS"));
    }
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(900)))
        .user_agent("pulsar-language-installer")
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| format!("could not download {url}: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(|e| format!("could not download {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_refused_before_any_request() {
        let error = fetch("http://example.com/x.zip").unwrap_err();
        assert!(error.contains("not HTTPS"), "{error}");
    }
}
