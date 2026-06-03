# window_manager

Centralised window management for the Pulsar engine editor.
Every window that opens in the editor goes through this crate — no `cx.open_window` calls exist outside of it.

---

## Architecture

```
window_manager
├── WindowManager   — GPUI Global; routes all cx.open_window calls; runs hooks + telemetry
├── WindowRegistry  — GPUI Global; name → opener map; drives dynamic dispatch
├── PulsarWindow    — trait every window type implements once
├── PulsarWindowExt — blanket extension trait (ui_common) that adds .open() and .register()
└── WindowConfig    — named WindowOptions presets (editor, entry, dialog, detached_panel)
```

`WindowManager` and `WindowRegistry` are both GPUI globals, installed at app startup in `main.rs` before any windows are opened.

---

## Defining a window

Implement `PulsarWindow` on your view type. That is the only thing you need to add in your crate.

```rust
// In my_crate/src/window.rs
use window_manager::{default_window_options, PulsarWindow};
use gpui::{App, Entity, Render, Window, WindowOptions};

pub struct MyWindow { /* ... */ }

impl Render for MyWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // ...
    }
}

impl PulsarWindow for MyWindow {
    /// Zero-param windows use `()`. Pass anything `Send + 'static`.
    type Params = ();

    /// Unique name used as the registry key and in telemetry.
    fn window_name() -> &'static str { "MyWindow" }

    /// Return the WindowOptions for this window.
    /// Use WindowConfig presets or default_window_options for standard sizes.
    fn window_options(_: &()) -> WindowOptions {
        default_window_options(800.0, 600.0)       // or WindowConfig::dialog(800.0, 600.0)
    }

    /// Build and return the root entity. Wrapped in Root<T> automatically by the opener.
    fn build(_: (), window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| MyWindow::new(window, cx))
    }
}
```

### WindowConfig presets

| Method | Size | Use for |
|---|---|---|
| `WindowConfig::editor()` | 1600×900 | Main editor window |
| `WindowConfig::entry()` | 1100×700 | Entry / project-selection screen |
| `WindowConfig::dialog(w, h)` | custom | Settings, About, Docs, any dialog |
| `WindowConfig::detached_panel(cursor)` | 800×600, cursor-relative | Panels popped out of the dock |

`default_window_options(w, h)` is an alias for `WindowConfig::dialog(w, h)`.

---

## Opening a window

Import `PulsarWindowExt` and call `::open`:

```rust
use ui_common::PulsarWindowExt as _;

// Zero-param window
MyWindow::open((), cx);
SettingsWindow::open((), cx);

// Parameterised window (path, entity, etc.)
GitManager::open(project_path, cx);
PulsarRoot::open(project_path, cx);          // opens the full editor
LoadingScreen::open((path, on_complete), cx);
```

`open` routes through `WindowManager` (hooks, telemetry, tracking) and wraps the entity in `Root` for theming.

---

## Registering a window for dynamic dispatch

Zero-param windows can be registered in the `WindowRegistry` so they can be opened by name — no type knowledge required at the call site. Put the registration in your crate's `init(cx)`:

```rust
// In my_crate/src/lib.rs
pub fn init(cx: &mut gpui::App) {
    use ui_common::PulsarWindowExt as _;
    MyWindow::register(cx);
}
```

Call `my_crate::init(cx)` from `main.rs` after the globals are installed (see **Startup order** below).

Opening by name from anywhere:

```rust
use gpui::UpdateGlobal as _;
use window_manager::WindowRegistry;

WindowRegistry::update_global(cx, |reg, cx| reg.open("MyWindow", cx));
```

### Mapping GPUI menu actions to the registry

`ui_core::init(cx)` maps existing GPUI menu actions (Settings, AboutApp, etc.) to registry lookups. It imports no window crates — only the action types:

```rust
cx.on_action(|_: &Settings, cx| {
    WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
});
```

To add a new menu-triggered window: add `MyWindow::register(cx)` to your crate's `init()`, add one `cx.on_action` line in `ui_core::init()`, and you're done.

---

## Startup order (main.rs)

```rust
gpui_app.run(|cx| {
    // 1. Install globals first
    cx.set_global(WindowManager::new());
    cx.set_global(WindowRegistry::new());

    // 2. Each crate self-registers its windows
    ui_settings::init(cx);
    ui_about::init(cx);
    ui_documentation::init(cx);
    ui_plugin_manager::init(cx);
    ui_log_viewer::init(cx);
    ui_fab_search::init(cx);
    // add more here when new zero-param windows are created

    // 3. Wire GPUI actions → registry (no window-type imports needed)
    ui_core::init(cx);

    // 4. Open the first window
    open_via_loading_screen(path, cx);  // or entry window
});
```

---

## WindowManager hooks

The `WindowManager` runs lifecycle hooks around every `create_window` call. Register custom hooks at startup:

```rust
use window_manager::{HookType, WindowManager};
use gpui::UpdateGlobal as _;

WindowManager::update_global(cx, |wm, _| {
    wm.register_hook(HookType::AfterCreate, Box::new(MyAnalyticsHook));
    wm.register_hook(HookType::BeforeClose, Box::new(MyCleanupHook));
});
```

Built-in hooks: `LoggingHook` (AfterCreate, AfterClose) and `TelemetryHook` (AfterCreate, AfterClose) are installed automatically.

---

## WindowRegistry API

```rust
// Register a custom opener (for parameterised windows or complex setup)
WindowRegistry::update_global(cx, |reg, _| {
    reg.register("MyWindow", |cx| {
        let params = compute_params();
        MyWindow::open(params, cx);
    });
});

// Open by name
WindowRegistry::update_global(cx, |reg, cx| reg.open("MyWindow", cx));

// Query
WindowRegistry::read_global(cx).is_registered("MyWindow");
WindowRegistry::read_global(cx).registered_names(); // Vec<&'static str>
```

---

## Checklist for adding a new window

1. Implement `PulsarWindow` for your view type in your crate.
2. If zero-param: add `pub fn init(cx: &mut gpui::App)` that calls `MyWindow::register(cx)`.
3. Call `my_crate::init(cx)` from `main.rs`.
4. If triggered by a menu action: add one `cx.on_action` line in `ui_core::init()`.
5. Call `MyWindow::open(params, cx)` anywhere via `use ui_common::PulsarWindowExt as _`.
