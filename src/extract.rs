//! Dominant-colour extraction: k-means in OKLab (perceptually even), with a
//! deterministic k-means++ seed so the same image always gives the same palette.

use crate::color::{linear_to_oklab, linear_to_rgb, oklab_to_linear, to_linear};
use image::RgbaImage;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Swatch {
    pub rgb: [u8; 3],
    pub lab: [f32; 3], // OKLab centroid
    pub share: f32,    // fraction of pixels 0..1
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[inline]
fn d2(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    // OKLab a/b span ~±0.3 while L spans 0..1; boost hue/chroma so a blue
    // sky and a pink cloud don't average into grey.
    let dl = a[0] - b[0];
    let da = (a[1] - b[1]) * 2.2;
    let db = (a[2] - b[2]) * 2.2;
    dl * dl + da * da + db * db
}

pub fn pixel_oklab(p: &[u8]) -> [f32; 3] {
    match p {
        [r, g, b, ..] => linear_to_oklab([to_linear(*r), to_linear(*g), to_linear(*b)]),
        _ => [0.0; 3],
    }
}

pub fn nearest(centroids: &[[f32; 3]], c: &[f32; 3]) -> usize {
    let mut best = 0;
    let mut bd = f32::MAX;
    for (i, k) in centroids.iter().enumerate() {
        let d = d2(k, c);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Extract `k` dominant colours, sorted from largest share to smallest.
pub fn extract(img: &RgbaImage, k: usize) -> Vec<Swatch> {
    let k = k.clamp(1, 32);
    let total = (img.width() as usize) * (img.height() as usize);
    if total == 0 {
        return Vec::new();
    }
    let stride = (total / 60_000).max(1);
    let samples: Vec<[f32; 3]> = img
        .as_raw()
        .chunks_exact(4)
        .step_by(stride)
        .filter(|p| p.get(3).copied().unwrap_or(255) > 16)
        .map(pixel_oklab)
        .collect();
    if samples.is_empty() {
        return Vec::new();
    }

    // k-means++ seeding.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut cents: Vec<[f32; 3]> = Vec::with_capacity(k);
    let first = (rng.next() as usize) % samples.len();
    cents.push(samples[first.min(samples.len() - 1)]);
    let mut dist: Vec<f32> = samples.iter().map(|s| d2(s, &cents[0])).collect();
    while cents.len() < k {
        let sum: f32 = dist.iter().sum();
        if sum <= 0.0 {
            break;
        }
        let mut target = rng.unit() * sum;
        let mut pick = samples.len() - 1;
        for (i, d) in dist.iter().enumerate() {
            target -= d;
            if target <= 0.0 {
                pick = i;
                break;
            }
        }
        let c = samples[pick];
        cents.push(c);
        dist.par_iter_mut().zip(samples.par_iter()).for_each(|(d, s)| {
            *d = d.min(d2(s, &c));
        });
    }

    // Lloyd iterations.
    let mut counts = vec![0usize; cents.len()];
    for _ in 0..24 {
        let kk = cents.len();
        let (sums, cnt) = samples
            .par_iter()
            .fold(
                || (vec![[0.0f64; 3]; kk], vec![0usize; kk]),
                |(mut s, mut c), p| {
                    let i = nearest(&cents, p);
                    if let (Some(si), Some(ci)) = (s.get_mut(i), c.get_mut(i)) {
                        si[0] += p[0] as f64;
                        si[1] += p[1] as f64;
                        si[2] += p[2] as f64;
                        *ci += 1;
                    }
                    (s, c)
                },
            )
            .reduce(
                || (vec![[0.0f64; 3]; kk], vec![0usize; kk]),
                |(mut s1, mut c1), (s2, c2)| {
                    for i in 0..kk {
                        for j in 0..3 {
                            s1[i][j] += s2[i][j];
                        }
                        c1[i] += c2[i];
                    }
                    (s1, c1)
                },
            );
        let mut moved = 0.0f32;
        for i in 0..kk {
            if cnt[i] > 0 {
                let n = cnt[i] as f64;
                let nc = [(sums[i][0] / n) as f32, (sums[i][1] / n) as f32, (sums[i][2] / n) as f32];
                moved = moved.max(d2(&nc, &cents[i]));
                cents[i] = nc;
            }
        }
        counts = cnt;
        if moved < 1e-7 {
            break;
        }
    }

    let n = samples.len() as f32;
    let mut out: Vec<Swatch> = cents
        .iter()
        .zip(&counts)
        .filter(|(_, c)| **c > 0)
        .map(|(c, &cnt)| Swatch { rgb: linear_to_rgb(oklab_to_linear(*c)), lab: *c, share: cnt as f32 / n })
        .collect();
    out.sort_by(|a, b| b.share.total_cmp(&a.share));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_colour_image() {
        let mut img = RgbaImage::new(40, 10);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 30 { image::Rgba([200, 30, 30, 255]) } else { image::Rgba([20, 40, 200, 255]) };
        }
        let s = extract(&img, 2);
        assert_eq!(s.len(), 2);
        assert!((s[0].share - 0.75).abs() < 0.02);
        assert!(s[0].rgb[0] > 190 && s[1].rgb[2] > 190, "{s:?}");
    }

    #[test]
    fn empty_image() {
        assert!(extract(&RgbaImage::new(0, 0), 5).is_empty());
    }
}
