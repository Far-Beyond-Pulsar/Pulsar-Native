use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "debugger";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Debugger", "Integrated debugger and profiler settings")
        .setting("break_on_exception",
            SchemaEntry::new("Pause execution when a scripting exception is thrown", true)
                .label("Break on Exception").page("Debugger")
                .field_type(FieldType::Checkbox))
        .setting("break_on_lua_error",
            SchemaEntry::new("Break into the debugger on any Lua error", true)
                .label("Break on Lua Error").page("Debugger")
                .field_type(FieldType::Checkbox))
        .setting("show_locals",
            SchemaEntry::new("Show local variable panel when the debugger pauses", true)
                .label("Show Locals").page("Debugger")
                .field_type(FieldType::Checkbox))
        .setting("max_call_stack_depth",
            SchemaEntry::new("Maximum frames shown in the call stack panel", 64_i64)
                .label("Max Call Stack Depth").page("Debugger")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(512.0), step: Some(8.0) })
                .validator(Validator::int_range(8, 512)))
        .setting("auto_attach_to_server",
            SchemaEntry::new("Automatically attach the debugger when launching a dedicated server", false)
                .label("Auto Attach to Server").page("Debugger")
                .field_type(FieldType::Checkbox))
        .setting("debugger_port",
            SchemaEntry::new("Port for the DAP (Debug Adapter Protocol) server", 7878_i64)
                .label("Debugger Port").page("Debugger")
                .field_type(FieldType::NumberInput { min: Some(1024.0), max: Some(65535.0), step: Some(1.0) })
                .validator(Validator::int_range(1024, 65535)))
        .setting("profiler_sample_rate_hz",
            SchemaEntry::new("CPU profiler sampling frequency in Hz", 1000_i64)
                .label("Profiler Sample Rate (Hz)").page("Debugger")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(10000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 10_000)))
        .setting("memory_profiling",
            SchemaEntry::new("Enable memory allocation tracking in the profiler", false)
                .label("Memory Profiling").page("Debugger")
                .field_type(FieldType::Checkbox))
        .setting("inline_values",
            SchemaEntry::new("Show variable values inline in the editor while debugging", true)
                .label("Inline Values").page("Debugger")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
