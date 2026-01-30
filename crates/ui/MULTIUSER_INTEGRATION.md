# Multi-User Editing Integration Guide

This guide shows how to integrate the new multi-user replication system with Pulsar Engine.

## Architecture Overview

The multi-user editing system consists of three layers:

1. **UI Layer** (`crates/ui/src/replication`) - State tracking and UI components
2. **Networking Layer** (existing `multiuser_server` & `multiuser_client`) - Message transport
3. **Application Layer** - Glue code that connects UI to networking

## Quick Start

### 1. Enable Replication in Your Application

The replication system is automatically initialized when you call `ui::init(cx)`:

```rust
use gpui::App;

fn main() {
    let app = App::new();
    app.run(|cx| {
        // Initialize UI (includes replication)
        ui::init(cx);

        // Your app code...
    });
}
```

### 2. Make a Component Replicated

Add replication fields to any component state:

```rust
use ui::replication::{ReplicationConfig, ReplicationMode, Replicator};

pub struct MyComponentState {
    // ... existing fields ...

    // Add these:
    replication_id: String,
    replication_config: ReplicationConfig,
}

impl MyComponentState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            // ... existing initialization ...

            replication_id: format!("my_component_{}", cx.entity_id()),
            replication_config: ReplicationConfig::new(ReplicationMode::MultiEdit)
                .with_debounce(100)
                .with_presence(true),
        }
    }
}

// Implement the Replicator trait
impl Replicator for MyComponentState {
    fn replication_id(&self) -> String {
        self.replication_id.clone()
    }

    fn replication_config(&self) -> &ReplicationConfig {
        &self.replication_config
    }

    fn replication_config_mut(&mut self) -> &mut ReplicationConfig {
        &mut self.replication_config
    }

    fn serialize_state(&self, _cx: &App) -> Result<serde_json::Value, String> {
        Ok(json!({
            "value": self.value,
            // ... other fields to sync ...
        }))
    }

    fn deserialize_state(
        &mut self,
        state: serde_json::Value,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let value = state.get("value")
            .and_then(|v| v.as_str())
            .ok_or("Missing value")?;

        self.set_value(value.to_string(), window, cx);
        Ok(())
    }
}
```

### 3. Connect to Multiuser Network

Integrate with the existing multiuser client:

```rust
use ui::replication::{ReplicationMessageHandler, ReplicationMessage};
use engine_backend::subsystems::networking::multiuser::{MultiuserClient, ServerMessage};

async fn handle_multiuser_messages(
    client: Arc<RwLock<MultiuserClient>>,
    cx: &mut App,
) {
    let mut handler = ReplicationMessageHandler::new(cx);

    // Listen for server messages
    loop {
        let client_guard = client.read().await;
        if let Some(message) = client_guard.receive().await {
            match message {
                ServerMessage::ReplicationUpdate { data } => {
                    // Deserialize and handle replication message
                    if let Ok(rep_msg) = serde_json::from_str::<ReplicationMessage>(&data) {
                        if let Some(response) = handler.handle_message(rep_msg) {
                            // Send response back
                            let json = serde_json::to_string(&response).unwrap();
                            client_guard.send_replication(json).await;
                        }
                    }
                }
                // ... handle other message types ...
                _ => {}
            }
        }
    }
}
```

### 4. Extend ServerMessage and ClientMessage

Add replication support to your multiuser protocol:

```rust
// In crates/engine_backend/src/subsystems/networking/multiuser.rs

pub enum ClientMessage {
    // ... existing variants ...

    /// Replication state update
    ReplicationUpdate { data: String },
}

pub enum ServerMessage {
    // ... existing variants ...

    /// Broadcast replication update to all peers
    ReplicationUpdate { data: String },
}
```

### 5. Add Tab Presence Indicators

Enhance your TabPanel to show which users are active:

