# Plugin_Mocap

Motion capture for the Pulsar editor: live capture from cameras (with body tracking by an embedded
pose model), VMC/OSC sources and sensors, multi-camera calibration and triangulation, the fused
scene view, and the `.mocap` take format with its editor.

This repository is vendored into [Pulsar-Native](https://github.com/Far-Beyond-Pulsar/Pulsar-Native)
as a git submodule at `plugins/vendor/mocap`. Design, formats and status: `.agents/MOCAP.md` in
Pulsar-Native.

| Crate | What |
|---|---|
| `mocap_plugin` (root) | `pulsar.mocap`: registers the editor provider |
| `format` (`mocap_format`) | the `.mocap` take format: chunks, codecs, reader and writer |
| `capture` (`mocap_capture`) | sources, mixing, pose estimation, calibration, scene fusion |
| `editor` (`mocap_editor`) | the dock workspace: viewport, scene, sources, timeline, inspector, calibration |

Setup illustrations in `editor/assets/` are rendered by `editor/tools/render_setups.py` (Blender).

## Building and testing

The crates are members of the Pulsar-Native workspace; build and test from there
(`cargo test -p mocap_capture -p mocap_format -p mocap_editor`).
