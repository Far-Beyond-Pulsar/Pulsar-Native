# analyze_blueprint_performance

Analyze blueprint performance characteristics.

## Description

Profiles blueprint execution, identifies bottlenecks, and provides detailed performance metrics and recommendations for improvement.

## Parameters

- **profile** (boolean): Run performance profiling on the blueprint
- **detailed** (boolean): Include detailed breakdown by node and connection

## Returns

```json
{
    "status": "success",
    "total_execution_time_ms": 1250,
    "profiled": true,
    "detailed": true,
    "nodes": [
        {
            "name": "ProcessNode",
            "execution_time_ms": 500,
            "memory_mb": 2.5,
            "calls": 100
        }
    ]
}
```

## Usage Examples

### Quick Performance Check
```json
{
    "profile": false,
    "detailed": false
}
```

### Full Profiling with Details
```json
{
    "profile": true,
    "detailed": true
}
```

## Best Practices

1. Profile with detailed=true to identify bottlenecks
2. Use after making changes to compare performance
3. Focus optimization on high-execution-time nodes
4. Use with `optimize_blueprint` to fix issues
