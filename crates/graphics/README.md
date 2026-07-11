# graphics

Vendored graphics stack — the Far-Beyond-Pulsar wgpu fork as a git submodule.

The `wgpu` crate is still resolved via git in `Cargo.toml` (wgpu is its own cargo workspace with
`workspace = true` deps, which prevents direct path-dep usage inside Pulsar-Native's workspace).
This submodule serves as a local reference copy for development.
