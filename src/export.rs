//! Exports: study views at full size (paint-by-number with numbers burned
//! in), palette cards with recipes, and a headless `--study` command.

use crate::color::{describe, hex, lightness, rgb_to_hsv};
use crate::extract::{extract, Swatch};
use crate::filters::{render, upscale_to, work_copy, Mode, NumberLabel, Params};
use crate::palette::{Recipe, Solver};
use image::{Rgba, RgbaImage};
use std::path::Path;

const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Draw `n` centred at (cx, cy) with a tiny bitmap font.
pub fn draw_number(img: &mut RgbaImage, cx: f32, cy: f32, n: usize, scale: u32, color: [u8; 3]) {
    let s = n.to_string();
    let scale = scale.max(1) as i64;
    let glyph_w = 3 * scale;
    let gap = scale;
    let total_w = s.len() as i64 * (glyph_w + gap) - gap;
    let total_h = 5 * scale;
    let x0 = cx as i64 - total_w / 2;
    let y0 = cy as i64 - total_h / 2;
    let (w, h) = (img.width() as i64, img.height() as i64);
    for (gi, ch) in s.chars().enumerate() {
        let Some(d) = ch.to_digit(10).and_then(|d| DIGITS.get(d as usize)) else { continue };
        let gx = x0 + gi as i64 * (glyph_w + gap);
        for (row, bits) in d.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for yy in 0..scale {
                    for xx in 0..scale {
                        let px = gx + col * scale + xx;
                        let py = y0 + row as i64 * scale + yy;
                        if (0..w).contains(&px) && (0..h).contains(&py) {
                            img.put_pixel(px as u32, py as u32, Rgba([color[0], color[1], color[2], 255]));
                        }
                    }
                }
            }
        }
    }
}

/// Upscale a study render to the original size; burn in PBN numbers.
pub fn study_full_size(study: &RgbaImage, labels: &[NumberLabel], full_w: u32, full_h: u32) -> RgbaImage {
    let mut out = upscale_to(study, full_w, full_h);
    let sx = full_w as f32 / study.width().max(1) as f32;
    for l in labels {
        let room = l.room * sx;
        let scale = ((room / 5.0).floor() as u32).clamp(1, 6);
        if room < 4.0 {
            continue;
        }
        draw_number(&mut out, l.x * sx, l.y * sx, l.number, scale, [90, 90, 96]);
    }
    out
}

/// A palette card: numbered swatches beside their predicted mixes.
pub fn palette_card(swatches: &[Swatch], recipes: &[Recipe]) -> RgbaImage {
    let row = 64u32;
    let w = 360u32;
    let h = (swatches.len() as u32 * row).max(row);
    let mut img = RgbaImage::from_pixel(w, h, Rgba([245, 243, 238, 255]));
    for (i, s) in swatches.iter().enumerate() {
        let y = i as u32 * row;
        for yy in y + 6..y + row - 6 {
            for xx in 40..160 {
                img.put_pixel(xx, yy, Rgba([s.rgb[0], s.rgb[1], s.rgb[2], 255]));
            }
            if let Some(r) = recipes.get(i) {
                for xx in 170..290 {
                    img.put_pixel(xx, yy, Rgba([r.predicted[0], r.predicted[1], r.predicted[2], 255]));
                }
            }
        }
        draw_number(&mut img, 20.0, y as f32 + row as f32 / 2.0, i + 1, 3, [60, 60, 60]);
        // Value bar 0..10 on the right.
        let v = (lightness(s.rgb) / 10.0).round() as u32;
        draw_number(&mut img, 325.0, y as f32 + row as f32 / 2.0, v as usize, 3, [60, 60, 60]);
    }
    img
}

pub fn recipes_text(swatches: &[Swatch], recipes: &[Recipe], solver: &Solver, source: &str) -> String {
    let mut s = format!("Palette study — {source}\nMixing palette: {}\n\n", solver.palette.name);
    for (i, sw) in swatches.iter().enumerate() {
        let hsv = rgb_to_hsv(sw.rgb);
        s.push_str(&format!(
            "{}. {}  {:.0}% of image   HSV {:.0}°/{:.0}%/{:.0}%   value {:.1}/10   {}\n",
            i + 1,
            hex(sw.rgb),
            sw.share * 100.0,
            hsv[0],
            hsv[1],
            hsv[2],
            lightness(sw.rgb) / 10.0,
            describe(sw.rgb)
        ));
        if let Some(r) = recipes.get(i) {
            for line in r.describe(&solver.palette) {
                s.push_str(&format!("     {line}\n"));
            }
            s.push_str(&format!("     → predicted {}  (ΔE {:.1}, {})\n\n", hex(r.predicted), r.delta_e, r.accuracy_word()));
        }
    }
    s
}

/// Headless: `viewer --study <image> [out_dir] [colours]` renders every
/// study view plus the palette card and recipes.
pub fn study_cli(input: &Path, out_dir: &Path, k: usize, solver: &Solver) -> Result<String, String> {
    let full = crate::loader::decode(input)?;
    let work = work_copy(&full);
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let sw = extract(&work, k);
    let recipes: Vec<Recipe> = {
        use rayon::prelude::*;
        sw.par_iter().map(|s| solver.solve(s.rgb)).collect()
    };
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("image".into());
    let p = Params::default();
    for m in Mode::ALL {
        if m == Mode::Original {
            continue;
        }
        let r = render(&work, m, &p, &sw, None);
        let img = study_full_size(&r.image, &r.labels, work.width(), work.height());
        let name = format!("{stem}_{}.png", m.label().split_whitespace().next().unwrap_or("view").to_lowercase());
        img.save(out_dir.join(name)).map_err(|e| e.to_string())?;
    }
    palette_card(&sw, &recipes).save(out_dir.join(format!("{stem}_palette.png"))).map_err(|e| e.to_string())?;
    let text = recipes_text(&sw, &recipes, solver, &input.display().to_string());
    std::fs::write(out_dir.join(format!("{stem}_recipes.txt")), &text).map_err(|e| e.to_string())?;
    Ok(text)
}
