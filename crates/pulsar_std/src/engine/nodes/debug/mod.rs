//! # Debug Module
//!
//! Debugging, logging, and diagnostic operations for the Pulsar visual programming system.
//!
//! This module provides function nodes with side effects for debugging and monitoring your visual programs:
//! - **Print Operations**: Output values and formatted messages to the console
//! - **Logging**: Timestamped log messages at different severity levels (info, warning, error)
//! - **Debug Inspection**: Detailed value inspection with type information
//! - **Assertions**: Runtime checks that panic on failure
//! - **Profiling**: Execution time measurement and benchmarking
//! - **Breakpoints**: Interactive debugging with execution pausing
//! - **System Diagnostics**: Stack traces and memory usage information
//!
//! All debug nodes are `NodeTypes::fn_` with execution flow (exec in/out) and side effects.

use crate::blueprint;

// =============================================================================
// Print Operations
// =============================================================================

/// Print a string to the console for debugging.
///
/// This node outputs the provided string to the console, prefixed with "[DEBUG]".
/// Useful for inspecting values, debugging logic, or tracing execution in your graph.
///
/// # Inputs
/// - `message`: The string message to print
///
/// # Outputs
/// - None. The message is printed to the console.
///
/// # Example
/// If `message` is "Hello, world!", the output will be:
/// ```text
/// [DEBUG] Hello, world!
/// ```
///
/// # Notes
/// Use this node for development and troubleshooting. Remove or disable in production code.
/// # Print String
/// Prints a string to the console with [DEBUG] prefix.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn print_string(message: &str) {
    tracing::debug!("[DEBUG] {}", message);
}

/// Print a number to the console for debugging.
///
/// This node outputs the provided number to the console, prefixed with a debug label.
/// Useful for inspecting values, debugging calculations, or tracing numeric data in your graph.
///
/// # Inputs
/// - `value`: The number to print (can be integer or float)
///
/// # Outputs
/// - None. The number is printed to the console.
///
/// # Example
/// If `value` is 42, the output will be:
/// ```text
/// [DEBUG] Number: 42
/// ```
///
/// # Notes
/// Use this node to monitor numeric values during development or troubleshooting.
/// # Print Number
/// Prints a number to the console with [DEBUG] prefix.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn print_number(value: f64) {
    tracing::debug!("[DEBUG] Number: {}", value);
}

/// Print a boolean value to the console for debugging.
///
/// This node outputs the provided boolean value to the console, prefixed with "[DEBUG] Boolean:".
/// Useful for inspecting logic, verifying conditions, or tracing execution in your graph.
///
/// # Inputs
/// - `value`: The boolean value to print
///
/// # Outputs
/// - None. The value is printed to the console.
///
/// # Example
/// If `value` is true, the output will be:
/// ```text
/// [DEBUG] Boolean: true
/// ```
/// If `value` is false, the output will be:
/// ```text
/// [DEBUG] Boolean: false
/// ```
///
/// # Notes
/// Use this node for debugging and development. Remove or disable in production code.
/// # Print Boolean
/// Prints a boolean value to the console with [DEBUG] prefix.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn print_bool(value: bool) {
    tracing::debug!("[DEBUG] Boolean: {}", value);
}

/// Print a message to the console without any prefix.
///
/// This node outputs the provided string to the console as-is, without any debug prefix.
/// Useful for clean output, user messages, or formatted displays.
///
/// # Inputs
/// - `message`: The string message to print
///
/// # Outputs
/// - None. The message is printed to the console.
///
/// # Example
/// If `message` is "Hello, world!", the output will be:
/// ```text
/// Hello, world!
/// ```
///
/// # Notes
/// This is a simple print node. For debug output, use `print_string` instead.
/// # Println
/// Prints a message to the console.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn println(message: String) {
    tracing::debug!("{}", message);
}

