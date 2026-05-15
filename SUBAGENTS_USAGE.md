# Subagents Support in Agent Chat UI

## Overview

The agent chat UI now supports displaying subagent invocations with nested execution steps. When an agent spawns a subagent to handle a task, the UI renders a collapsible card showing the subagent's progress and detailed steps.

## Type Definitions

### SubagentStepStatus

Tracks the execution state of a subagent or individual step:

```rust
pub enum SubagentStepStatus {
    Pending,   // Queued, waiting to execute
    Running,   // Currently executing
    Success,   // Completed successfully
    Error,     // Failed with an error
}
```

### SubagentStepDisplay

Represents a single step within a subagent's execution:

```rust
pub struct SubagentStepDisplay {
    pub id: String,                           // Unique identifier
    pub description: String,                  // Brief title (e.g., "Analyzing file structure")
    pub details: String,                      // Execution details/output
    pub status: SubagentStepStatus,          // Current status
    pub started_at_ms: u64,                  // Unix epoch milliseconds
    pub finished_at_ms: Option<u64>,         // Set when step completes
}
```

### DisplayItem::SubagentInvocation

A display item representing a complete subagent invocation:

```rust
DisplayItem::SubagentInvocation {
    name: String,                    // Subagent name (e.g., "Code Analyzer")
    task: String,                    // Task description
    steps: Vec<SubagentStepDisplay>, // Nested execution steps
    is_expanded: bool,              // UI state: whether card is expanded
    status: SubagentStepStatus,     // Overall invocation status
    started_at_ms: u64,            // When subagent was invoked
    finished_at_ms: Option<u64>,   // When subagent returned
}
```

## Usage Example

From the agent provider code, create a subagent invocation and add it to the display items:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

fn get_current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// Create steps as the subagent progresses
let mut steps = vec![];
let start_ms = get_current_ms();

// Step 1: Initial analysis
steps.push(SubagentStepDisplay {
    id: "step-1".to_string(),
    description: "Analyzing file structure".to_string(),
    details: "Found 42 TypeScript files in src/".to_string(),
    status: SubagentStepStatus::Success,
    started_at_ms: start_ms,
    finished_at_ms: Some(start_ms + 150),
});

// Step 2: Running task
let step2_start = start_ms + 150;
steps.push(SubagentStepDisplay {
    id: "step-2".to_string(),
    description: "Generating suggestions".to_string(),
    details: "Processing patterns...".to_string(),
    status: SubagentStepStatus::Running,
    started_at_ms: step2_start,
    finished_at_ms: None,
});

// Create the subagent invocation display item
let subagent_item = DisplayItem::SubagentInvocation {
    name: "Code Refactoring Agent".to_string(),
    task: "Identify and suggest refactoring opportunities in the codebase".to_string(),
    steps,
    is_expanded: false,  // Initially collapsed
    status: SubagentStepStatus::Running,
    started_at_ms: start_ms,
    finished_at_ms: None,
};

// Add to display items (typically after an assistant message)
panel.display_items.push(subagent_item);
```

## UI Behavior

### Visual Design

- **Icon**: Git Branch (represents spawning a new execution path)
- **Header**: Shows subagent name, task, elapsed time, and status icon
- **Expandable**: Click header to expand/collapse nested steps
- **Color-coded**: Status indicator uses semantic colors:
  - `danger` (red) for errors
  - `success` (green) for successful completion
  - `info` (blue) for running/pending
  - `muted_foreground` for pending states

### Step Display

When expanded, each step shows:
- Status icon (Circle, Loader, CircleCheck, or CircleX)
- Step description in bold
- Detailed output in monospace font (JetBrains Mono)

### Serialization

Subagent invocations are preserved in chat history files. On disk, they're always stored in collapsed state (`is_expanded: false`) regardless of runtime state.

## Integration Points

### From Agent Providers

When your agent provider spawns a subagent:

1. Create `SubagentStepDisplay` entries as the subagent executes
2. Update step status and `finished_at_ms` as work progresses
3. Wrap in a `DisplayItem::SubagentInvocation`
4. Add to `panel.display_items` at appropriate point in conversation
5. Notify UI: `cx.notify()`

### From Streaming Layer

The streaming system handles:
- Height estimation for virtual list scrolling
- Proper state preservation during save/load
- Collapsible animation and expand/collapse actions

## Design Language Consistency

The subagent card rendering follows the same pattern as existing components:

- **Collapsible cards** with `border_1()`, `rounded(px(6.0))`, `bg(secondary)`
- **Semantic colors** from theme (danger, success, info, muted_foreground)
- **Consistent spacing** with `gap_2()`, `px_3()`, `py_2()` patterns
- **Typography**: `text_xs` for details, `font_semibold` for titles, `font_family("JetBrains Mono")` for technical output
- **Icons** from the standard icon set (GitBranch, CircleX, CircleCheck, Loader, Circle, ChevronUp, ChevronDown)
- **Interactive patterns**: Clickable headers, ghost buttons, tooltips

This maintains visual harmony with tool call groups, thinking blocks, and compaction summaries.

## Subagent Control Tools

The agent system includes built-in tools for spawning and managing subagents. These are defined in [agent-providers/agent_chat_tools/src/lib.rs](agent-providers/agent_chat_tools/src/lib.rs) using the `#[tool(category = "subagent")]` macro pattern.

