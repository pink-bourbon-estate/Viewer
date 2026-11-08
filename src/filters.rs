//! Study views. Every filter works on a downscaled "work" copy of the image
//! (long edge ≤ WORK_EDGE) so sliders stay interactive.

use crate::color::{lightness, lstar_to_y, to_linear, to_srgb};
use crate::extract::{nearest, pixel_oklab, Swatch};
use crate::palette::grisaille_lut;
use image::RgbaImage;
use rayon::prelude::*;

pub const WORK_EDGE: u32 = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Original,
    Grayscale,
    Values,
    Notan,
    Grisaille,
    Simplify,
    Mosaic,
    PaintByNumber,
    LineArt,
}

impl Mode {
    pub const ALL: [Mode; 9] = [
        Mode::Original,
        Mode::Grayscale,
        Mode::Values,
        Mode::Notan,
        Mode::Grisaille,
        Mode::Simplify,
        Mode::Mosaic,
        Mode::PaintByNumber,
        Mode::LineArt,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Original => "Original",
            Mode::Grayscale => "Black & white",
            Mode::Values => "Value steps",
            Mode::Notan => "Notan (2 values)",
            Mode::Grisaille => "Grisaille (umber + ultramarine)",
            Mode::Simplify => "Simplify (colour masses)",
            Mode::Mosaic => "Blocks (mosaic)",
            Mode::PaintByNumber => "Paint by number",
            Mode::LineArt => "Line art",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Mode::Original => "0",
            Mode::Grayscale => "1",
            Mode::Values => "2",
            Mode::Notan => "3",
            Mode::Grisaille => "4",
            Mode::Simplify => "5",
            Mode::Mosaic => "6",
            Mode::PaintByNumber => "7",
            Mode::LineArt => "8",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Params {
    pub value_steps: u32,
    pub notan_threshold: f32,
    pub grisaille_steps: u32,
    pub grisaille_temp: f32,
    pub simplify_smooth: u32,
    pub mosaic_block: u32,
    pub pbn_min_area: u32,
    pub pbn_fill: bool,
    pub line_detail: u32,
    pub line_weight: f32,
    pub squint: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            value_steps: 5,
            notan_threshold: 50.0,
            grisaille_steps: 0,
            grisaille_temp: 0.45,
            simplify_smooth: 3,
            mosaic_block: 16,
            pbn_min_area: 500,
            pbn_fill: false,
            line_detail: 2,
            line_weight: 22.0,
            squint: 0,
        }
    }
}

/// A paint-by-number label position in work-image pixels.
#[derive(Clone, Debug)]
pub struct NumberLabel {
    pub x: f32,
    pub y: f32,
    pub number: usize,
    pub room: f32,
}

pub struct Rendered {
    pub image: RgbaImage,
    pub labels: Vec<NumberLabel>,
}

/// Downscale to the work size (keeps aspect).
pub fn work_copy(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= WORK_EDGE || long == 0 {
        return img.clone();
    }
    let s = WORK_EDGE as f32 / long as f32;
    let nw = ((w as f32 * s).round() as u32).max(1);
    let nh = ((h as f32 * s).round() as u32).max(1);
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

// ---------- blurs on float planes ----------

fn blur_rows(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    out.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, s)| {
        let norm = 1.0 / (2 * r + 1) as f32;
        let at = |i: isize| s[i.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for x in 0..w {
            o[x] = acc * norm;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
    out
}

fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src[y * w + x];
        }
    });
    out
}

/// Approximate Gaussian: three box passes in each direction.
pub fn blur_plane(src: &[f32], w: usize, h: usize, r: u32) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return src.to_vec();
    }
    let r = r as usize;
    let mut a = src.to_vec();
    for _ in 0..3 {
        a = blur_rows(&a, w, h, r);
    }
    let mut t = transpose(&a, w, h);
    for _ in 0..3 {
        t = blur_rows(&t, h, w, r);
    }
    transpose(&t, h, w)
}

fn split_linear(img: &RgbaImage) -> [Vec<f32>; 3] {
    let n = (img.width() * img.height()) as usize;
    let mut r = Vec::with_capacity(n);
    let mut g = Vec::with_capacity(n);
    let mut b = Vec::with_capacity(n);
    for p in img.pixels() {
        r.push(to_linear(p[0]));
        g.push(to_linear(p[1]));
        b.push(to_linear(p[2]));
    }
    [r, g, b]
}

fn join_linear(planes: &[Vec<f32>; 3], w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        p[0] = to_srgb(planes[0].get(i).copied().unwrap_or(0.0));
        p[1] = to_srgb(planes[1].get(i).copied().unwrap_or(0.0));
        p[2] = to_srgb(planes[2].get(i).copied().unwrap_or(0.0));
        p[3] = 255;
    });
    out
}

