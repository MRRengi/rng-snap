import { clamp, seg, eo, back, hash } from '/core/lib.js';
import * as UI from './ui.js';
import * as PX from './mascot.js';
import * as FM from './film.js';

const Q = new URLSearchParams(location.search), NOSUBS = Q.has('nosubs');
const wD = document.getElementById('wD'), spr = document.getElementById('spr'), hudEl = document.getElementById('hud');
const mbg = document.getElementById('mbg'), mbw = document.getElementById('mbw');

const get = async u => (await fetch(u)).json();
const words = await get('voices/words.json'); words.__dur = await get('voices/dur.json');
const feed = await get('assets/dog/feed.json'), catf = await get('assets/cat/feed.json');
FM.build(words, feed, catf);
const load = src => new Promise(r => { const im = new Image(); im.onload = () => r(im); im.onerror = () => r(im); im.src = src; });
const pad = i => String(i).padStart(3, '0');
const IMS = await Promise.all([...Array(feed.n).keys()].map(i => load(`assets/dog/s/${pad(i)}.jpg`)));
const IMB = await Promise.all([...Array(feed.n).keys()].map(i => load(`assets/dog/b/${pad(i)}.jpg`)));
const IMC = await Promise.all([...Array(catf.n).keys()].map(i => load(`assets/cat/s/${pad(i)}.jpg`)));
window.DUR = FM.DUR;
window.EV = FM.EV.slice().sort((a, b) => a.t - b.t);
window.SUBS = FM.SUBS.map(s => ({ t0: s.t0, t1: s.t1, text: s.text.replace(/[{}]/g, '') }));
window.T = FM.T;

// the phone is held upright, so Lens shows the portrait picture pillarboxed in the 16:9 stage
function drawVideo(cv, V, edge) {
  const g = cv.getContext('2d'), w = cv.width, h = cv.height, c = V.crop, s = IMS[V.i], b = IMB[V.i];
  const dw = h * c.w / c.h, dx = (w - dw) / 2;
  const d = im => g.drawImage(im, c.x, c.y, c.w, c.h, dx, 0, dw, h);
  g.fillStyle = '#0B0B0C'; g.fillRect(0, 0, w, h);
  if (V.blur === 1) d(b); else d(s);
  if (V.blur === 'edge') { const ex = clamp(edge) * w; g.save(); g.beginPath(); g.rect(0, 0, ex, h); g.clip(); d(b); g.restore();
    if (ex > dx && ex < dx + dw) { g.fillStyle = 'rgba(168,242,230,.55)'; g.fillRect(ex - 1, 0, 2, h); } }
}
function drawCat(cv, i) { const g = cv.getContext('2d'); g.drawImage(IMC[i], 0, 0, cv.width, cv.height); }

