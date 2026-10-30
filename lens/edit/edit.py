"""Edits the Lens take into social videos, paced by the narration.

python3 edit.py <take-dir> <voice-dir> <out-dir> [--pick 1] [--fast] [16x9] [1x1] [--still t1,t2,...]
"""
import bisect, json, math, subprocess, sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).parent
FPS = 60
FD = Path("/usr/share/fonts/opentype/inter")
F_SEMI, F_BOLD, F_MED = str(FD / "Inter-SemiBold.otf"), str(FD / "Inter-Bold.otf"), str(FD / "Inter-Medium.otf")
F_BLACK, F_DMED = str(FD / "InterDisplay-Black.otf"), str(FD / "InterDisplay-Medium.otf")
ACCENT = (255, 106, 61)  # Lens orange (RGB)
STOCK = Path("/tmp/claude-0/media/results/stock")
TB = 48  # title bar height in window pixels (1.5x scale)
RAW_W, RAW_H = 2100, 1000
WIN_W, WIN_H = RAW_W, RAW_H + TB


def ease(t):
    t = min(1.0, max(0.0, t))
    return t * t * t * (t * (t * 6 - 15) + 10)


def ease_out(t):
    t = min(1.0, max(0.0, t))
    return 1 - (1 - t) ** 3


_fonts = {}


def font(p, s):
    k = (p, int(s))
    if k not in _fonts:
        _fonts[k] = ImageFont.truetype(p, int(s))
    return _fonts[k]


# ---------------------------------------------------------------- take


class Take:
    def __init__(self, d: Path):
        self.d = d
        self.beats = []
        for line in (d / "beats.txt").read_text().splitlines():
            p = line.split(" ", 2)
            self.beats.append((int(p[0]), p[1], p[2].strip() if len(p) > 2 else ""))
        cur = [tuple(map(int, l.split())) for l in (d / "cursor.txt").read_text().splitlines() if l.strip()]
        self.ct, self.cx, self.cy = [c[0] for c in cur], [c[1] for c in cur], [c[2] for c in cur]
        w0, m0, self.T0, self.K = map(float, (d / "clock.txt").read_text().split())
        self.off = w0 - m0
        out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries", "packet=pts_time", "-of", "csv=p=0", str(d / "raw.mkv")],
                             capture_output=True, text=True).stdout
        self.pts = sorted(float(x) for x in out.split() if x.strip())
        self.clicks = [(ms, x) for ms, k, x in self.beats if k == "click"]

    def click(self, name):
        return next(ms for ms, x in self.clicks if x == name)

    def beat(self, kind, text=""):
        return next(ms for ms, k, x in self.beats if k == kind and x.startswith(text))

    def frame_index(self, ms):
        wall = self.T0 + ms / 1000.0 * self.K + self.off
        return max(0, bisect.bisect_right(self.pts, wall) - 1)

    def cursor(self, ms):
        i = bisect.bisect_left(self.ct, ms)
        if i <= 0:
            return self.cx[0], self.cy[0]
        if i >= len(self.ct):
            return self.cx[-1], self.cy[-1]
        t0, t1 = self.ct[i - 1], self.ct[i]
        a = (ms - t0) / max(1, t1 - t0)
        return self.cx[i - 1] + (self.cx[i] - self.cx[i - 1]) * a, self.cy[i - 1] + (self.cy[i] - self.cy[i - 1]) * a


class Frames:
    """Sequential reader of the raw take (requests only move forward, with one allowed cut)."""

    def __init__(self, path):
        self.path = path
        self.cap = cv2.VideoCapture(str(path))
        self.pos, self.cur = -1, None

    def get(self, idx):
        if idx < self.pos:
            self.cap.release()
            self.cap = cv2.VideoCapture(str(self.path))
            self.pos, self.cur = -1, None
        while self.pos < idx:
            ok, f = self.cap.read()
            if not ok:
                break
            self.cur, self.pos = f, self.pos + 1
        return self.cur


# ---------------------------------------------------------------- plan (voice-driven)


