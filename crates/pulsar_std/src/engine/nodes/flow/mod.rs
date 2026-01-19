//! # Flow Control Module
//!
//! Flow control and execution routing nodes for the Pulsar visual programming system.
//!
//! This module provides control flow nodes including:
//! - **Branching**: Conditional execution paths (branch, multi_branch)
//! - **Switching**: Value-based routing (switch_on_int, switch_on_bool, switch_on_string, range_switch, string_contains_switch)
//! - **Loops**: Iteration constructs (for_loop, while_loop)
//! - **Sequencing**: Ordered execution (sequence)
//! - **Gating**: Conditional blocking (gate, multi_gate, flip_flop)
//! - **Limiting**: Execution constraints (do_once, do_n)
//! - **Timing**: Delays and timing control (delay, retriggerable_delay)
//!
//! All flow control nodes use `NodeTypes::control_flow` and manage execution flow through
//! execution pins created with the `exec_output!()` macro.

use crate::{blueprint, NodeTypes, exec_output};

// =============================================================================
// Branching Operations
// =============================================================================

/// A node that generates a branch based on a boolean condition.
///
/// This node evaluates the input condition and executes one of two branches
/// depending on whether the condition is true or false.
///
/// # Inputs
/// - `condition`: The boolean condition to evaluate
///
/// # Execution Outputs
/// - `True`: Executes if the condition is true
/// - `False`: Executes if the condition is false
///
/// # Example
/// If `condition` is true, the code connected to `True` will run.
/// If false, the code connected to `False` will run.
///
/// # Notes
/// Use this node for conditional logic and flow control in your graph.
/// # Branch
/// Routes execution based on a boolean condition.
/// If the condition is true, the True pin executes. Otherwise, the False pin executes.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn branch(condition: bool) {
    if condition {
        exec_output!("True");
    } else {
        exec_output!("False");
    }
}

/// A node that provides multiple conditional branches (if/else if/else chain).
///
/// This node evaluates multiple boolean conditions in sequence and executes the first branch whose condition is true.
/// If none of the conditions are true, the else branch is executed. Useful for complex decision trees, multi-way branching, and control flow.
///
/// # Inputs
/// - `condition1`: The first condition to check
/// - `condition2`: The second condition to check
/// - `condition3`: The third condition to check
///
/// # Execution Outputs
/// - `Branch1`: Executes if the first condition is true
/// - `Branch2`: Executes if the second condition is true
/// - `Branch3`: Executes if the third condition is true
/// - `Else`: Executes if none of the conditions are true
///
/// # Example
/// If `condition1` is false, `condition2` is true, and `condition3` is false, the node will execute `Branch2`.
///
/// # Notes
/// Conditions are checked in order. Only the first true branch is executed. Use this node for multi-way branching logic.
/// # Multi Branch
/// Evaluates multiple conditions and executes the first matching branch.
/// Conditions are checked in order. Only the first true branch executes.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn multi_branch(condition1: bool, condition2: bool, condition3: bool) {
    if condition1 {
        exec_output!("Branch1");
    } else if condition2 {
        exec_output!("Branch2");
    } else if condition3 {
        exec_output!("Branch3");
    } else {
        exec_output!("Else");
    }
}

// =============================================================================
// Switch Operations
// =============================================================================

/// Execute different branches based on integer value.
///
/// This node matches the input integer value and executes a corresponding branch for each case.
/// If the value does not match any explicit case, the default branch is executed.
///
/// # Inputs
/// - `value`: The integer value to match against (i32)
///
/// # Execution Outputs
/// - `Case0`: Executes if value is 0
/// - `Case1`: Executes if value is 1
/// - `Case2`: Executes if value is 2
/// - `Case3`: Executes if value is 3
/// - `Default`: Executes for any other value
///
/// # Example
/// If `value` is 2, the node will execute `Case2`.
/// If `value` is 5, the node will execute `Default`.
///
/// # Notes
/// You can customize the number of explicit cases by editing the match arms.
/// # Switch on Int
/// Routes execution based on an integer value.
/// Matches the value against predefined cases or executes the default branch.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn switch_on_int(value: i64) {
    match value {
        0 => exec_output!("Case0"),
        1 => exec_output!("Case1"),
        2 => exec_output!("Case2"),
        3 => exec_output!("Case3"),
        _ => exec_output!("Default"),
    }
}

