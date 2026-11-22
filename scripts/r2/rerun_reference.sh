#!/bin/bash
# Reruns the comparisons against the reference path tracer
# (rendering.md section 4, "Direct light samples and the reference"):
#
#   1. indoor_cathedral_gi: the real-time image and its indirect diffuse
#      against the converged reference (linear dumps and final images), and
#      the GPU cost of GI and of a reference frame, the build before
#      (BASE_CATHEDRAL) interleaved with this one.
#   2. voxel_craft, the first pose of the tour: r0's, r1's and this build's
#      real-time image raw (debug mode 250: the post shows a debug view's
#      scene colour without exposure or tone curve, as it shows the
#      reference's view 62) and indirect diffuse (23) against the
#      reference's (62, 63); this build's real-time views (the frame, 22,
#      23) snapshotted linear with the reference (HELIO_REFERENCE_DUMP); the
#      reference's parts (reference_parts.py: the sky seen, the lights at the
#      first hit, the sky there, the first bounce, later bounces) and its
#      first hit's albedo against the G-buffer's (4).
#
# Run from a Pulsar-Native checkout after building:
#   cargo build --release -p gi-examples --bin indoor_cathedral_gi \
#       -p helio-voxel-examples --bin voxel_craft
# Every GPU run goes through the shared GPU lock. Rerun at integration, when
# the meshes' and the terrain's Full surfaces land: the reference sees what
# the tracers answer.
#
# Environment: TARGET (cargo target dir), OUT (results), BASE_CATHEDRAL,
# R0_CRAFT, R1_CRAFT (kept executables), FRAMES (the reference's frames; on
# voxel_craft past CRAFT_WAIT, default 5000, which covers its kernels'
# compile on a cold cache),
# PART (cathedral, craft or all), NO_FOLIAGE (1: voxel_craft without its
# procedural grass in every run; the reference does not trace grass, so with
# it the real-time image has a meadow where the reference has bare turf).
set -u
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TARGET=${TARGET:-C:/t/pulsar-r1}
OUT=${OUT:-C:/t/r2d/compare}
FRAMES=${FRAMES:-2400}
PART=${PART:-all}
GPU=${GPU:-C:/Users/thiag/shared-locks/gpu.sh}
WHO=${WHO:-r2-reference}
BASE_CATHEDRAL=${BASE_CATHEDRAL:-C:/t/int/indoor_cathedral_gi_r2.exe}
R0_CRAFT=${R0_CRAFT:-C:/Users/thiag/rewrite-baselines/voxel_craft_r0.exe}
R1_CRAFT=${R1_CRAFT:-C:/t/int/voxel_craft_r1_merged.exe}
COMPARE="python $ROOT/scripts/r2/reference_compare.py"
mkdir -p "$OUT/cathedral" "$OUT/craft"

# ── 1. The cathedral ─────────────────────────────────────────────────────
if [ "$PART" != craft ]; then
cp "$TARGET/release/indoor_cathedral_gi.exe" "$OUT/cathedral/cathedral_new.exe"
cd "$OUT/cathedral" || exit 1
cathedral() { # exe name [env...]
    local exe=$1 name=$2
    shift 2
    env "$@" HELIO_CAPTURE="$OUT/cathedral/$name.png" HELIO_CAPTURE_TIMINGS=1 \
        bash "$GPU" "$WHO" "$exe" > "$OUT/cathedral/$name.log" 2>&1
}
# Cost and image, the two builds interleaved, two runs each (frame 240).
for run in a b; do
    cathedral "$BASE_CATHEDRAL" "base_$run" HELIO_CAPTURE_FRAME=240
    cathedral ./cathedral_new.exe "new_$run" HELIO_CAPTURE_FRAME=240
