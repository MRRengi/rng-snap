"""Films the Lens demo take in slow time. python3 take.py <out-dir> [explore]"""
import json, os, subprocess, sys, time, math
from pathlib import Path

L = Path("/tmp/claude-0/lens")
REPO = Path("/home/claude/rng-lens")
OUT = Path(sys.argv[1]); OUT.mkdir(parents=True, exist_ok=True)
EXPLORE = "explore" in sys.argv
K = 6.0
DISP = ":94"
WW, WH = 2100, 1000
PORT = 38550
env0 = dict(os.environ, DISPLAY=DISP)
procs = []

def sh(*a, **k):
    return subprocess.run(list(a), env=env0, **k)

xv = subprocess.Popen(["Xvfb", DISP, "-noreset", "-screen", "0", "2200x1300x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
procs.append(xv); time.sleep(1.5)
sh("python3", str(L / "setdpi.py"), "144")

home = OUT / "home"; (home / "rng-lens").mkdir(parents=True, exist_ok=True)
(home / "rng-lens" / "rng-lens.json").write_text(json.dumps({
    "slots": [{"autoRotate": False, "outWidth": 1920, "outHeight": 1080, "outFps": 30, "virtualCamera": True, "autoConnect": False,
               "phone": {"width": 1920, "height": 1080, "fps": 30},
               "filters": {"blur": 0.85, "frameTightness": 0.38, "frameSpeed": 0.4}}, {}],
    "audio": False, "theme": "dark", "autoUpdate": False}))

T0 = time.monotonic()
def at():  # app time, seconds
    return (time.monotonic() - T0) / K
appenv = dict(env0, XDG_CONFIG_HOME=str(home), HOME=str(home), SLOW=str(K), SLOW_T0=repr(T0),
              LD_PRELOAD=f"{L}/slowtime.so:{L}/fakev4l.so")
app = subprocess.Popen([str(REPO / "desktop/build-linux/rng-lens")], env=appenv, stdout=open(OUT / "app.log", "w"), stderr=subprocess.STDOUT)
procs.append(app)
win = None
for _ in range(60):
    r = sh("xdotool", "search", "--name", "^Lens$", capture_output=True, text=True).stdout.split()
    if r: win = r[0]; break
    time.sleep(0.5)
time.sleep(2)
sh("xdotool", "windowsize", win, str(WW), str(WH)); sh("xdotool", "windowmove", win, "0", "0")
time.sleep(12)

beats = open(OUT / "beats.txt", "w")
curs = open(OUT / "cursor.txt", "w")
cx, cy = 1250.0, 700.0
sh("xdotool", "mousemove", str(int(cx)), str(int(cy)))

def beat(kind, text=""):
    beats.write(f"{int(at() * 1000)} {kind} {text}\n"); beats.flush()

def logc():
    curs.write(f"{int(at() * 1000)} {int(cx)} {int(cy)}\n")

def until(t):
    while at() < t:
        logc(); time.sleep(0.03)

def glide(x, y, dur):
    """Eased glide over `dur` app-seconds, slight arc."""
    global cx, cy
    x0, y0, t0 = cx, cy, at()
    dx, dy = x - x0, y - y0
    nx, ny = -dy * 0.08, dx * 0.08
    while True:
        u = min(1.0, (at() - t0) / dur)
        e = u * u * u * (u * (u * 6 - 15) + 10)
        bow = math.sin(math.pi * e)
        cx, cy = x0 + dx * e + nx * bow, y0 + dy * e + ny * bow
        subprocess.run(["xdotool", "mousemove", str(int(cx)), str(int(cy))], env=env0)
        logc()
        if u >= 1: break

def click(name):
    beat("down", name)
    sh("xdotool", "mousedown", "1"); until(at() + 0.08); sh("xdotool", "mouseup", "1")
    beat("click", name)

def shot(name):
    sh("ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-video_size", f"{WW}x{WH}", "-draw_mouse", "0", "-i", f"{DISP}+0,0", "-frames:v", "1", str(OUT / f"{name}.png"))

fp = None
def start_phone():
    global fp
    fenv = dict(env0, SLOW=str(K), PATH=f"{L}/fbin:" + env0["PATH"], FAKE_DEVICE="Pixel 8", FAKE_VERSION="1.3.0")
    fp = subprocess.Popen(["java", "-cp", str(L / "fp/classes"), "FakePhone", str(PORT), "opus", str(L / "feed/slow6.mp4")],
                          env=fenv, stdout=open(OUT / "fp.log", "w"), stderr=subprocess.STDOUT)
    procs.append(fp)

def wait_log(word, t_max):
    while at() < t_max:
        if word in (OUT / "fp.log").read_text(errors="ignore"):
            return True
        logc(); time.sleep(0.05)
    return False

rec = None
try:
    if EXPLORE:
        shot("e0_empty")
        start_phone(); until(at() + 3); shot("e1_found")
        sys.exit(0)
    (OUT / "clock.txt").write_text(f"{time.time()} {time.monotonic()} {T0} {K}\n")
    rec = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-framerate", "8", "-video_size", f"{WW}x{WH}", "-draw_mouse", "0",
                            "-i", f"{DISP}+0,0", "-copyts", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-pix_fmt", "yuv444p", "-f", "matroska", str(OUT / "raw.mkv")], env=env0)
    time.sleep(2)
    exec(open(str(L / "scenario.py")).read())
finally:
    for f in (beats, curs):
        try: f.close()
        except Exception: pass
    if rec:
        rec.send_signal(2); rec.wait(timeout=30)
    for p in reversed(procs):
        try: p.terminate()
        except Exception: pass
    time.sleep(1)
    for p in procs:
        try: p.kill()
        except Exception: pass
