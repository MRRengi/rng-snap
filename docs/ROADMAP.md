# Rengi Snap - where we are

**Done**
- Decisions: Rust, native Windows, no UI framework. Hotkey Scroll Lock. All monitors freeze. Loupe on. Autostart + tray. MIT.
- **Block 1 - Capture core**: tray, autostart, Scroll Lock, freeze, frame selection + window auto-detect, loupe, top mini panel, clipboard (bitmap + PNG), preview card with Copy / Save / Save to / Folder, recent folders. Spec: `docs/specs/1-capture-core.md`
- **Block 2 - Smart regions**: hover snaps to elements inside windows (child windows + UI Automation + pixel boxes for games/canvas), mouse wheel walks levels element > panel > window > monitor. Spec: `docs/specs/2-smart-regions.md`
- **Block 3 - Video**: Video mode on the panel (30/60/120/Max fps, Original/1080p/720p, system sound + mic), Windows.Graphics.Capture -> GPU -> hardware encoder (H.264, HEVC above the H.264 limit), control bar invisible to capture (timer, pause, stop, mic mute, discard), MP4 saved straight to the folder, card with Copy file / Save to / Folder / GIF. 47 end-to-end checks pass on a Windows CI desktop (120 fps measured at 120.5). Spec: `docs/specs/3-video.md`

**WE ARE HERE**
- Maks checks block 3 on his own PC: 120/144 fps on his monitor, system sound + mic (CI has no audio devices), whether the yellow capture border shows.

**Ahead**
- **Block 4 - Editor**: arrow, rectangle, text, blur.
- **Block 5 - Release**: installer, exe icon, README with GIFs, GitHub Releases, winget.