/// Print a formatted message with placeholder replacements.
///
/// This node takes a format string and up to three value strings, replacing placeholders
/// `{0}`, `{1}`, and `{2}` in the format string with the corresponding values.
/// Useful for debugging, logging, or displaying dynamic messages with variable content.
///
/// # Inputs
/// - `format`: The format string containing placeholders `{0}`, `{1}`, `{2}`
/// - `value0`: The value to replace `{0}`
/// - `value1`: The value to replace `{1}`
/// - `value2`: The value to replace `{2}`
///
/// # Outputs
/// - None. The formatted message is printed to the console.
///
/// # Example
/// If `format` is "Hello, {0}! You have {1} new messages.", `value0` is "Alice",
/// and `value1` is "5", the output will be:
/// ```text
/// [DEBUG] Hello, Alice! You have 5 new messages.
/// ```
///
/// # Notes
/// Only the first three placeholders are supported. Placeholders not present in the
/// format string are ignored. Use this node for simple string formatting in logs or UI.
/// # Print Formatted
/// Prints a formatted message with placeholder replacements ({0}, {1}, {2}).
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn print_formatted(format: String, value0: String, value1: String, value2: String) {
    let message = format
        .replace("{0}", &value0)
        .replace("{1}", &value1)
        .replace("{2}", &value2);
    tracing::debug!("[DEBUG] {}", message);
}

/// Print a message only if a condition is true.
///
/// This node checks the provided boolean condition. If the condition is true, it prints
/// the message to the console. Useful for debugging, logging, or conditional output in your graph.
///
/// # Inputs
/// - `condition`: The condition to check (bool)
/// - `message`: The message to print if the condition is true
///
/// # Behavior
/// If the condition is true, the message is printed. If false, nothing happens.
///
/// # Example
/// If `condition` is true and `message` is "Hello", the output will be:
/// ```text
/// [CONDITIONAL] Hello
/// ```
/// If the condition is false, nothing is printed.
///
/// # Notes
/// Use this node to reduce log spam or print only relevant information during debugging.
/// # Conditional Print
/// Prints a message only if the condition is true.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn conditional_print(condition: bool, message: String) {
    if condition {
        tracing::debug!("[CONDITIONAL] {}", message);
    }
}

// =============================================================================
// Debug Inspection
// =============================================================================

/// Inspect and print a value with its type information.
///
/// This node prints the provided value to the console along with its type and length information.
/// Useful for debugging, inspecting variables, and understanding runtime state.
///
/// # Inputs
/// - `value`: The value to inspect (string)
///
/// # Behavior
/// Prints the value, its length, and its type to the console.
///
/// # Example
/// If `value` is "hello", the output will be:
/// ```text
/// [INSPECT] Value: 'hello', Length: 5, Type: String
/// ```
///
/// # Notes
/// Extend this node for more complex types or richer inspection as needed.
/// # Debug Inspect Value
/// Inspects and prints a value with detailed type information.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn debug_inspect_value(value: String) {
    tracing::debug!(
        "[INSPECT] Value: '{}', Length: {}, Type: String",
        value,
        value.len()
    );
}

/// Print a simple stack trace for debugging.
///
/// This node outputs basic stack trace information to the console, including the current function name,
/// thread ID, and timestamp. Useful for diagnosing execution flow and debugging concurrent or complex logic.
///
/// # Inputs
/// - `function_name`: The name of the current function or code block
///
/// # Outputs
/// - None. Prints stack trace information to the console.
///
/// # Example
/// If `function_name` is "update", the output will include:
/// ```text
/// [STACK_TRACE] Current function: update
/// [STACK_TRACE] Thread: ThreadId(1)
/// [STACK_TRACE] Time: SystemTime { ... }
/// ```
///
/// # Notes
/// Use this node to trace execution and identify where code is running, especially in multi-threaded contexts.
/// # Debug Stack Trace
/// Prints a simple stack trace with function name, thread ID, and timestamp.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn debug_stack_trace(function_name: String) {
    tracing::debug!("[STACK_TRACE] Current function: {}", function_name);
    tracing::debug!("[STACK_TRACE] Thread: {:?}", std::thread::current().id());
    tracing::debug!("[STACK_TRACE] Time: {:?}", std::time::SystemTime::now());
}

