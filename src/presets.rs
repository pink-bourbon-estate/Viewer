//! Preset library: built-in looks (compiled into the binary), user presets
//! ("Save as new preset" forks), and the condenser that turns a folder of
//! hundreds of Lightroom presets / LUTs into a few dozen representative looks.

use crate::develop::{eval_color, parse_file, Develop, Lut};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub note: String,
    pub dev: Develop,
}

pub const CATEGORY_ORDER: &[&str] = &[
    "My presets",
    "Black & White",
    "Kodak",
    "Fuji",
    "Ilford",
    "Leica",
    "Agfa",
    "Cinema",
    "Instant & Vintage",
    "Portrait & Wedding",
    "Landscape & Travel",
    "Creative",
];

/// Card colours for the category grid (background, text).
pub fn category_colors(cat: &str) -> ([u8; 3], [u8; 3]) {
    match cat {
        "Black & White" => ([40, 40, 40], [240, 240, 240]),
        "Kodak" => ([246, 184, 52], [196, 32, 32]),
        "Fuji" => ([22, 150, 100], [250, 250, 250]),
        "Ilford" => ([16, 16, 16], [245, 245, 245]),
        "Leica" => ([200, 20, 30], [255, 255, 255]),
        "Agfa" => ([30, 70, 160], [250, 210, 60]),
        "Cinema" => ([20, 52, 64], [250, 170, 80]),
        "Instant & Vintage" => ([226, 214, 190], [90, 70, 50]),
        "Portrait & Wedding" => ([214, 186, 172], [70, 50, 50]),
        "Landscape & Travel" => ([76, 110, 70], [240, 236, 220]),
        "My presets" => ([92, 76, 52], [250, 230, 200]),
        _ => ([70, 60, 90], [240, 230, 250]),
    }
}

pub fn builtin() -> Vec<Preset> {
    let mut v: Vec<Preset> = serde_json::from_str(include_str!("../assets/film_presets.json")).unwrap_or_default();
    let condensed: Vec<Preset> = serde_json::from_str(include_str!("../assets/condensed_presets.json")).unwrap_or_default();
    v.extend(condensed);
    v
}

fn user_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("my_presets.json"))
}

pub fn load_user() -> Vec<Preset> {
    user_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_user(list: &[Preset]) -> Result<(), String> {
    let p = user_path().ok_or("no config directory")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| e.to_string())
}

// --------------------------------------------------------------------------
// Condensing a preset library
// --------------------------------------------------------------------------

/// Pick a category from a preset's name and path.
pub fn categorise(name: &str, path: &str, d: &Develop) -> &'static str {
    let s = format!("{} {}", name, path).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if has(&["ilford", "hp5", "delta", "fp4", "pan f", "xp2"]) {
        return "Ilford";
    }
    if d.grayscale || has(&["b&w", "b-w", "bw ", "mono", "noir", "ansel", "tri-x", "trix", "tmax", "acros"]) {
        return "Black & White";
    }
    if has(&["leica"]) {
        return "Leica";
    }
    if has(&["cine", "movie", "film look", "teal", "500t", "800t", "vision", "dope", "avatar"]) {
        return "Cinema";
    }
    if has(&["fuji", "400h", "160ns", "superia", "velvia", "provia", "astia", "pro 400", "natura"]) {
        return "Fuji";
    }
    if has(&["kodak", "portra", "ektar", "kodachrome", "ektachrome", "gold", "ultramax", "colorplus", "kf"]) {
        return "Kodak";
    }
    if has(&["agfa", "vista", "optima", "ultra 100"]) {
        return "Agfa";
    }
    if has(&["autochrome", "polaroid", "instax", "vintage", "expired", "retro", "fade", "old"]) {
        return "Instant & Vintage";
    }
    if has(&["villa", "davina", "daniel", "jide", "chrisman", "wedding", "skin", "portrait", "dvlop", "affob"]) {
        return "Portrait & Wedding";
    }
    if has(&["burkard", "mcclintock", "landscape", "travel", "sky", "earth", "flora", "sunlight", "outdoor"]) {
        return "Landscape & Travel";
    }
    "Creative"
}

/// A clean display name: strip numbering, prefixes and file noise.
pub fn clean_name(stem: &str) -> String {
    let mut s = stem.replace(['_', '+'], " ");
    s = s.trim_start_matches(|c: char| c == '~' || c == '-' || c == '.' || c.is_whitespace()).to_string();
    // "02a. " / "A. " / "04 " numbering
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_digit()) {
        i += 1;
    }
    if i > 0 && i < bytes.len() {
        let mut j = i;
        if j < bytes.len() && bytes[j].is_ascii_lowercase() {
            j += 1;
        }
        if j < bytes.len() && (bytes[j] == '.' || bytes[j] == ' ' || bytes[j] == '-') {
            s = bytes[j + 1..].iter().collect::<String>().trim().to_string();
        }
    } else if bytes.len() > 3 && bytes[0].is_ascii_uppercase() && bytes[1] == '.' && bytes[2] == ' ' {
        s = bytes[3..].iter().collect();
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() { stem.to_string() } else { s }
}

