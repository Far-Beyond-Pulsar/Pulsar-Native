# VG stress museum credits

Statues are from the Stanford 3D Scanning Repository (Lucy, Thai Statue/xyzrgb_statuette,
xyzrgb_dragon, xyzrgb_manuscript, Happy Buddha, Dragon). Credit: Stanford Computer Graphics
Laboratory. Free for research with credit; not for commercial use or inclusion in a product
for sale. They are downloaded on demand by `crates/examples/vg_stress_prep` (during
`cargo vg-stress` builds or on the demo's first run), which prints this notice, and the
converted GLBs are git-ignored.

The architectural kit (`kit/meshes/*.fbx`) is original geometry authored for this demo.

The trees and marble textures are CC0 assets from Poly Haven (https://polyhaven.com,
https://polyhaven.com/license): Pine Tree 01, Fir Tree 01, Island Tree 01/02/03 and Small
Tree 02 (Rob Tuytel, Rico Cilliers and others) and the marble sets Grey Cartago 01 and 03
(Lucas Sanchez Ros, Jenelle van Heerden), Marble 01 (Rob Tuytel) and Marble Mosaic Tiles
(Amal Kumar). They are downloaded on demand by `vg_stress_prep/polyhaven.rs`, which writes
the exact per-asset credits, with the authors reported by the Poly Haven API, to
`assets/vg_stress/CREDITS_POLYHAVEN.md`. CC0 requires no attribution; it is given with thanks.
