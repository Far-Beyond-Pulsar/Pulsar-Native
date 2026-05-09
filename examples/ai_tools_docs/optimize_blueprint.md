# optimize_blueprint

Optimize blueprint performance and structure.

## Description

Applies various optimizations including node consolidation, dead code removal, and connection path optimization. Can run in automatic mode or show recommendations.

## Parameters

- **auto_apply** (boolean): Automatically apply optimizations or just show recommendations
- **focus** (string): Focus area - "performance", "size", or "readability"

## Returns

```json
{
    "status": "success",
    "optimizations_applied": 3,
    "focus": "performance",
    "auto_applied": true,
    "performance_improvement_percent": 15.5,
    "size_reduction_bytes": 1024
}
```

## Usage Examples

### Focus on Performance
```json
{
    "auto_apply": false,
    "focus": "performance"
}
```

### Automatically Optimize for Size
```json
{
    "auto_apply": true,
    "focus": "size"
}
```

## Best Practices

1. Validate before optimizing with `validate_blueprint`
2. Start with "readability" focus for complex blueprints
3. Use "auto_apply": false first to review changes
4. Re-validate after optimization

## Related Tools

- **validate_blueprint** - Check blueprint integrity
- **analyze_blueprint_performance** - Measure performance improvements