```rust
use ui::replication::{PresenceAware, TabPresenceIndicator, UserPresence};

impl TabPanel {
    fn render_tab_with_presence(
        &self,
        panel: &Arc<dyn PanelView>,
        is_active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let panel_id = panel.panel_name(cx);

        // Get users present in this tab
        let registry = ReplicationRegistry::global(cx);
        let user_ids = registry.get_panel_users(panel_id);
        let presences = user_ids.iter()
            .filter_map(|id| registry.get_user_presence(id))
            .collect::<Vec<_>>();

        // Create tab with presence indicator
        Tab::new(panel.title(window, cx))
            .selected(is_active)
            .when(!presences.is_empty(), |this| {
                this.child(TabPresenceIndicator::new(presences))
            })
            .on_click(cx.listener({
                let panel_id = panel_id.to_string();
                move |view, _, window, cx| {
                    view.set_active_tab(ix, window, cx);

                    // Notify registry that we entered this panel
                    let registry = ReplicationRegistry::global(cx);
                    let peer_id = view.get_current_peer_id(cx);
                    registry.add_panel_presence(&panel_id, &peer_id);
                }
            }))
    }
}
```

## Replication Modes

### NoRep - Local Only

```rust
// User preferences, window positions, etc.
let config = ReplicationConfig::new(ReplicationMode::NoRep);
```

### MultiEdit - Collaborative Editing

```rust
// Script parameters, entity properties
let config = ReplicationConfig::new(ReplicationMode::MultiEdit)
    .with_debounce(100)      // Sync every 100ms
    .with_presence(true)      // Show who's editing
    .with_cursors(true);      // Show cursor positions
```

### LockedEdit - Exclusive Access

```rust
// Build settings, critical configs
let config = ReplicationConfig::new(ReplicationMode::LockedEdit)
    .with_presence(true);     // Show who has the lock
```

### RequestEdit - Moderated Changes

```rust
// Production settings, release configs
let config = ReplicationConfig::new(ReplicationMode::RequestEdit)
    .with_max_editors(1);     // Only one editor after approval
```

### BroadcastOnly - Presenter Mode

```rust
// Following host's viewport, timeline scrubbing
let config = ReplicationConfig::new(ReplicationMode::BroadcastOnly);
```

### Follow - Shadow Another User

```rust
// Learning, pair programming
let config = ReplicationConfig::new(ReplicationMode::Follow);
```

### QueuedEdit - Sequential Operations

```rust
// Animation timeline, ordered events
let config = ReplicationConfig::new(ReplicationMode::QueuedEdit);
```

### PartitionedEdit - User-Specific Sections

```rust
// Per-user annotations, individual layers
let config = ReplicationConfig::new(ReplicationMode::PartitionedEdit);
```

## Panel Presence Tracking

Track when users enter/leave panels:

```rust
impl TabPanel {
    fn on_panel_focused(&mut self, panel_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let registry = ReplicationRegistry::global(cx);
        let peer_id = self.get_current_peer_id(cx);

        // Add presence
        registry.add_panel_presence(panel_id, &peer_id);

        // Update user presence
        if let Some(mut presence) = registry.get_user_presence(&peer_id) {
            presence.current_panel = Some(panel_id.to_string());
            presence.touch();
            registry.update_user_presence(presence);
        }

        // Notify network
        let message = ReplicationMessageBuilder::panel_joined(panel_id, peer_id);
        self.send_replication_message(message, cx);
    }

    fn on_panel_blurred(&mut self, panel_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let registry = ReplicationRegistry::global(cx);
        let peer_id = self.get_current_peer_id(cx);

        // Remove presence
        registry.remove_panel_presence(panel_id, &peer_id);

        // Notify network
        let message = ReplicationMessageBuilder::panel_left(panel_id, peer_id);
        self.send_replication_message(message, cx);
    }
}
```

## Field-Level Presence Indicators

Show which user is editing a specific field:

