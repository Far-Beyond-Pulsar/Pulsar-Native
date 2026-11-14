# Asset Viewer examples

Files to open in the Asset Viewer, one set per file type it registers. This
folder is read-only: copy a file into your project to import or use it.

## `.fbx` (model source files)

Hand-written ASCII FBX files. Lines starting with `;` are comments, so the
files explain themselves.

| File | Teaches |
|---|---|
| `Pyramid.fbx` | The smallest useful model: 5 vertices, a quad and four triangles, normals, UVs, one material |
| `TwoMaterialBox.fbx` | Two materials on one mesh (per-polygon material indices), which become two material slots |

## `.mesh` (native baked meshes)

Engine-native meshes, made from the FBX examples above by the model importer
(`Pyramid.fbx` becomes `Pyramid.mesh`). The format is binary: the sibling `.fbx`
is its readable source.

| File | Teaches |
|---|---|
| `Pyramid.mesh` | A single-section mesh with one material slot |
| `TwoMaterialBox.mesh` | Two sections and two material slots, which the viewer lists so each can be assigned a material |

## `.png` (images)

Generated test images, each useful as a texture of a different kind.

| File | Teaches |
|---|---|
| `UvGrid.png` | Colour by U and V with grid lines: check texture mapping and stretching |
| `BumpNormals.png` | A tangent-space normal map (flat blue, bumps tilt red and green) |
| `RoughnessRamp.png` | A greyscale gradient for a roughness or mask channel |
| `AlphaDisc.png` | An RGBA image with a soft transparent edge |
| `Bricks.png` | A tileable albedo of offset brick rows |