pub fn blur_rgba(img: &RgbaImage, r: u32) -> RgbaImage {
    if r == 0 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let [r0, g0, b0] = split_linear(img);
    let planes = [
        blur_plane(&r0, w as usize, h as usize, r),
        blur_plane(&g0, w as usize, h as usize, r),
        blur_plane(&b0, w as usize, h as usize, r),
    ];
    join_linear(&planes, w, h)
}

fn map_pixels(img: &RgbaImage, f: impl Fn([u8; 3]) -> [u8; 3] + Sync) -> RgbaImage {
    let mut out = img.clone();
    out.as_mut().par_chunks_mut(4).for_each(|p| {
        let c = f([p[0], p[1], p[2]]);
        p[0] = c[0];
        p[1] = c[1];
        p[2] = c[2];
    });
    out
}

fn grey_of(l: f32) -> [u8; 3] {
    let g = to_srgb(lstar_to_y(l));
    [g, g, g]
}

/// Quantise L* into `steps` bands; returns the band centre.
pub fn quantize_l(l: f32, steps: u32) -> f32 {
    if steps < 2 {
        return l;
    }
    let s = steps as f32;
    let band = ((l / 100.0) * s).floor().clamp(0.0, s - 1.0);
    // Darkest band paints at the darkest value, lightest at near-white.
    band / (s - 1.0) * 92.0 + 4.0
}

// ---------- labels / segmentation ----------

pub fn label_map(img: &RgbaImage, swatches: &[Swatch], smooth: u32) -> Vec<u8> {
    let src = if smooth > 0 { blur_rgba(img, smooth) } else { img.clone() };
    let cents: Vec<[f32; 3]> = swatches.iter().map(|s| s.lab).collect();
    if cents.is_empty() {
        return vec![0; (img.width() * img.height()) as usize];
    }
    src.as_raw()
        .par_chunks_exact(4)
        .map(|p| nearest(&cents, &pixel_oklab(p)) as u8)
        .collect()
}

fn majority(labels: &[u8], w: usize, h: usize, r: isize, k: usize) -> Vec<u8> {
    let mut out = vec![0u8; labels.len()];
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        let mut counts = vec![0u16; k.max(1)];
        for (x, o) in row.iter_mut().enumerate() {
            counts.iter_mut().for_each(|c| *c = 0);
            for dy in -r..=r {
                let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                for dx in -r..=r {
                    let xx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    if let Some(&l) = labels.get(yy * w + xx)
                        && let Some(c) = counts.get_mut(l as usize)
                    {
                        *c += 1;
                    }
                }
            }
            let mut best = 0usize;
            for (i, &c) in counts.iter().enumerate() {
                if c > counts[best] {
                    best = i;
                }
            }
            *o = best as u8;
        }
    });
    out
}

/// Connected components (4-neighbour). Returns (component id per pixel, sizes).
fn components(labels: &[u8], w: usize, h: usize) -> (Vec<u32>, Vec<usize>) {
    let mut comp = vec![u32::MAX; labels.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..labels.len() {
        if comp[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let lab = labels[start];
        comp[start] = id;
        stack.push(start);
        let mut size = 0usize;
        while let Some(i) = stack.pop() {
            size += 1;
            let x = i % w;
            let y = i / w;
            let mut push = |j: usize| {
                if comp[j] == u32::MAX && labels[j] == lab {
                    comp[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        sizes.push(size);
    }
    (comp, sizes)
}

/// Merge regions smaller than `min_area` into their most common neighbour.
fn merge_small(labels: &mut [u8], w: usize, h: usize, min_area: usize, k: usize) {
    for _ in 0..3 {
        let (comp, sizes) = components(labels, w, h);
        let mut votes: Vec<Vec<u32>> = vec![Vec::new(); sizes.len()];
        let mut any = false;
        for i in 0..labels.len() {
            let c = comp[i] as usize;
            if sizes[c] >= min_area {
                continue;
            }
            any = true;
            if votes[c].is_empty() {
                votes[c] = vec![0; k];
            }
            let x = i % w;
            let y = i / w;
            let mut vote = |j: usize| {
                if comp[j] as usize != c
                    && let Some(v) = votes[c].get_mut(labels[j] as usize)
                {
                    *v += 1;
                }
            };
            if x > 0 {
                vote(i - 1);
            }
            if x + 1 < w {
                vote(i + 1);
            }
            if y > 0 {
                vote(i - w);
            }
            if y + 1 < h {
                vote(i + w);
            }
        }
        if !any {
            break;
        }
        let new_label: Vec<Option<u8>> = votes
            .iter()
            .map(|v| {
                let (best, cnt) = v.iter().enumerate().max_by_key(|(_, c)| **c)?;
                if *cnt > 0 { Some(best as u8) } else { None }
            })
            .collect();
        for i in 0..labels.len() {
            if let Some(Some(l)) = new_label.get(comp[i] as usize) {
                labels[i] = *l;
            }
        }
    }
}

fn is_edge(labels: &[u8], w: usize, h: usize, i: usize) -> bool {
    let x = i % w;
    let y = i / w;
    let l = labels[i];
    (x + 1 < w && labels[i + 1] != l) || (y + 1 < h && labels[i + w] != l)
}

/// Chamfer distance to the nearest region boundary.
fn boundary_distance(labels: &[u8], w: usize, h: usize) -> Vec<f32> {
    let big = 1e9f32;
    let mut d: Vec<f32> = (0..labels.len())
        .map(|i| {
            let x = i % w;
            let y = i / w;
            let l = labels[i];
            let edge = (x > 0 && labels[i - 1] != l)
                || (x + 1 < w && labels[i + 1] != l)
                || (y > 0 && labels[i - w] != l)
                || (y + 1 < h && labels[i + w] != l)
                || x == 0
                || y == 0
                || x + 1 == w
                || y + 1 == h;
            if edge { 0.0 } else { big }
        })
        .collect();
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + a);
            }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 {
                    v = v.min(d[i - w - 1] + b);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + b);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + a);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + b);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + b);
                }
            }
            d[i] = v;
        }
    }
    d
}

