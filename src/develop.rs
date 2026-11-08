//! Develop engine: Lightroom-style global adjustments, applied through a
//! 33³ colour lookup table (so full-resolution export is fast), plus the few
//! spatial effects (clarity, vignette, grain) applied per pixel.
//!
//! Also parses Lightroom presets (.xmp, .lrtemplate) and .cube LUTs into
//! [`Develop`] so a preset library can be imported and condensed.

use crate::color::{linear_to_oklab, oklab_to_linear, to_linear};
use image::RgbaImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
const BAND_HUES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

/// A 3D LUT (from a .cube), stored at 17³, values 0..1, red fastest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lut {
    pub size: usize,
    pub data: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Develop {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub temp: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub fade: f32,
    /// Point curves, x/y in 0..255 (Lightroom convention).
    pub curve: Vec<[f32; 2]>,
    pub curve_r: Vec<[f32; 2]>,
    pub curve_g: Vec<[f32; 2]>,
    pub curve_b: Vec<[f32; 2]>,
    /// Parametric curve: shadows, darks, lights, highlights (-100..100).
    pub param: [f32; 4],
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
    pub split_sh_hue: f32,
    pub split_sh_sat: f32,
    pub split_mid_hue: f32,
    pub split_mid_sat: f32,
    pub split_hi_hue: f32,
    pub split_hi_sat: f32,
    pub split_balance: f32,
    pub grayscale: bool,
    pub gray_mix: [f32; 8],
    pub vignette: f32,
    pub grain: f32,
    pub grain_size: f32,
    /// Camera calibration: red/green/blue primary hue & saturation, shadow tint.
    pub cal_hue: [f32; 3],
    pub cal_sat: [f32; 3],
    pub shadow_tint: f32,
    pub lut: Option<Lut>,
    /// Overall strength of the look, 0..1.
    pub amount: f32,
}

impl Default for Develop {
    fn default() -> Self {
        Develop {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temp: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            fade: 0.0,
            curve: Vec::new(),
            curve_r: Vec::new(),
            curve_g: Vec::new(),
            curve_b: Vec::new(),
            param: [0.0; 4],
            hue: [0.0; 8],
            sat: [0.0; 8],
            lum: [0.0; 8],
            split_sh_hue: 0.0,
            split_sh_sat: 0.0,
            split_mid_hue: 0.0,
            split_mid_sat: 0.0,
            split_hi_hue: 0.0,
            split_hi_sat: 0.0,
            split_balance: 0.0,
            grayscale: false,
            gray_mix: [0.0; 8],
            vignette: 0.0,
            grain: 0.0,
            grain_size: 25.0,
            cal_hue: [0.0; 3],
            cal_sat: [0.0; 3],
            shadow_tint: 0.0,
            lut: None,
            amount: 1.0,
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn curve_is_identity(c: &[[f32; 2]]) -> bool {
    c.len() < 2 || c.iter().all(|p| (p[0] - p[1]).abs() < 0.5)
}

/// Monotone cubic (Fritsch–Carlson) curve sampled to 256 entries (0..1).
pub fn curve_lut(points: &[[f32; 2]]) -> Option<[f32; 256]> {
    if curve_is_identity(points) {
        return None;
    }
    let mut p: Vec<[f32; 2]> = points.iter().map(|q| [q[0].clamp(0.0, 255.0), q[1].clamp(0.0, 255.0)]).collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]));
    p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-3);
    let n = p.len();
    let mut out = [0.0f32; 256];
    if n < 2 {
        return None;
    }
    let mut d = vec![0.0f32; n - 1];
    for i in 0..n - 1 {
        d[i] = (p[i + 1][1] - p[i][1]) / (p[i + 1][0] - p[i][0]).max(1e-3);
    }
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..n - 1 {
        if d[i].abs() < 1e-6 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let a = m[i] / d[i];
            let b = m[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    for (x, o) in out.iter_mut().enumerate() {
        let xf = x as f32;
        let y = if xf <= p[0][0] {
            p[0][1]
        } else if xf >= p[n - 1][0] {
            p[n - 1][1]
        } else {
            let mut i = 0;
            while i + 1 < n && p[i + 1][0] < xf {
                i += 1;
            }
            let h = (p[i + 1][0] - p[i][0]).max(1e-3);
            let t = (xf - p[i][0]) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * p[i][1]
                + (t3 - 2.0 * t2 + t) * h * m[i]
                + (-2.0 * t3 + 3.0 * t2) * p[i + 1][1]
                + (t3 - t2) * h * m[i + 1]
        };
        *o = (y / 255.0).clamp(0.0, 1.0);
    }
    Some(out)
}

fn sample_curve(lut: &Option<[f32; 256]>, v: f32) -> f32 {
    match lut {
        None => v,
        Some(l) => {
            let x = v.clamp(0.0, 1.0) * 255.0;
            let i = (x as usize).min(254);
            let f = x - i as f32;
            l[i] * (1.0 - f) + l[i + 1] * f
        }
    }
}

fn hsv_hue(c: [f32; 3]) -> f32 {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d <= 1e-6 {
        return 0.0;
    }
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    h.rem_euclid(360.0)
}

/// Band weights (sum 1) for an HSV hue.
fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    for i in 0..8 {
        let a = BAND_HUES[i];
        let b = if i == 7 { 360.0 } else { BAND_HUES[i + 1] };
        if h >= a && h < b {
            let t = (h - a) / (b - a);
            w[i] = 1.0 - t;
            w[(i + 1) % 8] += t;
            return w;
        }
    }
    w[0] = 1.0;
    w
}

