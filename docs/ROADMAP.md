# Rengi Snap - where we are

**Done**
- Decisions: Rust, native Windows, no UI framework. Hotkey Scroll Lock. All monitors freeze. Loupe on. Autostart + tray. MIT.
- **Block 1 - Capture core** built: tray, autostart, Scroll Lock, freeze, frame selection + window auto-detect, loupe, top mini panel, clipboard (bitmap + PNG), preview card with Copy / Save / Save to / Folder, recent folders. 23 end-to-end checks pass on a Windows CI desktop; hotkey to overlay 30-55 ms. Spec: `docs/specs/1-capture-core.md`

**WE ARE HERE**
- Maks checks block 1 on his own PC (multi-monitor, his DPI, real apps).

**Ahead**
- **Block 2 - Smart regions**: detect UI elements inside windows (panels, tabs, buttons) via UI Automation, like Snagit.
- **Block 3 - Video**: MP4 via the GPU hardware encoder, FPS and resolution on the panel, system sound + mic toggles, GIF export.
- **Block 4 - Editor**: arrow, rectangle, text, blur.
- **Block 5 - Release**: installer, exe icon, README with GIFs, GitHub Releases, winget.
