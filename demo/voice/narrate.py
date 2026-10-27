"""Designs a narrator voice and records the whole narration in one take. Runs on CI.

python demo/voice/narrate.py <out-dir>   (needs ELEVENLABS_API_KEY)

Writes design-<n>.mp3 (previews), narration-<n>.mp3 (full take per voice),
words-<n>.json (Scribe word timestamps), music.mp3 and voices.json.
"""
import base64, json, os, sys
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


# 1. Voice design: previews of a new voice from the description, speaking our own text.
d = cfg["design"]
sample = cfg["text"].replace("\n\n", " ")[:1000]
r = requests.post(f"{API}/v1/text-to-voice/design?output_format=mp3_44100_128", headers=H, timeout=600, json={
    "voice_description": d["voice_description"], "model_id": d["model_id"], "text": sample,
    "guidance_scale": d["guidance_scale"], "loudness": d["loudness"]})
voices = []
if ok(r, "design"):
    for i, p in enumerate(r.json()["previews"]):
        (out / f"design-{i}.mp3").write_bytes(base64.b64decode(p["audio_base_64"]))
        c = requests.post(f"{API}/v1/text-to-voice", headers=H, timeout=120, json={
            "voice_name": f"Snap narrator {chr(65 + i)}", "voice_description": d["voice_description"], "generated_voice_id": p["generated_voice_id"]})
        if ok(c, f"save voice {i}"):
            voices.append(c.json()["voice_id"])
            note(f"voice {i}: {voices[-1]}")
(out / "voices.json").write_text(json.dumps(voices))

# 2. The whole narration in one take per voice, then word timestamps.
t = cfg["tts"]
for i, vid in enumerate(voices):
    r = requests.post(f"{API}/v1/text-to-speech/{vid}?output_format=mp3_44100_192", headers=H, timeout=600, json={
        "text": cfg["text"], "model_id": t["model_id"], "voice_settings": t["settings"], "apply_text_normalization": "on"})
    if not ok(r, f"tts {i}"):
        r = requests.post(f"{API}/v1/text-to-speech/{vid}?output_format=mp3_44100_128", headers=H, timeout=600, json={
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

# 3. Music bed.
r = requests.post(f"{API}/v1/music?output_format=mp3_44100_128", headers=H, timeout=600, json={"prompt": cfg["music_prompt"], "music_length_ms": cfg["music_ms"]})
if ok(r, "music"):
    (out / "music.mp3").write_bytes(r.content)
    note("music ok")
(out / "voice-log.txt").write_text("\n".join(log))