fn paint_by_number(img: &RgbaImage, sw: &[Swatch], p: &Params) -> Rendered {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let k = sw.len().max(1);
    let mut labels = label_map(img, sw, 3);
    labels = majority(&labels, w, h, 3, k);
    labels = majority(&labels, w, h, 2, k);
    merge_small(&mut labels, w, h, p.pbn_min_area.max(4) as usize, k);
    let (comp, sizes) = components(&labels, w, h);
    let dist = boundary_distance(&labels, w, h);

    // Best label spot per component = point furthest from its boundary.
    let mut best: Vec<(f32, usize)> = vec![(-1.0, 0); sizes.len()];
    for i in 0..labels.len() {
        let c = comp[i] as usize;
        if dist[i] > best[c].0 {
            best[c] = (dist[i], i);
        }
    }
    let labels_out = best
        .iter()
        .enumerate()
        .filter(|(c, (d, _))| *d >= 3.0 && sizes[*c] >= 30)
        .map(|(_, (d, i))| NumberLabel {
            x: (i % w) as f32 + 0.5,
            y: (i / w) as f32 + 0.5,
            number: labels[*i] as usize + 1,
            room: *d,
        })
        .collect();

    let mut out = RgbaImage::new(w as u32, h as u32);
    out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
        let c = if is_edge(&labels, w, h, i) {
            [70, 70, 74]
        } else if p.pbn_fill {
            sw.get(labels[i] as usize).map(|s| s.rgb).unwrap_or([255; 3])
        } else {
            [250, 249, 246]
        };
        px.copy_from_slice(&[c[0], c[1], c[2], 255]);
    });
    Rendered { image: out, labels: labels_out }
}

fn line_art(img: &RgbaImage, p: &Params) -> RgbaImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let lum: Vec<f32> = img.pixels().map(|q| lightness([q[0], q[1], q[2]]) / 100.0).collect();
    let r1 = p.line_detail.max(1);
    let r2 = ((r1 as f32) * 1.6).ceil() as u32 + 1;
    let g1 = blur_plane(&lum, w, h, r1);
    let g2 = blur_plane(&lum, w, h, r2);
    let gain = p.line_weight.max(1.0);
    let mut out = RgbaImage::new(w as u32, h as u32);
    out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
        let d = g1[i] - g2[i];
        // Threshold out faint texture (canvas weave, brush ridges).
        let ink = ((-d * gain - 0.08) / 0.92).clamp(0.0, 1.0).powf(0.7);
        let v = (250.0 - ink * 225.0) as u8;
        px.copy_from_slice(&[v, v, v.saturating_sub(2), 255]);
    });
    out
}

fn mosaic(img: &RgbaImage, block: u32) -> RgbaImage {
    let b = block.max(2);
    let (w, h) = img.dimensions();
    let small_w = w.div_ceil(b).max(1);
    let small_h = h.div_ceil(b).max(1);
    let small = image::imageops::resize(img, small_w, small_h, image::imageops::FilterType::Triangle);
    image::imageops::resize(&small, w, h, image::imageops::FilterType::Nearest)
}

