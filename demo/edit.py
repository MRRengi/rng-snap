"""Edits the raw demo footage into social videos.

python3 demo/edit.py <results-dir> <out-dir> [--fast]

Reads demo-raw.mp4, beats.txt, cursor.txt from the results dir and writes
rengi-snap-16x9.mp4 and rengi-snap-1x1.mp4. The direction (what to cut, where
the camera goes) lives in DIRECTION below and is anchored to beats, so a new
take with different timing still edits correctly.
"""
import bisect, math, subprocess, sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

FPS = 60
FONT_DIR = Path("/usr/share/fonts/opentype/inter")
F_SEMI = str(FONT_DIR / "Inter-SemiBold.otf")
F_BOLD = str(FONT_DIR / "Inter-Bold.otf")
F_MED = str(FONT_DIR / "Inter-Medium.otf")
F_XBOLD = str(FONT_DIR / "InterDisplay-ExtraBold.otf") if (FONT_DIR / "InterDisplay-ExtraBold.otf").exists() else F_BOLD

# ---------------------------------------------------------------- data


class Take:
    def __init__(self, d: Path):
        self.d = d
        self.beats = []  # (ms, kind, text)
        for line in (d / "beats.txt").read_text().splitlines():
            p = line.split(" ", 2)
            if len(p) >= 2:
                self.beats.append((int(p[0]), p[1], p[2] if len(p) > 2 else ""))
        cur = [tuple(map(int, l.split())) for l in (d / "cursor.txt").read_text().splitlines() if l.strip()]
        self.ct = [c[0] for c in cur]
        self.cx = [c[1] for c in cur]
        self.cy = [c[2] for c in cur]
        self.vp = None
        for line in (d / "layout.txt").read_text().splitlines():
            p = line.split()
            if p[0] == "viewport":
                self.vp = tuple(map(int, p[1:5]))
        self.video = str(d / "demo-raw.mp4")
        out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries", "packet=pts_time", "-of", "csv=p=0", self.video], capture_output=True, text=True).stdout
        self.pts = sorted(float(x) for x in out.split() if x.strip())
        self.flash_frame = find_flash(self.video)
        self.t_flash = self.pts[self.flash_frame]
        self.wm = watermark(self.video, self.src_index(1200))
        self.end = next((t for t, k, x in self.beats if k == "mark" and x == "end"), self.beats[-1][0])

    def src_index(self, ms):
        """Index of the source frame on screen at beat time `ms` (0 = flash on screen)."""
        return max(0, bisect.bisect_right(self.pts, self.t_flash + ms / 1000.0) - 1)

    def cursor(self, ms):
        i = bisect.bisect_left(self.ct, ms)
        if i <= 0:
            return self.cx[0], self.cy[0]
        if i >= len(self.ct):
            return self.cx[-1], self.cy[-1]
        t0, t1 = self.ct[i - 1], self.ct[i]
        a = (ms - t0) / max(1, t1 - t0)
        return self.cx[i - 1] + (self.cx[i] - self.cx[i - 1]) * a, self.cy[i - 1] + (self.cy[i] - self.cy[i - 1]) * a

    def t(self, kind, text=None, nth=0, after=0):
        """Time of the nth beat of `kind` (whose text starts with `text`) after time `after`."""
        n = 0
        for ms, k, x in self.beats:
            if ms >= after and k == kind and (text is None or x.startswith(text)):
                if n == nth:
                    return ms
                n += 1
        raise KeyError(f"no beat {kind} {text} #{nth}")


def find_flash(path):
    cap = cv2.VideoCapture(path)
    i = 0
    while True:
        ok, f = cap.read()
        if not ok:
            break
        if f[::8, ::8].mean() < 10:
            cap.release()
            return i
        i += 1
    cap.release()
    raise SystemExit("no sync flash in the footage")


WM_BOX = (1690, 1022, 1920, 1080)


