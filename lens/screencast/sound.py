"""Lens living screencast: score + foley + VO → mix.wav, all timed from events.json.
Two resolutions: a hi-fi band (Salamander piano, jazz double bass, MuldjordKit, vibraphone, glockenspiel) is the world;
a band-limited square wave is Pix. Intro piano only → band enters on chapter 01 → fills at seams → chapter 04 drops the drums,
and when the camera pauses the band goes "to sleep" (low-passed, quieter) → the notification wakes it → ending chord + Pix's motif."""
import sys, os, json, numpy as np, soundfile as sf, soxr
ROOT = os.path.expanduser('~/lemo-opuscar'); sys.path.insert(0, ROOT)
from core.audio.sfx import SR, bp, lp, hp, noise, add, compress, limit, click as sclick
from core.audio import sampler as S
HERE = os.path.dirname(os.path.abspath(__file__))
EVJ = json.load(open(os.path.join(HERE, 'events.json'))); E = EVJ['ev']; DUR = EVJ['dur']
rng = np.random.default_rng(5); S.seed(5)
NS = int((DUR + 1.5) * SR); B = .6
def tt(d): return np.arange(int(round(d * SR))) / SR
def db(x): return 10 ** (x / 20)
def first(tp): return next(e['t'] for e in E if e['type'] == tp)
def all_(tp): return [e for e in E if e['type'] == tp]
hz = S.hz
def pulse(f, d, duty=.25, vol=1., att=.003, rel=.05, slide=None):
    t = tt(d); fr = np.full(len(t), float(f)) if slide is None else f * (slide ** (t / d))
    ph = np.cumsum(fr) / SR; x = np.zeros(len(t))
    for k in range(1, 24):
        m = fr * k < 16000; x += m * (np.sin(np.pi * k * duty) / k) * np.cos(2 * np.pi * k * (ph - duty / 2))
    e = np.minimum(1, t / att) * np.minimum(1, (d - t) / rel).clip(0)
    return (x * e * vol * .5).astype(np.float32)
def crush(x, bits=5): q = 2 ** bits; return (np.round(x * q) / q).astype(np.float32)
def nz(d, lo, hi, vol=1., tau=None):
    x = bp(rng.standard_normal(int(d * SR)), lo, hi) * vol
    if tau: x *= np.exp(-tt(d)[:len(x)] / tau)
    return x.astype(np.float32)

W1, W2, W3, W4 = [e['t'] for e in all_('wipe')]
T_MORPH, T_LIVE, T_BRB, T_TOAST, T_OUT, T_END = first('morph'), first('stream') + .32, first('brb'), first('toast'), [e['t'] for e in all_('whoosh') if e.get('d') == 0][0], first('hit')
T_RES = round((T_TOAST + .2) / B) * B
LAND0 = sorted(e['t'] for e in all_('land'))[0]

PROG = [('D2', ['F#3', 'A3', 'C#4', 'E4']), ('B1', ['D3', 'F#3', 'A3', 'C#4']), ('G1', ['B3', 'D4', 'F#4', 'A4']), ('A1', ['E3', 'A3', 'B3', 'D4'])]
def chord(t): return PROG[int(np.floor(t / (4 * B) + 1e-6)) % 4]
EVN = []
def N(t, inst, p, d, v, pan=0.): EVN.append((t, inst, p, d, v, pan))

# intro: soft Dmaj9 arpeggio on piano (eighths), vibes on the brand
ARP = ['D4', 'A4', 'E5', 'F#5', 'C#5', 'A4']
t = .5; i = 0
while t < W1 - .01: N(t, 'piano', ARP[i % 6], .6, .22 + .12 * min(1, t / 4), -.15); t += B / 2; i += 1
N(.9, 'vibraphone', 'F#4', 3, .35, .2); N(.9, 'vibraphone', 'A4', 3, .3, .25)
for k in range(8): tk = W1 - 1.2 + k * .15; N(tk, 'drum_kit', 'snare_1', .15, .15 + .4 * k / 8, .1)

