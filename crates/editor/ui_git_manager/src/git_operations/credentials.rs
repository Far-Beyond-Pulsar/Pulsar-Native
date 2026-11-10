//! Git credentials for remote operations: the credential store, auth-error
//! detection and the libgit2 callbacks that supply them.

use super::remote_context::same_credential_scope;
use crate::models::*;
use engine_fs::FsProvider as _;

/// Returns true if the git2 error indicates an authentication failure.
pub fn is_auth_error(e: &git2::Error) -> bool {
    let msg = e.message().to_lowercase();
    msg.contains("authentication")
        || msg.contains("401")
        || msg.contains("credentials")
        || e.class() == git2::ErrorClass::Http
}

pub(super) fn load_git_credentials(remote_url: &str) -> Option<(String, String)> {
    let entry = keyring::Entry::new("pulsar-git", remote_url).ok()?;
    let secret = entry.get_password().ok()?;
    let mut parts = secret.splitn(2, '\n');
    let username = parts.next()?.to_string();
    let password = parts.next()?.to_string();
    Some((username, password))
}

pub(super) fn store_git_credentials(remote_url: &str, username: &str, password: &str) {
    let Ok(entry) = keyring::Entry::new("pulsar-git", remote_url) else {
        return;
    };

    if let Err(error) = entry.set_password(&format!("{username}\n{password}")) {
        tracing::warn!(%error, "Failed to store Git credentials");
    }
}

pub(super) fn load_stored_credentials(remote_url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(remote_url).ok()?;
    parsed.host_str()?;
    load_git_credentials(remote_url)
}

/// Build remote callbacks using credentials scoped to the selected remote URL.
pub(super) fn make_callbacks(
    creds: Option<(String, String)>,
    credential_url: String,
) -> git2::RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(move |url, username, allowed_types| {
        if let Some((ref user, ref pass)) = creds {
            if same_credential_scope(&credential_url, url)
                && allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT)
            {
                return git2::Cred::userpass_plaintext(user, pass);
            }
        }

        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if let Ok(credential) = git2::Cred::ssh_key_from_agent(username.unwrap_or("git")) {
                return Ok(credential);
            }
        }

        if allowed_types.contains(git2::CredentialType::DEFAULT) {
            if let Ok(credential) = git2::Cred::default() {
                return Ok(credential);
            }
        }

        Err(git2::Error::from_str("No credentials available"))
    });
    callbacks
}