/// OKLab a/b direction for an HSV hue (how Lightroom-style hue pickers map).
fn hue_dir(h: f32) -> [f32; 2] {
    let hh = (h.rem_euclid(360.0)) / 60.0;
    let x = 1.0 - (hh % 2.0 - 1.0).abs();
    let (r, g, b) = match hh as i32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let lab = linear_to_oklab([to_linear((r * 255.0) as u8), to_linear((g * 255.0) as u8), to_linear((b * 255.0) as u8)]);
    let n = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt().max(1e-6);
    [lab[1] / n, lab[2] / n]
}

impl Develop {
    pub fn is_identity(&self) -> bool {
        let d = Develop { amount: self.amount, grain_size: self.grain_size, ..Develop::default() };
        let mut me = self.clone();
        for c in [&mut me.curve, &mut me.curve_r, &mut me.curve_g, &mut me.curve_b] {
            if curve_is_identity(c) {
                c.clear();
            }
        }
        me == d || self.amount <= 0.001
    }

    fn has_spatial(&self) -> bool {
        self.clarity.abs() > 0.5 || self.vignette.abs() > 0.5 || self.grain > 0.5
    }
}

/// Precomputed pieces of a [`Develop`] for evaluating colours.
struct Prepared<'a> {
    d: &'a Develop,
    master: Option<[f32; 256]>,
    r: Option<[f32; 256]>,
    g: Option<[f32; 256]>,
    b: Option<[f32; 256]>,
    sh_dir: [f32; 2],
    mid_dir: [f32; 2],
    hi_dir: [f32; 2],
    cal: Option<[[f32; 3]; 3]>,
}

/// Calibration as a 3×3 matrix on linear RGB: each primary's hue is rotated
/// and its chroma scaled in OKLab, then rows are normalised so white stays white.
fn calibration_matrix(d: &Develop) -> Option<[[f32; 3]; 3]> {
    if d.cal_hue.iter().chain(&d.cal_sat).all(|v| v.abs() < 0.5) {
        return None;
    }
    let mut cols = [[0.0f32; 3]; 3];
    for i in 0..3 {
        let mut e = [0.0f32; 3];
        e[i] = 1.0;
        let mut lab = linear_to_oklab(e);
        let ang = (d.cal_hue[i] / 100.0 * 12.0).to_radians();
        let (sn, cs) = ang.sin_cos();
        let (a, b) = (lab[1], lab[2]);
        let k = 1.0 + d.cal_sat[i] / 100.0 * 0.3;
        lab[1] = (a * cs - b * sn) * k;
        lab[2] = (a * sn + b * cs) * k;
        let v = oklab_to_linear(lab);
        // Keep each primary's luminance so calibration shifts colour, not exposure.
        let y0 = [0.2126, 0.7152, 0.0722][i];
        let y1 = (0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]).max(1e-4);
        cols[i] = [v[0] * y0 / y1, v[1] * y0 / y1, v[2] * y0 / y1];
    }
    let mut m = [[0.0f32; 3]; 3];
    for r in 0..3 {
        let sum: f32 = (0..3).map(|c| cols[c][r]).sum::<f32>().max(1e-4);
        let white = [0.2126f32, 0.7152, 0.0722].iter().sum::<f32>();
        let _ = white;
        for c in 0..3 {
            m[r][c] = cols[c][r] / sum;
        }
    }
    Some(m)
}

impl<'a> Prepared<'a> {
    fn new(d: &'a Develop) -> Self {
        Prepared {
            d,
            master: curve_lut(&d.curve),
            r: curve_lut(&d.curve_r),
            g: curve_lut(&d.curve_g),
            b: curve_lut(&d.curve_b),
            sh_dir: hue_dir(d.split_sh_hue),
            mid_dir: hue_dir(d.split_mid_hue),
            hi_dir: hue_dir(d.split_hi_hue),
            cal: calibration_matrix(d),
        }
    }

    /// sRGB (0..1) in -> sRGB (0..1) out, global adjustments only.
    fn eval(&self, c: [f32; 3]) -> [f32; 3] {
        let d = self.d;
        let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let enc = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        };
        let mut l = [lin(c[0]), lin(c[1]), lin(c[2])];