/// Test chart in sRGB: grey ramp, hue wheel at three values, skin tones.
fn chart() -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    for i in 0..9 {
        let g = i as f32 / 8.0;
        v.push([g, g, g]);
    }
    for (val, sat) in [(0.85f32, 0.35f32), (0.6, 0.6), (0.35, 0.5)] {
        for h in (0..360).step_by(30) {
            let h = h as f32 / 60.0;
            let x = 1.0 - (h % 2.0 - 1.0).abs();
            let (r, g, b) = match h as i32 {
                0 => (1.0, x, 0.0),
                1 => (x, 1.0, 0.0),
                2 => (0.0, 1.0, x),
                3 => (0.0, x, 1.0),
                4 => (x, 0.0, 1.0),
                _ => (1.0, 0.0, x),
            };
            v.push([val * (1.0 - sat + sat * r), val * (1.0 - sat + sat * g), val * (1.0 - sat + sat * b)]);
        }
    }
    for s in [[0.87, 0.69, 0.58], [0.72, 0.52, 0.40], [0.45, 0.30, 0.22]] {
        v.push(s);
    }
    v
}

/// How a develop transforms the chart, in OKLab (the "look fingerprint").
pub fn fingerprint(d: &Develop, chart: &[[f32; 3]]) -> Vec<f32> {
    let mut f = Vec::with_capacity(chart.len() * 3);
    for c in chart {
        let o = eval_color(d, *c);
        let lab = crate::color::linear_to_oklab([
            crate::color::to_linear((o[0].clamp(0.0, 1.0) * 255.0) as u8),
            crate::color::to_linear((o[1].clamp(0.0, 1.0) * 255.0) as u8),
            crate::color::to_linear((o[2].clamp(0.0, 1.0) * 255.0) as u8),
        ]);
        f.extend_from_slice(&[lab[0], lab[1] * 1.5, lab[2] * 1.5]);
    }
    f
}

fn dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt() / (a.len() as f32).sqrt()
}

struct Item {
    name: String,
    category: &'static str,
    dev: Develop,
    fp: Vec<f32>,
}

fn sample_curve_pts(p: &[[f32; 2]]) -> Vec<f32> {
    let lut = crate::develop::curve_lut(p);
    (0..=8)
        .map(|i| {
            let x = i * 32;
            let x = x.min(255);
            match &lut {
                Some(l) => l[x] * 255.0,
                None => x as f32,
            }
        })
        .collect()
}

fn curve_from_samples(s: &[f32]) -> Vec<[f32; 2]> {
    let pts: Vec<[f32; 2]> = s.iter().enumerate().map(|(i, &y)| [((i * 32).min(255)) as f32, y.clamp(0.0, 255.0)]).collect();
    if pts.iter().all(|p| (p[0] - p[1]).abs() < 0.5) { Vec::new() } else { pts }
}

