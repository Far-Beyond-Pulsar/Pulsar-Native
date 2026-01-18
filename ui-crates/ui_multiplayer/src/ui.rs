//! UI rendering for the multiplayer window

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::Button,
    clipboard::Clipboard,
    h_flex, v_flex,
    input::TextInput,
    tab::{Tab, TabBar},
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt,
};

use super::state::MultiplayerWindow;
use super::types::*;
use super::utils::format_timestamp;

impl MultiplayerWindow {
    pub(super) fn render_connection_form(&self, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .p_4()
            .child(
                // Header
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::User)
                            .size(px(24.))
                            .text_color(cx.theme().primary)
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child("Multiplayer Collaboration")
                    )
            )
            .child(
                // Server Address Input
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .text_color(cx.theme().foreground)
                            .child("Server Address")
                    )
                    .child(
                        TextInput::new(&self.server_address_input)
                    )
            )
            .child(
                // Create or Join Section
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .text_color(cx.theme().muted_foreground)
                            .child("CREATE NEW SESSION")
                    )
                    .child(
                        Button::new("create-session")
                            .label("Create New Session")
                            .icon(IconName::Plus)
                            .w_full()
                            .disabled(self.server_address_input.read(cx).text().to_string().is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_session(window, cx);
                            }))
                    )
            )
            .child(
                // Divider
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().h(px(1.)).bg(cx.theme().border))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("OR")
                    )
                    .child(div().flex_1().h(px(1.)).bg(cx.theme().border))
            )
            .child(
                // Join Session Section
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .text_color(cx.theme().muted_foreground)
                            .child("JOIN EXISTING SESSION")
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child("Session ID")
                            )
                            .child(
                                TextInput::new(&self.session_id_input)
                            )
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child("Password")
                            )
                            .child(
                                TextInput::new(&self.session_password_input)
                            )
                    )
                    .child(
                        Button::new("join-session")
                            .label("Join Session")
                            .icon(IconName::LogIn)
                            .w_full()
                            .disabled(
                                self.server_address_input.read(cx).text().to_string().is_empty() ||
                                self.session_id_input.read(cx).text().to_string().is_empty() ||
                                self.session_password_input.read(cx).text().to_string().is_empty()
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.join_session(window, cx);
                            }))
                    )
            )
            .when_some(
                match &self.connection_status {
                    ConnectionStatus::Error(msg) => Some(msg.clone()),
                    _ => None,
                },
                |this, error_msg| {
                    this.child(
                        div()
                            .p_3()
                            .rounded(px(6.))
                            .bg(cx.theme().danger.opacity(0.1))
                            .border_1()
                            .border_color(cx.theme().danger)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(px(16.))
                                            .text_color(cx.theme().danger)
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().danger)
                                            .child(error_msg)
                                    )
                            )
                    )
                }
            )
    }


    pub(super) fn render_tab_bar(&self, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        let selected_index = match self.current_tab {
            SessionTab::Info => 0,
            SessionTab::Presence => 1,
            SessionTab::FileSync => 2,
            SessionTab::Chat => 3,
        };

        let out_of_sync_count = self.file_assets.iter()
            .filter(|a| a.status != FileSyncStatus::Synced)
            .count();

        TabBar::new("multiplayer-tabs")
            .w_full()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .selected_index(selected_index)
            .on_click(cx.listener(|this, ix: &usize, _window, cx| {
                this.current_tab = match ix {
                    0 => SessionTab::Info,
                    1 => SessionTab::Presence,
                    2 => SessionTab::FileSync,
                    3 => SessionTab::Chat,
                    _ => SessionTab::Info,
                };
                cx.notify();
            }))
            .child(Tab::new("Info"))
            .child(Tab::new("Presence"))
            .child(Tab::new(if out_of_sync_count > 0 {
                format!("Files ({})", out_of_sync_count)
            } else {
                "Files".to_string()
            }))
            .child(Tab::new(if !self.chat_messages.is_empty() {
                format!("Chat ({})", self.chat_messages.len())
            } else {
                "Chat".to_string()
            }))
    }


    pub(super) fn render_session_info_tab(&self, session: &ActiveSession, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        let session_id = session.session_id.clone();
        let join_token = session.join_token.clone();
        let server_address = session.server_address.clone();

        v_flex()
            .gap_3()
            .p_4()
            .child(
                // Session ID with copy button
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("SESSION ID")
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(session_id.clone())
                            )
                            .child(
                                Clipboard::new("copy-session-id")
                                    .value_fn({
                                        let id = session_id.clone();
                                        move |_, _| SharedString::from(id.clone())
                                    })
                                    .on_copied(|_, window, cx| {
                                        tracing::debug!("Session ID copied to clipboard");
                                    })
                            )
                    )
            )
            .child(
                // Password with copy button
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("PASSWORD")
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(join_token.clone())
                            )
                            .child(
                                Clipboard::new("copy-password")
                                    .value_fn({
                                        let token = join_token.clone();
                                        move |_, _| SharedString::from(token.clone())
                                    })
                                    .on_copied(|_, window, cx| {
                                        tracing::debug!("Password copied to clipboard");
                                    })
                            )
                    )
            )
            .child(
                // Server address
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("SERVER")
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(server_address)
                    )
            )
            .child(
                // Share join command with copy button
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("SHARE WITH TEAMMATES")
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .bg(cx.theme().secondary)
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().foreground)
                                    .child(format!("Join: {} / {}", session_id, join_token))
                            )
                            .child(
                                Clipboard::new("copy-join-command")
                                    .value_fn({
                                        let id = session_id.clone();
                                        let token = join_token.clone();
                                        move |_, _| SharedString::from(format!("Session: {}\nPassword: {}", id, token))
                                    })
                                    .on_copied(|_, window, cx| {
                                        tracing::debug!("Join credentials copied to clipboard");
                                    })
                            )
                    )
            )
            .child(
                div()
                    .h(px(1.))
                    .w_full()
                    .bg(cx.theme().border)
            )
            .child(
                // Users list with enhanced display
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::User)
                                    .size(px(16.))
                                    .text_color(cx.theme().primary)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_bold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} PARTICIPANT{}",
                                        session.connected_users.len(),
                                        if session.connected_users.len() == 1 { "" } else { "S" }
                                    ))
                            )
                            .child(
                                // Active indicator
                                div()
                                    .ml_auto()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(rgb(0x00ff00))
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Active")
                                    )
                            )
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .children(
                                self.format_participants(&session.connected_users).iter().map(|user| {
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .px_3()
                                        .py_2()
                                        .rounded(px(6.))
                                        .bg(cx.theme().secondary)
                                        .child(
                                            Icon::new(IconName::User)
                                                .size(px(14.))
                                                .text_color(cx.theme().muted_foreground)
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().foreground)
                                                .child(user.clone())
                                        )
                                        .when(user.contains("(Host)"), |this| {
                                            this.child(
                                                div()
                                                    .ml_auto()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded(px(4.))
                                                    .bg(cx.theme().primary)
                                                    .text_xs()
                                                    .font_bold()
                                                    .text_color(cx.theme().primary_foreground)
                                                    .child("HOST")
                                            )
                                        })
                                        .into_any_element()
                                })
                            )
                            .when(session.connected_users.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_center()
                                        .py_4()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No participants yet")
                                )
                            })
                    )
            )
            .child(
                // Disconnect button at bottom
                div()
                    .mt_4()
                    .child(
                        Button::new("disconnect")
                            .label("Disconnect")
                            .w_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.disconnect(window, cx);
                            }))
                    )
            )
    }


    pub(super) fn render_chat_tab(&self, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                // Messages - scrollable container
                div()
                    .flex_1()
                    .p_4()
                    .id("chat-messages")
                    .child(
                        v_flex()
                            .gap_3()
                            .when(self.chat_messages.is_empty(), |this| {
                                this.child(
                                    v_flex()
                                        .size_full()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::ChatBubble)
                                                .size(px(48.))
                                                .text_color(cx.theme().muted_foreground.opacity(0.3))
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No messages yet")
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                                .child("Start chatting with your team!")
                                        )
                                )
                            })
                            .children(
                                self.chat_messages.iter().map(|msg| {
                                    let peer_name = if msg.is_self {
                                        "You".to_string()
                                    } else {
                                        // Shorten peer ID for chat display
                                        if msg.peer_id.len() > 8 {
                                            format!("{}...", &msg.peer_id[..8])
                                        } else {
                                            msg.peer_id.clone()
                                        }
                                    };

                                    let timestamp_str = format_timestamp(msg.timestamp);

                                    v_flex()
                                        .gap_1()
                                        .when(msg.is_self, |this| this.items_end())
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_baseline()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(cx.theme().foreground)
                                                        .child(peer_name)
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(timestamp_str)
                                                )
                                        )
                                        .child(
                                            div()
                                                .max_w(px(400.))
                                                .px_3()
                                                .py_2()
                                                .rounded(px(8.))
                                                .bg(if msg.is_self {
                                                    cx.theme().primary
                                                } else {
                                                    cx.theme().secondary
                                                })
                                                .text_sm()
                                                .text_color(if msg.is_self {
                                                    cx.theme().primary_foreground
                                                } else {
                                                    cx.theme().foreground
                                                })
                                                .child(msg.message.clone())
                                        )
                                        .into_any_element()
                                })
                            )
                    )
            )
            .child(
                // Input area
                v_flex()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                TextInput::new(&self.chat_input)
                                    .flex_1()
                            )
                            .child(
                                Button::new("send")
                                    .label("Send")
                                    .icon(IconName::Send)
                                    .disabled(self.chat_input.read(cx).text().len() == 0)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_chat_message(window, cx);
                                    }))
                            )
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Press Enter to send")
                    )
            )
    }

    pub(super) fn render_file_sync_tab(&self, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        // Show progress if sync is in progress
        if self.file_sync_in_progress {
            tracing::debug!("RENDER: Showing sync progress - message: {:?}, percent: {:?}",
                self.sync_progress_message, self.sync_progress_percent);

            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child("Synchronizing files...")
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.sync_progress_message.clone().unwrap_or_else(|| "Starting...".to_string()))
                )
                .child(
                    // Progress bar container
                    div()
                        .w(px(400.))
                        .h(px(12.))
                        .rounded(px(6.))
                        .bg(cx.theme().secondary)
                        .child(
                            // Progress bar fill
                            div()
                                .w(px(400. * self.sync_progress_percent.unwrap_or(0.0)))
                                .h_full()
                                .rounded(px(6.))
                                .bg(cx.theme().accent)
                        )
                )
                .child(
                    // Percentage text
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{}%", (self.sync_progress_percent.unwrap_or(0.0) * 100.0) as u32))
                )
                .into_any_element();
        }

        // Check if there's a pending file sync or files to review
        if let Some((diff, host_peer_id)) = &self.pending_file_sync {
            tracing::debug!("Rendering FileSync tab with pending diff");

            // Show the new studio-quality file sync UI with action bar
            v_flex()
                .size_full()
                .child(
                    // Action bar at top
                    div()
                        .p_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().secondary)
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child(format!("Synchronize with {}", host_peer_id))
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .text_xs()
                                                .child(
                                                    div()
                                                        .text_color(cx.theme().success)
                                                        .child(format!("+{} added", diff.files_to_add.len()))
                                                )
                                                .child(
                                                    div()
                                                        .text_color(cx.theme().warning)
                                                        .child(format!("~{} modified", diff.files_to_update.len()))
                                                )
                                                .child(
                                                    div()
                                                        .text_color(cx.theme().danger)
                                                        .child(format!("-{} deleted", diff.files_to_delete.len()))
                                                )
                                        )
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("sync-cancel")
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.cancel_file_sync(cx);
                                                }))
                                        )
                                        .child(
                                            Button::new("sync-approve")
                                                .label("Sync Files")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.approve_file_sync(cx);
                                                }))
                                        )
                                )
                        )
                )
                .child(
                    // Diff Viewer for file sync
                    div()
                        .flex_1()
                        .child(self.diff_viewer.clone())
                )
                .into_any_element()
        } else {
            tracing::debug!("Rendering FileSync tab - no pending sync");
            // No pending sync - show sync status and dev button
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    Icon::new(IconName::Check)
                        .size(px(48.))
                        .text_color(cx.theme().success)
                )
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child("Files are synchronized")
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Your project is up to date with the session")
                )
                .child(
                    // Dev button to simulate diff
                    div()
                        .pt_8()
                        .child(
                            Button::new("simulate-diff-dev")
                                .label("Simulate Diff (Dev)")
                                .icon(IconName::Code)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.simulate_diff_for_dev(window, cx);
                                }))
                        )
                )
                .into_any_element()
        }
    }

    pub(super) fn render_presence_tab(&self, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        // Check if we're the host (for kick permissions)
        let is_host = self.active_session.as_ref()
            .and_then(|s| s.connected_users.first())
            .map(|first_peer| Some(first_peer) == self.current_peer_id.as_ref())
            .unwrap_or(false);

        v_flex()
            .size_full()
            .child(
                // Header
                v_flex()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Activity)
                                    .size(px(20.))
                                    .text_color(cx.theme().primary)
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_bold()
                                    .text_color(cx.theme().foreground)
                                    .child("User Presence & Management")
                            )
                    )
                    .when(is_host, |this| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(4.))
                                .bg(cx.theme().primary.opacity(0.1))
                                .text_xs()
                                .text_color(cx.theme().primary)
                                .child("🔑 You have host privileges")
                        )
                    })
            )
            .child(
                // User list - scrollable
                div()
                    .flex_1()
                    .p_4()
                    .child(
                        v_flex()
                            .gap_3()
                            .when(self.user_presences.is_empty(), |this| {
                                this.child(
                                    v_flex()
                                        .size_full()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::User)
                                                .size(px(48.))
                                                .text_color(cx.theme().muted_foreground.opacity(0.3))
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No users connected")
                                        )
                                )
                            })
                            .children(
                                self.user_presences.iter().map(|presence| {
                                    let is_self = Some(&presence.peer_id) == self.current_peer_id.as_ref();
                                    let short_id = if presence.peer_id.len() > 8 {
                                        format!("{}...", &presence.peer_id[..8])
                                    } else {
                                        presence.peer_id.clone()
                                    };

                                    let (r, g, b) = (presence.color[0], presence.color[1], presence.color[2]);
                                    let color_value = ((r * 255.0) as u32) << 16 | ((g * 255.0) as u32) << 8 | ((b * 255.0) as u32);

                                    let jump_id = SharedString::from(format!("jump-{}", presence.peer_id));
                                    let kick_id = SharedString::from(format!("kick-{}", presence.peer_id));
                                    let peer_id_for_jump = presence.peer_id.clone();
                                    let peer_id_for_kick = presence.peer_id.clone();

                                    v_flex()
                                        .gap_3()
                                        .px_4()
                                        .py_3()
                                        .rounded(px(8.))
                                        .bg(cx.theme().secondary)
                                        .border_l(px(4.))
                                        .border_color(rgb(color_value))
                                        .child(
                                            // Header: name + status
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .size(px(10.))
                                                        .rounded_full()
                                                        .bg(if presence.is_idle {
                                                            rgb(0x888888)
                                                        } else {
                                                            rgb(0x00ff00)
                                                        })
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_sm()
                                                        .font_bold()
                                                        .text_color(cx.theme().foreground)
                                                        .child(if is_self {
                                                            format!("{} (You)", short_id)
                                                        } else {
                                                            short_id
                                                        })
                                                )
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(4.))
                                                        .bg(if presence.is_idle {
                                                            cx.theme().muted
                                                        } else {
                                                            cx.theme().primary.opacity(0.2)
                                                        })
                                                        .text_xs()
                                                        .text_color(if presence.is_idle {
                                                            cx.theme().muted_foreground
                                                        } else {
                                                            cx.theme().primary
                                                        })
                                                        .child(presence.activity_status().to_string())
                                                )
                                        )
                                        .child(
                                            // Activity details
                                            v_flex()
                                                .gap_1()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .when_some(presence.current_tab.as_ref(), |this, tab| {
                                                    this.child(
                                                        h_flex()
                                                            .gap_1()
                                                            .child(Icon::new(IconName::Eye).size(px(12.)))
                                                            .child(format!("Viewing: {}", tab))
                                                    )
                                                })
                                                .when_some(presence.editing_file.as_ref(), |this, file| {
                                                    this.child(
                                                        h_flex()
                                                            .gap_1()
                                                            .child(Icon::new(IconName::Edit).size(px(12.)))
                                                            .child(format!("Editing: {}", file))
                                                    )
                                                })
                                                .when_some(presence.selected_object.as_ref(), |this, obj| {
                                                    this.child(
                                                        h_flex()
                                                            .gap_1()
                                                            .child(Icon::new(IconName::Check).size(px(12.)))
                                                            .child(format!("Selected: {}", obj))
                                                    )
                                                })
                                        )
                                        .when(!is_self, |this| {
                                            this.child(
                                                // Divider
                                                div()
                                                    .h(px(1.))
                                                    .w_full()
                                                    .bg(cx.theme().border.opacity(0.5))
                                            )
                                            .child(
                                                // Action buttons inside card
                                                h_flex()
                                                    .gap_2()
                                                    .w_full()
                                                    .child(
                                                        Button::new(jump_id)
                                                            .label("Jump to View")
                                                            .icon(IconName::Eye)
                                                            .flex_1()
                                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                                this.jump_to_user_view(peer_id_for_jump.clone(), window, cx);
                                                            }))
                                                    )
                                                    .when(is_host, |this| {
                                                        this.child(
                                                            Button::new(kick_id)
                                                                .label("Kick")
                                                                .icon(IconName::Close)
                                                                .flex_1()
                                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                                    this.kick_user(peer_id_for_kick.clone(), window, cx);
                                                                }))
                                                        )
                                                    })
                                            )
                                        })
                                        .into_any_element()
                                })
                            )
                    )
            )
    }



    pub(super) fn render_active_session(&self, session: &ActiveSession, cx: &mut Context<MultiplayerWindow>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_tab_bar(cx))
            .child(
                match self.current_tab {
                    SessionTab::Info => self.render_session_info_tab(session, cx).into_any_element(),
                    SessionTab::Presence => self.render_presence_tab(cx).into_any_element(),
                    SessionTab::FileSync => self.render_file_sync_tab(cx).into_any_element(),
                    SessionTab::Chat => self.render_chat_tab(cx).into_any_element(),
                }
            )
    }
}
