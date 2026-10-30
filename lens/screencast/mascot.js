// "Pix": a little phone that came alive. 14×13 square pixels: teal bezel, mint screen-face, two eyes, an earpiece slot.
// Teal is a hue the Lens UI never uses (it is orange/neutral), so Pix finds the eye in any frame.
export const BODY = '#1FB5A3', SHADE = '#14857A', FACE = '#A8F2E6', EYE = '#0D2321', SLOT = '#14857A';
export const PAL0 = { 1: BODY, 2: EYE, 3: SLOT, 4: FACE, 5: SHADE };
export const W = 14, H = 13;

// returns [[c, r, kind]], kind: 1 bezel 2 eye 3 slot 4 face 5 shade
export function grid({ eyes = 'n', arms = 'down', legs = 'stand' } = {}) {
  const P = new Map(), put = (c, r, v = 1) => P.set(c + ',' + r, v);
  for (let r = 0; r <= 10; r++) for (let c = 3; c <= 10; c++) { if ((r === 0 || r === 10) && (c === 3 || c === 10)) continue; put(c, r, r === 10 ? 5 : 1); }
  for (let r = 2; r <= 8; r++) for (let c = 4; c <= 9; c++) put(c, r, 4);
  put(6, 1, 3); put(7, 1, 3);
  if (arms === 'up') for (const r of [1, 2, 3, 4]) { put(1, r); put(2, r); put(11, r); put(12, r); }
  else if (arms === 'wave') { for (const r of [6, 7]) { put(1, r); put(2, r); } for (const r of [1, 2, 3, 4]) { put(11, r); put(12, r); } }
  else if (arms === 'point') { for (const r of [6, 7]) { put(1, r); put(2, r); } put(11, 5); put(12, 5); put(13, 5); put(11, 6); put(12, 6); put(13, 6); }
  else if (arms !== 'tuck') for (const r of [6, 7]) for (const c of [1, 2, 11, 12]) put(c, r);
  const L = { stand: [[4, 2], [9, 2]], walkA: [[4, 2], [9, 1]], walkB: [[4, 1], [9, 2]], tuck: [[4, 1], [9, 1]], none: [] }[legs];
  for (const [c, h] of L) for (let r = 11; r < 11 + h; r++) put(c, r);
  const E = { n: [[5, 4], [5, 5], [8, 4], [8, 5]], r: [[6, 4], [6, 5], [9, 4], [9, 5]], l: [[4, 4], [4, 5], [7, 4], [7, 5]],
    u: [[5, 3], [5, 4], [8, 3], [8, 4]], d: [[5, 5], [5, 6], [8, 5], [8, 6]], dl: [[4, 5], [4, 6], [7, 5], [7, 6]], dr: [[6, 5], [6, 6], [9, 5], [9, 6]],
    blink: [[5, 5], [8, 5]], happy: [[4, 5], [5, 4], [6, 5], [7, 5], [8, 4], [9, 5]], wide: [[5, 3], [5, 4], [5, 5], [8, 3], [8, 4], [8, 5]],
    shut: [[4, 5], [5, 5], [8, 5], [9, 5]] }[eyes];
  for (const [c, r] of E) put(c, r, 2);
  if (eyes === 'shut' || eyes === 'happy') { put(6, 7, 2); put(7, 7, 2); }     // a little mouth when sleeping / delighted
  return [...P].map(([k, v]) => { const [c, r] = k.split(',').map(Number); return [c, r, v]; });
}

