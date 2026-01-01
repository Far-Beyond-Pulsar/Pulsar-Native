//! Discord Rich Presence Integration
//!
//! Provides Discord activity status showing:
//! - Current project being worked on
//! - Active editor tab and file
//! - Time spent in the project

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use parking_lot::RwLock;
use rust_discord_activity::*;

/// Discord Rich Presence state
pub struct DiscordPresence {
    inner: Arc<RwLock<DiscordPresenceInner>>,
}

struct DiscordPresenceInner {
    client: Option<DiscordClient>,
    application_id: String,
    project_name: Option<String>,
    active_tab: Option<String>,
    active_file: Option<String>,
    discord_icon_key: Option<String>,  // Store the custom icon key
    start_time: u128,
    enabled: bool,
}

impl DiscordPresence {
    /// Create a new Discord presence instance
    /// 
    /// # Arguments
    /// * `application_id` - Your Discord application ID (get from https://discord.com/developers/applications)
    pub fn new(application_id: impl Into<String>) -> Self {
        let app_id = application_id.into();
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        Self {
            inner: Arc::new(RwLock::new(DiscordPresenceInner {
                client: None,
                application_id: app_id,
                project_name: None,
                active_tab: None,
                active_file: None,
                discord_icon_key: None,
                start_time,
                enabled: false,
            })),
        }
    }

    /// Connect to Discord
    pub fn connect(&self) -> anyhow::Result<()> {
        let mut inner = self.inner.write();
        
        if inner.client.is_some() {
            return Ok(()); // Already connected
        }

        let mut client = DiscordClient::new(&inner.application_id);
        client.connect().map_err(|e| anyhow::anyhow!("Failed to connect to Discord: {:?}", e))?;
        
        inner.client = Some(client);
        inner.enabled = true;
        
        Ok(())
    }

    /// Disconnect from Discord
    pub fn disconnect(&self) {
        let mut inner = self.inner.write();
        inner.client = None;
        inner.enabled = false;
    }

    /// Check if Discord Rich Presence is enabled and connected
    pub fn is_enabled(&self) -> bool {
        self.inner.read().enabled && self.inner.read().client.is_some()
    }

    /// Set the current project name
    pub fn set_project(&self, project_name: Option<String>) {
        let mut inner = self.inner.write();
        inner.project_name = project_name;
        drop(inner);
        self.update_presence();
    }

    /// Set the active editor tab type
    pub fn set_active_tab(&self, tab_name: Option<String>) {
        let mut inner = self.inner.write();
        inner.active_tab = tab_name;
        drop(inner);
        self.update_presence();
    }

    /// Set the active file being edited
    pub fn set_active_file(&self, file_path: Option<String>) {
        let mut inner = self.inner.write();
        inner.active_file = file_path;
        drop(inner);
        self.update_presence();
    }

    /// Update all presence information at once
    pub fn update_all(&self, project_name: Option<String>, tab_name: Option<String>, file_path: Option<String>) {
        self.update_all_with_icon(project_name, tab_name, file_path, None);
    }

    /// Update all presence information at once with custom Discord icon key
    pub fn update_all_with_icon(&self, project_name: Option<String>, tab_name: Option<String>, file_path: Option<String>, icon_key: Option<&'static str>) {
        tracing::debug!("🎮 Discord::update_all_with_icon called: project={:?}, tab={:?}, file={:?}, icon={:?}", 
            project_name, tab_name, file_path, icon_key);
        let mut inner = self.inner.write();
        inner.project_name = project_name;
        inner.active_tab = tab_name;
        inner.active_file = file_path;
        inner.discord_icon_key = icon_key.map(|s| s.to_string());
        drop(inner);
        self.update_presence();
    }

    /// Update the Discord presence with current state
    fn update_presence(&self) {
        let inner = self.inner.read();
        
        if !inner.enabled || inner.client.is_none() {
            tracing::debug!("⚠️  Discord presence update skipped: enabled={}, client={}", 
                inner.enabled, inner.client.is_some());
            return;
        }

        // Build the state string
        let state = if let Some(ref tab) = inner.active_tab {
            format!("Editing in {}", tab)
        } else {
            "Idle".to_string()
        };

        // Build the details string
        let details = match (&inner.project_name, &inner.active_file) {
            (Some(project), Some(file)) => {
                // Extract just the filename for brevity
                let filename = std::path::Path::new(file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file);
                format!("Project: {} | {}", project, filename)
            }
            (Some(project), None) => format!("Project: {}", project),
            (None, Some(file)) => {
                let filename = std::path::Path::new(file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file);
                format!("Editing {}", filename)
            }
            (None, None) => "Pulsar Game Engine".to_string(),
        };

        tracing::debug!("📤 Sending Discord presence: state='{}', details='{}'", state, details);

        // Create timestamp
        let timestamp = Timestamp::new(Some(inner.start_time), None);

        // Create asset with large image (main Pulsar logo) and small image (editor icon)
        let asset = if let Some(ref icon_key) = inner.discord_icon_key {
            // Use the custom icon key provided by the panel
            let small_image_text = inner.active_tab.as_ref()
                .map(|s| s.as_str())
                .unwrap_or("Editor");
            
            tracing::debug!("🎨 Using panel's Discord icon: large='pulsar_logo', small='{}', text='{}'", icon_key, small_image_text);
            
            // NOTE: You need to upload these images to your Discord Application's Rich Presence Art Assets
            // Go to: https://discord.com/developers/applications/<app_id>/rich-presence/assets
            // Each panel can define its own icon key via the discord_icon_key() method
            
            Some(Asset::new(
                Some("pulsar_logo".into()),        // Large image key (main logo)
                Some("Pulsar Engine".into()),      // Large image hover text
                Some(icon_key.clone()),            // Small image key (from panel)
                Some(small_image_text.into()),     // Small image hover text
            ))
        } else {
            // No custom icon - just show main logo
            tracing::debug!("🎨 Using default Discord icon: large='pulsar_logo', small=none");
            Some(Asset::new(
                Some("pulsar_logo".into()),
                Some("Pulsar Engine".into()),
                None,
                None,
            ))
        };

        // Create activity
        let mut activity = Activity::new();
        activity
            .set_state(Some(state.clone()))
            .set_details(Some(details.clone()))
            .set_timestamps(Some(timestamp))
            .set_activity_type(Some(ActivityType::GAME));
        
        // Set the assets!
        if let Some(asset) = asset {
            activity.set_assets(Some(asset));
        }

        let payload = Payload::new(EventName::Activity, EventData::Activity(activity));

        // Send the update
        if let Some(ref client) = inner.client {
            // Clone client to avoid holding the lock during send
            let client_ptr = client as *const DiscordClient as *mut DiscordClient;
            drop(inner);
            
            // SAFETY: We're just sending data, not modifying the client state in a conflicting way
            unsafe {
                if let Err(e) = (*client_ptr).send_payload(payload) {
                    tracing::debug!("❌ Failed to update Discord presence: {:?}", e);
                } else {
                    tracing::debug!("✅ Discord presence updated successfully!");
                }
            }
        }
    }

    /// Get current project name
    pub fn get_project(&self) -> Option<String> {
        self.inner.read().project_name.clone()
    }

    /// Get current active tab
    pub fn get_active_tab(&self) -> Option<String> {
        self.inner.read().active_tab.clone()
    }

    /// Get current active file
    pub fn get_active_file(&self) -> Option<String> {
        self.inner.read().active_file.clone()
    }
}

impl Clone for DiscordPresence {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Drop for DiscordPresenceInner {
    fn drop(&mut self) {
        // Clean disconnect when dropping
        self.client = None;
    }
}
