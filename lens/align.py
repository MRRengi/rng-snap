"""Word timestamps for the screencast voice-over via ElevenLabs forced alignment → results/align/words.json"""
import json, os, sys, requests
src, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
lines = json.load(open('lens/screencast_lines.json'))
items = lines.items() if isinstance(lines, dict) else [(l['id'], l['text']) for l in lines]
res = {}
for lid, text in items:
    with open(os.path.join(src, f'{lid}.mp3'), 'rb') as f:
        r = requests.post('https://api.elevenlabs.io/v1/forced-alignment', headers={'xi-api-key': os.environ['ELEVENLABS_API_KEY']},
                          files={'file': (f'{lid}.mp3', f, 'audio/mpeg')}, data={'text': text}, timeout=120)
    print(lid, r.status_code, flush=True)
    r.raise_for_status()
    res[lid] = [[w['text'], w['start'], w['end']] for w in r.json()['words'] if w['text'].strip()]
json.dump(res, open(os.path.join(out, 'words.json'), 'w'), indent=1)
