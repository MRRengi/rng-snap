"""Phone camera feed → sharp + background-blurred frames and face boxes, with the models Lens itself ships
(PP-HumanSeg person segmentation, YuNet face detector). usage: feed.py <video> <outdir> <width> [--plain] [--crop x,y,w,h]"""
import cv2, numpy as np, onnxruntime as ort, json, os, sys
SRC, OUT, W = sys.argv[1], sys.argv[2], int(sys.argv[3]); PLAIN = '--plain' in sys.argv
CROP = [int(v) for v in sys.argv[sys.argv.index('--crop') + 1].split(',')] if '--crop' in sys.argv else None
os.makedirs(f'{OUT}/s', exist_ok=True); os.makedirs(f'{OUT}/b', exist_ok=True)
sess = ort.InferenceSession('/home/claude/deps/ml/pphumanseg.onnx')
cap = cv2.VideoCapture(SRC); AREAS = []; faces = []; prev = None; i = 0; H = None; fd = None
while True:
    ok, f = cap.read()
    if not ok: break
    if CROP: x, y, w, h = CROP; f = f[y:y + h, x:x + w]
    if H is None: H = round(f.shape[0] * W / f.shape[1]); H -= H % 2
    f = cv2.resize(f, (W, H), interpolation=cv2.INTER_AREA)
    cv2.imwrite(f'{OUT}/s/{i:03d}.jpg', f, [cv2.IMWRITE_JPEG_QUALITY, 88])
    if not PLAIN:
        x = (cv2.resize(cv2.cvtColor(f, cv2.COLOR_BGR2RGB), (192, 192)).astype(np.float32) / 255 - .5) / .5
        o = sess.run(None, {'x': x.transpose(2, 0, 1)[None]})[0][0]; e = np.exp(o - o.max(0)); m = (e[1] / e.sum(0)).astype(np.float32)
        m = cv2.resize(m, (W, H))
        # fill holes inside the subject (dark fur reads as background to the model)
        k = (m > .5).astype(np.uint8); k = cv2.morphologyEx(k, cv2.MORPH_CLOSE, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (25, 25)))
        cs, _ = cv2.findContours(k, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE); fill = np.zeros_like(k)
        if cs: cv2.drawContours(fill, [max(cs, key=cv2.contourArea)], -1, 1, -1)
        m = np.maximum(m, fill.astype(np.float32) * .95); m = cv2.GaussianBlur(m, (0, 0), 2.5)
        area = float((m > .5).mean()); AREAS.append(area); med = float(np.median(AREAS[-60:]))
        if prev is not None and area < .7 * med: m = prev          # the model lost the subject this frame: hold the last good mask
        m = m if prev is None else .6 * m + .4 * prev; prev = m
        a = np.clip((m - .35) / .3, 0, 1)[..., None]
        bg = cv2.GaussianBlur(f, (0, 0), W / 60)
        cv2.imwrite(f'{OUT}/b/{i:03d}.jpg', (f * a + bg * (1 - a)).astype(np.uint8), [cv2.IMWRITE_JPEG_QUALITY, 88])
        if fd is None: fd = cv2.FaceDetectorYN.create('/home/claude/deps/ml/face_detection_yunet_2023mar.onnx', '', (W, H), .5)
        _, d = fd.detect(f); faces.append([round(float(v), 1) for v in d[0][:4]] if d is not None and len(d) else None)
    i += 1
json.dump({'n': i, 'w': W, 'h': H, 'faces': faces}, open(f'{OUT}/feed.json', 'w'))
print(OUT, i, W, H, sum(1 for x in faces if x))