/// Execute different branches based on a boolean value.
///
/// This node evaluates the input boolean value and executes one of two branches:
/// - If the value is `true`, the `True` branch is executed.
/// - If the value is `false`, the `False` branch is executed.
///
/// # Inputs
/// - `value`: The boolean value to test
///
/// # Execution Outputs
/// - `True`: Executes if value is true
/// - `False`: Executes if value is false
///
/// # Example
/// If `value` is `true`, the code connected to the `True` branch will run.
/// If `value` is `false`, the code connected to the `False` branch will run.
///
/// # Notes
/// This node is useful for conditional execution based on a boolean result.
/// # Switch on Bool
/// Routes execution based on a boolean value.
/// Executes the True or False branch depending on the input.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn switch_on_bool(value: bool) {
    match value {
        true => exec_output!("True"),
        false => exec_output!("False"),
    }
}

/// Execute different branches based on string value.
///
/// This node matches the input string against several predefined options and executes
/// the corresponding branch for the first match. If the input does not match any option,
/// the default branch is executed.
///
/// # Inputs
/// - `value`: The string value to match against the options
///
/// # Execution Outputs
/// - `Option1`: Executes if value matches "option1"
/// - `Option2`: Executes if value matches "option2"
/// - `Option3`: Executes if value matches "option3"
/// - `Default`: Executes if no match is found
///
/// # Example
/// If `value` is "option2", the node will execute the code connected to `Option2`.
/// If `value` is "unknown", the node will execute the code connected to `Default`.
///
/// # Notes
/// You can customize the option strings and add more branches as needed.
/// # Switch on String
/// Routes execution based on a string value.
/// Matches the string against predefined options or executes the default branch.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn switch_on_string(value: String) {
    match value.as_str() {
        "option1" => exec_output!("Option1"),
        "option2" => exec_output!("Option2"),
        "option3" => exec_output!("Option3"),
        _ => exec_output!("Default"),
    }
}

/// Execute branches based on numeric ranges.
///
/// This node checks which range the input value falls into and executes the corresponding branch.
/// Useful for categorizing numeric values into ranges.
///
/// # Inputs
/// - `value`: The numeric value to check
///
/// # Execution Outputs
/// - `Negative`: Executes if value < 0.0
/// - `Low`: Executes if 0.0 <= value < 10.0
/// - `Medium`: Executes if 10.0 <= value < 50.0
/// - `High`: Executes if 50.0 <= value < 100.0
/// - `Extreme`: Executes if value >= 100.0
///
/// # Example
/// If `value` is 25.0, the `Medium` branch will execute.
/// If `value` is -5.0, the `Negative` branch will execute.
///
/// # Notes
/// Range boundaries can be customized by modifying the threshold values.
/// # Range Switch
/// Routes execution based on which numeric range the value falls into.
/// Useful for categorizing values into predefined ranges.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn range_switch(value: f64) {
    if value < 0.0 {
        exec_output!("Negative");
    } else if value < 10.0 {
        exec_output!("Low");
    } else if value < 50.0 {
        exec_output!("Medium");
    } else if value < 100.0 {
        exec_output!("High");
    } else {
        exec_output!("Extreme");
    }
}

/// A node that executes different branches based on whether a string contains specific patterns.
///
/// This node checks the input string for the presence of up to three patterns, executing the corresponding branch for the first pattern found.
/// If none of the patterns are found, the "none" branch is executed.
/// Useful for conditional logic, routing, or handling different cases based on string content.
///
/// # Inputs
/// - `text`: The input string to check
/// - `pattern1`: The first pattern to search for
/// - `pattern2`: The second pattern to search for
/// - `pattern3`: The third pattern to search for
///
/// # Execution Outputs
/// - `Contains1`: Executes if the first pattern is found
/// - `Contains2`: Executes if the second pattern is found
/// - `Contains3`: Executes if the third pattern is found
/// - `None`: Executes if none of the patterns are found
///
/// # Example
/// If `text` is "hello world", `pattern1` is "hello", and `pattern2` is "foo", the node will execute `Contains1`.
/// If none of the patterns are present, the node will execute `None`.
///
/// # Notes
/// Patterns are checked in order. Only the first matching branch is executed. Use this node for simple string-based routing or filtering.
/// # String Contains Switch
/// Routes execution based on which pattern the string contains.
/// Checks patterns in order and executes the first matching branch.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn string_contains_switch(text: String, pattern1: String, pattern2: String, pattern3: String) {
    if text.contains(&pattern1) {
        exec_output!("Contains1");
    } else if text.contains(&pattern2) {
        exec_output!("Contains2");
    } else if text.contains(&pattern3) {
        exec_output!("Contains3");
    } else {
        exec_output!("None");
    }
}