def groove(t0, t1, drums=True, lite=False):
    for n in range(int(round(t0 / B)), int(round(t1 / B))):
        t = n * B; bi = n % 4; root, ch = chord(t + 1e-3)
        if drums:
            if bi in (0, 2) and not lite: N(t, 'drum_kit', 'kick_drum_left', .3, .7, 0)
            if bi == 2 and not lite: N(t + B * .75, 'drum_kit', 'kick_drum_left', .3, .4, 0)
            if bi in (1, 3): N(t, 'drum_kit', 'snare_2', .3, .3 if not lite else .2, .05); N(t, 'claps', None, .3, .3, .08)
            for h in (0, .5): N(t + h * B, 'drum_kit', 'hi_hat_closed', .1, .26 if h == 0 else .16, .3)
            N(t + .75 * B, 'shaker', None, .12, .18, -.35)
        N(t, 'jazz_bass', root, .32, .62, 0)
        if bi == 3: N(t + B / 2, 'jazz_bass', S.name(S.midi(root) + 7), .22, .4, 0)
        if bi == 0: [N(t + j * .012, 'piano', p, 2.0, .34, -.15) for j, p in enumerate(ch)]
        elif bi == 2: [N(t + B / 2, 'piano', p, .25, .24, -.15) for p in ch[1:]]
groove(W1, W4)
for w in (W2, W3, W4):
    for k, tm in enumerate(['tom_1', 'tom_2', 'tom_3', 'tom_4']): N(w - B + k * B / 4, 'drum_kit', tm, .3, .4 + .08 * k, (k - 1.5) * .3)
    N(w, 'drum_kit', 'crash_left', 2.0, .45, -.2)
N(W1, 'drum_kit', 'crash_right', 2.2, .5, .2); N(W1, 'drum_kit', 'kick_drum_left', .4, .9, 0)
groove(W4, T_TOAST, drums=False)            # chapter 4: band without drums
# wake up: crash, light groove to the end card
N(T_RES, 'drum_kit', 'crash_left', 2.4, .55, -.2); groove(T_RES, T_OUT, lite=True)
# ending: one Dmaj9 chord, vibes, glockenspiel arpeggio
N(T_END, 'jazz_bass', 'D1', 3.5, .7, 0)
for j, p in enumerate(['D2', 'A2', 'F#3', 'C#4', 'E4', 'A4']): N(T_END + j * .03, 'piano', p, 3.8, .45, -.1 + j * .05)
for j, p in enumerate(['D6', 'F#6', 'A6', 'C#7', 'E7']): N(T_END + .2 + j * B / 4, 'glockenspiel', p, 1.5, .35, .3)
N(T_END, 'vibraphone_bowed', 'A4', 3.4, .3, .2); N(T_END, 'vibraphone_bowed', 'F#4', 3.4, .3, -.2)
band = np.zeros((NS, 2), np.float32); bm = S.render(EVN, dur=DUR + 1.5); band[:len(bm)] += bm[:NS]
# sleep: when the camera pauses the band dips under water; the notification wakes it
tn = np.arange(NS) / SR
def ramp(a, b): return np.clip((tn - a) / (b - a), 0, 1)
night = ramp(T_BRB, T_BRB + .6) * (1 - ramp(T_TOAST, T_TOAST + .15))
dark = np.stack([lp(band[:, c], 650, 2) for c in (0, 1)], 1).astype(np.float32) * .9
band = band * (1 - night[:, None]) + dark * night[:, None]

# pixel layer: Pix's motif
chip = np.zeros((NS, 2), np.float32)
MOTIF = [('A5', 0, .5), ('D6', .5, .5), ('F#6', 1, .5), ('E6', 1.5, 1), ('A6', 2.5, 1.5)]
def motif(t0, vol=.3, pan=.15, glock=True):
    for p, o, d in MOTIF:
        ts = t0 + o * B / 2; add(chip, pulse(hz(p), d * B / 2 * .9, .25, vol), ts, 1, pan)
        if glock: add(band, S.note('glockenspiel', p, d * B / 2, .45), ts, .45, pan)
