// Simple test to verify plugin loading works
use plugin_manager::PluginManager;

fn main() {
    println!("🧪 Testing plugin loading...\n");

    let mut plugin_manager = PluginManager::new();

    let plugins_dir = std::path::Path::new("plugins/editor");
    println!("📂 Loading plugins from: {:?}", plugins_dir);

    match plugin_manager.load_plugins_from_dir(plugins_dir) {
        Ok(_) => {
            println!("✅ Plugins loaded successfully!\n");

            let loaded_plugins = plugin_manager.get_plugins();
            println!("📊 Loaded {} plugin(s):", loaded_plugins.len());

            for plugin in loaded_plugins {
                println!("\n  📦 {}", plugin.name);
                println!("     Version: {}", plugin.version);
                println!("     Author: {}", plugin.author);
                println!("     Description: {}", plugin.description);
                println!("     ID: {}", plugin.id.as_str());
            }

            // Test file type registration
            println!("\n📋 Registered file types:");
            let file_types = plugin_manager.get_file_types();
            for ft in file_types {
                println!("  - {} (id: {})", ft.display_name, ft.id.as_str());
            }

            println!("\n✅ All tests passed!");
        }
        Err(e) => {
            eprintln!("❌ Failed to load plugins: {}", e);
            std::process::exit(1);
        }
    }
}
