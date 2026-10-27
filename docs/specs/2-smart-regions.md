# Block 2 - Smart regions

## Goal

When I hover in the frozen overlay, the highlight snaps to the element under the cursor (a button, a text field, a picture, a sidebar, a tab strip), not just the whole window. The mouse wheel walks the levels: up makes it bigger (element, panel, window, monitor), down goes back. Click captures what is highlighted. It works in apps that expose nothing to accessibility too (games, Unity, canvas), by finding rectangles in the frozen pixels. The overlay never waits for any of this.

## Scope

1. **Levels chain**
   - For the cursor point, the chain runs from the smallest element up: elements, then the top-level window, then the monitor.
   - Elements smaller than 12x12 px are ignored. Elements are clipped to their window. Rects within 2 px of each other on every side count as one level (the larger one is kept).
   - Default level is the smallest. Wheel up = one level bigger, wheel down = one level smaller. The chosen level is remembered while I move the mouse (counted from the top, so "window level" stays window level across the screen) until the overlay closes.
   - Click without dragging captures the highlighted level. Dragging a frame works as before.

2. **Where elements come from** (in this order, merged into one chain)
   - Child windows: walking child HWNDs under the point (cheap, catches classic Win32 controls and panels).
   - UI Automation: starting from the deepest child HWND, descend the element tree one level at a time, always into the smallest child that contains the point, collecting each rect. Offscreen elements are skipped. Hard limits: 48 levels, 400 ms per query, UIA transaction timeout 250 ms, so a frozen app cannot hang anything.
   - Pixels: when the smallest element found is still big (at least 200x150) or nothing was found, search the frozen image inside it for rectangles: straight edges forming closed boxes around the point (panels, cards, buttons, image frames). These go into the chain like any other element.

3. **Never wait**
   - All element work runs on a background thread. The overlay shows and highlights windows immediately, as in block 1, and sharpens the highlight when an answer arrives (normally within tens of ms).
   - Only the latest query matters; older pending ones are dropped.
   - Idle cost stays zero: the worker thread sleeps on a channel.

4. **Loupe hint**
   - Under the color line: "Wheel  bigger / smaller" and "C  copy color".

## Out of scope

Scrolling capture, video, the editor, OCR.

## How this gets checked without anyone at the desktop

The Windows CI end-to-end test gets two new windows:
- A Win32 window with a bordered child panel and a button inside it. Hover over the button and click: the clipboard image has exactly the button's size. Wheel up once: the panel's size. Wheel up again: the window's size.
- A window with no child controls that only paints a bordered card with a filled button inside (pure pixels). Same three checks via the pixel path.
- Explorer opened on the runner, hovered, with screenshots of the highlight at two wheel levels for visual review.

The pixel rectangle finder and the levels logic are unit tested on synthetic images and point sets.