        // Optional imported LUT first (it was authored on display-referred input).
        if let Some(lut) = &d.lut {
            let o = apply_lut(lut, c);
            l = [lin(o[0]), lin(o[1]), lin(o[2])];
        }

        if let Some(m) = &self.cal {
            let o = l;
            for r in 0..3 {
                l[r] = (m[r][0] * o[0] + m[r][1] * o[1] + m[r][2] * o[2]).max(0.0);
            }
        }

        // White balance.
        let t = d.temp / 100.0;
        let ti = d.tint / 100.0;
        l[0] *= (0.32 * t).exp2();
        l[2] *= (-0.32 * t).exp2();
        l[1] *= (-0.22 * ti).exp2();

        // Exposure.
        let ev = (d.exposure).exp2();
        l.iter_mut().for_each(|v| *v *= ev);

        let contrast = d.contrast + d.dehaze * 0.35;
        let blacks = d.blacks - d.dehaze * 0.25;

        // Tone on luminance, ratio-preserving.
        let y = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
        let mut p = y.max(0.0).powf(1.0 / 2.2);
        p += (d.highlights / 100.0) * 0.22 * smoothstep(0.45, 1.05, p) * (1.15 - p).max(0.0);
        p += (d.shadows / 100.0) * 0.24 * (1.0 - smoothstep(0.0, 0.55, p)) * p.min(0.25).max(0.04) * 4.0;
        p *= 1.0 + (d.whites / 100.0) * 0.16 * smoothstep(0.55, 1.0, p);
        p += (blacks / 100.0) * 0.09 * (1.0 - smoothstep(0.0, 0.32, p));
        let k = contrast / 100.0;
        if k >= 0.0 {
            let pc = p.clamp(0.0, 1.0);
            let s = pc * pc * (3.0 - 2.0 * pc);
            p += k * 0.9 * (s - pc);
        } else {
            p = 0.5 + (p - 0.5) * (1.0 + k * 0.6);
        }
        const CENTRES: [f32; 4] = [0.125, 0.375, 0.625, 0.875];
        for (a, cen) in d.param.iter().zip(CENTRES) {
            if a.abs() > 0.1 {
                let x = ((p - cen) / 0.2).clamp(-1.0, 1.0);
                let bump = (1.0 - x * x).powi(2);
                p += a / 100.0 * 0.16 * bump;
            }
        }
        let f = (d.fade / 100.0) * 0.22;
        p = f + p.max(0.0) * (1.0 - f);
        let y2 = p.max(0.0).powf(2.2);
        if y > 1e-5 {
            let s = y2 / y;
            l.iter_mut().for_each(|v| *v *= s);
        } else {
            l = [y2; 3];
        }
        // Roll off over-range highlights instead of clipping hue.
        let mx = l[0].max(l[1]).max(l[2]);
        if mx > 1.0 {
            let yy = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
            let t = ((mx - 1.0) / mx).clamp(0.0, 1.0);
            l.iter_mut().for_each(|v| *v = (*v * (1.0 - t) + yy.min(1.0) * t).min(1.0));
        }

        // Point curves in display space.
        let mut s = [enc(l[0]), enc(l[1]), enc(l[2])];
        if self.master.is_some() {
            s = [sample_curve(&self.master, s[0]), sample_curve(&self.master, s[1]), sample_curve(&self.master, s[2])];
        }
        s = [sample_curve(&self.r, s[0]), sample_curve(&self.g, s[1]), sample_curve(&self.b, s[2])];

        // Colour work in OKLab.
        let hue = hsv_hue(s);
        let w = band_weights(hue);
        let mut lab = linear_to_oklab([lin(s[0]), lin(s[1]), lin(s[2])]);
        let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
        let colourful = (chroma / 0.09).min(1.0);

        if d.grayscale {
            let mut dl = 0.0;
            for i in 0..8 {
                dl += w[i] * d.gray_mix[i] / 100.0;
            }
            lab[0] = (lab[0] + dl * 0.18 * colourful).clamp(0.0, 1.0);
            lab[1] = 0.0;
            lab[2] = 0.0;
        } else {
            let (mut dh, mut ds, mut dl) = (0.0f32, 0.0f32, 0.0f32);
            for i in 0..8 {
                dh += w[i] * d.hue[i];
                ds += w[i] * d.sat[i];
                dl += w[i] * d.lum[i];
            }
            let ang = (dh / 100.0 * 30.0).to_radians() * colourful;
            if ang.abs() > 1e-4 {
                let (sn, cs) = ang.sin_cos();
                let (a, b) = (lab[1], lab[2]);
                lab[1] = a * cs - b * sn;
                lab[2] = a * sn + b * cs;
            }
            lab[0] = (lab[0] + dl / 100.0 * 0.12 * colourful).clamp(0.0, 1.0);
            let vib = 1.0 + d.vibrance / 100.0 * 0.9 * (1.0 - (chroma / 0.2).min(1.0));
            let sat = (1.0 + ds / 100.0) * (1.0 + (d.saturation + d.dehaze * 0.2) / 100.0) * vib;
            lab[1] *= sat.max(0.0);
            lab[2] *= sat.max(0.0);
        }

