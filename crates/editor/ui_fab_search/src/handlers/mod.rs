use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::input::InputEvent;

use crate::FabSearchWindow;
use crate::search_index::{
    SearchPage, fetch_sketchfab_download_info, fetch_sketchfab_models,
    fetch_sketchfab_model_detail, sketchfab_like_model, sketchfab_unlike_model,
};
use crate::utils::actions::{DownloadMsg, DownloadState};

pub(crate) fn on_go_back(window: &mut FabSearchWindow, cx: &mut Context<FabSearchWindow>) {
    window.selected_item_uid = None;
    window.item_detail = None;
    window.detail_loading = false;
    window.detail_error = None;
    cx.notify();
}

pub(crate) fn on_set_token(
    window: &mut FabSearchWindow,
    token: String,
    cx: &mut Context<FabSearchWindow>,
) {
    let token = token.trim().to_string();
    if token.is_empty() {
        return;
    }
    crate::auth::save_token(&token);
    window.api_token = Some(token);
    window.me = None;
    window.show_token_input = false;
    window.fetch_me(cx);
}

pub(crate) fn on_clear_token(window: &mut FabSearchWindow, cx: &mut Context<FabSearchWindow>) {
    crate::auth::delete_token();
    window.api_token = None;
    window.me = None;
    window.me_loading = false;
    window.liked_uids.clear();
    window.like_inflight.clear();
    cx.notify();
}

pub(crate) fn on_start_download(
    window: &mut FabSearchWindow,
    uid: String,
    cx: &mut Context<FabSearchWindow>,
) {
    if matches!(
        window.download_state.get(&uid),
        Some(DownloadState::InProgress { .. })
    ) {
        return;
    }
    let token = match &window.api_token {
        Some(t) => t.clone(),
        None => return,
    };

    let filename = format!("{}.zip", uid);
    window.download_state.insert(
        uid.clone(),
        DownloadState::InProgress {
            filename: filename.clone(),
            bytes_received: 0,
            total_bytes: None,
            speed_history: Vec::new(),
            speed_bps: 0.0,
        },
    );
    cx.notify();

    let (tx, rx) = smol::channel::unbounded::<DownloadMsg>();
    let uid_thread = uid.clone();
    let filename_thread = filename.clone();

    std::thread::spawn(move || {
        let run = || -> Result<(), String> {
            let info = fetch_sketchfab_download_info(&uid_thread, &token)?;
            let fmt = info
                .gltf
                .or(info.glb)
                .or(info.source)
                .ok_or_else(|| "No downloadable format available".to_string())?;

            let dest_dir = dirs::download_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Sketchfab");
            std::fs::create_dir_all(&dest_dir).map_err(|e| format!("mkdir: {e}"))?;
            let dest = dest_dir.join(&filename_thread);

            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .user_agent("Pulsar-Native/1.0")
                .build()
                .map_err(|e| e.to_string())?;

            let resp = client.get(&fmt.url).send().map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("HTTP {} downloading file", status));
            }
            let total = resp.content_length();

            let mut file =
                std::fs::File::create(&dest).map_err(|e| format!("create: {e}"))?;
            let mut bytes_total: u64 = 0;
            let mut last_sample = std::time::Instant::now();
            let mut bytes_since_sample: u64 = 0;
            let mut buf = vec![0u8; 64 * 1024];
            let mut reader = std::io::BufReader::new(resp);
            use std::io::{Read, Write};

            loop {
                let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])
                    .map_err(|e| format!("write: {e}"))?;
                bytes_total += n as u64;
                bytes_since_sample += n as u64;

                let elapsed = last_sample.elapsed();
                if elapsed >= std::time::Duration::from_millis(500) {
                    let speed = bytes_since_sample as f64 / elapsed.as_secs_f64();
                    smol::block_on(tx.send(DownloadMsg::Progress {
                        bytes_received: bytes_total,
                        total,
                        speed_bps: speed,
                    }));
                    last_sample = std::time::Instant::now();
                    bytes_since_sample = 0;
                }
            }

            file.flush().map_err(|e| format!("flush: {e}"))?;
            smol::block_on(tx.send(DownloadMsg::Done {
                path: dest,
                total: bytes_total,
            }));
            Ok(())
        };

        if let Err(e) = run() {
            smol::block_on(tx.send(DownloadMsg::Error(e)));
        }
    });

    cx.spawn(async move |this, cx| {
        while let Ok(msg) = rx.recv().await {
            match msg {
                DownloadMsg::Progress {
                    bytes_received,
                    total,
                    speed_bps,
                } => {
                    cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            if let Some(DownloadState::InProgress {
                                bytes_received: br,
                                total_bytes: tb,
                                speed_bps: sb,
                                speed_history: sh,
                                ..
                            }) = view.download_state.get_mut(&uid)
                            {
                                *br = bytes_received;
                                *tb = total;
                                *sb = speed_bps;
                                sh.push(speed_bps);
                                if sh.len() > 60 {
                                    sh.remove(0);
                                }
                            }
                            cx.notify();
                        });
                    });
                }
                DownloadMsg::Done { path, total } => {
                    cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            let filename = view
                                .download_state
                                .get(&uid)
                                .map(|s| s.filename().to_string())
                                .unwrap_or_default();
                            view.download_state.insert(
                                uid.clone(),
                                DownloadState::Done {
                                    filename,
                                    path,
                                    total_bytes: total,
                                },
                            );
                            cx.notify();
                        });
                    });
                    break;
                }
                DownloadMsg::Error(msg) => {
                    cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            let filename = view
                                .download_state
                                .get(&uid)
                                .map(|s| s.filename().to_string())
                                .unwrap_or_default();
                            view.download_state.insert(
                                uid.clone(),
                                DownloadState::Error {
                                    filename,
                                    message: msg,
                                },
                            );
                            cx.notify();
                        });
                    });
                    break;
                }
            }
        }
    })
    .detach();
}

