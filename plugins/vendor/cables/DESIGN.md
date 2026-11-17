# Cables plugin design

## Ownership and data flow

`pulsar.cables` owns the definition and connector component classes, their
GPU source-row schemas, the ID matcher, persistent cable simulation, and the
render feature. The component crate declares `#[gpu]` row fields and their
SceneDB mirror dispatches; the plugin root registers their growable columns
and a `SceneRowsRegistration`. Endpoint placement uses the shared scene join
contracts. A cable-owned GPU derivation expands each ID group into every
unordered endpoint pair and publishes the resulting pair rows. It uses a
stable ordering over source-row indices, writes a GPU count/indirect args,
and sizes capacity from source-buffer capacity with a declared budget and
overflow diagnostic. The CPU never groups IDs, creates strand geometry, or
reads simulation results back.

The feature owns persistent per-pair strand state. Its compute step detects
new/reassigned pairs and changed reset revisions, initializes their points
on a straight line, then uses Verlet integration with distance constraints
and gravity/wind forces to settle into droop and sway. GPU-generated draw
arguments feed separate 2D ribbon and 3D tube draw paths. Bundle strands share
one centerline and use a stable normal/binormal frame; the 2D path ignores
bundle settings. The component's reset method increments a reset revision;
the editor action and scripts call that same method.

The 2D texture is taken in through SceneDB's texture binding and represented
in the GPU row by its scene texture slot. The 3D profile uses mesh contracts
from `helio-mesh-api`, never the meshes feature implementation crate. The
plugin's root registers the feature, row columns, component classes, asset
bindings and editor extension. Cables remains outside `plugins/installed`
until the feature is complete.

## AAA comparison and scale

Unreal's Cable Component uses a Verlet-style segment chain with distance
constraints; position-based rope solvers such as XPBD use iterative stretch
and bend constraints for better stability as timestep and stiffness change.
The plugin uses a GPU Verlet/constraint chain because each cable pair needs
its own state and the renderer already runs compute work before drawing.
XPBD's iterations improve stiffness but multiply per-pair compute cost; the
iteration count is a quality setting, and collision against arbitrary scene
geometry is outside this first version. The renderer's frame budget, not
blocking CPU sync, determines the allowed work.

Authored row uploads scale with edits. ID grouping and pair emission scale
with endpoint rows plus emitted pairs; the number of spans for an ID with `n`
connectors is inherently `n(n-1)/2`. Simulation memory scales with active
pairs times points per cable. Per-frame compute scales with active points and
constraint iterations; drawing scales with visible pairs, strand count,
segment count and view pixels. GPU capacities grow only when input capacity
grows, never by recreating resources per frame. Overflow is explicit rather
than silently dropping connectors. The feature and component live in the
plugin because they are cable-kind behavior; Helio owns only the generic
pass, graph, scene buffer and draw-phase seams.

## Implementation shape

- `api`: contracts, GPU row ABIs and matching WGSL snippets.
- `component`: authored classes, SceneStore source rows, asset bindings,
  reset method and the plugin's scene derivation factory.
- `feature`: GPU pair expansion, simulation and 2D/3D rendering passes,
  registered as graph nodes through `FeatureRegistrar`.
- `editor`: a reset action that calls the component method.
- root: all plugin-gated registrations.

Every feature buffer has one contract owner. Pair matching and simulation are
GPU work; SceneDB's automatic field mirroring only handles authored source
rows and does not replace those derived passes.