        // Split toning / colour grading.
        let lval = lab[0];
        let bal = d.split_balance / 100.0 * 0.25;
        let wsh = 1.0 - smoothstep(0.0, 0.6 + bal, lval);
        let whi = smoothstep(0.4 + bal, 1.0, lval);
        let wmid = (1.0 - (lval - 0.5).abs() * 2.0).max(0.0);
        for (wgt, sat, dir) in [
            (wsh, d.split_sh_sat, self.sh_dir),
            (whi, d.split_hi_sat, self.hi_dir),
            (wmid, d.split_mid_sat, self.mid_dir),
        ] {
            if sat > 0.1 {
                let m = wgt * sat / 100.0 * 0.07;
                lab[1] += dir[0] * m;
                lab[2] += dir[1] * m;
            }
        }

        if d.shadow_tint.abs() > 0.1 {
            // Positive = magenta, negative = green, in the shadows only.
            let m = wsh * d.shadow_tint / 100.0 * 0.05;
            lab[1] += m;
            lab[2] -= m * 0.3;
        }

        let o = oklab_to_linear(lab);
        [enc(o[0]), enc(o[1]), enc(o[2])]
    }
}

pub fn apply_lut(lut: &Lut, c: [f32; 3]) -> [f32; 3] {
    let n = lut.size;
    if n < 2 || lut.data.len() < n * n * n * 3 {
        return c;
    }
    let s = (n - 1) as f32;
    let fx = c[0].clamp(0.0, 1.0) * s;
    let fy = c[1].clamp(0.0, 1.0) * s;
    let fz = c[2].clamp(0.0, 1.0) * s;
    let (x0, y0, z0) = ((fx as usize).min(n - 2), (fy as usize).min(n - 2), (fz as usize).min(n - 2));
    let (dx, dy, dz) = (fx - x0 as f32, fy - y0 as f32, fz - z0 as f32);
    let at = |x: usize, y: usize, z: usize, k: usize| lut.data[((z * n + y) * n + x) * 3 + k];
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let c00 = at(x0, y0, z0, k) * (1.0 - dx) + at(x0 + 1, y0, z0, k) * dx;
        let c10 = at(x0, y0 + 1, z0, k) * (1.0 - dx) + at(x0 + 1, y0 + 1, z0, k) * dx;
        let c01 = at(x0, y0, z0 + 1, k) * (1.0 - dx) + at(x0 + 1, y0, z0 + 1, k) * dx;
        let c11 = at(x0, y0 + 1, z0 + 1, k) * (1.0 - dx) + at(x0 + 1, y0 + 1, z0 + 1, k) * dx;
        let c0 = c00 * (1.0 - dy) + c10 * dy;
        let c1 = c01 * (1.0 - dy) + c11 * dy;
        *o = c0 * (1.0 - dz) + c1 * dz;
    }
    out
}

/// The global part of a develop baked into a 33³ LUT.
pub fn bake(d: &Develop) -> Lut {
    let n = 33;
    let p = Prepared::new(d);
    let mut data = vec![0.0f32; n * n * n * 3];
    data.par_chunks_mut(n * n * 3).enumerate().for_each(|(z, plane)| {
        for y in 0..n {
            for x in 0..n {
                let c = [x as f32 / 32.0, y as f32 / 32.0, z as f32 / 32.0];
                let o = p.eval(c);
                let i = (y * n + x) * 3;
                plane[i..i + 3].copy_from_slice(&o);
            }
        }
    });
    Lut { size: n, data }
}

