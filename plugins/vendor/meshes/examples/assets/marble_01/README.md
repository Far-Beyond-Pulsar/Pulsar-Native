# Marble 01

Original asset by Rob Tuytel, distributed by Poly Haven under CC0.

- Asset: https://polyhaven.com/a/marble_01
- License: https://polyhaven.com/license
- CC0: https://creativecommons.org/publicdomain/zero/1.0/
- Download URLs and SHA-256 hashes: sources.json

The unmodified 2K JPEG diffuse, OpenGL normal, and AO/roughness/metal maps
are embedded in the cathedral's paving material. The UVs repeat every two
metres. The GBuffer uses the map's roughness with a 1.3 scale for worn,
less polished nave paving. Runtime mipmaps filter the maps; the material
shader normalizes the sampled normal vector. No displacement is generated.

This is an illustrative material, not a historical reconstruction.
