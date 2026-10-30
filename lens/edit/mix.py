"""Narration (with inserted pauses) + ducked music bed + sound effects under the videos.

python3 mix.py <narration.mp3> <music.mp3> <sfx-dir> <video-dir>
"""
import json, subprocess, sys
from pathlib import Path

import numpy as np

narr, music, sfxdir, vid = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
SR = 48000


def run(*a):
    subprocess.run(list(a), check=True)


def load(p):
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(p), "-f", "f32le", "-ac", "2", "-ar", str(SR), "-"], capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.float32).reshape(-1, 2)


# 1. Narration with the pauses the edit asked for.
ins = json.loads((vid / "inserts.json").read_text()) if (vid / "inserts.json").exists() else []
v = load(narr)
parts, prev = [], 0
for t, d in sorted(ins):
    k = int(t * SR)
    parts += [v[prev:k], np.zeros((int(d * SR), 2), np.float32)]
    prev = k
parts.append(v[prev:])
voice = np.concatenate(parts)

# 2. Sound effects on the output timeline.
fx = json.loads((vid / "sfx.json").read_text())
cache = {}
total = max(len(voice) / SR, max(t for t, _, _ in fx) + 3) + 5
sfx = np.zeros((int(total * SR), 2), np.float32)
for t, name, g in fx:
    if name not in cache:
        a = load(sfxdir / f"sfx-{name}.mp3")
        cache[name] = a / max(1e-6, np.abs(a).max()) * 0.9  # every effect peak-normalised; the gain sets its level
    a = cache[name] * g
    k = max(0, int(t * SR))
    n = min(len(a), len(sfx) - k)
    sfx[k:k + n] += a[:n]

for arr, name in ((voice, "voice.wav"), (sfx, "sfx.wav")):
    tmp = vid / (name + ".raw")
    tmp.write_bytes(arr.astype(np.float32).tobytes())
    run("ffmpeg", "-y", "-loglevel", "error", "-f", "f32le", "-ar", str(SR), "-ac", "2", "-i", str(tmp), str(vid / name))
    tmp.unlink()

for vf in sorted(vid.glob("lens-*.mp4")):
    if vf.stem.endswith("-voice"):
        continue
    dur = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(vf)], capture_output=True, text=True).stdout)
    fc = (
        f"[1:a]apad,atrim=0:{dur:.3f}[nv];[nv]asplit=2[vo][sc];"
        f"[2:a]aresample={SR},aformat=channel_layouts=stereo,apad,atrim=0:{dur:.3f},afade=t=out:st={dur - 2.2:.3f}:d=2.2,volume=0.34[mus];"
        "[mus][sc]sidechaincompress=threshold=0.02:ratio=5:attack=40:release=500:makeup=1[duck];"
        f"[3:a]apad,atrim=0:{dur:.3f},volume=0.9[fx];"
        "[vo][duck][fx]amix=inputs=3:normalize=0,loudnorm=I=-14:TP=-1.5:LRA=11[mix]"
    )
    o = vf.with_name(vf.stem + "-voice.mp4")
    run("ffmpeg", "-y", "-loglevel", "error", "-i", str(vf), "-i", str(vid / "voice.wav"), "-i", str(music), "-i", str(vid / "sfx.wav"),
        "-filter_complex", fc, "-map", "0:v", "-map", "[mix]", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-ar", str(SR), "-movflags", "+faststart", str(o))
    print("wrote", o)