/// Evaluate a develop on one sRGB colour (used for preset clustering).
pub fn eval_color(d: &Develop, c: [f32; 3]) -> [f32; 3] {
    Prepared::new(d).eval(c)
}

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Apply a develop to an image. `scale` is the image's size relative to the
/// original (so grain and clarity look the same in preview and export).
pub fn apply(img: &RgbaImage, d: &Develop, scale: f32) -> RgbaImage {
    if d.is_identity() {
        return img.clone();
    }
    let lut = bake(d);
    let amount = d.amount.clamp(0.0, 1.0);
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    out.as_mut().par_chunks_mut(4).for_each(|px| {
        let c = [px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0];
        let o = apply_lut(&lut, c);
        for k in 0..3 {
            let v = c[k] + (o[k] - c[k]) * amount;
            px[k] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    });
    if !d.has_spatial() {
        return out;
    }

    // Clarity: local contrast on luminance.
    if d.clarity.abs() > 0.5 && w > 2 && h > 2 {
        let lum: Vec<f32> = out.pixels().map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).collect();
        let r = ((w.max(h) as f32 / 90.0).max(2.0)) as u32;
        let blur = crate::filters::blur_plane(&lum, w as usize, h as usize, r);
        let k = d.clarity / 100.0 * 0.9 * amount;
        out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
            let l = lum[i];
            let detail = (l - blur[i]) * k * (1.0 - (2.0 * l - 1.0).abs()).max(0.2);
            for c in px.iter_mut().take(3) {
                *c = (*c as f32 + detail * 255.0).clamp(0.0, 255.0) as u8;
            }
        });
    }

    let vig = d.vignette / 100.0 * amount;
    // Downscaled previews average grain away; match what full size will show.
    let grain = d.grain / 100.0 * amount * scale.clamp(0.25, 1.0).sqrt();
    let gsize = (d.grain_size.max(1.0) / 25.0 * 1.6 * scale).max(0.6);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let wf = w as usize;
    out.as_mut().par_chunks_mut(4).enumerate().for_each(|(i, px)| {
        let x = (i % wf) as f32;
        let y = (i / wf) as f32;
        let mut mul = 1.0f32;
        if vig.abs() > 0.005 {
            let dx = (x - cx) / cx.max(1.0);
            let dy = (y - cy) / cy.max(1.0);
            let dist = ((dx * dx + dy * dy) / 2.0).sqrt();
            let m = smoothstep(0.3, 1.0, dist);
            mul = if vig < 0.0 { 1.0 + vig * 0.85 * m } else { 1.0 + vig * 0.6 * m };
        }
        let mut add = 0.0f32;
        if grain > 0.005 {
            let gx = x / gsize;
            let gy = y / gsize;
            let (ix, iy) = (gx as u32, gy as u32);
            let (fx, fy) = (gx.fract(), gy.fract());
            let n00 = hash(ix, iy, 1);
            let n10 = hash(ix + 1, iy, 1);
            let n01 = hash(ix, iy + 1, 1);
            let n11 = hash(ix + 1, iy + 1, 1);
            let n = (n00 * (1.0 - fx) + n10 * fx) * (1.0 - fy) + (n01 * (1.0 - fx) + n11 * fx) * fy;
            let l = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
            add = (n - 0.5) * grain * 0.32 * (4.0 * l * (1.0 - l) + 0.15) * 255.0;
        }
        for c in px.iter_mut().take(3) {
            let v = if vig > 0.0 { 255.0 - (255.0 - *c as f32) * (2.0 - mul) } else { *c as f32 * mul };
            *c = (v + add).clamp(0.0, 255.0) as u8;
        }
    });
    out
}

// --------------------------------------------------------------------------
// Preset parsing
// --------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Val {
    Num(f32),
    Bool(bool),
    Str(String),
    List(Vec<f32>),
}

fn num(m: &HashMap<String, Val>, k: &str) -> Option<f32> {
    match m.get(k)? {
        Val::Num(n) => Some(*n),
        Val::Str(s) => s.trim().trim_start_matches('+').parse().ok(),
        Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Val::List(_) => None,
    }
}

fn pts(m: &HashMap<String, Val>, k: &str) -> Vec<[f32; 2]> {
    match m.get(k) {
        Some(Val::List(v)) => v.chunks_exact(2).map(|c| [c[0], c[1]]).collect(),
        _ => Vec::new(),
    }
}

/// Keys that make a preset a "look" (vs. sharpening / noise / lens / brushes).
const LOOK_KEYS: &[&str] = &[
    "Exposure2012", "Contrast2012", "Highlights2012", "Shadows2012", "Whites2012", "Blacks2012", "Temperature",
    "IncrementalTemperature", "Tint", "IncrementalTint", "Vibrance", "Saturation", "Clarity2012", "Dehaze",
    "ParametricShadows", "ParametricDarks", "ParametricLights", "ParametricHighlights", "SplitToningShadowSaturation",
    "SplitToningHighlightSaturation", "ColorGradeShadowSat", "ColorGradeHighlightSat", "ColorGradeMidtoneSat",
    "GrainAmount", "PostCropVignetteAmount", "Exposure", "Contrast", "Brightness", "FillLight", "RedHue",
    "GreenHue", "BlueHue", "RedSaturation", "GreenSaturation", "BlueSaturation",
];