/// Render a study view of `work` (already work-sized).
pub fn render(work: &RgbaImage, mode: Mode, p: &Params, sw: &[Swatch], isolate: Option<usize>) -> Rendered {
    let (w, h) = (work.width() as usize, work.height() as usize);
    let mut r = match mode {
        Mode::Original => Rendered { image: work.clone(), labels: Vec::new() },
        Mode::Grayscale => Rendered { image: map_pixels(work, |c| grey_of(lightness(c))), labels: Vec::new() },
        Mode::Values => {
            // Light smoothing first so bands read as value *shapes*, not speckle.
            let src = blur_rgba(work, 2);
            Rendered {
                image: map_pixels(&src, |c| grey_of(quantize_l(lightness(c), p.value_steps.max(2)))),
                labels: Vec::new(),
            }
        }
        Mode::Notan => {
            let t = p.notan_threshold;
            let src = blur_rgba(work, 2);
            Rendered {
                image: map_pixels(&src, move |c| if lightness(c) < t { [38, 37, 40] } else { [236, 233, 226] }),
                labels: Vec::new(),
            }
        }
        Mode::Grisaille => {
            let lut = grisaille_lut(p.grisaille_temp);
            let steps = p.grisaille_steps;
            Rendered {
                image: map_pixels(work, move |c| {
                    let l = quantize_l(lightness(c), steps);
                    lut.get(l.round().clamp(0.0, 100.0) as usize).copied().unwrap_or(c)
                }),
                labels: Vec::new(),
            }
        }
        Mode::Simplify => {
            let mut labels = label_map(work, sw, p.simplify_smooth);
            labels = majority(&labels, w, h, 1 + p.simplify_smooth as isize / 2, sw.len().max(1));
            let mut out = work.clone();
            out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
                let c = sw.get(labels[i] as usize).map(|s| s.rgb).unwrap_or([0; 3]);
                px.copy_from_slice(&[c[0], c[1], c[2], 255]);
            });
            Rendered { image: out, labels: Vec::new() }
        }
        Mode::Mosaic => Rendered { image: mosaic(work, p.mosaic_block), labels: Vec::new() },
        Mode::PaintByNumber => paint_by_number(work, sw, p),
        Mode::LineArt => Rendered { image: line_art(work, p), labels: Vec::new() },
    };

    if let Some(idx) = isolate
        && mode != Mode::PaintByNumber
    {
        let labels = label_map(work, sw, 1);
        r.image.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
            if labels.get(i).copied().map(usize::from) != Some(idx) {
                let l = lightness([px[0], px[1], px[2]]);
                let g = to_srgb(lstar_to_y(l * 0.35 + 30.0));
                px[0] = g;
                px[1] = g;
                px[2] = g;
            }
        });
    }

    if p.squint > 0 {
        r.image = blur_rgba(&r.image, p.squint);
    }
    r
}

/// Encode a work-sized render back at the original's resolution for export.
pub fn upscale_to(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    image::imageops::resize(img, w, h, image::imageops::FilterType::Nearest)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract;
    use image::Rgba;

    fn test_img() -> RgbaImage {
        RgbaImage::from_fn(120, 80, |x, y| {
            if (x as i32 - 60).pow(2) + (y as i32 - 40).pow(2) < 600 {
                Rgba([220, 60, 40, 255])
            } else if y < 30 {
                Rgba([140, 180, 220, 255])
            } else {
                Rgba([70, 110, 50, 255])
            }
        })
    }

    #[test]
    fn every_mode_renders_same_size() {
        let img = test_img();
        let sw = extract(&img, 3);
        let p = Params::default();
        for m in Mode::ALL {
            let r = render(&img, m, &p, &sw, None);
            assert_eq!(r.image.dimensions(), img.dimensions(), "{m:?}");
        }
    }

    #[test]
    fn pbn_has_one_label_per_region() {
        let img = test_img();
        let sw = extract(&img, 3);
        let p = Params { pbn_min_area: 50, ..Params::default() };
        let r = render(&img, Mode::PaintByNumber, &p, &sw, None);
        assert_eq!(r.labels.len(), 3, "{:?}", r.labels);
    }

    #[test]
    fn quantize_bounds() {
        assert!((quantize_l(0.0, 5) - 4.0).abs() < 1e-3);
        assert!((quantize_l(100.0, 5) - 96.0).abs() < 1e-3);
    }

    #[test]
    fn blur_preserves_flat() {
        let v = vec![0.5f32; 50 * 20];
        let b = blur_plane(&v, 50, 20, 3);
        assert!(b.iter().all(|x| (x - 0.5).abs() < 1e-4));
    }

    #[test]
    fn tiny_images_do_not_crash() {
        let img = RgbaImage::new(1, 1);
        let sw = extract(&img, 4);
        for m in Mode::ALL {
            let _ = render(&img, m, &Params::default(), &sw, Some(0));
        }
    }
}
