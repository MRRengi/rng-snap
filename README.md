# Rengi Snap

Free, open-source screenshot and screen recording tool for Windows. One small native exe (Rust, no runtime, no installer), built to open instantly and stay out of the way.

**[Download the latest release](https://github.com/MRRengi/rng-snap/releases/latest)**

## Screenshots
- **Scroll Lock** freezes every monitor; drag a frame, or click what is highlighted
- Smart regions: hover snaps to buttons, panels, rows and windows (child windows + UI Automation + pixel boxes for games and canvas apps); the **mouse wheel** walks element > panel > window > monitor
- Loupe with coordinates, frame size and pixel color (**C** copies the hex)
- Instantly on the clipboard; a preview card offers Copy / Save / Save to / Folder

## Video
- Up to your monitor's refresh rate (120 / 144 / 240 fps), straight from the GPU into its hardware encoder (H.264, HEVC when needed)
- System sound and microphone, pause, discard; the control bar never appears in the video
- MP4 saved to your folder; Copy puts the file on the clipboard for Telegram / Discord; one-click GIF export

## Notes
- Windows 10 2004+; Windows 11 recommended
- The exe is not signed, so SmartScreen may warn on first run: More info > Run anyway
- Settings live in `%APPDATA%\Rengi Snap\config.toml`

## Building
```
cargo build --release
```
CI builds on Windows, runs unit tests and an end-to-end test that drives the app on a real desktop, and publishes a release when the version in `Cargo.toml` goes up.

MIT license.
