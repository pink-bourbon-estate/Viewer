//! Background work: decoding, folder listing, thumbnails, analysis, the
//! develop + study render, preset previews, batch/export and preset import.
//! Nothing here blocks the UI thread.

use crate::develop::Develop;
use crate::extract::{extract, Swatch};
use crate::filters::{render, work_copy, Mode, NumberLabel, Params};
use crate::palette::{Recipe, Solver};
use crate::presets::Preset;
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
    pub display: RgbaImage,
}

pub struct Analysis {
    pub image_gen: u64,
    pub k: usize,
    pub swatches: Vec<Swatch>,
    pub recipes: Vec<Recipe>,
}

#[derive(Clone)]
pub struct Hist {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub l: Vec<u32>,
}

impl Hist {
    pub fn of(img: &RgbaImage) -> Hist {
        let mut h = Hist { r: vec![0; 256], g: vec![0; 256], b: vec![0; 256], l: vec![0; 256] };
        for p in img.pixels().step_by(2) {
            h.r[p[0] as usize] += 1;
            h.g[p[1] as usize] += 1;
            h.b[p[2] as usize] += 1;
            let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) as usize;
            h.l[l.min(255)] += 1;
        }
        h
    }
}

pub struct Processed {
    pub generation: u64,
    /// None = nothing to show beyond the original.
    pub image: Option<RgbaImage>,
    pub labels: Vec<NumberLabel>,
    pub hist: Hist,
}

pub struct ProcessJob {
    pub generation: u64,
    pub work: Arc<RgbaImage>,
    pub scale: f32,
    pub develop: Develop,
    pub mode: Mode,
    pub params: Params,
    pub swatches: Vec<Swatch>,
    pub isolate: Option<usize>,
}

pub enum Msg {
    Full { generation: u64, result: Result<Loaded, (PathBuf, String)> },
    Thumb { path: PathBuf, result: Result<(RgbaImage, (u32, u32)), String> },
    Listing { generation: u64, entries: Vec<crate::library::Entry> },
    Analysis(Analysis),
    Processed(Processed),
    Pinned { rgb: [u8; 3], recipe: Recipe },
    PresetThumbs { generation: u64, items: Vec<(String, RgbaImage)> },
    BatchDone(Result<String, String>),
    Imported(Result<(Vec<Preset>, usize), String>),
    UpdateChecked { manual: bool, result: Result<Option<crate::updater::Release>, String> },
    UpdateInstalled(Result<PathBuf, String>),
    Doc { generation: u64, result: Result<crate::docview::DocContent, String> },
    PdfPage { generation: u64, page: usize, scale: f32, result: Result<RgbaImage, String> },
    ModelFrame { generation: u64, img: RgbaImage },
    Mentions { path: PathBuf, list: Vec<crate::textdoc::Mention> },
    Blender(Result<PathBuf, String>),
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
        Ok((Workers { tx, ctx, thumbs: Arc::new(thumbs), solver: Arc::new(Mutex::new(Arc::new(solver))) }, rx))
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
                let t = if s >= 1.0 {
                    img
                } else {
                    image::imageops::thumbnail(&img, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1))
                };
                (t, (w, h))
            });
            me.send(Msg::Thumb { path, result });
        });
    }

    pub fn list(&self, generation: u64, dir: PathBuf, recursive: bool) {
        let me = self.clone();
        std::thread::spawn(move || {
            let entries = crate::library::list(&dir, recursive);
            me.send(Msg::Listing { generation, entries });
        });
    }

    pub fn list_paths(&self, generation: u64, paths: Vec<PathBuf>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let entries = paths
                .into_iter()
                .filter_map(|p| {
                    let md = std::fs::metadata(&p).ok()?;
                    let modified = md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    Some(crate::library::Entry { path: p, is_dir: false, size: md.len(), modified })
                })
                .collect();
            me.send(Msg::Listing { generation, entries });
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

    pub fn process(&self, job: ProcessJob) {
        let me = self.clone();
        std::thread::spawn(move || {
            let developed = if job.develop.is_identity() {
                None
            } else {
                Some(crate::develop::apply(&job.work, &job.develop, job.scale))
            };
            let base: &RgbaImage = developed.as_ref().unwrap_or(&job.work);
            let study = job.mode != Mode::Original || job.isolate.is_some() || job.params.squint > 0;
            let (image, labels) = if study {
                let r = render(base, job.mode, &job.params, &job.swatches, job.isolate);
                (Some(r.image), r.labels)
            } else {
                (developed, Vec::new())
            };
            let hist = Hist::of(image.as_ref().unwrap_or(&job.work));
            me.send(Msg::Processed(Processed { generation: job.generation, image, labels, hist }));
        });
    }

    /// Render small previews of presets on the current image.
    pub fn preset_thumbs(&self, generation: u64, work: Arc<RgbaImage>, list: Vec<Preset>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let (w, h) = work.dimensions();
            let s = 150.0 / w.max(h).max(1) as f32;
            let small = image::imageops::thumbnail(&*work, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1));
            let items = {
                use rayon::prelude::*;
                list.par_iter()
                    .map(|p| (p.name.clone(), crate::develop::apply(&small, &p.dev, s * w as f32 / 2048.0)))
                    .collect()
            };
            me.send(Msg::PresetThumbs { generation, items });
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

    /// Run `f` on a worker thread and deliver its message.
    pub fn run<F: FnOnce() -> Msg + Send + 'static>(&self, f: F) {
        let me = self.clone();
        std::thread::spawn(move || {
            let m = f();
            me.send(m);
        });
    }

    pub fn check_update(&self, manual: bool) {
        let me = self.clone();
        std::thread::spawn(move || {
            let result = crate::updater::check();
            me.send(Msg::UpdateChecked { manual, result });
        });
    }

    pub fn install_update(&self, rel: crate::updater::Release) {
        let me = self.clone();
        std::thread::spawn(move || {
            let r = crate::updater::install(&rel);
            me.send(Msg::UpdateInstalled(r));
        });
    }

    pub fn import_presets(&self, dir: PathBuf, target: usize) {
        let me = self.clone();
        std::thread::spawn(move || {
            let r = crate::presets::condense(&dir, target);
            me.send(Msg::Imported(r));
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
