# Block 9 - Video editor: speed and a timeline that explains itself

## What Maks asked for
- The video editor lags, and the preview strip at the bottom fills in badly.
- It lacks visual cues for which part is which and how it works.
- Answers:
  - The timeline should look like Final Cut / CapCut.
  - Labels plus tooltips.
  - Zoom with Ctrl+wheel and a slider.
  - Scrubbing shows a frame at once, with the exact frame right after.

## Why it lagged
- **Decoding**: Media Foundation decoded on the CPU, and its basic converter (`ENABLE_VIDEO_PROCESSING`) turned every frame into RGB on the CPU too. For a 1080p frame that is the slow part.
- **Blur**: every new frame re-rendered the window offscreen and blurred all of it for the glass, although the picture never sits under the glass.
- **Thumbnails**: the strip decoded the whole video frame by frame, front to back, at full size. A long video filled in slowly, from the left.

## What changes
- **GPU decode**:
  - Readers can now decode on the GPU (a D3D11 device manager plus hardware transforms) with the advanced video processor, which also scales on the GPU.
  - The preview decodes at most 1920x1080.
  - If the system can't do this, it falls back to the old path.
- **Rendering**:
  - No offscreen scene and no blur pass per frame. The glass has nothing moving behind it, so it uses its plain tint.
- **Thumbnails**:
  - Each thumbnail is a seek straight to its slot, decoded at thumbnail size (135 px tall) by the video processor.
  - Slots fill coarse to fine (0, 1/2, 1/4, 3/4...), so the whole length shows within moments and then sharpens.
  - The same frames serve as the instant scrub preview and the hover preview.
- **Scrubbing**:
  - While the exact frame is on its way, the preview shows the nearest strip frame. The exact frame replaces it as soon as it lands.
- **Timeline (Final Cut / CapCut)**:
  - **Track names** on the left, each with an icon: Video, Audio, Marks.
  - **Video track**: a filmstrip pinned to time, so it doesn't swim when scrolling, and a duration pill on each clip.
  - **Trim grips**: a yellow frame with grips when a clip is selected, and a lighter one on hover.
  - **Audio** has its own track: a mirrored waveform under each clip that moves and trims with it. "No sound in this video" when there's none.
  - **Marks**: each bar shows its tool's icon and name, with edge grips.
  - **Playhead**: a line through every track and a blue time bubble (0:05.25).
  - **Hover**: a thin line plus a card above the timeline with that frame and its time.
  - **Ruler**: labelled ticks with smaller ones between, adapting down to 0.1 s.
  - **Status** ("Split", export progress, "Saved") is a toast pill above the timeline. The stray blue "Split" next to the Split button is gone.
- **Zoom**:
  - Ctrl+wheel zooms around the cursor; the wheel scrolls through time.
  - The transport row has a [-] slider [+] and a "show all" button.
  - A scrollbar appears under the tracks when zoomed in.
  - The view follows the playhead during playback. Zoomed all the way in, 0.5 s fills the timeline.
- **Tooltips**:
  - After half a second on any control, track, clip, clip edge, mark, mark edge, ruler or toolbar button, a glass tooltip says what it does, with its key.
  - Moving between controls keeps tooltips coming right away.

## Checked by
- **Unit tests**:
  - Timeline view: zoom around the cursor, limits, following the playhead.
  - Ruler steps and labels.
  - Coarse-to-fine slot order.
  - Fit-size maths.
- **End-to-end on the Windows runner**:
  - Playback logs real decode and compose timings, and the check needs 25+ fps.
  - The thumbnails finish within 4 s.
  - The play button's tooltip appears.
  - Hovering the timeline shows the frame card.
  - Ctrl+wheel zooms and Fit resets.
  - The existing cut, reorder, text, zoom and export checks still pass.
- **Visual review**: I crop and zoom each region of the screenshots (toolbar, transport, timeline, preview) instead of only looking at the whole screen. That's how the stray "Split" got missed before.
