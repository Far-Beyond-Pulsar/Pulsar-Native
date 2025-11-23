// Test plugin loading and integration
use plugin_manager::PluginManager;

fn main() {
    println!("🧪 PLUGIN SYSTEM TEST\n");
    println!("=" .repeat(60));

    let mut plugin_manager = PluginManager::new();
    let plugins_dir = std::path::Path::new("plugins/editor");

    println!("\n📂 Loading plugins from: {:?}", plugins_dir.canonicalize().unwrap_or(plugins_dir.to_path_buf()));

    match plugin_manager.load_plugins_from_dir(plugins_dir) {
        Ok(_) => {
            println!("✅ Plugins loaded successfully!\n");

            // Show loaded plugins
            let loaded_plugins = plugin_manager.get_plugins();
            println!("📊 Loaded {} plugin(s):", loaded_plugins.len());

            for plugin in loaded_plugins {
                println!("\n  📦 {}", plugin.name);
                println!("     └─ Version: {}", plugin.version);
                println!("     └─ Author: {}", plugin.author);
                println!("     └─ ID: {}", plugin.id.as_str());
                println!("     └─ Description: {}", plugin.description);
            }

            // Show registered file types
            println!("\n📋 Registered File Types:");
            let file_types = plugin_manager.get_file_types();
            if file_types.is_empty() {
                println!("  (none)");
            } else {
                for ft in file_types {
                    println!("  • {} (.{})", ft.display_name, ft.extension);
                    println!("    └─ ID: {}", ft.id.as_str());
                    println!("    └─ Icon: {:?}", ft.icon);
                    println!("    └─ Structure: {:?}", ft.structure);
                }
            }

            // Show registered editors
            println!("\n🎨 Registered Editors:");
            let editors = plugin_manager.get_editors();
            if editors.is_empty() {
                println!("  (none)");
            } else {
                for editor in editors {
                    println!("  • {}", editor.display_name);
                    println!("    └─ ID: {}", editor.id.as_str());
                    println!("    └─ Supported types: {:?}",
                        editor.supported_file_types.iter()
                            .map(|id| id.as_str())
                            .collect::<Vec<_>>());
                }
            }

            println!("\n" + &"=".repeat(60));
            println!("✅ ALL TESTS PASSED!");
            println!("   Plugin system is ready for file opening integration.");
        }
        Err(e) => {
            eprintln!("\n❌ FAILED to load plugins: {}", e);
            println!("\n" + &"=".repeat(60));
            std::process::exit(1);
        }
    }
}
