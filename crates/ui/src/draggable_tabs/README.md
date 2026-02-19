# Draggable Tabs System

A Chrome-like draggable tab system for Pulsar, completely independent from the dock system.

## Features

✅ **Always Visible Tabs** - Tabs are always shown as tabs, even with only one tab
✅ **Drag to Reorder** - Drag tabs within the tab bar to reorder them
✅ **Drag Out to New Window** - Drag tabs outside the window bounds to create a new window
✅ **Cross-Window Docking** - Drag tabs from one window to another to redock them
✅ **Visual Feedback** - Clear border indicators when dragging over drop zones
✅ **Window Pre-Attached** - New windows spawn with the tab bar attached to cursor position

## Architecture

The system consists of two main components:

### `DraggableTab`
Individual tab component with:
- Label, icon, prefix/suffix support
- Closable button (optional)
- Selected/unselected visual states
- Drag-and-drop support built-in

### `DraggableTabBar`
Container component managing multiple tabs:
- Horizontal scrolling tab bar
- Content area showing selected tab
- Drag detection (inside vs outside window bounds)
- Window creation on drag-out
- Event emission for tab actions

## Usage Example

```rust
use gpui_component::draggable_tabs::{DraggableTabBar, TabBarEvent};

// In your window creation
let tab_bar = cx.new(|cx| {
    let mut bar = DraggableTabBar::new("main-tabs", window, cx);

    // Add tabs
    bar.add_tab(
        "tab1",
        "Welcome",
        cx.new(|cx| WelcomePanel::new(cx)).into(),
        true  // closable
    );

    bar.add_tab(
        "tab2",
        "Editor",
        cx.new(|cx| EditorPanel::new(cx)).into(),
        false  // not closable
    );

    bar
});

// Listen to events
cx.subscribe(&tab_bar, |_this, _bar, event, _window, cx| {
    match event {
        TabBarEvent::TabSelected(ix) => {
            tracing::trace!("Tab {} selected", ix);
        }
        TabBarEvent::TabClosed(ix) => {
            tracing::trace!("Tab {} closed", ix);
        }
        TabBarEvent::TabReordered { from, to } => {
            tracing::trace!("Tab moved from {} to {}", from, to);
        }
        TabBarEvent::TabDropped { tab, at_index } => {
            // Handle tab dropped from another window
            tracing::trace!("Tab '{}' dropped at index {}", tab.label, at_index);
        }
    }
})
.detach();
```

## How It Works

### Drag Within Tab Bar
- Tabs show a left border indicator when hovering
- Dropping reorders the tab to that position
- Selected index automatically adjusts

### Drag Outside Window
- System detects when drag cursor leaves window bounds (with 20px margin)
- On drop outside, creates a new window with:
  - The dragged tab as the only tab
  - Window positioned so tab bar is under cursor
  - Fully functional DraggableTabBar in new window

### Cross-Window Docking
- Each `DraggedTab` carries its `tab_bar_id`
- Drop handler checks if source bar == target bar
- If different, emits `TabDropped` event
- Parent can handle adding tab to target bar

## Integration with Main Window

To replace the existing dock/tab system:

1. **Remove dock-based tab panel** from main window
2. **Create DraggableTabBar** as root component
3. **Add your panels** as tab content (AnyView)
4. **Handle TabBarEvent::TabDropped** to accept tabs from other windows

Example:

```rust
// In PulsarApp or main window creation
let tab_bar = cx.new(|cx| {
    let mut bar = DraggableTabBar::new("main", window, cx);
    bar.add_tab("level-editor", "Level Editor", level_editor_view.into(), false);
    bar.add_tab("blueprint-editor", "Blueprint", blueprint_view.into(), true);
    bar.add_tab("daw", "DAW", daw_view.into(), true);
    bar
});

// Wrap in Root
cx.new(|cx| Root::new(tab_bar.into(), window, cx))
```

## Visual Behavior

### Tab Appearance
- **Inactive**: Transparent background, dimmed text
- **Hover**: Semi-transparent active background preview
- **Active**: Full active background, accent top border (2px)
- **Dragging**: Border indicators on drop zones

### Window Creation
- New window spawns at cursor position
- Window bounds calculated so tab bar aligns under cursor
- Creates illusion of "carrying" the tab with the mouse
- Drop anywhere to finalize window creation

## Events

All events are strongly typed:

```rust
pub enum TabBarEvent {
    TabSelected(usize),           // User clicked tab
    TabClosed(usize),             // User clicked close button
    TabReordered { from, to },    // User dragged to reorder
    TabDropped { tab, at_index }, // Tab from another window dropped here
}
```

## Limitations & Future Work

- **No Tab Persistence**: Tabs are not saved/restored across sessions
- **No Tab Context Menu**: Right-click context menu not implemented
- **No Tab Groups**: Cannot group related tabs
- **No Tab Pinning**: Cannot pin tabs to prevent closing/reordering

## vs. Dock System

| Feature | Dock System | Draggable Tabs |
|---------|-------------|----------------|
| Complexity | High (StackPanel, TabPanel, DockArea hierarchy) | Low (just TabBar + Tabs) |
| Splitting | Yes (vertical/horizontal splits) | No (single horizontal bar) |
| Docking Areas | Yes (left/right/bottom/center) | No (one bar per window) |
| Drag to New Window | Fake (creates dock in new window) | Real (simple new window) |
| Always Show Tabs | No (hides with 1 tab) | Yes (always visible) |
| Chrome-like UX | No | Yes |

Use **Draggable Tabs** when you want a simple, Chrome-like tab experience.
Use **Dock System** when you need complex panel layouts with splits.

