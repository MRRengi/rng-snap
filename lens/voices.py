"""Casting: trending social-media creator voices from the Voice Library + designed ones, all reading the same lines on eleven_v4."""
import base64, json, os, sys
from pathlib import Path
import requests

out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
H = {"xi-api-key": os.environ["ELEVENLABS_API_KEY"]}
API = "https://api.elevenlabs.io"
TEXT = ("[excited] Okay, so my phone got so hot on a video call, I literally started cooking breakfast on it. [laughs] "
        "Any normal person would just buy a webcam. [pause] I opened an AI chat at two in the morning instead. "
        "Twelve coffees and one very confident AI later... [sighs] it actually works. Anyway, it's free and open source.")
log = []
def note(s):
    print(s, flush=True); log.append(s)

cast = []
for gender in ("male", "female"):
    r = requests.get(f"{API}/v1/shared-voices", headers=H, timeout=60, params={
        "page_size": 30, "category": "", "gender": gender, "age": "young", "language": "en", "accent": "american",
        "use_cases": "social_media", "sort": "trending"})
    if r.status_code != 200:
        note(f"FAIL search {gender}: {r.status_code} {r.text[:300]}"); continue
    voices = r.json().get("voices", [])
    note(f"{gender}: {len(voices)} found: " + ", ".join(v.get('name','?') for v in voices[:8]))
    for v in voices[:2]:
        a = requests.post(f"{API}/v1/voices/add/{v['public_owner_id']}/{v['voice_id']}", headers=H, timeout=60, json={"new_name": f"Lens creator {v['name']}"[:40]})
        if a.status_code == 200:
            cast.append((gender, v["name"], a.json()["voice_id"], v.get("description", "")[:160]))
        else:
            note(f"FAIL add {v['name']}: {a.status_code} {a.text[:200]}")

designs = {
    "male": "A real tech YouTuber in his late twenties, American, talking to his phone camera. Fast, playful and self-ironic, "
            "with natural laughs and little breaths, like telling friends a funny story. Bright, close microphone, not a radio voice.",
    "female": "A real tech content creator in her late twenties, American, talking straight to camera. Quick, witty and warm, "
              "a little sarcastic, natural laughs, like a funny story to friends. Bright, close microphone, not an announcer.",
}
for gender, desc in designs.items():
    r = requests.post(f"{API}/v1/text-to-voice/design?output_format=mp3_44100_128", headers=H, timeout=600, json={
        "voice_description": desc, "model_id": "eleven_ttv_v3", "text": TEXT.replace("[excited] ", "").replace("[laughs] ", "").replace("[pause] ", "").replace("[sighs] ", ""),
        "guidance_scale": 3.0})
    if r.status_code != 200:
        note(f"FAIL design {gender}: {r.status_code} {r.text[:300]}"); continue
    p = r.json()["previews"][0]
    c = requests.post(f"{API}/v1/text-to-voice", headers=H, timeout=120, json={"voice_name": f"Lens creator designed {gender}", "voice_description": desc, "generated_voice_id": p["generated_voice_id"]})
    if c.status_code == 200:
        cast.append((gender, f"designed {gender}", c.json()["voice_id"], desc[:120]))

meta = []
for i, (gender, name, vid, desc) in enumerate(cast):
    r = requests.post(f"{API}/v1/text-to-speech/{vid}?output_format=mp3_44100_192", headers=H, timeout=600, json={
        "text": TEXT, "model_id": "eleven_v4", "voice_settings": {"stability": 0.4, "similarity_boost": 0.75}})
    if r.status_code != 200:
        note(f"FAIL tts {name}: {r.status_code} {r.text[:300]}"); continue
    fn = f"{gender}-{i}-{''.join(ch for ch in name if ch.isalnum())[:20]}.mp3"
    (out / fn).write_bytes(r.content)
    meta.append({"file": fn, "gender": gender, "name": name, "voice_id": vid, "desc": desc})
    note(f"ok {fn}")
(out / "cast.json").write_text(json.dumps(meta, indent=1))
(out / "log.txt").write_text("\n".join(log))