motif(LAND0 + .05, .26)
motif(T_END + 1.4, .3)
for k, p in enumerate(['D4', 'F#4', 'A4', 'D5', 'F#5', 'A5', 'D6']): add(chip, pulse(hz(p), .08, .125, .24), T_MORPH + k * .07, 1, (k - 3) * .12)
for k, p in enumerate(['A5', 'D6', 'F#6', 'A6']): add(chip, pulse(hz(p), .07, .25, .2), T_LIVE + k * .06, 1, .2)
# lullaby while asleep: two soft notes per zz
for e in all_('zz'): add(chip, pulse(hz('F#5'), .18, .5, .08), e['t'], 1, .3); add(chip, pulse(hz('D5'), .3, .5, .07), e['t'] + .22, 1, .3)
music = S.room(band + chip, size=.35, mix=.12)

# ═════════ foley
fx = np.zeros((NS, 2), np.float32)
def key(v=.8):
    d = .1; t = tt(d); th = np.sin(2 * np.pi * (160 + rng.uniform(-15, 15)) * t) * np.exp(-t / .02) * .6
    ck = nz(d, 2500, 7000, .5, .006) + nz(d, 900, 2200, .35, .014); return ((th + ck[:len(th)]) * v).astype(np.float32)
def tick(v=.5, f=2600): t = tt(.05); return (np.sin(2 * np.pi * f * t) * np.exp(-t / .008) * v + nz(.05, 3000, 8000, .15 * v, .004)[:len(t)]).astype(np.float32)
def pop(v=.6, f0=520, f1=980): t = tt(.12); f = f0 * (f1 / f0) ** np.minimum(1, t / .05); return (np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / .035) * v).astype(np.float32)
def swish(d=.35, v=.5, lo=400, hi=4000):
    x = rng.standard_normal(int(d * SR)); t = tt(d)[:len(x)]; return (bp(x, lo, hi) * np.sin(np.pi * t / d) ** 2 * v).astype(np.float32)
def boom(v=1., f0=90, d=.9): v *= .45; t = tt(d); f = f0 * np.exp(-t * 2.2) + 32; return (np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / .3) * v).astype(np.float32)
def chime(notes, t0, v=.4, gap=.09, pan=0):
    for k, p in enumerate(notes): add(fx, S.note('glockenspiel', p, .9, v), t0 + k * gap, .7, pan)
