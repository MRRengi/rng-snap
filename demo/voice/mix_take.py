"""Lays a one-take narration (starting at 0) and a ducked music bed under the videos.

python3 demo/voice/mix_take.py <narration.mp3> <music.mp3> <video-dir>
Silence listed in <video-dir>/inserts.json ([[at_s, dur_s], ...]) is added to
the narration at those points, in the pauses between sentences.
Writes <name>-voice.mp4 next to each rengi-snap-*.mp4.
"""
import subprocess, sys
from pathlib import Path

import json

narr, music, vid = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
ins = json.loads((vid / "inserts.json").read_text()) if (vid / "inserts.json").exists() else []
edited = vid / "narration-edited.wav"
parts, fc, prev = [], [], 0.0
for k, (t, d) in enumerate(sorted(ins)):
    fc.append(f"[0:a]atrim={prev:.4f}:{t:.4f},asetpts=PTS-STARTPTS[p{k}]")
    fc.append(f"anullsrc=r=44100:cl=mono,atrim=0:{d:.4f}[s{k}]")
    parts += [f"[p{k}]", f"[s{k}]"]
    prev = t
fc.append(f"[0:a]atrim={prev:.4f},asetpts=PTS-STARTPTS[tail]")
parts.append("[tail]")
fc.append("".join(parts) + f"concat=n={len(parts)}:v=0:a=1[out]")
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(narr), "-filter_complex", ";".join(fc), "-map", "[out]", "-ac", "1", "-ar", "44100", str(edited)], check=True)
narr = edited
for v in sorted(vid.glob("rengi-snap-*.mp4")):
    if v.stem.endswith("-voice"):
        continue
    total = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(v)], capture_output=True, text=True).stdout)
    fc = (
        f"[1:a]aresample=48000,aformat=channel_layouts=stereo,apad,atrim=0:{total:.3f}[nv];[nv]asplit=2[vo][sc];"
        f"[2:a]aresample=48000,aformat=channel_layouts=stereo,atrim=0:{total:.3f},afade=t=in:d=1.0,afade=t=out:st={total - 2.5:.3f}:d=2.5,volume=0.30[mus];"
        "[mus][sc]sidechaincompress=threshold=0.02:ratio=5:attack=60:release=600:makeup=1[duck];"
        "[vo][duck]amix=inputs=2:normalize=0,loudnorm=I=-14:TP=-1.5:LRA=11[mix]"
    )
    o = v.with_name(v.stem + "-voice.mp4")
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(v), "-i", str(narr), "-i", str(music), "-filter_complex", fc,
                    "-map", "0:v", "-map", "[mix]", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-movflags", "+faststart", str(o)], check=True)
    print("wrote", o)