done
# The scene held still (HELIO_STILL=1: no flicker, rows rewritten
# unchanged, so the real-time frame is right but a reference restarts every
# frame; =2: rows left alone after 60 frames, so the reference accumulates).
# The real-time image and its linear snapshot come from a =1 run, its
# indirect diffuse from the deferred lighting's view 23, the reference (view
# 62, its linear dump) and its indirect diffuse (view 63) from =2 runs.
cathedral ./cathedral_new.exe realtime HELIO_STILL=1 HELIO_CAPTURE_FRAME=240     HELIO_DEBUG_MODE=62 HELIO_REFERENCE_DUMP="$OUT/cathedral/realtime"
cathedral ./cathedral_new.exe still_realtime HELIO_STILL=1 HELIO_CAPTURE_FRAME=240 HELIO_DEBUG_MODE=250
cathedral ./cathedral_new.exe still_indirect HELIO_STILL=1 HELIO_CAPTURE_FRAME=240 HELIO_DEBUG_MODE=23
cathedral ./cathedral_new.exe reference HELIO_STILL=2 HELIO_CAPTURE_FRAME="$FRAMES"     HELIO_DEBUG_MODE=62 HELIO_REFERENCE_DUMP="$OUT/cathedral/reference"
cathedral ./cathedral_new.exe reference_diffuse HELIO_STILL=2 HELIO_CAPTURE_FRAME="$FRAMES"     HELIO_DEBUG_MODE=63
cp "$OUT/cathedral/realtime_realtime.f32" "$OUT/cathedral/reference_realtime.f32"
echo "== cathedral: linear, real-time against the reference"
$COMPARE dump "$OUT/cathedral/reference" "$OUT/cathedral"
echo "== cathedral: final images against the reference's"
$COMPARE png "$OUT/cathedral/reference.png" "$OUT/cathedral" \
    realtime="$OUT/cathedral/still_realtime.png"
echo "== cathedral: indirect diffuse (deferred view 23) against the reference's (63)"
$COMPARE png "$OUT/cathedral/reference_diffuse.png" "$OUT/cathedral" \
    indirect="$OUT/cathedral/still_indirect.png"