fn to_develop(m: &HashMap<String, Val>) -> Option<Develop> {
    let mut look = LOOK_KEYS.iter().any(|k| num(m, k).is_some_and(|v| v.abs() > 0.01));
    for k in ["ToneCurvePV2012", "ToneCurvePV2012Red", "ToneCurvePV2012Green", "ToneCurvePV2012Blue", "ToneCurve"] {
        if !curve_is_identity(&pts(m, k)) {
            look = true;
        }
    }
    let gray = matches!(m.get("ConvertToGrayscale"), Some(Val::Bool(true)))
        || matches!(m.get("ConvertToGrayscale"), Some(Val::Str(s)) if s.eq_ignore_ascii_case("true"));
    for b in BANDS {
        for p in ["HueAdjustment", "SaturationAdjustment", "LuminanceAdjustment"] {
            if num(m, &format!("{p}{b}")).is_some_and(|v| v.abs() > 0.01) {
                look = true;
            }
        }
    }
    if !(look || gray) {
        return None;
    }
    let g = |k: &str| num(m, k).unwrap_or(0.0);
    let mut d = Develop::default();
    let pv2012 = m.contains_key("Exposure2012") || m.contains_key("Contrast2012") || m.contains_key("ToneCurvePV2012");
    if pv2012 {
        d.exposure = g("Exposure2012");
        d.contrast = g("Contrast2012");
        d.highlights = g("Highlights2012");
        d.shadows = g("Shadows2012");
        d.whites = g("Whites2012");
        d.blacks = g("Blacks2012");
        d.clarity = g("Clarity2012");
        d.curve = pts(m, "ToneCurvePV2012");
        d.curve_r = pts(m, "ToneCurvePV2012Red");
        d.curve_g = pts(m, "ToneCurvePV2012Green");
        d.curve_b = pts(m, "ToneCurvePV2012Blue");
    } else {
        // Process 2010 and earlier: approximate.
        d.exposure = g("Exposure") + g("Brightness") / 100.0;
        d.contrast = num(m, "Contrast").map(|c| (c - 25.0) * 0.8).unwrap_or(0.0);
        d.shadows = g("FillLight") * 0.8;
        d.blacks = num(m, "Shadows").map(|s| (5.0 - s) * 3.0).unwrap_or(0.0);
        d.highlights = -g("HighlightRecovery") * 0.8;
        d.clarity = g("Clarity");
        d.curve = pts(m, "ToneCurve");
    }
    if let Some(t) = num(m, "IncrementalTemperature") {
        d.temp = t;
    } else if let Some(k) = num(m, "Temperature").filter(|k| *k > 1500.0) {
        d.temp = (150.0 * (k / 5500.0).ln()).clamp(-100.0, 100.0);
    }
    if let Some(t) = num(m, "IncrementalTint") {
        d.tint = t;
    } else if let Some(t) = num(m, "Tint") {
        d.tint = (t / 1.5).clamp(-100.0, 100.0);
    }
    d.vibrance = g("Vibrance");
    d.saturation = g("Saturation");
    d.dehaze = g("Dehaze");
    d.param = [g("ParametricShadows"), g("ParametricDarks"), g("ParametricLights"), g("ParametricHighlights")];
    for (i, b) in BANDS.iter().enumerate() {
        d.hue[i] = g(&format!("HueAdjustment{b}"));
        d.sat[i] = g(&format!("SaturationAdjustment{b}"));
        d.lum[i] = g(&format!("LuminanceAdjustment{b}"));
        d.gray_mix[i] = g(&format!("GrayMixer{b}"));
    }
    d.grayscale = gray;
    if num(m, "ColorGradeShadowSat").is_some() || num(m, "ColorGradeHighlightSat").is_some() {
        d.split_sh_hue = g("ColorGradeShadowHue");
        d.split_sh_sat = g("ColorGradeShadowSat");
        d.split_hi_hue = g("ColorGradeHighlightHue");
        d.split_hi_sat = g("ColorGradeHighlightSat");
        d.split_mid_hue = g("ColorGradeMidtoneHue");
        d.split_mid_sat = g("ColorGradeMidtoneSat");
        d.split_balance = g("SplitToningBalance");
    } else {
        d.split_sh_hue = g("SplitToningShadowHue");
        d.split_sh_sat = g("SplitToningShadowSaturation");
        d.split_hi_hue = g("SplitToningHighlightHue");
        d.split_hi_sat = g("SplitToningHighlightSaturation");
        d.split_balance = g("SplitToningBalance");
    }
    d.cal_hue = [g("RedHue"), g("GreenHue"), g("BlueHue")];
    d.cal_sat = [g("RedSaturation"), g("GreenSaturation"), g("BlueSaturation")];
    d.shadow_tint = g("ShadowTint");
    d.vignette = g("PostCropVignetteAmount");
    d.grain = g("GrainAmount");
    d.grain_size = num(m, "GrainSize").unwrap_or(25.0);
    Some(d)
}

