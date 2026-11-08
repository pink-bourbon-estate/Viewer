//! Paints, Kubelka–Munk style mixing, and the recipe solver.
//!
//! Mixing model: each paint has a masstone colour and a scattering
//! strength S. Per linear-RGB channel, K/S comes from the masstone via the
//! Kubelka–Munk relation K/S = (1-R)^2 / 2R, so K = (K/S)·S. A mixture's
//! K/S is ΣcK / ΣcS, and reflectance is recovered with
//! R = 1 + K/S − sqrt((K/S)^2 + 2K/S). This is subtractive: ultramarine +
//! cadmium yellow gives a dark olive green, not grey. It is an approximation
//! (three channels, not a spectrum), so recipes are a starting pile, not a
//! formula. Calibrate by editing `palette.json` with photos of your own
//! tube colours.

use crate::color::{delta_e2000, linear_to_lab, linear_to_rgb, rgb_to_lab, rgb_to_linear};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Paint {
    pub name: String,
    /// Masstone colour (sRGB) as it looks thickly applied.
    pub rgb: [u8; 3],
    /// Scattering / hiding power, 0.05 (transparent) .. 1.0 (white).
    pub scatter: f32,
    /// Relative tinting strength multiplier (1.0 = typical).
    pub strength: f32,
    /// True for the white used to lighten (excluded from the "colour" picks).
    #[serde(default)]
    pub is_white: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PaletteDef {
    pub name: String,
    pub paints: Vec<Paint>,
}

fn paint(name: &str, rgb: [u8; 3], scatter: f32, strength: f32) -> Paint {
    Paint { name: name.to_string(), rgb, scatter, strength, is_white: false }
}

impl PaletteDef {
    /// Patrick's 12-colour palette ("My Palette for Painting ANYTHING in Oils").
    pub fn patrick() -> Self {
        let mut white = paint("Titanium White", [247, 247, 242], 1.0, 1.0);
        white.is_white = true;
        PaletteDef {
            name: "Patrick's palette".into(),
            paints: vec![
                white,
                paint("Cadmium Yellow Light", [252, 222, 40], 0.65, 1.0),
                paint("Cadmium Yellow Medium", [250, 180, 18], 0.65, 1.0),
                paint("Cadmium Orange", [238, 112, 22], 0.6, 1.0),
                paint("Cadmium Red Light", [212, 42, 32], 0.55, 1.0),
                paint("Quinacridone Magenta", [150, 28, 84], 0.12, 1.1),
                paint("Ultramarine Blue", [32, 40, 138], 0.15, 1.0),
                paint("Cobalt Blue", [24, 78, 172], 0.35, 0.75),
                paint("Viridian", [18, 102, 86], 0.18, 0.8),
                paint("Ivory Black", [30, 30, 32], 0.4, 1.0),
                paint("Burnt Sienna", [138, 64, 34], 0.3, 1.0),
                paint("Yellow Ochre", [198, 144, 52], 0.6, 0.75),
            ],
        }
    }
}

/// Precomputed optics of one paint.
#[derive(Clone, Copy, Debug)]
pub struct Optics {
    pub k: [f32; 3],
    pub s: f32,
}

pub fn optics(p: &Paint) -> Optics {
    let lin = rgb_to_linear(p.rgb);
    let mut k = [0.0; 3];
    for i in 0..3 {
        let r = lin[i].clamp(0.002, 0.995);
        k[i] = (1.0 - r) * (1.0 - r) / (2.0 * r) * p.scatter;
    }
    Optics { k, s: p.scatter }
}

/// Mix paints (by optics) with concentration weights. Returns linear RGB.
pub fn mix(optics: &[Optics], weights: &[f32]) -> [f32; 3] {
    let mut kk = [0.0f32; 3];
    let mut ss = 0.0f32;
    for (o, &w) in optics.iter().zip(weights) {
        if w <= 0.0 {
            continue;
        }
        for i in 0..3 {
            kk[i] += w * o.k[i];
        }
        ss += w * o.s;
    }
    if ss <= 0.0 {
        return [0.0; 3];
    }
    let mut out = [0.0; 3];
    for i in 0..3 {
        let ks = kk[i] / ss;
        out[i] = (1.0 + ks - (ks * ks + 2.0 * ks).sqrt()).clamp(0.0, 1.0);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Recipe {
    /// (paint index, fraction of the pile 0..1), largest first.
    pub parts: Vec<(usize, f32)>,
    pub predicted: [u8; 3],
    pub delta_e: f32,
}

impl Recipe {
    /// Painter-friendly text, e.g. "4 Titanium White : 1 Ultramarine Blue + touch of Burnt Sienna".
    pub fn describe(&self, pal: &PaletteDef) -> Vec<String> {
        let name = |i: usize| pal.paints.get(i).map(|p| p.name.as_str()).unwrap_or("?");
        let main: Vec<&(usize, f32)> = self.parts.iter().filter(|(_, f)| *f >= 0.06).collect();
        let touches: Vec<&(usize, f32)> = self.parts.iter().filter(|(_, f)| *f < 0.06).collect();
        let smallest = main.iter().map(|(_, f)| *f).fold(1.0f32, f32::min).max(0.01);
        let mut lines = Vec::new();
        for (i, f) in &main {
            let ratio = f / smallest;
            let parts = if ratio < 1.25 {
                "1 part".to_string()
            } else {
                let r = (ratio * 2.0).round() / 2.0;
                if (r - r.round()).abs() < 0.01 {
                    format!("{} parts", r.round() as i32)
                } else {
                    format!("{r:.1} parts")
                }
            };
            lines.push(format!("{parts}  {}  ({:.0}%)", name(*i), f * 100.0));
        }
        for (i, f) in &touches {
            let word = if *f < 0.02 { "a speck of" } else { "a touch of" };
            lines.push(format!("{word}  {}  ({:.0}%)", name(*i), (f * 100.0).max(1.0)));
        }
        lines
    }

    pub fn accuracy_word(&self) -> &'static str {
        match self.delta_e {
            d if d < 2.5 => "very close",
            d if d < 5.0 => "close",
            d if d < 9.0 => "near — adjust by eye",
            _ => "out of gamut for this palette — closest possible",
        }
    }
}

pub struct Solver {
    pub palette: PaletteDef,
    optics: Vec<Optics>,
    white: Option<usize>,
}

/// All weight vectors with `n` non-negative integer parts summing to `total`.
fn compositions(n: usize, total: u32, out: &mut Vec<Vec<u32>>) {
    fn rec(n: usize, left: u32, cur: &mut Vec<u32>, out: &mut Vec<Vec<u32>>) {
        if n == 1 {
            cur.push(left);
            out.push(cur.clone());
            cur.pop();
            return;
        }
        for v in 0..=left {
            cur.push(v);
            rec(n - 1, left - v, cur, out);
            cur.pop();
        }
    }
    if n > 0 {
        rec(n, total, &mut Vec::new(), out);
    }
}

impl Solver {
    pub fn new(palette: PaletteDef) -> Self {
        let optics = palette.paints.iter().map(optics).collect();
        let white = palette.paints.iter().position(|p| p.is_white);
        Solver { palette, optics, white }
    }

    fn eval(&self, idx: &[usize], w: &[f32], target: [f32; 3]) -> ([f32; 3], f32) {
        let mut o = [Optics { k: [0.0; 3], s: 0.0 }; 4];
        let mut ww = [0.0f32; 4];
        let mut n = 0;
        for (&i, &wi) in idx.iter().zip(w) {
            if n >= 4 {
                break;
            }
            if let (Some(op), Some(p)) = (self.optics.get(i), self.palette.paints.get(i)) {
                o[n] = *op;
                ww[n] = wi * p.strength;
                n += 1;
            }
        }
        let lin = mix(&o[..n], &ww[..n]);
        let lab = linear_to_lab(lin);
        (lin, delta_e2000(lab, target))
    }

    fn complexity(set: &[usize], w: &[f32]) -> f32 {
        // A painter would rather mix 3 paints at ΔE 2.5 than 4 at ΔE 0.
        // Each paint beyond the first costs 1.2 ΔE; tiny specks cost extra
        // because they are hard to measure on a palette.
        let extra: f32 = w
            .iter()
            .skip(1)
            .map(|&f| if f < 0.025 { 1.8 } else { 1.2 })
            .sum();
        let _ = set;
        extra + if w.len() >= 4 { 1.5 } else { 0.0 }
    }

    fn refine(&self, set: &[usize], mut w: Vec<f32>, target: [f32; 3]) -> (Vec<f32>, f32) {
        let mut step = 0.03f32;
        let (_, mut cur) = self.eval(set, &w, target);
        for _ in 0..80 {
            let mut improved = false;
            for i in 0..w.len() {
                for dir in [-1.0f32, 1.0] {
                    let mut t = w.clone();
                    if let Some(v) = t.get_mut(i) {
                        *v = (*v + dir * step).max(0.002);
                    }
                    let s: f32 = t.iter().sum();
                    t.iter_mut().for_each(|v| *v /= s);
                    let (_, de) = self.eval(set, &t, target);
                    if de < cur {
                        cur = de;
                        w = t;
                        improved = true;
                    }
                }
            }
            if !improved {
                step *= 0.5;
                if step < 0.0008 {
                    break;
                }
            }
        }
        (w, cur)
    }

    /// Find a simple recipe (≤ 3 colours + white) for `target`.
    pub fn solve(&self, target_rgb: [u8; 3]) -> Recipe {
        let target = rgb_to_lab(target_rgb);
        let n = self.palette.paints.len();
        let colours: Vec<usize> = (0..n).filter(|&i| Some(i) != self.white).collect();

        // Candidate paint sets.
        let mut sets: Vec<Vec<usize>> = Vec::new();
        for a in 0..colours.len() {
            sets.push(vec![colours[a]]);
            for b in a + 1..colours.len() {
                sets.push(vec![colours[a], colours[b]]);
                for c in b + 1..colours.len() {
                    sets.push(vec![colours[a], colours[b], colours[c]]);
                }
            }
        }
        if let Some(w) = self.white {
            let with_white: Vec<Vec<usize>> = sets
                .iter()
                .map(|s| {
                    let mut v = s.clone();
                    v.push(w);
                    v
                })
                .collect();
            sets.extend(with_white);
            sets.push(vec![w]);
        }

        let mut grids: Vec<Vec<Vec<u32>>> = vec![Vec::new()];
        for k in 1..=4 {
            let mut g = Vec::new();
            // Finer grids for fewer paints keep the search cheap but precise.
            let total = match k {
                1 => 1,
                2 => 40,
                3 => 24,
                _ => 16,
            };
            compositions(k, total, &mut g);
            g.retain(|c| c.iter().all(|&v| v > 0));
            grids.push(g);
        }

        // Coarse grid search per paint set.
        let mut coarse: Vec<(f32, Vec<usize>, Vec<f32>)> = sets
            .par_iter()
            .filter_map(|set| {
                let grid = grids.get(set.len())?;
                let mut best: Option<(f32, Vec<f32>)> = None;
                for comp in grid {
                    let tot: u32 = comp.iter().sum();
                    let mut w = [0.0f32; 4];
                    for (d, &v) in w.iter_mut().zip(comp) {
                        *d = v as f32 / tot as f32;
                    }
                    let (_, de) = self.eval(set, &w[..set.len()], target);
                    if best.as_ref().is_none_or(|b| de < b.0) {
                        best = Some((de, w[..set.len()].to_vec()));
                    }
                }
                best.map(|(de, w)| (de, set.clone(), w))
            })
            .collect();
        coarse.sort_by(|a, b| a.0.total_cmp(&b.0));
        coarse.truncate(24);

        // Refine the most promising sets (coordinate descent), then pick by
        // accuracy with a small penalty per extra paint so simple piles win ties.
        let refined: Vec<(f32, Vec<usize>, Vec<f32>)> = coarse
            .into_par_iter()
            .map(|(_, set, w)| {
                let (w, de) = self.refine(&set, w, target);
                let score = de + Self::complexity(&set, &w);
                (score, set, w)
            })
            .collect();
        let Some((_, set, w)) = refined.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)) else {
            return Recipe { parts: Vec::new(), predicted: [0; 3], delta_e: f32::MAX };
        };

        let (lin, de) = self.eval(&set, &w, target);
        let mut parts: Vec<(usize, f32)> = set.into_iter().zip(w).collect();
        parts.sort_by(|a, b| b.1.total_cmp(&a.1));
        Recipe { parts, predicted: linear_to_rgb(lin), delta_e: de }
    }
}

