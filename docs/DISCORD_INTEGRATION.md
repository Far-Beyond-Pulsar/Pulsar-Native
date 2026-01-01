# Discord Rich Presence Integration

This document explains how Discord Rich Presence is integrated into the Pulsar Game Engine and how to configure it.

## Overview

Discord Rich Presence allows your Discord profile to show:
- **Current Project**: The name of the project you're working on
- **Active Editor Tab**: Which editor you're currently using (Script Editor, Level Editor, DAW, etc.)
- **Active File**: The file you're currently editing (when applicable)
- **Time Elapsed**: How long you've been working in the engine

## Setup Instructions

### 1. Create a Discord Application

1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Click "New Application"
3. Give it a name (e.g., "Pulsar Game Engine")
4. Click "Create"
5. Copy the **Application ID** from the application page

### 2. Configure the Engine

Open `crates/engine/src/main.rs` and find this line:

```rust
let discord_app_id = "YOUR_DISCORD_APPLICATION_ID_HERE";
```

Replace `"YOUR_DISCORD_APPLICATION_ID_HERE"` with your actual Discord Application ID:

```rust
let discord_app_id = "1234567890123456789"; // Your actual ID
```

### 3. (Optional) Add Custom Assets

You can customize the icons that appear in Discord:

1. In the [Discord Developer Portal](https://discord.com/developers/applications), select your application
2. Go to "Rich Presence" → "Art Assets"
3. Upload images for:
   - **Large Image**: Main engine icon (recommended: 1024x1024 px)
   - **Small Image**: Status indicator (recommended: 256x256 px)
4. Give each image a unique name (e.g., "pulsar_logo", "active_status")

Then update `crates/engine_state/src/discord.rs` in the `update_presence` method:

```rust
// Add custom assets
let asset = Asset::new(
    Some("https://your-cdn.com/large-icon.png".into()),
    Some("Pulsar Engine".into()),
    Some("https://your-cdn.com/small-icon.png".into()),
    Some("Active".into()),
);
activity.set_assets(Some(asset));
```

Or use Discord's uploaded assets by name:

```rust
let asset = Asset::new(
    Some("pulsar_logo".into()),  // Name from Discord Developer Portal
    Some("Pulsar Engine".into()),
    Some("active_status".into()),
    Some("Active".into()),
);
activity.set_assets(Some(asset));
```

## How It Works

### Architecture

The Discord integration is implemented in the `engine_state` crate:

- **`discord.rs`**: Core Discord Rich Presence module using `rust-discord-activity`
- **`lib.rs`**: EngineState integration for global access
- **`ui_core/app.rs`**: UI hooks to update presence when tabs/files change

### Automatic Updates

The presence automatically updates when:

1. **Project is loaded**: Shows the project name
2. **Tab is switched**: Updates the active editor type
3. **File is opened**: Updates the active file name
4. **Tab is closed**: Refreshes the presence state

### Data Flow

```
User Action (Open File/Switch Tab)
    ↓
PulsarApp::update_discord_presence()
    ↓
EngineState::update_discord_presence()
    ↓
DiscordPresence::update_all()
    ↓
Discord Client (IPC)
    ↓
Discord shows status on your profile
```

## API Reference

### EngineState Methods

```rust
// Initialize Discord (called once at startup)
engine_state.init_discord("your_app_id")?;

// Get Discord presence instance
let discord = engine_state.discord();

// Update presence (called automatically by UI)
engine_state.update_discord_presence(
    Some("ProjectName".into()),
    Some("Script Editor".into()),
    Some("main.rs".into())
);
```

### DiscordPresence Methods

```rust
// Create new instance
let presence = DiscordPresence::new("app_id");

// Connect to Discord
presence.connect()?;

// Disconnect
presence.disconnect();

// Check if enabled
if presence.is_enabled() { ... }

// Update individual fields
presence.set_project(Some("ProjectName".into()));
presence.set_active_tab(Some("Level Editor".into()));
presence.set_active_file(Some("scene.level".into()));

// Update all at once
presence.update_all(
    Some("Project".into()),
    Some("Tab".into()),
    Some("File".into())
);

// Get current values
let project = presence.get_project();
let tab = presence.get_active_tab();
let file = presence.get_active_file();
```

## Troubleshooting

### Discord Presence Not Showing

1. **Discord Not Running**: Make sure Discord is running before starting the engine
2. **Application ID Wrong**: Double-check your application ID in `main.rs`
3. **Activity Privacy Settings**: Check Discord Settings → Activity Privacy → "Display current activity as a status message"

### Connection Errors

If you see connection errors in the logs:

```
⚠️  Discord Rich Presence failed to initialize: ...
```

This is usually because:
- Discord isn't running
- The application ID is incorrect
- Discord IPC is disabled

The engine will continue to work normally, just without Discord integration.

### Disabling Discord Integration

To disable Discord Rich Presence, simply comment out or don't set the application ID:

```rust
// Don't change this line - it will automatically disable Discord
let discord_app_id = "YOUR_DISCORD_APPLICATION_ID_HERE";
```

Or completely remove the initialization code:

```rust
// // Initialize Discord Rich Presence
// match engine_state.init_discord(discord_app_id) {
//     Ok(_) => tracing::debug!("✅ Discord Rich Presence initialized"),
//     Err(e) => tracing::warn!("⚠️  Discord Rich Presence failed to initialize: {}", e),
// }
```

## Example Presence Display

When working on a project called "MyGame" with a script file "player.rs" open, Discord will show:

```
Playing Pulsar Game Engine
Project: MyGame | player.rs
Editing in Script Editor
Elapsed: 01:23:45
```

## Privacy Notes

The Discord integration only shares:
- Project name (from folder name)
- Editor type (Script Editor, Level Editor, etc.)
- Active filename (not full path)
- Time elapsed

No code content, file paths, or sensitive information is shared.

## Dependencies

- **rust-discord-activity**: Cross-platform Discord RPC client library from [Far-Beyond-Pulsar/Discord-RPC-RS](https://github.com/Far-Beyond-Pulsar/Discord-RPC-RS)

## License

The Discord integration follows the same license as the Pulsar Engine.
