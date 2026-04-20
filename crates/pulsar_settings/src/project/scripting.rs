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
                ]}))
        .setting("script_search_paths",
            SchemaEntry::new("Additional directories to search for script files (semicolon-separated)", "")
                .label("Script Search Paths").page("Scripting")
                .field_type(FieldType::TextInput { placeholder: Some("scripts/;plugins/my_mod/scripts/".into()), multiline: false }))
        .setting("preload_scripts",
            SchemaEntry::new("Comma-separated list of script files to load before any scene initializes", "")
                .label("Preload Scripts").page("Scripting")
                .field_type(FieldType::TextInput { placeholder: Some("scripts/preload.lua".into()), multiline: false }))
        .setting("expose_engine_api",
            SchemaEntry::new("Expose full engine API to scripts (disable to limit to a safe subset)", true)
                .label("Expose Engine API").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("expose_filesystem_api",
            SchemaEntry::new("Allow scripts to read/write files through the engine FS API", false)
                .label("Expose Filesystem API").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("expose_network_api",
            SchemaEntry::new("Allow scripts to make outbound network requests", false)
                .label("Expose Network API").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("expose_audio_api",
            SchemaEntry::new("Expose audio playback controls to scripts", true)
                .label("Expose Audio API").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("strict_type_checking",
            SchemaEntry::new("Enforce strict type annotations in supported scripting backends (Rhai)", false)
                .label("Strict Type Checking").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("co_routine_tick_hz",
            SchemaEntry::new("How many times per second yielded coroutines are resumed", 30_i64)
                .label("Coroutine Tick (Hz)").page("Scripting")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(120.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 120)))
        .setting("require_path",
            SchemaEntry::new("Lua package.path pattern for require() resolution (empty = default)", "")
                .label("Lua require Path").page("Scripting")
                .field_type(FieldType::TextInput { placeholder: Some("./scripts/?.lua;./?.lua".into()), multiline: false }))
        .setting("global_namespace",
            SchemaEntry::new("Name of the global table that engine APIs are mounted under in Lua", "Engine")
                .label("Global Namespace").page("Scripting")
                .field_type(FieldType::TextInput { placeholder: Some("Engine".into()), multiline: false }))
        .setting("script_profiling",
            SchemaEntry::new("Enable per-function profiling inside the scripting VM", false)
                .label("Script Profiling").page("Scripting")
                .field_type(FieldType::Checkbox))
        .setting("wasm_linear_memory_pages",
            SchemaEntry::new("Initial WebAssembly linear memory size in 64 KiB pages", 256_i64)
                .label("WASM Memory Pages").page("Scripting")
                .field_type(FieldType::NumberInput { min: Some(16.0), max: Some(65536.0), step: Some(16.0) })
                .validator(Validator::int_range(16, 65536)))
        .setting("wasm_max_memory_pages",
            SchemaEntry::new("Maximum WebAssembly linear memory size in 64 KiB pages", 16384_i64)
                .label("WASM Max Memory Pages").page("Scripting")
                .field_type(FieldType::NumberInput { min: Some(256.0), max: Some(65536.0), step: Some(256.0) })
                .validator(Validator::int_range(256, 65536)));

    let _ = cfg.register(NS, OWNER, schema);
}
