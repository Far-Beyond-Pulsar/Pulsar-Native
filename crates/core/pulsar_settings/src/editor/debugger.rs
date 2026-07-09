use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "debugger";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Debugger", "Integrated debugger and profiler settings")
        // ---- Breakpoints ----
        .setting(
            "break_on_exception",
            SchemaEntry::new(
                "Pause execution when any scripting exception is thrown",
                true,
            )
            .label("Break on Exception")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "break_on_uncaught_exception",
            SchemaEntry::new(
                "Only break on exceptions that are not caught by a try/catch block",
                true,
            )
            .label("Break on Uncaught Only")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "break_on_lua_error",
            SchemaEntry::new("Break into the debugger on any Lua runtime error", true)
                .label("Break on Lua Error")
                .page("Debugger")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "break_on_assert",
            SchemaEntry::new(
                "Break when an assertion fails (assert() returns false)",
                true,
            )
            .label("Break on Assert")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "conditional_breakpoints",
            SchemaEntry::new(
                "Enable conditional breakpoints (break only when an expression is true)",
                true,
            )
            .label("Conditional Breakpoints")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "log_points",
            SchemaEntry::new(
                "Enable log-points that print a message without stopping execution",
                true,
            )
            .label("Log Points")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "data_breakpoints",
            SchemaEntry::new(
                "Enable data watchpoints that break when a memory address is written",
                false,
            )
            .label("Data Breakpoints")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        // ---- Variables & Inspection ----
        .setting(
            "show_locals",
            SchemaEntry::new(
                "Show the local variables panel when the debugger pauses",
                true,
            )
            .label("Show Locals")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "inline_values",
            SchemaEntry::new(
                "Display variable values inline in the source editor while paused",
                true,
            )
            .label("Inline Values")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_hex_values",
            SchemaEntry::new(
                "Display integer variable values in hexadecimal in the watch panel",
                false,
            )
            .label("Hex Values")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "max_string_preview_length",
            SchemaEntry::new(
                "Maximum characters to show in a string variable preview before truncating",
                200_i64,
            )
            .label("Max String Preview Length")
            .page("Debugger")
            .field_type(FieldType::NumberInput {
                min: Some(20.0),
                max: Some(2000.0),
                step: Some(20.0),
            })
            .validator(Validator::int_range(20, 2000)),
        )
        .setting(
            "max_array_preview_elements",
            SchemaEntry::new(
                "Maximum array/table elements to show before collapsing",
                64_i64,
            )
            .label("Max Array Preview Elements")
            .page("Debugger")
            .field_type(FieldType::NumberInput {
                min: Some(8.0),
                max: Some(512.0),
                step: Some(8.0),
            })
            .validator(Validator::int_range(8, 512)),
        )
        .setting(
            "watch_auto_refresh",
            SchemaEntry::new(
                "Automatically refresh watch expressions after each step",
                true,
            )
            .label("Watch Auto-Refresh")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        // ---- Call Stack ----
        .setting(
            "max_call_stack_depth",
            SchemaEntry::new("Maximum frames shown in the call stack panel", 64_i64)
                .label("Max Call Stack Depth")
                .page("Debugger")
                .field_type(FieldType::NumberInput {
                    min: Some(8.0),
                    max: Some(512.0),
                    step: Some(8.0),
                })
                .validator(Validator::int_range(8, 512)),
        )
        .setting(
            "just_my_code",
            SchemaEntry::new(
                "Skip stepping into engine internals and external library code",
                true,
            )
            .label("Just My Code")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "step_over_external",
            SchemaEntry::new(
                "Step over calls into code outside the current project directory",
                true,
            )
            .label("Step Over External Code")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        // ---- DAP Server ----
        .setting(
            "debugger_port",
            SchemaEntry::new("Port for the DAP (Debug Adapter Protocol) server", 7878_i64)
                .label("Debugger Port")
                .page("Debugger")
                .field_type(FieldType::NumberInput {
                    min: Some(1024.0),
                    max: Some(65535.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1024, 65535)),
        )
        .setting(
            "auto_attach_to_server",
            SchemaEntry::new(
                "Automatically attach the debugger when launching a PIE dedicated server",
                false,
            )
            .label("Auto Attach to Server")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "hot_patch_on_save",
            SchemaEntry::new(
                "Apply script changes live while paused without restarting the session",
                true,
            )
            .label("Hot-Patch on Save")
            .page("Debugger")
            .field_type(FieldType::Checkbox),
        )
        // ---- Profiler ----
        .setting(
            "profiler_type",
            SchemaEntry::new("Profiler sampling mode", "statistical")
                .label("Profiler Type")
                .page("Profiler")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Statistical (low overhead)", "statistical"),
                        DropdownOption::new("Instrumented (accurate)", "instrumented"),
                        DropdownOption::new("Tracy", "tracy"),
                    ],
                })
                .validator(Validator::string_one_of([
                    "statistical",
                    "instrumented",
                    "tracy",
                ])),
        )
        .setting(
            "profiler_sample_rate_hz",
            SchemaEntry::new(
                "CPU profiler sampling frequency in Hz (statistical mode)",
                1000_i64,
            )
            .label("Profiler Sample Rate (Hz)")
            .page("Profiler")
            .field_type(FieldType::NumberInput {
                min: Some(100.0),
                max: Some(10000.0),
                step: Some(100.0),
            })
            .validator(Validator::int_range(100, 10_000)),
        )
        .setting(
            "memory_profiling",
            SchemaEntry::new(
                "Track allocation/deallocation events in the profiler",
                false,
            )
            .label("Memory Profiling")
            .page("Profiler")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "gpu_profiling",
            SchemaEntry::new(
                "Collect GPU timestamp queries for render pass timing",
                false,
            )
            .label("GPU Profiling")
            .page("Profiler")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "profiler_frame_history",
            SchemaEntry::new(
                "Number of frames of profiler data to retain in memory",
                300_i64,
            )
            .label("Profiler Frame History")
            .page("Profiler")
            .field_type(FieldType::NumberInput {
                min: Some(60.0),
                max: Some(3000.0),
                step: Some(60.0),
            })
            .validator(Validator::int_range(60, 3000)),
        )
        .setting(
            "profiler_output_dir",
            SchemaEntry::new(
                "Directory where captured profiler traces are saved",
                "profiler_traces/",
            )
            .label("Profiler Output Directory")
            .page("Profiler")
            .field_type(FieldType::TextInput {
                placeholder: Some("profiler_traces/".into()),
                multiline: false,
            }),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
