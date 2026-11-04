# Castle Wall Variation

Original asset by Rob Tuytel, distributed by Poly Haven under CC0.

- Asset: https://polyhaven.com/a/castle_wall_varriation
- License: https://polyhaven.com/license
- CC0: https://creativecommons.org/publicdomain/zero/1.0/
- Sources and integrity hashes: sources.json

The unmodified 1K PNG diffuse, OpenGL normal and AO/roughness/metal maps are embedded in the cathedral example. The source identifies a two-metre tile width. Diffuse is sampled as sRGB; normals and ARM use linear UNORM. Runtime box-filtered mipmaps average color in linear light and leave normal-vector normalization to the material shader. No displacement geometry is generated. This is a wall material, not a historical reconstruction of a particular cathedral.

HLFS_NO_STONE_TEXTURES=1 retains the flat-material control; HLFS_NO_STONE_NORMALS=1 keeps color/ARM while disabling the normal map. The checker diagnostic takes precedence over these assets. Assets require no download or machine-specific path at runtime.
