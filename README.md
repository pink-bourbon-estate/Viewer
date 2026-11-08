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

## Using it

| Key | Action |
| --- | --- |
| ← → / Space / Backspace | previous / next image in the folder |
| Wheel · drag · double-click | zoom at cursor · pan · fit ↔ 100% |
| F / Z | fit / actual size |
| 0–8 | Original, B&W, Value steps, Notan, Grisaille, Simplify, Blocks, Paint-by-number, Line art |
| S | split compare — drag the divider between original and study |
| M / G / Q | mirror · grid (thirds / quarters / squares) · squint blur |
| I or Ctrl+click | pipette: sample any spot and get its mix |
| T | always on top (float a reference over your painting app) |
| Tab | hide all panels · B / C toggle browser / colour panel · F11 full screen |
| Ctrl+R / Ctrl+L | rotate file(s) right / left |
| F2 · Del | rename · move to recycle bin |
| Ctrl+B | batch: rotate, flip, resize, convert, rename (`study_{n:3}`), into a subfolder or in place |
| Ctrl+E | export the current study view at full size (paint-by-number numbers burned in) |
| Right-click | everything else |

Click a colour card to **isolate** that colour on the canvas.

### Headless study export
```
viewer --study painting.jpg out_folder 10
```
Writes every study view, a numbered palette card and `*_recipes.txt` for 10 colours.

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
