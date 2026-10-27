# Block 3 - Video

## Goal

I switch the panel to Video, pick a region exactly like a screenshot (drag a frame, or click an element / window / monitor), and recording starts at once. Up to my monitor's refresh rate (120, 144, 240 fps), smooth, light on the system because frames go from the GPU straight into its hardware encoder. A small control bar sits next to the region but never shows up in the video. When I stop, the MP4 is already saved and the preview card offers Copy, Save to, Folder and GIF.

## Scope

1. **Panel in Video mode**
   - Photo / Video switch works; the last mode is remembered.
   - In Video mode the panel adds: FPS (30 / 60 / 120 / Max, default 60; Max = refresh rate of the monitor being recorded), Resolution (Original / 1080p / 720p, default Original; only ever scales down), System sound toggle, Microphone toggle (both off by default). Choices are remembered.

2. **Recording**
   - Region = the confirmed selection, rounded down to even width and height. A region spanning several monitors is clipped to the monitor holding most of it.
   - Capture with Windows.Graphics.Capture on that monitor, cursor included, no yellow border where Windows allows it.
   - Constant frame rate: a high-resolution timer ticks at the chosen FPS; each tick encodes the newest frame (the last one is repeated when the screen did not change).
   - Encoding through Media Foundation on the GPU (NVENC / QuickSync / AMF, software fallback). H.264 MP4 by default; HEVC only when the size and rate exceed H.264 level 5.2 (for example 4K at 120). Bitrate scales with pixels x fps, capped at 80 Mbit/s.
   - Audio (when toggled): system sound via WASAPI loopback and/or the default microphone, mixed by their timestamps into one AAC 48 kHz stereo track, kept in sync with video. A missing audio device is skipped, never an error.
   - File goes straight into the current save folder: `Video 2026-10-27 09-30-12.mp4`.

3. **Control bar and frame**
   - A thin accent outline just outside the region and a small bar below it (above when there is no room, inside the bottom edge when the region is the whole monitor): red dot + timer, Pause/Resume, Stop, Mic mute (only when the mic is on), Discard.
   - Both are excluded from capture (`WDA_EXCLUDEFROMCAPTURE`), so they never appear in the video even when inside the region.
   - Scroll Lock while recording = Stop.

4. **After recording**
   - Preview card with the first frame as thumbnail and a status line (`0:12 · 1920x1080 · 120 fps`).
   - Buttons: Copy (puts the file on the clipboard, pastes into Telegram / Discord / Explorer), Save to (moves the file to another folder from the recent list or a picker), Folder (Explorer with the file selected), GIF (exports `.gif` next to the MP4: 30 fps max, width up to 960, per-frame palette with dithering, progress in the status line).
   - Clicking the thumbnail plays the video in the default player.

## Out of scope

Webcam overlay, trimming / editing video, click highlights, live streaming.

## How this gets checked without anyone at the desktop

The Windows CI end-to-end test (software encoder on the runner):
- Records a region over an animated test window for about 2 s at 60 fps and at 120 fps, then decodes the MP4 with Media Foundation: frame size equals the even-rounded region, duration about 2 s, frame count about fps x duration, consecutive frames actually change.
- Records the full monitor with the control bar inside it and checks the decoded pixels under the bar match the window behind it (the bar is invisible to capture).
- 720p option on the full 1024x768 desktop gives a 960x720 video.
- GIF button produces a valid multi-frame GIF.
- Scroll Lock stops a recording; Discard leaves no file behind.

Pure logic (codec / bitrate / size plan, audio timeline mixer) is unit tested.
