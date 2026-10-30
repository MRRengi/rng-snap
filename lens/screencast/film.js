// "Lens — a living screencast": VO word anchors, camera, Pix's performance, cursor, UI state, sound events
import { clamp, lerp, seg, ss, eio, eo, ei, back, hash } from '/core/lib.js';
import { WIN, G } from './ui.js';
import * as PX from './mascot.js';

export const BPM = 100, B = 60 / BPM;
export const DUR = 36.0;
export const VO = { s1: .7, s2: 5.4, s3: 8.6, s4: 11.5, s5: 13.1, s6: 16.9, s7: 23.4, s8: 26.6, s9: 29.5 };
let WORDS = {}, FEED = null;
const nrm = s => s.toLowerCase().replace(/[^a-z0-9]/g, '');
export const W = (id, word, n = 0) => { let k = 0; for (const [w, a] of WORDS[id] || []) if (nrm(w) === nrm(word) && k++ === n) return VO[id] + a; throw new Error(`word ${word} not in ${id}`); };
export const WE = (id, word, n = 0) => { let k = 0; for (const [w, , b] of WORDS[id] || []) if (nrm(w) === nrm(word) && k++ === n) return VO[id] + b; throw new Error(`word ${word} not in ${id}`); };

export const EV = [];
const ev = (t, type, o = {}) => EV.push({ t: +t.toFixed(3), type, ...o });

// ───────── camera (recording-software camera: zoom ≥ 1, never past the monitor)
const EASE = { eio, ss, eo, whip: x => { x = clamp(x); return x < .5 ? 16 * x ** 5 : 1 - Math.pow(-2 * x + 2, 5) / 2; } };
let CAM0 = [330, 175, 2.6]; const MOVES = [];
const cam = (t0, d, to, e = 'eio') => MOVES.push({ t0, d, to, e });
export function camAt(t) {
  let v = CAM0.slice(); v[2] = Math.log(v[2]);
  for (const m of MOVES) { if (t <= m.t0) break; const p = EASE[m.e]((t - m.t0) / m.d); v = [lerp(v[0], m.to[0], p), lerp(v[1], m.to[1], p), lerp(v[2], Math.log(m.to[2]), p)]; }
  const z = Math.max(1, Math.exp(v[2])), hw = 960 / z, hh = 540 / z;
  const x = clamp(v[0] + Math.sin(t * .7) * 1.1, hw, 1920 - hw), y = clamp(v[1] + Math.sin(t * .53 + 1) * .8, hh, 1080 - hh);
  return [x, y, z];
}

