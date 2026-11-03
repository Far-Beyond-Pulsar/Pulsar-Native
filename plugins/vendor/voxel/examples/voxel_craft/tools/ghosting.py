"""Ghosting in a voxel_craft flight (`VOXEL_CRAFT_FLIGHT`): each moving frame
`flight_NNN.png` against `still_NNN.png`, the same camera held still until
converged. History that trails behind what moves shows as light (or dark)
left where the still frame has none.

    python ghosting.py <dir> [<dir> ...]

Per pair: mean |dL| (display luminance, 0..1), the share of pixels brighter
than the still by over 0.1 (`trail`) and darker by over 0.1 (`dark`), and the
mean excess over the trail pixels. Writes `ghost_NNN.png` next to each pair:
the still in grey, brighter in red, darker in blue (x4).
"""
import sys, glob, os
import numpy as np
from PIL import Image

LUMA = np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)


def load(path):
    return np.asarray(Image.open(path).convert('RGB'), dtype=np.float32) / 255.0


for d in sys.argv[1:]:
    rows = []
    for still_path in sorted(glob.glob(os.path.join(d, 'still_*.png'))):
        n = still_path[-7:-4]
        flight_path = os.path.join(d, f'flight_{n}.png')
        if not os.path.exists(flight_path):
            continue
        still, flight = load(still_path), load(flight_path)
        ls, lf = still @ LUMA, flight @ LUMA
        diff = lf - ls
        trail = diff > 0.1
        dark = diff < -0.1
        excess = float(diff[trail].mean()) if trail.any() else 0.0
        rows.append((n, np.abs(diff).mean(), trail.mean(), dark.mean(), excess))
        grey = np.repeat(ls[..., None] * 0.5, 3, axis=2)
        grey[..., 0] += np.clip(diff * 4.0, 0.0, 1.0)
        grey[..., 2] += np.clip(-diff * 4.0, 0.0, 1.0)
        Image.fromarray((np.clip(grey, 0.0, 1.0) * 255).astype(np.uint8)).save(os.path.join(d, f'ghost_{n}.png'))
    print(d)
    for n, mean, trail, dark, excess in rows:
        print(f"  frame {n}: mean |dL| {mean:.4f}, trail {trail * 100:.2f}%, dark {dark * 100:.2f}%, trail excess {excess:.3f}")
    if rows:
        moving = rows[1:]  # frame 0 is still itself
        if moving:
            print(f"  moving frames: mean |dL| {np.mean([r[1] for r in moving]):.4f}, trail {np.mean([r[2] for r in moving]) * 100:.2f}%, dark {np.mean([r[3] for r in moving]) * 100:.2f}%")