echo "== cathedral: GPU timings (ms)"
grep -h "\[timings\]" "$OUT"/cathedral/*_[ab].log | sort | head -200

fi

# ── 2. voxel_craft, the tour's first pose ────────────────────────────────
# Every run keeps its executable's pipeline cache (PULSAR_PIPELINE_CACHE):
# on a cold cache the terrain's draw and ray kernels take a minute and a
# half to compile, and until they have GI holds its probes and the
# reference sums nothing (a frame a tracer answered short of is not
# summed). CRAFT_WAIT frames before a real-time capture (default 5000)
# cover that; the reference's runs wait FRAMES more frames than that.
if [ "$PART" != cathedral ]; then
CRAFT_WAIT=${CRAFT_WAIT:-5000}
REF_WAIT=$((CRAFT_WAIT + FRAMES))
cp "$TARGET/release/voxel_craft.exe" "$OUT/craft/voxel_craft_new.exe"
cd "$OUT/craft" || exit 1
craft() { # exe dir [env...]
    local exe=$1 dir=$2
    shift 2
    rm -rf "$OUT/craft/$dir"
    mkdir -p "$OUT/craft/$dir"
    env "$@" ${NO_FOLIAGE:+VOXEL_CRAFT_NO_FOLIAGE=1} \
        PULSAR_PIPELINE_CACHE="$OUT/craft/cache_$(basename "$exe" .exe).bin" \
        VOXEL_CRAFT_SHOTS="$OUT/craft/$dir" VOXEL_CRAFT_POSES=1 VOXEL_CRAFT_NO_CLOUDS=1 \
        RUST_LOG=info,helio_pass_reference=debug,helio::compile=info \
        bash "$GPU" "$WHO" "$exe" > "$OUT/craft/$dir/log.txt" 2>&1
}
burst() { # exe dir [env...]: eight frames after CRAFT_WAIT
    local exe=$1 dir=$2
    shift 2
    craft "$exe" "$dir" VOXEL_CRAFT_BURST=8 VOXEL_CRAFT_WAIT="$CRAFT_WAIT" \
        VOXEL_CRAFT_WAIT_MAX=$((CRAFT_WAIT * 2)) "$@"
}
once() { # exe dir [env...]: one frame after REF_WAIT
    local exe=$1 dir=$2
    shift 2
    craft "$exe" "$dir" VOXEL_CRAFT_BURST=1 VOXEL_CRAFT_WAIT="$REF_WAIT" \
        VOXEL_CRAFT_WAIT_MAX=$((REF_WAIT * 2)) "$@"
}
# The real-time images of r0, r1 and this build: the whole frame raw (a
# debug mode no pass draws, 250: the post shows any debug view's scene
# colour without exposure or tone curve, as it shows the reference's 62, so
# the two compare) and its indirect diffuse (view 23, against 63).
for build in r0 r1 r2; do
    case $build in r0) exe=$R0_CRAFT;; r1) exe=$R1_CRAFT;; r2) exe=./voxel_craft_new.exe;; esac
    burst "$exe" "${build}_raw" VOXEL_CRAFT_DEBUG=250
    burst "$exe" "${build}_indirect" VOXEL_CRAFT_DEBUG=23
done
# The reference through the same post (fixed exposure): its image (62) and
# its indirect diffuse (63).
once ./voxel_craft_new.exe reference VOXEL_CRAFT_DEBUG=62
once ./voxel_craft_new.exe reference_diffuse VOXEL_CRAFT_DEBUG=63
# Linear, this build only (HELIO_REFERENCE_DUMP: the reference traces
# unseen, the real-time scene colour of the view shown snapshotted with
# it): the whole frame, its direct light (22), its indirect diffuse (23).
for view in 0 22 23; do
    once ./voxel_craft_new.exe "linear_$view" VOXEL_CRAFT_DEBUG=$view \
        HELIO_REFERENCE_DUMP="$OUT/craft/linear_$view/dump"
done
# The reference's parts (reference_parts.py, from disk) and its first hit's
# albedo against the G-buffer's albedo view (4).
SHADER="$ROOT/crates/renderer/helio/crates/passes/3d/helio-pass-reference/shaders/reference.wgsl"
cp "$SHADER" "$OUT/craft/reference.wgsl.kept"
trap 'cp "$OUT/craft/reference.wgsl.kept" "$SHADER"' EXIT
python "$ROOT/scripts/r2/reference_parts.py" "$SHADER" "$OUT/craft/parts" > /dev/null
for part in visible direct sky first later albedo; do
    cp "$OUT/craft/parts/reference_$part.wgsl" "$SHADER"
    view=0
    [ "$part" = albedo ] && view=4
    once ./voxel_craft_new.exe "part_$part" HELIO_SHADER_FROM_DISK=1 VOXEL_CRAFT_DEBUG=$view \
        HELIO_REFERENCE_DUMP="$OUT/craft/part_$part/dump"
done
cp "$OUT/craft/reference.wgsl.kept" "$SHADER"
mean_burst() { # dir -> dir/mean.png
    python -c "
import sys, glob, numpy as np
from PIL import Image
files = sorted(glob.glob(sys.argv[1] + '/shot_00*.png'))
mean = np.mean([np.array(Image.open(f).convert('RGB'), dtype=np.float64) for f in files], axis=0)
Image.fromarray((mean + 0.5).astype(np.uint8)).save(sys.argv[1] + '/mean.png')
print(sys.argv[1], len(files), 'frames')" "$OUT/craft/$1"
}
for d in r0_raw r1_raw r2_raw r0_indirect r1_indirect r2_indirect reference reference_diffuse; do mean_burst "$d"; done
grep -h "the sum restarts\|helio::compile" "$OUT"/craft/reference/log.txt | tail -6
echo "== voxel_craft: the whole frame (raw, 250) against the reference's (62)"
$COMPARE png "$OUT/craft/reference/mean.png" "$OUT/craft" \
    r0="$OUT/craft/r0_raw/mean.png" r1="$OUT/craft/r1_raw/mean.png" r2="$OUT/craft/r2_raw/mean.png"
echo "== voxel_craft: indirect diffuse (view 23) against the reference's (63)"
$COMPARE png "$OUT/craft/reference_diffuse/mean.png" "$OUT/craft" \
    r0_indirect="$OUT/craft/r0_indirect/mean.png" r1_indirect="$OUT/craft/r1_indirect/mean.png" \
    r2_indirect="$OUT/craft/r2_indirect/mean.png"
echo "== voxel_craft: linear, this build against the reference"
$COMPARE dump "$OUT/craft/linear_0/dump_shot_00" "$OUT/craft/linear"
echo "== voxel_craft: the reference's parts and the real-time views (linear mean luminance)"
$COMPARE parts total="$OUT/craft/linear_0/dump_shot_00" direct_22="$OUT/craft/linear_22/dump_shot_00" \
    indirect_23="$OUT/craft/linear_23/dump_shot_00" \
    visible="$OUT/craft/part_visible/dump_shot_00" direct="$OUT/craft/part_direct/dump_shot_00" \
    sky="$OUT/craft/part_sky/dump_shot_00" first="$OUT/craft/part_first/dump_shot_00" \
    later="$OUT/craft/part_later/dump_shot_00" albedo_4="$OUT/craft/part_albedo/dump_shot_00"
fi