// =============================================================================
// Loop Operations
// =============================================================================

/// A node that executes a loop for a specified number of iterations.
///
/// This node runs the connected code block a specified number of times, using a simple for-loop.
/// Useful for repeating actions, batch processing, or iterating over a fixed range.
///
/// # Inputs
/// - `count`: The number of iterations to execute (integer)
///
/// # Execution Outputs
/// - `Body`: Executes for each iteration
///
/// # Behavior
/// The loop variable `i` runs from 0 up to (but not including) `count`. The connected code block is executed once per iteration.
///
/// # Example
/// If `count` is 3, the code block will execute three times.
///
/// # Notes
/// Use this node for simple iteration. For more complex iteration (e.g., over arrays), use dedicated array or collection nodes.
/// # For Loop
/// Executes a loop body a specified number of times.
/// Iterates from 0 to count-1, executing the body each time.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn for_loop(count: i64) {
    for _i in 0..count {
        exec_output!("Body");
    }
}

/// A node that executes a while loop based on a condition.
///
/// This node repeatedly executes the connected body as long as the input condition is `true`.
/// The condition is evaluated before each iteration, and the loop terminates when the condition becomes `false`.
///
/// # Inputs
/// - `condition`: The boolean condition to test before each iteration
///
/// # Execution Outputs
/// - `Body`: Executes repeatedly while the condition is `true`
///
/// # Example
/// If `condition` is initially `true` and becomes `false` after 5 iterations, the body will execute 5 times.
///
/// # Notes
/// Use caution to avoid infinite loops. The condition should eventually become `false` to terminate the loop.
/// # While Loop
/// Executes a loop body repeatedly while a condition is true.
/// WARNING: Ensure the condition eventually becomes false to avoid infinite loops.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn while_loop(condition: bool) {
    while condition {
        exec_output!("Body");
    }
}

// =============================================================================
// Sequencing Operations
// =============================================================================

/// A node that executes multiple outputs in sequence (like Unreal's Sequence).
///
/// This node triggers a series of execution outputs in order, one after another, each time it is called.
/// Useful for chaining actions, triggering multiple effects, or ensuring a specific order of operations in your graph.
///
/// # Execution Outputs
/// - `Then0`: Executes first
/// - `Then1`: Executes second
/// - `Then2`: Executes third
/// - `Then3`: Executes fourth
///
/// # Behavior
/// Each connected output is executed in sequence.
/// All outputs are triggered every time the node is called.
///
/// # Example
/// If connected to four print nodes, the output will print messages 0, 1, 2, 3 in order every time this node is triggered.
///
/// # Notes
/// The number of outputs is fixed to four for simplicity. Extend the implementation for more outputs as needed.
/// Use this node to enforce execution order or to trigger multiple actions from a single event.
/// # Sequence
/// Executes multiple execution pins in sequential order.
/// All outputs fire in order each time the node is triggered.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn sequence() {
    exec_output!("Then0");
    exec_output!("Then1");
    exec_output!("Then2");
    exec_output!("Then3");
}

// =============================================================================
// Gating Operations
// =============================================================================

/// A node that controls execution flow using an open/close gate (like Unreal's Gate).
///
/// This node acts as a gate that can be opened or closed to allow or block execution of connected code.
/// The gate maintains its state internally and responds to open/close signals.
///
/// # Inputs
/// - `open`: If true, opens the gate (allows execution)
/// - `close`: If true, closes the gate (blocks execution)
///
/// # Execution Outputs
/// - `Then`: Executes only if the gate is open
///
/// # Behavior
/// The gate is controlled by static state. When opened, execution passes through; when closed, execution is blocked.
/// Multiple open/close signals can be sent; the last signal determines the state.
///
/// # Example
/// If `open` is true, the gate opens and allows execution. If `close` is true, the gate closes and blocks execution.
///
/// # Notes
/// Useful for controlling flow in event graphs, gating triggers, or synchronizing logic.
/// # Gate
/// Controls execution flow with an open/close gate.
/// Execution only passes through when the gate is open.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn gate(open: bool, close: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static GATE_OPEN: AtomicBool = AtomicBool::new(false);

    if open {
        GATE_OPEN.store(true, Ordering::Relaxed);
    }

    if close {
        GATE_OPEN.store(false, Ordering::Relaxed);
    }

    if GATE_OPEN.load(Ordering::Relaxed) {
        exec_output!("Then");
    }
}

