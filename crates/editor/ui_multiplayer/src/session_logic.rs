//! Pure, GPUI-free helpers for the multiplayer session flows.
//!
//! Owns input validation, participant-list transitions, message-payload parsing
//! and chunk arithmetic used by both the guest flow (`peer_state::join_session`)
//! and the host flow (`sync_protocol::create_session`). Nothing here touches
//! GPUI entities, so everything can be unit tested with plain `cargo test`.
//!
//! Entry points: [`validate_join_input`], [`validate_server_address`],
//! [`add_peer`], [`remove_peer`], [`parse_files_payload`], [`write_file_chunk`].

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::net::multiuser::PeerProfile;
use engine_fs::events::FsChangeKind;
use engine_state::MultiuserParticipant;

/// Size in bytes of each `FileChunk` sent when serving a single file.
pub(crate) const FILE_CHUNK_SIZE: usize = 8192;

/// Checks that a server address was entered.
pub(crate) fn validate_server_address(server_address: &str) -> Result<(), &'static str> {
    if server_address.is_empty() {
        return Err("Server address is required");
    }
    Ok(())
}

/// Checks the three fields of the join form, reporting the first problem.
pub(crate) fn validate_join_input(
    server_address: &str,
    session_id: &str,
    join_token: &str,
) -> Result<(), &'static str> {
    validate_server_address(server_address)?;
    if session_id.is_empty() || join_token.is_empty() {
        return Err("Session ID and password are required");
    }
    Ok(())
}

/// Formats an error reported by the server for display and engine status.
pub(crate) fn server_error_text(message: &str) -> String {
    format!("Server error: {}", message)
}

/// Formats a transport-level connection failure.
pub(crate) fn connection_failed_text(error: impl std::fmt::Display) -> String {
    format!("Connection failed: {}", error)
}

/// The host is the first participant; falls back to our own id when the list is empty.
pub(crate) fn host_peer_id_or(participants: &[String], own_peer_id: &str) -> String {
    participants
        .first()
        .cloned()
        .unwrap_or_else(|| own_peer_id.to_string())
}

/// Display name for the participant at the given 1-based position.
pub(crate) fn user_label(position: usize) -> String {
    format!("User {}", position)
}

/// Adds `peer_id` to `users` unless already present. Returns `true` if it was added.
pub(crate) fn add_peer(users: &mut Vec<String>, peer_id: &str) -> bool {
    if users.iter().any(|p| p == peer_id) {
        return false;
    }
    users.push(peer_id.to_string());
    true
}

/// Removes every occurrence of `peer_id` from `users`.
pub(crate) fn remove_peer(users: &mut Vec<String>, peer_id: &str) {
    users.retain(|p| p != peer_id);
}

/// True when `sender` is the local peer.
pub(crate) fn is_self_peer(current_peer_id: Option<&str>, sender: &str) -> bool {
    current_peer_id == Some(sender)
}

/// Maps the wire-format change kind to an engine filesystem event kind.
/// Unknown kinds are treated as modifications.
pub(crate) fn fs_change_kind(kind: &str) -> FsChangeKind {
    match kind {
        "created" => FsChangeKind::Created,
        "deleted" => FsChangeKind::Deleted,
        _ => FsChangeKind::Modified,
    }
}

/// True when the chunk starting at `offset` with `chunk_len` bytes ends the file.
pub(crate) fn is_last_chunk(offset: usize, chunk_len: usize, total_len: usize) -> bool {
    offset + chunk_len >= total_len
}

/// Only single-chunk `FilesChunk` transfers are supported for now.
pub(crate) fn is_single_chunk_transfer(chunk_index: usize, total_chunks: usize) -> bool {
    chunk_index == 0 && total_chunks == 1
}

/// Files to request for preview: added files followed by updated files.
pub(crate) fn preview_paths(files_to_add: &[String], files_to_update: &[String]) -> Vec<String> {
    let mut paths = Vec::with_capacity(files_to_add.len() + files_to_update.len());
    paths.extend_from_slice(files_to_add);
    paths.extend_from_slice(files_to_update);
    paths
}

/// Parses the JSON payload of a `FilesChunk` into `(relative_path, bytes)` pairs.
pub(crate) fn parse_files_payload(
    files_json: &str,
) -> Result<Vec<(String, Vec<u8>)>, serde_json::Error> {
    serde_json::from_str(files_json)
}

