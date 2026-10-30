"""Clean camera feed → sharp + background-blurred frames (Lens's own model: selfie_landscape + YuNet) and face boxes."""
import cv2, numpy as np, onnxruntime as ort, json, os, sys
SRC = '/tmp/claude-0/lens/feed/fwd.mp4'; OUT = sys.argv[1]; W, H = 960, 540
os.makedirs(f'{OUT}/s', exist_ok=True); os.makedirs(f'{OUT}/b', exist_ok=True)
sess = ort.InferenceSession('/home/claude/deps/ml/pphumanseg.onnx'); inp = sess.get_inputs()[0]
print(inp.name, inp.shape)
ih, iw = (inp.shape[1], inp.shape[2]) if inp.shape[-1] == 3 else (inp.shape[2], inp.shape[3])
fd = cv2.FaceDetectorYN.create('/home/claude/deps/ml/face_detection_yunet_2023mar.onnx', '', (W, H), .6)
cap = cv2.VideoCapture(SRC); faces = []; prev = None; i = 0
while True:
    ok, f = cap.read()
    if not ok: break
    f = cv2.resize(f, (W, H), interpolation=cv2.INTER_AREA)
    x = (cv2.resize(cv2.cvtColor(f, cv2.COLOR_BGR2RGB), (iw, ih)).astype(np.float32) / 255 - .5) / .5
    x = x.transpose(2, 0, 1)[None]
    o = sess.run(None, {inp.name: x})[0][0]; e = np.exp(o - o.max(0)); m = (e[1] / e.sum(0)).astype(np.float32)
    m = cv2.resize(m, (W, H)); m = cv2.GaussianBlur(m, (0, 0), 3)
    m = m if prev is None else .6 * m + .4 * prev; prev = m
    a = np.clip((m - .35) / .3, 0, 1)[..., None]
    bg = cv2.GaussianBlur(f, (0, 0), 16)
    b = (f * a + bg * (1 - a)).astype(np.uint8)
    cv2.imwrite(f'{OUT}/s/{i:03d}.jpg', f, [cv2.IMWRITE_JPEG_QUALITY, 88]); cv2.imwrite(f'{OUT}/b/{i:03d}.jpg', b, [cv2.IMWRITE_JPEG_QUALITY, 88])
    _, d = fd.detect(f); faces.append([round(float(v), 1) for v in d[0][:4]] if d is not None and len(d) else None)
    i += 1
json.dump({'n': i, 'w': W, 'h': H, 'faces': faces}, open(f'{OUT}/feed.json', 'w'))
print(i, sum(1 for x in faces if x))