// x,y = midpoint under the feet (world coords); px = pixel side; sq>0 squash, <0 stretch; flip faces left; rot radians
export function sprite({ x, y, px = 7, flip = false, sq = 0, rot = 0, op = 1, shadow = 0, pal = PAL0, ...pose }) {
  const Gd = grid(pose); let s = '';
  for (let [c, r, v] of Gd) { if (flip) c = W - 1 - c; s += `<rect x="${c}" y="${r}" width="1.04" height="1.04" fill="${pal[v]}"/>`; }
  const sy = 1 - sq, sx = 1 + sq * .7, w = W * px * sx, h = H * px * sy;
  const sh = shadow > 0 ? `<div style="position:absolute;left:${x - 5 * px}px;top:${y - px * .4}px;width:${10 * px}px;height:${px * .8}px;border-radius:50%;background:rgba(0,0,0,${.3 * shadow})"></div>` : '';
  return sh + `<svg class="px" viewBox="0 0 ${W} ${H}" width="${w}" height="${h}" style="left:${x - w / 2}px;top:${y - h}px;opacity:${op};transform:rotate(${rot}rad);transform-origin:50% 60%" shape-rendering="crispEdges">${s}</svg>`;
}
export function pixels(o) {
  const { x, y, px = 7, flip = false } = o;
  return grid(o).map(([c, r, v]) => ({ x: x + ((flip ? W - 1 - c : c) - W / 2 + .5) * px, y: y + (r - H + .5) * px, v }));
}
export function pixart(rows, { x, y, px = 7, pal, op = 1 }) {
  let s = ''; rows.forEach((row, r) => [...row].forEach((ch, c) => { if (pal[ch]) s += `<rect x="${c}" y="${r}" width="1.04" height="1.04" fill="${pal[ch]}"/>`; }));
  const w = rows[0].length, h = rows.length;
  return `<svg class="px" viewBox="0 0 ${w} ${h}" width="${w * px}" height="${h * px}" style="left:${x}px;top:${y}px;opacity:${op}" shape-rendering="crispEdges">${s}</svg>`;
}
const YEL = '#FFD24A';
export const BANG = (x, y, px = 6, op = 1) => pixart(['.kkk.', 'kwwwk', 'kwkwk', 'kwkwk', 'kwwwk', 'kwkwk', 'kwwwk', '.kkk.', '..k..'], { x, y, px, op, pal: { w: '#fff', k: EYE } });
export const DOTS = (x, y, px = 6, n = 3, op = 1) => n < 1 ? '' : pixart(['ww.ww.ww'.slice(0, n * 3 - 1), 'ww.ww.ww'.slice(0, n * 3 - 1)], { x, y, px, op, pal: { w: '#F3F1EC' } });
export const SPARK = (x, y, px = 6, col = YEL, op = 1) => pixart(['..y..', '..y..', 'yy.yy', '..y..', '..y..'], { x: x - 2.5 * px, y: y - 2.5 * px, px, op, pal: { y: col } });
export const HEART = (x, y, px = 5, op = 1) => pixart(['.rr.rr.', 'rrrrrrr', 'rrrrrrr', '.rrrrr.', '..rrr..', '...r...'], { x: x - 3.5 * px, y: y - 3 * px, px, op, pal: { r: '#FF5A4E' } });
export const CHECK = (x, y, px = 6, op = 1) => pixart(['.....g', '....gg', 'g..gg.', 'gggg..', '.gg...'], { x, y, px, op, pal: { g: '#3DD68C' } });
export const ZED = (x, y, px = 5, op = 1) => pixart(['zzzz', '..z.', '.z..', 'zzzz'], { x, y, px, op, pal: { z: '#F3F1EC' } });
export function dust(x, y, t, px = 5, n = 6, seed = 1) {
  if (t <= 0 || t >= 1) return ''; let s = '';
  for (let i = 0; i < n; i++) {
    const side = i % 2 ? 1 : -1, k = (i >> 1) + 1, e = 1 - Math.pow(1 - t, 2);
    const dx = side * (14 + k * 13 + ((seed * 7 + i * 13) % 9)) * e, dy = -(7 + k * 4) * Math.sin(Math.PI * t * .9), sz = px * (1 - t * .7);
    s += `<div style="position:absolute;left:${x + dx - sz / 2}px;top:${y + dy - sz}px;width:${sz}px;height:${sz}px;background:#CFE9E4;opacity:${1 - t}"></div>`;
  }
  return s;
}
export function speedlines(x, y, dir, t, px = 5) {
  if (t <= 0 || t >= 1) return ''; let s = '';
  for (let i = 0; i < 4; i++) {
    const len = (3 + (i % 2) * 3) * px, off = (i * 23 + 11) % 50 - 25;
    s += `<div style="position:absolute;left:${x - dir * (34 + i * 12) - (dir > 0 ? len : 0)}px;top:${y - 36 + off}px;width:${len}px;height:${px}px;background:${BODY};opacity:${(1 - t) * .75}"></div>`;
  }
  return s;
}