def watermark(path, frame_no):
    """The runner's 'Test Mode' desktop text: (mask, clean patch, reference patch)."""
    cap = cv2.VideoCapture(path)
    cap.set(cv2.CAP_PROP_POS_FRAMES, frame_no)
    ok, f = cap.read()
    cap.release()
    x0, y0, x1, y1 = WM_BOX
    ref = f[y0:y1, x0:x1].copy()
    blur = cv2.GaussianBlur(ref, (0, 0), 6)
    mask = (np.abs(ref.astype(np.int16) - blur.astype(np.int16)).max(axis=2) > 18).astype(np.uint8) * 255
    mask = cv2.dilate(mask, np.ones((3, 3), np.uint8), iterations=2)
    clean = cv2.inpaint(ref, mask, 6, cv2.INPAINT_TELEA)
    return mask > 0, clean, ref


def unwatermark(f, wm):
    mask, clean, ref = wm
    x0, y0, x1, y1 = WM_BOX
    sub = f[y0:y1, x0:x1]
    same = np.abs(sub.astype(np.int16) - ref.astype(np.int16)).max(axis=2) < 40
    m = mask & same
    if m.any():
        sub[m] = clean[m]


# ---------------------------------------------------------------- easing


def ease(t):
    t = min(1.0, max(0.0, t))
    return t * t * t * (t * (t * 6 - 15) + 10)


def ease_out(t):
    t = min(1.0, max(0.0, t))
    return 1 - (1 - t) ** 3


class Track:
    """Piecewise eased keyframes: [(ms, value_tuple, transition_ms)]."""

    def __init__(self, keys):
        self.keys = sorted(keys, key=lambda k: k[0])

    def at(self, ms):
        ks = self.keys
        if ms <= ks[0][0]:
            return ks[0][1]
        prev = ks[0]
        for k in ks[1:]:
            if ms < k[0]:
                break
            prev = k
        # Transition from the key before `prev` into `prev` over prev[2] ms.
        i = ks.index(prev)
        if i == 0:
            return prev[1]
        before = self.at(prev[0] - 1) if prev[2] > 0 else prev[1]
        a = ease((ms - prev[0]) / max(1, prev[2]))
        return tuple(b + (c - b) * a for b, c in zip(before, prev[1]))


# ---------------------------------------------------------------- drawing helpers


def font(path, size):
    return ImageFont.truetype(path, size)


def rounded(size, radius, fill):
    im = Image.new("RGBA", size, (0, 0, 0, 0))
    ImageDraw.Draw(im).rounded_rectangle([0, 0, size[0] - 1, size[1] - 1], radius, fill=fill)
    return im


