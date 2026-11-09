# Viewer

A fast, minimal viewer for images, documents, audio and 3D — and a study tool for painters. It browses folders like Adobe Bridge, views like PureRef, batch-edits like the old Windows Photo Viewer, and adds study tools: colour extraction with oil-paint mixing recipes, value studies, grisaille, notan, paint-by-number and line art.

One ~12 MB Rust binary (egui). No runtime, no Electron.

## Not just images

The type filter (next to the sort menu) shows **All**, Images, Documents, Text & code, Audio or 3D.
Non-image files open in the preview pane — quick to read, quick to edit, no big app needed.
**Open in…** hands any file to its usual program; **Show in folder** finds it.

| Type | In Viewer |
| --- | --- |
| Markdown (`.md`) | **Edit live** — Preview / Split / Edit, autosave, Ctrl+S, find & replace. `[[links]]` open notes in the same folder; notes elsewhere in the open folder tree show their location with *Open* / *Show in folder*; a missing note can be created. **Linked mentions** lists notes that link here. No vault indexing, so huge note folders stay fast. |
| Text & code (`.txt .json .toml .yaml .js .rs .c …`) | Edit with syntax colouring, find & replace, autosave. Files changed by another program are noticed. |
| Python (`.py`) | Edit, then **▶ Run** (Ctrl+Enter) with your installed Python; live output, Stop, arguments. |
| CSV / TSV | Editable grid: click a cell, Enter to confirm, add rows/columns, Save. |
| Excel / ODS (`.xlsx .xls .ods`) | Read-only sheets with tabs. |
| Word (`.docx`) | Readable preview: headings, lists, tables, pictures. Editing stays in WordCraft / Word. |
| PDF | Pages, zoom (Ctrl+wheel), fit width. |
| Audio (`.mp3 .wav .flac .ogg .m4a .aac .aiff`) | Waveform, play/pause (Space), seek by clicking, loop, volume. Previews load up to 30 minutes. |
| 3D (`.obj .stl .ply .gltf .glb .3mf`) | Orbit (drag), pan (right-drag), zoom (wheel), wireframe, Y-up toggle, size and triangle counts. Drawn on the CPU, so a bad model can't upset the graphics driver. |
| 3D via Blender (`.fbx .abc .blend .dae .usd …`) | Blender converts them in the background (cached). **Export to .blend** saves a `.blend` next to the file. Viewer finds Blender automatically or via *Set Blender location…*. |
| 3ds Max (`.max`) | Can't be opened outside 3ds Max — export from Max as FBX or OBJ. |

Saving always goes through a temporary file, so a crash can't leave half a file.

## Updates

Every change pushed to GitHub is built automatically and published on the repo's **Releases** page
(`build-N`, with `viewer.exe`, the Linux binary and SHA-256 checksums).

Viewer checks for a newer build when it starts (at most every 6 hours). When one exists, an
**⬆ Update to build N** button appears in the toolbar: click it and Viewer downloads the new
version, verifies its checksum, replaces itself and restarts on the same image. You can also use
**? → Check for updates**, or run `viewer --update`. Turn the automatic check off in the **?** menu.

Keep `viewer.exe` in a folder you can write to (e.g. `C:\Tools\Viewer`), not Program Files.

## Install

### Windows
1. Download `viewer.exe` from the latest release on the **Releases** page
   (or build it: install Rust from https://rustup.rs, then `cargo build --release`).
2. Put it somewhere permanent you can write to, e.g. `C:\Tools\Viewer\viewer.exe`.
3. (Optional, for SmartScreen) right-click → Properties → tick *Unblock*.
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
