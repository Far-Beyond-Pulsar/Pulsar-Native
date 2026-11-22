"""Compares real-time images against the reference path tracer's
(Helio's `helio-pass-reference`, rendering.md section 4, "Direct light
samples and the reference").

Two forms of input:

- Linear dumps (`<prefix>_reference.f32`, `_diffuse.f32`, `_realtime.f32`:
  `width`, `height` as little-endian u32, then RGBA f32 a pixel; the
  reference's are sums of paths with their count in alpha), written by a
  host at its capture (the GI examples' `HELIO_REFERENCE_DUMP`).
- Final images (PNG, 8-bit sRGB after the frame's tone map): a real-time
  capture against a capture of the reference's debug view through the same
  post chain (fixed exposure).

    python reference_compare.py dump <prefix> <out_dir>
    python reference_compare.py png <reference.png> <out_dir> <name=a.png> [<name=b.png> ...]
    python reference_compare.py parts <name=prefix> [<name=prefix> ...]

`parts`: for each linear dump, the mean luminance (linear) of the
reference's sum and of the real-time snapshot taken with it, over pixels
the reference covers: the reference's parts (`reference_parts.py`) against
the real-time image's views (direct 22, indirect 23, the whole frame).

Prints, for each image against the reference: the mean and 99th
percentile of the absolute error (luminance), the mean relative error where
the reference is lit, the ratio of mean luminances, and writes side-by-side
images and error maps into <out_dir>.
"""

import sys
import os

import numpy as np
from PIL import Image

LUMA = np.array([0.2126, 0.7152, 0.0722])


def load_dump(path):
    raw = open(path, "rb").read()
    w, h = np.frombuffer(raw[:8], dtype="<u4")
    data = np.frombuffer(raw[8:], dtype="<f4").reshape(h, w, 4)
    return data


def mean_of(sums):
    return sums[..., :3] / np.maximum(sums[..., 3:4], 1.0)


def to_srgb8(linear):
    x = linear / (1.0 + linear)
    x = np.where(x <= 0.0031308, 12.92 * x, 1.055 * np.power(np.maximum(x, 0), 1 / 2.4) - 0.055)
    return (np.clip(x, 0, 1) * 255 + 0.5).astype(np.uint8)


def from_srgb8(img):
    x = img.astype(np.float64) / 255.0
    return np.where(x <= 0.04045, x / 12.92, np.power((x + 0.055) / 1.055, 2.4))


def stats(name, image, reference, mask):
    li = image @ LUMA
    lr = reference @ LUMA
    err = np.abs(li - lr)[mask]
    rel_mask = mask & (lr > 1e-3 * max(lr[mask].mean(), 1e-9))
    rel = (np.abs(li - lr)[rel_mask] / lr[rel_mask])
    row = {
        "name": name,
        "mean_abs": float(err.mean()),
        "p99_abs": float(np.percentile(err, 99)),
        "mean_rel": float(rel.mean()),
        "p99_rel": float(np.percentile(rel, 99)),
        "luminance_ratio": float(li[mask].mean() / max(lr[mask].mean(), 1e-12)),
    }
    print(
        f"{name:>24}: mean |err| {row['mean_abs']:.5f}  p99 {row['p99_abs']:.5f}  "
        f"mean rel {row['mean_rel']:.4f}  p99 rel {row['p99_rel']:.4f}  "
        f"luminance ratio {row['luminance_ratio']:.4f}"
    )
    return row


def side_by_side(out, left, right, diff):
    Image.fromarray(np.concatenate([left, right], axis=1)).save(out + "_side_by_side.png")
    Image.fromarray(diff).save(out + "_diff_x8.png")


def run_dump(prefix, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    ref = load_dump(prefix + "_reference.f32")
    diffuse = load_dump(prefix + "_diffuse.f32")
    count = ref[..., 3]
    print(f"reference: {ref.shape[1]}x{ref.shape[0]}, paths a pixel {count.min():.0f}..{count.max():.0f}")
    mask = count > 0
    reference = mean_of(ref)
    base = os.path.join(out_dir, os.path.basename(prefix))
    Image.fromarray(to_srgb8(reference)).save(base + "_reference.png")
    Image.fromarray(to_srgb8(mean_of(diffuse))).save(base + "_reference_diffuse.png")
    rows = []
    path = prefix + "_realtime.f32"
    if os.path.exists(path):
        rt = load_dump(path)[..., :3]
        rows.append(stats("real-time (linear)", rt, reference, mask))
        diff = to_srgb8(np.abs(rt - reference) * 8.0)
        side_by_side(base + "_realtime_vs_reference", to_srgb8(rt), to_srgb8(reference), diff)
    return rows


def run_png(reference_png, out_dir, others):
    os.makedirs(out_dir, exist_ok=True)
    ref8 = np.array(Image.open(reference_png).convert("RGB"))
    reference = from_srgb8(ref8)
    mask = np.ones(reference.shape[:2], dtype=bool)
    rows = []
    for spec in others:
        name, path = spec.split("=", 1)
        img8 = np.array(Image.open(path).convert("RGB"))
        image = from_srgb8(img8)
        rows.append(stats(name, image, reference, mask))
        diff = (np.clip(np.abs(img8.astype(int) - ref8.astype(int)) * 8, 0, 255)).astype(np.uint8)
        side_by_side(os.path.join(out_dir, name + "_vs_reference"), img8, ref8, diff)
    return rows


def run_parts(specs):
    print(f"{'part':>12}  {'reference':>10}  {'real-time':>10}  paths a pixel")
    for spec in specs:
        name, prefix = spec.split("=", 1)
        ref = load_dump(prefix + "_reference.f32")
        mask = ref[..., 3] > 0
        reference = (mean_of(ref) @ LUMA)[mask].mean()
        realtime = float("nan")
        if os.path.exists(prefix + "_realtime.f32"):
            realtime = (load_dump(prefix + "_realtime.f32")[..., :3] @ LUMA)[mask].mean()
        count = ref[..., 3][mask]
        print(f"{name:>12}  {reference:10.5f}  {realtime:10.5f}  {count.min():.0f}..{count.max():.0f}")


if __name__ == "__main__":
    if len(sys.argv) >= 4 and sys.argv[1] == "dump":
        run_dump(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 3 and sys.argv[1] == "parts":
        run_parts(sys.argv[2:])
    elif len(sys.argv) >= 5 and sys.argv[1] == "png":
        run_png(sys.argv[2], sys.argv[3], sys.argv[4:])
    else:
        print(__doc__)
        sys.exit(2)
