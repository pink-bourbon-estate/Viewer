//! Colour science: sRGB transfer, CIELAB (D65), OKLab, CIEDE2000, HSV and
//! painter-language descriptions (Munsell-style value, hue family, chroma).

use std::sync::OnceLock;

/// sRGB 8-bit -> linear light lookup.
fn srgb_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

#[inline]
pub fn to_linear(c: u8) -> f32 {
    srgb_lut()[c as usize]
}

#[inline]
pub fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

pub fn rgb_to_linear(rgb: [u8; 3]) -> [f32; 3] {
    [to_linear(rgb[0]), to_linear(rgb[1]), to_linear(rgb[2])]
}

pub fn linear_to_rgb(l: [f32; 3]) -> [u8; 3] {
    [to_srgb(l[0]), to_srgb(l[1]), to_srgb(l[2])]
}

/// CIELAB (D65) from linear sRGB.
pub fn linear_to_lab(l: [f32; 3]) -> [f32; 3] {
    let x = 0.412_456_4 * l[0] + 0.357_576_1 * l[1] + 0.180_437_5 * l[2];
    let y = 0.212_672_9 * l[0] + 0.715_152_2 * l[1] + 0.072_175 * l[2];
    let z = 0.019_333_9 * l[0] + 0.119_192 * l[1] + 0.950_304_1 * l[2];
    let f = |t: f32| {
        if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
    };
    let fx = f(x / 0.950_47);
    let fy = f(y);
    let fz = f(z / 1.088_83);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn rgb_to_lab(rgb: [u8; 3]) -> [f32; 3] {
    linear_to_lab(rgb_to_linear(rgb))
}

/// Relative luminance L* (0..100) of an sRGB colour.
pub fn lightness(rgb: [u8; 3]) -> f32 {
    let l = rgb_to_linear(rgb);
    let y = 0.212_672_9 * l[0] + 0.715_152_2 * l[1] + 0.072_175 * l[2];
    if y > 0.008_856 { 116.0 * y.cbrt() - 16.0 } else { 903.3 * y }
}

/// Inverse of [`lightness`] for a neutral grey: L* -> linear Y.
pub fn lstar_to_y(l: f32) -> f32 {
    let l = l.clamp(0.0, 100.0);
    if l > 8.0 { ((l + 16.0) / 116.0).powi(3) } else { l / 903.3 }
}

/// OKLab from linear sRGB (Björn Ottosson, public domain formulation).
pub fn linear_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_47 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2];
    let m = 0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2];
    let s = 0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2];
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn oklab_to_linear(c: [f32; 3]) -> [f32; 3] {
    let l = c[0] + 0.396_337_78 * c[1] + 0.215_803_76 * c[2];
    let m = c[0] - 0.105_561_346 * c[1] - 0.063_854_17 * c[2];
    let s = c[0] - 0.089_484_18 * c[1] - 1.291_485_5 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// CIEDE2000 colour difference between two CIELAB colours.
pub fn delta_e2000(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (l1, a1, b1) = (a[0] as f64, a[1] as f64, a[2] as f64);
    let (l2, a2, b2) = (b[0] as f64, b[1] as f64, b[2] as f64);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cb = (c1 + c2) / 2.0;
    let cb7 = cb.powi(7);
    let g = 0.5 * (1.0 - (cb7 / (cb7 + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let hp = |bb: f64, ap: f64| {
        if bb == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = bb.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let h1p = hp(b1, a1p);
    let h2p = hp(b2, a2p);
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let mut dhp = 0.0;
    if c1p * c2p != 0.0 {
        dhp = h2p - h1p;
        if dhp > 180.0 {
            dhp -= 360.0;
        } else if dhp < -180.0 {
            dhp += 360.0;
        }
    }
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lbp = (l1 + l2) / 2.0;
    let cbp = (c1p + c2p) / 2.0;
    let hbp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hbp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hbp).to_radians().cos()
        + 0.32 * (3.0 * hbp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hbp - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hbp - 275.0) / 25.0).powi(2)).exp();
    let cbp7 = cbp.powi(7);
    let rc = 2.0 * (cbp7 / (cbp7 + 25f64.powi(7))).sqrt();
    let sl = 1.0 + (0.015 * (lbp - 50.0).powi(2)) / (20.0 + (lbp - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -(2.0 * dtheta.to_radians()).sin() * rc;
    let v = (dlp / sl).powi(2)
        + (dcp / sc).powi(2)
        + (dhp_big / sh).powi(2)
        + rt * (dcp / sc) * (dhp_big / sh);
    v.max(0.0).sqrt() as f32
}

/// HSV: hue in degrees, saturation and value 0..100.
pub fn rgb_to_hsv(rgb: [u8; 3]) -> [f32; 3] {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    [h, s * 100.0, max * 100.0]
}

/// A short painter-language description: "light, muted blue-green, cool".
pub fn describe(rgb: [u8; 3]) -> String {
    let lab = rgb_to_lab(rgb);
    let v = lab[0] / 10.0;
    let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    let value_word = match v {
        x if x < 2.0 => "very dark",
        x if x < 4.0 => "dark",
        x if x < 6.0 => "mid",
        x if x < 8.0 => "light",
        _ => "very light",
    };
    if chroma < 6.0 {
        return format!("{value_word} neutral grey");
    }
    let chroma_word = match chroma {
        c if c < 18.0 => "greyed",
        c if c < 40.0 => "muted",
        c if c < 65.0 => "clear",
        _ => "intense",
    };
    let ok = linear_to_oklab(rgb_to_linear(rgb));
    let mut h = ok[2].atan2(ok[1]).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    let family = hue_family(h);
    // Temperature: oranges/yellows/reds warm, blues/blue-greens cool.
    let temp = if (20.0..=110.0).contains(&h) || h >= 345.0 {
        "warm"
    } else if (160.0..=290.0).contains(&h) {
        "cool"
    } else {
        "neutral temp"
    };
    // Low-value yellows read as olive / browns.
    let family = match (family, v) {
        ("orange", x) | ("yellow-orange", x) if x < 5.0 => "brown",
        ("yellow", x) if x < 6.0 => "olive",
        (f, _) => f,
    };
    format!("{value_word}, {chroma_word} {family}, {temp}")
}

/// OKLCh hue angle -> family name.
pub fn hue_family(h: f32) -> &'static str {
    match h {
        x if x < 15.0 => "magenta-red",
        x if x < 40.0 => "red",
        x if x < 60.0 => "red-orange",
        x if x < 75.0 => "orange",
        x if x < 90.0 => "yellow-orange",
        x if x < 110.0 => "yellow",
        x if x < 135.0 => "yellow-green",
        x if x < 165.0 => "green",
        x if x < 200.0 => "blue-green",
        x if x < 245.0 => "blue",
        x if x < 285.0 => "blue-violet",
        x if x < 320.0 => "violet",
        x if x < 345.0 => "magenta",
        _ => "magenta-red",
    }
}

pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=255u8 {
            assert_eq!(to_srgb(to_linear(i)), i);
        }
    }

    #[test]
    fn lab_white_black() {
        let w = rgb_to_lab([255, 255, 255]);
        assert!((w[0] - 100.0).abs() < 0.1 && w[1].abs() < 0.5 && w[2].abs() < 0.5);
        let b = rgb_to_lab([0, 0, 0]);
        assert!(b[0].abs() < 0.01);
    }

    #[test]
    fn de2000_reference_pair() {
        // Sharma et al. test data, pair 1: expected 2.0425
        let d = delta_e2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((d - 2.0425).abs() < 1e-3, "{d}");
    }

    #[test]
    fn oklab_roundtrip() {
        let c = [0.2, 0.5, 0.8];
        let back = oklab_to_linear(linear_to_oklab(c));
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn hsv_basic() {
        let h = rgb_to_hsv([255, 0, 0]);
        assert!((h[0]).abs() < 0.01 && (h[1] - 100.0).abs() < 0.01);
        let b = rgb_to_hsv([0, 0, 255]);
        assert!((b[0] - 240.0).abs() < 0.01);
    }
}
