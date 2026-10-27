# Block 4 - Editor

## Goal

I can mark up a screenshot two ways. Quick: release the frame with Ctrl held, a tool strip appears next to it, I draw an arrow or blur a password right on the frozen screen and press Enter. Full: the Edit button on the preview card opens an editor window with the same tools plus crop, zoom and undo. The look is Snagit's: tapered arrows with a soft shadow, clean numbered steps, readable text. The clipboard always holds the latest marked-up version, so Ctrl+V anywhere just works.

## Tools (both places, crop only in the editor)

| Key | Tool | Behaviour |
|---|---|---|
| V | Select | click an item to select, drag to move, drag handles to resize / move endpoints, Delete removes, double-click text to edit it |
| A | Arrow | tapered shaft, solid head, soft drop shadow |
| L | Line | round caps, Shift snaps to 45 degrees (also for arrows) |
| R | Rectangle | outline, Shift = square |
| E | Ellipse | outline, Shift = circle |
| P | Pen | freehand, smoothed |
| H | Highlighter | wide translucent marker that multiplies with what is under it |
| T | Text | click to place, type, Enter for a new line, Esc or click outside to finish; coloured text with a contrasting outline |
| B | Blur | drag a box; toggle in the strip between blur and pixelate |
| N | Steps | click to drop numbered circles 1, 2, 3...; numbers follow the order of the steps left on the image |
| C | Crop | editor only; drag the new edges, Enter applies, Esc cancels |

- Style: 8 colours (red, orange, yellow, green, blue, purple, black, white) plus a custom colour picker; 3 thicknesses. The last choice is remembered.
- Ctrl+Z / Ctrl+Y undo and redo everything (items, moves, deletes, crop).

## Quick mode on the overlay

- Releasing a frame with Ctrl held keeps it, as in block 1; now a tool strip appears beside the frame (below it, above it if there is no room, inside as a last resort).
- The strip: Select (move/resize the frame itself), the drawing tools, colour, thickness, Undo, then Copy (= Enter), Save, and Open in editor.
- Drawing is clipped to the frame. Moving or resizing the frame keeps the marks where they are on screen.
- Enter / double-click / Copy finishes: the marked-up image goes to the clipboard and the preview card as usual.

## Editor window

- Opens from the card's Edit button (marks made on the overlay stay editable) and keeps one window per capture.
- Top toolbar: tools, colour, thickness, blur/pixelate toggle, Undo, Redo, then Copy, Save (Ctrl+S, overwrites the file it saved before), Save to (recent folders / picker).
- Canvas: image centred on a dark background, Ctrl+wheel zooms around the cursor, space+drag or middle-drag pans, fits the window on open, 100% at most.
- Every change is copied to the clipboard about 150 ms later (bitmap + PNG on demand, as for captures).
- Normal resizable window, remembers nothing between sessions except colour/thickness/tool.

## How this gets checked without anyone at the desktop

Rendering and editing logic are pure Rust (tiny-skia + ab_glyph), unit tested on synthetic images: arrow/line/rect/ellipse land where expected, highlighter multiplies, blur lowers detail, pixelate makes uniform blocks, step numbers renumber, text draws, crop and undo/redo restore exact pixels, hit-testing picks the topmost item.

The Windows CI end-to-end test:
- Overlay: Ctrl-release a frame, press A, drag an arrow, Enter: the clipboard image has the frame's size and arrow-red pixels along the drag line.
- Editor: Edit on the card opens the window; a rectangle, a step and a blur each update the clipboard by themselves; crop + Enter changes the clipboard size, Ctrl+Z restores it; Ctrl+S writes a PNG.
- Screenshots of the strip and the editor for visual review.