/// Grisaille ramp: burnt umber + ultramarine darkened neutral, lightened with
/// white. `temperature` 0 = all umber (warm), 1 = all ultramarine (cool).
/// Returns a 101-entry LUT from L* (0..=100) to sRGB.
pub fn grisaille_lut(temperature: f32) -> Vec<[u8; 3]> {
    let umber = optics(&paint("Burnt Umber", [74, 50, 36], 0.4, 1.0));
    let ultra = optics(&paint("Ultramarine Blue", [32, 40, 138], 0.15, 1.0));
    let white = optics(&paint("Titanium White", [247, 247, 242], 1.0, 1.0));
    let t = temperature.clamp(0.0, 1.0);
    // Ultramarine is the stronger tinter; scale so 0.5 reads near neutral.
    let wu = (1.0 - t) * 1.0;
    let wb = t * 0.55;
    let mut lut = Vec::with_capacity(101);
    for l in 0..=100 {
        let target = l as f32;
        // Binary search the white fraction whose mix hits target L*.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        let mut best = [0.0f32; 3];
        for _ in 0..22 {
            let f = 0.5 * (lo + hi);
            let dark = 1.0 - f;
            let lin = mix(&[umber, ultra, white], &[dark * wu, dark * wb, f]);
            let ls = linear_to_lab(lin)[0];
            best = lin;
            if ls < target {
                lo = f;
            } else {
                hi = f;
            }
        }
        lut.push(linear_to_rgb(best));
    }
    lut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb_to_hsv;

    #[test]
    fn blue_plus_yellow_is_green() {
        let pal = PaletteDef::patrick();
        let o: Vec<Optics> = pal.paints.iter().map(optics).collect();
        // index 1 = cad yellow light, 6 = ultramarine
        let lin = mix(&[o[1], o[6]], &[1.0, 0.3]);
        let rgb = linear_to_rgb(lin);
        let h = rgb_to_hsv(rgb)[0];
        assert!((60.0..=170.0).contains(&h), "hue {h} rgb {rgb:?}");
    }

    #[test]
    fn white_dominates_lightening() {
        let pal = PaletteDef::patrick();
        let o: Vec<Optics> = pal.paints.iter().map(optics).collect();
        let lin = mix(&[o[0], o[6]], &[4.0, 1.0]);
        let lab = linear_to_lab(lin);
        assert!(lab[0] > 45.0 && lab[2] < -20.0, "{lab:?}");
    }

    #[test]
    fn solver_finds_paint_itself() {
        let s = Solver::new(PaletteDef::patrick());
        let r = s.solve([138, 64, 34]);
        assert!(r.delta_e < 1.5, "{r:?}");
        assert_eq!(r.parts.first().map(|p| p.0), Some(10));
    }

    #[test]
    fn solver_sky_blue_uses_white_and_blue() {
        let s = Solver::new(PaletteDef::patrick());
        let r = s.solve([150, 180, 215]);
        assert!(r.delta_e < 6.0, "{r:?}");
        assert_eq!(r.parts.first().map(|p| p.0), Some(0), "{r:?}");
    }

    #[test]
    fn grisaille_lut_monotone() {
        let lut = grisaille_lut(0.5);
        let ls: Vec<f32> = lut.iter().map(|c| crate::color::lightness(*c)).collect();
        for w in ls.windows(2) {
            assert!(w[1] + 0.5 >= w[0]);
        }
        assert!(ls[100] > 90.0);
    }
}