def shadowed(im, blur=18, alpha=110, off=(0, 8)):
    pad = blur * 2
    sh = Image.new("RGBA", (im.width + pad * 2, im.height + pad * 2), (0, 0, 0, 0))
    a = im.split()[3].point(lambda v: v * alpha // 255)
    sh.paste((0, 0, 0, 255), (pad + off[0], pad + off[1]), a)
    sh = sh.filter(ImageFilter.GaussianBlur(blur))
    sh.alpha_composite(im, (pad, pad))
    return sh, pad


def pill(text, size, fg=(255, 255, 255, 255), bg=(17, 19, 24, 228), padx=None, pady=None, fpath=F_SEMI):
    f = font(fpath, size)
    l, t, r, b = f.getbbox(text)
    padx = padx or int(size * 0.75)
    pady = pady or int(size * 0.48)
    w, h = r - l + padx * 2, int(size * 1.0) + pady * 2
    im = rounded((w, h), h // 2, bg)
    d = ImageDraw.Draw(im)
    d.text((padx - l, (h - size) // 2 - int(size * 0.06)), text, font=f, fill=fg)
    return shadowed(im, blur=16, alpha=120)


def keycap(label, size):
    f = font(F_SEMI, size)
    l, t, r, b = f.getbbox(label)
    padx, h = int(size * 0.7), int(size * 2.0)
    w = max(h, r - l + padx * 2)
    im = Image.new("RGBA", (w, h + 6), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle([0, 6, w - 1, h + 5], int(size * 0.45), fill=(12, 13, 17, 235))
    d.rounded_rectangle([0, 0, w - 1, h - 1], int(size * 0.45), fill=(255, 255, 255, 250), outline=(214, 217, 224, 255), width=2)
    d.text(((w - (r - l)) // 2 - l, (h - size) // 2 - int(size * 0.08)), label, font=f, fill=(20, 22, 28, 255))
    return shadowed(im, blur=14, alpha=110)


def blend(dst, src, x, y, alpha=1.0):
    """Alpha-blend RGBA numpy `src` onto BGR numpy `dst` at x,y."""
    h, w = src.shape[:2]
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(dst.shape[1], x + w), min(dst.shape[0], y + h)
    if x1 <= x0 or y1 <= y0:
        return
    s = src[y0 - y : y1 - y, x0 - x : x1 - x]
    a = s[..., 3:4].astype(np.float32) / 255.0 * alpha
    d = dst[y0:y1, x0:x1].astype(np.float32)
    rgb = s[..., 2::-1].astype(np.float32)  # RGBA -> BGR
    dst[y0:y1, x0:x1] = (d * (1 - a) + rgb * a).astype(np.uint8)


def to_np(im):
    return np.array(im)


# ---------------------------------------------------------------- direction


def direction(tk: Take):
    """Returns (segments, camera keys, captions, keys) in beat time.

    segments: [(src_start_ms, src_end_ms, speed)] played in order.
    camera: [(ms, (cx, cy, zoom), transition_ms)] in screen pixels; zoom 1 = whole screen.
    """
    vx, vy, vr, vb = tk.vp
    P = lambda x, y: (vx + x, vy + y)
    T = tk.t
    cap = lambda text: T("caption", text)

    t_freeze = T("key", "Scroll Lock")
    t_smart = cap("It finds")
    t_wheel = cap("Mouse wheel")
    t_quick = cap("Hold Ctrl")
    t_enter = T("key", "Enter")
    t_open = cap("Need more")
    t_blur = cap("Blur")
    t_note = cap("Add a note")
    t_clip = cap("Every change")
    t_video = cap("Video too")
    t_rec = T("mark", "recording")
    t_saved = cap("Saved as MP4")
    t_gifc = cap("One click")
    t_gif = T("mark", "gif-done")
    t_end = tk.end

    seg = [
        (t_freeze - 1600, t_open + 1300, 1.0),
        (t_open + 1300, t_blur - 300, 1.6),  # editor opening
        (t_blur - 300, t_clip + 1500, 1.0),
        (t_clip + 1500, t_video - 200, 2.5),  # editor closing
        (t_video - 200, t_rec + 300, 1.0),
        (t_rec + 300, t_saved, 1.0),
        (t_saved, t_gifc + 300, 1.4),
        (t_gifc + 300, t_gif, 2.5),  # gif encoding
        (t_gif, min(t_gif + 1800, t_end), 1.0),
    ]

    cx, cy = 960, 540
    cam = [
        (t_freeze - 1600, (cx, cy, 1.0), 0),
        (t_smart + 200, (*P(720, 300), 1.35), 900),
        (t_wheel + 200, (*P(700, 520), 1.15), 800),
        (t_quick + 600, (cx, cy, 1.0), 800),
        (T("key", "R") - 200, (*P(1190, 400), 1.6), 900),
        (T("key", "N") + 300, (*P(600, 360), 1.35), 900),
        (t_enter, (cx, cy, 1.0), 900),
        (t_blur - 100, (*P(1400, 120), 1.7), 900),
        (t_note - 100, (*P(1250, 470), 1.6), 800),
        (t_clip + 200, (cx, cy, 1.0), 900),
        (t_video + 300, (cx, 120, 1.9), 700),
        (t_rec - 900, (*P(700, 600), 1.25), 900),
        (t_saved + 200, (cx, cy, 1.0), 900),
        (t_gifc - 200, (1500, 860, 1.6), 900),
        (t_gif + 400, (cx, cy, 1.0), 1000),
    ]

    caps = [(ms, x) for ms, k, x in tk.beats if k == "caption"]
    keys = [(ms, x) for ms, k, x in tk.beats if k == "key"]
    clicks = [ms for ms, k, x in tk.beats if k in ("click", "down")]
    return seg, cam, caps, keys, clicks


# ---------------------------------------------------------------- render


class Renderer:
    def __init__(self, tk, out_w, out_h, square):
        self.tk, self.W, self.H, self.square = tk, out_w, out_h, square
        self.seg, cam, self.caps, self.keys, self.clicks = direction(tk)
        self.cam = Track(cam)
        s = out_h / 1080
        self.s = s
        self.cap_cache = {}
        self.key_cache = {}
        self.ring = self.make_ring()
        self.cursors = {}

    def make_cursor(self, size):
        """Arrow pointer, tip at (pad, pad) of the returned RGBA array."""
        k = 4
        pts = [(0, 0), (0, 17.5), (4.2, 13.6), (7.0, 20.0), (9.6, 18.9), (6.9, 12.7), (12.6, 12.4)]
        sc = size / 20.0 * k
        pad = 6 * k
        W = int(14 * sc + pad * 2)
        H = int(21 * sc + pad * 2)
        im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        poly = [(pad + x * sc, pad + y * sc) for x, y in pts]
        sh = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        ImageDraw.Draw(sh).polygon([(x + 1.2 * k, y + 2 * k) for x, y in poly], fill=(0, 0, 0, 120))
        sh = sh.filter(ImageFilter.GaussianBlur(2.2 * k))
        im.alpha_composite(sh)
        d = ImageDraw.Draw(im)
        d.polygon(poly, fill=(255, 255, 255, 255), outline=(16, 16, 20, 255), width=int(1.6 * k))
        im = im.resize((W // k, H // k), Image.LANCZOS)
        return to_np(im), pad // k

    def make_ring(self):
        r = 34
        im = Image.new("RGBA", (r * 2 + 4, r * 2 + 4), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        d.ellipse([2, 2, r * 2 + 1, r * 2 + 1], fill=(255, 214, 10, 70), outline=(255, 214, 10, 170), width=3)
        return to_np(im)

    def timeline(self):
        """Yields beat-time ms for each output frame."""
        for a, b, sp in self.seg:
            n = int((b - a) / sp * FPS / 1000)
            for i in range(n):
                yield a + i * sp * 1000 / FPS

    def caption_img(self, text):
        if text not in self.cap_cache:
            im, pad = pill(text, int(40 * self.s * (1.1 if self.square else 1.0)))
            self.cap_cache[text] = (to_np(im), pad)
        return self.cap_cache[text]

    def key_img(self, label):
        nice = {"wheel-up": "Scroll ↑", "wheel-down": "Scroll ↓"}.get(label, label)
        if nice not in self.key_cache:
            im, pad = keycap(nice, int(30 * self.s * (1.1 if self.square else 1.0)))
            self.key_cache[nice] = (to_np(im), pad)
        return self.key_cache[nice]

    def frame(self, src, ms):
        tk = self.tk
        unwatermark(src, tk.wm)
        cx, cy, z = self.cam.at(ms)
        if self.square:
            # Square window: 1080x1080 at zoom 1, centred on the camera x.
            vw, vh = 1080 / z, 1080 / z
        else:
            vw, vh = 1920 / z, 1080 / z
        x0 = min(max(cx - vw / 2, 0), 1920 - vw)
        y0 = min(max(cy - vh / 2, 0), 1080 - vh)
        sc = self.W / vw
        M = np.float32([[sc, 0, -x0 * sc], [0, sc, -y0 * sc]])
        img = cv2.warpAffine(src, M, (self.W, self.H), flags=cv2.INTER_CUBIC if sc > 1.01 else cv2.INTER_AREA)

        # Cursor halo and click ripples.
        px, py = tk.cursor(ms)
        sx, sy = (px - x0) * sc, (py - y0) * sc
        r = self.ring
        blend(img, r, int(sx - r.shape[1] / 2), int(sy - r.shape[0] / 2), 1.0)
        for c in self.clicks:
            dt = ms - c
            if 0 <= dt < 450:
                a = dt / 450
                rad = int((18 + 40 * ease_out(a)) * self.s * max(1.0, sc * 0.8))
                ov = img.copy()
                cv2.circle(ov, (int(sx), int(sy)), rad, (10, 214, 255), max(2, int(4 * self.s)), cv2.LINE_AA)
                cv2.addWeighted(ov, 0.85 * (1 - a), img, 1 - 0.85 * (1 - a), 0, img)

        csize = int(round(26 * self.s * min(1.6, max(1.0, sc ** 0.5))))
        if csize not in self.cursors:
            self.cursors[csize] = self.make_cursor(csize)
        cim, cpad = self.cursors[csize]
        blend(img, cim, int(round(sx)) - cpad, int(round(sy)) - cpad, 1.0)

        # Caption: current one, fade in/out.
        cur = None
        for i, (t, text) in enumerate(self.caps):
            nxt = self.caps[i + 1][0] if i + 1 < len(self.caps) else t + 4000
            end = min(nxt, t + 4200)
            if t - 100 <= ms < end:
                cur = (t, end, text)
        if cur:
            t, end, text = cur
            a = min(1.0, (ms - t + 100) / 250) * min(1.0, (end - ms) / 250)
            capi, pad = self.caption_img(text)
            lift = int((1 - ease_out(min(1.0, (ms - t + 100) / 300))) * 18 * self.s)
            x = (self.W - capi.shape[1]) // 2
            y = self.H - capi.shape[0] - int(46 * self.s) + lift + pad
            blend(img, capi, x, y, max(0.0, a))

        # Key presses: stacked bottom-right of the caption area, newest right.
        live = [(t, k) for t, k in self.keys if 0 <= ms - t < 1100]
        live = live[-3:]
        if live:
            ims = [(self.key_img(k), t) for t, k in live]
            gap = int(10 * self.s)
            total = sum(i[0][0].shape[1] - i[0][1] * 2 for i in ims) + gap * (len(ims) - 1)
            x = self.W - total - int(48 * self.s)
            y_base = int(48 * self.s)
            for (ki, pad), t in ims:
                age = ms - t
                a = min(1.0, age / 120) * min(1.0, (1100 - age) / 250)
                pop = 1.0 - 0.15 * (1 - ease_out(min(1.0, age / 180)))
                kim = ki if pop > 0.99 else cv2.resize(ki, None, fx=pop, fy=pop, interpolation=cv2.INTER_AREA)
                blend(img, kim, x - pad + (ki.shape[1] - kim.shape[1]) // 2, y_base - pad + (ki.shape[0] - kim.shape[0]) // 2, max(0.0, a))
                x += ki.shape[1] - pad * 2 + gap
        return img

    def card(self, kind, n):
        """Title or end card frames over a blurred wallpaper."""
        W, H, s = self.W, self.H, self.s
        wall = Image.open(Path(__file__).with_name("wallpaper.jpg")).convert("RGB")
        if self.square:
            wall = wall.crop((420, 0, 1500, 1080))
        wall = wall.resize((W, H)).filter(ImageFilter.GaussianBlur(6))
        base = wall.convert("RGBA")
        frames = []
        f_big = font(F_XBOLD, int(118 * s))
        f_sub = font(F_MED, int(40 * s))
        f_url = font(F_SEMI, int(36 * s))
        if kind == "title":
            lines = [("Rengi Snap", f_big, (255, 255, 255)), ("The free, open-source Snagit alternative", f_sub, (226, 232, 255))]
            if self.square:
                lines[1] = ("The free, open-source\nSnagit alternative", f_sub, (226, 232, 255))
        else:
            lines = [
                ("Rengi Snap", f_big, (255, 255, 255)),
                ("Free · Open source · Windows", f_sub, (226, 232, 255)),
                ("github.com/MRRengi/rng-snap", f_url, (255, 255, 255)),
            ]
        for i in range(n):
            a = ease_out(i / (FPS * 0.6))
            im = base.copy()
            d = ImageDraw.Draw(im)
            # Logo mark.
            m = int(96 * s)
            total_h = m + int(36 * s) + sum(d.multiline_textbbox((0, 0), t, font=f, align="center", spacing=int(10 * s))[3] + int(22 * s) for t, f, _ in lines)
            y = (H - total_h) / 2 + (1 - a) * 30 * s
            lx = (W - m) // 2
            logo = Image.new("RGBA", (m, m), (0, 0, 0, 0))
            ImageDraw.Draw(logo).rounded_rectangle([0, 0, m - 1, m - 1], int(m * 0.28), fill=(255, 255, 255, int(255 * a)))
            ImageDraw.Draw(logo).rounded_rectangle([int(m * 0.24), int(m * 0.24), int(m * 0.76), int(m * 0.76)], int(m * 0.1), outline=(79, 70, 229, int(255 * a)), width=max(3, int(m * 0.09)))
            im.alpha_composite(logo, (lx, int(y)))
            y += m + int(36 * s)
            for idx, (t, f, col) in enumerate(lines):
                bb = d.multiline_textbbox((0, 0), t, font=f, align="center", spacing=int(10 * s))
                la = ease_out((i - idx * 6) / (FPS * 0.6))
                if kind == "end" and idx == 2:
                    pw, ph = bb[2] + int(56 * s), bb[3] + int(34 * s)
                    p = rounded((pw, ph), ph // 2, (17, 19, 24, int(220 * max(0, la))))
                    im.alpha_composite(p, ((W - pw) // 2, int(y - int(14 * s))))
                d.multiline_text(((W - bb[2]) / 2, y), t, font=f, fill=(*col, int(255 * max(0, la))), align="center", spacing=int(10 * s))
                y += bb[3] + int(22 * s) + (int(20 * s) if kind == "end" and idx == 1 else 0)
            frames.append(cv2.cvtColor(np.array(im.convert("RGB")), cv2.COLOR_RGB2BGR))
        return frames

    def render(self, path, fast=False):
        enc = subprocess.Popen(
            ["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgr24", "-s", f"{self.W}x{self.H}", "-r", str(FPS), "-i", "-",
             "-c:v", "libx264", "-preset", "veryfast" if fast else "slow", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(path)],
            stdin=subprocess.PIPE,
        )
        w = enc.stdin.write
        title = self.card("title", int(FPS * 2.2))
        footage = []
        cap = cv2.VideoCapture(self.tk.video)
        pos, cur = -1, None
        first = None
        n = 0
        for ms in self.timeline():
            want = self.tk.src_index(ms)
            while pos < want:
                ok, f = cap.read()
                if not ok:
                    break
                cur, pos = f, pos + 1
            if cur is None:
                continue
            fr = self.frame(cur, ms)
            if first is None:
                first = fr
                # Cross-fade from the title card into the footage.
                for i, t in enumerate(title):
                    k = max(0.0, (i - (len(title) - FPS * 0.35)) / (FPS * 0.35))
                    w((t * (1 - k) + fr * k).astype(np.uint8).tobytes() if k > 0 else t.tobytes())
            w(fr.tobytes())
            last = fr
            n += 1
        end = self.card("end", int(FPS * 3.2))
        for i, t in enumerate(end):
            k = min(1.0, i / (FPS * 0.35))
            w((last * (1 - k) + t * k).astype(np.uint8).tobytes() if k < 1 else t.tobytes())
        enc.stdin.close()
        enc.wait()
        return n


def main():
    d, out = Path(sys.argv[1]), Path(sys.argv[2])
    fast = "--fast" in sys.argv
    only = [a for a in sys.argv[3:] if a in ("16x9", "1x1")]
    out.mkdir(parents=True, exist_ok=True)
    tk = Take(d)
    print("flash at frame", tk.flash_frame)
    if not only or "16x9" in only:
        n = Renderer(tk, 1920, 1080, False).render(out / "rengi-snap-16x9.mp4", fast)
        print("16x9 footage frames", n)
    if not only or "1x1" in only:
        n = Renderer(tk, 1080, 1080, True).render(out / "rengi-snap-1x1.mp4", fast)
        print("1x1 footage frames", n)


if __name__ == "__main__":
    main()
