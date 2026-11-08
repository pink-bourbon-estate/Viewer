//! Geometry edits: quarter turns, flips, straighten (any angle) and crop.
//! The crop rectangle is normalised (0..1) in the rotated/flipped frame.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    /// Clockwise quarter turns, 0..=3.
    pub quarter: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Straighten angle in degrees, positive = clockwise.
    pub angle: f32,
    /// x0, y0, x1, y1 in 0..1.
    pub crop: [f32; 4],
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry { quarter: 0, flip_h: false, flip_v: false, angle: 0.0, crop: [0.0, 0.0, 1.0, 1.0] }
    }
}

impl Geometry {
    pub fn is_identity(&self) -> bool {
        *self == Geometry::default()
    }

    pub fn has_crop(&self) -> bool {
        self.crop != [0.0, 0.0, 1.0, 1.0]
    }

    /// Size after quarter turns.
    pub fn turned(&self, w: f32, h: f32) -> (f32, f32) {
        if self.quarter % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// The largest centred box (fraction of each side) that stays inside the
    /// image after straightening — crops are kept within it.
    pub fn safe_scale(&self, w: f32, h: f32) -> f32 {
        let a = self.angle.to_radians().abs();
        if a < 1e-4 {
            return 1.0;
        }
        let (c, s) = (a.cos(), a.sin());
        (w / (w * c + h * s)).min(h / (w * s + h * c)).min(1.0)
    }

    /// Clamp the crop inside the straighten-safe area.
    pub fn constrain(&mut self, w: f32, h: f32) {
        let (tw, th) = self.turned(w, h);
        let s = self.safe_scale(tw, th);
        let (lo, hi) = (0.5 - s / 2.0, 0.5 + s / 2.0);
        let c = &mut self.crop;
        if c[0] > c[2] {
            c.swap(0, 2);
        }
        if c[1] > c[3] {
            c.swap(1, 3);
        }
        for i in 0..4 {
            c[i] = c[i].clamp(lo, hi);
        }
        if c[2] - c[0] < 0.02 {
            c[2] = (c[0] + 0.02).min(hi);
        }
        if c[3] - c[1] < 0.02 {
            c[3] = (c[1] + 0.02).min(hi);
        }
    }
}

fn bilinear(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if w == 0 || h == 0 {
        return Rgba([0, 0, 0, 0]);
    }
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let get = |xx: i64, yy: i64| {
        let p = img.get_pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32);
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut o = [0u8; 4];
    for k in 0..4 {
        let v = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
        o[k] = v.round().clamp(0.0, 255.0) as u8;
    }
    Rgba(o)
}

/// Apply turns, flips, straighten and crop.
pub fn apply(img: &RgbaImage, g: &Geometry) -> RgbaImage {
    use image::imageops;
    let mut im = match g.quarter % 4 {
        1 => imageops::rotate90(img),
        2 => imageops::rotate180(img),
        3 => imageops::rotate270(img),
        _ => img.clone(),
    };
    if g.flip_h {
        im = imageops::flip_horizontal(&im);
    }
    if g.flip_v {
        im = imageops::flip_vertical(&im);
    }
    let (w, h) = im.dimensions();
    let (wf, hf) = (w as f32, h as f32);
    let [x0, y0, x1, y1] = g.crop;
    let ox = (x0 * wf).floor().max(0.0);
    let oy = (y0 * hf).floor().max(0.0);
    let cw = (((x1 - x0) * wf).round() as u32).clamp(1, w.max(1));
    let ch = (((y1 - y0) * hf).round() as u32).clamp(1, h.max(1));
    if g.angle.abs() < 1e-3 {
        return imageops::crop_imm(&im, ox as u32, oy as u32, cw.min(w - ox as u32), ch.min(h - oy as u32)).to_image();
    }
    // Rotate about the image centre, sampling only the cropped area.
    let (sn, cs) = (-g.angle.to_radians()).sin_cos();
    let (cx, cy) = (wf / 2.0, hf / 2.0);
    let mut out = RgbaImage::new(cw, ch);
    let row = cw as usize * 4;
    out.as_mut().par_chunks_mut(row.max(4)).enumerate().for_each(|(y, line)| {
        for x in 0..cw as usize {
            let px = ox + x as f32 + 0.5 - cx;
            let py = oy + y as f32 + 0.5 - cy;
            let sx = px * cs - py * sn + cx - 0.5;
            let sy = px * sn + py * cs + cy - 0.5;
            let p = bilinear(&im, sx, sy);
            line[x * 4..x * 4 + 4].copy_from_slice(&p.0);
        }
    });
    out
}

/// Angle (degrees) that levels a line drawn from a to b.
pub fn level_angle(a: (f32, f32), b: (f32, f32)) -> f32 {
    let mut ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
    // Treat near-vertical lines as verticals.
    if ang > 45.0 {
        ang -= 90.0;
    } else if ang < -45.0 {
        ang += 90.0;
    }
    (-ang).clamp(-45.0, 45.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_and_turn() {
        let img = RgbaImage::from_pixel(100, 50, Rgba([1, 2, 3, 255]));
        let g = Geometry { quarter: 1, crop: [0.0, 0.0, 0.5, 1.0], ..Geometry::default() };
        let o = apply(&img, &g);
        assert_eq!(o.dimensions(), (25, 100));
    }

    #[test]
    fn straighten_keeps_crop_size() {
        let img = RgbaImage::from_pixel(80, 60, Rgba([9, 9, 9, 255]));
        let mut g = Geometry { angle: 10.0, ..Geometry::default() };
        g.constrain(80.0, 60.0);
        let o = apply(&img, &g);
        assert!(o.width() < 80 && o.height() < 60);
        // Everything inside the safe crop is real image, not border.
        assert!(o.pixels().all(|p| p[3] == 255));
    }

    #[test]
    fn level() {
        assert!((level_angle((0.0, 0.0), (10.0, 1.0)) + 5.71).abs() < 0.1);
        assert!(level_angle((0.0, 0.0), (1.0, 10.0)).abs() < 6.0);
    }
}
