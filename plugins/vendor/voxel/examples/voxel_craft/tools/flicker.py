import sys, glob, numpy as np
from PIL import Image
d = sys.argv[1]
poses = sorted({p.split('shot_')[1][:2] for p in glob.glob(d + '/shot_*_*.png')})
for pose in poses:
    files = sorted(glob.glob(f'{d}/shot_{pose}_*.png'))
    fr = [np.asarray(Image.open(f).convert('RGB'), dtype=np.float32) / 255.0 for f in files]
    lum = [f @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32) for f in fr]
    diffs = [np.abs(lum[i + 1] - lum[i]) for i in range(len(lum) - 1)]
    mean = np.mean([x.mean() for x in diffs])
    big = np.mean([(x > 0.02).mean() for x in diffs])
    # Per-pixel temporal std over the burst.
    std = np.std(np.stack(lum), axis=0)
    # Frames where much of the image changes at once (over 5% of the pixels
    # by more than 2%): not flicker but an event, such as the NVIDIA lost
    # G-buffer writes (8x8-pixel blocks showing the background, bug
    # 6939141), which a burst should be retaken over.
    events = [i + 1 for i, x in enumerate(diffs) if (x > 0.02).mean() > 0.05]
    print(f"pose {pose}: mean |dL| {mean:.5f}, pixels changing >2% {big*100:.2f}%, temporal std mean {std.mean():.5f}, p99 {np.percentile(std, 99):.4f}, event frames {events}")
    np.save(f'{d}/std_{pose}.npy', std)
