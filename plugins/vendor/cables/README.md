# Cables plugin

`pulsar.cables` is linked by `plugins/installed`. It contributes authored
cable/connector components, SceneDB GPU rows, the endpoint placement join,
and a Helio render-graph feature. The design and ownership boundary is
recorded in [DESIGN.md](DESIGN.md).

Cable definitions and connectors pair by a nonzero integer ID. For `n`
connectors on an ID, the GPU vertex stage considers every unordered pair
(`n * (n - 1) / 2`). The current GPU feature draws a sagged span with animated
wind sway and bundle lanes. `reset_simulation()` increments a revision in the
GPU row; persistent reset-driven physics, texture/model asset binding, and a
dedicated editor reset action remain to be completed.

The render-mode field is present in the GPU ABI. The current pass still uses
camera-facing ribbon geometry in both modes; true 3D tube meshes and sampled
2D tile textures are not implemented yet.
