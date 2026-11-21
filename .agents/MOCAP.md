# Motion capture (`pulsar.mocap`)

Status: phase 1 in progress (see "Phases"). Camera and device capture, a
streaming take file format (`.mocap`) and its editor, as one project plugin at
`plugins/vendor/mocap` (in-tree, like `trait_editor`; it can become its own
`Plugin_Mocap` repository when it ships).

The goal is AAA use: a performance capture session an animator can trust
(low latency live preview, no dropped samples on disk, calibrate once, retarget
to the project's skeletons, clean up and bake to `.animseq`), not a demo.

## Crates

| Crate | Package | Holds | Depends on |
|---|---|---|---|
| `format/` | `mocap_format` | the `.mocap` container, the take data model, rig profiles, signal tools (resample, One-Euro filter, gap fill). GPUI-free, engine-free, no I/O policy | `crc32fast`, `lz4_flex`, `serde` |
| `capture/` | `mocap_capture` | `CaptureSource`, the frame bus, device sources (VMC/OSC over UDP, synthetic), the camera source (`camera` feature), the pose estimator seam, calibration, the recorder (streams a take into `.mocap` as it is captured) | `mocap_format` |
| `editor/` | `mocap_editor` | the workspace, the camera viewport, sources, timeline, inspector | `mocap_capture`, `ui`, `gpui-ce`, `ui_helio_viewport` seams |
| root | `mocap_plugin` | descriptor, `build`: the file type, the editor provider; contract `pulsar.mocap.api` | all of the above, editor ones behind `editor` |

Only the root registers. Other plugins read `mocap_format` through the
`pulsar.mocap.api` contract.

## The file format (`.mocap`, magic `PMOC`)

Requirements, in order: a crash during a take loses at most the last block;
a take of an hour opens instantly and scrubs without loading it all; the
schema is open (any rig, any channel) and versioned; nothing is lossy unless
the author chose it.

```
file   = header chunk*
header = "PMOC" u16 version u16 flags
chunk  = fourcc[4] u32 payload_len u32 crc32(payload) payload
```

| Chunk | Payload | Notes |
|---|---|---|
| `META` | take: id, name, fps (rational), start timecode, created, performer, notes, sources | JSON; first chunk |
| `RIGG` | the channel schema: `RigProfile` (name, joints with parent, channels with kind/unit/quantisation) | JSON; second chunk |
| `CALB` | calibration: per-source offsets, T-pose snapshot, scale | JSON; optional, repeatable (last wins) |
| `BLCK` | one block of `block_frames` frames of every channel, delta + zigzag varint (lossless f32 bit pattern) or 16-bit quantised against a per-block range, then LZ4 | the stream: appended as captured |
| `EVNT` | markers and events (slate, clap sync, user marks) with frame and text | JSON; repeatable |
| `INDX` | per block: first frame, file offset, frame count; the take's frame count | last; if missing the reader rebuilds it by scanning chunks (a crashed take) |

Rules:

- Channels are dense columns (`ChannelKind`: `Position3`, `Rotation4` (unit
  quaternion, hemisphere-continuous on write), `Scalar`, `Confidence`), one
  `f32` sample per frame; a missing sample is `NaN` (a gap), never zero.
- A reader skips an unknown chunk by its length; a version newer than the
  reader's major is refused with a message.
- Every chunk is checksummed; a bad `BLCK` is reported with its frame range
  and the rest of the take still reads.
- Writing is append-only (`TakeWriter`), so a recording streams to disk from
  the capture thread without holding the take in memory.

## Capture

```
CaptureSource  --poses-->  PoseBus (bounded SPSC, drop-oldest, counted)  --> Processor --> Recorder (.mocap)
               --video-->  VideoMailbox (latest frame wins)              --> Viewport / PoseEstimator
```

- A **source** is a thread-owning object described by `SourceDescriptor` (id,
  kind, label, capabilities) with a `SourceStatus` the editor lists. Sources
  are discovered by `SourceProvider`s (cameras by enumeration; VMC by a port
  the author adds).
- **Video** never queues: the viewport and any estimator read the newest frame
  (`VideoMailbox`), so a slow consumer costs frames, not latency.
- **Poses** are a bounded queue; overflow drops the oldest and counts it (the
  session HUD shows drops, the recorder refuses to claim a clean take with
  drops).
- Device sources shipped: **VMC** (OSC over UDP: the protocol VRM trackers,
  Rokoko and Xsens bridges speak: bones, root, blendshapes) and **Synthetic**
  (a deterministic animated figure: demos, tests, CI without hardware).
- **Camera** (`camera` feature, `nokhwa`): enumerate, open at a chosen mode,
  frames into the mailbox as RGBA. Pose estimation from video goes through
  `PoseEstimator` (video frame in, pose frame out); no model ships in phase 1,
  so a camera shows live video and records it as a reference but produces
  joints only through an estimator plugin (phase 3).
- **Processing**: per-channel One-Euro filter (latency-adaptive smoothing, the
  standard for live mocap), gap fill on playback, hemisphere continuity for
  quaternions. Processing never edits the recorded raw take; it is a stack
  applied on read and baked on export.

## The editor (golden UI patterns)

One `Workspace` (dock, `DockChannel`, resizable, restorable) per open `.mocap`
or a live session:

| Panel | Pattern |
|---|---|
| Viewport | `wgpu_surface` fed by `FrameBlit` (the camera feed as a texture, first camera in the engine UI), with `PanelCanvas` overlay cards (record, status, drops, latency: drag/resize, slots) |
| Skeleton | the live or scrubbed pose, orbit, drawn on a GPUI canvas in phase 1; a Helio preview in phase 2 (load `helio-architecture` first) |
| Sources | virtualized list of sources with status, add VMC, start/stop |
| Timeline | virtualized tracks (`v_virtual_list`) beside a canvas of curves; scrub, zoom, markers; resizable tracks |
| Inspector | take, rig, calibration, filter stack |

Every async job (enumeration, opening a camera, recording, bake) is an editor
task (`.agents/EDITOR_TASKS.md`). The camera runs on its own capture thread
registered as a service; the viewport only reads the mailbox.

## Phases

1. Format + capture core + VMC + synthetic + recorder + the editor shell with
   viewport, sources, timeline, skeleton. (this branch)
2. Camera backend and the blit surface verified on hardware; Helio skeleton preview; bake to
   `.animseq` through `skeletal_assets` with a retarget map; `MocapPlayback`
   component.
3. Pose estimation plugin (ONNX runtime, body/hands/face), multi-camera
   calibration and triangulation, IMU suits.
4. Cleanup tools (foot lock, outlier rejection, keyframe reduction), take
   browser, timecode sync, live link into a level (stream to a character).

## Decisions

- Extension `.mocap`, magic `PMOC`. Not Foxglove's MCAP: that is a robotics
  log container; ours is a typed animation take. No conflict on magic.
- Raw is sacred: filters and retargeting are views over the raw take.
- No model ships in phase 1: honest about what a camera can do today.

## Unverified / known gaps

- The camera backend and the blit path have not run against real hardware.
- VMC is implemented from the published spec, not tested against a live app.

## Multiple and mixed sources (as built)

Any number and mix of sources run at once: cameras, VMC performers, OSC sensors
(`OscSensorSource`, declared address-to-channel maps; the IMU preset is
`/imu/quat`, `/imu/accel`, `/imu/gyro`), synthetic figures. Each source has its
own `PoseBus` and `VideoMailbox`.

- **Poses and sensors** are merged by `Mixer` into one stream and one rig
  (`merge_rigs`): one source keeps its names; several are namespaced
  `<source id>/<name>`. A merged frame is emitted per source frame, holding the
  others' latest values; a source silent for more than `MAX_HOLD_S` is a gap.
  The set of recorded sources is fixed while recording.
- **Cameras** are tiled in the viewport (`tile_rect`, one texture and uniform
  buffer per feed). A camera alone has no joints until a pose estimator exists,
  so Record needs a pose or sensor source (the HUD says so).
- **Target skeleton:** recording needs none; a take keeps its sources' rigs.
  Choosing a skeleton happens when a take is baked or retargeted (phase 2).

The reader always scans chunk headers to find blocks; `INDX` is for external
tools and is not required to open a crashed take.

## Body tracking from cameras (as built)

`Track body` on a camera adds an `EstimatorSource` (`pose:<camera id>`) that runs
a pose model on that camera's newest frame, stamped with the frame's capture
time, and delivers poses like any source (they mix, record and plot with the
rest). Output is the COCO-17 image-space rig (`coco17_rig`): per keypoint
`(x, y)` as fractions of the frame and `z` as confidence; a keypoint under 0.25 is a
gap. The overlay draws them on that camera's tile.

- Backend: `OnnxPoseEstimator` (`pose` feature, ONNX Runtime through `ort`; the
  runtime is fetched by `ort`'s build and linked in). Input layout, size and
  element type (`f32`, `i32`, `u8`) are read from the model; output is the first
  tensor, 17 x `(y, x, score)` (MoveNet-style). It loads the bundled model in
  about 0.15 s. `tract` was tried first and rejected: 0.21.10 runs out of memory
  (8 GB and growing) typing this MoveNet file. `PULSAR_MOCAP_POSE_SCALE`
  (`unit`, `signed`) sets the input range when it is not 0..255.
- The model is built in: `capture/build.rs` fetches MoveNet SinglePose Lightning
  (Apache-2.0, Xenova's ONNX conversion, pinned revision and SHA-256, cached in
  `~/.cargo/pulsar-mocap-models`) and embeds it; offline builds set
  `PULSAR_MOCAP_POSE_MODEL_FILE`, and a build without it asks for a model. A model
  file wins over the built-in one: the path chosen with `Pose model`, else
  `PULSAR_MOCAP_POSE_MODEL`, else `<project>/.pulsar/models/pose.onnx`.
- Not built: converting keypoints to joint rotations (retargeting), multi-person,
  and triangulating calibrated cameras into 3D.

## The Scene panel (as built)

The Skeleton panel is replaced by **Scene**: one orbitable 3D picture of
everything captured, built by `mocap_capture::build_scene` from the merged rig and
the live (or taken) pose. In one metric space (+Y up, figures facing +Z, left at
+X):

One subject is assumed for now (marking and labelling subjects comes later): every
body-shaped source is fused into a single character, "Subject 1".

- **Body sources** (VMC, suit, synthetic): joint rotations and root positions are
  averaged by joint name across sources, then posed on the richest skeleton.
- **Camera keypoints** (`Track body`): the best camera's (highest total confidence
  this frame; no geometry is invented between uncalibrated views) are lifted
  (`lift_keypoints`: torso-sized, hips at hip height) and fill the limb joints no
  body source observes (highlighted). With no body source they are the character,
  flat and flagged 2D; depth needs calibrated multi-camera triangulation.
- **Sensors**: each orientation channel is an RGB axis triad attached beside the
  character, listed as unassigned.
- **Cameras**: each running camera is a small frustum in front, looking at the stage.

The legend lists every item. The camera view's overlay (Skeleton toggle) still
draws bodies and keypoints on each video tile.

## Multi-camera calibration and triangulation (as built)

Cameras are not assumed to be anywhere. With two or more cameras running `Track body`,
**Calibrate cameras** walks the subject through guided movements (stand still, left
hand up, right hand up, arms out, swing arms, walk in place, jog in place, turn
around). `Guide` detects each from the subject's own keypoints (measured in torso
lengths, from the camera that sees them best); samples are collected only while the
movement is being done, tagged with the step. When the steps are done (or **Solve
now**), `Calibrator::solve` runs as an editor task:

1. Pairs of cameras: essential matrix from keypoint correspondences (8-point,
   RANSAC), decomposed by cheirality; poses chained from camera 0.
2. Metric scale from human bone-length priors (median over all bones and samples),
   times `body_scale` (the subject's height over 1.75 m).
3. Refinement: alternating triangulation, rescaling and per-camera Levenberg-Marquardt
   on rotation, translation and focal length, minimising reprojection error.
4. World alignment to the subject: +Y along the torso, floor at the ankles, origin
   under the mean hips, +Z the way the subject faces, left at +X.

It refuses, with a reason, when there are too few cameras or samples, a camera shares
too little of the subject, the poses are degenerate or the fit is worse than 8 px.
`Calibrator::validate` then checks the result against the steps (a raised hand reads
up in 3D, the arm span matches the body).

Intrinsics: pinhole, principal point at the image centre, focal from a 70 degree
field of view and refined. Metric scale is as good as the subject is close to the
priors: set the body scale when the height is known.

Once calibrated, every merged frame gets 17 triangulated world keypoints
(`subject3d/kp3d.*`, weighted DLT, a view that disagrees by over 12 px dropped, a
point over 15 px dropped): they are recorded in the take, the take stores the camera
calibration, and the Scene draws the cameras where they are and uses the 3D keypoints
as (or to complete) the subject. Tested against synthetic ground truth: four cameras
around a moving, turning subject are located within 15 cm and 3 degrees, and
triangulated joints are within 3 cm after rigid alignment.

Not built: camera lens distortion, time offsets between cameras (frames within 80 ms
count as simultaneous), tracking more than one subject across views, and saving a
calibration to reuse for a later session.