// ───────── actor: Pix
class Actor {
  constructor(px = 6) { this.px = px; this.segs = []; this.jumps = []; }
  add(s) { this.segs.push(s); return this; }
  stand(t0, t1, at, o = {}) { return this.add({ k: 'stand', t0, t1, at, ...o }); }
  walk(t0, t1, from, to, o = {}) { for (let t = t0 + .05; t < t1; t += o.run ? .075 : .11) ev(t, 'step', { v: o.run ? .4 : .5 }); return this.add({ k: 'walk', t0, t1, from, to, ...o }); }
  jump(t0, t1, from, to, h = 120, o = {}) { this.jumps.push({ t0, t1 }); ev(t0 - .02, 'jump', { d: t1 - t0 }); ev(t1, 'land', { v: o.big ? 1 : .6 }); return this.add({ k: 'jump', t0, t1, from, to, h, ...o }); }
  hide(t0, t1) { return this.add({ k: 'hide', t0, t1 }); }
  seg(t) { let s = null; for (const x of this.segs) { if (x.t0 <= t && t < x.t1) return x; if (x.t0 <= t) s = x; } return s; }
  eval(t, A, ctx) {
    const s = this.seg(t); if (!s || s.k === 'hide') return null;
    const px = s.px || this.px; let pos, legs = 'stand', flip = s.flip || false, sq = 0, rot = 0, arms = 'down', eyes = 'n';
    const P = f => typeof f === 'function' ? f(A, t) : f;
    if (s.k === 'stand') pos = P(s.at);
    if (s.k === 'walk') { const a = P(s.from), b = P(s.to); if (!a || !b) return null; const p = (s.ease || ss)(seg(t, s.t0, s.t1));
      pos = { x: lerp(a.x, b.x, p), y: lerp(a.y, b.y, p) }; legs = Math.floor(t * (s.run ? 15 : 9)) % 2 ? 'walkA' : 'walkB'; flip = b.x < a.x; if (s.run) arms = 'tuck'; }
    if (s.k === 'jump') { const a = P(s.from), b = P(s.to); if (!a || !b) return null; const p = seg(t, s.t0, s.t1);
      const ap = Math.min(a.y, b.y) - s.h, cy = 2 * ap - (a.y + b.y) / 2, x = lerp(a.x, b.x, p), y = (1 - p) ** 2 * a.y + 2 * (1 - p) * p * cy + p * p * b.y;
      pos = { x, y }; legs = 'tuck'; flip = b.x < a.x - 2 ? true : b.x > a.x + 2 ? false : flip;
      sq = p < .5 ? -.22 * (1 - p * 2) : -.12 * (p * 2 - 1); if (s.flipRot) rot = p * Math.PI * 2 * (b.x < a.x ? -1 : 1); arms = 'up'; }
    if (!pos) return null;
    for (const j of this.jumps) {
      if (t >= j.t0 - .14 && t < j.t0) sq = Math.max(sq, .28 * ss(seg(t, j.t0 - .14, j.t0)));
      if (t >= j.t1 && t < j.t1 + .22) { const k = seg(t, j.t1, j.t1 + .22); sq = .34 * (1 - k) * Math.cos(k * 7); }
    }
    if (s.k === 'stand' && !s.still) { const bp = ((t % B) + B) % B; if (bp < .12) sq += .07 * Math.sin(bp / .12 * Math.PI); }
    const bl = Math.floor(t / 2.7), bt = t - bl * 2.7 - hash(bl * 3.7) * 1.8;
    if (bt > 0 && bt < .1) eyes = 'blink';
    if (s.eyes) { const e = typeof s.eyes === 'function' ? s.eyes(t) : s.eyes; if (e && !(eyes === 'blink' && 'nlrud'.includes(e[0]) && e !== 'up')) eyes = e; }
    if (s.arms) arms = typeof s.arms === 'function' ? s.arms(t) : s.arms;
    if (s.legs) legs = typeof s.legs === 'function' ? s.legs(t) : s.legs;
    if (s.sq) sq += s.sq(t);
    if (s.flipF) flip = s.flipF(t);
    const x = Math.round(pos.x / (px / 2)) * (px / 2);
    return { x, y: pos.y, px, flip, sq, rot, legs, arms, eyes, op: 1, shadow: s.k === 'jump' ? 0 : 1 };
  }
}
// ───────── cursor
class Cursor {
  constructor() { this.moves = []; this.clicks = []; this.vis = []; }
  show(t0, t1) { this.vis.push([t0, t1]); return this; }
  move(t0, t1, to, arc = 40) { this.moves.push({ t0, t1, to, arc }); return this; }
  click(t) { this.clicks.push(t); ev(t, 'click'); return this; }
  eval(t, A) {
    let op = 0; for (const [a, b] of this.vis) op = Math.max(op, Math.min(seg(t, a, a + .15), 1 - seg(t, b - .2, b)));
    if (op <= 0) return null;
    let p = null, prev = null;
    for (const m of this.moves) { const to = m.to(A, t); if (!to) continue; if (t < m.t0) break;
      if (!prev) { p = to; prev = to; continue; }
      const k = seg(t, m.t0, m.t1), e = eio(k), from = prev, dx = to.x - from.x, dy = to.y - from.y, L = Math.hypot(dx, dy) || 1;
      p = { x: lerp(from.x, to.x, e) - dy / L * m.arc * Math.sin(Math.PI * e), y: lerp(from.y, to.y, e) + dx / L * m.arc * Math.sin(Math.PI * e), sp: Math.abs(Math.sin(Math.PI * k)) * L / (m.t1 - m.t0) };
      prev = k >= 1 ? to : from; if (k < 1) break; }
    if (!p) return null;
    let sc = 1, ring = 0; for (const c of this.clicks) { if (t >= c && t < c + .1) sc = .82; if (t >= c && t < c + .45) ring = seg(t, c, c + .45); }
    return { ...p, op, sc, ring };
  }
}
// anchors: reuse an element's last position while it is absent (frames render in order)
const at = (key, fx = .5, fy = 0, dx = 0, dy = 0) => { let last = null; return A => { const p = A(key, fx, fy); if (p) last = { x: p.x + dx, y: p.y + dy }; return last; }; };
const pt = (x, y) => () => ({ x, y });

