use crate::gist_storage;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

static LISTENER: once_cell::sync::Lazy<Mutex<Option<ListenerHandle>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

struct ListenerHandle {
    shutdown: tokio::sync::oneshot::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

/// Start the notification WebSocket listener in a background thread.
/// Reads the user's home server from their gist and connects to its WS endpoint.
pub fn start() {
    let mut guard = LISTENER.lock();
    if guard.is_some() {
        tracing::info!("[NotificationListener] already running");
        return;
    }

    let token = match pulsar_auth::load_access_token() {
        Ok(Some(t)) => t,
        _ => {
            tracing::warn!("[NotificationListener] no token available");
            return;
        }
    };
    let username = match gist_storage::get_own_username() {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("[NotificationListener] failed to get username: {:?}", e);
            return;
        }
    };
    let home_servers = gist_storage::read_engine_friends_file_meta(&username).unwrap_or_default();
    let home_server = match home_servers.first() {
        Some(hs) => hs.clone(),
        None => {
            tracing::warn!("[NotificationListener] no home server configured");
            return;
        }
    };

    let hs = home_server.clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let thread = std::thread::Builder::new()
        .name("notification-listener".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for notification listener");

            rt.block_on(run_loop(&hs, &token, &username, shutdown_rx));
        })
        .expect("failed to spawn notification listener thread");

    *guard = Some(ListenerHandle {
        shutdown: shutdown_tx,
        thread,
    });
    tracing::info!(
        "[NotificationListener] started, connecting to {}",
        home_server
    );
}

/// Stop the notification WebSocket listener.
pub fn stop() {
    let mut guard = LISTENER.lock();
    if let Some(handle) = guard.take() {
        let _ = handle.shutdown.send(());
        let _ = handle.thread.join();
        tracing::info!("[NotificationListener] stopped");
    }
}

/// Take all pending notifications received via WebSocket.
pub fn take_notifications() -> Vec<serde_json::Value> {
    NOTIFICATIONS.lock().drain(..).collect()
}

static NOTIFICATIONS: once_cell::sync::Lazy<Mutex<VecDeque<serde_json::Value>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(VecDeque::new()));

fn push_notification(note: serde_json::Value) {
    NOTIFICATIONS.lock().push_back(note);
}

fn notifications_ws_url(home_server: &str) -> String {
    let ws_url = format!(
        "{}/api/v1/notifications/ws",
        home_server.trim_end_matches('/')
    );
    // Convert to ws:// if http://, wss:// if https://
    ws_url
        .replace("http://", "ws://")
        .replace("https://", "wss://")
}

async fn run_loop(
    home_server: &str,
    token: &str,
    username: &str,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let ws_url = notifications_ws_url(home_server);

    let mut shutdown_rx = std::pin::pin!(shutdown_rx);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx.as_mut() => {
                tracing::info!("[NotificationListener] shutdown signal received");
                return;
            }
            result = connect_and_listen(&ws_url, token, username) => {
                match result {
                    Ok(()) => {
                        tracing::info!("[NotificationListener] connection closed gracefully");
                    }
                    Err(e) => {
                        tracing::warn!("[NotificationListener] connection error: {:?}, reconnecting in 5s", e);
                    }
                }
                // Wait before reconnecting (unless shutdown)
                tokio::select! {
                    _ = &mut shutdown_rx.as_mut() => return,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                }
            }
        }
    }
}

async fn connect_and_listen(ws_url: &str, token: &str, username: &str) -> Result<(), String> {
    use async_tungstenite::tokio::connect_async;
    use async_tungstenite::tungstenite::Message;
    use futures::StreamExt;

    tracing::info!("[NotificationListener] connecting to {}", ws_url);

    let (ws_stream, _) = connect_async(ws_url)
        .await
        .map_err(|e| format!("connect: {}", e))?;
    let (mut write, mut read) = ws_stream.split();

    // Send GitHub token as first message for authentication
    write
        .send(Message::Text(token.to_string().into()))
        .await
        .map_err(|e| format!("send: {}", e))?;
    tracing::info!("[NotificationListener] authenticated as {}", username);

    // Listen for notifications
    while let Some(msg) = read.next().await {
        match msg.map_err(|e| format!("recv: {}", e))? {
            Message::Text(text) => {
                if let Ok(note) = serde_json::from_str::<serde_json::Value>(&text) {
                    tracing::info!("[NotificationListener] received notification: {:?}", note);
                    push_notification(note);
                }
            }
            Message::Close(_) | Message::Ping(_) | Message::Pong(_) => {}
            _ => {}
        }
    }

    tracing::info!("[NotificationListener] WebSocket closed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_maps_schemes_and_trims_trailing_slash() {
        assert_eq!(
            notifications_ws_url("https://relay.test/"),
            "wss://relay.test/api/v1/notifications/ws"
        );
        assert_eq!(
            notifications_ws_url("http://relay.test:8080"),
            "ws://relay.test:8080/api/v1/notifications/ws"
        );
    }

    #[test]
    fn take_notifications_drains_in_fifo_order() {
        // Only test touching the process-wide queue.
        take_notifications();
        push_notification(serde_json::json!({"n": 1}));
        push_notification(serde_json::json!({"n": 2}));
        let taken = take_notifications();
        assert_eq!(
            taken,
            vec![serde_json::json!({"n": 1}), serde_json::json!({"n": 2})]
        );
        assert!(take_notifications().is_empty());
    }
}
