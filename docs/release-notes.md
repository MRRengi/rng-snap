Free, open-source screenshot and screen recording tool for Windows. Small, native, no runtime.

**New in 0.8**
- **Window** mode on the panel (Photo / Video / Window): click a window and Snap records that window itself. Telegram popups, notifications or anything else on top of it never get into the video. Resize it and it stays fitted in the frame; minimize it and the recording pauses until it's back.
- The frame shows the mode: **blue** for a screenshot, **red** for a video, so you don't record by accident.
- New shortcut **Alt+Shift+S**, comfortable for the left hand. Scroll Lock is gone; if you used it, Snap moves you over. Windows' Win+Shift+S (Snipping Tool) is left alone.
- **Cleaner system sound** in recordings: the faint buzz is gone.

**New in 0.7**
- **Cut out** (K in the editor), like Apple's Lift subject: point at an object and its outline glows, click and it's on the clipboard as a transparent PNG, ready to paste into a chat, a doc or Figma. Shift+click adds more objects. **Sticker** gives it a white outline and a shadow; **Save PNG** keeps it.
- Runs locally on your PC, nothing is uploaded. The first time, Snap asks to download the model once (57 MB).

**New in 0.6.2**
- The app is now just **Snap**: Snap.exe, the Start menu, Apps & features, the installer, Task Manager. Installing over an older version moves everything over by itself (settings, the captures folder, autostart) and removes the old install.

**Fixed in 0.6.1**
- Snap didn't start after installing when an older copy (for example a downloaded Snap.exe) was still running in the tray. The new one now replaces it, and the setup closes old copies first.

**New in 0.6**
- **Setup** (`Snap-Setup-0.8.0.exe`): per-user install without an admin prompt, Start menu entry, optional autostart and "Open with Snap" for MP4 / PNG, clean uninstall. Or take the **portable zip**.
- **Home window**: Screenshot, Record video and Edit a video as big tiles with their keys, your latest captures (click one to open it in the editor), and the settings that matter: shortcut, save folder, start with Windows. The tray icon opens it.
- **Liquid Glass redesign**: floating glass toolbars that blur what's under them, glass capture panel, loupe, preview card and recording bar, capsule buttons, Apple system colors, new app icon.
- **Light and dark**: follows the Windows setting, switching live.
- The exe has its icon and name ("Snap") in Explorer, the taskbar and Task Manager.

**Screenshots**
- Alt+Shift+S freezes all monitors; drag a frame or click a window / element (the mouse wheel walks element > panel > window > monitor)
- Smart element detection: child windows, UI Automation, and pixel boxes for games and canvas apps
- Works inside browsers and Electron apps too (Chrome, Edge, VS Code, Slack, Discord): hover a card or button on a web page and it snaps to it
- Loupe with coordinates and pixel color (C copies the hex)
- Copied to the clipboard instantly; preview card with Copy / Save / Save to / Folder

**Marks and editor**
- Hold Ctrl when you release the frame: a quick toolbar appears right on the frozen screen (arrow, line, rectangle, ellipse, pen, marker, text, blur, numbered steps)
- Edit button on the card opens the full editor: same tools plus crop, 8 colours + any colour, 3 thicknesses, blur or pixelate, undo/redo, zoom (Ctrl+wheel)
- Snagit-style looks: arrows with soft shadows, outlined text, numbered circles that count themselves
- Every change goes to the clipboard by itself; Ctrl+S saves, Ctrl+C copies
- Keys: V select, A arrow, L line, R rectangle, E ellipse, P pen, H marker, T text, B blur, N step, C crop, K cut out, Shift snaps

**Video**
- Up to your monitor's refresh rate (120 / 144 / 240 fps), GPU hardware encoding (H.264, HEVC when needed)
- System sound and microphone, pause, discard; the control bar never shows up in the video
- MP4 saved straight to your folder, one-click GIF export

**Video editor** (Edit on the video card, Edit a video on Home, Open with Snap, or `Snap.exe --edit file.mp4`)
- Cut out what you don't need: S splits at the playhead, Delete removes a piece, drag a piece's edge to trim it
- Make a reel: split out the moments you want and drag them into any order
- Text, arrows, boxes, blur, numbered steps and the other editor tools right on the video, each for as long as you like (drag its bar on the timeline)
- Zoom (Z): draw an area and the camera glides in and back out
- Timeline with thumbnails and the sound wave; preview plays with sound; undo / redo for everything
- Export (Ctrl+E) writes "<name> edit.mp4" next to the original and puts it on the clipboard; GIF button too

**Run it:** download `Snap-Setup-0.8.0.exe` and run it (or unzip the portable build). Windows SmartScreen may warn because it is not signed: More info > Run anyway. Snap opens Home, then lives in the tray.

Requires Windows 10 2004 or later, Windows 11 recommended.