// ───────── feed: frame index, smoothed face → auto-frame crop
const pp = (i, n) => { const m = 2 * (n - 1); i = ((i % m) + m) % m; return i < n ? i : m - i; };
export const feedIndex = t => pp(Math.floor((t - 7.6) * 30) + 90, FEED.n);
let FACE = [];
function smoothFaces() {
  const f = FEED.faces, n = f.length, raw = f.map((b, i) => b || f.slice(0, i).reverse().find(Boolean) || f.find(Boolean));
  FACE = raw.map((_, i) => { let a = [0, 0, 0, 0], c = 0; for (let k = -12; k <= 12; k++) { const b = raw[pp(i + k, n)]; a = a.map((v, j) => v + b[j]); c++; } return a.map(v => v / c); });
}
export function cropAt(t, az) {
  const [x, y, w, h] = FACE[feedIndex(t)], ch = clamp(h * 1.95, 300, 540), cw = ch * 16 / 9;
  const cx = clamp(x + w / 2, cw / 2, 960 - cw / 2), cy = clamp(y + h * .72, ch / 2, 540 - ch / 2);
  const T = { x: cx - cw / 2, y: cy - ch / 2, w: cw, h: ch };
  return { x: lerp(0, T.x, az), y: lerp(0, T.y, az), w: lerp(960, T.w, az), h: lerp(540, T.h, az), T };
}

