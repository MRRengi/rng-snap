"""Records the Lens narration in one take per try with the rng narrator voice, word timestamps and a music bed.

python lens/narrate.py <out-dir>   (needs ELEVENLABS_API_KEY)
"""
import json, os, sys
from pathlib import Path

import requests

cfg = json.loads(Path(__file__).with_name("narration.json").read_text())
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
H = {"xi-api-key": os.environ["ELEVENLABS_API_KEY"]}
API = "https://api.elevenlabs.io"
log = []


def note(s):
    print(s, flush=True)
    log.append(s)


def ok(r, what):
    if r.status_code != 200:
        note(f"FAIL {what}: {r.status_code} {r.text[:500]}")
        return False
    return True


t = cfg["tts"]
for i in range(cfg["takes"]):
    r = requests.post(f"{API}/v1/text-to-speech/{cfg['voice_id']}?output_format=mp3_44100_192", headers=H, timeout=600, json={
        "text": cfg["text"], "model_id": t["model_id"], "voice_settings": t["settings"], "apply_text_normalization": "on"})
    if not ok(r, f"tts {i}"):
        r = requests.post(f"{API}/v1/text-to-speech/{cfg['voice_id']}?output_format=mp3_44100_128", headers=H, timeout=600, json={
            "text": cfg["text"], "model_id": t["model_id"], "voice_settings": t["settings"], "apply_text_normalization": "on"})
        if not ok(r, f"tts {i} 128"):
            continue
    f = out / f"narration-{i}.mp3"
    f.write_bytes(r.content)
    note(f"narration {i}: {len(r.content)} bytes")
    with open(f, "rb") as fh:
        s = requests.post(f"{API}/v1/speech-to-text", headers=H, timeout=600, files={"file": fh},
                          data={"model_id": "scribe_v2", "timestamps_granularity": "word", "tag_audio_events": "false"})
    if ok(s, f"stt {i}"):
        (out / f"words-{i}.json").write_text(json.dumps(s.json()["words"]))
        note(f"stt {i}: {s.json()['text']}")

r = requests.post(f"{API}/v1/music?output_format=mp3_44100_128", headers=H, timeout=600, json={"prompt": cfg["music_prompt"], "music_length_ms": cfg["music_ms"]})
if ok(r, "music"):
    (out / "music.mp3").write_bytes(r.content)
    note("music ok")
(out / "voice-log.txt").write_text("\n".join(log))
