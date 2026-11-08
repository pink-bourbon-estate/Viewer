//! Batch transforms — the bit of old Windows Photo Viewer everyone misses:
//! rotate, flip, resize, convert and rename many files at once.

use crate::loader::decode;
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Rotate {
    None,
    Cw90,
    R180,
    Ccw90,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutFormat {
    Keep,
    Png,
    Jpeg,
    Webp,
    Bmp,
    Tiff,
}

impl OutFormat {
    pub const ALL: [OutFormat; 6] =
        [OutFormat::Keep, OutFormat::Png, OutFormat::Jpeg, OutFormat::Webp, OutFormat::Bmp, OutFormat::Tiff];
    pub fn label(self) -> &'static str {
        match self {
            OutFormat::Keep => "Keep format",
            OutFormat::Png => "PNG",
            OutFormat::Jpeg => "JPEG",
            OutFormat::Webp => "WebP (lossless)",
            OutFormat::Bmp => "BMP",
            OutFormat::Tiff => "TIFF",
        }
    }
    fn ext(self) -> Option<&'static str> {
        match self {
            OutFormat::Keep => None,
            OutFormat::Png => Some("png"),
            OutFormat::Jpeg => Some("jpg"),
            OutFormat::Webp => Some("webp"),
            OutFormat::Bmp => Some("bmp"),
            OutFormat::Tiff => Some("tif"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BatchJob {
    #[serde(skip)]
    pub files: Vec<PathBuf>,
    pub rotate: Rotate,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Resize so the long edge is at most this many pixels (0 = keep).
    pub max_edge: u32,
    pub format: OutFormat,
    pub jpeg_quality: u8,
    /// Rename pattern; empty keeps names. Tokens: {name} {n} {n:3} {ext}
    pub rename: String,
    pub start_number: u32,
    /// Write into this subfolder instead of overwriting originals.
    pub subfolder: String,
    /// Added to the file name of each output (e.g. "_edit"); empty = none.
    #[serde(default)]
    pub suffix: String,
    /// A look to bake in (Develop panel).
    #[serde(skip)]
    pub develop: Option<crate::develop::Develop>,
    /// Crop / straighten / turns (Export panel; single image).
    #[serde(skip)]
    pub geometry: Option<crate::geom::Geometry>,
    /// Write exactly here (single file "Save as…").
    #[serde(skip)]
    pub dest: Option<PathBuf>,
}

impl Default for BatchJob {
    fn default() -> Self {
        BatchJob {
            files: Vec::new(),
            rotate: Rotate::None,
            flip_h: false,
            flip_v: false,
            max_edge: 0,
            format: OutFormat::Keep,
            jpeg_quality: 92,
            rename: String::new(),
            start_number: 1,
            subfolder: String::new(),
            suffix: String::new(),
            develop: None,
            geometry: None,
            dest: None,
        }
    }
}

impl BatchJob {
    pub fn changes_pixels(&self) -> bool {
        self.rotate != Rotate::None
            || self.flip_h
            || self.flip_v
            || self.max_edge > 0
            || self.format != OutFormat::Keep
            || self.develop.as_ref().is_some_and(|d| !d.is_identity())
            || self.geometry.as_ref().is_some_and(|g| !g.is_identity())
    }
}

/// Expand a rename pattern for file number `n`.
pub fn expand_name(pattern: &str, stem: &str, ext: &str, n: u32) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &after[..end];
        match token {
            "name" => out.push_str(stem),
            "ext" => out.push_str(ext),
            "n" => out.push_str(&n.to_string()),
            t if t.starts_with("n:") => {
                let width: usize = t[2..].parse().unwrap_or(1).min(9);
                out.push_str(&format!("{n:0width$}"));
            }
            other => {
                out.push('{');
                out.push_str(other);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    // Strip characters that are illegal in Windows file names.
    out.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')).collect()
}

fn transform(img: RgbaImage, job: &BatchJob) -> RgbaImage {
    use image::imageops;
    let img = match &job.develop {
        Some(d) if !d.is_identity() => crate::develop::apply(&img, d, 1.0),
        _ => img,
    };
    let img = match &job.geometry {
        Some(g) if !g.is_identity() => crate::geom::apply(&img, g),
        _ => img,
    };
    let mut img = match job.rotate {
        Rotate::None => img,
        Rotate::Cw90 => imageops::rotate90(&img),
        Rotate::R180 => imageops::rotate180(&img),
        Rotate::Ccw90 => imageops::rotate270(&img),
    };
    if job.flip_h {
        img = imageops::flip_horizontal(&img);
    }
    if job.flip_v {
        img = imageops::flip_vertical(&img);
    }
    let (w, h) = img.dimensions();
    if job.max_edge > 0 && w.max(h) > job.max_edge {
        let s = job.max_edge as f32 / w.max(h) as f32;
        img = imageops::resize(
            &img,
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
            imageops::FilterType::Lanczos3,
        );
    }
    img
}

pub fn save(img: &RgbaImage, path: &Path, jpeg_quality: u8) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let fmt = ImageFormat::from_extension(&ext).ok_or_else(|| format!("unknown format .{ext}"))?;
    // Write to a temp file first so a failure never destroys the original.
    let tmp = path.with_extension(format!("{ext}.viewer-tmp"));
    let res = (|| -> Result<(), String> {
        match fmt {
            ImageFormat::Jpeg => {
                let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
                let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
                let mut w = std::io::BufWriter::new(f);
                let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, jpeg_quality.clamp(1, 100));
                rgb.write_with_encoder(enc).map_err(|e| e.to_string())
            }
            ImageFormat::Bmp | ImageFormat::Pnm | ImageFormat::Ico => {
                let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
                rgb.save_with_format(&tmp, fmt).map_err(|e| e.to_string())
            }
            _ => img.save_with_format(&tmp, fmt).map_err(|e| e.to_string()),
        }
    })();
    match res {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

pub fn run(job: &BatchJob, progress: &AtomicUsize) -> Result<String, String> {
    let mut done = 0usize;
    let mut errors = Vec::new();
    let mut n = job.start_number;
    for src in &job.files {
        let res = (|| -> Result<(), String> {
            let dir = src.parent().ok_or("no parent folder")?;
            let out_dir = if job.subfolder.trim().is_empty() { dir.to_path_buf() } else { dir.join(job.subfolder.trim()) };
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let src_ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = job.format.ext().map(str::to_string).unwrap_or(src_ext.clone());
            let new_stem = if job.rename.trim().is_empty() { stem.clone() } else { expand_name(job.rename.trim(), &stem, &ext, n) };
            let dest = match &job.dest {
                Some(d) => d.clone(),
                None => out_dir.join(format!("{new_stem}{}.{ext}", job.suffix.trim())),
            };
            if dest != *src && dest.exists() {
                return Err(format!("{} already exists", dest.display()));
            }
            if job.changes_pixels() {
                let img = transform(decode(src)?, job);
                save(&img, &dest, job.jpeg_quality)?;
                if job.subfolder.trim().is_empty() && job.suffix.trim().is_empty() && job.dest.is_none() && dest != *src {
                    // Converted/renamed in place: the old file goes to the recycle bin.
                    trash::delete(src).map_err(|e| e.to_string())?;
                }
            } else if dest != *src {
                if job.subfolder.trim().is_empty() {
                    std::fs::rename(src, &dest).map_err(|e| e.to_string())?;
                } else {
                    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            errors.push(format!("{}: {e}", src.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()));
        }
        n += 1;
        done += 1;
        progress.store(done, Ordering::Relaxed);
    }
    if errors.is_empty() {
        Ok(format!("Processed {done} file(s)."))
    } else {
        Err(format!("{} of {done} failed:\n{}", errors.len(), errors.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_tokens() {
        assert_eq!(expand_name("study_{n:3}", "x", "png", 7), "study_007");
        assert_eq!(expand_name("{name}-{n}", "cat", "jpg", 12), "cat-12");
        assert_eq!(expand_name("bad{zz}", "a", "b", 1), "bad{zz}");
        assert_eq!(expand_name("a:b/{name", "s", "e", 1), "ab{name");
    }

    #[test]
    fn batch_rotate_and_rename() {
        let dir = std::env::temp_dir().join(format!("viewer-batch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("a.png");
        let img = RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255]));
        assert!(save(&img, &f, 90).is_ok());
        let job = BatchJob {
            files: vec![f.clone()],
            rotate: Rotate::Cw90,
            rename: "r_{n:2}".into(),
            subfolder: "out".into(),
            ..BatchJob::default()
        };
        let p = AtomicUsize::new(0);
        assert!(run(&job, &p).is_ok());
        let out = decode(&dir.join("out").join("r_01.png"));
        assert_eq!(out.map(|i| i.dimensions()).ok(), Some((2, 4)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
