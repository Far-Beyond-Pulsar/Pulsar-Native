use std::sync::Arc;

use ui::download_manager::DownloadEntry;

use crate::FabSearchWindow;
use crate::utils::actions::DownloadState;

pub fn build_download_entries(window: &FabSearchWindow) -> Vec<DownloadEntry> {
    window
        .download_state
        .iter()
        .map(|(uid, state)| match state {
            DownloadState::InProgress {
                filename,
                bytes_received,
                total_bytes,
                speed_bps,
                speed_history,
            } => {
                let progress_pct = total_bytes
                    .filter(|&t| t > 0)
                    .map(|t| (*bytes_received as f32 / t as f32 * 100.0).min(100.0))
                    .unwrap_or(0.0);
                DownloadEntry {
                    uid: gpui::SharedString::from(uid.clone()),
                    filename: gpui::SharedString::from(filename.clone()),
                    progress_pct,
                    speed_bps: *speed_bps,
                    speed_history: speed_history.clone(),
                    status: ui::download_item::DownloadItemStatus::InProgress,
                    bytes_received: *bytes_received,
                    total_bytes: *total_bytes,
                    path: None,
                }
            }
            DownloadState::Done {
                filename,
                path,
                total_bytes,
            } => DownloadEntry {
                uid: gpui::SharedString::from(uid.clone()),
                filename: gpui::SharedString::from(filename.clone()),
                progress_pct: 100.0,
                speed_bps: 0.0,
                speed_history: Vec::new(),
                status: ui::download_item::DownloadItemStatus::Done,
                bytes_received: *total_bytes,
                total_bytes: Some(*total_bytes),
                path: Some(path.clone()),
            },
            DownloadState::Error { filename, message } => DownloadEntry {
                uid: gpui::SharedString::from(uid.clone()),
                filename: gpui::SharedString::from(filename.clone()),
                progress_pct: 0.0,
                speed_bps: 0.0,
                speed_history: Vec::new(),
                status: ui::download_item::DownloadItemStatus::Error(message.clone()),
                bytes_received: 0,
                total_bytes: None,
                path: None,
            },
        })
        .collect()
}
