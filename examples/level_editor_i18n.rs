//! Example: Using the Level Editor Translation System
//!
//! This example demonstrates how to use and switch between languages
//! in the Level Editor UI.

use ui_level_editor::{locale, set_locale};

fn main() {
    // Initialize the application (simplified example)
    println!("Level Editor Translation System Demo\n");

    // Get current locale
    println!("Current locale: {}", locale());

    // Display some translations in English (default)
    demo_translations();

    // Switch to Simplified Chinese
    println!("\n--- Switching to Simplified Chinese (zh-CN) ---");
    set_locale("zh-CN");
    demo_translations();

    // Switch to Traditional Chinese
    println!("\n--- Switching to Traditional Chinese (zh-HK) ---");
    set_locale("zh-HK");
    demo_translations();

    // Switch to Italian
    println!("\n--- Switching to Italian (it) ---");
    set_locale("it");
    demo_translations();

    // Back to English
    println!("\n--- Back to English (en) ---");
    set_locale("en");
    demo_translations();
}

fn demo_translations() {
    use rust_i18n::t;

    println!("Toolbar:");
    println!("  - Play: {}", t!("LevelEditor.Toolbar.StartSimulation"));
    println!("  - Stop: {}", t!("LevelEditor.Toolbar.StopSimulation"));
    println!("  - Time: {}", t!("LevelEditor.Toolbar.TimeScale"));

    println!("\nPanels:");
    println!("  - {}", t!("LevelEditor.Hierarchy.Title"));
    println!("  - {}", t!("LevelEditor.Properties.Title"));
    println!("  - {}", t!("LevelEditor.Performance.Title"));

    println!("\nActions:");
    println!("  - {}", t!("LevelEditor.Hierarchy.AddObject"));
    println!("  - {}", t!("LevelEditor.Properties.AddComponent"));
    println!("  - {}", t!("LevelEditor.Assets.Refresh"));
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