pub(crate) fn on_toggle_like(
    window: &mut FabSearchWindow,
    uid: String,
    cx: &mut Context<FabSearchWindow>,
) {
    let token = match &window.api_token {
        Some(t) => t.clone(),
        None => return,
    };
    if window.like_inflight.contains(&uid) {
        return;
    }
    let currently_liked = window.liked_uids.contains(&uid);
    if currently_liked {
        window.liked_uids.remove(&uid);
    } else {
        window.liked_uids.insert(uid.clone());
    }
    window.like_inflight.insert(uid.clone());
    cx.notify();

    let (tx, rx) = smol::channel::bounded::<Result<(), String>>(1);
    let uid_thread = uid.clone();
    std::thread::spawn(move || {
        let result = if currently_liked {
            sketchfab_unlike_model(&uid_thread, &token)
        } else {
            sketchfab_like_model(&uid_thread, &token)
        };
        smol::block_on(tx.send(result));
    });

    cx.spawn(async move |this, cx| {
        if let Ok(result) = rx.recv().await {
            cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.like_inflight.remove(&uid);
                    if let Err(_) = result {
                        if currently_liked {
                            view.liked_uids.insert(uid);
                        } else {
                            view.liked_uids.remove(&uid);
                        }
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

pub(crate) fn on_open_item_detail(
    window: &mut FabSearchWindow,
    uid: String,
    cx: &mut Context<FabSearchWindow>,
) {
    if window.selected_item_uid.as_deref() == Some(&uid) {
        return;
    }
    window.selected_item_uid = Some(uid.clone());
    window.item_detail = None;
    window.detail_loading = true;
    window.detail_error = None;
    window.selected_gallery_idx = 0;
    cx.notify();

    let (tx, rx) = smol::channel::bounded::<(
        Vec<String>,
        Result<Box<crate::parser::SketchfabModelDetail>, String>,
    )>(1);
    std::thread::spawn(move || {
        smol::block_on(tx.send(fetch_sketchfab_model_detail(&uid)));
    });

    cx.spawn(async move |this, cx| {
        if let Ok((_, result)) = rx.recv().await {
            cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.detail_loading = false;
                    match result {
                        Ok(detail) => {
                            let urls: Vec<String> = detail
                                .all_thumbnail_urls()
                                .into_iter()
                                .take(12)
                                .map(|s| s.to_string())
                                .collect();
                            for url in urls {
                                view.ensure_image_loaded(url, cx);
                            }
                            if let Some(ref user) = detail.user {
                                if let Some(url) = user.avatar_url(128) {
                                    view.ensure_image_loaded(url.to_string(), cx);
                                }
                            }
                            view.item_detail = Some(detail);
                        }
                        Err(e) => {
                            view.detail_error = Some(e);
                        }
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

pub(crate) fn on_begin_search(window: &mut FabSearchWindow, cx: &mut Context<FabSearchWindow>) {
    if window.search_query.trim().is_empty() {
        return;
    }

    let url = window.build_search_url();
    window.is_loading = true;
    window.is_loading_more = false;
    window.results.clear();
    window.next_url = None;
    window.error = None;
    window.last_url = Some(url.clone());
    cx.notify();

    let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<SearchPage, String>)>(1);
    std::thread::spawn(move || {
        smol::block_on(tx.send(fetch_sketchfab_models(&url)));
    });

    cx.spawn(async move |this, cx| {
        if let Ok((_, result)) = rx.recv().await {
            cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.is_loading = false;
                    match result {
                        Ok(page) => {
                            view.next_url = page.next;
                            view.results = page.models;
                            let thumb_urls: Vec<String> = view
                                .results
                                .iter()
                                .filter_map(|m| m.thumb_url(260).map(|s| s.to_string()))
                                .collect();
                            for url in thumb_urls {
                                view.ensure_image_loaded(url, cx);
                            }
                        }
                        Err(e) => {
                            view.error = Some(e);
                        }
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

pub(crate) fn on_load_more(window: &mut FabSearchWindow, cx: &mut Context<FabSearchWindow>) {
    let url = match window.next_url.clone() {
        Some(u) => u,
        None => return,
    };
    window.is_loading_more = true;
    cx.notify();

    let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<SearchPage, String>)>(1);
    std::thread::spawn(move || {
        smol::block_on(tx.send(fetch_sketchfab_models(&url)));
    });

    cx.spawn(async move |this, cx| {
        if let Ok((_, result)) = rx.recv().await {
            cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.is_loading_more = false;
                    match result {
                        Ok(page) => {
                            view.next_url = page.next;
                            let thumb_urls: Vec<String> = page
                                .models
                                .iter()
                                .filter_map(|m| m.thumb_url(260).map(|s| s.to_string()))
                                .collect();
                            view.results.extend(page.models);
                            for url in thumb_urls {
                                view.ensure_image_loaded(url, cx);
                            }
                        }
                        Err(e) => {
                            view.error = Some(e);
                        }
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}
