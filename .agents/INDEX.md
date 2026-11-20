# .agents - Pulsar-Native documentation

New to the project? Read [`../ARCHITECTURE.md`](../ARCHITECTURE.md) first: it is a
short tour of the layers, the plugin model, startup, scene data and one traced
editor action, and it links back here for depth.

This directory holds the reference documentation for each major subsystem. The
list of reference documents, with a line on what each covers, is the quick index
in the root [`AGENTS.md`](../AGENTS.md); it is not repeated here. Reference files
are written from the source code and describe how the systems work today.

Plan-like documents (audits, designs, specs, dispatch notes) sit in the same
directory. They are records of intent and may be ahead of, or behind, the code.
Each starts with a `Status:` line; the table below lists them.

## Plans and status

Status values: `proposed` (designed, not built), `in progress` (partly built),
`as built` (built; the text is now a record), `abandoned`, `unknown - needs
owner review`. Dates are when the status was last checked.

| Document | Status (2026-11-20) | Note |
|---|---|---|
| [BACKLOG_DISPATCH_PLAN.md](BACKLOG_DISPATCH_PLAN.md) | in progress | waves partly done; V1, K2, B5-B7 open |
| [COMPONENT_EVENT_SOURCES_PLAN.md](COMPONENT_EVENT_SOURCES_PLAN.md) | as built | P1-P6 built and committed |
| [COMPONENT_LIFECYCLE_PLAN.md](COMPONENT_LIFECYCLE_PLAN.md) | as built | P1-P6 built |
| [COMPONENT_RUNTIME_GAPS.md](COMPONENT_RUNTIME_GAPS.md) | as built | implemented; gap tables are historical |
| [COOK_GRAPH_PLAN.md](COOK_GRAPH_PLAN.md) | proposed | no cook graph in the code |
| [HELIO_SCENE_API_MIGRATION.md](HELIO_SCENE_API_MIGRATION.md) | as built | historical audit |
| [HIERARCHY_PERF_PLAN.md](HIERARCHY_PERF_PLAN.md) | in progress | waves 0-2 landed, 3-5 not started |
| [LANGUAGE_PLUGINS_PLAN.md](LANGUAGE_PLUGINS_PLAN.md) | in progress | P2-P6 built, cleanup pass open |
| [MULTIUSER_EDITING_SPEC.md](MULTIUSER_EDITING_SPEC.md) | proposed | target behaviour; partly present in code, not verified |
| [PLUGIN_API_REFACTOR_PLAN.md](PLUGIN_API_REFACTOR_PLAN.md) | in progress | phases 1-5 built; binary plugin bundling open |
| [PROJECT_GENERATION_PLAN.md](PROJECT_GENERATION_PLAN.md) | in progress | `pulsar_project` built, no callers yet |
| [QUICK_BUILD_PLAN.md](QUICK_BUILD_PLAN.md) | in progress | pack step built; quick build not verified |
| [SCRIPTING_EPIC.md](SCRIPTING_EPIC.md) | as built | agents A-F complete; some details superseded |
| [SCRIPT_VM_PLAN.md](SCRIPT_VM_PLAN.md) | as built | a few "Not yet" items remain |
| [VG_DIAGNOSIS.md](VG_DIAGNOSIS.md) | proposed | diagnosis only; rework awaits an owner decision |
| [VOXEL_STREAMING.md](VOXEL_STREAMING.md) | proposed | spec, nothing built |
| [plans/issue-705-mesh-bounds-viewport-picking.md](plans/issue-705-mesh-bounds-viewport-picking.md) | as built | PMSH v5 bounds and BVH picking |
| [plans/issue-reusable-helio-viewport.md](plans/issue-reusable-helio-viewport.md) | as built | `ui_helio_viewport` exists |
| [plans/virtual-geometry.md](plans/virtual-geometry.md) | in progress | approved; first tasks done |
| [plans/flamegraph-viewer/PLAN.md](plans/flamegraph-viewer/PLAN.md) | unknown - needs owner review | only phase 1 is traceable |
| [plans/layout-persistence/PLAN.md](plans/layout-persistence/PLAN.md) | as built | see [LAYOUT.md](LAYOUT.md) |
| [plans/settings-rebuild/README.md](plans/settings-rebuild/README.md) | in progress | steps 2-7 present, step 8 not verified |

Not in the table: [READABILITY_REPORT.md](READABILITY_REPORT.md) is a one-off
report with its own fix plan, not a subsystem plan.