class Plan:
    """Maps output time to take time. Pieces are anchored to narration words; silence is inserted between
    sentences when an action needs more time than the voice gives it."""

    SMAX, SMIN = 1.55, 0.72

    def __init__(self, tk, words, narr_s):
        self.tk = tk
        self.W = [w for w in words if w.get("type") == "word"]
        self.narr_s = narr_s
        self.inserts = []  # (at_s in original narration, dur)
        for _ in range(6):
            if not self.build():
                break

    def N(self, t):
        return t + sum(d for a, d in self.inserts if a <= t + 1e-6)

    def ws(self, i):
        return self.N(self.W[i]["start"])

    def we(self, i):
        return self.N(self.W[i]["end"])

    def gap_before(self, i):
        """Insert point: the middle of the pause before word i (original narration time)."""
        return (self.W[i - 1]["end"] + self.W[i]["start"]) / 2

    def build(self):
        tk = self.tk
        c = tk.click
        found = tk.beat("scene", "found")
        self.intro_end = self.we(17) + 0.05  # app full shot after "PC."
        # (app_ms_start, app_ms_end, out_start, out_end, word index whose start pins out_end)
        P = []
        def piece(a0, a1, o0, wi, off):
            o1 = self.ws(wi) + off
            P.append([a0, a1, o0, o1, wi])
            return o1
        o = self.intro_end - 0.45
        a_start = c("connect") - int((self.ws(26) - 0.05 - o) * 1000)  # footage runs at 1x up to the connect click
        a_start = max(a_start, found - 2500)
        o = piece(a_start, c("connect"), o, 26, -0.05)
        o = piece(c("connect"), c("output"), o, 34, 0.12)
        o = piece(c("output"), c("blur"), o, 46, 0.04)
        o = piece(c("blur"), c("autoframe"), o, 53, 0.0)
        o = piece(c("autoframe"), c("pause"), o, 66, 0.0)
        o = piece(c("pause"), c("resume"), o, 72, 0.62)
        self.cut_at = o + 0.65
        o2 = self.ws(73) - 0.12
        hold = max(0.65, o2 - o)
        P.append([c("resume"), c("resume") + int(hold * 1000), o, o + hold, None])
        P.append([c("res") + 280, c("4k"), o2, self.ws(76) + 0.2, 76])
        o = self.ws(76) + 0.2
        o = piece(c("4k"), c("60"), o, 81, 0.25)
        self.end_at = self.ws(89) - 0.3
        P.append([c("60"), c("60") + int((self.end_at + 0.5 - o) * 1000), o, self.end_at + 0.5, None])
        self.pieces = P
        # Inserts: where footage would need to run too fast, add silence before the pinned word.
        changed = False
        for a0, a1, o0, o1, wi in P:
            if wi is None:
                continue
            need = (a1 - a0) / 1000.0 / self.SMAX
            if o1 - o0 < need - 0.02:
                self.inserts.append((self.gap_before(wi), need - (o1 - o0) + 0.05))
                changed = True
        if self.cut_at > o2 - 0.25 + 1e-3:
            self.inserts.append((self.gap_before(73), self.cut_at - (o2 - 0.25) + 0.05))
            changed = True
        if changed:
            merged = {}
            for a, d in self.inserts:
                merged[round(a, 3)] = merged.get(round(a, 3), 0) + d
            self.inserts = sorted(merged.items())
        return changed

    def app_ms(self, t):
        for a0, a1, o0, o1, _ in self.pieces:
            if o0 <= t < o1:
                return a0 + (a1 - a0) * (t - o0) / (o1 - o0)
        last = self.pieces[-1]
        return last[1] if t >= last[3] else self.pieces[0][0]

    def report(self):
        for a0, a1, o0, o1, wi in self.pieces:
            print(f"  out {o0:6.2f}-{o1:6.2f}  app {a0:6d}-{int(a1):6d}  speed {(a1 - a0) / 1000 / (o1 - o0):.2f}  {self.W[wi]['text'] if wi is not None else ''}")
        print("  inserts", [(round(a, 2), round(d, 2)) for a, d in self.inserts])


# ---------------------------------------------------------------- drawing helpers


def rounded_mask(w, h, r):
    m = Image.new("L", (w, h), 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, w - 1, h - 1], r, fill=255)
    return np.array(m)


