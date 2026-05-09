# validate_blueprint

Validate the blueprint for structural and logical errors.

## Description

This tool performs comprehensive validation of the blueprint graph including:
- **Cycle detection**: Identifies circular dependencies that could cause infinite loops
- **Type checking**: Verifies that all connections have compatible types
- **Unused node detection**: Finds nodes that are never executed
- **Connection validation**: Ensures all connections are properly formed
- **Reference validation**: Checks that all references point to valid targets

You can run the full validation or focus on specific checks based on your needs.

## Parameters

- **check_cycles** (boolean): Enable cycle detection to find circular dependencies
- **check_types** (boolean): Enable type checking on all connections
- **find_unused** (boolean): Identify nodes that are never executed

## Returns

```json
{
    "status": "success",
    "issues_found": 2,
    "issues": [
        {
            "type": "cycle",
            "severity": "high",
            "message": "Circular dependency: ProcessNode → DataNode → ProcessNode",
            "affected_nodes": ["ProcessNode", "DataNode"]
        },
        {
            "type": "unused",
            "severity": "low",
            "message": "Node 'LegacyProcessor' is never executed",
            "affected_nodes": ["LegacyProcessor"]
        }
    ],
    "check_cycles": true,
    "check_types": true,
    "find_unused": true
}
```

## Usage Example

### Full validation (all checks)

```json
{
    "check_cycles": true,
    "check_types": true,
    "find_unused": true
}
```

Result:
```json
{
    "status": "success",
    "issues_found": 0,
    "issues": [],
    "check_cycles": true,
    "check_types": true,
    "find_unused": true
}
```

### Check only for cycles

```json
{
    "check_cycles": true,
    "check_types": false,
    "find_unused": false
}
```

### Find unused nodes only

```json
{
    "check_cycles": false,
    "check_types": false,
    "find_unused": true
}
```

Result:
```json
{
    "status": "success",
    "issues_found": 1,
    "issues": [
        {
            "type": "unused",
            "severity": "low",
            "message": "Node 'OldProcessor' is never executed",
            "affected_nodes": ["OldProcessor"]
        }
    ],
    "check_cycles": false,
    "check_types": false,
    "find_unused": true
}
```

## Issue Types

### Cycle (High Severity)
```json
{
    "type": "cycle",
    "severity": "high",
    "message": "Circular dependency: A → B → C → A",
    "affected_nodes": ["A", "B", "C"]
}
```
Indicates nodes that form circular dependencies. Will cause infinite loops at runtime.

### Type Mismatch (High Severity)
```json
{
    "type": "type_mismatch",
    "severity": "high",
    "message": "Connection from NodeA (output: Integer) to NodeB (input: String)",
    "affected_nodes": ["NodeA", "NodeB"]
}
```
Connection between nodes with incompatible types. Will cause runtime errors.

### Unused (Low Severity)
```json
{
    "type": "unused",
    "severity": "low",
    "message": "Node 'X' is never executed",
    "affected_nodes": ["X"]
}
```
Nodes that are never reached during execution. These can be safely removed.

### Dangling Reference (Medium Severity)
```json
{
    "type": "dangling_reference",
    "severity": "medium",
    "message": "Node 'X' references non-existent node 'Y'",
    "affected_nodes": ["X", "Y"]
}
```
References to nodes that don't exist. Will cause runtime failures.

## Best Practices

1. **Run after major changes** - Always validate after making refactoring changes
2. **Fix high severity issues first** - Address cycles and type mismatches immediately
3. **Review low severity issues** - Consider removing unused nodes to clean up
4. **Full validation workflow** - Use all three checks for comprehensive analysis
5. **Version control integration** - Validate before committing blueprint changes
6. **Use with other tools** - Combine with `refactor_blueprint_rename_node` and `optimize_blueprint`

## Fixing Issues

### Fix a Cycle
1. Run validation with `check_cycles: true`
2. Identify the circular path in the issues
3. Break the cycle by removing or redirecting one connection
4. Run validation again to confirm

### Fix Type Mismatches
1. Run validation with `check_types: true`
2. For each type mismatch issue, modify connections or add converters
3. Run validation again to verify fix

### Clean Up Unused Nodes
1. Run validation with `find_unused: true`
2. Review unused nodes to ensure they're not needed
3. Delete unused nodes with the editor
4. Run validation to confirm removal

## Related Tools

- **refactor_blueprint_rename_node** - Rename nodes without breaking references
- **optimize_blueprint** - Clean up and optimize the blueprint
- **analyze_blueprint_performance** - Check performance after validation fixes

## Performance Considerations

- Full validation timeout: 8 seconds
- Cycle detection: O(n) where n = number of nodes
- Type checking: O(n + m) where m = number of connections
- Unused detection: O(n) graph traversal
- Large blueprints (1000+ nodes) may use most of the timeout