/// A node that cycles through multiple outputs in sequence (like Unreal's MultiGate).
///
/// This node maintains an internal index and cycles through multiple execution outputs each time it is triggered.
/// Useful for round-robin logic, distributing work, or sequencing actions across multiple branches.
///
/// # Inputs
/// - `reset`: If true, resets the internal index to zero
///
/// # Execution Outputs
/// - `Output0`: First output in the cycle
/// - `Output1`: Second output in the cycle
/// - `Output2`: Third output in the cycle
/// - `Output3`: Fourth output in the cycle
///
/// # Behavior
/// On each trigger, the node increments its internal index and executes the corresponding output branch.
/// When the index exceeds the number of outputs, it wraps around to zero.
/// If `reset` is true, the index is reset to zero and no output is executed.
///
/// # Example
/// If connected to four print nodes, the output will cycle through printing "0", "1", "2", "3" on consecutive triggers, then repeat.
///
/// # Notes
/// Uses a static atomic integer for thread-safe state tracking. The number of outputs is fixed to 4 for simplicity.
/// Extend the implementation for more outputs as needed.
/// # Multi Gate
/// Cycles through multiple outputs in sequence.
/// Each trigger advances to the next output, wrapping around after the last.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn multi_gate(reset: bool) {
    use std::sync::atomic::{AtomicI32, Ordering};
    static CURRENT_INDEX: AtomicI32 = AtomicI32::new(0);

    if reset {
        CURRENT_INDEX.store(0, Ordering::Relaxed);
        return;
    }

    let index = CURRENT_INDEX.fetch_add(1, Ordering::Relaxed);
    let num_outputs = 4; // Fixed to 4 outputs for simplicity
    let current = index % num_outputs;

    match current {
        0 => exec_output!("Output0"),
        1 => exec_output!("Output1"),
        2 => exec_output!("Output2"),
        3 => exec_output!("Output3"),
        _ => {}
    }
}

/// A node that alternates between two outputs each time it is triggered (like Unreal's FlipFlop).
///
/// This node maintains an internal state and switches between two execution branches (`A` and `B`) on each call.
/// Useful for toggling behavior, alternating actions, or implementing simple state machines.
///
/// # Execution Outputs
/// - `A`: Executes on the first call, then every other call
/// - `B`: Executes on the second call, then every other call
///
/// # Behavior
/// On the first call, executes branch A.
/// On the next call, executes branch B.
/// Alternates between A and B on subsequent calls.
///
/// # Example
/// If connected to two print nodes, the output will alternate between printing "A" and "B" each time this node is triggered.
///
/// # Notes
/// Uses a static atomic boolean for thread-safe state tracking. The state persists for the lifetime of the process.
/// # Flip Flop
/// Alternates between two outputs each time it's triggered.
/// Useful for toggling behavior or alternating actions.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn flip_flop() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FLIP_STATE: AtomicBool = AtomicBool::new(false);

    let current_state = FLIP_STATE.load(Ordering::Relaxed);
    FLIP_STATE.store(!current_state, Ordering::Relaxed);

    if current_state {
        exec_output!("A");
    } else {
        exec_output!("B");
    }
}

// =============================================================================
// Execution Limiting Operations
// =============================================================================

/// A node that executes only once until reset (like Unreal's DoOnce).
///
/// This node ensures that the connected code executes only a single time until it is reset.
/// After the first execution, further triggers are ignored until the reset input is activated.
///
/// # Inputs
/// - `reset`: If true, resets the node so it can execute again
///
/// # Execution Outputs
/// - `Then`: Executes only the first time, or after a reset
///
/// # Behavior
/// Uses a static atomic flag to track execution state. When reset, the flag is cleared and the node can execute again.
///
/// # Example
/// If triggered repeatedly, the output will only execute once until reset.
///
/// # Notes
/// Useful for initialization, one-time events, or gating logic that should not repeat until explicitly reset.
/// # Do Once
/// Executes only once until reset.
/// Useful for one-time initialization or events that should not repeat.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn do_once(reset: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static EXECUTED: AtomicBool = AtomicBool::new(false);

    if reset {
        EXECUTED.store(false, Ordering::Relaxed);
        return;
    }

    if !EXECUTED.load(Ordering::Relaxed) {
        EXECUTED.store(true, Ordering::Relaxed);
        exec_output!("Then");
    }
}