/// Parse Adobe XMP develop settings (crs:* attributes, elements and curve sequences).
pub fn parse_xmp(text: &str) -> Option<Develop> {
    let mut m: HashMap<String, Val> = HashMap::new();
    // Attribute form: crs:Key="value"
    let mut rest = text;
    while let Some(i) = rest.find("crs:") {
        rest = &rest[i + 4..];
        let key: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let after = &rest[key.len()..];
        if let Some(v) = after.strip_prefix("=\"") {
            if let Some(end) = v.find('"') {
                let raw = &v[..end];
                let val = match raw {
                    "True" | "true" => Val::Bool(true),
                    "False" | "false" => Val::Bool(false),
                    _ => raw.trim_start_matches('+').parse::<f32>().map(Val::Num).unwrap_or(Val::Str(raw.into())),
                };
                m.entry(key).or_insert(val);
            }
        } else if let Some(v) = after.strip_prefix('>') {
            // Element form: <crs:Key>value</crs:Key> or a curve sequence.
            let close = format!("</crs:{key}>");
            if let Some(end) = v.find(&close) {
                let inner = &v[..end];
                if inner.contains("<rdf:li>") {
                    let mut nums = Vec::new();
                    for li in inner.split("<rdf:li>").skip(1) {
                        let item = li.split("</rdf:li>").next().unwrap_or("");
                        for part in item.split(',') {
                            if let Ok(n) = part.trim().parse::<f32>() {
                                nums.push(n);
                            }
                        }
                    }
                    if !nums.is_empty() {
                        m.insert(key, Val::List(nums));
                    }
                } else if let Ok(n) = inner.trim().trim_start_matches('+').parse::<f32>() {
                    m.entry(key).or_insert(Val::Num(n));
                } else if !inner.contains('<') {
                    m.entry(key).or_insert(Val::Str(inner.trim().into()));
                }
            }
        }
    }
    // Camera profiles carry binary LUT tables and no look settings.
    to_develop(&m)
}

/// Parse a Lightroom .lrtemplate (Lua table) develop preset.
pub fn parse_lrtemplate(text: &str) -> Option<Develop> {
    let start = text.find("settings")?;
    let body = &text[start..];
    let bytes: Vec<char> = body.chars().collect();
    let mut m: HashMap<String, Val> = HashMap::new();
    let mut i = 0;
    let n = bytes.len();
    let mut depth = 0i32;
    while i < n {
        let c = bytes[i];
        if c == '{' {
            depth += 1;
            i += 1;
            continue;
        }
        if c == '}' {
            depth -= 1;
            if depth <= 0 && i > 0 {
                break;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let s = i;
            while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
                i += 1;
            }
            let key: String = bytes[s..i].iter().collect();
            let mut j = i;
            while j < n && bytes[j].is_whitespace() {
                j += 1;
            }
            if j < n && bytes[j] == '=' && depth == 1 {
                j += 1;
                while j < n && bytes[j].is_whitespace() {
                    j += 1;
                }
                if j < n && bytes[j] == '{' {
                    let e = bytes[j..].iter().position(|&ch| ch == '}').map(|p| j + p).unwrap_or(n);
                    let inner: String = bytes[j + 1..e].iter().collect();
                    let nums: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    m.insert(key, Val::List(nums));
                    i = e + 1;
                    continue;
                } else if j < n && bytes[j] == '"' {
                    let e = bytes[j + 1..].iter().position(|&ch| ch == '"').map(|p| j + 1 + p).unwrap_or(n);
                    let s: String = bytes[j + 1..e.min(n)].iter().collect();
                    m.insert(key, Val::Str(s));
                    i = e + 1;
                    continue;
                } else {
                    let s = j;
                    while j < n && !matches!(bytes[j], ',' | '\n' | '}') {
                        j += 1;
                    }
                    let raw: String = bytes[s..j].iter().collect::<String>().trim().to_string();
                    let v = match raw.as_str() {
                        "true" => Val::Bool(true),
                        "false" => Val::Bool(false),
                        _ => match raw.parse::<f32>() {
                            Ok(f) => Val::Num(f),
                            Err(_) => Val::Str(raw),
                        },
                    };
                    m.insert(key, v);
                    i = j;
                    continue;
                }
            }
            continue;
        }
        i += 1;
    }
    to_develop(&m)
}

/// Parse an Adobe/Resolve .cube 3D LUT and resample it to 17³.
pub fn parse_cube(text: &str) -> Option<Lut> {
    let mut size = 0usize;
    let mut dmin = [0.0f32; 3];
    let mut dmax = [1.0f32; 3];
    let mut data: Vec<f32> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with("TITLE") {
            continue;
        }
        if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
            size = r.trim().parse().ok()?;
            continue;
        }
        if l.starts_with("LUT_1D_SIZE") {
            return None;
        }
        let parts: Vec<f32> = l.split_whitespace().filter_map(|p| p.parse().ok()).collect();
        if let Some(r) = l.strip_prefix("DOMAIN_MIN") {
            let p: Vec<f32> = r.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            if p.len() == 3 {
                dmin = [p[0], p[1], p[2]];
            }
            continue;
        }
        if let Some(r) = l.strip_prefix("DOMAIN_MAX") {
            let p: Vec<f32> = r.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            if p.len() == 3 {
                dmax = [p[0], p[1], p[2]];
            }
            continue;
        }
        if parts.len() == 3 {
            for k in 0..3 {
                data.push((parts[k] - dmin[k]) / (dmax[k] - dmin[k]).max(1e-6));
            }
        }
    }
    if size < 2 || data.len() < size * size * size * 3 {
        return None;
    }
    let src = Lut { size, data };
    let n = 17;
    let mut out = Vec::with_capacity(n * n * n * 3);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let c = [x as f32 / 16.0, y as f32 / 16.0, z as f32 / 16.0];
                out.extend_from_slice(&apply_lut(&src, c));
            }
        }
    }
    Some(Lut { size: n, data: out })
}

