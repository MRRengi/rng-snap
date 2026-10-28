# Snap by rng

Free, open-source screenshot and screen recording tool for Windows. One small native exe (Rust, no runtime, no installer) that opens instantly and stays out of the way.

![Snap demo](docs/media/demo.gif)

▶ **[Watch the demo with sound](docs/media/snap-demo.mp4)** · **[Download the latest release](https://github.com/MRRengi/rng-snap/releases/latest)**

## Screenshots
- **Scroll Lock** freezes every monitor; drag a frame, or click what is highlighted
- **Smart regions**: hover snaps to buttons, cards, panels, rows and windows, in native apps, browsers and Electron apps alike (child windows + UI Automation + pixel boxes for games and canvas apps); the **mouse wheel** walks element > panel > window > monitor
- Loupe with coordinates, frame size and pixel color (**C** copies the hex)
- Instantly on the clipboard; a preview card offers Copy / Save / Save to / Folder / Edit

## Marks and editor
- Hold **Ctrl** when you release the frame and mark it up right on the frozen screen: arrow, line, rectangle, ellipse, pen, marker, text, blur, numbered steps
- **Edit** on the preview card opens the full editor: the same tools plus crop, 8 colors and any custom color, 3 thicknesses, blur or pixelate, undo/redo, zoom
- Arrows with soft shadows, outlined text, numbered circles that count themselves
- Every change goes to the clipboard by itself

## Video
- Up to your monitor's refresh rate (120 / 144 / 240 fps), straight from the GPU into its hardware encoder (H.264, HEVC when needed)
- System sound and microphone, pause, discard; the control bar never appears in the video
- MP4 saved to your folder; Copy puts the file on the clipboard for Telegram / Discord; one-click GIF export

## Video editor
- **Edit** on the video card, **Edit a video...** in the tray menu, or `RengiSnap.exe --edit file.mp4`
- Cut out pieces (**S** splits at the playhead, **Delete** removes the selected piece, drag an edge to trim), or split out several moments and drag them into a new order
- Text, arrows, boxes, blur, numbered steps and the other editor tools right on the video; each one is a bar on the timeline you can move and stretch
- **Zoom (Z)**: draw an area, the camera glides in and back out
- Thumbnails and the sound wave on the timeline; the preview plays with sound; undo / redo for clips and marks together
- **Export (Ctrl+E)** writes `<name> edit.mp4` next to the original and puts it on the clipboard; **GIF** makes a GIF of the result

## Keys
| Where | Keys |
|---|---|
| Anywhere | **Scroll Lock** capture (press again to stop a recording) |
| Selecting | **Wheel** bigger / smaller region · **Ctrl** + release: mark it up · **C** copy pixel color · **Esc** cancel |
| Marks and editor | **V** select · **A** arrow · **L** line · **R** rectangle · **E** ellipse · **P** pen · **H** marker · **T** text · **B** blur · **N** step · **C** crop (editor) · **Shift** snap to 45° / square |
| Editor | **Ctrl+Z / Ctrl+Y** undo / redo · **Ctrl+C** copy · **Ctrl+S** save · **Ctrl+wheel** zoom · **Ctrl+0** fit · **Ctrl+1** 100% · **Space**+drag pan |
| Quick marks | **Enter** finish and copy |
| Video editor | **Space** play / pause · **S** split · **Delete** remove piece or mark · **Z** zoom · **←/→** frame step (**Shift** 1 s) · **Home/End** · **Ctrl+E** export · **Ctrl+Z / Ctrl+Y** undo / redo |

## Notes
- Windows 10 2004+; Windows 11 recommended
- The exe is not signed, so SmartScreen may warn on first run: More info > Run anyway
- Lives in the tray and starts with Windows (untick it in the tray menu)
- Settings live in `%APPDATA%\Rengi Snap\config.toml`

## Building
```
cargo build --release
```
CI builds on Windows, runs unit tests and an end-to-end test that drives the app on a real desktop, and publishes a release when the version in `Cargo.toml` goes up. The demo video is filmed by CI too (`demo` branch).

MIT license.
