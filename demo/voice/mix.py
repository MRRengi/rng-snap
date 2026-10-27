"""Puts the ElevenLabs voice lines and music under the edited videos.

python3 demo/voice/mix.py <results-dir> <voice-dir> <video-dir>
Writes <name>-voice.mp4 next to each rengi-snap-*.mp4.
"""
import json, subprocess, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import edit  # noqa: E402

res, vdir, vid = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
cfg = json.loads(Path(__file__).with_name("lines.json").read_text())
tk = edit.Take(res)
seg = edit.direction(tk)[0]
T0 = int(edit.FPS * edit.TITLE_S) / edit.FPS
footage = sum(int((b - a) / sp * edit.FPS / 1000) for a, b, sp in seg) / edit.FPS


def out(beat):
    t = T0
    for a, b, sp in seg:
        if beat <= a:
            break
        t += (min(beat, b) - a) / sp / 1000
    return t


def dur(p):
    return float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(p)], capture_output=True, text=True).stdout)


T = tk.t
cap = lambda s: T("caption", s)
anchors = {
    "title": 0.15,
    "freeze": T0 + 0.05,
    "finds": out(cap("It finds")) + 0.1,
    "wheel": out(cap("Mouse wheel")),
    "ctrl": out(cap("Hold Ctrl")),
    "copied": out(T("key", "Enter")) - 0.25,
    "editor": out(cap("Need more")),
    "blur": out(cap("Blur")),
    "note": out(cap("Add a note")),
    "auto": out(cap("Every change")),
    "video": out(cap("Video too")),
    "mp4": out(cap("Saved as MP4")),
    "gif": out(cap("One click")),
    "end": T0 + footage + 0.45,
}
total = T0 + footage + edit.END_S
starts, end_prev = {}, 0.0
for ln in cfg["lines"]:
    i = ln["id"]
    s = max(anchors[i], end_prev + 0.18)
    d = dur(vdir / f"{i}.mp3")
    starts[i] = s
    end_prev = s + d
    print(f"{i:8s} {s:6.2f} -> {s + d:6.2f}  (anchor {anchors[i]:6.2f})")
print(f"total {total:.2f}")

# Voice track: every line delayed to its start, mixed without level normalisation.
ins, parts = [], []
for k, ln in enumerate(cfg["lines"]):
    ins += ["-i", str(vdir / f"{ln['id']}.mp3")]
    parts.append(f"[{k}:a]aresample=48000,aformat=channel_layouts=stereo,adelay={int(starts[ln['id']] * 1000)}:all=1[v{k}]")
n = len(cfg["lines"])
mi = n
ins += ["-i", str(vdir / "music.mp3")]
fc = ";".join(parts)
fc += ";" + "".join(f"[v{k}]" for k in range(n)) + f"amix=inputs={n}:normalize=0:duration=longest,apad,atrim=0:{total:.3f},volume=1.0[voice]"
fc += ";[voice]asplit=2[vo][sc]"
fc += f";[{mi}:a]aresample=48000,aformat=channel_layouts=stereo,atrim=0:{total:.3f},afade=t=in:d=1.2,afade=t=out:st={total - 2.5:.3f}:d=2.5,volume=0.32[mus]"
fc += ";[mus][sc]sidechaincompress=threshold=0.02:ratio=6:attack=40:release=450:makeup=1[duck]"
fc += ";[vo][duck]amix=inputs=2:normalize=0,loudnorm=I=-14:TP=-1.5:LRA=11[mix]"
mixwav = vid / "mix.wav"
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", *ins, "-filter_complex", fc, "-map", "[mix]", "-ar", "48000", str(mixwav)], check=True)
for v in sorted(vid.glob("rengi-snap-*.mp4")):
    if v.stem.endswith("-voice"):
        continue
    o = v.with_name(v.stem + "-voice.mp4")
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(v), "-i", str(mixwav), "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart", str(o)], check=True)
    print("wrote", o)
