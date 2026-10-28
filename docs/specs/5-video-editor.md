# Block 5 - Video editor

## What Maks asked for
- Cut out unneeded parts of a recording, or make a reel of several moments, and add text and other marks on top.
- Answers: open from the **Edit** button on the video card (plus any mp4 from the tray); clips **can be reordered**; on top: **text and all photo-editor tools, plus camera zooms**; export to a **new file**, put on the clipboard, plus a GIF button.

## How it works
- **Window**: toolbar (photo-editor tools + Zoom, colours, widths, undo/redo, GIF, Export), the preview, a transport row (play, time, Split, Delete) and the timeline.
- **Timeline**:
  - Clip row with thumbnails and the audio waveform.
  - **S** splits at the playhead, **Delete** removes the selected clip, dragging a clip moves it, dragging a clip edge trims it.
  - Annotation row: every mark and zoom is a bar. Drag to move it in time, drag its ends to change when it appears and disappears.
- **Annotations**:
  - Drawn right on the preview with the same tools as the photo editor (Canvas).
  - Each item has a span `[start, end)` in output time; new items start at the playhead and last 3 s.
  - They fade in and out over 160 ms. Steps keep their numbers across the whole project.
- **Zoom (Z)**:
  - A rect drawn on the frame. The camera eases in (450 ms), holds, and eases out.
  - The rect is grown to the frame's aspect ratio and never goes smaller than 12% of the width.
  - With the Zoom tool on (or a zoom selected), the preview shows the whole frame with the zoom areas outlined.
- **Undo/redo** covers clips and marks together.
- **Preview**: a player thread decodes the source (seek + decode forward) and composes the frame at preview size, in real time while playing.
- **Export**:
  - Clips in order through the same composer, hardware H.264 (HEVC above the H.264 limit), source fps.
  - Audio is cut to the same clips with 5 ms fades at the joins.
  - Saved as `<name> edit.mp4` next to the source (never overwrites) and copied to the clipboard.
  - **GIF** exports first if needed, then makes `<name> edit.gif`.
- `RengiSnap.exe --edit <video>` opens a file in the editor; a second instance hands it to the running one.

## Code
- `src/vedit.rs` (pure, unit-tested): `Timeline` (split / delete / move / trim / locate), `camera` and `zoom_rect`, `alpha`, `transform_item`, `Composer` (fast threaded bilinear resample, cached annotation layers), audio helpers, `edited_path`.
- `src/annot`: `Item.span`, `Shape::Zoom`, `Tool::Zoom`; Canvas shows and edits only items on screen at `Canvas.time`.
- `src/win/mp4read.rs`: seek, fps, duration, `AudioReader`.
- `src/win/vplayer.rs`: player thread, thumbnail and waveform maker.
- `src/win/vexport.rs`: export thread.
- `src/win/veditor.rs`: the window.

## Checked by
- Unit tests: timeline maths, zoom easing and clamping, fades, step numbers, composition (pass-through, zoom content, scaled and faded marks), audio fades and peaks, file names.
- End-to-end on the Windows runner:
  - Writes a synthetic clip (one colour per second, a white box, a tone) and opens it with `--edit`.
  - Splits at 1 s and 3 s, deletes the middle, drags the last clip to the front, adds text at 0 s and a zoom onto the white box at 1.2 s, then exports.
  - Checks the result: 2 s long with audio, yellow first, then red, the text is there, and the frame at 1.9 s is zoomed onto the box.
