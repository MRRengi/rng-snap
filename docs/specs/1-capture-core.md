# Block 1 - Capture core

## Goal

I press Scroll Lock, every monitor freezes instantly, I drag a frame or click a window, and the image is already in my clipboard with a small preview card in the corner offering Copy / Save / Folder / Edit. No main window, no menus in the way. It must feel faster than the Windows Snipping Tool.

## Scope

1. **Process**
   - Single exe, Windows 10 1903+ and Windows 11, x64.
   - Single instance (named mutex). A second launch tells the first one to start a capture and exits.
   - Per-Monitor V2 DPI awareness. All internal coordinates are physical pixels on the virtual desktop.
   - Idle cost: no timers, no polling, one blocked message loop.

2. **Tray**
   - Icon with tooltip "Rengi Snap".
   - Left click: start capture.
   - Menu: Capture, Open captures folder, Start with Windows (checkbox), Exit.

3. **Autostart**
   - On by default. `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `RengiSnap`. Toggling the tray checkbox writes or removes it.

4. **Hotkey**
   - Default: Scroll Lock. Configurable (`hotkey` in config, virtual-key code plus modifiers).
   - Implemented with a low-level keyboard hook (`WH_KEYBOARD_LL`), so it works even if another app registered the key. The key press is swallowed, so Scroll Lock never toggles its state.
   - Injected input must trigger it too (automated tests rely on it).

5. **Freeze**
   - On hotkey, grab the whole virtual desktop (all monitors) into one BGRA buffer before anything is shown.
   - Then show one topmost borderless overlay window covering the virtual desktop, drawing the frozen image.
   - Hotkey to overlay on screen: target under 100 ms on a 4K desktop.

6. **Selection overlay**
   - Frozen image dimmed; the selected area is shown at full brightness with a 1px accent border and handles.
   - Hover without pressing: the top-level window under the cursor is highlighted (real visible bounds from DWM, not the shadowed rect; skips cloaked, minimized, invisible and own windows). Click without dragging captures that window's rectangle, clipped to the desktop.
   - Click and drag: free rectangle. While dragging, a size readout `W x H` sits next to the frame.
   - After release the selection stays editable: drag inside to move, drag edges or corners to resize, arrow keys nudge 1 px (Shift: 10 px). Enter or double-click confirms. A plain release with no editing confirms after release - see below.
   - Default flow: release the mouse = capture is done (Snipping Tool feel). Holding Ctrl while releasing keeps the frame open for adjusting, then Enter confirms.
   - Esc or right click: cancel.
   - Loupe (on by default, `loupe = false` disables): near the cursor, 8x zoom of the pixels around it with a crosshair, cursor coordinates, frame size, and the hex color under the cursor. Pressing C while the overlay is open copies that hex color to the clipboard and closes the overlay.

7. **Top mini panel**
   - Small floating bar at the top center of the monitor under the cursor, on top of the overlay.
   - Items: Photo / Video mode switch (Video disabled with a "soon" tooltip until block 3), save folder dropdown (recent folders plus "Choose folder..."), close button.
   - The panel never ends up inside the capture.

8. **After capture**
   - The cropped image goes straight to the clipboard (CF_DIBV5 with alpha plus a "PNG" format, so both old apps and browsers paste it).
   - If auto-save is on (`auto_save = true`, default false), it is also saved as PNG to the current folder.
   - A preview card appears in the bottom-right corner of the monitor where the capture ended: thumbnail plus buttons Copy, Save, Save to... (recent folders and Choose), Open folder, Edit (disabled until block 4), and a close X.
   - The card hides itself after 6 s unless the mouse is over it. Clicking the thumbnail opens the PNG in the default viewer (saved to a temp file first if needed).
   - Saved file name: `Snap 2026-10-27 01-42-05.png`; no overwrites.

9. **Folders**
   - Default folder: `Pictures\Rengi Snap`.
   - Up to 8 recent folders, most recent first, shared by the panel and the card.

10. **Config**
    - `%APPDATA%\Rengi Snap\config.toml`, created with defaults on first run. Bad values fall back to defaults; the app never refuses to start because of config.

## Out of scope for this block

UI element detection inside windows (block 2), video, audio and GIF (block 3), the editor (block 4), the installer and releases (block 5).

## How this gets checked without anyone at the desktop

A Windows CI job builds the exe and runs an end-to-end test on the runner's real desktop:
- The test opens its own window filled with a known pattern at a known position.
- It sends Scroll Lock, drags a frame over part of the pattern, releases, and checks that the clipboard holds an image of exactly that size whose pixels match the pattern.
- It sends Scroll Lock again, hovers over its window, clicks without dragging, and checks the clipboard image has the window's exact size.
- It checks the preview card window appeared, and that Esc cancels without touching the clipboard.

Pure logic (window picking, selection editing, file naming, config parsing, recent folders) is covered by unit tests that run on Linux and Windows.