### spawn_subagent

Spawn a new subagent to handle a specific task.

**Parameters:**
- `name` (string, required): Display name for the subagent (e.g., "Code Analyzer", "Research Agent")
- `task` (string, required): Description of what the subagent should do
- `model` (string, optional): Model to use for the subagent (defaults to "default")
- `instructions` (string, optional): Additional system instructions for the subagent

**Returns:**
```json
{
  "ok": true,
  "subagent_id": "subagent-1715774865000-12345",
  "name": "Code Analyzer",
  "status": "spawned"
}
```

**Example Usage:**
```rust
spawn_subagent(
    name: "Code Refactoring Agent".to_string(),
    task: "Analyze this TypeScript file and suggest refactoring opportunities".to_string(),
    model: Some("gpt-4".to_string()),
    instructions: Some("Focus on performance improvements and readability".to_string()),
)
// Returns: subagent_id for tracking
```

### query_running_subagents

List all currently running or queued subagents.

**Returns:**
```json
{
  "ok": true,
  "count": 0,
  "subagents": [
    {
      "id": "subagent-1715774865000-12345",
      "name": "Code Analyzer",
      "status": "running",
      "progress": 0.45
    }
  ]
}
```

**Example Usage:**
```rust
query_running_subagents()
// Check if any subagents are still processing
```

### get_subagent_result

Retrieve the result from a completed subagent.

**Parameters:**
- `subagent_id` (string, required): The ID returned from spawn_subagent

**Returns:**
```json
{
  "ok": true,
  "subagent_id": "subagent-1715774865000-12345",
  "status": "completed",
  "result": {
    "suggestions": [
      "Extract duplicate validation logic into a shared utility",
      "Consider using a factory pattern for component creation"
    ]
  }
}
```

**Example Usage:**
```rust
get_subagent_result("subagent-1715774865000-12345".to_string())
// Retrieve the subagent's findings
```

### cancel_subagent

Cancel a running subagent.

**Parameters:**
- `subagent_id` (string, required): The ID returned from spawn_subagent

**Returns:**
```json
{
  "ok": true,
  "subagent_id": "subagent-1715774865000-12345",
  "status": "cancelled"
}
```

**Example Usage:**
```rust
cancel_subagent("subagent-1715774865000-12345".to_string())
// Stop the subagent if it's taking too long
```

## Tool Pattern Reference

All subagent tools follow the standard `#[tool]` macro pattern established in the codebase:

```rust
#[tool(category = "subagent")]
pub fn my_subagent_tool(
    required_param: String,
    optional_param: Option<String>,
) -> anyhow::Result<Value> {
    debug!("my_subagent_tool start param={}", required_param);
    
    // Implementation
    let result = json!({
        "ok": true,
        "data": "result"
    });
    
    debug!("my_subagent_tool end");
    
    Ok(result)
}
```

### Key Patterns

1. **Macro**: `#[tool(category = "subagent")]` - registers tool in category
2. **Return Type**: `anyhow::Result<Value>` - tools return JSON or errors
3. **Debug Logging**: Entry/exit points logged for tracing
4. **JSON Response**: All responses follow `{ "ok": bool, ... }` pattern
5. **Error Handling**: Use `anyhow!()` for errors

### Integration Points

Tools are registered automatically via the `#[tool]` macro:
- Discovered by `tool_registry` crate
- Documented via tracing output
- Available to agents for invocation
- Executed in separate context with `ToolContext`

## Implementation Notes

Current implementations are stubs designed to show the API surface. A production implementation would need:

1. **Persistent Storage**: Store subagent metadata and results
2. **Execution Engine**: Actual subagent spawning and lifecycle management
3. **Result Caching**: Retrieve results even after agent restarts
4. **Cancellation Signals**: Graceful shutdown mechanisms
5. **Status Tracking**: Progress updates and resource monitoring

The tools provide the interface that agents will use once the execution layer is implemented.
