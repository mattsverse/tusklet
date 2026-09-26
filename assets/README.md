# Tusklet artwork

`source/tusklet-reference.png` preserves the supplied artwork unchanged. The two transparent masters were prepared from that reference with the built-in imagegen tool:

- `source/tusklet.png`: color logo with its gray background removed and the character centered. Used for the app sidebar and packaged application icon.
- `source/tray.png`: simplified black outline of the same elephant/database character, with transparent interiors. Used for the macOS template icon; opaque white fills would become solid areas in the menu bar.

The runtime assets are committed. Regenerate them from the repository root with ImageMagick:

```sh
magick assets/source/tusklet.png -resize 1024x1024 -strip assets/tusklet.png
magick assets/source/tray.png -trim +repage -resize 40x32 -gravity center -background none -extent 44x36 -strip assets/tray.png
```

The packaged icon remains square. The tray asset is 44 × 36 pixels, displayed by `tray-icon` at 22 × 18 logical points on macOS, with room around the artwork for antialiasing. Keep `.with_icon_as_template(true)` in `src/tray.rs` so the system chooses its light or dark appearance.

## Image preparation prompts

Both edits used `source/tusklet-reference.png` as the edit target and requested a transparent background. Keep the committed masters to reproduce runtime assets without another generative edit.

### Color logo

```text
Use case: background-extraction
Asset type: Tusklet desktop app logo and packaged application icon, square PNG.
Input image 1 is the edit target, the user's exact chosen logo: a friendly blue database cylinder/elephant with ivory tusks and navy outlines.
Primary request: remove the gray background completely and return the isolated original logo on genuine transparency. Preserve the exact original character design, navy contours, blue shading, cream tusks, smiling closed eyes, trunk shape, and cylinder proportions. Do not redesign, redraw in a different style, or add anything. Reframe the existing artwork so its full visible bounds are centered horizontally and vertically in a square canvas with about 8% transparent margin on the left and right. The whole character including tusk tips and bottom must fit. No text, shadow, tile, added border, or background. Keep all spaces between tusks and cylinder transparent.
```

### Tray icon

```text
Use case: style-transfer
Asset type: monochrome macOS menu bar template icon for Tusklet.
Input image 1 is the edit target and must determine the character identity and silhouette.
Primary request: adapt this exact friendly elephant/database logo into a very simple, clean, black-only template icon on genuine transparency, readable at just 22 pixels across. Preserve the short wide database cylinder, two prominent upward-curving tusks, centered hanging trunk curled to the right, and two happy closed-eye arches. Use thick smooth solid black contours and transparent open interiors. The top cylinder ellipse, tusks, eyes, and trunk must stay distinguishable when tiny. Simplify away shading, highlights, texture, trunk wrinkles, and minor database grooves; keep only the top ellipse and one side cylinder division where space permits. All formerly blue or ivory filled interiors must become transparent, NOT opaque white. Black ink and transparent negative space ONLY. No gray background, white background, opaque white pixels, color, shadows, gradients, text, tiles, or extra elements. Center full artwork in a square canvas, filling approximately 90% of its width with equal left/right margin. Do not crop any tusk tips.
```
