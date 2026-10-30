# Block 7 - Cut out

## What Maks asked for
- Cut objects out of a picture by their shape, like Apple's sticker / "Lift subject".
- Answers:
  - The model downloads on first use.
  - The object is picked by hover + click, like Apple.
  - It lives in the photo editor.
  - The result is a clean cutout with a sticker toggle.

## How it works
- **Model**: MobileSAM (Apache-2.0), run locally through ONNX Runtime (MIT) on the CPU.
  - Nothing is uploaded anywhere.
  - The encoder looks at the picture once (about 1 s). After that, the decoder answers each hover and click in tens of milliseconds.
- **Model files**:
  - `tools/cutout/export.py` exports the encoder and decoder from the official weights. The `cutout-assets` workflow publishes them, with `onnxruntime.dll`, to the pre-release `cutout-models-v1`.
  - `tools/cutout/check.py` verifies the exported files on a synthetic picture before publishing.
- **First use**:
  - Picking the Cut out tool (K) shows a glass card: "Cut out runs on your PC. It needs a one-time 57 MB download" with a Download button.
  - The download (WinHTTP, with a progress bar) goes to `%LOCALAPPDATA%\Snap\models` and is checked against SHA-256. It is never downloaded again.
- **Using it**:
  - With the tool on, the object under the cursor glows along its outline.
  - A click cuts it: everything else dims, and the cutout is copied to the clipboard as a transparent PNG.
  - Shift+click adds another object to the same cut.
  - Esc clears the cut.
- **Result bar** (glass, at the bottom): Copy, Sticker on/off, Save PNG, Clear.
  - The sticker version adds a white outline and a soft shadow, like an iMessage sticker.
- **Mask quality** (`src/cutout.rs`):
  - SAM gives 3 granularities. Snap takes the largest one that is about as confident as the best and stable, so a click on a shirt lifts the person.
  - Stray specks are dropped, small holes are filled, and edges stay anti-aliased.

## Code
- `src/cutout.rs`: preprocessing, the ONNX engine, mask choice and upscaling, tidy, cut, sticker, edge glow. It is unit-tested, and checked end to end on Linux with the real models (ignored tests).
- `src/win/download.rs`: HTTPS download with progress and SHA-256.
- `src/win/cutui.rs`: the editor's Cut out worker thread and its drawing.
- The clipboard gets a "PNG" format with alpha plus a DIBV5 with alpha.

## Checked by
- Unit tests for the pixel maths.
- End-to-end on the Windows runner:
  - Opens a test picture (a red circle and a blue square on a gradient).
  - Picks Cut out, downloads the models and clicks the circle.
  - The clipboard PNG must be transparent in the corners and opaque in the middle, with the circle's size.
  - Turns on Sticker and checks the white outline.
