#!/bin/bash
# Reruns the comparisons against the reference path tracer
# (rendering.md section 4, "Direct light samples and the reference"):
#
#   1. indoor_cathedral_gi: the real-time image and its indirect diffuse
#      against the converged reference (linear dumps and final images), and
#      the GPU cost of GI and of a reference frame, the build before
#      (BASE_CATHEDRAL) interleaved with this one.
#   2. voxel_craft, the first pose of the tour: the reference's final image
#      and indirect diffuse against r1's and r0's captures (or fresh runs of
#      the kept executables).
#
# Run from a Pulsar-Native checkout after building:
#   cargo build --release -p gi-examples --bin indoor_cathedral_gi \
#       -p helio-voxel-examples --bin voxel_craft
# Every GPU run goes through the shared GPU lock. Rerun at integration, when
# the meshes' and the terrain's Full surfaces land: the reference sees what
# the tracers answer.
#
# Environment: TARGET (cargo target dir), OUT (results), BASE_CATHEDRAL,
# R0_CRAFT, R1_CRAFT (kept executables), FRAMES (the reference's frames),
# PART (cathedral, craft or all).
set -u
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TARGET=${TARGET:-C:/t/pulsar-r2d}
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
cathedral ./cathedral_new.exe still_realtime HELIO_STILL=1 HELIO_CAPTURE_FRAME=240
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
if [ "$PART" != cathedral ]; then
cp "$TARGET/release/voxel_craft.exe" "$OUT/craft/voxel_craft_new.exe"
cd "$OUT/craft" || exit 1
craft() { # exe dir [env...]
    local exe=$1 dir=$2
    shift 2
    rm -rf "$OUT/craft/$dir"
    mkdir -p "$OUT/craft/$dir"
    env "$@" VOXEL_CRAFT_SHOTS="$OUT/craft/$dir" VOXEL_CRAFT_POSES=1 VOXEL_CRAFT_NO_CLOUDS=1 \
        RUST_LOG=info,helio_pass_reference=debug \
        bash "$GPU" "$WHO" "$exe" > "$OUT/craft/$dir/log.txt" 2>&1
}
craft "$R0_CRAFT" r0 VOXEL_CRAFT_BURST=8 VOXEL_CRAFT_WAIT=1300 VOXEL_CRAFT_WAIT_MAX=8000
craft "$R1_CRAFT" r1 VOXEL_CRAFT_BURST=8 VOXEL_CRAFT_WAIT=1300 VOXEL_CRAFT_WAIT_MAX=8000
craft "$R0_CRAFT" r0_indirect VOXEL_CRAFT_BURST=8 VOXEL_CRAFT_WAIT=1300 VOXEL_CRAFT_WAIT_MAX=8000 VOXEL_CRAFT_DEBUG=23
craft "$R1_CRAFT" r1_indirect VOXEL_CRAFT_BURST=8 VOXEL_CRAFT_WAIT=1300 VOXEL_CRAFT_WAIT_MAX=8000 VOXEL_CRAFT_DEBUG=23
craft ./voxel_craft_new.exe reference VOXEL_CRAFT_BURST=1 VOXEL_CRAFT_WAIT="$FRAMES" \
    VOXEL_CRAFT_WAIT_MAX=$((FRAMES * 3)) VOXEL_CRAFT_DEBUG=62
craft ./voxel_craft_new.exe reference_diffuse VOXEL_CRAFT_BURST=1 VOXEL_CRAFT_WAIT="$FRAMES" \
    VOXEL_CRAFT_WAIT_MAX=$((FRAMES * 3)) VOXEL_CRAFT_DEBUG=63
mean_burst() { # dir -> dir/mean.png
    python -c "
import sys, glob, numpy as np
from PIL import Image
files = sorted(glob.glob(sys.argv[1] + '/shot_00*.png'))
mean = np.mean([np.array(Image.open(f).convert('RGB'), dtype=np.float64) for f in files], axis=0)
Image.fromarray((mean + 0.5).astype(np.uint8)).save(sys.argv[1] + '/mean.png')
print(sys.argv[1], len(files), 'frames')" "$OUT/craft/$1"
}
for d in r0 r1 r0_indirect r1_indirect reference reference_diffuse; do mean_burst "$d"; done
grep -h "the sum restarts" "$OUT"/craft/reference/log.txt | tail -3
echo "== voxel_craft: final images against the reference's"
$COMPARE png "$OUT/craft/reference/mean.png" "$OUT/craft" \
    r0="$OUT/craft/r0/mean.png" r1="$OUT/craft/r1/mean.png"
echo "== voxel_craft: indirect diffuse (view 23) against the reference's (63)"
$COMPARE png "$OUT/craft/reference_diffuse/mean.png" "$OUT/craft" \
    r0_indirect="$OUT/craft/r0_indirect/mean.png" r1_indirect="$OUT/craft/r1_indirect/mean.png"
fi