/// A node that executes a connected branch N times, then stops until reset.
///
/// This node tracks the number of executions and only allows the connected branch to run up to N times.
/// After N executions, further triggers are ignored until the node is reset.
///
/// # Inputs
/// - `n`: The maximum number of times to execute (integer)
/// - `reset`: If true, resets the execution counter to zero
///
/// # Execution Outputs
/// - `Then`: Executes up to N times
///
/// # Behavior
/// Maintains an internal counter using a static atomic variable.
/// When triggered, increments the counter and executes the branch if the count is less than N.
/// If `reset` is true, resets the counter and does not execute the branch.
///
/// # Example
/// If `n` is 3, the branch will execute three times on consecutive triggers, then stop until reset.
///
/// # Notes
/// Useful for limiting the number of times an action can occur, such as for initialization, retries, or one-shot events.
/// # Do N
/// Executes N times then stops until reset.
/// Useful for limiting the number of times an action can occur.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn do_n(n: i64, reset: bool) {
    use std::sync::atomic::{AtomicI32, Ordering};
    static COUNTER: AtomicI32 = AtomicI32::new(0);

    let max_count = n as i32;

    if reset {
        COUNTER.store(0, Ordering::Relaxed);
        return;
    }

    let current = COUNTER.load(Ordering::Relaxed);
    if current < max_count {
        COUNTER.fetch_add(1, Ordering::Relaxed);
        exec_output!("Then");
    }
}

// =============================================================================
// Timing Operations
// =============================================================================

/// A node that introduces a delay/sleep for a specified duration.
///
/// This node pauses execution for the specified number of milliseconds. It is useful for timing control,
/// animations, throttling, or waiting for asynchronous events.
///
/// # Inputs
/// - `milliseconds`: The duration to sleep, in milliseconds (u64)
///
/// # Behavior
/// The node blocks the current thread for at least the specified duration. Actual sleep time may be longer
/// due to system scheduling.
///
/// # Example
/// If `milliseconds` is 1000, the node will sleep for approximately 1 second.
///
/// # Notes
/// Use with caution in performance-critical code, as sleeping blocks the thread and may affect responsiveness.
/// # Delay
/// Pauses execution for a specified duration in milliseconds.
/// WARNING: Blocks the current thread during the delay.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn delay(milliseconds: i64) {
    std::thread::sleep(std::time::Duration::from_millis(milliseconds as u64));
}

/// A node that provides a retriggerable delay (restartable timer).
///
/// This node introduces a delay before executing the connected branch, but if triggered again during the delay, the timer is reset.
/// Useful for debouncing, throttling, or ensuring an action only occurs after a period of inactivity.
///
/// # Inputs
/// - `delay_ms`: The delay duration in milliseconds (u64)
///
/// # Execution Outputs
/// - `Completed`: Executes after the delay, unless retriggered
///
/// # Behavior
/// When triggered, starts a timer for the specified delay.
/// If triggered again before the delay elapses, the timer is reset.
/// Only after the timer completes without retriggering does the node execute the connected branch.
///
/// # Example
/// If `delay_ms` is 1000, and the node is triggered every 500ms, the output will not fire until 1 second passes with no triggers.
///
/// # Notes
/// Useful for debouncing user input, delaying actions until a pause, or implementing restartable timers.
/// # Retriggerable Delay
/// Delay that resets if triggered again before completion.
/// Useful for debouncing or waiting for a period of inactivity.
#[blueprint(type: NodeTypes::control_flow, category: "Flow", color: "#BD10E0")]
pub fn retriggerable_delay(delay_ms: i64) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static DELAY_STATE: Mutex<Option<Instant>> = Mutex::new(None);

    let delay_duration = Duration::from_millis(delay_ms as u64);

    // Set/reset the delay start time
    {
        let mut state = DELAY_STATE.lock().unwrap();
        *state = Some(Instant::now());
    }

    // Sleep for the delay duration
    std::thread::sleep(delay_duration);

    // Check if we weren't retriggered during the delay
    {
        let state = DELAY_STATE.lock().unwrap();
        if let Some(start_time) = *state {
            if start_time.elapsed() >= delay_duration {
                exec_output!("Completed");
            }
        }
    }
}