/// Average a group of develops parameter by parameter.
pub fn average(devs: &[&Develop]) -> Develop {
    let n = devs.len().max(1) as f32;
    let mean = |f: &dyn Fn(&Develop) -> f32| devs.iter().map(|d| f(d)).sum::<f32>() / n;
    let mut a = Develop {
        exposure: mean(&|d| d.exposure),
        contrast: mean(&|d| d.contrast),
        highlights: mean(&|d| d.highlights),
        shadows: mean(&|d| d.shadows),
        whites: mean(&|d| d.whites),
        blacks: mean(&|d| d.blacks),
        temp: mean(&|d| d.temp),
        tint: mean(&|d| d.tint),
        vibrance: mean(&|d| d.vibrance),
        saturation: mean(&|d| d.saturation),
        clarity: mean(&|d| d.clarity),
        dehaze: mean(&|d| d.dehaze),
        fade: mean(&|d| d.fade),
        split_balance: mean(&|d| d.split_balance),
        shadow_tint: mean(&|d| d.shadow_tint),
        vignette: mean(&|d| d.vignette),
        grain: mean(&|d| d.grain),
        grain_size: mean(&|d| d.grain_size),
        grayscale: devs.iter().filter(|d| d.grayscale).count() * 2 > devs.len(),
        ..Develop::default()
    };
    for i in 0..4 {
        a.param[i] = mean(&|d| d.param[i]);
    }
    for i in 0..8 {
        a.hue[i] = mean(&|d| d.hue[i]);
        a.sat[i] = mean(&|d| d.sat[i]);
        a.lum[i] = mean(&|d| d.lum[i]);
        a.gray_mix[i] = mean(&|d| d.gray_mix[i]);
    }
    for i in 0..3 {
        a.cal_hue[i] = mean(&|d| d.cal_hue[i]);
        a.cal_sat[i] = mean(&|d| d.cal_sat[i]);
    }
    // Split toning: average as vectors so hues don't cancel badly.
    let avg_hue = |hs: &dyn Fn(&Develop) -> (f32, f32)| {
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for d in devs {
            let (h, s) = hs(d);
            x += h.to_radians().cos() * s;
            y += h.to_radians().sin() * s;
        }
        let h = y.atan2(x).to_degrees().rem_euclid(360.0);
        (h, (x * x + y * y).sqrt() / n)
    };
    (a.split_sh_hue, a.split_sh_sat) = avg_hue(&|d| (d.split_sh_hue, d.split_sh_sat));
    (a.split_hi_hue, a.split_hi_sat) = avg_hue(&|d| (d.split_hi_hue, d.split_hi_sat));
    (a.split_mid_hue, a.split_mid_sat) = avg_hue(&|d| (d.split_mid_hue, d.split_mid_sat));
    let avg_curve = |get: &dyn Fn(&Develop) -> &Vec<[f32; 2]>| {
        let mut acc = [0.0f32; 9];
        for d in devs {
            for (a, v) in acc.iter_mut().zip(sample_curve_pts(get(d))) {
                *a += v;
            }
        }
        curve_from_samples(&acc.map(|v| v / n))
    };
    a.curve = avg_curve(&|d| &d.curve);
    a.curve_r = avg_curve(&|d| &d.curve_r);
    a.curve_g = avg_curve(&|d| &d.curve_g);
    a.curve_b = avg_curve(&|d| &d.curve_b);
    let luts: Vec<&Lut> = devs.iter().filter_map(|d| d.lut.as_ref()).collect();
    if !luts.is_empty() && luts.len() * 2 >= devs.len() && luts.iter().all(|l| l.size == luts[0].size) {
        let len = luts[0].data.len();
        let mut data = vec![0.0f32; len];
        for l in &luts {
            for (d, v) in data.iter_mut().zip(&l.data) {
                *d += v;
            }
        }
        let k = luts.len() as f32;
        data.iter_mut().for_each(|v| *v /= k);
        a.lut = Some(Lut { size: luts[0].size, data });
    }
    a
}

/// Agglomerative (average-linkage) clustering down to `k` groups.
fn cluster(fps: &[&[f32]], k: usize) -> Vec<Vec<usize>> {
    let n = fps.len();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    if n <= k {
        return groups;
    }
    let mut d = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let v = dist(fps[i], fps[j]);
            d[i][j] = v;
            d[j][i] = v;
        }
    }
    let mut alive: Vec<bool> = vec![true; n];
    let mut count = n;
    while count > k {
        let mut best = (f32::MAX, 0, 0);
        for i in 0..n {
            if !alive[i] {
                continue;
            }
            for j in i + 1..n {
                if alive[j] && d[i][j] < best.0 {
                    best = (d[i][j], i, j);
                }
            }
        }
        let (_, a, b) = best;
        let (na, nb) = (groups[a].len() as f32, groups[b].len() as f32);
        for x in 0..n {
            if alive[x] && x != a && x != b {
                let v = (d[a][x] * na + d[b][x] * nb) / (na + nb);
                d[a][x] = v;
                d[x][a] = v;
            }
        }
        let moved = std::mem::take(&mut groups[b]);
        groups[a].extend(moved);
        alive[b] = false;
        count -= 1;
    }
    groups.into_iter().enumerate().filter(|(i, _)| alive[*i]).map(|(_, g)| g).collect()
}

