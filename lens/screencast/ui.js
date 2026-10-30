// Pure 2D redraw of the Lens desktop app (dark theme, from the real 1.3.0 UI) + the desktop around it. state → HTML string, deterministic.
export const WIN = { x: 120, y: 84, w: 1680, h: 888 };
const TB = 36, SIDE = 270, RIGHT = 448;
// world-space geometry used by the camera plan (anchors used by the mascot are measured from the DOM)
export const G = {
  cx: WIN.x + SIDE + (WIN.w - SIDE - RIGHT) / 2,            // centre of the preview column
  vid: (() => { const sx = WIN.x + SIDE + 18, sw = WIN.w - SIDE - RIGHT - 36, sy = WIN.y + TB + 18, sh = WIN.h - TB - 36, vh = sw * 9 / 16; return { x: sx, y: sy + (sh - vh) / 2, w: sw, h: vh, sy, sh }; })(),
  right: WIN.x + WIN.w - RIGHT,
};
const st = (o = {}) => { let s = ''; if (o.op !== undefined && o.op < 1) s += `opacity:${Math.max(0, o.op).toFixed(3)};`; if (o.tf) s += `transform:${o.tf};`; return s; };

// ───────── icons (stroke, currentColor)
const sv = (d, w = 18) => `<svg width="${w}" height="${w}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
export const IC = {
  phone: sv('<rect x="6.5" y="2.5" width="11" height="19" rx="2.5"/><path d="M11.5 18h1"/>'),
  camera: sv('<path d="M3 8.5A1.5 1.5 0 0 1 4.5 7h2.8l1.6-2h6.2l1.6 2h2.8A1.5 1.5 0 0 1 21 8.5v9A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z"/><circle cx="12" cy="13" r="3.3"/>'),
  effects: sv('<path d="M9 3l1.6 4.4L15 9l-4.4 1.6L9 15l-1.6-4.4L3 9l4.4-1.6z"/><path d="M17 13l.9 2.1L20 16l-2.1.9L17 19l-.9-2.1L14 16l2.1-.9z"/><path d="M18 3v3M16.5 4.5h3"/>'),
  output: sv('<rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M8 20h8M12 16v4"/>'),
  sound: sv('<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>'),
  settings: sv('<circle cx="12" cy="12" r="3"/><path d="M12 2.5v2.2M12 19.3v2.2M4.6 4.6l1.6 1.6M17.8 17.8l1.6 1.6M2.5 12h2.2M19.3 12h2.2M4.6 19.4l1.6-1.6M17.8 6.2l1.6-1.6"/>'),
  info: sv('<path d="M3 12h4l2.5-6 4 12L16 12h5"/>'),
  pause: sv('<path d="M9 5.5v13M15 5.5v13"/>', 20), play: sv('<path d="M7.5 5l11 7-11 7z" fill="currentColor"/>', 20),
  blur: sv('<path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16"/><circle cx="12" cy="12" r="3.2"/>', 20),
  frame: sv('<path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16"/><path d="M9 10h.01M15 10h.01M9.5 14.5c1.4 1.2 3.6 1.2 5 0"/>', 20),
  mirror: sv('<path d="M12 3v18M9 7l-5 5 5 5zM15 7l5 5-5 5z"/>', 20),
  rotate: sv('<path d="M20 12a8 8 0 1 1-2.6-5.9M20 4v4.5h-4.5"/>', 20),
  mic: sv('<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 10.2 5.3M18.4 12.5c.1-.5.1-1 .1-1.5M12 17.5V21M4 4l16 16"/>', 20),
  torch: sv('<path d="M7 3h10v4l-2.5 3.5V21h-5V10.5L7 7z"/><path d="M7 7h10"/>', 20),
  check: '<svg width="13" height="13" viewBox="0 0 14 14"><path d="M3 7.4l2.6 2.6L11 4.3" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};
export const DOT = (s = 22) => `<svg width="${s}" height="${s}" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="#FF6A3D"/><circle cx="12" cy="12" r="4.6" fill="#1B1A1C"/><circle cx="13.6" cy="10.4" r="1.4" fill="#FF6A3D" opacity=".9"/></svg>`;

// ───────── desktop: wallpaper, taskbar, other windows, toast, end card
export function desk(D) {
  let h = `<img id="wall" src="assets/wall.jpg">`;
  h += `<div id="taskbar"><div class="tbc"><i class="win4"><b></b><b></b><b></b><b></b></i><i class="tbi"></i><i class="tbi"></i><i class="tbi lens ${D.lensOn ? 'on' : ''}">${DOT(22)}</i><i class="tbi"></i></div><div class="tbr">ENG&nbsp;&nbsp;&nbsp;9:41</div></div>`;
  return h;
}
export function callWin(c) {
  // a generic video-call window (no logos, no copied UI): title, the camera feed it receives, its camera picker
  return `<div class="cwin" data-a="cw-${c.k}" style="left:${c.x}px;top:${c.y}px;${st(c)}">
    <div class="ctb" data-a="ct-${c.k}"><span class="cdot"></span>${c.name}</div>
    <div class="cbody"><canvas class="vid" data-v="1" width="384" height="216"></canvas>${c.brb ? `<div class="brb sm"><span>${IC.pause}</span>Be right back</div>` : ''}</div>
    <div class="cbar"><span>Camera</span><span class="csel ${c.hot ? 'hot' : ''}">${c.hot ? `<i>${IC.check}</i>` : ''}Lens<em>▾</em></span></div></div>`;
}
export function toast(T) {
  return `<div class="toast2" data-a="toast" style="${st(T)}"><div class="t2h">${DOT(16)}<span>Lens</span><em>now</em></div>
    <div class="t2b" data-a="toast-t">Free and open source</div><div class="t2s" data-a="toast-s">github.com/MRRengi/rng-lens</div></div>`;
}

// ───────── the Lens window
export function lensWin(S, t) {
  return `<div class="win" style="left:${WIN.x}px;top:${WIN.y}px;width:${WIN.w}px;height:${WIN.h}px;${st(S.winSt)}">
    <div class="ttl"><span class="tic">${DOT(14)}</span>Lens<span class="wc"><i>&#8211;</i><i>&#9744;</i><i>&#10005;</i></span></div>
    <div class="cols">${sidebar(S)}${center(S, t)}${panel(S, t)}</div></div>`;
}
const NAV = [['phone', 'Phone'], ['camera', 'Camera'], ['effects', 'Effects'], ['output', 'Output'], ['sound', 'Sound'], ['settings', 'Settings'], ['info', 'Info']];
function sidebar(S) {
  const dev = S.connected
    ? `<div class="devn">Pixel 8</div><div class="devs"><i class="gd"></i>Connected</div><div class="devm"><span><svg width='14' height='10' viewBox='0 0 14 10'><rect x='.7' y='.7' width='11' height='8.6' rx='2' fill='none' stroke='currentColor' stroke-width='1.2'/><rect x='2.2' y='2.2' width='6.6' height='5.6' rx='.8' fill='currentColor'/><rect x='12.2' y='3.3' width='1.4' height='3.4' rx='.6' fill='currentColor'/></svg> 77%</span><span><svg width='8' height='12' viewBox='0 0 8 12'><path d='M2.6 1.6a1.4 1.4 0 0 1 2.8 0v5.3a2.7 2.7 0 1 1-2.8 0z' fill='none' stroke='currentColor' stroke-width='1.1'/></svg> 32°C</span><span>${S.uptime}</span></div>`
    : `<div class="devn">No phone</div><div class="devs"><i></i>Not connected</div>`;
  return `<div class="side"><div class="brand" data-a="brand"><b>Lens</b><span>1.3.0</span></div>
    <div class="dev ${S.connected ? 'on' : ''}" data-a="dev">${dev}</div>
    <div class="nav">${NAV.map(([k, n]) => `<div class="ni ${S.nav === k ? 'on' : ''} ${S.navHov === k ? 'hov' : ''}" data-a="nav-${k}"><span class="nic">${IC[k]}</span>${n}${k === 'effects' && S.fxOn ? '<em>On</em>' : ''}</div>`).join('')}</div></div>`;
}
function center(S, t) {
  let h = '';
  if (!S.connected || S.emptyOp > 0) {
    const E = S.empty || {};
    h += `<div class="empty" style="${st({ op: S.emptyOp ?? 1 })}">
      <div class="eic" data-a="es-icon" style="${E.jit ? `transform:translate(${Math.sin(t * 61) * E.jit}px,${Math.cos(t * 47) * E.jit * .6}px) rotate(${Math.sin(t * 39) * E.jit * 1.4}deg)` : ''}">${E.gone ? '<svg width="44" height="58" viewBox="0 0 44 58"><rect x="2" y="2" width="40" height="54" rx="9" fill="none" stroke="rgba(255,106,61,.45)" stroke-width="2" stroke-dasharray="5 5"/></svg>' : PHONE_ICON(E.op ?? 1)}</div>
      <div class="eh" data-a="es-h">Connect your phone</div>
      <div class="es">Open Lens on the phone, on the same Wi-Fi.<br>It shows up under Phone. Or plug in USB with USB debugging on.</div></div>`;
  }
  if (S.connected) {
    const V = S.vidUI || {};
    h += `<div class="vwrap" data-a="vid" style="left:${G.vid.x - WIN.x - SIDE}px;top:${G.vid.y - WIN.y - TB}px;width:${G.vid.w}px;height:${G.vid.h}px;${st({ op: S.vidOp ?? 1 })}">
      <canvas class="vid" data-v="1" width="960" height="540"></canvas>
      ${V.af ? afBrackets(V.af) : ''}
      ${V.brb ? `<div class="brb" data-a="brb" style="${st({ op: V.brb, tf: `translate(-50%,-50%) scale(${.9 + .1 * V.brb})` })}"><span>${IC.pause}</span>Be right back</div>` : ''}
      <div class="badge" data-a="badge"><i></i>LIVE&nbsp;&nbsp;1920x1080&nbsp;&nbsp;30 fps</div>
      <div class="tbar2" data-a="tbar">${[['pause', V.paused ? 'play' : 'pause', V.paused], ['blur', 'blur', V.blur], ['frame', 'frame', V.frame], ['mirror', 'mirror'], ['rotate', 'rotate'], ['mic', 'mic'], ['torch', 'torch']]
        .map(([k, ic, on]) => `<span class="tb ${on ? 'on' : ''} ${V.hov === k ? 'hov' : ''}" data-a="tb-${k}">${IC[ic]}${V.tip && V.tip.key === k ? `<span class="tip" style="opacity:${V.tip.op}">${V.tip.text}</span>` : ''}</span>`).join('')}</div>
    </div>`;
  }
  return `<div class="ctr"><div class="stage" data-a="stage">${h}</div></div>`;
}
const PHONE_ICON = (op = 1) => `<svg width="44" height="58" viewBox="0 0 44 58" style="opacity:${op}"><rect x="3" y="3" width="38" height="52" rx="8" fill="none" stroke="#FF6A3D" stroke-width="3.4"/><circle cx="22" cy="45" r="2.4" fill="#FF6A3D"/></svg>`;
function afBrackets(A) {
  // four orange corner brackets around the region auto-frame keeps (video-local px)
  const L = 26, s = `opacity:${A.op};`; const c = (k, x, y, d) => `<i class="afb" data-a="af-${k}" style="${s}left:${x}px;top:${y}px;border-width:${d}"></i>`;
  return c('tl', A.x, A.y, '3px 0 0 3px') + c('tr', A.x + A.w - L, A.y, '3px 3px 0 0') + c('bl', A.x, A.y + A.h - L, '0 0 3px 3px') + c('br', A.x + A.w - L, A.y + A.h - L, '0 3px 3px 0');
}
function panel(S, t) {
  let h = '';
  if (S.panel === 'phone0') {
    const F = S.found;
    h = `<div class="ph">Phone</div><div class="sec" data-a="sec-wifi">Phones on this Wi-Fi</div>
      <div class="card" data-a="card-wifi">${F ? `<div class="row" data-a="row-pixel" style="${st({ op: Math.min(1, F.p * 2), tf: `translateY(${(1 - F.p) * 10}px)` })}"><span>Pixel 8</span><span class="btn acc ${F.press ? 'dn' : ''}" data-a="btn-connect">${F.busy ? '<i class="spin"></i>Connecting' : 'Connect'}</span></div>`
        : `<div class="row mute"><i class="spin" style="transform:rotate(${t * 360}deg)"></i>Looking for phones…</div>`}</div>
      <div class="sec">Other ways</div>
      <div class="card"><div class="row"><span>USB cable</span><span class="btn">Connect</span></div>
        <div class="row"><span class="fld">Phone address, e.g. 192.168.1.20</span><span class="btn">Connect</span></div>
        <div class="row"><span>Reconnect automatically</span><span class="tog"><i></i></span></div></div>`;
  } else if (S.panel === 'phone1') {
    h = `<div class="ph">Phone</div><div class="sec">This phone</div>
      <div class="card"><div class="row"><span>Phone</span><span class="v">Pixel 8</span></div><div class="row"><span>Address</span><span class="v">192.168.1.42</span></div>
        <div class="row"><span>Battery</span><span class="v">77%</span></div><div class="row"><span>Temperature</span><span class="v">32°C</span></div>
        <div class="row"><span>Streaming for</span><span class="v">${S.uptime}</span></div></div>
      <div class="card" style="margin-top:14px"><div class="row"><span>Disconnect</span><span class="btn">Disconnect</span></div></div>`;
  } else if (S.panel === 'output') {
    h = `<div class="ph">Output</div><div class="sec">Virtual camera</div>
      <div class="card" data-a="card-vc"><div class="row" data-a="row-cam"><span>Camera "Lens"</span><span class="tog on"><i></i></span></div>
        <div class="row"><span>Size</span><span class="sel">1920 x 1080<b>⌄</b></span></div>
        <div class="row"><span>Frame rate</span><span class="seg2"><span class="on">30 fps</span><span>60 fps</span></span></div></div>
      <div class="foot" data-a="foot">An app is using the camera (${S.frames} frames sent).</div>
      <div class="sec">Orientation</div>
      <div class="card"><div class="row"><span>Auto-rotate</span><span class="tog"><i></i></span></div><div class="row"><span>Mirror</span><span class="tog"><i></i></span></div>
        <div class="row"><span>Flip upside down</span><span class="tog"><i></i></span></div><div class="row"><span>Rotate</span><span class="pill2">90°</span></div></div>
      <div class="foot">Mirror only changes what you send. Most call apps mirror your self-view on their own.</div>`;
  }
  return `<div class="rp" data-a="rp">${h}</div>`;
}