/// Parse any supported preset file by extension.
pub fn parse_file(path: &std::path::Path) -> Option<Develop> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    match ext.as_str() {
        "xmp" => parse_xmp(&text),
        "lrtemplate" => parse_lrtemplate(&text),
        "cube" => parse_cube(&text).map(|lut| Develop { lut: Some(lut), ..Develop::default() }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_identity() {
        let d = Develop::default();
        assert!(d.is_identity());
        for c in [[0.1, 0.5, 0.9], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.8, 0.2, 0.3]] {
            let o = eval_color(&d, c);
            for k in 0..3 {
                assert!((o[k] - c[k]).abs() < 0.01, "{c:?} -> {o:?}");
            }
        }
    }

    #[test]
    fn exposure_brightens() {
        let d = Develop { exposure: 1.0, ..Develop::default() };
        let o = eval_color(&d, [0.3, 0.3, 0.3]);
        assert!(o[0] > 0.38);
    }

    #[test]
    fn grayscale_is_grey() {
        let d = Develop { grayscale: true, ..Develop::default() };
        let o = eval_color(&d, [0.8, 0.2, 0.1]);
        assert!((o[0] - o[1]).abs() < 0.01 && (o[1] - o[2]).abs() < 0.01);
    }

    #[test]
    fn warm_temp_is_warmer() {
        let d = Develop { temp: 50.0, ..Develop::default() };
        let o = eval_color(&d, [0.5, 0.5, 0.5]);
        assert!(o[0] > o[2] + 0.05);
    }

    #[test]
    fn curve_lut_matches_points() {
        let l = curve_lut(&[[0.0, 20.0], [128.0, 140.0], [255.0, 240.0]]);
        let l = l.unwrap_or([0.0; 256]);
        assert!((l[0] * 255.0 - 20.0).abs() < 1.0);
        assert!((l[128] * 255.0 - 140.0).abs() < 1.0);
        assert!((l[255] * 255.0 - 240.0).abs() < 1.0);
    }

    #[test]
    fn xmp_parses() {
        let x = r#"<x:xmpmeta><rdf:Description crs:Exposure2012="+0.35" crs:Contrast2012="-12"
            crs:ConvertToGrayscale="False" crs:SaturationAdjustmentBlue="-20" crs:SplitToningShadowHue="210"
            crs:SplitToningShadowSaturation="15"><crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 18</rdf:li>
            <rdf:li>255, 245</rdf:li></rdf:Seq></crs:ToneCurvePV2012></rdf:Description></x:xmpmeta>"#;
        let d = parse_xmp(x);
        let d = d.unwrap_or_default();
        assert!((d.exposure - 0.35).abs() < 1e-4);
        assert_eq!(d.contrast, -12.0);
        assert_eq!(d.sat[5], -20.0);
        assert_eq!(d.curve, vec![[0.0, 18.0], [255.0, 245.0]]);
        assert_eq!(d.split_sh_sat, 15.0);
    }

    #[test]
    fn sharpening_only_is_not_a_look() {
        assert!(parse_xmp(r#"<rdf:Description crs:Sharpness="40" crs:LuminanceSmoothing="20"/>"#).is_none());
    }

    #[test]
    fn lrtemplate_parses() {
        let t = r#"s = { id = "x", internalName = "Fuji", title = "Fuji", type = "Develop",
            value = { settings = { ConvertToGrayscale = false, Exposure2012 = 0.2, Shadows2012 = 30,
            ToneCurvePV2012 = { 0, 10, 128, 128, 255, 250, }, Vibrance = -10, }, uuid = "u" }, version = 0, }"#;
        let d = parse_lrtemplate(t).unwrap_or_default();
        assert!((d.exposure - 0.2).abs() < 1e-4);
        assert_eq!(d.shadows, 30.0);
        assert_eq!(d.vibrance, -10.0);
        assert_eq!(d.curve.len(), 3);
    }

    #[test]
    fn cube_parses_identity() {
        let mut s = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    s.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        let l = parse_cube(&s);
        let l = l.unwrap_or(Lut { size: 0, data: vec![] });
        let o = apply_lut(&l, [0.25, 0.5, 0.75]);
        assert!((o[0] - 0.25).abs() < 1e-3 && (o[2] - 0.75).abs() < 1e-3);
    }

    #[test]
    fn apply_runs_with_spatial() {
        let img = RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 4) as u8, (y * 5) as u8, 128, 255]));
        let d = Develop { clarity: 40.0, vignette: -30.0, grain: 30.0, contrast: 20.0, ..Develop::default() };
        let o = apply(&img, &d, 1.0);
        assert_eq!(o.dimensions(), img.dimensions());
    }
}
