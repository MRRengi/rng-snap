# Rengi Snap - where we are

**Done**
- Decisions: Rust, native Windows, no UI framework. Hotkey Scroll Lock. All monitors freeze. Loupe on. Autostart + tray. MIT.
- **Block 1 - Capture core**: tray, autostart, Scroll Lock, freeze, frame selection + window auto-detect, loupe, top mini panel, clipboard (bitmap + PNG), preview card with Copy / Save / Save to / Folder, recent folders. Spec: `docs/specs/1-capture-core.md`
- **Block 2 - Smart regions**: hover snaps to elements inside windows (child windows + UI Automation + pixel boxes for games/canvas), mouse wheel walks levels element > panel > window > monitor. Spec: `docs/specs/2-smart-regions.md`
- **Block 3 - Video**: Video mode on the panel (30/60/120/Max fps, Original/1080p/720p, system sound + mic), Windows.Graphics.Capture -> GPU -> hardware encoder (H.264, HEVC above the H.264 limit), control bar invisible to capture (timer, pause, stop, mic mute, discard), MP4 saved straight to the folder, card with Copy file / Save to / Folder / GIF. 47 end-to-end checks pass on a Windows CI desktop (120 fps measured at 120.5). Spec: `docs/specs/3-video.md`

- **Block 4 - Editor**: quick marks on the overlay (Ctrl while releasing the frame), full editor window from the card's Edit button. Tools: select, arrow, line, rectangle, ellipse, pen, marker, text, blur/pixelate, numbered steps, crop (editor). 8 colours + custom, 3 widths, undo/redo, zoom/pan, auto-copy after every change, last tool/colour remembered. Snagit-style rendering (shadows, outlined text). Checked end-to-end on the Windows CI desktop. Spec: `docs/specs/4-editor.md`

- **Block 5 - Video editor**: Edit on the video card / "Edit a video..." in the tray / `--edit file`. Timeline with thumbnails + waveform: split (S), delete, trim by dragging edges, reorder by dragging pieces. All editor tools on the video with on-screen time as bars you move and stretch, camera zooms (Z) that glide in and out, undo/redo, preview with sound. Export to `<name> edit.mp4` (hardware H.264/HEVC, audio cut with soft joins) + clipboard, GIF. Checked end-to-end on the Windows CI desktop with a synthetic clip (cut, reorder, text, zoom verified frame by frame). Spec: `docs/specs/5-video-editor.md`

- **Block 6 - Setup and Liquid Glass redesign**:
  - **Setup** `RengiSnap-Setup-0.6.1.exe`: no admin prompt, Start menu, autostart, "Open with Snap" for mp4/png, clean uninstall. Plus a **portable zip**. The exe now has an icon and the name "Snap by rng".
  - **Home window** on launch and from the tray icon: Screenshot / Record video / Edit a video tiles with their keys, recent captures (a click opens the editor), and settings (shortcut, folder, start with Windows).
  - **Whole UI in Liquid Glass**: floating glass toolbars with a real blur of what's underneath, a glass panel, loupe, card and recording bar, capsule buttons, Apple colours, a new icon.
  - **Light and dark** follow Windows, switching live.
  - Checked on CI: e2e in both themes, plus a real silent install and uninstall.
  - Spec: `docs/specs/6-setup-and-redesign.md`

- **Releases** v0.3.0 (blocks 1-3), v0.4.0 (+ editor), v0.4.1 (smart regions inside Chrome/Edge/Electron apps), v0.5.0 (+ video editor), v0.6.0 (+ setup, Home, redesign) on GitHub Releases.
- **Demo video** for LinkedIn/X (16:9 + 1:1, 47 s) filmed on the Windows CI desktop (`demo` branch, `demo/edit.py` edits it), posts written. CI publishes a new release by itself whenever the version in Cargo.toml goes up and all tests pass.

**WE ARE HERE**
- Maks installs v0.6.0 from the setup and checks:
  - Home.
  - The glass look in his theme, and switching the Windows theme.
  - "Open with Snap".
  - The video editor from block 5, including preview sound (CI has no speakers).
- Still open from block 3:
  - 120/144 fps on his monitor.
  - System sound and mic (CI has no audio devices).
  - Whether the yellow capture border shows.

**Ahead**
- **Block 7 - Distribution**:
  - winget package.
  - Code signing, so SmartScreen stops warning.
  - Auto-update check on Home.