const tf = ([cx, cy, z]) => `translate(960px,540px) scale(${z}) translate(${-cx}px,${-cy}px)`;
function render(t) {
  const F = FM.frame(t), [cx, cy, z] = F.cam;
  wD.innerHTML = UI.desk(F.desk) + (F.app ? UI.lensWin(F.app, t) : '') + (F.calls || []).map(UI.callWin).join('') + (F.toast ? UI.toast(F.toast) : '') + (F.extra || '');
  for (const el of [wD, spr]) el.style.transform = tf(F.cam);
  const cache = {};
  const A = (key, fx = .5, fy = 0) => { const el = cache[key] !== undefined ? cache[key] : (cache[key] = wD.querySelector(`[data-a="${key}"]`));
    if (!el) return null; const r = el.getBoundingClientRect(); return { x: (r.left + r.width * fx - 960) / z + cx, y: (r.top + r.height * fy - 540) / z + cy, w: r.width / z, h: r.height / z }; };
  const c1 = F.actor.eval(t, A, F);
  // the blur boundary follows Pix as it runs along the top of the picture
  let edge = 0; if (F.video.blur === 'edge') { const v = A('vid', 0, 0); edge = c1 && v ? (c1.x - v.x) / v.w : 0; }
  wD.querySelectorAll('canvas[data-v]').forEach(cv => drawVideo(cv, F.video, edge));
  wD.querySelectorAll('canvas[data-cat]').forEach(cv => drawCat(cv, F.catI));
  // motion blur on whip pans only
  const [px, py] = FM.camAt(t - 1 / 30), vx = (cx - px) * z, vy = (cy - py) * z;
  const bx = Math.min(36, Math.max(0, Math.abs(vx) - 30) * .3), by = Math.min(36, Math.max(0, Math.abs(vy) - 30) * .3), mb = bx > .6 || by > .6;
  mbg.setAttribute('stdDeviation', `${bx.toFixed(2)} ${by.toFixed(2)}`); mbw.style.filter = mb ? 'url(#mb)' : '';
  spr.innerHTML = sprites(F, t, A, z, c1) + F.fx.map(f => f(A)).join('');
  hud(F, t, A, z, cx, cy);
}
function sprites(F, t, A, z, c1) {
  const T = FM.T, act = F.actor; let h = '';
  if (c1) {
    h += PX.sprite(c1);
    const s = act.seg(t), top = c1.y - c1.px * 13, P = c1.px;
    if (s && s.dash) h += PX.speedlines(c1.x, c1.y, 1, .5, Math.max(3, P * .7));
    for (const j of act.jumps) if (t >= j.t1 && t < j.t1 + .4) { const L = act.eval(j.t1 + .001, A, F); if (L) h += PX.dust(L.x, L.y, seg(t, j.t1, j.t1 + .4), Math.max(3, L.px * .7), 6, j.t1 * 10); }
    if (t > T.land0 && t < T.land0 + .6) h += PX.BANG(c1.x + 4 * P, top - 66, 5, 1 - seg(t, T.land0 + .45, T.land0 + .6));
    if (t > 8.55 && t < T.found + .1) h += PX.DOTS(c1.x + 6 * P, top - 14, 5, 1 + Math.floor((t - 8.55) * 5) % 3);
    if (t > T.limits && t < T.limits + .7) { const k = seg(t, T.limits, T.limits + .7); for (let i = 0; i < 6; i++) { const a = i / 6 * Math.PI * 2 + .3; h += PX.SPARK(c1.x + Math.cos(a) * (26 + k * 46), c1.y - 30 + Math.sin(a) * (18 + k * 30), 4, '#FFD24A', 1 - k); } }
    if (t > T.live && t < T.live + .6) { const k = seg(t, T.live, T.live + .6); h += PX.SPARK(c1.x - 70 - k * 26, top - 4 - k * 18, 5, '#FFD24A', 1 - k) + PX.SPARK(c1.x + 70 + k * 26, top - 14 - k * 18, 5, '#FFD24A', 1 - k); }
    if (t > T.run1 + .1 && t < T.run1 + 1) h += PX.CHECK(c1.x - 64, top - 30, 5, 1 - seg(t, T.run1 + .7, T.run1 + 1));
    if (t > T.lensW && t < T.lensW + .7) { const k = seg(t, T.lensW, T.lensW + .7); for (let i = 0; i < 6; i++) { const a = i / 6 * Math.PI * 2 + .3; h += PX.SPARK(c1.x + Math.cos(a) * (28 + k * 50), c1.y - 30 + Math.sin(a) * (18 + k * 34), 4, '#FFD24A', 1 - k); } }
    if (t > T.toast && t < T.toast + .45) h += PX.BANG(c1.x + 4 * P, top - 66, 5);
    if (t > T.end + .9 && t < T.end + 1.9) { const k = seg(t, T.end + .9, T.end + 1.9); h += PX.HEART(c1.x + 56 + k * 14, top - 10 - k * 46, 6, 1 - k * k); }
    const zz = t0 => { if (t < t0) return; for (let i = 0; i < 3; i++) { const k = ((t - t0) / 1.6 + i / 3) % 1; h += PX.ZED(c1.x + 30 + k * 34 + i * 4, top - 6 - k * 70, Math.max(2.5, P * (.5 + k * .4)), Math.min(1, k * 4) * (1 - k)); } };
    if (t > T.sleep2) zz(T.sleep2);
  }
  const cu = F.cursor.eval(t, A);
  if (cu) {
    if (cu.sp > 900) for (let i = 1; i <= 4; i++) { const g = F.cursor.eval(t - i * .012, A); if (g) h += cursorSvg(g.x, g.y, 1, cu.op * .16 * (5 - i) / 4, z); }
    if (cu.ring > 0) h += `<div class="ring" style="left:${cu.x - 24 * cu.ring - 4}px;top:${cu.y - 24 * cu.ring - 4}px;width:${48 * cu.ring + 8}px;height:${48 * cu.ring + 8}px;opacity:${(1 - cu.ring) * .85};border-width:${3 / z}px"></div>`;
    h += cursorSvg(cu.x, cu.y, cu.sc, cu.op, z);
  }
  return h;
}
const cursorSvg = (x, y, sc, op, z) => { const s = 1.5 * sc / z;
  return `<svg style="position:absolute;left:${x - 2 * s}px;top:${y - 2 * s}px;opacity:${op};filter:drop-shadow(0 ${2 / z}px ${3 / z}px rgba(0,0,0,.45))" width="${22 * s}" height="${30 * s}" viewBox="0 0 22 30"><path d="M2 2 L2 24 L7.5 18.5 L11 27 L15 25.2 L11.6 17 L19 17 Z" fill="#fff" stroke="#111" stroke-width="1.5" stroke-linejoin="round"/></svg>`; };

