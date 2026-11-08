//! App state, navigation, background-message handling, keyboard, and the
//! dockable layout. Panel contents live in `browse.rs` (folders, content,
//! metadata), `panels.rs` (develop, colours, composition, art views, export)
//! and `canvas.rs` (the viewer).

use crate::batch::{BatchJob, Rotate};
use crate::develop::Develop;
use crate::filters::{Mode, NumberLabel};
use crate::geom::Geometry;
use crate::library::{Entry, Library};
use crate::loader::{Analysis, Hist, Loaded, Msg, ProcessJob, Workers};
use crate::palette::{Recipe, Solver};
use crate::presets::Preset;
use crate::settings::{Center, PanelKind, Settings, SortBy, TabState, DOCK_NAMES};
use crate::theme;
use egui::{vec2, Align, Align2, Color32, ColorImage, Key, Layout, Modifiers, RichText, TextureFilter, TextureHandle, TextureOptions, Ui, Vec2};
use image::RgbaImage;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

pub const THUMB_DECODE: u32 = 256;
pub const MAX_THUMBS: usize = 900;

pub enum Thumb {
    Pending,
    Ready(TextureHandle, u64),
    Failed,
}

pub struct Current {
    pub path: PathBuf,
    pub full: Arc<RgbaImage>,
    pub work: Arc<RgbaImage>,
    pub tex: TextureHandle,
    pub size: Vec2,
}

pub struct ProcTex {
    pub tex: TextureHandle,
    pub labels: Vec<NumberLabel>,
    pub work_size: Vec2,
    pub image: RgbaImage,
}

pub struct BatchUi {
    pub job: BatchJob,
    pub progress: Arc<AtomicUsize>,
    pub running: bool,
    pub total: usize,
    pub result: Option<Result<String, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    None,
    Crop,
    Level,
}

#[derive(Clone, Copy, Debug)]
pub enum CropDrag {
    Move { start: [f32; 4], origin: egui::Pos2 },
    Corner { fixed: (f32, f32) },
    New { origin: (f32, f32) },
}

pub struct App {
    pub workers: Workers,
    rx: Receiver<Msg>,
    pub s: Settings,
    pub lib: Library,
    pub ctx_handle: egui::Context,

    // listing
    pub all: Vec<Entry>,
    pub files: Vec<Entry>,
    pub list_gen: u64,
    pub listing: bool,
    pub dims: HashMap<PathBuf, (u32, u32)>,
    pub pending_open: Option<PathBuf>,
    pub current: Option<usize>,
    pub selection: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub thumbs: HashMap<PathBuf, Thumb>,
    pub frame_no: u64,
    pub scroll_to_current: bool,
    pub grid_cols: usize,

    // image
    pub image_gen: u64,
    pub loading: Option<PathBuf>,
    pub error: Option<String>,
    pub img: Option<Current>,
    cache: Vec<Loaded>,
    pub zoom: f32,
    pub pan: Vec2,
    pub fit: bool,
    pub mirror: bool,

    // analysis & processing
    pub analysis: Option<Analysis>,
    pub analysing: bool,
    proc_gen: u64,
    pub processed: Option<ProcTex>,
    pub proc_dirty: bool,
    processing: bool,
    pub hist: Option<Hist>,
    pub isolate: Option<usize>,
    pub pinned: Vec<([u8; 3], Recipe)>,
    pub hover: Option<[u8; 3]>,
    pub pick_mode: bool,

    // develop & presets
    pub dev: Develop,
    edits: HashMap<PathBuf, (Develop, Geometry)>,
    pub geo: Geometry,
    pub presets: Vec<Preset>,
    pub user_presets: Vec<Preset>,
    pub preset_cat: Option<String>,
    pub preset_thumbs: HashMap<String, TextureHandle>,
    preset_thumb_req: Option<(u64, String)>,
    pub active_preset: Option<String>,
    pub copied_dev: Option<Develop>,
    pub curve_channel: usize,
    pub hsl_tab: usize,
    pub save_preset: Option<(String, String)>,
    pub importing: bool,

    // tools
    pub tool: Tool,
    pub crop_aspect: Option<f32>,
    pub crop_drag: Option<CropDrag>,
    pub level_line: Option<(egui::Pos2, egui::Pos2)>,

    // folders & metadata
    pub tree_children: HashMap<PathBuf, Vec<PathBuf>>,
    pub expanded: HashSet<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub exif_cache: Option<(PathBuf, Vec<(String, String)>)>,
    pub keyword_input: String,
    pub collection_input: String,

    // dialogs
    pub rename: Option<(PathBuf, String)>,
    pub confirm_delete: Option<Vec<PathBuf>>,
    pub batch: Option<BatchUi>,
    pub toast: Option<(String, f64)>,
    pub show_help: bool,
    pub fullscreen: bool,
    pub on_top: bool,
    pub palette_note: Option<String>,
    pub max_tex: usize,
}

pub fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

