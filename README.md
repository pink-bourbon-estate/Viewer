# Viewer

A fast, minimal image viewer for painters. It browses folders like Adobe Bridge, views like PureRef, batch-edits like the old Windows Photo Viewer, and adds study tools: colour extraction with oil-paint mixing recipes, value studies, grisaille, notan, paint-by-number and line art.

One ~12 MB Rust binary (egui). No runtime, no Electron.

## Install

### Windows
1. Install Rust from https://rustup.rs (default options).
2. In this folder: `cargo build --release`
3. Copy `target\release\viewer.exe` somewhere permanent, e.g. `C:\Tools\Viewer\viewer.exe`.
4. Make it the default viewer: run `viewer.exe --register` (or right-click the canvas → *Make Viewer the default image viewer*). Windows then opens Settings → Default apps → Viewer; click **Set default**. Windows requires this last click from you; no program may take it over silently.
5. To undo: `viewer.exe --unregister`.

Alternatively push this folder to GitHub: the included workflow (`.github/workflows/build.yml`) builds `viewer.exe` and the Linux binary as downloadable artifacts.

### Linux
1. Put the `viewer` binary somewhere permanent, e.g. `~/.local/bin/viewer` (or build: `cargo build --release`).
2. `viewer --register` writes `~/.local/share/applications/io.github.pink_bourbon_estate.Viewer.desktop` and runs `xdg-mime default` for every image type (needs `xdg-utils`).
3. To undo: `viewer --unregister`, then pick another viewer in your desktop settings.

## Layout

Bridge-style browser on the left, the image on the right, panels docked around them.

- **Centre:** `Grid`, `Split` (browser + viewer) or `Viewer` (V cycles). Drag the divider to resize.
- **Folder tabs** under the toolbar, like a browser: `+` / Ctrl+T for a new tab, middle-click or `×` to close.
- **Panels:** Folders, Metadata, Develop, Colours, Composition, Art views, Export. They sit in four stacks
  (left top/bottom, right top/bottom). Click a tab to switch; **right-click a tab to move it** to another stack or hide it;
  *Panels* (top right) shows hidden ones or resets the layout. Tab hides every panel.

## Browsing (Bridge)

- **Folders panel:** favourites (☆ next to the path pins a folder), collections, and a folder tree.
- **Toolbar:** back / forward / up, clickable path, **Subfolders** (show every image below this folder), search (names and keywords).
- **Content:** tiles, thumbnails only, or details list (sortable columns); `− slider +` or Ctrl+wheel to zoom thumbnails;
  sort by name, date, size, rating or type; filter by minimum stars. Folders show as tiles: double-click to go in.
- **Metadata panel:** star rating, keywords (with suggestions), collection membership, file properties and camera EXIF.
  Applies to the whole selection. Stored in `library.json` in the config folder.

## Develop (presets and adjustments)

- **Looks:** category cards (Black & White, Kodak, Fuji, Ilford, Leica, Agfa, Cinema, Instant & Vintage, plus your own).
  Each preset shows a live preview on the current image; click to apply, then set its **strength**.
- 41 built-in film emulations (Portra 160/400/800, Ektar, Gold, Kodachrome, Ektachrome, Tri-X, T-Max, Pro 400H, 160NS,
  Superia, Velvia, Provia, Astia, Classic Chrome / Negative, Acros, HP5, Delta 3200, FP4, Pan F, XP2, Leica looks,
  Agfa Vista / APX, CineStill 800T / 50D, Vision3 500T, Eterna, Teal & Orange, Bleach Bypass, Polaroid, Instax,
  Autochrome, Expired) — built from published descriptions of each stock; the values live in `tools/gen_film_presets.py`.
- **Adjust:** light (exposure, contrast, highlights, shadows, whites, blacks), colour (temperature, tint, vibrance, saturation),
  effects (clarity, dehaze, fade, vignette, grain), **curves** (RGB + R/G/B, drag points), HSL colour mixer, colour grading,
  black & white mixer, calibration. Double-click a slider to reset it.