```rust
use ui::replication::{FieldPresenceIndicator, ReplicationRegistry};

fn render_shared_input(
    input_state: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let element_id = input_state.read(cx).replication_id();
    let registry = ReplicationRegistry::global(cx);
    let editors = registry.get_editors(&element_id);

    // Get presence for other users editing this field
    let other_editors = editors.iter()
        .filter(|peer_id| *peer_id != &my_peer_id)
        .filter_map(|peer_id| registry.get_user_presence(peer_id))
        .collect::<Vec<_>>();

    TextInput::new(input_state)
        .placeholder("Shared field...")
        // Show presence indicator if others are editing
        .when_some(other_editors.first(), |this, presence| {
            this.suffix(
                FieldPresenceIndicator::new(presence.clone())
                    .locked(false)
            )
        })
        .on_focus(cx.listener({
            let element_id = element_id.clone();
            move |_, _, window, cx| {
                // Notify that we started editing
                let message = ReplicationMessageBuilder::editor_joined(&element_id, &my_peer_id);
                send_replication_message(message, cx);
            }
        }))
        .on_blur(cx.listener({
            let element_id = element_id.clone();
            move |_, _, window, cx| {
                // Notify that we stopped editing
                let message = ReplicationMessageBuilder::editor_left(&element_id, &my_peer_id);
                send_replication_message(message, cx);
            }
        }))
}
```

## Cursor Sharing (Text Inputs)

Show where other users are typing:

```rust
use ui::replication::{RemoteCursor, ReplicationRegistry};

fn render_input_with_cursors(
    input_state: &Entity<InputState>,
    cx: &mut App,
) -> impl IntoElement {
    let element_id = input_state.read(cx).replication_id();
    let registry = ReplicationRegistry::global(cx);

    // Get all editors and their cursor positions
    let editors = registry.get_editors(&element_id);
    let cursors = editors.iter()
        .filter_map(|peer_id| {
            registry.get_user_presence(peer_id).and_then(|presence| {
                presence.cursor_position.map(|pos| (presence, pos))
            })
        })
        .collect::<Vec<_>>();

    div()
        .relative()
        .child(TextInput::new(input_state))
        .children(cursors.into_iter().map(|(presence, position)| {
            RemoteCursor::new(presence, position)
        }))
}
```

## Complete Example: Shared Script Editor

```rust
use ui::replication::*;

pub struct SharedScriptEditor {
    script_content: Entity<InputState>,
    script_name: Entity<InputState>,
    enabled: Entity<CheckboxState>,
    multiuser_client: Option<Arc<RwLock<MultiuserClient>>>,
    peer_id: String,
}

impl SharedScriptEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Create replicated inputs
        let script_content = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.replication_config = ReplicationConfig::new(ReplicationMode::MultiEdit)
                .with_debounce(100)
                .with_presence(true)
                .with_cursors(true);
            state.replication_id = "script_content".to_string();
            state
        });

        let script_name = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.replication_config = ReplicationConfig::new(ReplicationMode::LockedEdit)
                .with_presence(true);
            state.replication_id = "script_name".to_string();
            state
        });

        let enabled = cx.new(|cx| {
            let mut state = CheckboxState::new();
            state.replication_config = ReplicationConfig::new(ReplicationMode::MultiEdit);
            state.replication_id = "script_enabled".to_string();
            state
        });

        // Register all replicated elements
        let registry = ReplicationRegistry::global(cx);
        registry.register_element(
            "script_content".to_string(),
            script_content.read(cx).replication_config.clone(),
        );
        registry.register_element(
            "script_name".to_string(),
            script_name.read(cx).replication_config.clone(),
        );
        registry.register_element(
            "script_enabled".to_string(),
            enabled.read(cx).replication_config.clone(),
        );

        Self {
            script_content,
            script_name,
            enabled,
            multiuser_client: None,
            peer_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn on_script_content_change(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // Update local state
        self.script_content.update(cx, |state, cx| {
            state.set_value(text.clone(), window, cx);
        });

        // Serialize and send to network
        let state_json = self.script_content.read(cx).serialize_state(cx).unwrap();
        let message = ReplicationMessageBuilder::state_update(
            "script_content",
            state_json,
            &self.peer_id,
        );

        self.send_message(message, cx);
    }

    fn send_message(&self, message: ReplicationMessage, cx: &App) {
        if let Some(client) = &self.multiuser_client {
            let json = serde_json::to_string(&message).unwrap();

            cx.spawn({
                let client = client.clone();
                async move |_this, _cx| {
                    let client_guard = client.write().await;
                    let _ = client_guard.send(ClientMessage::ReplicationUpdate {
                        data: json,
                    }).await;
                }
            }).detach();
        }
    }

    fn handle_remote_message(&mut self, message: ReplicationMessage, window: &mut Window, cx: &mut Context<Self>) {
        match message {
            ReplicationMessage::StateUpdate { element_id, state, .. } => {
                match element_id.as_str() {
                    "script_content" => {
                        self.script_content.update(cx, |input_state, cx| {
                            let _ = input_state.deserialize_state(state, window, cx);
                        });
                    }
                    "script_name" => {
                        self.script_name.update(cx, |input_state, cx| {
                            let _ = input_state.deserialize_state(state, window, cx);
                        });
                    }
                    "script_enabled" => {
                        // Update checkbox
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

impl Render for SharedScriptEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let registry = ReplicationRegistry::global(cx);

        // Get presence info
        let content_editors = registry.get_editors("script_content");
        let name_lock_holder = registry.get_element_state("script_name")
            .and_then(|state| state.locked_by)
            .and_then(|peer_id| registry.get_user_presence(&peer_id));

        v_flex()
            .gap_4()
            .p_4()
            // Script Name (locked edit)
            .child(
                v_flex()
                    .gap_2()
                    .child(Label::new("Script Name"))
                    .child(
                        TextInput::new(&self.script_name)
                            .placeholder("Enter script name...")
                            .when_some(name_lock_holder, |this, presence| {
                                this.suffix(
                                    FieldPresenceIndicator::new(presence)
                                        .locked(true)
                                )
                            })
                    )
            )
            // Script Content (multi-edit)
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(Label::new("Script Content"))
                            .when(!content_editors.is_empty(), |this| {
                                this.child(
                                    PresenceStack::new(
                                        content_editors.iter()
                                            .filter_map(|id| registry.get_user_presence(id))
                                            .collect()
                                    )
                                    .max_visible(3)
                                    .small()
                                )
                            })
                    )
                    .child(
                        TextInput::new(&self.script_content)
                            .multi_line(true)
                            .height(px(200.0))
                    )
            )
            // Enabled (simple sync)
            .child(
                Checkbox::new("enabled")
                    .label("Enabled")
                    .checked(self.enabled.read(cx).checked())
                    .on_click(cx.listener(|this, checked, window, cx| {
                        // Update and sync
                    }))
            )
    }
}
```