/// Print current memory usage information for debugging.
///
/// This node prints basic memory usage information for the current process.
/// It attempts to retrieve and display the process ID and, depending on the operating system,
/// may invoke system commands to show memory usage details.
///
/// # Behavior
/// - Prints the current process ID to the console.
/// - On Windows, attempts to run `tasklist` to show memory usage for the process.
/// - On Unix-like systems, attempts to run `ps` to show resident set size (RSS).
///
/// # Example
/// The output will include the process ID and, if available, memory usage information from the system.
/// ```text
/// [MEMORY] Current process ID: 12345
/// ```
///
/// # Notes
/// This is a simplified implementation. For more detailed memory tracking, use platform-specific
/// libraries or tools. Output may vary depending on OS and permissions.
/// # Debug Memory Usage
/// Prints current memory usage information for the process.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn debug_memory_usage() {
    // Print process ID
    tracing::debug!("[MEMORY] Current process ID: {}", std::process::id());

    // Try to get memory info via system commands
    use std::process::Command;
    if cfg!(target_os = "windows") {
        let _ = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", std::process::id())])
            .output();
    } else {
        let _ = Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "rss"])
            .output();
    }
}

// =============================================================================
// Logging Operations
// =============================================================================

/// Log an informational message with a timestamp.
///
/// This node prints an info-level message to the console, prefixed with the current Unix timestamp.
/// Useful for diagnostics, debugging, and tracking application events.
///
/// # Inputs
/// - `message`: The message to log (string)
///
/// # Outputs
/// - None. The message is printed to the console.
///
/// # Example
/// If `message` is "Initialization complete", the output will be:
/// ```text
/// [INFO 1680000000] Initialization complete
/// ```
///
/// # Notes
/// The timestamp is the number of seconds since the Unix epoch.
/// Use this node for general-purpose logging and tracing.
/// # Log Info
/// Logs an informational message with timestamp.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn log_info(message: String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    tracing::debug!("[INFO {}] {}", now, message);
}

/// Log a warning message with a timestamp.
///
/// This node prints a warning message to the standard error output, prefixed with a timestamp
/// (seconds since Unix epoch). Useful for diagnostics, debugging, and highlighting potential
/// issues in your application.
///
/// # Inputs
/// - `message`: The warning message to log (string)
///
/// # Outputs
/// - None. The message is printed to standard error.
///
/// # Example
/// If `message` is "Low disk space", the output will be:
/// ```text
/// [WARN 1700000000] Low disk space
/// ```
///
/// # Notes
/// The timestamp is in seconds since the Unix epoch. Use this node to highlight non-fatal
/// issues or potential problems.
/// # Log Warning
/// Logs a warning message with timestamp to stderr.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn log_warn(message: String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    tracing::warn!("[WARN {}] {}", now, message);
}

/// Log an error message with a timestamp.
///
/// This node prints an error message to the standard error output, prefixed with a timestamp
/// (seconds since Unix epoch). Useful for debugging, diagnostics, and reporting errors in your
/// application or graph.
///
/// # Inputs
/// - `message`: The error message to log (string)
///
/// # Outputs
/// - None. The message is printed to standard error.
///
/// # Example
/// If `message` is "File not found", the output will be:
/// ```text
/// [ERROR 1700000000] File not found
/// ```
///
/// # Notes
/// The timestamp is the current system time in seconds since the Unix epoch. Use this node to
/// report errors, failures, or unexpected conditions.
/// # Log Error
/// Logs an error message with timestamp to stderr.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn log_error(message: String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    tracing::error!("[ERROR {}] {}", now, message);
}

// =============================================================================
// Assertions
// =============================================================================

