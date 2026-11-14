# Render graph examples

Small teaching `.rendergraph` assets. Open one in the Render Graph Editor, or
copy it into your project (the editor's "Save a copy to the project") and pick
it in World Settings. This folder is read-only.

These are not the engine's pipelines: those are in the Default Pipelines
plugin's folder, made from every installed plugin's passes. The graphs here
are deliberately small and each shows one idea. They pass the graph
validation (nothing that stops a graph from building); they are flow-only
chains, the form that needs no probed pins, so they open at once in the editor.
Each carries a `name`, a `description` and comment boxes on the canvas.

| File | Teaches |
|---|---|
| `Minimal.rendergraph` | The empty pipeline: On Frame Start flows into Output. What a new graph is |
| `LeanForward.rendergraph` | A small forward pipeline stage by stage: frame setup (the lighting model), background, depth pyramid, geometry, FXAA, post-processing |
| `NoPostProcess.rendergraph` | A deferred-model graph with a stage removed: delete the node, wire its neighbours together |
| `DebugOverlays.rendergraph` | Display passes (buffer view, performance overlay, debug draw) that run after the final image |
| `Resources.rendergraph` | Resource nodes: transient colour and depth textures, a persistent texture, their `name`, `format` and `size` parameters |

A `.rendergraph` is JSON: `nodes` (an `id`, the node `kind`, its `params`, a
canvas `position`), `connections` (from a node's output pin to another's
input pin; `out` to `in` is the execution flow) and `comments` (labelled boxes).