// ───────── build
export const SUBS = [], T = {};
let C, CUR;
const CALLS = [['zoom', 'Zoom', 30, 52], ['teams', 'Teams', 1490, 52], ['obs', 'OBS', 1490, 700], ['discord', 'Discord', 30, 700]];
export function build(words, feed) {
  WORDS = words; FEED = feed; smoothFaces(); EV.length = 0; MOVES.length = 0; SUBS.length = 0;
  C = new Actor(6); CUR = new Cursor();
  const cap = { s1: 'This is {Lens}. It turns your phone into a webcam.', s2: 'Open the app on your phone, and it shows up {right here}.', s3: "One click, and you're {live}.",
    s4: '{Blur} the background.', s5: 'Turn on {auto-frame}, and the shot follows you.', s6: 'Zoom, Teams, OBS and Discord all see one camera, called {Lens}.',
    s7: 'Need a minute? {Ctrl + Alt + P}.', s8: 'Everyone sees {Be right back}.', s9: '{Free}, and open source.' };
  const dur = words.__dur;
  for (const [id, t] of Object.entries(VO)) { ev(t, 'vo', { id }); SUBS.push({ id, t0: t + .05, t1: Math.max(t + dur[id] + .35, t + 1.9), text: cap[id] }); }
  // keep consecutive captions from overlapping
  for (let i = 0; i < SUBS.length - 1; i++) SUBS[i].t1 = Math.min(SUBS[i].t1, SUBS[i + 1].t0 - .05);

  // ═════ A. cold open: the brand, then the empty state's phone icon comes alive (0–4.8)
  T.lens = W('s1', 'lens'); T.whip0 = W('s1', 'turns') - .2;
  cam(0, 2.2, [345, 180, 2.45], 'ss');
  cam(T.whip0, .5, [G.cx, 530, 2.3], 'whip'); ev(T.whip0 - .02, 'whoosh', { d: 1 });
  T.jit0 = W('s1', 'your'); T.born0 = W('s1', 'phone') - .02; T.born1 = T.born0 + .5; ev(T.born0, 'morph');
  T.land0 = T.born1 + .38;
  cam(W('s1', 'webcam') - .05, 1.3, [960, 540, 1.0], 'eio');
  C.hide(0, T.born1)
   .jump(T.born1, T.land0, at('es-icon', .5, 1), at('es-h', .5, 0), 46)
   .stand(T.land0, 4.8, at('es-h', .5, 0), { eyes: t => t < T.land0 + .5 ? 'wide' : t < 4.05 ? 'l' : 'r' });

  // ═════ B. 01 Connect (4.8–10.8)
  T.wipe1 = 4.8;
  cam(5.0, .1, [1576, 330, 1.7]); cam(5.2, 2.7, [1566, 322, 1.78], 'ss');
  T.found = W('s2', 'shows') - .04; ev(T.found, 'found');
  T.click = W('s3', 'click') - .03; T.live = W('s3', 'live'); T.stream = T.live - .32;
  ev(T.click + .02, 'press'); ev(T.stream, 'stream');
  CUR.show(8.05, T.click + .45).move(8.05, 8.05, pt(1700, 660)).move(8.1, T.click - .03, at('btn-connect', .5, .55), 30).click(T.click);
  cam(T.click + .22, .5, [G.cx, 560, 1.2], 'whip'); ev(T.click + .2, 'whoosh', { d: -1 });
  C.hide(4.8, 5.6)
   .jump(5.6, 5.98, at('sec-wifi', .62, 0, 0, -240), at('sec-wifi', .62, 0), 10)
   .stand(5.98, T.found + .12, at('sec-wifi', .62, 0), { eyes: t => t < 6.5 ? 'd' : 'r' })
   .jump(T.found + .12, T.found + .44, at('sec-wifi', .62, 0), at('row-pixel', .4, 0), 34)
   .stand(T.found + .44, T.click + .03, at('row-pixel', .4, 0), { eyes: t => t < T.found + .9 ? 'wide' : 'r', arms: t => t > W('s2', 'right') - .1 ? 'point' : 'down' })
   .jump(T.click + .03, T.click + .3, at('row-pixel', .4, 0), at('row-pixel', .4, 0), 26)
   .jump(T.click + .36, T.live, at('row-pixel', .4, 0), at('badge', .5, 0), 170, { big: true, px: 6 })
   .stand(T.live, 10.8, at('badge', .5, 0), { eyes: t => t < T.live + .7 ? 'happy' : 'r' });

  // ═════ C. 02 Effects (10.8–16.2)
  T.wipe2 = 10.8;
  cam(11.0, .1, [G.cx, 560, 1.22]); cam(11.3, 2.0, [G.cx, 545, 1.28], 'ss');
  T.blur = W('s4', 'blur') - .03; T.run0 = T.blur + .12; T.run1 = WE('s4', 'background') + .05;
  ev(T.run0, 'blurwipe', { d: T.run1 - T.run0 });
  T.af = W('s5', 'autoframe') - .02; T.br0 = T.af + .15; T.br1 = T.br0 + .6; T.z0 = T.br1 + .05; T.z1 = T.z0 + .55;
  ev(T.br0, 'brackets'); ev(T.br1, 'snap'); ev(T.z0, 'zoomin');
  T.follow = W('s5', 'follows');
  CUR.show(11.0, T.af + .45).move(11.0, 11.0, pt(G.cx + 360, 940)).move(11.05, T.blur - .03, at('tb-blur', .5, .55), 30).click(T.blur)
     .move(T.blur + .5, T.af - .03, at('tb-frame', .5, .55), 14).click(T.af);
  cam(T.af - .1, .8, [G.cx, 560, 1.24], 'eio');
  C.hide(10.8, 11.5)
   .jump(11.15, 11.5, at('vid', .04, 0, 0, -240), at('vid', .04, 0), 10)
   .stand(11.5, T.run0, at('vid', .04, 0), { eyes: 'dr' })
   .walk(T.run0, T.run1, at('vid', .04, 0), at('vid', .955, 0), { run: true, dash: true, ease: x => x, eyes: 'r' })
   .stand(T.run1, T.br0 + .05, at('vid', .955, 0), { flip: true, eyes: t => t < T.run1 + .8 ? 'happy' : 'dl' })
   .jump(T.br0 + .05, T.br0 + .4, at('vid', .955, 0), at('af-tl', .55, 0, 4, 0), 70)
   .stand(T.br0 + .4, T.z1 + .05, at('af-tl', .55, 0, 4, 0), { still: true, eyes: t => t < T.z0 ? 'dr' : 'wide' })
   .walk(T.z1 + .05, T.follow + .1, at('af-tl', .55, 0, 4, 0), at('vid', .5, 0), { eyes: 'r' })
   .stand(T.follow + .1, 16.2, at('vid', .5, 0), { eyes: t => t < T.follow + .9 ? 'd' : 'happy' });

  // ═════ D. 03 Every app (16.2–22.8)
  T.wipe3 = 16.2;
  cam(16.4, .1, [960, 540, 1.0]);
  T.outClick = 16.78;
  CUR.show(16.4, 17.15).move(16.4, 16.4, pt(760, 760)).move(16.45, T.outClick - .03, at('nav-output', .35, .5), 40).click(T.outClick);
  T.pops = ['zoom', 'teams', 'obs', 'discord'].map(n => W('s6', n));
  T.pops.forEach((x, i) => ev(x - .1, 'pop', { i }));
  T.hot = W('s6', 'camera'); [0, 1, 2, 3].forEach(i => ev(T.hot + i * .06, 'tick', { i }));
  T.called = W('s6', 'called'); T.lensW = W('s6', 'lens'); T.close = T.called - .25;
  cam(T.called - .1, .65, [1560, 340, 1.5], 'eio'); ev(T.called - .12, 'whoosh', { d: 1 });
  T.spot0 = T.lensW + .15; T.spot1 = 22.65;
  const ct = k => at('ct-' + k, .5, 0);
  C.hide(16.2, 16.72)
   .jump(16.72, T.pops[0], at('ct-zoom', .5, 0, 0, -230), ct('zoom'), 10);
  for (let i = 1; i < 4; i++) { const k = CALLS[i][0], kp = CALLS[i - 1][0];
    C.stand(T.pops[i - 1], T.pops[i - 1] + .06, ct(kp), { eyes: 'happy' }).jump(T.pops[i - 1] + .06, T.pops[i], ct(kp), ct(k), i === 2 ? 60 : 150); }
  C.stand(T.pops[3], T.close + .12, ct('discord'), { eyes: t => t < T.hot ? 'u' : 'happy', arms: t => t > T.hot && t < T.hot + .6 ? 'up' : 'down' })
   .jump(T.close + .12, T.lensW, ct('discord'), at('row-cam', .6, 0), 260, { big: true })
   .stand(T.lensW, 22.8, at('row-cam', .6, 0), { eyes: t => t < T.spot0 + .2 ? 'happy' : 'd' });

  // ═════ E. 04 Pause (22.8–29.3)
  T.wipe4 = 22.8;
  cam(23.0, .1, [G.cx, 560, 1.3]); cam(23.1, 2.8, [G.cx, 552, 1.36], 'ss');
  T.keys = [W('s7', 'control'), W('s7', 'alt'), W('s7', 'p')]; T.keys.forEach((x, i) => ev(x, 'key', { i }));
  T.brb = T.keys[2] + .04; ev(T.brb, 'brb');
  T.every = W('s8', 'everyone'); cam(T.every - .12, .8, [960, 540, 1.0], 'eio'); [0, 1, 2, 3].forEach(i => ev(T.every + .1 + i * .07, 'pop', { i, soft: 1 }));
  T.close2 = 28.85; T.sleep = T.brb + .95;
  C.hide(22.8, 23.5)
   .jump(23.15, 23.5, at('vid', .5, 0, 0, -240), at('vid', .5, 0), 10)
   .stand(23.5, T.brb + .12, at('vid', .5, 0), { eyes: t => t < T.keys[0] - .2 ? 'n' : 'dl' })
   .jump(T.brb + .12, T.brb + .45, at('vid', .5, 0), at('brb', .5, 0), 50)
   .stand(T.brb + .45, 29.32, at('brb', .5, 0), { eyes: t => t < T.sleep - .4 ? 'n' : t < T.sleep ? 'blink' : 'shut', legs: t => t > T.sleep ? 'tuck' : 'stand', arms: t => t > T.sleep ? 'tuck' : 'down', still: true,
     sq: t => t > T.sleep ? .05 + .05 * Math.sin((t - T.sleep) * 3.2) : 0 });
  for (let t = T.sleep + .3; t < 29.2; t += 1.05) ev(t, 'zz');

  // ═════ F. end: a real notification, then the wordmark (29.3–36)
  T.toast = 29.2; ev(T.toast, 'toast');
  T.free = W('s9', 'free'); T.open = W('s9', 'source');
  cam(29.3, 1.1, [1440, 800, 1.42], 'eio');
  T.out = 31.85; ev(T.out, 'whoosh', { d: 0 });
  T.end = 32.6; ev(T.end, 'hit'); T.sleep2 = 34.25;
  cam(T.out, .9, [960, 540, 1.0], 'eio'); cam(T.out + 1, DUR - T.out - 1, [960, 548, 1.05], 'ss');
  C.jump(29.32, T.free, at('brb', .5, 0), at('toast', .28, 0), 150)
   .stand(T.free, T.open - .3, at('toast', .28, 0), { eyes: 'happy' })
   .jump(T.open - .3, T.open, at('toast', .28, 0), at('toast', .72, 0), 40)
   .stand(T.open, T.out + .05, at('toast', .72, 0), { eyes: 'happy', arms: t => t < T.open + .5 ? 'up' : 'down' })
   .jump(T.out + .05, T.end, at('toast', .72, 0), at('e-word', .66, 0, 0, 4), 230, { big: true, px: 7 })
   .stand(T.end, DUR + 1, at('e-word', .66, 0, 0, 4), { px: 7, still: true, eyes: t => t < T.end + .8 ? 'wide' : t < T.sleep2 - .3 ? 'happy' : t < T.sleep2 ? 'blink' : 'shut',
     arms: t => t > 33.1 && t < 33.9 ? 'wave' : t > T.sleep2 ? 'tuck' : 'down', legs: t => t > T.sleep2 ? 'tuck' : 'stand', sq: t => t > T.sleep2 ? .05 + .05 * Math.sin((t - T.sleep2) * 3) : 0 });
  for (let t = T.sleep2 + .3; t < DUR - .4; t += 1.05) ev(t, 'zz');
  CH.forEach(([t0]) => ev(t0, 'wipe'));
  T.fade = DUR - .8;
}

