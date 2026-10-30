`multi_skin.gltf` is a small generated glTF 2.0 regression model. Both skins are
named `Rig` and reference the same two-primitive mesh. A repeated node reuses the
first skin; an unskinned node uses the same mesh translated five units along X.
The skins have different joint palettes and inverse binds, share `A0`, and use
different skeleton roots. `Move` animates that shared joint.

The three vertices have nine, eight equal, and two meaningful influences across
`JOINTS_0`/`WEIGHTS_0` through `JOINTS_2`/`WEIGHTS_2`. The first retains joints
8, 6, 5, 4 with weights 0.625, 0.15625, 0.125, 0.09375; the tie retains 0–3.
The two primitives share the attribute accessors and use separate materials.

Khronos glTF Validator reports zero errors. Its warning about the unskinned node
using a weighted mesh is intentional: that instance must import as static content.
