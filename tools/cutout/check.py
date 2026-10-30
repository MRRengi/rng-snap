"""Checks the exported models on a synthetic picture with ONNX Runtime, using
the same pre/post-processing Snap does (src/cutout.rs).

python tools/cutout/check.py <out-dir>
"""
import sys
import numpy as np
import onnxruntime as ort
from PIL import Image, ImageDraw

out = sys.argv[1]
W, H = 1280, 720
img = Image.new("RGB", (W, H))
px = np.zeros((H, W, 3), np.uint8)
px[..., 0] = np.linspace(40, 200, W, dtype=np.uint8)[None, :]
px[..., 1] = np.linspace(60, 160, H, dtype=np.uint8)[:, None]
px[..., 2] = 180
img = Image.fromarray(px)
d = ImageDraw.Draw(img)
d.ellipse([400, 200, 700, 500], fill=(230, 40, 40))
d.rectangle([850, 250, 1100, 450], fill=(30, 30, 200))
truth = np.zeros((H, W), bool)
yy, xx = np.mgrid[0:H, 0:W]
truth[((xx - 550) / 150) ** 2 + ((yy - 350) / 150) ** 2 <= 1] = True

scale = 1024 / max(W, H)
nw, nh = round(W * scale), round(H * scale)
x = np.asarray(img.resize((nw, nh), Image.BILINEAR), np.float32)
x = (x - [123.675, 116.28, 103.53]) / [58.395, 57.12, 57.375]
pad = np.zeros((1024, 1024, 3), np.float32)
pad[:nh, :nw] = x
enc = ort.InferenceSession(f"{out}/snap_cutout_encoder.onnx")
emb = enc.run(None, {"image": pad.transpose(2, 0, 1)[None].astype(np.float32)})[0]
dec = ort.InferenceSession(f"{out}/snap_cutout_decoder.onnx")
pts = np.array([[[550 * scale, 350 * scale], [0, 0]]], np.float32)
lab = np.array([[1, -1]], np.float32)
masks, iou, low = dec.run(None, {"image_embeddings": emb, "point_coords": pts, "point_labels": lab,
                                 "mask_input": np.zeros((1, 1, 256, 256), np.float32), "has_mask_input": np.zeros(1, np.float32),
                                 "orig_im_size": np.array([H, W], np.float32)})
print("iou predictions", iou)
best = 0
for k in range(1, 4):
    m = masks[0, k] > 0
    inter = (m & truth).sum()
    union = (m | truth).sum()
    print(f"mask {k}: iou with truth {inter / union:.3f}, area {m.sum()}")
    best = max(best, inter / union)
assert emb.shape == (1, 256, 64, 64), emb.shape
assert low.shape == (1, 4, 256, 256), low.shape
assert best > 0.9, best
print("check ok")