/// Text previews for the files whose contents are valid UTF-8; binary files are skipped.
pub(crate) fn text_previews(files: &[(String, Vec<u8>)]) -> Vec<(String, String)> {
    files
        .iter()
        .filter_map(|(path, data)| {
            String::from_utf8(data.clone())
                .ok()
                .map(|content| (path.clone(), content))
        })
        .collect()
}

/// Progress message shown after a successful sync.
pub(crate) fn synced_message(written_count: usize) -> String {
    format!("Synced {} files successfully", written_count)
}

/// Progress message shown after a failed sync.
pub(crate) fn sync_failed_message(error: impl std::fmt::Display) -> String {
    format!("Sync failed: {}", error)
}

/// Writes one chunk of a received file at `offset`, creating parent directories
/// and the file as needed. I/O errors are ignored, matching the original behavior.
pub(crate) fn write_file_chunk(project_root: &Path, file_path: &str, offset: u64, data: &[u8]) {
    let full_path = project_root.join(file_path);

    if let Some(parent) = full_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&full_path)
    {
        let _ = file.seek(SeekFrom::Start(offset));
        let _ = file.write_all(data);
    }
}

/// Inserts or updates the participant entry matching `profile.peer_id`.
pub(crate) fn upsert_profile(profiles: &mut Vec<MultiuserParticipant>, profile: &PeerProfile) {
    if let Some(existing) = profiles.iter_mut().find(|p| p.peer_id == profile.peer_id) {
        existing.display_name = profile.display_name.clone();
        existing.avatar_url = profile.avatar_url.clone();
        existing.github_login = profile.github_login.clone();
    } else {
        profiles.push(MultiuserParticipant {
            peer_id: profile.peer_id.clone(),
            display_name: profile.display_name.clone(),
            avatar_url: profile.avatar_url.clone(),
            github_login: profile.github_login.clone(),
            ping_ms: None,
        });
    }
}

