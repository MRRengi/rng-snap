# Block 6 - Setup and the Liquid Glass redesign

## What Maks asked for
- "No idea what's where and how" and "why a bare exe?": a real installer, and a UI that explains itself.
- Answers:
  - **Setup + portable zip**.
  - **macOS Tahoe Liquid Glass** look for the whole UI.
  - A **Home window** after launch.
  - **Follow the Windows theme** (light and dark).
- Design skills used as the bar: apple-design (HIG, Liquid Glass rules), emil-design-eng (motion and polish), impeccable, design-taste-frontend.

## Install
- **Setup** (`RengiSnap-Setup-<ver>.exe`, Inno Setup 6):
  - Per-user by default, no admin prompt. Installs to `%LOCALAPPDATA%\Programs\Rengi Snap`. Can install for all users from the dialog.
  - Adds a Start menu shortcut.
  - Optional tasks:
    - Desktop icon (off by default).
    - **Start with Windows** (on).
    - **"Open with Snap"** for `.mp4` and `.png` (on).
  - Finish page: "Launch Snap" (opens the Home window).
  - Updating over an old version and uninstalling close the running app first (`--quit`). Uninstall removes the autostart value and the "Open with" keys. Captures and settings stay.
- **Portable zip** (`RengiSnap-<ver>-portable.zip`): the exe plus a short read-me.
- **Exe resources**: the app icon (16-256 px, drawn by `src/icon.rs`) and version info ("Snap by rng"), so Explorer, the taskbar, Task Manager and the installer all show the right name and icon.
  - `build.rs` writes a `.res` file and hands it to the MSVC linker. No resource compiler or extra crate is needed.
- **CI** builds the setup and the zip, tests them, and attaches both to the GitHub release.

## Command line
| Argument | Effect |
|---|---|
| none | Start, or show Home if already running |
| `--background` | Start in the tray only (autostart uses this) |
| `--capture` | Start a capture |
| `--edit <file>` | `.mp4` opens the video editor, `.png` opens the photo editor |
| `--quit` | Close the running instance |

## Theme
- Light or dark follows Windows "app mode" and switches live (`WM_SETTINGCHANGE` "ImmersiveColorSet").
- All windows share one module, `src/win/theme.rs`: Apple system colours, the glass material, fonts and radii.
- **Colours**:
  - Accent: systemBlue (`#0A84FF` dark, `#007AFF` light).
  - Red, purple and teal: Apple system values.
  - Label, secondary and tertiary text: white or black at 92/55/28%.
  - Separators and fills: at 10/8/16%.
- **Glass**: floating rounded shapes (capsules for toolbars) made of five layers, back to front:
  1. A soft shadow.
  2. The content behind, blurred and slightly saturated. This is real wherever the content is ours (overlay, editors).
  3. A tint (dark: graphite at 55%, light: white at 60%).
  4. A gradient rim that is bright at the top and faint at the bottom.
  5. A thin outer hairline.
  - Popups whose backdrop is the desktop (card, recording bar) use the system acrylic backdrop when Windows offers it. Otherwise they use an opaque tint of the same colour.
- **Type**: Segoe UI Variable (Text and Display) when present, else Segoe UI. Icons are Segoe Fluent Icons; the tool icons are tinted to the theme.
- **Motion**:
  - Hover fills fade in over 120 ms (ease-out).
  - The card slides up 12 px and fades in.
  - Nothing animates on keyboard actions or on the capture overlay, which has to appear instantly.

## Home window
- Opens:
  - On a normal launch.
  - From a left click on the tray icon.
  - When the exe or the Start menu shortcut is opened again.
- Closing it hides it. The app keeps running in the tray, and the footer says so.
- **Layout** (Mica window, glass cards):
  - **Header**: the app icon, "Snap" and "by rng".
  - **Three action tiles**, each with its key shown as a keycap:
    - **Screenshot** - Scroll Lock.
    - **Record video** - opens the overlay already in Video mode.
    - **Edit a video** - pick an mp4.
  - **Recent**: the 8 latest captures in the save folder (png, mp4, gif) as rounded thumbnails, with the name and "2 min ago".
    - Clicking a png opens the photo editor and an mp4 opens the video editor. A gif opens with Windows.
    - Right click: show in folder.
    - Empty state: "Your captures will show up here".
  - **Settings** as grouped rows:
    - Shortcut: a menu of common keys - Scroll Lock, Print Screen, Ctrl+Shift+S, Ctrl+Shift+X, Ctrl+Alt+S.
    - Save to: a folder picker.
    - Start with Windows: an Apple-style switch.
    - Open captures folder.

## Restyled windows
- **Capture overlay**:
  - The top panel becomes a glass capsule. Photo/Video is a segmented control with a sliding glass thumb.
  - The size label and the loupe become glass.
  - Selection handles become white circles with a shadow.
  - The quick-marks strip becomes a glass capsule.
- **Preview card**: a glass card with a rounded thumbnail, a row of icon buttons with labels, and a small round close button.
- **Recording bar**: a glass capsule with a red dot and the timer in fixed-width digits, followed by round icon buttons.
- **Photo editor**:
  - The image fills the window on a themed canvas.
  - The toolbar is a floating glass capsule at the top centre, over the image and blurring it.
  - Zoom and status are small glass pills at the bottom.
- **Video editor**:
  - The same floating toolbar.
  - The transport and timeline sit on a glass sheet at the bottom.
  - Clips have 8 px corners, annotation bars are capsules, and the playhead has a round knob.
- **Tray menu** follows the theme.

## Tests
- Windows publish where their buttons are (`ui.txt` in the test home, written when `RENGI_SNAP_HOME` is set). The end-to-end test clicks by name instead of by hard-coded pixels.
- New checks:
  - **Home**:
    - Shows on a second launch.
    - The Screenshot tile opens the overlay, and Record video opens it in Video mode.
    - Recent lists the test captures.
    - Screenshots are taken in light and dark.
  - **Theme**: the editor, video editor, card and overlay are filmed in both themes.
  - **Installer** (CI step):
    - A silent per-user install puts the exe, the shortcut, the Run value and the Open-with keys in place.
    - The installed app starts.
    - A silent uninstall removes everything and stops the app.
  - **Zip**: contains the exe.
- Version 0.6.0.