for e in E:
    t0, tp = e['t'], e['type']
    if tp == 'click': add(fx, sclick(1.1, .8), t0, .5, .1)
    elif tp == 'press': add(fx, pop(.25, 300, 520), t0, 1, .3)
    elif tp == 'key': add(fx, key(.9), t0, .6, -.2); add(fx, pulse(hz(['D5', 'F#5', 'A5'][e['i']]), .07, .25, .12), t0 + .01, 1, -.3)
    elif tp == 'found': chime(['A5', 'E6'], t0, .35, .08, .4)
    elif tp == 'stream': add(fx, swish(.45, .3, 800, 9000), t0 - .1, 1, 0); chime(['D6', 'F#6', 'A6'], t0 + .25, .3, .06)
    elif tp == 'whoosh': add(fx, swish(.5, .34, 300, 3500), t0 - .12, 1, (e.get('d') or 0) * .4)
    elif tp == 'blurwipe':
        d = e['d']; x = nz(d, 500, 3000, .2) * np.sin(np.pi * tt(d)[:int(d * SR)] / d); add(fx, x.astype(np.float32), t0, 1, 0)
    elif tp == 'brackets':
        for k in range(4): add(fx, tick(.22, 3000 + k * 250), t0 + k * .05, 1, (k % 2 - .5) * .6)
    elif tp == 'snap': add(fx, tick(.4, 2400), t0, 1, 0); add(fx, pop(.2, 700, 1200), t0, 1, 0)
    elif tp == 'zoomin': add(fx, swish(.55, .22, 600, 6000), t0, 1, 0)
    elif tp == 'pop': add(fx, pop(.22 if e.get('soft') else .32, 480 + e['i'] * 60, 900 + e['i'] * 90), t0, 1, [-.6, .6, .6, -.6][e['i']])
    elif tp == 'tick': add(fx, tick(.25, 3200 + e['i'] * 200), t0, 1, [-.6, .6, .6, -.6][e['i']])
    elif tp == 'brb': add(fx, pop(.3, 700, 380), t0, 1, 0); add(fx, boom(.35, 80, .7), t0, 1, 0)
    elif tp == 'toast': add(fx, swish(.4, .2, 1000, 6000), t0, 1, .5); chime(['E6', 'A6'], t0 + .15, .4, .11, .5)
    elif tp == 'hit': add(fx, boom(.5, 95, .7), t0, 1, 0)
    elif tp == 'wipe': add(fx, swish(.7, .22, 600, 5000), t0, 1, -.3)
    elif tp == 'jump': d = min(.16, e.get('d', .3) * .4); add(fx, pulse(330, d, .25, .16, slide=2.4), t0, 1, .1)
    elif tp == 'land': add(fx, pulse(110, .06, .5, .2 * e.get('v', .6)), t0, 1, 0); add(fx, crush(nz(.05, 200, 2000, .22 * e.get('v', .6), .015)), t0, 1, 0)
    elif tp == 'step': add(fx, crush(nz(.02, 1500, 6000, .06, .005), 4), t0, 1, .1)
    elif tp == 'morph':
        for k in range(24): add(fx, pulse(rng.uniform(900, 2600), .03, .125, .06), t0 + rng.uniform(0, .5), 1, rng.uniform(-.6, .6))
# quiet room tone so silence never reads as a dropout
fx += (lp(rng.standard_normal(NS), 500) * .002).astype(np.float32)[:, None]

# ═════════ voice + ducking
vo = np.zeros((NS, 2), np.float32)
for e in all_('vo'):
    y, sr = sf.read(os.path.join(HERE, 'voices', e['id'] + '.wav'), dtype='float32')
    if y.ndim > 1: y = y.mean(1)
    if sr != SR: y = soxr.resample(y, sr, SR).astype(np.float32)
    add(vo, compress(y, .3, 3.0), e['t'], 1.0, 0)
env = np.abs(vo[:, 0]); k = int(.15 * SR); env = np.convolve(env, np.ones(k) / k, 'same'); env = np.clip(env / (env.max() * .25), 0, 1)
music *= (1 - .6 * env)[:, None]; fx *= (1 - .3 * env)[:, None]
music = music / (np.abs(music).max() + 1e-9) * .5
fx = fx / (np.abs(fx).max() + 1e-9) * .4
vo = vo / (np.abs(vo).max() + 1e-9) * .9
mix = music * db(float(os.environ.get('GM', 0))) + fx * db(float(os.environ.get('GF', 0))) + vo
os.makedirs(os.path.join(HERE, 'out'), exist_ok=True)
for nm, y in (('music', music), ('fx', fx), ('vo', vo)): sf.write(os.path.join(HERE, 'out', nm + '.wav'), y[:int((DUR + .1) * SR)], SR)
fade = np.ones(NS); fo = int(1.0 * SR); end = int(DUR * SR); fade[end - fo:end] = np.linspace(1, 0, fo); fade[end:] = 0
mix = limit(mix * fade[:, None], .95)
sf.write(os.path.join(HERE, 'mix.wav'), mix[:int((DUR + .1) * SR)], SR)
print('mix.wav', DUR, 's; notes', len(EVN))