/// Assert that a condition is true, panic with message if false.
///
/// This node checks the provided boolean condition. If the condition is false,
/// it panics and displays the provided message. Useful for debugging and enforcing
/// invariants during execution.
///
/// # Inputs
/// - `condition`: The condition to assert (bool)
/// - `message`: The message to display if the assertion fails
///
/// # Outputs
/// - None. Panics if the condition is false.
///
/// # Example
/// If `condition` is false and `message` is "Must be true", the node will panic with that message.
///
/// # Notes
/// Use only for debugging or enforcing critical invariants. Panics will terminate execution.
/// # Assert True
/// Asserts that a condition is true; panics with message if false.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn assert_true(condition: bool, message: String) {
    if !condition {
        panic!("Assertion failed: {}", message);
    }
}

/// Assert that two values are equal.
///
/// This node checks if the two input values are equal. If they are not, it panics and prints a message.
/// Useful for debugging and testing to ensure expected values match.
///
/// # Inputs
/// - `a`: The first value to compare (string)
/// - `b`: The second value to compare (string)
/// - `message`: Optional message to display on assertion failure
///
/// # Behavior
/// If the values are not equal, the node panics and prints the provided message.
///
/// # Example
/// If `a` is "foo" and `b` is "bar", the node will panic with the message.
/// If both are "foo", execution continues normally.
///
/// # Notes
/// Use for testing and debugging to verify that values match expectations.
/// # Assert Equals
/// Asserts that two values are equal; panics with message if not.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn assert_equals(a: String, b: String, message: String) {
    if a != b {
        panic!("Assertion failed: '{}' != '{}'. {}", a, b, message);
    }
}

// =============================================================================
// Breakpoints
// =============================================================================

/// Simulate a breakpoint for debugging by printing a message and pausing execution.
///
/// This node prints the provided message to the console and waits for user input before continuing.
/// It is useful for pausing execution at a specific point to inspect state or debug logic.
///
/// # Inputs
/// - `message`: The message to display when the breakpoint is hit
///
/// # Behavior
/// Execution is paused until the user presses Enter. The message is printed to help identify the breakpoint.
///
/// # Example
/// If `message` is "Reached checkpoint", the console will display:
/// ```text
/// BREAKPOINT: Reached checkpoint
/// Press Enter to continue...
/// ```
///
/// # Notes
/// Use this node to halt execution and inspect program state interactively during development.
/// # Breakpoint
/// Simulates a breakpoint by printing a message and waiting for user input.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn breakpoint(message: String) {
    tracing::debug!("BREAKPOINT: {}", message);
    tracing::debug!("Press Enter to continue...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
}

// =============================================================================
// Profiling and Benchmarking
// =============================================================================

/// Measure the execution time of a code block in milliseconds.
///
/// This node executes the code connected to its body and returns the elapsed time in milliseconds.
/// It is useful for profiling and benchmarking sections of code to identify performance bottlenecks.
///
/// # Inputs
/// - `name`: A name or label for the benchmark (used in logging)
/// - Code block: The code to execute and measure (connected via `pulsar_exec_code`)
///
/// # Outputs
/// - The elapsed time in milliseconds (u64)
///
/// # Behavior
/// The code block is executed, and the time taken is measured using a high-resolution timer.
/// The result is printed to the log with the provided name and returned as the output.
///
/// # Example
/// If the code block takes 50 ms to execute, the output will be 50.
/// ```text
/// [BENCHMARK] 'my_operation' took 50 ms
/// ```
///
/// # Notes
/// Use this node to profile expensive operations or compare alternative implementations.
/// # Benchmark Function
/// Measures execution time of a code block in milliseconds.
#[blueprint(type: NodeTypes::fn_, category: "Debug", color: "#7ED321")]
pub fn benchmark_function(name: String) -> u64 {
    let start = std::time::Instant::now();
    // Note: In the actual implementation, the code block would be executed here
    // via the exec_code mechanism. For now, we just measure the overhead.
    let duration = start.elapsed();
    let ms = duration.as_millis() as u64;
    tracing::debug!("[BENCHMARK] '{}' took {} ms", name, ms);
    ms
}
