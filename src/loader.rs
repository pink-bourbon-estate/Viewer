//! Background decoding: full images and thumbnails, plus the analysis worker
//! (palette extraction, recipes, study renders). Nothing here blocks the UI.

use crate::extract::{extract, Swatch};
use crate::filters::{render, work_copy, Mode, NumberLabel, Params};
use crate::palette::{Recipe, Solver};
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Decode an image file, honouring EXIF orientation.
pub fn decode(path: &Path) -> Result<RgbaImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img.to_rgba8())
}

pub struct Loaded {
    pub path: PathBuf,
    pub full: Arc<RgbaImage>,
    pub work: Arc<RgbaImage>,
    /// Display copy, already shrunk to the GPU's maximum texture size.
    pub display: RgbaImage,
}

pub struct Analysis {
    pub image_gen: u64,
    pub k: usize,
    pub swatches: Vec<Swatch>,
    pub recipes: Vec<Recipe>,
}

pub struct Study {
    pub generation: u64,
    pub image: RgbaImage,
    pub labels: Vec<NumberLabel>,
}

pub enum Msg {
    Full { generation: u64, result: Result<Loaded, (PathBuf, String)> },
    Thumb { path: PathBuf, result: Result<RgbaImage, String> },
    Analysis(Analysis),
    Study(Study),
    Pinned { rgb: [u8; 3], recipe: Recipe },
    BatchDone(Result<String, String>),
}

#[derive(Clone)]
pub struct Workers {
    tx: Sender<Msg>,
    ctx: egui::Context,
    thumbs: Arc<rayon::ThreadPool>,
    pub solver: Arc<Mutex<Arc<Solver>>>,
}

impl Workers {
    pub fn new(ctx: egui::Context, solver: Solver) -> Result<(Self, Receiver<Msg>), String> {
        let (tx, rx) = channel();
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 6);
        let thumbs = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("thumb-{i}"))
            .build()
            .map_err(|e| e.to_string())?;
        Ok((
            Workers { tx, ctx, thumbs: Arc::new(thumbs), solver: Arc::new(Mutex::new(Arc::new(solver))) },
            rx,
        ))
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }

    pub fn solver(&self) -> Arc<Solver> {
        match self.solver.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub fn set_solver(&self, s: Solver) {
        match self.solver.lock() {
            Ok(mut g) => *g = Arc::new(s),
            Err(p) => *p.into_inner() = Arc::new(s),
        }
    }

    pub fn load_full(&self, generation: u64, path: PathBuf, max_tex: usize) {
        let me = self.clone();
        std::thread::spawn(move || {
            let result = decode(&path)
                .map(|full| {
                    let work = work_copy(&full);
                    let (w, h) = full.dimensions();
                    let long = w.max(h) as usize;
                    let display = if long > max_tex && long > 0 {
                        let s = max_tex as f32 / long as f32;
                        image::imageops::resize(
                            &full,
                            ((w as f32 * s) as u32).max(1),
                            ((h as f32 * s) as u32).max(1),
                            image::imageops::FilterType::Triangle,
                        )
                    } else {
                        full.clone()
                    };
                    Loaded { path: path.clone(), full: Arc::new(full), work: Arc::new(work), display }
                })
                .map_err(|e| (path.clone(), e));
            me.send(Msg::Full { generation, result });
        });
    }

    pub fn load_thumb(&self, path: PathBuf, size: u32) {
        let me = self.clone();
        self.thumbs.spawn(move || {
            let result = decode(&path).map(|img| {
                let (w, h) = img.dimensions();
                let s = size as f32 / w.max(h).max(1) as f32;
                if s >= 1.0 {
                    img
                } else {
                    image::imageops::thumbnail(&img, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1))
                }
            });
            me.send(Msg::Thumb { path, result });
        });
    }

    pub fn analyse(&self, image_gen: u64, work: Arc<RgbaImage>, k: usize) {
        let me = self.clone();
        std::thread::spawn(move || {
            let swatches = extract(&work, k);
            let solver = me.solver();
            let recipes = {
                use rayon::prelude::*;
                swatches.par_iter().map(|s| solver.solve(s.rgb)).collect()
            };
            me.send(Msg::Analysis(Analysis { image_gen, k, swatches, recipes }));
        });
    }

    pub fn study(&self, generation: u64, work: Arc<RgbaImage>, mode: Mode, params: Params, sw: Vec<Swatch>, iso: Option<usize>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let r = render(&work, mode, &params, &sw, iso);
            me.send(Msg::Study(Study { generation, image: r.image, labels: r.labels }));
        });
    }

    pub fn pin(&self, rgb: [u8; 3]) {
        let me = self.clone();
        std::thread::spawn(move || {
            let recipe = me.solver().solve(rgb);
            me.send(Msg::Pinned { rgb, recipe });
        });
    }

    pub fn batch(&self, job: crate::batch::BatchJob, progress: Arc<std::sync::atomic::AtomicUsize>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let r = crate::batch::run(&job, &progress);
            me.send(Msg::BatchDone(r));
        });
    }
}

/// Natural ("human") ordering: img2 < img10.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// List images in a folder, naturally sorted.
pub fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && crate::assoc::is_image(p))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| {
        let an = a.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let bn = b.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        natural_cmp(&an, &bn)
    });
    v
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering::*;
    #[test]
    fn natural() {
        assert_eq!(natural_cmp("img2.png", "img10.png"), Less);
        assert_eq!(natural_cmp("B.png", "a.png"), Greater);
        assert_eq!(natural_cmp("x007", "x7"), Equal);
    }
}