// ───────── per frame
export const CH = [[4.8, '01', 'Connect'], [10.8, '02', 'Effects'], [16.2, '03', 'Every app'], [22.8, '04', 'Pause']];
export function frame(t) {
  const F = { t, hud: {}, fx: [], actor: C, cursor: CUR };
  F.cam = camAt(t);
  F.desk = { lensOn: t >= T.stream };
  const S = F.app = { nav: 'phone', panel: 'phone0', connected: t >= T.stream, uptime: '0:' + String(Math.max(0, Math.floor(t - T.live + 7))).padStart(2, '0'), fxOn: t >= T.blur };
  // A: empty state, the icon wobbles, then is reborn as Pix
  S.empty = { jit: t > T.jit0 && t < T.born0 ? 1.2 + 2.2 * seg(t, T.jit0, T.born0) : 0, gone: t >= T.born0 + .02 };
  S.emptyOp = t < T.stream ? 1 : 1 - seg(t, T.stream, T.stream + .25);
  if (t >= T.born0 && t < T.born1 + .03) F.fx.push(A => birth(A, seg(t, T.born0, T.born1)));
  // B
  if (t >= T.found) S.found = { p: back(seg(t, T.found, T.found + .3)), press: t >= T.click && t < T.click + .12, busy: t >= T.click + .12 && t < T.stream };
  if (t >= T.live) S.panel = 'phone1';
  S.vidOp = seg(t, T.stream, T.stream + .25);
  // C/D/E video state
  const V = S.vidUI = { blur: t >= T.blur, frame: t >= T.af, paused: t >= T.brb };
  if (t > T.blur - .45 && t < T.blur + .35) V.hov = 'blur', V.tip = { key: 'blur', text: 'Background blur', op: Math.min(seg(t, T.blur - .45, T.blur - .3), 1 - seg(t, T.blur + .2, T.blur + .35)) };
  if (t > T.af - .45 && t < T.af + .35) V.hov = 'frame', V.tip = { key: 'frame', text: 'Auto frame', op: Math.min(seg(t, T.af - .45, T.af - .3), 1 - seg(t, T.af + .2, T.af + .35)) };
  const az = eio(seg(t, T.z0, T.z1)), crop = cropAt(t, az);
  F.video = { i: feedIndex(t), blur: t < T.run0 ? 0 : t < T.run1 ? 'edge' : 1, crop, brb: eo(seg(t, T.brb, T.brb + .35)) };
  if (t >= T.br0 && t < T.z1 + .3) {      // brackets: close in on the region auto-frame keeps, then the picture zooms to it
    const k = eio(seg(t, T.br0, T.br1)), Tg = crop.T, sx = G.vid.w / crop.w, sy = G.vid.h / crop.h;
    const tr = { x: (Tg.x - crop.x) * sx, y: (Tg.y - crop.y) * sy, w: Tg.w * sx, h: Tg.h * sy };
    V.af = { x: lerp(0, tr.x, k) + 10 * (1 - k), y: lerp(0, tr.y, k) + 10 * (1 - k), w: lerp(G.vid.w, tr.w, k) - 20 * (1 - k), h: lerp(G.vid.h, tr.h, k) - 20 * (1 - k), op: Math.min(seg(t, T.br0, T.br0 + .12), 1 - seg(t, T.z1, T.z1 + .3)) };
    if (t >= T.z0) Object.assign(V.af, { x: tr.x + 6 * az, y: tr.y + 6 * az, w: tr.w - 12 * az, h: tr.h - 12 * az });
  }
  if (t >= T.brb) V.brb = eo(seg(t, T.brb, T.brb + .3));
  // D: nav → Output
  if (t >= T.outClick) S.nav = 'output', S.panel = 'output';
  if (t > T.outClick - .4 && t < T.outClick) S.navHov = 'output';
  S.frames = 412 + Math.max(0, Math.floor((t - 16.2) * 30));
  F.calls = [];
  const callsUp = (t0s, t1, brb, hot) => CALLS.forEach(([k, name, x, y], i) => { const t0 = t0s[i]; if (t < t0 - .1 || t > t1 + .3) return;
    const p = t < t1 ? back(seg(t, t0 - .1, t0 + .18), 1.4) : 1 - eio(seg(t, t1, t1 + .3));
    F.calls.push({ k, name, x, y, op: Math.min(1, p * 3), tf: `scale(${.6 + .4 * p})`, brb, hot: hot && t >= T.hot + i * .06 }); });
  callsUp(T.pops, T.close, false, true);
  callsUp([0, 1, 2, 3].map(i => T.every + .1 + i * .07), T.close2, true, false);
  // F: toast, then the window leaves and the end card arrives
  if (t >= T.toast) { const k = eo(seg(t, T.toast, T.toast + .45)); F.toast = { op: Math.min(1, k * 2) * (1 - seg(t, T.out, T.out + .6)), tf: `translateX(${(1 - k) * 440}px)` }; }
  if (t >= T.out) { const k = eio(seg(t, T.out, T.out + .8)); S.winSt = { op: 1 - k, tf: `scale(${1 - .08 * k})` }; if (t >= T.out + .8) F.app = null; }
  if (t >= T.out + .1) F.extra = endCard(t);
  hud(F, t);
  return F;
}
// Pix assembles out of the phone icon: its orange outline pixels fly to Pix's pixels and turn teal
function birth(A, p) {
  const ic = A('es-icon', 0, 0); if (!ic) return '';
  const dst = PX.pixels({ x: ic.x + ic.w / 2, y: ic.y + ic.h, px: 6 });
  const N = dst.length; let h = '';
  dst.forEach((d, i) => {
    const a = (i / N) * Math.PI * 2, sx = ic.x + ic.w / 2 + Math.cos(a) * ic.w * .5, sy = ic.y + ic.h / 2 + Math.sin(a) * ic.h * .5;
    const k = eio(clamp((p - hash(i * 1.7) * .3) / .7)), ay = -Math.sin(k * Math.PI) * 26 * hash(i * 5.3), ax = Math.sin(k * Math.PI) * (hash(i * 3.1) - .5) * 40;
    const col = k < .55 ? '#FF6A3D' : PX.PAL0[d.v];
    const sz = lerp(3.4, 6, k);
    h += `<div style="position:absolute;left:${lerp(sx, d.x, k) + ax - sz / 2}px;top:${lerp(sy, d.y, k) + ay - sz / 2}px;width:${sz + .4}px;height:${sz + .4}px;background:${col}"></div>`;
  });
  return h;
}
function endCard(t) {
  const k = seg(t, T.out + .25, T.out + .8), s = seg(t, T.end + .2, T.end + .7), u = seg(t, T.end + .6, T.end + 1.1);
  return `<div class="scrim" style="opacity:${eo(seg(t, T.out, T.out + .8)) * .55}"></div>
   <div class="end" style="top:372px;opacity:${k};transform:translateY(${(1 - eo(k)) * 30}px)"><span class="ew" data-a="e-word"><span class="ed" style="transform:scale(${back(k)})"></span>Lens</span></div>
   <div class="end es1" style="top:600px;opacity:${s};transform:translateY(${(1 - eo(s)) * 14}px)">Your phone, as your webcam.</div>
   <div class="end es2" style="top:668px;opacity:${u}"><b>Free</b> · Open source · github.com/MRRengi/rng-lens</div>`;
}
function hud(F, t) {
  const H = F.hud;
  const s = SUBS.find(x => t >= x.t0 && t < x.t1);
  if (s) H.cap = { text: s.text, op: Math.min(seg(t, s.t0, s.t0 + .12), 1 - seg(t, s.t1 - .15, s.t1)), y: (1 - eo(seg(t, s.t0, s.t0 + .2))) * 10 };
  CH.forEach(([t0, no, nm], i) => {
    const t1 = i < CH.length - 1 ? CH[i + 1][0] - .15 : 29.2;
    if (t >= t0 && t < t0 + .85) H.wipe = { p: (t - t0) / .85, no, nm };
    if (t >= t0 + .6 && t < t1) H.chap = { no, nm, op: Math.min(seg(t, t0 + .6, t0 + .9), 1 - seg(t, t1 - .3, t1)) };
  });
  const k0 = T.keys[0] - .35, k1 = T.brb + 1.1;
  if (t >= k0 && t < k1) H.keys = { keys: ['Ctrl', 'Alt', 'P'], label: 'pause camera', op: Math.min(seg(t, k0, k0 + .15), 1 - seg(t, k1 - .25, k1)), y: (1 - back(seg(t, k0, k0 + .3))) * 20,
    held: T.keys.map(x => t >= x && t < T.brb + .45), dn: T.keys.map(x => t >= x && t < x + .16) };
  if (t > T.spot0 && t < T.spot1 + .3) H.spot = { key: 'foot', op: Math.min(seg(t, T.spot0, T.spot0 + .25), 1 - seg(t, T.spot1, T.spot1 + .3)) };
  if (t > T.fade) H.fade = seg(t, T.fade, DUR);
}