/// Read every preset under `dir`, group near-identical looks and return about
/// `target` averaged presets. `progress` is called with (done, total).
pub fn condense(dir: &Path, target: usize) -> Result<(Vec<Preset>, usize), String> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                // Camera profiles are camera-specific colour tables, not looks.
                if p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("CameraProfiles")) {
                    continue;
                }
                stack.push(p);
            } else if p.extension().is_some_and(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                e == "xmp" || e == "lrtemplate" || e == "cube"
            }) {
                files.push(p);
            }
        }
    }
    let total = files.len();
    let ch = chart();
    let mut items: Vec<Item> = files
        .par_iter()
        .filter_map(|p| {
            let dev = parse_file(p)?;
            let stem = p.file_stem()?.to_string_lossy().replace(".cube", "");
            let name = clean_name(&stem);
            let category = categorise(&name, &p.to_string_lossy(), &dev);
            let fp = fingerprint(&dev, &ch);
            Some(Item { name, category, dev, fp })
        })
        .collect();
    // The same preset often ships as both .xmp and .lrtemplate.
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    items.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name) && dist(&a.fp, &b.fp) < 0.01);
    // Drop looks that barely change anything.
    let ident = fingerprint(&Develop::default(), &ch);
    items.retain(|it| dist(&it.fp, &ident) > 0.004);
    let looks = items.len();
    if looks == 0 {
        return Ok((Vec::new(), total));
    }

    // Share the target across categories by sqrt(size), at least one each.
    let mut cats: Vec<&'static str> = items.iter().map(|i| i.category).collect();
    cats.sort_unstable();
    cats.dedup();
    let weights: Vec<f32> = cats.iter().map(|c| (items.iter().filter(|i| i.category == *c).count() as f32).sqrt()).collect();
    let wsum: f32 = weights.iter().sum();
    let mut out = Vec::new();
    for (ci, cat) in cats.iter().enumerate() {
        let members: Vec<&Item> = items.iter().filter(|i| i.category == *cat).collect();
        let k = ((target as f32 * weights[ci] / wsum).round() as usize).clamp(1, members.len());
        let fps: Vec<&[f32]> = members.iter().map(|m| m.fp.as_slice()).collect();
        let groups = cluster(&fps, k);
        for g in groups {
            let devs: Vec<&Develop> = g.iter().map(|&i| &members[i].dev).collect();
            let avg = average(&devs);
            // Name after the member closest to the group's average look.
            let afp = fingerprint(&avg, &ch);
            let rep = g
                .iter()
                .min_by(|&&a, &&b| dist(&members[a].fp, &afp).total_cmp(&dist(&members[b].fp, &afp)))
                .map(|&i| members[i].name.clone())
                .unwrap_or_else(|| "Look".into());
            let mut names: Vec<String> = g.iter().map(|&i| members[i].name.clone()).collect();
            names.sort();
            names.dedup();
            let note = if names.len() == 1 {
                format!("From \"{}\"", names[0])
            } else {
                let shown: Vec<&str> = names.iter().take(6).map(String::as_str).collect();
                let more = if names.len() > 6 { format!(" and {} more", names.len() - 6) } else { String::new() };
                format!("Average of {} presets: {}{}", names.len(), shown.join(", "), more)
            };
            out.push(Preset { name: rep, category: (*cat).to_string(), note, dev: avg });
        }
    }
    // Unique names.
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for p in &mut out {
        let c = seen.entry(p.name.to_lowercase()).or_insert(0);
        *c += 1;
        if *c > 1 {
            p.name = format!("{} {}", p.name, c);
        }
    }
    out.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    Ok((out, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_clean() {
        assert_eq!(clean_name("~~A. Fuji 160ns - Noritsu"), "Fuji 160ns - Noritsu");
        assert_eq!(clean_name("02a. Soft Light (Shade)"), "Soft Light (Shade)");
        assert_eq!(clean_name("FP 18"), "FP 18");
    }

    #[test]
    fn categories() {
        let d = Develop::default();
        assert_eq!(categorise("JV - Fuji 400h - Frontier", "", &d), "Fuji");
        assert_eq!(categorise("Portra 400", "", &d), "Kodak");
        assert_eq!(categorise("HP5 Plus", "", &d), "Ilford");
        let g = Develop { grayscale: true, ..Develop::default() };
        assert_eq!(categorise("Moody", "", &g), "Black & White");
    }

    #[test]
    fn clustering_merges_near_duplicates() {
        let a = Develop { exposure: 0.3, ..Develop::default() };
        let b = Develop { exposure: 0.32, ..Develop::default() };
        let c = Develop { grayscale: true, ..Develop::default() };
        let ch = chart();
        let f: Vec<Vec<f32>> = [&a, &b, &c].iter().map(|d| fingerprint(d, &ch)).collect();
        let refs: Vec<&[f32]> = f.iter().map(|v| v.as_slice()).collect();
        let g = cluster(&refs, 2);
        assert_eq!(g.len(), 2);
        assert!(g.iter().any(|g| g.len() == 2 && g.contains(&0) && g.contains(&1)));
    }

    #[test]
    fn builtin_presets_load() {
        let b = builtin();
        assert!(b.len() >= 20, "{}", b.len());
    }
}