def pill_img(text, size, fg=(255, 255, 255), bg=(18, 16, 20, 225), fp=F_SEMI, padx=None, pady=None, dot=None):
    f = font(fp, size)
    l, t, r, b = f.getbbox(text)
    padx = padx or int(size * 0.8)
    pady = pady or int(size * 0.5)
    dw = int(size * 0.75) if dot else 0
    w, h = r - l + 2 * padx + dw, int(size * 1.0) + 2 * pady
    im = Image.new("RGBA", (w + 60, h + 60), (0, 0, 0, 0))
    sh = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(sh).rounded_rectangle([30, 36, 30 + w, 36 + h], h // 2, fill=(0, 0, 0, 120))
    sh = sh.filter(ImageFilter.GaussianBlur(14))
    im.alpha_composite(sh)
    d = ImageDraw.Draw(im)
    d.rounded_rectangle([30, 30, 30 + w, 30 + h], h // 2, fill=bg)
    x = 30 + padx
    if dot:
        rr = int(size * 0.2)
        cy = 30 + h // 2
        d.ellipse([x, cy - rr, x + 2 * rr, cy + rr], fill=dot)
        x += dw
    d.text((x - l, 30 + (h - size) // 2 - int(size * 0.07) - t + int(size * 0.07)), text, font=f, fill=fg)
    return im


def caption_img(text, size, maxw):
    """Caption pill; wraps to two lines when wider than maxw."""
    f = font(F_SEMI, size)
    lines = [text]
    if f.getlength(text) + size * 1.6 > maxw:
        words = text.split(" ")
        best = None
        for k in range(1, len(words)):
            a, b = " ".join(words[:k]), " ".join(words[k:])
            m = max(f.getlength(a), f.getlength(b))
            if best is None or m < best[0]:
                best = (m, [a, b])
        lines = best[1]
    lw = int(max(f.getlength(x) for x in lines))
    lh = int(size * 1.28)
    padx, pady = int(size * 0.85), int(size * 0.5)
    w, h = lw + 2 * padx, lh * len(lines) + 2 * pady - int(size * 0.2)
    r = min(h // 2, int(size * 1.0))
    im = Image.new("RGBA", (w + 80, h + 80), (0, 0, 0, 0))
    sh = Image.new("RGBA", im.size, (0, 0, 0, 0))
    ImageDraw.Draw(sh).rounded_rectangle([40, 48, 40 + w, 48 + h], r, fill=(0, 0, 0, 130))
    im.alpha_composite(sh.filter(ImageFilter.GaussianBlur(18)))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle([40, 40, 40 + w, 40 + h], r, fill=(16, 14, 18, 228))
    asc = f.getbbox("Hg")[1]
    for k, line in enumerate(lines):
        lx = 40 + (w - f.getlength(line)) / 2
        d.text((lx, 40 + pady + k * lh - asc + int(size * 0.02)), line, font=f, fill=(255, 255, 255))
    return im


def blend(dst, src, x, y, alpha=1.0):
    """Alpha-blend RGBA PIL/np image onto BGR np dst at x,y (top-left)."""
    if isinstance(src, Image.Image):
        a = np.array(src)
    else:
        a = src
    h, w = a.shape[:2]
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(dst.shape[1], x + w), min(dst.shape[0], y + h)
    if x1 <= x0 or y1 <= y0:
        return
    s = a[y0 - y:y1 - y, x0 - x:x1 - x]
    al = s[..., 3:4].astype(np.float32) / 255.0 * alpha
    rgb = s[..., :3][..., ::-1].astype(np.float32)
    d = dst[y0:y1, x0:x1].astype(np.float32)
    dst[y0:y1, x0:x1] = (d * (1 - al) + rgb * al).astype(np.uint8)


def make_cursor(k=1.5):
    poly = [(0, 0), (0, 17), (4.2, 13.2), (7.2, 19.8), (9.6, 18.8), (6.6, 12.4), (12, 12.2)]
    pad = 8
    W, H = int(16 * k) + 2 * pad, int(24 * k) + 2 * pad
    im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    sh = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    P = [(pad + x * k, pad + y * k) for x, y in poly]
    ImageDraw.Draw(sh).polygon([(x + 1.0 * k, y + 1.8 * k) for x, y in P], fill=(0, 0, 0, 110))
    im.alpha_composite(sh.filter(ImageFilter.GaussianBlur(2.2)))
    ImageDraw.Draw(im).polygon(P, fill=(255, 255, 255, 255), outline=(14, 14, 18, 255), width=max(1, int(1.4 * k)))
    return np.array(im), pad


# ---------------------------------------------------------------- the renderer


class Renderer:
    def __init__(self, tk, plan, W, H, square):
        self.tk, self.plan, self.W, self.H, self.sq = tk, plan, W, H, square
        self.s = W / 1920 if not square else 1.0
        wall = cv2.imread(str(HERE / "wallpaper.png"))
        if square:
            wall = wall[:, 840:840 + 2160]
        self.wall = cv2.resize(wall, (int(W * 1.12), int(H * 1.12)), interpolation=cv2.INTER_AREA)
        self.frames = Frames(tk.d / "raw.mkv")
        self.cursor, self.cpad = make_cursor(1.5)
        self.titlebar = self.make_titlebar()
        m = rounded_mask(WIN_W, WIN_H, 13)
        self.win_mask = m
        shadow = np.zeros((WIN_H + 240, WIN_W + 240), np.uint8)
        shadow[120 + 18:120 + 18 + WIN_H, 120:120 + WIN_W] = m
        self.shadow = cv2.GaussianBlur(shadow, (0, 0), 38)
        self.cap_cache = {}
        self.cam = self.camera_keys()

    # ---- window chrome (Windows 11 dark title bar as the real app shows on Windows)
    def make_titlebar(self):
        im = Image.new("RGB", (WIN_W, TB), (32, 32, 32))
        d = ImageDraw.Draw(im)
        ic = Image.open("/home/claude/rng-lens/dist/rng-lens-icon-512.png").convert("RGBA").resize((24, 24), Image.LANCZOS)
        im.paste(ic, (18, 12), ic)
        d.text((54, 12), "Lens", font=font(F_MED, 18), fill=(235, 235, 235))
        bw = 69
        x = WIN_W - 3 * bw
        c = (230, 230, 230)
        cy = TB // 2
        d.line([(x + 29, cy), (x + 40, cy)], fill=c, width=1)  # minimize
        x += bw
        d.rectangle([x + 29, cy - 6, x + 40, cy + 5], outline=c, width=1)  # maximize
        x += bw
        d.line([(x + 29, cy - 6), (x + 40, cy + 5)], fill=c, width=1)
        d.line([(x + 29, cy + 5), (x + 40, cy - 6)], fill=c, width=1)
        return cv2.cvtColor(np.array(im), cv2.COLOR_RGB2BGR)

    # ---- camera keyframes: (out_t, cx, cy, view_width, transition_s); window coords (title bar included)
    def camera_keys(self):
        p = self.plan
        c = lambda n: None
        full = (1050, WIN_H / 2, 2330)
        prev = (939, 499 + TB, 1500)
        K = [
            (0.0, *full, 0.0),
            (p.intro_end - 0.45, *full, 0.0),
            (p.ws(18) + 0.15, 1790, 205 + TB, 1250, 1.0),         # "Open Lens on the phone": phones on this Wi-Fi
            (p.ws(26) + 0.25, *full, 0.85),                       # connected: the whole app
            (p.ws(30) - 0.2, 720, 470 + TB, 1720, 1.1),            # Wi-Fi or USB: phone card + live preview
            (p.ws(34) - 0.55, *full, 0.6),                        # click Output
            (p.ws(37), 1815, 250 + TB, 1120, 1.0),                 # camera "Lens" + footer
            (p.ws(46) - 0.7, 939, 520 + TB, 1500, 0.8),                       # blur
            # blur, auto-frame and pause are one idea (the picture): the camera holds still on it
            (p.ws(73) - 0.12, 1860, 300 + TB, 1180, 0.0),          # cut: resolution list
            (p.ws(81) + 0.55, *full, 1.3),            # one slow pull-back: no ads, no watermark
        ]
        return K

    def camera(self, t):
        K = self.cam
        cur = K[0][1:4]
        for i in range(1, len(K)):
            tk, x, y, vw, tr = K[i]
            if t < tk:
                break
            prevv = cur
            if tr <= 0 or t >= tk + tr:
                cur = (x, y, vw)
            else:
                e = ease((t - tk) / tr)
                # zoom in log space so it feels even
                lv = math.exp(math.log(prevv[2]) + (math.log(vw) - math.log(prevv[2])) * e)
                cur = (prevv[0] + (x - prevv[0]) * e, prevv[1] + (y - prevv[1]) * e, lv)
                break
        x, y, vw = cur
        if self.sq:
            vw = vw * 0.56
            if t >= self.plan.intro_end - 0.5:
                xs = [self.tk.cursor(self.plan.app_ms(t + 0.15 * k))[0] for k in range(1, 8)]
                span = [self.tk.cursor(self.plan.app_ms(t + 0.2 * k))[0] for k in range(-6, 8)]
                wgt = 0.6 * min(1.0, (max(span) - min(span)) / 260.0)
                if t > self.plan.ws(81) + 0.5 or cur[2] < 2000:
                    wgt = 0.0  # only full shots follow the cursor; close-ups hold still
                x = (1 - wgt) * x + wgt * (sum(xs) / len(xs))
        vh = vw * self.H / self.W
        m = 0.035 * vw
        if vw >= WIN_W + 2 * m:
            x = WIN_W / 2
        else:
            x = min(max(x, vw / 2 - m), WIN_W - vw / 2 + m)
        if vh >= WIN_H + 2 * m:
            y = WIN_H / 2
        else:
            y = min(max(y, vh / 2 - m), WIN_H - vh / 2 + m)
        return x, y, vw

    # ---- one footage frame
    def window_frame(self, t):
        ms = self.plan.app_ms(t)
        raw = self.frames.get(self.tk.frame_index(ms))
        win = np.empty((WIN_H, WIN_W, 3), np.uint8)
        win[:TB] = self.titlebar
        win[TB:] = raw
        # cursor
        x, y = self.tk.cursor(ms)
        y += TB
        # click rings (in output time, so they look the same at any speed)
        for cms, name in self.tk.clicks:
            ct = self.out_time_of(cms)
            if ct is None:
                continue
            u = (t - ct) / 0.5
            if 0 <= u <= 1:
                cxy = self.tk.cursor(cms)
                r = int(14 + 34 * ease_out(u))
                a = 1 - u
                ov = win.copy()
                col = (ACCENT[2], ACCENT[1], ACCENT[0])
                cv2.circle(ov, (int(cxy[0]), int(cxy[1] + TB)), r, col, -1, cv2.LINE_AA)
                cv2.addWeighted(ov, 0.28 * a, win, 1 - 0.28 * a, 0, win)
                cv2.circle(win, (int(cxy[0]), int(cxy[1] + TB)), r, col, 3, cv2.LINE_AA)
        # press: cursor shrinks a touch around a click
        sc = 1.0
        for cms, _ in self.tk.clicks:
            ct = self.out_time_of(cms)
            if ct is not None and abs(t - ct) < 0.12:
                sc = 0.88
        cur = self.cursor if sc == 1.0 else cv2.resize(self.cursor, None, fx=sc, fy=sc, interpolation=cv2.INTER_AREA)
        pad = int(self.cpad * sc)
        blend(win, cur, int(x) - pad, int(y) - pad)
        return win

    def out_time_of(self, ms):
        for a0, a1, o0, o1, _ in self.plan.pieces:
            if a0 <= ms <= a1 and a1 > a0:
                return o0 + (ms - a0) / (a1 - a0) * (o1 - o0)
        return None

    def compose(self, t, win):
        W, H = self.W, self.H
        cx, cy, vw = self.camera(t)
        s = W / vw
        # wallpaper with a little parallax
        ww = self.wall
        z = (2330 / (vw / (0.56 if self.sq else 1))) ** 0.12
        bw, bh = int(W * z), int(H * z)
        ox = (ww.shape[1] - W) // 2
        oy = (ww.shape[0] - H) // 2
        canvas = ww[oy:oy + H, ox:ox + W].copy()
        if z > 1.001:
            canvas = cv2.resize(canvas, (bw, bh), interpolation=cv2.INTER_LINEAR)[(bh - H) // 2:(bh - H) // 2 + H, (bw - W) // 2:(bw - W) // 2 + W]
        M = np.float32([[s, 0, W / 2 - cx * s], [0, s, H / 2 - cy * s]])
        Ms = np.float32([[s, 0, W / 2 - (cx + 120) * s], [0, s, H / 2 - (cy + 120) * s]])
        sh = cv2.warpAffine(self.shadow, Ms, (W, H), flags=cv2.INTER_LINEAR, borderValue=0).astype(np.float32) / 255 * 0.62
        canvas = (canvas.astype(np.float32) * (1 - sh[..., None])).astype(np.uint8)
        wimg = cv2.warpAffine(win, M, (W, H), flags=cv2.INTER_AREA if s < 1 else cv2.INTER_CUBIC, borderValue=0)
        wm = cv2.warpAffine(self.win_mask, M, (W, H), flags=cv2.INTER_LINEAR, borderValue=0).astype(np.float32)[..., None] / 255
        out = (canvas * (1 - wm) + wimg * wm).astype(np.uint8)
        return out

    # ---- captions
    def captions(self):
        p = self.plan
        return [
            (p.ws(0) - 0.05, p.we(8) + 0.35, "Your phone has a better camera than your laptop."),
            (p.ws(9) + 0.05, p.we(17) + 0.3, "Lens turns it into a webcam for your PC."),
            (p.ws(18) - 0.05, p.we(29) + 0.3, "Open Lens on the phone. The PC finds it by itself."),
            (p.ws(30) - 0.05, p.we(33) + 0.35, "Over Wi-Fi or USB."),
            (p.ws(34) - 0.05, p.we(39) + 0.25, "Zoom, Teams, OBS, Discord..."),
            (p.ws(40) - 0.05, p.we(45) + 0.3, "...just pick the camera called “Lens”."),
            (p.ws(46) - 0.05, p.we(51) + 0.3, "Blur the background with one click."),
            (p.ws(52) - 0.05, p.we(61) + 0.3, "Auto-frame follows your face."),
            (p.ws(62) - 0.05, p.we(64) + 0.25, "Need a minute?"),
            (p.ws(65) - 0.05, p.we(72) + 0.4, "Hit pause. Everyone sees “Be right back”."),
            (p.ws(73) - 0.05, p.we(81) + 0.3, "Up to 4K at 60 fps."),
            (p.ws(82) - 0.05, p.ws(84) - 0.05, "No ads."),
            (p.ws(84) - 0.05, p.ws(86) - 0.05, "No ads. No watermark."),
            (p.ws(86) - 0.05, p.end_at + 0.15, "No ads. No watermark. No pro version."),
        ]

    def draw_captions(self, img, t):
        size = int(40 * self.s) if not self.sq else 40
        caps = self.captions()
        for k, (a, b, text) in enumerate(caps):
            grows_in = k > 0 and text.startswith(caps[k - 1][2].rstrip("."))
            grows_out = k + 1 < len(caps) and caps[k + 1][2].startswith(text.rstrip("."))
            if k + 1 < len(caps) and not grows_out:
                b = min(b, caps[k + 1][0] - 0.37)
            lo = a if grows_in else a - 0.18
            hi = caps[k + 1][0] if grows_out else b + 0.18
            if lo <= t < hi:
                fin = 1.0 if grows_in else ease_out((t - (a - 0.18)) / 0.18)
                fout = 1.0 if grows_out else ease_out(((b + 0.18) - t) / 0.18)
                al = min(fin, fout)
                key = (text, size)
                if key not in self.cap_cache:
                    self.cap_cache[key] = np.array(caption_img(text, size, self.W - 120))
                c = self.cap_cache[key]
                h, w = c.shape[:2]
                y = self.H - h - int(26 * (self.H / 1080)) + int((1 - fin) * 10)
                blend(img, c, (self.W - w) // 2, y, al)

    # ---- intro: laptop webcam vs phone, then the name
    def intro_frames(self):
        p = self.plan
        W, H = self.W, self.H
        n = int(round(p.intro_end * FPS))
        cap = cv2.VideoCapture(str(STOCK / "10446-1080.mp4"))
        src_fps = cap.get(cv2.CAP_PROP_FPS)
        cur, pos = None, -1
        t_wipe = p.ws(9) - 0.05
        t_word = p.ws(9) + 0.25
        rng = np.random.default_rng(7)
        lab_l = np.array(pill_img("Laptop webcam", int(30 * self.s) if not self.sq else 28, bg=(16, 14, 18, 215), dot=(150, 150, 150)))
        lab_r = np.array(pill_img("Your phone", int(30 * self.s) if not self.sq else 28, bg=(16, 14, 18, 215), dot=ACCENT))
        word = self.wordmark()
        for i in range(n):
            t = i / FPS
            want = int((t + 0.6) * src_fps)
            while pos < want:
                ok, f = cap.read()
                if not ok:
                    break
                cur, pos = f, pos + 1
            fr = cv2.resize(cur, (int(H * 16 / 9), H), interpolation=cv2.INTER_AREA)
            fw = fr.shape[1]
            e = ease((t - t_wipe) / 0.6)
            half = W // 2
            right_w = int(half + (W - half) * e)
            out = np.zeros((H, W, 3), np.uint8)
            # right: the phone, crisp, a centred window that grows
            x0 = (fw - right_w) // 2
            out[:, W - right_w:] = fr[:, x0:x0 + right_w]
            # left: the same shot through a typical laptop webcam
            lw = W - right_w
            if lw > 0:
                small = cv2.resize(fr, (fw // 4, H // 4), interpolation=cv2.INTER_AREA)
                small = cv2.GaussianBlur(small, (0, 0), 0.9)
                small = small.astype(np.float32)
                small = (small - 128) * 0.78 + 128 + np.array([6, 4, -8])  # flat, slightly green-grey
                small = small * 0.82 + 8
                small += rng.normal(0, 7, small.shape[:2])[..., None]
                small = np.clip(small, 0, 255).astype(np.uint8)
                ok, buf = cv2.imencode(".jpg", small, [cv2.IMWRITE_JPEG_QUALITY, 22])
                small = cv2.imdecode(buf, cv2.IMREAD_COLOR)
                bad = cv2.resize(small, (fw, H), interpolation=cv2.INTER_LINEAR)
                lx0 = (fw - half) // 2 + (half - lw)  # slides out to the left
                out[:, :lw] = bad[:, lx0:lx0 + lw]
                cv2.rectangle(out, (lw - 2, 0), (lw + 1, H), (12, 10, 14), -1)
                al = 1 - ease((t - t_wipe) / 0.25)
                ly = int(34 * (H / 1080))
                if al > 0:
                    blend(out, lab_l, lw - half + (half - lab_l.shape[1]) // 2, ly, al)
                    blend(out, lab_r, W - right_w + (half - lab_r.shape[1]) // 2, ly, al)
            # the name over the phone picture
            if t >= t_word - 0.05:
                a = ease_out((t - t_word) / 0.7)
                dark = 0.42 * a
                if a > 0:
                    k = int(1 + 10 * a) | 1
                    blurred = cv2.GaussianBlur(out, (0, 0), 1 + 7 * a)
                    out = (blurred.astype(np.float32) * (1 - dark) + np.array([14, 8, 20]) * dark).astype(np.uint8)
                self.draw_word(out, word, t - t_word)
            self.draw_captions(out, t)
            yield out
        cap.release()

    def wordmark(self):
        big = int((290 * self.s) if not self.sq else 250)
        f_big = font(F_BLACK, big)
        f_by = font(F_DMED, int(big * 0.15))
        track = -0.022 * big
        letters = "LENS"
        widths = [f_big.getlength(ch) for ch in letters]
        word_w = int(sum(widths) + track * (len(letters) - 1))
        asc, desc = f_big.getmetrics()
        im = Image.new("RGBA", (word_w + 20, asc + desc), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        x = 0.0
        for ch, cw in zip(letters, widths):
            d.text((x, 0), ch, font=f_big, fill=(255, 255, 255, 255))
            x += cw + track
        top = f_big.getbbox("L")[1]
        cap_h = f_big.getbbox("L")[3] - top
        im = im.crop((0, top, im.width, top + cap_h))
        return {"img": im, "w": word_w, "h": cap_h, "big": big, "f_by": f_by}

    def draw_word(self, out, wd, tt, url=False, ta=0.0):
        W, H = self.W, self.H
        a = ease_out(tt / 0.7)
        a_by = ease_out((tt - 0.3) / 0.6)
        big, im, word_w, cap_h = wd["big"], wd["img"], wd["w"], wd["h"]
        by = "by rng"
        f_by = wd["f_by"]
        by_w = int(f_by.getlength(by))
        bb = f_by.getbbox(by)
        gap = int(big * 0.09)
        f_url = font(F_SEMI, int(34 * (self.s if not self.sq else 1)))
        ub = f_url.getbbox("github.com/MRRengi/rng-lens")
        ph = int(68 * (self.s if not self.sq else 1))
        block = cap_h + gap + bb[3] + (int(70 * (self.s if not self.sq else 1)) + ph if url else 0)
        top = (H - block) // 2
        left = (W - word_w) // 2
        sc = 1.0 + 0.035 * (1 - a)
        wimg = im if sc == 1.0 else im.resize((int(im.width * sc), int(im.height * sc)), Image.LANCZOS)
        arr = np.array(wimg)
        blend(out, arr, left - (wimg.width - im.width) // 2, top - (wimg.height - im.height) // 2, a)
        lay = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        d = ImageDraw.Draw(lay)
        bx = left + word_w - by_w - int(big * 0.02)
        byy = top + cap_h + gap - bb[1] + int((1 - a_by) * 10)
        d.text((bx, byy), by, font=f_by, fill=(255, 255, 255, int(200 * max(0.0, a_by))))
        if url:
            a_u = ease_out((tt - 0.5) / 0.6)
            uy = top + cap_h + gap + bb[3] + int(70 * (self.s if not self.sq else 1))
            pw = ub[2] - ub[0] + int(64 * (self.s if not self.sq else 1))
            d.rounded_rectangle([(W - pw) // 2, uy, (W + pw) // 2, uy + ph], ph // 2, fill=(16, 14, 18, int(220 * max(0.0, a_u))))
            d.text(((W - (ub[2] - ub[0])) // 2 - ub[0], uy + (ph - (ub[3] - ub[1])) // 2 - ub[1]), "github.com/MRRengi/rng-lens", font=f_url,
                   fill=(255, 255, 255, int(255 * max(0.0, a_u))))
        blend(out, np.array(lay), 0, 0, 1.0)

    def end_frames(self, last, n):
        W, H = self.W, self.H
        wall = self.wall
        ox, oy = (wall.shape[1] - W) // 2, (wall.shape[0] - H) // 2
        base = wall[oy:oy + H, ox:ox + W]
        base = cv2.GaussianBlur(base, (0, 0), 12)
        base = (base.astype(np.float32) * 0.62 + np.array([20, 10, 22]) * 0.38).astype(np.uint8)
        wd = self.wordmark()
        for i in range(n):
            tt = i / FPS
            fr = base.copy()
            self.draw_word(fr, wd, tt, url=True)
            k = min(1.0, tt / 0.4)
            if k < 1:
                fr = (last.astype(np.float32) * (1 - k) + fr.astype(np.float32) * k).astype(np.uint8)
            yield fr

    def render(self, path, fast=False, stills=None):
        p = self.plan
        total_end = p.we(92) + 1.6
        enc = None
        if not stills:
            enc = subprocess.Popen(["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgr24", "-s", f"{self.W}x{self.H}", "-r", str(FPS), "-i", "-",
                                    "-c:v", "libx264", "-preset", "veryfast" if fast else "slow", "-crf", "19", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(path)],
                                   stdin=subprocess.PIPE)
        want = sorted(stills or [])
        saved = []

        def emit(fr, t):
            if enc:
                enc.stdin.write(fr.tobytes())
            while want and t >= want[0]:
                o = Path(path).with_name(f"still-{'sq' if self.sq else 'w'}-{want[0]:.2f}.jpg")
                cv2.imwrite(str(o), fr)
                saved.append(str(o))
                want.pop(0)

        intro = list(self.intro_frames()) if (not stills or min(stills) < p.intro_end) else None
        n_intro = int(round(p.intro_end * FPS))
        xf = int(0.45 * FPS)
        i = 0
        last = None
        t_end = p.end_at
        while True:
            t = i / FPS
            if t >= t_end + 0.0:
                break
            if stills and not want:
                break
            if t >= n_intro / FPS - 0.45 or not intro:
                if stills and want and t < want[0] - 0.02 and t >= p.intro_end:
                    i += 1
                    continue
                fr = self.compose(t, self.window_frame(t))
                if t >= p.intro_end - 0.6:
                    self.draw_overlays(fr, t)
            else:
                fr = None
            if intro and i < n_intro:
                it = intro[i]
                k = (i - (n_intro - xf)) / xf
                if k > 0 and fr is not None:
                    e = ease(k)
                    fr = (it.astype(np.float32) * (1 - e) + fr.astype(np.float32) * e).astype(np.uint8)
                else:
                    fr = it
            emit(fr, t)
            last = fr
            i += 1
        if not stills or want:
            n_end = int(round((total_end - t_end) * FPS))
            for k, fr in enumerate(self.end_frames(last, n_end)):
                emit(fr, t_end + k / FPS)
        if enc:
            enc.stdin.close()
            enc.wait()
        return saved

    def draw_overlays(self, fr, t):
        self.draw_captions(fr, t)


def main():
    take, vdir, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    pick = sys.argv[sys.argv.index("--pick") + 1] if "--pick" in sys.argv else "1"
    fast = "--fast" in sys.argv
    stills = None
    if "--still" in sys.argv:
        stills = [float(x) for x in sys.argv[sys.argv.index("--still") + 1].split(",")]
    only = [a for a in sys.argv[4:] if a in ("16x9", "1x1")] or ["16x9", "1x1"]
    tk = Take(take)
    words = json.loads((vdir / f"words-{pick}.json").read_text())
    narr = vdir / f"narration-{pick}.mp3"
    narr_s = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(narr)], capture_output=True, text=True).stdout)
    plan = Plan(tk, words, narr_s)
    plan.report()
    (out / "inserts.json").write_text(json.dumps(plan.inserts))
    for o in only:
        W, H = (1920, 1080) if o == "16x9" else (1080, 1080)
        r = Renderer(tk, plan, W, H, o == "1x1")
        res = r.render(out / f"lens-{o}.mp4", fast, stills)
        print(o, "done", res if res else "")


if __name__ == "__main__":
    main()
