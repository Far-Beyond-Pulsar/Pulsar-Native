//! Example: Using the Level Editor Translation System
//!
//! This example demonstrates how to use and switch between languages
//! in the Level Editor UI.

use ui_level_editor::{locale, set_locale};

fn main() {
    // Initialize the application (simplified example)
    tracing::trace!("Level Editor Translation System Demo\n");

    // Get current locale
    tracing::trace!("Current locale: {}", locale());

    // Display some translations in English (default)
    demo_translations();

    // Switch to Simplified Chinese
    tracing::trace!("\n--- Switching to Simplified Chinese (zh-CN) ---");
    set_locale("zh-CN");
    demo_translations();

    // Switch to Traditional Chinese
    tracing::trace!("\n--- Switching to Traditional Chinese (zh-HK) ---");
    set_locale("zh-HK");
    demo_translations();

    // Switch to Italian
    tracing::trace!("\n--- Switching to Italian (it) ---");
    set_locale("it");
    demo_translations();

    // Back to English
    tracing::trace!("\n--- Back to English (en) ---");
    set_locale("en");
    demo_translations();
}

fn demo_translations() {
    use rust_i18n::t;

    tracing::trace!("Toolbar:");
    tracing::trace!("  - Play: {}", t!("LevelEditor.Toolbar.StartSimulation"));
    tracing::trace!("  - Stop: {}", t!("LevelEditor.Toolbar.StopSimulation"));
    tracing::trace!("  - Time: {}", t!("LevelEditor.Toolbar.TimeScale"));

    tracing::trace!("\nPanels:");
    tracing::trace!("  - {}", t!("LevelEditor.Hierarchy.Title"));
    tracing::trace!("  - {}", t!("LevelEditor.Properties.Title"));
    tracing::trace!("  - {}", t!("LevelEditor.Performance.Title"));

    tracing::trace!("\nActions:");
    tracing::trace!("  - {}", t!("LevelEditor.Hierarchy.AddObject"));
    tracing::trace!("  - {}", t!("LevelEditor.Properties.AddComponent"));
    tracing::trace!("  - {}", t!("LevelEditor.Assets.Refresh"));
}

// Example output:
//
// Current locale: en
// Toolbar:
//   - Play: Start Simulation (F5)
//   - Stop: Stop Simulation (Shift+F5)
//   - Time: Time Scale
//
// Panels:
//   - Hierarchy
//   - Properties
//   - Performance
//
// Actions:
//   - Add Object
//   - Add Component
//   - Refresh
//
// --- Switching to Simplified Chinese (zh-CN) ---
// Toolbar:
//   - Play: 开始模拟 (F5)
//   - Stop: 停止模拟 (Shift+F5)
//   - Time: 时间缩放
//
// Panels:
//   - 层级
//   - 属性
//   - 性能
//
// Actions:
//   - 添加对象
//   - 添加组件
//   - 刷新
