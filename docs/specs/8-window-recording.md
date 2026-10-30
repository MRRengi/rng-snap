# Block 8 - Window recording, mode colors, hotkey, clean sound

## What Maks asked for
- He keeps recording a video when he meant a screenshot. The frame must show the mode: **blue in Photo, red in Video**.
- Record **one window** for a client so a Telegram popup (or anything else on top) never ends up in the video.
- System sound on recordings squeaks a little.
- The hotkey moves to the left hand. **Scroll Lock goes away**, Windows' own Win+Shift+S (Snipping Tool) stays untouched.

## Answers
- Window recording is a separate **Window** mode on the panel: Photo / Video / Window.
- The window is resized while recording: the video keeps its size, and the window is scaled to fit and centred, with black bars if needed.
- The window is minimized: the recording pauses by itself and continues when it comes back.
- Red in Video and Window: the selection frame, its handles and the size pill, and the hover highlight of windows and elements.
- New hotkey: **Alt+Shift+S** (left hand). Scroll Lock is removed from the shortcut list, and configs that still use it move to Alt+Shift+S.
- Cursor in Window mode (Maks didn't remember, so I took the default): it is recorded only while it is over that window.

## How it works
- **Window mode**:
  - Hover highlights whole top-level windows only (no elements, no dragging); a click (or Enter) starts recording that window.
  - Windows.Graphics.Capture records the window itself (`CreateForWindow`): other windows on top, popups and notifications never appear.
  - The frame pool follows the window's content size. Each frame is drawn into the fixed output size with aspect-fit on the GPU.
  - Minimized or closed window: the recording pauses itself (video and sound), and the pause is cut out of the file like a manual pause.
  - FPS, resolution, system sound and mic options work as in Video.
- **Colors**: the overlay uses the accent blue in Photo and the system red in Video and Window, for the frame, handles, size pill and suggestions.
- **Hotkey**:
  - The default is Alt+Shift+S. The presets on Home are Alt+Shift+S, Ctrl+Shift+S, Ctrl+Shift+X, Ctrl+Alt+S and Print Screen.
  - When the hook swallows a hotkey that has Alt or Win in it, it sends one unassigned key, so releasing Alt opens no menu bar in the app underneath and Alt+Shift doesn't switch the keyboard layout.
- **Sound**:
  - Before, each 10 ms packet was placed by its own timestamp, and rounding left one-sample gaps or overlaps between packets 100 times a second. That is the buzz.
  - Now each source plays as one continuous stream. A packet is placed right after the previous one, and the timestamp is used only when the device reports a break or the stream drifts more than 20 ms (plus after a pause).

## Checked by
- **Unit tests**:
  - A 440 Hz tone cut into jittery packets comes out sample-exact.
  - A real break still resyncs.
  - Scroll Lock configs migrate.
- **End-to-end on the Windows runner**:
  - The overlay frame is red in Video and blue in Photo.
  - Window mode records the animated test window while a bright "popup" window covers part of it, and the popup's colour must not be in the video.
  - The video keeps its size when the window is resized during recording.
  - The hotkey is Alt+Shift+S everywhere.
