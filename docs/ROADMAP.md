# Rengi Snap - where we are

**Done**
- Decisions: Rust, native Windows, no UI framework. Hotkey Scroll Lock. All monitors freeze. Loupe on. Autostart + tray. MIT.
- **Block 1 - Capture core**: tray, autostart, Scroll Lock, freeze, frame selection + window auto-detect, loupe, top mini panel, clipboard (bitmap + PNG), preview card with Copy / Save / Save to / Folder, recent folders. Spec: `docs/specs/1-capture-core.md`
- **Block 2 - Smart regions**: hover snaps to elements inside windows (child windows + UI Automation + pixel boxes for games/canvas), mouse wheel walks levels element > panel > window > monitor, background worker so the overlay never waits. 31 end-to-end checks pass on a Windows CI desktop. Spec: `docs/specs/2-smart-regions.md`

**WE ARE HERE**
- Maks checks block 2 on his own PC (Chrome, Telegram, VS Code, Unity, a game).

**Ahead**
- **Block 3 - Video**: MP4 via the GPU hardware encoder, FPS and resolution on the panel, system sound + mic toggles, GIF export.
- **Block 4 - Editor**: arrow, rectangle, text, blur.
- **Block 5 - Release**: installer, exe icon, README with GIFs, GitHub Releases, winget.