- **Save as new preset** forks the current settings into *My presets* (or any category). Copy / Paste settings between images.
- **Import preset folder…** reads a folder of Lightroom presets (`.xmp`, `.lrtemplate`) and `.cube` LUTs, skips
  non-look presets (sharpening, noise, lens, brushes) and camera profiles, groups near-identical looks by how they change
  a colour chart, and keeps about 45 averaged presets — saved inside Viewer, so the original folder can be deleted.
  Same thing from the command line: `viewer --condense <folder> out.json 45`.
- Edits are non-destructive until you save. **Split (S)** compares original and edit.

## Composition, art views, export

- **Composition:** RGB + luminance histogram with clipping and value zones; overlays (rule of thirds, golden ratio,
  centre, quarters, diagonals, dynamic-symmetry armature, square grid) in any colour/opacity; squint blur; mirror.
- **Art views (0–8):** original, B&W, value steps, notan, grisaille (burnt umber + ultramarine), simplify, blocks,
  paint-by-number, line art — applied on top of any look.
- **Export:** crop (free or fixed ratio; drag corners or move), straighten slider or **Level by line**, rotate 90°, flip;
  save a copy (`_edit` suffix), save as, or overwrite; JPEG quality and long-edge resize; apply the current look to
  every selected image at once. Batch rename / convert is still on Ctrl+B.

## Keys

| Key | Action |
| --- | --- |
| ← → / Space | previous / next image · ↑ ↓ move by row in the grid |
| Alt+← → ↑ / Backspace | back / forward / up a folder |
| Enter / double-click | open folder, or switch to the viewer |
| V · Tab · F11 | grid / split / viewer · hide panels · full screen |
| Ctrl+T / Ctrl+W | new tab / close tab |
| Ctrl+0…5 | star rating |
| Wheel · drag · double-click | zoom at cursor · pan · fit ↔ 100% |
| F / Z | fit / actual size |
| 0–8 | art views |
| S | split compare original ↔ edit |
| G / Shift+G | thirds / golden-ratio overlay |
| M / Q / T | mirror / squint / always on top |
| C | crop tool (Enter to finish) |
| I or Ctrl+click | pipette: sample any spot and get its mix |
| Ctrl+Shift+C / V | copy / paste develop settings |
| Ctrl+E | save an edited copy |
| Ctrl+R / Ctrl+L | rotate file(s) right / left |
| F2 · Del · F5 | rename · recycle bin · refresh |
| Ctrl+B | batch rotate / resize / convert / rename |
| Right-click | everything else |

### Headless study export
```
viewer --study painting.jpg out_folder 10
```

## The colour panel

For each dominant colour (k-means in OKLab):
- hex, share of the image, HSV, CIE L*, and **value 0–10** (Munsell-style, from L*)
- a painter's description: "mid, muted red-orange, warm"
- a **recipe from Patrick's palette**: Titanium White, Cadmium Yellow Light, Cadmium Yellow Medium, Cadmium Orange, Cadmium Red Light, Quinacridone Magenta, Ultramarine Blue, Cobalt Blue, Viridian, Ivory Black, Burnt Sienna, Yellow Ochre
- the predicted mix (corner of the swatch) and ΔE2000 to the target

Mixing is simulated with a Kubelka–Munk model (subtractive: blue + yellow gives green, white lightens and cools). The solver searches every combination of up to three colours plus white, and prefers simpler piles. Treat a recipe as the starting pile, then adjust by eye.

**Calibrate it:** *Edit palette…* writes `palette.json` to your config folder (`%APPDATA%\Viewer\config` on Windows, `~/.config/viewer` on Linux). Change names, add or remove paints, or set each `rgb` from a photo of your own masstone swatch. `scatter` is hiding power (white 1.0, transparent paints ~0.1–0.2) and `strength` is tinting strength. Click *Reload* afterwards.

## Formats
JPEG, PNG, WebP, GIF, BMP, TIFF, TGA, ICO, QOI, PBM/PGM/PPM. EXIF rotation is respected. RAW, HEIC and AVIF are not supported.

## Development
```
cargo test --release
cargo clippy --release
cargo check --features mockwin   # type-checks the Windows registry code on Linux
```
Follows the PhotoCraft / LightCraft house rules: no `unwrap` / `expect` / `panic` / `unsafe` in app code.
