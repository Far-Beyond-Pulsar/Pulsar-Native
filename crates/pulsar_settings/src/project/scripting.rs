use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "scripting";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Scripting", "Game scripting backend and runtime settings")
        .setting("scripting_backend",
            SchemaEntry::new("Primary scripting language / runtime", "lua")
                .label("Scripting Backend").page("Scripting")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Lua (LuaJIT)", "lua"),
                    DropdownOption::new("Lua 5.4", "lua54"),
                    DropdownOption::new("WebAssembly (WASM)", "wasm"),
                    DropdownOption::new("Rhai (Rust native)", "rhai"),
                    DropdownOption::new("None", "none"),
                ]}))
        .setting("script_timeout_ms",
            SchemaEntry::new("Maximum execution time for a single script call before abort (ms)", 5000_i64)
                .label("Script Timeout (ms)").page("Scripting")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(60000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 60_000)))
        .setting("memory_limit_mb",
            SchemaEntry::new("Maximum heap memory per script VM in megabytes", 256_i64)
                .label("Script Memory Limit (MB)").page("Scripting")
                .field_type(FieldType::NumberInput { min: Some(16.0), max: Some(4096.0), step: Some(16.0) })
                .validator(Validator::int_range(16, 4096)))
        .setting("debug_hooks",
            SchemaEntry::new("Enable debug hooks (line/call events) — disables JIT", false)
                .label("Debug Hooks").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("jit_enabled",
            SchemaEntry::new("Enable JIT compilation for Lua (LuaJIT only)", true)
                .label("JIT Compilation").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("sandbox_level",
            SchemaEntry::new("Security sandbox restrictions for game scripts", "standard")
                .label("Sandbox Level").page("Scripting")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None (full access)", "none"),
                    DropdownOption::new("Standard (restrict OS/IO)", "standard"),
                    DropdownOption::new("Strict (restrict networking too)", "strict"),
                ]})
                .validator(Validator::string_one_of(["none", "standard", "strict"])))
        .setting("auto_reload_scripts",
            SchemaEntry::new("Automatically reload changed script files without restarting", true)
                .label("Auto-Reload Scripts").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("error_handler",
            SchemaEntry::new("Global script error handler mode", "log")
                .label("Error Handler").page("Scripting")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Log (continue)", "log"),
                    DropdownOption::new("Log + Traceback", "traceback"),
                    DropdownOption::new("Break into debugger", "debug"),
                    DropdownOption::new("Abort game", "abort"),
                ]}));

    let _ = cfg.register(NS, OWNER, schema);
}