pub fn tex_options() -> TextureOptions {
    TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<PathBuf>) -> Result<Self, String> {
        theme::apply(&cc.egui_ctx);
        let (palette, palette_note) = crate::settings::load_palette();
        let (workers, rx) = Workers::new(cc.egui_ctx.clone(), Solver::new(palette))?;
        let mut s = Settings::load();
        if s.tabs.is_empty() {
            s.tabs.push(TabState::default());
        }
        s.active_tab = s.active_tab.min(s.tabs.len() - 1);
        let mut app = App {
            workers,
            rx,
            s,
            lib: Library::load(),
            ctx_handle: cc.egui_ctx.clone(),
            all: Vec::new(),
            files: Vec::new(),
            list_gen: 0,
            listing: false,
            dims: HashMap::new(),
            pending_open: None,
            current: None,
            selection: BTreeSet::new(),
            anchor: None,
            thumbs: HashMap::new(),
            frame_no: 0,
            scroll_to_current: false,
            grid_cols: 1,
            image_gen: 0,
            loading: None,
            error: None,
            img: None,
            cache: Vec::new(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            mirror: false,
            analysis: None,
            analysing: false,
            proc_gen: 0,
            processed: None,
            proc_dirty: false,
            processing: false,
            hist: None,
            isolate: None,
            pinned: Vec::new(),
            hover: None,
            pick_mode: false,
            dev: Develop::default(),
            edits: HashMap::new(),
            geo: Geometry::default(),
            presets: crate::presets::builtin(),
            user_presets: crate::presets::load_user(),
            preset_cat: None,
            preset_thumbs: HashMap::new(),
            preset_thumb_req: None,
            active_preset: None,
            copied_dev: None,
            curve_channel: 0,
            hsl_tab: 0,
            save_preset: None,
            importing: false,
            tool: Tool::None,
            crop_aspect: None,
            crop_drag: None,
            level_line: None,
            tree_children: HashMap::new(),
            expanded: HashSet::new(),
            roots: crate::library::roots(),
            exif_cache: None,
            keyword_input: String::new(),
            collection_input: String::new(),
            rename: None,
            confirm_delete: None,
            batch: None,
            toast: None,
            show_help: false,
            fullscreen: false,
            on_top: false,
            palette_note,
            max_tex: 8192,
        };
        if let Some(p) = open.into_iter().next() {
            // Opened from the file manager: show that image straight away.
            if p.is_file() {
                app.s.center = if app.s.center == Center::Grid { Center::Viewer } else { app.s.center };
            }
            app.open_path(&p);
        } else if let Some(f) = app.tab().folder.clone().filter(|f| f.is_dir()) {
            app.navigate(&f, false);
        } else if let Some(c) = app.tab().collection.clone() {
            app.open_collection(&c);
        }
        Ok(app)
    }

    // ---------------------------------------------------------------- tabs & navigation

    pub fn tab(&self) -> &TabState {
        let i = self.s.active_tab.min(self.s.tabs.len().saturating_sub(1));
        &self.s.tabs[i]
    }

    pub fn tab_mut(&mut self) -> &mut TabState {
        if self.s.tabs.is_empty() {
            self.s.tabs.push(TabState::default());
        }
        let i = self.s.active_tab.min(self.s.tabs.len() - 1);
        &mut self.s.tabs[i]
    }

    pub fn open_path(&mut self, p: &Path) {
        if p.is_dir() {
            self.navigate(p, true);
        } else {
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
            self.pending_open = Some(p.to_path_buf());
            if self.tab().folder.as_deref() == Some(dir.as_path()) && self.tab().collection.is_none() && !self.listing {
                self.select_pending();
            } else {
                self.navigate(&dir, true);
            }
        }
    }

    pub fn navigate(&mut self, dir: &Path, push: bool) {
        let prev = self.tab().folder.clone();
        let t = self.tab_mut();
        if push
            && let Some(p) = prev
            && p != dir
        {
            t.back.push(p);
            t.forward.clear();
        }
        t.folder = Some(dir.to_path_buf());
        t.collection = None;
        for a in dir.ancestors().skip(1) {
            self.expanded.insert(a.to_path_buf());
        }
        self.refresh_listing();
    }

    pub fn go_back(&mut self) {
        let cur = self.tab().folder.clone();
        if let Some(p) = self.tab_mut().back.pop() {
            if let Some(c) = cur {
                self.tab_mut().forward.push(c);
            }
            self.navigate(&p, false);
        }
    }

    pub fn go_forward(&mut self) {
        let cur = self.tab().folder.clone();
        if let Some(p) = self.tab_mut().forward.pop() {
            if let Some(c) = cur {
                self.tab_mut().back.push(c);
            }
            self.navigate(&p, false);
        }
    }

    pub fn go_up(&mut self) {
        if let Some(p) = self.tab().folder.clone().and_then(|f| f.parent().map(Path::to_path_buf)) {
            self.navigate(&p, true);
        }
    }

    pub fn open_collection(&mut self, name: &str) {
        let t = self.tab_mut();
        t.collection = Some(name.to_string());
        self.refresh_listing();
    }

    pub fn new_tab(&mut self) {
        let t = TabState { folder: self.tab().folder.clone(), recursive: false, ..TabState::default() };
        self.s.tabs.push(t);
        self.switch_tab(self.s.tabs.len() - 1);
    }

    pub fn close_tab(&mut self, i: usize) {
        if self.s.tabs.len() <= 1 {
            return;
        }
        self.s.tabs.remove(i);
        let a = if self.s.active_tab >= i && self.s.active_tab > 0 { self.s.active_tab - 1 } else { self.s.active_tab };
        self.switch_tab(a.min(self.s.tabs.len() - 1));
    }

    pub fn switch_tab(&mut self, i: usize) {
        self.s.active_tab = i.min(self.s.tabs.len().saturating_sub(1));
        self.refresh_listing();
    }

    pub fn refresh_listing(&mut self) {
        self.list_gen += 1;
        self.listing = true;
        self.selection.clear();
        let t = self.tab().clone();
        if let Some(c) = &t.collection {
            let paths = self.lib.collection_files(c);
            self.workers.list_paths(self.list_gen, paths);
        } else if let Some(f) = t.folder {
            self.workers.list(self.list_gen, f, t.recursive);
        } else {
            self.listing = false;
            self.all.clear();
            self.files.clear();
        }
    }

    /// Search, rating filter and sort of the raw listing into `files`.
    pub fn apply_filter(&mut self) {
        let keep_path = self.current.and_then(|i| self.files.get(i)).map(|e| e.path.clone());
        let sel_paths: Vec<PathBuf> = self.selection.iter().filter_map(|i| self.files.get(*i)).map(|e| e.path.clone()).collect();
        let q = self.tab().search.trim().to_lowercase();
        let min = self.s.min_rating;
        let lib = &self.lib;
        let mut v: Vec<Entry> = self
            .all
            .iter()
            .filter(|e| {
                if e.is_dir {
                    return q.is_empty() || file_name(&e.path).to_lowercase().contains(&q);
                }
                if min > 0 && lib.rating(&e.path) < min {
                    return false;
                }
                q.is_empty()
                    || file_name(&e.path).to_lowercase().contains(&q)
                    || lib.keywords(&e.path).iter().any(|k| k.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        let sort = self.s.sort;
        let desc = self.s.sort_desc;
        let lib = &self.lib;
        v.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                return b.is_dir.cmp(&a.is_dir);
            }
            let name = || crate::loader::natural_cmp(&file_name(&a.path), &file_name(&b.path));
            let o = match sort {
                SortBy::Name => name(),
                SortBy::Modified => a.modified.cmp(&b.modified).then_with(name),
                SortBy::Size => a.size.cmp(&b.size).then_with(name),
                SortBy::Rating => lib.rating(&a.path).cmp(&lib.rating(&b.path)).then_with(name),
                SortBy::Type => {
                    let ea = a.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    let eb = b.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    ea.cmp(&eb).then_with(name)
                }
            };
            if desc && !a.is_dir { o.reverse() } else { o }
        });
        self.files = v;
        self.current = keep_path.and_then(|p| self.files.iter().position(|e| e.path == p));
        self.selection = sel_paths.iter().filter_map(|p| self.files.iter().position(|e| e.path == *p)).collect();
        if let Some(c) = self.current {
            self.selection.insert(c);
        }
    }

    fn select_pending(&mut self) {
        if let Some(p) = self.pending_open.take() {
            match self.files.iter().position(|e| e.path == p) {
                Some(i) => self.go(i),
                None => {
                    // Not in the filtered list (e.g. rating filter): add it.
                    if let Some(e) = self.all.iter().find(|e| e.path == p).cloned() {
                        self.files.push(e);
                        let i = self.files.len() - 1;
                        self.go(i);
                    }
                }
            }
        }
    }

    pub fn image_indices(&self) -> Vec<usize> {
        self.files.iter().enumerate().filter(|(_, e)| !e.is_dir).map(|(i, _)| i).collect()
    }

    /// Open entry `idx`: folders are entered, images are loaded.
    pub fn go(&mut self, idx: usize) {
        let Some(entry) = self.files.get(idx).cloned() else { return };
        if entry.is_dir {
            self.navigate(&entry.path, true);
            return;
        }
        self.stash_edits();
        let path = entry.path;
        self.current = Some(idx);
        self.scroll_to_current = true;
        if !self.selection.contains(&idx) || self.selection.len() <= 1 {
            self.selection.clear();
            self.selection.insert(idx);
        }
        self.anchor = Some(idx);
        self.image_gen += 1;
        self.isolate = None;
        self.error = None;
        self.tool = Tool::None;
        let (d, g) = self.edits.get(&path).cloned().unwrap_or_default();
        self.dev = d;
        self.geo = g;
        self.active_preset = None;
        self.preset_thumbs.clear();
        self.preset_thumb_req = None;
        if let Some(pos) = self.cache.iter().position(|c| c.path == path) {
            let loaded = self.cache.remove(pos);
            self.adopt(loaded);
        } else {
            self.loading = Some(path.clone());
            self.workers.load_full(self.image_gen, path, self.max_tex);
        }
    }

    fn stash_edits(&mut self) {
        if let Some(c) = &self.img {
            if self.dev.is_identity() && self.geo.is_identity() {
                self.edits.remove(&c.path);
            } else {
                self.edits.insert(c.path.clone(), (self.dev.clone(), self.geo));
            }
        }
    }

    pub fn has_edits(&self, p: &Path) -> bool {
        if self.img.as_ref().is_some_and(|c| c.path == p) {
            return !self.dev.is_identity() || !self.geo.is_identity();
        }
        self.edits.contains_key(p)
    }

    pub fn step(&mut self, d: isize) {
        let imgs = self.image_indices();
        if imgs.is_empty() {
            return;
        }
        let pos = self.current.and_then(|c| imgs.iter().position(|&i| i == c));
        let n = imgs.len() as isize;
        let j = match pos {
            Some(p) => (p as isize + d).rem_euclid(n) as usize,
            None => 0,
        };
        self.go(imgs[j]);
    }

    fn adopt(&mut self, l: Loaded) {
        let tex = self.ctx_handle.load_texture("image", to_color_image(&l.display), tex_options());
        let size = vec2(l.full.width() as f32, l.full.height() as f32);
        self.dims.insert(l.path.clone(), (l.full.width(), l.full.height()));
        self.analysis = None;
        self.analysing = true;
        self.workers.analyse(self.image_gen, l.work.clone(), self.s.colours);
        self.processed = None;
        self.hist = None;
        self.proc_dirty = true;
        self.loading = None;
        self.fit = true;
        self.img = Some(Current { path: l.path, full: l.full, work: l.work, tex, size });
        self.prefetch_neighbours();
    }

    fn prefetch_neighbours(&mut self) {
        let imgs = self.image_indices();
        let Some(pos) = self.current.and_then(|c| imgs.iter().position(|&i| i == c)) else { return };
        let n = imgs.len();
        if n < 2 {
            return;
        }
        for j in [(pos + 1) % n, (pos + n - 1) % n] {
            if let Some(p) = imgs.get(j).and_then(|&i| self.files.get(i)).map(|e| e.path.clone())
                && !self.cache.iter().any(|c| c.path == p)
            {
                self.workers.load_full(0, p, self.max_tex);
            }
        }
    }

    pub fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toast = Some((msg.into(), ctx.input(|i| i.time) + 4.0));
    }

    // ---------------------------------------------------------------- messages

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Full { generation, result } => match result {
                    Ok(l) if generation == self.image_gen => self.adopt(l),
                    Ok(l) => {
                        if !self.cache.iter().any(|c| c.path == l.path) {
                            self.cache.push(l);
                            if self.cache.len() > 4 {
                                self.cache.remove(0);
                            }
                        }
                    }
                    Err((p, e)) if generation == self.image_gen => {
                        self.loading = None;
                        self.img = None;
                        self.error = Some(format!("Could not open {}: {e}", file_name(&p)));
                    }
                    Err(_) => {}
                },
                Msg::Thumb { path, result } => {
                    let t = match result {
                        Ok((img, dims)) => {
                            self.dims.insert(path.clone(), dims);
                            Thumb::Ready(
                                ctx.load_texture(format!("t:{}", path.display()), to_color_image(&img), tex_options()),
                                self.frame_no,
                            )
                        }
                        Err(_) => Thumb::Failed,
                    };
                    self.thumbs.insert(path, t);
                }
                Msg::Listing { generation, entries } => {
                    if generation == self.list_gen {
                        self.listing = false;
                        self.all = entries;
                        self.current = None;
                        self.files.clear();
                        self.apply_filter();
                        // Keep the open image selected if it is in this folder.
                        if let Some(c) = &self.img {
                            self.current = self.files.iter().position(|e| e.path == c.path);
                            if let Some(i) = self.current {
                                self.selection.insert(i);
                                self.scroll_to_current = true;
                            }
                        }
                        if self.pending_open.is_some() {
                            self.select_pending();
                        } else if self.current.is_none()
                            && self.s.center != Center::Grid
                            && let Some(&first) = self.image_indices().first()
                        {
                            self.go(first);
                        }
                    }
                }
                Msg::Analysis(a) => {
                    if a.image_gen == self.image_gen {
                        self.analysing = false;
                        if a.k == self.s.colours {
                            self.analysis = Some(a);
                            self.proc_dirty = true;
                        } else if let Some(img) = &self.img {
                            self.analysing = true;
                            self.workers.analyse(self.image_gen, img.work.clone(), self.s.colours);
                        }
                    }
                }
                Msg::Processed(p) => {
                    self.processing = false;
                    if p.generation == self.proc_gen {
                        self.hist = Some(p.hist);
                        self.processed = p.image.map(|image| {
                            let tex = ctx.load_texture("processed", to_color_image(&image), tex_options());
                            let work_size = vec2(image.width() as f32, image.height() as f32);
                            ProcTex { tex, labels: p.labels, work_size, image }
                        });
                    }
                }
                Msg::Pinned { rgb, recipe } => {
                    self.pinned.insert(0, (rgb, recipe));
                    self.pinned.truncate(12);
                    self.s.dock.reveal(PanelKind::Colours);
                }
                Msg::PresetThumbs { generation, items } => {
                    if generation == self.image_gen {
                        for (name, img) in items {
                            let tex = ctx.load_texture(format!("p:{name}"), to_color_image(&img), tex_options());
                            self.preset_thumbs.insert(name, tex);
                        }
                    }
                }
                Msg::BatchDone(r) => {
                    if let Some(b) = &mut self.batch {
                        b.running = false;
                        b.result = Some(r.clone());
                    }
                    match r {
                        Ok(m) => self.toast(ctx, m),
                        Err(e) => self.toast(ctx, e),
                    }
                    self.thumbs.clear();
                    self.cache.clear();
                    let keep = self.img.as_ref().map(|c| c.path.clone());
                    self.pending_open = keep.filter(|p| p.exists());
                    self.refresh_listing();
                }
                Msg::Imported(r) => {
                    self.importing = false;
                    match r {
                        Ok((list, total)) => {
                            let n = list.len();
                            for p in list {
                                self.user_presets.retain(|q| q.name != p.name || q.category != p.category);
                                self.user_presets.push(p);
                            }
                            let msg = match crate::presets::save_user(&self.user_presets) {
                                Ok(()) => format!(
                                    "Read {total} preset files and condensed them into {n} looks. They are saved in Viewer — the original folder is no longer needed."
                                ),
                                Err(e) => format!("Condensed {n} looks but could not save them: {e}"),
                            };
                            self.toast(ctx, msg);
                        }
                        Err(e) => self.toast(ctx, format!("Import failed: {e}")),
                    }
                }
            }
        }
    }

    fn drive_processing(&mut self) {
        if !self.proc_dirty || self.processing {
            return;
        }
        let Some(img) = &self.img else { return };
        let sw = self.analysis.as_ref().map(|a| a.swatches.clone()).unwrap_or_default();
        let needs_sw = matches!(self.s.mode, Mode::Simplify | Mode::PaintByNumber) || self.isolate.is_some();
        if needs_sw && sw.is_empty() {
            return;
        }
        self.proc_gen += 1;
        self.processing = true;
        self.proc_dirty = false;
        let scale = img.work.width() as f32 / img.full.width().max(1) as f32;
        self.workers.process(ProcessJob {
            generation: self.proc_gen,
            work: img.work.clone(),
            scale,
            develop: self.dev.clone(),
            mode: self.s.mode,
            params: self.s.params.clone(),
            swatches: sw,
            isolate: self.isolate,
        });
    }

    /// Ask for preset previews of the open category when needed.
    pub fn want_preset_thumbs(&mut self, cat: &str) {
        let Some(img) = &self.img else { return };
        if self.preset_thumb_req.as_ref().is_some_and(|(g, c)| *g == self.image_gen && c == cat) {
            return;
        }
        self.preset_thumb_req = Some((self.image_gen, cat.to_string()));
        let list: Vec<Preset> = self.all_presets().into_iter().filter(|p| p.category == cat).collect();
        self.workers.preset_thumbs(self.image_gen, img.work.clone(), list);
    }

    pub fn all_presets(&self) -> Vec<Preset> {
        let mut v: Vec<Preset> = self
            .user_presets
            .iter()
            .map(|p| {
                let mut p = p.clone();
                if p.category.is_empty() {
                    p.category = "My presets".into();
                }
                p
            })
            .collect();
        v.extend(self.presets.iter().cloned());
        v
    }

    pub fn reset_preset_req(&mut self) {
        self.preset_thumb_req = None;
    }

    pub fn is_user_preset(&self, name: &str) -> bool {
        self.user_presets.iter().any(|p| p.name == name)
    }

    pub fn dev_changed(&mut self) {
        self.proc_dirty = true;
    }

    // ---------------------------------------------------------------- shared actions

    pub fn set_mode(&mut self, m: Mode) {
        if self.s.mode != m {
            self.s.mode = m;
            self.proc_dirty = true;
        }
    }

    pub fn set_colours(&mut self, k: usize) {
        self.s.colours = k.clamp(2, 24);
        self.isolate = None;
        if let Some(img) = &self.img
            && !self.analysing
        {
            self.analysing = true;
            self.workers.analyse(self.image_gen, img.work.clone(), self.s.colours);
        }
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> =
            self.selection.iter().filter_map(|i| self.files.get(*i)).filter(|e| !e.is_dir).map(|e| e.path.clone()).collect();
        if v.is_empty()
            && let Some(p) = &self.img
        {
            v.push(p.path.clone());
        }
        v
    }

    pub fn open_batch(&mut self, all: bool) {
        let files: Vec<PathBuf> =
            if all { self.files.iter().filter(|e| !e.is_dir).map(|e| e.path.clone()).collect() } else { self.selected_paths() };
        if files.is_empty() {
            return;
        }
        let total = files.len();
        let job = BatchJob { files, ..BatchJob::default() };
        self.batch = Some(BatchUi { job, progress: Arc::new(AtomicUsize::new(0)), running: false, total, result: None });
    }

    pub fn run_job(&mut self, ctx: &egui::Context, job: BatchJob, msg: &str) {
        let n = job.files.len();
        self.workers.batch(job, Arc::new(AtomicUsize::new(0)));
        self.toast(ctx, format!("{msg} {n} file(s)…"));
    }

    pub fn quick_rotate(&mut self, ctx: &egui::Context, r: Rotate) {
        let files = self.selected_paths();
        if files.is_empty() {
            return;
        }
        let job = BatchJob { files, rotate: r, jpeg_quality: 95, ..BatchJob::default() };
        self.run_job(ctx, job, "Rotating");
    }

    pub fn set_rating(&mut self, r: u8) {
        for p in self.selected_paths() {
            self.lib.set_rating(&p, r);
        }
        if self.s.sort == SortBy::Rating || self.s.min_rating > 0 {
            self.apply_filter();
        }
    }

    pub fn reload_palette(&mut self, ctx: &egui::Context) {
        let (pal, note) = crate::settings::load_palette();
        self.palette_note = note;
        self.workers.set_solver(Solver::new(pal));
        if let Some(img) = &self.img {
            self.analysing = true;
            self.workers.analyse(self.image_gen, img.work.clone(), self.s.colours);
        }
        self.pinned.clear();
        self.toast(ctx, "Palette reloaded");
    }

    pub fn apply_on_top(&mut self, ctx: &egui::Context) {
        let level = if self.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        let msg = if self.on_top { "Always on top" } else { "Normal window" };
        self.toast(ctx, msg);
    }

    pub fn open_dialog(&mut self) {
        let exts: Vec<&str> = crate::assoc::EXTENSIONS.iter().map(|(e, _)| *e).collect();
        let mut d = rfd::FileDialog::new().add_filter("Images", &exts);
        if let Some(f) = &self.tab().folder {
            d = d.set_directory(f);
        }
        if let Some(p) = d.pick_file() {
            self.open_path(&p);
        }
    }

    pub fn toggle_split(&mut self, ctx: &egui::Context) {
        self.s.split = !self.s.split;
        self.s.split_pos = 0.5;
        if self.s.split && self.processed.is_none() {
            self.toast(ctx, "Split compares the original with a look or study view — pick one to see the split.");
        }
    }

    // ---------------------------------------------------------------- keyboard

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.rename.is_some() || self.save_preset.is_some() {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.batch.as_ref().is_some_and(|b| !b.running) {
                self.batch = None;
                return;
            }
            if self.confirm_delete.is_some() {
                self.confirm_delete = None;
                return;
            }
        }
        let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
        let none = Modifiers::NONE;
        if pressed(Key::ArrowLeft, Modifiers::ALT) {
            self.go_back();
        }
        if pressed(Key::ArrowRight, Modifiers::ALT) {
            self.go_forward();
        }
        if pressed(Key::ArrowUp, Modifiers::ALT) {
            self.go_up();
        }
        if pressed(Key::ArrowRight, none) || pressed(Key::Space, none) || pressed(Key::PageDown, none) {
            self.step(1);
        }
        if pressed(Key::ArrowLeft, none) || pressed(Key::PageUp, none) {
            self.step(-1);
        }
        if self.s.center != Center::Viewer {
            let cols = self.grid_cols.max(1) as isize;
            if pressed(Key::ArrowDown, none) {
                self.step(cols);
            }
            if pressed(Key::ArrowUp, none) {
                self.step(-cols);
            }
        }
        if pressed(Key::Backspace, none) {
            self.go_up();
        }
        if self.tool != Tool::None && pressed(Key::Enter, none) {
            self.tool = Tool::None;
            self.fit = true;
        }
        if pressed(Key::Enter, none) {
            match self.selection.iter().next().copied().and_then(|i| self.files.get(i).map(|e| (i, e.is_dir))) {
                Some((i, true)) => self.go(i),
                _ => self.s.center = if self.s.center == Center::Viewer { Center::Split } else { Center::Viewer },
            }
        }
        if pressed(Key::Home, none)
            && let Some(&i) = self.image_indices().first()
        {
            self.go(i);
        }
        if pressed(Key::End, none)
            && let Some(&i) = self.image_indices().last()
        {
            self.go(i);
        }
        for (k, r) in [(Key::Num0, 0u8), (Key::Num1, 1), (Key::Num2, 2), (Key::Num3, 3), (Key::Num4, 4), (Key::Num5, 5)] {
            if pressed(k, Modifiers::COMMAND) {
                self.set_rating(r);
            }
        }
        if pressed(Key::F, none) {
            self.fit = true;
        }
        if pressed(Key::Z, none) {
            self.fit = false;
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        let digits = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8];
        for (i, k) in digits.iter().enumerate() {
            if pressed(*k, none)
                && let Some(m) = Mode::ALL.get(i)
            {
                self.set_mode(*m);
            }
        }
        if pressed(Key::Tab, none) {
            self.s.show_panels = !self.s.show_panels;
        }
        if pressed(Key::V, none) {
            self.s.center = match self.s.center {
                Center::Grid => Center::Split,
                Center::Split => Center::Viewer,
                Center::Viewer => Center::Grid,
            };
        }
        if pressed(Key::G, none) {
            self.s.overlays.thirds = !self.s.overlays.thirds;
        }
        if pressed(Key::G, Modifiers::SHIFT) {
            self.s.overlays.golden = !self.s.overlays.golden;
        }
        if pressed(Key::M, none) {
            self.mirror = !self.mirror;
        }
        if pressed(Key::S, none) {
            self.toggle_split(ctx);
        }
        if pressed(Key::T, none) {
            self.on_top = !self.on_top;
            self.apply_on_top(ctx);
        }
        if pressed(Key::Q, none) {
            self.s.params.squint = if self.s.params.squint == 0 { 6 } else { 0 };
            self.proc_dirty = true;
        }
        if pressed(Key::I, none) || pressed(Key::P, none) {
            self.pick_mode = !self.pick_mode;
        }
        if pressed(Key::C, none) {
            self.tool = if self.tool == Tool::Crop { Tool::None } else { Tool::Crop };
            if self.tool == Tool::Crop {
                self.s.dock.reveal(PanelKind::Export);
                self.s.center = if self.s.center == Center::Grid { Center::Split } else { self.s.center };
            }
        }
        if pressed(Key::F11, none) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if pressed(Key::Escape, none) {
            if self.show_help {
                self.show_help = false;
            } else if self.tool != Tool::None {
                self.tool = Tool::None;
            } else if self.pick_mode {
                self.pick_mode = false;
            } else if self.isolate.is_some() {
                self.isolate = None;
                self.proc_dirty = true;
            } else if self.fullscreen {
                self.fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
        if pressed(Key::F1, none) || pressed(Key::Questionmark, Modifiers::SHIFT) || pressed(Key::Questionmark, none) {
            self.show_help = !self.show_help;
        }
        if pressed(Key::F2, none)
            && let Some(p) = self.selected_paths().first().cloned()
        {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.rename = Some((p, stem));
        }
        if pressed(Key::F5, none) {
            self.thumbs.clear();
            self.tree_children.clear();
            self.refresh_listing();
        }
        if pressed(Key::Delete, none) {
            let v = self.selected_paths();
            if !v.is_empty() {
                self.confirm_delete = Some(v);
            }
        }
        if pressed(Key::R, Modifiers::COMMAND) {
            self.quick_rotate(ctx, Rotate::Cw90);
        }
        if pressed(Key::L, Modifiers::COMMAND) {
            self.quick_rotate(ctx, Rotate::Ccw90);
        }
        if pressed(Key::B, Modifiers::COMMAND) {
            self.open_batch(false);
        }
        if pressed(Key::A, Modifiers::COMMAND) {
            self.selection = self.image_indices().into_iter().collect();
        }
        if pressed(Key::T, Modifiers::COMMAND) {
            self.new_tab();
        }
        if pressed(Key::W, Modifiers::COMMAND) {
            self.close_tab(self.s.active_tab);
        }
        if pressed(Key::C, Modifiers::COMMAND.plus(Modifiers::SHIFT)) {
            self.copied_dev = Some(self.dev.clone());
            self.toast(ctx, "Develop settings copied");
        }
        if pressed(Key::V, Modifiers::COMMAND.plus(Modifiers::SHIFT))
            && let Some(d) = self.copied_dev.clone()
        {
            self.dev = d;
            self.dev_changed();
        }
        if pressed(Key::E, Modifiers::COMMAND) {
            self.export_current(ctx, crate::panels::ExportTarget::Copy);
        }
        if pressed(Key::F, Modifiers::COMMAND) {
            self.s.dock.reveal(PanelKind::Folders);
        }
        if pressed(Key::O, Modifiers::COMMAND.plus(Modifiers::SHIFT)) {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                self.navigate(&d, true);
            }
        } else if pressed(Key::O, Modifiers::COMMAND) {
            self.open_dialog();
        }
    }

    // ---------------------------------------------------------------- dock

    fn panel_body(&mut self, ui: &mut Ui, p: PanelKind) {
        match p {
            PanelKind::Folders => self.panel_folders(ui),
            PanelKind::Metadata => self.panel_metadata(ui),
            PanelKind::Develop => self.panel_develop(ui),
            PanelKind::Colours => self.panel_colours(ui),
            PanelKind::Composition => self.panel_composition(ui),
            PanelKind::ArtViews => self.panel_art(ui),
            PanelKind::Export => self.panel_export(ui),
        }
    }

    fn dock_stack(&mut self, ui: &mut Ui, idx: usize) {
        let panels = self.s.dock.stacks[idx].clone();
        if panels.is_empty() {
            return;
        }
        let mut active = self.s.dock.active[idx].min(panels.len() - 1);
        let mut move_req: Option<(PanelKind, Option<usize>)> = None;
        egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, p) in panels.iter().enumerate() {
                    let text = RichText::new(p.label()).size(12.0);
                    let r = ui.selectable_label(i == active, if i == active { text.color(theme::TEXT) } else { text.color(theme::DIM) });
                    if r.clicked() {
                        active = i;
                    }
                    r.context_menu(|ui| {
                        ui.label(RichText::new("Move panel to").small().color(theme::DIM));
                        for (d, name) in DOCK_NAMES.iter().enumerate() {
                            if d != idx && ui.button(*name).clicked() {
                                move_req = Some((*p, Some(d)));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("Hide panel").clicked() {
                            move_req = Some((*p, None));
                            ui.close();
                        }
                    });
                }
            });
        });
        self.s.dock.active[idx] = active;
        let p = panels[active];
        egui::ScrollArea::vertical()
            .id_salt(("dock", idx, p.label()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 10, top: 4, bottom: 12 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    self.panel_body(ui, p);
                });
            });
        if let Some((p, to)) = move_req {
            self.s.dock.move_to(p, to);
        }
    }

    fn dock_side(&mut self, ui: &mut Ui, top: usize, bottom: usize) {
        let has_top = !self.s.dock.stacks[top].is_empty();
        let has_bottom = !self.s.dock.stacks[bottom].is_empty();
        if has_top && has_bottom {
            let h = ui.available_height();
            egui::Panel::top(("dock_split", top))
                .resizable(true)
                .default_size(h * 0.55)
                .size_range(80.0..=(h - 80.0).max(81.0))
                .frame(egui::Frame::new().fill(theme::PANEL))
                .show(ui, |ui| self.dock_stack(ui, top));
            egui::CentralPanel::no_frame().show(ui, |ui| self.dock_stack(ui, bottom));
        } else if has_top {
            self.dock_stack(ui, top);
        } else if has_bottom {
            self.dock_stack(ui, bottom);
        }
    }

    fn panels_menu(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Show / hide panels (Tab hides all)").small().color(theme::DIM));
        for p in PanelKind::ALL {
            let mut shown = self.s.dock.location(p).is_some();
            if ui.checkbox(&mut shown, p.label()).changed() {
                if shown {
                    self.s.dock.reveal(p);
                } else {
                    self.s.dock.move_to(p, None);
                }
            }
        }
        ui.separator();
        if ui.button("Reset layout").clicked() {
            self.s.dock = crate::settings::Dock::default();
            ui.close();
        }
        ui.label(RichText::new("Right-click a panel tab to move it.").small().color(theme::DIM));
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let imgs = self.files.iter().filter(|e| !e.is_dir).count();
            let dirs = self.files.len() - imgs;
            ui.label(RichText::new(format!("{imgs} images{}", if dirs > 0 { format!(", {dirs} folders") } else { String::new() })).color(theme::DIM));
            if self.selection.len() > 1 {
                ui.label(RichText::new(format!("{} selected", self.selection.len())).color(theme::ACCENT));
            }
            if let Some(img) = &self.img {
                ui.separator();
                ui.label(file_name(&img.path));
                ui.label(RichText::new(format!("{}×{}  {:.0}%", img.size.x, img.size.y, self.zoom * 100.0)).color(theme::DIM));
                if !self.dev.is_identity() || !self.geo.is_identity() {
                    ui.label(RichText::new("edited").color(theme::ACCENT));
                }
            } else if let Some(p) = &self.loading {
                ui.label(format!("Loading {}…", file_name(p)));
            }
            if self.listing {
                ui.spinner();
                ui.label(RichText::new("reading folder…").color(theme::DIM));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(c) = self.hover {
                    let hsv = crate::color::rgb_to_hsv(c);
                    ui.label(
                        RichText::new(format!(
                            "{}   H {:.0}° S {:.0}% V {:.0}%   value {:.1}",
                            crate::color::hex(c),
                            hsv[0],
                            hsv[1],
                            hsv[2],
                            crate::color::lightness(c) / 10.0
                        ))
                        .monospace(),
                    );
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, color32(c));
                }
                if self.processing || self.analysing || self.importing {
                    ui.spinner();
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        self.max_tex = ctx.input(|i| i.max_texture_side).max(2048);
        self.pump(&ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.first() {
            self.open_path(p);
        }
        self.keys(&ctx);
        self.drive_processing();
        self.lib.save_if_dirty();

        let title = match &self.img {
            Some(i) => format!("{} — Viewer", file_name(&i.path)),
            None => "Viewer".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        egui::Panel::top("toolbar")
            .exact_size(32.0)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(6, 3)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    self.toolbar(ui);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.selectable_label(self.show_help, " ? ").on_hover_text("Shortcuts (F1)").clicked() {
                            self.show_help = !self.show_help;
                        }
                        ui.menu_button("Panels", |ui| self.panels_menu(ui));
                        ui.separator();
                        for (c, label, tip) in [
                            (Center::Viewer, "Viewer", "Image only (V)"),
                            (Center::Split, "Split", "Browser + viewer side by side (V)"),
                            (Center::Grid, "Grid", "Browser only (V)"),
                        ] {
                            if ui.selectable_label(self.s.center == c, label).on_hover_text(tip).clicked() {
                                self.s.center = c;
                            }
                        }
                    });
                });
            });
        egui::Panel::top("tabs")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(6, 2)))
            .show(ui, |ui| self.tab_strip(ui));
        egui::Panel::bottom("status")
            .exact_size(22.0)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 2)))
            .show(ui, |ui| self.status_bar(ui));

        if self.s.show_panels {
            let d = &self.s.dock.stacks;
            if !d[0].is_empty() || !d[1].is_empty() {
                egui::Panel::left("dock_left")
                    .resizable(true)
                    .default_size(270.0)
                    .size_range(180.0..=700.0)
                    .frame(egui::Frame::new().fill(theme::PANEL))
                    .show(ui, |ui| self.dock_side(ui, 0, 1));
            }
            let d = &self.s.dock.stacks;
            if !d[2].is_empty() || !d[3].is_empty() {
                egui::Panel::right("dock_right")
                    .resizable(true)
                    .default_size(330.0)
                    .size_range(240.0..=700.0)
                    .frame(egui::Frame::new().fill(theme::PANEL))
                    .show(ui, |ui| self.dock_side(ui, 2, 3));
            }
        }

        egui::CentralPanel::no_frame().show(ui, |ui| match self.s.center {
            Center::Grid => self.content(ui),
            Center::Viewer => self.canvas(ui),
            Center::Split => {
                let w = ui.available_width();
                egui::Panel::left("content_split")
                    .resizable(true)
                    .default_size(w * 0.42)
                    .size_range(160.0..=(w - 200.0).max(161.0))
                    .frame(egui::Frame::new().fill(theme::CANVAS))
                    .show(ui, |ui| self.content(ui));
                egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
            }
        });
        self.dialogs(&ctx);
        let _ = Align2::CENTER_CENTER;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
        self.lib.save_if_dirty();
    }
}