function hud(F, t, A, z, cx, cy) {
  const H = F.hud; let h = '';
  const scr = p => ({ x: (p.x - cx) * z + 960, y: (p.y - cy) * z + 540 });
  if (H.spot) { const p = A(H.spot.key, 0, 0); if (p) { const s = scr(p), w = p.w * z, hh = p.h * z;
    h += `<div style="position:absolute;opacity:${H.spot.op};left:${s.x - 12}px;top:${s.y - 8}px;width:${w + 24}px;height:${hh + 16}px;border-radius:12px;box-shadow:0 0 0 3000px rgba(8,8,10,.5),0 0 0 2px rgba(255,106,61,.9)"></div>`; } }
  if (H.chap && !H.wipe) h += `<div class="chap" style="opacity:${H.chap.op}"><b>${H.chap.no}</b>${H.chap.nm}</div>`;
  if (H.keys) h += `<div class="keys" style="opacity:${H.keys.op};transform:translateY(${H.keys.y}px)">${H.keys.keys.map((k, i) => `<i class="${H.keys.dn[i] ? 'dn' : ''} ${H.keys.held[i] ? 'held' : ''}">${k}</i>${i < 2 ? '<s>+</s>' : ''}`).join('')}<span>${H.keys.label}</span></div>`;
  if (H.cap && !NOSUBS) h += `<div class="cap" style="opacity:${H.cap.op};transform:translate(-50%,${H.cap.y}px)">${H.cap.text.replace(/\{([^}]*)\}/g, '<em>$1</em>')}</div>`;
  if (H.wipe) h += wipe(H.wipe, t);
  if (H.fade) h += `<div class="fade" style="opacity:${H.fade}"></div>`;
  if (t < .4) h += `<div class="fade" style="opacity:${1 - eo(t / .4)}"></div>`;
  hudEl.innerHTML = h;
}
// chapter seam: orange pixel blocks sweep across, the chapter name, a little white Pix running through
function wipe(W, t) {
  const S = 60, cols = 32, rows = 18, p = W.p; let s = '<div class="wipe">';
  const pal = ['#FF6A3D', '#F25A2E', '#FF7E55'];
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) {
    const d = (c / cols) * .55 + ((rows - r) / rows) * .2 + hash(c * 13.1 + r * 7.7) * .12;
    const a = clamp((p / .42 - d) / .18), b = clamp(((p - .55) / .42 - d) / .18), k = a * (1 - b);
    if (k <= 0) continue; const sz = S * k;
    s += `<b style="left:${c * S + (S - sz) / 2}px;top:${r * S + (S - sz) / 2}px;width:${sz + .5}px;height:${sz + .5}px;background:${pal[Math.floor(hash(c * 3.7 + r * 11.3) * 3)]}"></b>`;
  }
  const lo = clamp(Math.min((p - .3) / .12, (.82 - p) / .1));
  if (lo > 0) {
    s += `<div class="wlabel" style="opacity:${lo};transform:translateY(calc(-50% + ${(1 - lo) * 16}px))"><div class="no">${W.no}</div><div class="nm">${W.nm}</div></div>`;
    s += PX.sprite({ x: 300 + p * 1320, y: 800, px: 6, legs: Math.floor(t * 12) % 2 ? 'walkA' : 'walkB', arms: 'tuck', eyes: 'r', op: lo, pal: { 1: '#FFF6F0', 2: '#B8401C', 3: '#F6D5C8', 4: '#FFD9CB', 5: '#F6D5C8' } });
  }
  return s + '</div>';
}
window.render = render;
await Promise.all([400, 500, 600, 700].flatMap(w => [`${w} 16px Inter`, `${w} 40px InterTight`]).map(f => document.fonts.load(f)));
render(+(Q.get('t') || 0));
window.READY = true;