/// Drops the participant entry for `peer_id`.
pub(crate) fn remove_profile(profiles: &mut Vec<MultiuserParticipant>, peer_id: &str) {
    profiles.retain(|p| p.peer_id != peer_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str) -> PeerProfile {
        PeerProfile {
            peer_id: id.to_string(),
            display_name: Some(name.to_string()),
            avatar_url: None,
            github_login: None,
        }
    }

    #[test]
    fn join_input_requires_server_address_first() {
        assert_eq!(
            validate_join_input("", "", ""),
            Err("Server address is required")
        );
    }

    #[test]
    fn join_input_requires_session_and_password() {
        let msg = Err("Session ID and password are required");
        assert_eq!(validate_join_input("host:1", "", "pw"), msg);
        assert_eq!(validate_join_input("host:1", "id", ""), msg);
        assert_eq!(validate_join_input("host:1", "id", "pw"), Ok(()));
    }

    #[test]
    fn server_address_validation() {
        assert!(validate_server_address("").is_err());
        assert!(validate_server_address("localhost:8080").is_ok());
    }

    #[test]
    fn error_texts() {
        assert_eq!(server_error_text("bad"), "Server error: bad");
        assert_eq!(connection_failed_text("boom"), "Connection failed: boom");
    }

    #[test]
    fn host_is_first_participant_or_self() {
        let list = vec!["a".to_string(), "b".to_string()];
        assert_eq!(host_peer_id_or(&list, "me"), "a");
        assert_eq!(host_peer_id_or(&[], "me"), "me");
    }

    #[test]
    fn user_labels_are_one_based_text() {
        assert_eq!(user_label(1), "User 1");
    }

    #[test]
    fn add_peer_is_idempotent() {
        let mut users = vec!["a".to_string()];
        assert!(add_peer(&mut users, "b"));
        assert!(!add_peer(&mut users, "b"));
        assert_eq!(users, vec!["a", "b"]);
    }

    #[test]
    fn remove_peer_removes_all_matches() {
        let mut users = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        remove_peer(&mut users, "a");
        assert_eq!(users, vec!["b"]);
        remove_peer(&mut users, "missing");
        assert_eq!(users, vec!["b"]);
    }

    #[test]
    fn self_peer_detection() {
        assert!(is_self_peer(Some("me"), "me"));
        assert!(!is_self_peer(Some("me"), "you"));
        assert!(!is_self_peer(None, "me"));
    }

    #[test]
    fn change_kind_parsing() {
        assert_eq!(fs_change_kind("created"), FsChangeKind::Created);
        assert_eq!(fs_change_kind("deleted"), FsChangeKind::Deleted);
        assert_eq!(fs_change_kind("modified"), FsChangeKind::Modified);
        assert_eq!(fs_change_kind("anything"), FsChangeKind::Modified);
    }

    #[test]
    fn last_chunk_detection() {
        assert!(is_last_chunk(0, 10, 10));
        assert!(!is_last_chunk(0, FILE_CHUNK_SIZE, FILE_CHUNK_SIZE + 1));
        assert!(is_last_chunk(FILE_CHUNK_SIZE, 1, FILE_CHUNK_SIZE + 1));
    }

    #[test]
    fn chunking_marks_only_final_chunk_last() {
        let data = vec![0u8; FILE_CHUNK_SIZE * 2 + 5];
        let flags: Vec<bool> = data
            .chunks(FILE_CHUNK_SIZE)
            .enumerate()
            .map(|(i, c)| is_last_chunk(i * FILE_CHUNK_SIZE, c.len(), data.len()))
            .collect();
        assert_eq!(flags, vec![false, false, true]);
    }

    #[test]
    fn only_single_chunk_transfers_supported() {
        assert!(is_single_chunk_transfer(0, 1));
        assert!(!is_single_chunk_transfer(0, 2));
        assert!(!is_single_chunk_transfer(1, 2));
    }

    #[test]
    fn preview_paths_adds_then_updates() {
        let add = vec!["a".to_string()];
        let upd = vec!["b".to_string(), "c".to_string()];
        assert_eq!(preview_paths(&add, &upd), vec!["a", "b", "c"]);
        assert!(preview_paths(&[], &[]).is_empty());
    }

    #[test]
    fn files_payload_round_trip() {
        let files = vec![("a.txt".to_string(), b"hi".to_vec())];
        let json = serde_json::to_string(&files).unwrap();
        assert_eq!(parse_files_payload(&json).unwrap(), files);
        assert!(parse_files_payload("not json").is_err());
    }

    #[test]
    fn text_previews_skip_binary() {
        let files = vec![
            ("a.txt".to_string(), b"hello".to_vec()),
            ("b.bin".to_string(), vec![0xff, 0xfe]),
        ];
        assert_eq!(
            text_previews(&files),
            vec![("a.txt".to_string(), "hello".to_string())]
        );
    }

    #[test]
    fn sync_messages() {
        assert_eq!(synced_message(3), "Synced 3 files successfully");
        assert_eq!(sync_failed_message("nope"), "Sync failed: nope");
    }

    #[test]
    fn upsert_profile_inserts_then_updates() {
        let mut list = Vec::new();
        upsert_profile(&mut list, &profile("p1", "One"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name.as_deref(), Some("One"));
        assert_eq!(list[0].ping_ms, None);

        list[0].ping_ms = Some(12);
        upsert_profile(&mut list, &profile("p1", "Renamed"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name.as_deref(), Some("Renamed"));
        assert_eq!(list[0].ping_ms, Some(12));
    }

    #[test]
    fn remove_profile_by_peer_id() {
        let mut list = Vec::new();
        upsert_profile(&mut list, &profile("p1", "One"));
        upsert_profile(&mut list, &profile("p2", "Two"));
        remove_profile(&mut list, "p1");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].peer_id, "p2");
    }

    #[test]
    fn write_file_chunk_writes_at_offset_and_creates_dirs() {
        let root =
            std::env::temp_dir().join(format!("ui_multiplayer_chunk_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        write_file_chunk(&root, "sub/dir/f.bin", 0, b"abcd");
        write_file_chunk(&root, "sub/dir/f.bin", 4, b"efgh");
        let contents = std::fs::read(root.join("sub/dir/f.bin")).unwrap();
        assert_eq!(contents, b"abcdefgh");

        let _ = std::fs::remove_dir_all(&root);
    }
}