## Best Practices

1. **Choose the Right Mode**
   - Use `NoRep` for user-specific UI state
   - Use `MultiEdit` for collaborative properties
   - Use `LockedEdit` for critical single-editor fields
   - Use `RequestEdit` for moderated workflows

2. **Debounce Appropriately**
   - Fast typing: 100-200ms debounce
   - Sliders/continuous: 50ms
   - Dropdowns/toggles: 0ms (immediate)

3. **Show Clear Feedback**
   - Always show presence indicators for locked/multi-edit
   - Use color coding consistently (same color per user)
   - Show tooltips with user names

4. **Handle Conflicts**
   - For text: Consider operational transformation
   - For values: Last-write-wins is usually fine
   - For critical data: Use LockedEdit or RequestEdit

5. **Optimize Network Traffic**
   - Use debouncing for high-frequency updates
   - Only sync changed fields
   - Batch updates when possible

## Integration with Existing Multiuser System

Your existing multiuser system already has:
- WebSocket signaling
- Session management
- File synchronization
- Chat

Add replication support by:

1. **Extend Message Types**
```rust
// Add to ClientMessage and ServerMessage
ReplicationUpdate { data: String }
```

2. **Wire Up Message Handler**
```rust
let mut rep_handler = ReplicationMessageHandler::new(cx);

// In your message loop:
ServerMessage::ReplicationUpdate { data } => {
    let message = serde_json::from_str(&data)?;
    if let Some(response) = rep_handler.handle_message(message) {
        // Broadcast response to all peers
    }
}
```

3. **Sync Presence on Join/Leave**
```rust
ServerMessage::PeerJoined { peer_id, .. } => {
    // Create user presence
    let presence = UserPresence::new(
        peer_id.clone(),
        "User Name",
        generate_user_color(&peer_id),
    );

    let registry = ReplicationRegistry::global(cx);
    registry.update_user_presence(presence);
}

ServerMessage::PeerLeft { peer_id } => {
    let registry = ReplicationRegistry::global(cx);
    registry.remove_user_presence(&peer_id);
}
```

That's it! Your UI components are now multi-user aware.
