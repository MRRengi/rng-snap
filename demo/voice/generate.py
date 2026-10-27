"""Generates the voice lines (and background music) with ElevenLabs. Runs on CI.

python demo/voice/generate.py <out-dir>   (needs ELEVENLABS_API_KEY)
"""
import json, os, sys, urllib.request, urllib.error
from pathlib import Path

cfg = json.loads(Path(__file__).with_name("lines.json").read_text())
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
key = os.environ["ELEVENLABS_API_KEY"]
log = []


def post(url, body, dest):
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"xi-api-key": key, "Content-Type": "application/json", "Accept": "audio/mpeg"})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            dest.write_bytes(r.read())
        log.append(f"ok {dest.name} {dest.stat().st_size}")
        return True
    except urllib.error.HTTPError as e:
        log.append(f"fail {dest.name} {e.code} {e.read()[:400]!r}")
        return False


lines = cfg["lines"]
for i, ln in enumerate(lines):
    body = {
        "text": ln["text"],
        "model_id": cfg["model_id"],
        "voice_settings": cfg["settings"],
        "previous_text": lines[i - 1]["text"] if i else None,
        "next_text": lines[i + 1]["text"] if i + 1 < len(lines) else None,
    }
    post(f"https://api.elevenlabs.io/v1/text-to-speech/{cfg['voice_id']}?output_format=mp3_44100_128", body, out / f"{ln['id']}.mp3")

post("https://api.elevenlabs.io/v1/music?output_format=mp3_44100_128", {"prompt": cfg["music_prompt"], "music_length_ms": cfg["music_ms"]}, out / "music.mp3")
(out / "voice-log.txt").write_text("\n".join(log))
print("\n".join(log))
