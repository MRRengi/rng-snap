"""Keynote voice-over: one clip per line on eleven_v4 with request stitching, so the intonation flows like one read."""
import json, os, sys
from pathlib import Path
import requests
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
H = {"xi-api-key": os.environ["ELEVENLABS_API_KEY"]}
VOICE = "wUc0HMbyLmMJH0m2egfS"
LINES = json.loads(Path(__file__).with_name("keynote_lines.json").read_text())
log = []
for i, ln in enumerate(LINES):
    body = {"text": ln["text"], "model_id": "eleven_v4", "voice_settings": {"stability": 0.55, "similarity_boost": 0.75},
            "previous_text": " ".join(l["text"] for l in LINES[:i])[-600:] or None,
            "next_text": " ".join(l["text"] for l in LINES[i + 1:])[:600] or None}
    r = requests.post(f"https://api.elevenlabs.io/v1/text-to-speech/{VOICE}?output_format=mp3_44100_192", headers=H, json=body, timeout=300)
    if r.status_code != 200:
        body.pop("previous_text"); body.pop("next_text")
        r = requests.post(f"https://api.elevenlabs.io/v1/text-to-speech/{VOICE}?output_format=mp3_44100_192", headers=H, json=body, timeout=300)
    if r.status_code == 200:
        (out / f"{ln['id']}.mp3").write_bytes(r.content); log.append(f"ok {ln['id']}")
    else:
        log.append(f"FAIL {ln['id']} {r.status_code} {r.text[:300]}")
(out / "log.txt").write_text("\n".join(log)); print("\n".join(log))
