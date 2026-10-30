import subprocess, sys
from PIL import Image
d = sys.argv[1]
w0, m0, T0, K = map(float, open(f"{d}/clock.txt").read().split())
pts = [float(x) for x in subprocess.run(["ffprobe","-v","error","-select_streams","v","-show_entries","packet=pts_time","-of","csv=p=0",f"{d}/raw.mkv"],capture_output=True,text=True).stdout.split()]
t0 = pts[0]
def wall(app_ms): return (T0 + app_ms/1000*K) + (w0 - m0)
ims=[]
for a in sys.argv[2:]:
    ms=float(a); ts=wall(ms)-t0
    subprocess.run(["ffmpeg","-loglevel","error","-y","-ss",f"{ts:.3f}","-i",f"{d}/raw.mkv","-frames:v","1","-vf","scale=700:-1","/tmp/claude-0/s.png"])
    ims.append(Image.open("/tmp/claude-0/s.png").convert("RGB"))
cols=2; w,h=ims[0].size; rows=(len(ims)+1)//2
s=Image.new("RGB",(w*cols,h*rows))
for i,im in enumerate(ims): s.paste(im,((i%cols)*w,(i//cols)*h))
s.save("/tmp/claude-0/sheet.jpg")
print("raw first pts", t0, "app_ms at first frame", ((t0-(w0-m0))-T0)/K*1000)
