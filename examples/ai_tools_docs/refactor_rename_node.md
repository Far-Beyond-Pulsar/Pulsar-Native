# refactor_blueprint_rename_node

Rename a node in the blueprint graph while updating all references throughout the entire graph structure.

## Description

This tool safely renames a blueprint node while automatically updating:
- All incoming connections to use the new name
- All outgoing connections from the renamed node
- Any cross-references in variable mappings
- Documentation and comments that reference the old name

The tool validates that the new name doesn't conflict with existing nodes and maintains graph integrity.

## Parameters

- **old_name** (string): The current name of the node to rename. Must exist in the blueprint.
- **new_name** (string): The new name for the node. Must be unique within the blueprint and follow naming conventions.

## Returns

```json
{
    "status": "success",
    "old_name": "ProcessNode_5",
    "new_name": "DataProcessor",
    "references_updated": 5,
    "connections_verified": true
}
```

Fields:
- **status**: Either "success" or "error"
- **old_name**: The name that was changed from
- **new_name**: The name that was changed to
- **references_updated**: Number of references that were updated
- **connections_verified**: Whether all connections remain valid after rename

## Usage Example

### Rename a processing node

```json
{
    "old_name": "ProcessNode_5",
    "new_name": "DataProcessor"
}
```

Result:
```json
{
    "status": "success",
    "old_name": "ProcessNode_5",
    "new_name": "DataProcessor",
    "references_updated": 5,
    "connections_verified": true
}
```

### Rename an input node

```json
{
    "old_name": "InputBuffer",
    "new_name": "RequestQueue"
}
```

Result:
```json
{
    "status": "success",
    "old_name": "InputBuffer",
    "new_name": "RequestQueue",
    "references_updated": 3,
    "connections_verified": true
}
```

## Error Cases

### Node Not Found
```json
{
    "status": "error",
    "error": "Node 'NonExistent' not found in blueprint"
}
```

### Name Already Exists
```json
{
    "status": "error",
    "error": "Node 'DataProcessor' already exists"
}
```

### Invalid Name Format
```json
{
    "status": "error",
    "error": "Name 'Invalid@Name' contains invalid characters"
}
```

### Names Are Identical
```json
{
    "status": "error",
    "error": "New name must differ from old name"
}
```

## Best Practices

1. **Verify the node exists** - Use `validate_blueprint` first to check the blueprint structure
2. **Choose descriptive names** - Use names that clearly describe the node's purpose
3. **Check references** - Review the "references_updated" count to understand impact
4. **Validate after rename** - Run `validate_blueprint` to ensure no issues were introduced
5. **Use for clarity** - Rename when nodes have ambiguous or unclear names
6. **Batch renames carefully** - If renaming multiple nodes, do one at a time to track changes

## Related Tools

- **validate_blueprint** - Verify blueprint integrity after renaming
- **optimize_blueprint** - Clean up and optimize after refactoring changes
- **analyze_blueprint_performance** - Check if rename affects performance

## Performance Considerations

- Renaming large graphs with many references may take a few seconds
- The tool scans the entire graph to update all references
- Timeout is set to 5 seconds; very large graphs might need longer
- No blueprint optimization occurs during rename; run `optimize_blueprint` separately

## Advanced Usage

### Bulk Rename Pattern

To rename multiple nodes safely:

1. Start with `validate_blueprint` to ensure integrity
2. Rename first node with `refactor_blueprint_rename_node`
3. After each rename, note the "references_updated" count
4. Once all renames complete, run `validate_blueprint` again
5. Finally run `optimize_blueprint` if needed

### Naming Conventions

Recommended naming patterns:
- **Action nodes**: `Verb_Noun` (e.g., `Process_Input`, `Validate_Data`)
- **Data nodes**: `Noun_Type` (e.g., `ConfigData`, `RequestQueue`)
- **Conditional nodes**: `Check_Condition` (e.g., `Check_Valid`, `Verify_Auth`)
- **I/O nodes**: `Direction_Resource` (e.g., `Input_File`, `Output_DB`)
