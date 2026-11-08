//! The viewer: browser grid (Bridge), canvas (PureRef), colour + study panel.

use crate::batch::{BatchJob, OutFormat, Rotate};
use crate::color::{describe, hex, lightness, rgb_to_hsv};
use crate::filters::{Mode, NumberLabel};
use crate::loader::{list_images, Analysis, Loaded, Msg, Workers};
use crate::palette::{Recipe, Solver};
use crate::settings::{Grid, Settings};
use crate::theme;
use egui::{
    pos2, vec2, Align, Align2, Color32, ColorImage, FontId, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense,
    Stroke, TextureFilter, TextureHandle, TextureOptions, Ui, Vec2,
};
use image::RgbaImage;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

const THUMB_DECODE: u32 = 256;
const MAX_THUMBS: usize = 700;

enum Thumb {
    Pending,
    Ready(TextureHandle, u64),
    Failed,
}

struct Current {
    path: PathBuf,
    full: Arc<RgbaImage>,
    work: Arc<RgbaImage>,
    tex: TextureHandle,
    size: Vec2,
}

struct StudyTex {
    tex: TextureHandle,
    labels: Vec<NumberLabel>,
    work_size: Vec2,
    image: RgbaImage,
}

struct BatchUi {
    job: BatchJob,
    progress: Arc<AtomicUsize>,
    running: bool,
    total: usize,
    result: Option<Result<String, String>>,
}

pub struct App {
    workers: Workers,
    rx: Receiver<Msg>,
    s: Settings,

    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    current: Option<usize>,
    selection: BTreeSet<usize>,
    anchor: Option<usize>,
    thumbs: HashMap<PathBuf, Thumb>,
    frame_no: u64,
    scroll_to_current: bool,

    image_gen: u64,
    loading: Option<PathBuf>,
    error: Option<String>,
    img: Option<Current>,
    cache: Vec<Loaded>,

    zoom: f32,
    pan: Vec2,
    fit: bool,
    mirror: bool,

    analysis: Option<Analysis>,
    analysing: bool,
    study_gen: u64,
    study: Option<StudyTex>,
    study_dirty: bool,
    studying: bool,
    isolate: Option<usize>,
    pinned: Vec<([u8; 3], Recipe)>,
    hover: Option<[u8; 3]>,
    pick_mode: bool,

    rename: Option<(PathBuf, String)>,
    confirm_delete: Option<Vec<PathBuf>>,
    batch: Option<BatchUi>,
    toast: Option<(String, f64)>,
    show_help: bool,
    fullscreen: bool,
    on_top: bool,
    palette_note: Option<String>,
    max_tex: usize,
    ctx_handle: egui::Context,
}

fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn tex_options() -> TextureOptions {
    TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<PathBuf>) -> Result<Self, String> {
        theme::apply(&cc.egui_ctx);
        let (palette, palette_note) = crate::settings::load_palette();
        let (workers, rx) = Workers::new(cc.egui_ctx.clone(), Solver::new(palette))?;
        let s = Settings::load();
        let mut app = App {
            workers,
            rx,
            s,
            folder: None,
            files: Vec::new(),
            current: None,
            selection: BTreeSet::new(),
            anchor: None,
            thumbs: HashMap::new(),
            frame_no: 0,
            scroll_to_current: false,
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
            study_gen: 0,
            study: None,
            study_dirty: false,
            studying: false,
            isolate: None,
            pinned: Vec::new(),
            hover: None,
            pick_mode: false,
            rename: None,
            confirm_delete: None,
            batch: None,
            toast: None,
            show_help: false,
            fullscreen: false,
            on_top: false,
            palette_note,
            max_tex: 8192,
            ctx_handle: cc.egui_ctx.clone(),
        };
        // Opening a file from the file manager: go straight to it.
        if let Some(p) = open.into_iter().next() {
            app.open_path(&p);
        } else if let Some(f) = app.s.last_folder.clone().filter(|f| f.is_dir()) {
            app.open_path(&f);
        }
        Ok(app)
    }

    // ---------------------------------------------------------------- files

    fn open_path(&mut self, p: &Path) {
        if p.is_dir() {
            self.set_folder(p.to_path_buf());
            if !self.files.is_empty() {
                self.go(0);
            } else {
                self.current = None;
                self.img = None;
            }
        } else {
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
            self.set_folder(dir);
            let idx = match self.files.iter().position(|f| f == p) {
                Some(i) => i,
                None => {
                    self.files.push(p.to_path_buf());
                    self.files.len() - 1
                }
            };
            self.go(idx);
        }
    }

    fn set_folder(&mut self, dir: PathBuf) {
        if self.folder.as_ref() != Some(&dir) {
            self.thumbs.clear();
        }
        self.files = list_images(&dir);
        self.selection.clear();
        self.s.last_folder = Some(dir.clone());
        self.folder = Some(dir);
    }

    fn rescan(&mut self) {
        let keep = self.current.and_then(|i| self.files.get(i).cloned());
        if let Some(d) = self.folder.clone() {
            self.files = list_images(&d);
        }
        self.selection.clear();
        self.thumbs.clear();
        self.cache.clear();
        self.current = keep.and_then(|k| self.files.iter().position(|f| *f == k));
        if self.current.is_none() && !self.files.is_empty() {
            self.go(0);
        } else if let Some(i) = self.current {
            self.go(i);
        } else {
            self.img = None;
        }
    }

    fn go(&mut self, idx: usize) {
        let Some(path) = self.files.get(idx).cloned() else { return };
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
        if let Some(pos) = self.cache.iter().position(|c| c.path == path) {
            let loaded = self.cache.remove(pos);
            self.cache.push(loaded);
            if let Some(l) = self.cache.pop() {
                self.adopt(l);
            }
        } else {
            self.loading = Some(path.clone());
            self.workers.load_full(self.image_gen, path, self.max_tex);
        }
    }

    fn step(&mut self, d: isize) {
        if self.files.is_empty() {
            return;
        }
        let n = self.files.len() as isize;
        let i = self.current.map(|c| c as isize).unwrap_or(0);
        let j = (i + d).rem_euclid(n) as usize;
        self.go(j);
    }

    fn adopt(&mut self, l: Loaded) {
        let ctx = self.ctx_handle.clone();
        let tex = ctx.load_texture("image", to_color_image(&l.display), tex_options());
        let size = vec2(l.full.width() as f32, l.full.height() as f32);
        self.analysis = None;
        self.analysing = true;
        self.workers.analyse(self.image_gen, l.work.clone(), self.s.colours);
        self.study = None;
        self.study_dirty = true;
        self.loading = None;
        self.fit = true;
        self.img = Some(Current { path: l.path, full: l.full, work: l.work, tex, size });
        self.prefetch_neighbours();
    }

    fn prefetch_neighbours(&mut self) {
        let Some(i) = self.current else { return };
        let n = self.files.len();
        if n < 2 {
            return;
        }
        for j in [(i + 1) % n, (i + n - 1) % n] {
            if let Some(p) = self.files.get(j)
                && !self.cache.iter().any(|c| c.path == *p)
            {
                self.workers.load_full(0, p.clone(), self.max_tex);
            }
        }
    }

    fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toast = Some((msg.into(), ctx.input(|i| i.time) + 3.5));
    }

    // ---------------------------------------------------------------- messages

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Full { generation, result } => match result {
                    Ok(l) if generation == self.image_gen => self.adopt(l),
                    Ok(l) => {
                        // Prefetch (generation 0) or a stale load: keep it in the small cache.
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
                        Ok(img) => Thumb::Ready(
                            ctx.load_texture(format!("t:{}", path.display()), to_color_image(&img), tex_options()),
                            self.frame_no,
                        ),
                        Err(_) => Thumb::Failed,
                    };
                    self.thumbs.insert(path, t);
                }
                Msg::Analysis(a) => {
                    if a.image_gen == self.image_gen {
                        self.analysing = false;
                        if a.k == self.s.colours {
                            self.analysis = Some(a);
                            self.study_dirty = true;
                        } else if let Some(img) = &self.img {
                            self.analysing = true;
                            self.workers.analyse(self.image_gen, img.work.clone(), self.s.colours);
                        }
                    }
                }
                Msg::Study(st) => {
                    self.studying = false;
                    if st.generation == self.study_gen {
                        let tex = ctx.load_texture("study", to_color_image(&st.image), tex_options());
                        let work_size = vec2(st.image.width() as f32, st.image.height() as f32);
                        self.study = Some(StudyTex { tex, labels: st.labels, work_size, image: st.image });
                    }
                }
                Msg::Pinned { rgb, recipe } => {
                    self.pinned.insert(0, (rgb, recipe));
                    self.pinned.truncate(8);
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
                    self.rescan();
                }
            }
        }
    }

    fn needs_swatches(mode: Mode) -> bool {
        matches!(mode, Mode::Simplify | Mode::PaintByNumber)
    }

    fn drive_study(&mut self) {
        if !self.study_dirty || self.studying {
            return;
        }
        let Some(img) = &self.img else { return };
        let mode = self.s.mode;
        if mode == Mode::Original && self.isolate.is_none() && self.s.params.squint == 0 {
            self.study = None;
            self.study_dirty = false;
            return;
        }
        let sw = self.analysis.as_ref().map(|a| a.swatches.clone()).unwrap_or_default();
        if (Self::needs_swatches(mode) || self.isolate.is_some()) && sw.is_empty() {
            return; // wait for the palette
        }
        self.study_gen += 1;
        self.studying = true;
        self.study_dirty = false;
        self.workers.study(self.study_gen, img.work.clone(), mode, self.s.params.clone(), sw, self.isolate);
    }

    // ---------------------------------------------------------------- actions

    fn set_mode(&mut self, m: Mode) {
        if self.s.mode != m {
            self.s.mode = m;
            self.study_dirty = true;
            if m == Mode::Original && self.isolate.is_none() && self.s.params.squint == 0 {
                self.study = None;
            }
        }
    }

    fn set_colours(&mut self, k: usize) {
        self.s.colours = k.clamp(2, 24);
        self.isolate = None;
        if let Some(img) = &self.img
            && !self.analysing
        {
            self.analysing = true;
            self.workers.analyse(self.image_gen, img.work.clone(), self.s.colours);
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.selection.iter().filter_map(|i| self.files.get(*i).cloned()).collect();
        if v.is_empty()
            && let Some(p) = self.current.and_then(|i| self.files.get(i))
        {
            v.push(p.clone());
        }
        v
    }

    fn open_batch(&mut self, all: bool) {
        let files = if all { self.files.clone() } else { self.selected_paths() };
        if files.is_empty() {
            return;
        }
        let total = files.len();
        let job = BatchJob { files, ..BatchJob::default() };
        self.batch = Some(BatchUi { job, progress: Arc::new(AtomicUsize::new(0)), running: false, total, result: None });
    }

    fn quick_rotate(&mut self, ctx: &egui::Context, r: Rotate) {
        let files = self.selected_paths();
        if files.is_empty() {
            return;
        }
        let job = BatchJob { files: files.clone(), rotate: r, jpeg_quality: 95, ..BatchJob::default() };
        let progress = Arc::new(AtomicUsize::new(0));
        self.workers.batch(job, progress);
        self.toast(ctx, format!("Rotating {} file(s)…", files.len()));
    }

    fn export_study(&mut self, ctx: &egui::Context) {
        let Some(img) = &self.img else { return };
        let stem = img.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("study".into());
        let mode = self.s.mode.label().split_whitespace().next().unwrap_or("view").to_lowercase();
        let Some(dest) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}_{mode}.png"))
            .add_filter("PNG", &["png"])
            .set_directory(img.path.parent().unwrap_or(Path::new(".")))
            .save_file()
        else {
            return;
        };
        let (w, h) = img.full.dimensions();
        let out = match &self.study {
            Some(st) => crate::export::study_full_size(&st.image, &st.labels, w, h),
            None => (*img.full).clone(),
        };
        let msg = match out.save(&dest) {
            Ok(()) => format!("Saved {}", file_name(&dest)),
            Err(e) => format!("Export failed: {e}"),
        };
        self.toast(ctx, msg);
    }

    fn export_palette(&mut self, ctx: &egui::Context) {
        let (Some(img), Some(a)) = (&self.img, &self.analysis) else { return };
        let stem = img.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("palette".into());
        let Some(dest) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}_palette.png"))
            .add_filter("PNG", &["png"])
            .set_directory(img.path.parent().unwrap_or(Path::new(".")))
            .save_file()
        else {
            return;
        };
        let solver = self.workers.solver();
        let card = crate::export::palette_card(&a.swatches, &a.recipes);
        let text = crate::export::recipes_text(&a.swatches, &a.recipes, &solver, &file_name(&img.path));
        let r = card
            .save(&dest)
            .map_err(|e| e.to_string())
            .and_then(|_| std::fs::write(dest.with_extension("txt"), text).map_err(|e| e.to_string()));
        let msg = match r {
            Ok(()) => format!("Saved {} + recipes .txt", file_name(&dest)),
            Err(e) => format!("Export failed: {e}"),
        };
        self.toast(ctx, msg);
    }

    fn reload_palette(&mut self, ctx: &egui::Context) {
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

    // ---------------------------------------------------------------- keyboard

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.rename.is_some() {
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
        if pressed(Key::ArrowRight, none) || pressed(Key::Space, none) || pressed(Key::PageDown, none) {
            self.step(1);
        }
        if pressed(Key::ArrowLeft, none) || pressed(Key::Backspace, none) || pressed(Key::PageUp, none) {
            self.step(-1);
        }
        if pressed(Key::Home, none) {
            self.go(0);
        }
        if pressed(Key::End, none) && !self.files.is_empty() {
            self.go(self.files.len() - 1);
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
            let any = self.s.show_browser || self.s.show_panel;
            self.s.show_browser = !any;
            self.s.show_panel = !any;
        }
        if pressed(Key::B, none) {
            self.s.show_browser = !self.s.show_browser;
        }
        if pressed(Key::C, none) {
            self.s.show_panel = !self.s.show_panel;
        }
        if pressed(Key::G, none) {
            self.s.grid = match self.s.grid {
                Grid::Off => Grid::Thirds,
                Grid::Thirds => Grid::Quarters,
                Grid::Quarters => Grid::Custom,
                Grid::Custom => Grid::Off,
            };
        }
        if pressed(Key::T, none) {
            self.on_top = !self.on_top;
            self.apply_on_top(ctx);
        }
        if pressed(Key::M, none) {
            self.mirror = !self.mirror;
        }
        if pressed(Key::S, none) {
            self.s.split = !self.s.split;
        }
        if pressed(Key::Q, none) {
            self.s.params.squint = if self.s.params.squint == 0 { 6 } else { 0 };
            self.study_dirty = true;
        }
        if pressed(Key::I, none) || pressed(Key::P, none) {
            self.pick_mode = !self.pick_mode;
        }
        if pressed(Key::F11, none) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if pressed(Key::Escape, none) {
            if self.show_help {
                self.show_help = false;
            } else if self.pick_mode {
                self.pick_mode = false;
            } else if self.isolate.is_some() {
                self.isolate = None;
                self.study_dirty = true;
            } else if self.fullscreen {
                self.fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
        if pressed(Key::F1, none) || pressed(Key::Questionmark, Modifiers::SHIFT) || pressed(Key::Questionmark, none) {
            self.show_help = !self.show_help;
        }
        if pressed(Key::F2, none)
            && let Some(p) = self.current.and_then(|i| self.files.get(i)).cloned()
        {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.rename = Some((p, stem));
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
            self.selection = (0..self.files.len()).collect();
        }
        if pressed(Key::E, Modifiers::COMMAND) {
            self.export_study(ctx);
        }
        if pressed(Key::O, Modifiers::COMMAND.plus(Modifiers::SHIFT)) {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                self.open_path(&d);
            }
        } else if pressed(Key::O, Modifiers::COMMAND) {
            self.open_dialog();
        }
    }

    fn apply_on_top(&mut self, ctx: &egui::Context) {
        let level = if self.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        let msg = if self.on_top { "Always on top" } else { "Normal window" };
        self.toast(ctx, msg);
    }

    fn open_dialog(&mut self) {
        let exts: Vec<&str> = crate::assoc::EXTENSIONS.iter().map(|(e, _)| *e).collect();
        let mut d = rfd::FileDialog::new().add_filter("Images", &exts);
        if let Some(f) = &self.folder {
            d = d.set_directory(f);
        }
        if let Some(p) = d.pick_file() {
            self.open_path(&p);
        }
    }

    // ---------------------------------------------------------------- panels

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if ui.button("Open").on_hover_text("Open image (Ctrl+O)").clicked() {
                self.open_dialog();
            }
            if ui.button("Folder").on_hover_text("Open folder (Ctrl+Shift+O)").clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                self.open_path(&d);
            }
            if let Some(f) = self.folder.clone() {
                if ui.small_button("Up").on_hover_text("Parent folder").clicked()
                    && let Some(p) = f.parent()
                {
                    let p = p.to_path_buf();
                    self.open_path(&p);
                }
                ui.label(RichText::new(f.display().to_string()).color(theme::DIM).small());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.selectable_label(self.show_help, "?").on_hover_text("Shortcuts (F1)").clicked() {
                    self.show_help = !self.show_help;
                }
                ui.toggle_value(&mut self.s.show_panel, "Colour").on_hover_text("Colour & study panel (C)");
                ui.toggle_value(&mut self.s.show_browser, "Browse").on_hover_text("Thumbnail browser (B)");
                ui.separator();
                if ui.selectable_label(self.pick_mode, "Pipette").on_hover_text("Sample colours (I)").clicked() {
                    self.pick_mode = !self.pick_mode;
                }
                if ui.selectable_label(self.s.split, "Split").on_hover_text("Compare original / study (S)").clicked() {
                    self.s.split = !self.s.split;
                }
                ui.separator();
                let mut mode = self.s.mode;
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(mode.label())
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for m in Mode::ALL {
                            ui.selectable_value(&mut mode, m, format!("{}   {}", m.key(), m.label()));
                        }
                    });
                self.set_mode(mode);
                ui.label(RichText::new("View").color(theme::DIM));
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if let Some(img) = &self.img {
                ui.label(file_name(&img.path));
                ui.label(RichText::new(format!("{}×{}", img.size.x, img.size.y)).color(theme::DIM));
                ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).color(theme::DIM));
                if let Ok(md) = std::fs::metadata(&img.path) {
                    ui.label(RichText::new(format!("{:.1} MB", md.len() as f64 / 1_048_576.0)).color(theme::DIM));
                }
            } else if let Some(p) = &self.loading {
                ui.label(format!("Loading {}…", file_name(p)));
            }
            if let Some(i) = self.current {
                ui.label(RichText::new(format!("{} / {}", i + 1, self.files.len())).color(theme::DIM));
            }
            if self.selection.len() > 1 {
                ui.label(RichText::new(format!("{} selected", self.selection.len())).color(theme::ACCENT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(c) = self.hover {
                    let hsv = rgb_to_hsv(c);
                    ui.label(
                        RichText::new(format!(
                            "{}   H {:.0}° S {:.0}% V {:.0}%   value {:.1}",
                            hex(c),
                            hsv[0],
                            hsv[1],
                            hsv[2],
                            lightness(c) / 10.0
                        ))
                        .monospace(),
                    );
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, color32(c));
                }
                if self.studying || self.analysing {
                    ui.spinner();
                }
            });
        });
    }

    fn browser(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} images", self.files.len())).color(theme::DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.s.thumb_size, 64.0..=240.0).show_value(false))
                    .on_hover_text("Thumbnail size");
            });
        });
        ui.separator();
        let t = self.s.thumb_size;
        let cell = vec2(t + 10.0, t + 26.0);
        let avail = ui.available_width();
        let cols = ((avail / cell.x).floor() as usize).max(1);
        let n = self.files.len();
        let rows = n.div_ceil(cols);
        let mut clicked: Option<(usize, Modifiers, bool)> = None;
        let mut menu_action: Option<(usize, &'static str)> = None;
        let frame = self.frame_no;
        let scroll_to = if self.scroll_to_current { self.current } else { None };
        let mut did_scroll = false;
        let mut want: Vec<PathBuf> = Vec::new();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, cell.y, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    for c in 0..cols {
                        let i = r * cols + c;
                        let Some(path) = self.files.get(i) else { break };
                        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                        let selected = self.selection.contains(&i);
                        let is_cur = self.current == Some(i);
                        let p = ui.painter();
                        if selected || resp.hovered() {
                            p.rect_filled(rect.shrink(1.0), 3.0, if selected { theme::SELECT } else { theme::CARD });
                        }
                        if is_cur {
                            p.rect_stroke(rect.shrink(1.0), 3.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
                        }
                        let img_box = Rect::from_min_size(rect.min + vec2(5.0, 5.0), vec2(t, t));
                        match self.thumbs.get_mut(path) {
                            Some(Thumb::Ready(tex, seen)) => {
                                *seen = frame;
                                let ts = tex.size_vec2();
                                let s = (t / ts.x.max(ts.y)).min(1.0 * t);
                                let r2 = Rect::from_center_size(img_box.center(), ts * s);
                                p.image(tex.id(), r2, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            Some(Thumb::Failed) => {
                                p.text(img_box.center(), Align2::CENTER_CENTER, "✕", FontId::proportional(18.0), theme::DIM);
                            }
                            Some(Thumb::Pending) => {}
                            None => want.push(path.clone()),
                        }
                        let name = file_name(path);
                        let label_rect = Rect::from_min_max(pos2(rect.min.x + 4.0, img_box.max.y + 3.0), rect.max - vec2(4.0, 2.0));
                        let galley = p.layout_no_wrap(name.clone(), FontId::proportional(10.5), theme::DIM);
                        let clip = p.with_clip_rect(label_rect);
                        let x = if galley.size().x < label_rect.width() {
                            label_rect.center().x - galley.size().x / 2.0
                        } else {
                            label_rect.min.x
                        };
                        clip.galley(pos2(x, label_rect.min.y), galley, theme::DIM);
                        if resp.clicked() {
                            clicked = Some((i, ui.input(|inp| inp.modifiers), false));
                        }
                        if resp.double_clicked() {
                            clicked = Some((i, Modifiers::NONE, true));
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                menu_action = Some((i, "open"));
                            }
                            if ui.button("Batch edit selection…").clicked() {
                                menu_action = Some((i, "batch"));
                            }
                            if ui.button("Rotate right").clicked() {
                                menu_action = Some((i, "rotr"));
                            }
                            if ui.button("Rotate left").clicked() {
                                menu_action = Some((i, "rotl"));
                            }
                            if ui.button("Rename…").clicked() {
                                menu_action = Some((i, "rename"));
                            }
                            if ui.button("Show in folder").clicked() {
                                menu_action = Some((i, "reveal"));
                            }
                            ui.separator();
                            if ui.button("Move to recycle bin").clicked() {
                                menu_action = Some((i, "delete"));
                            }
                        });
                        if scroll_to == Some(i) {
                            ui.scroll_to_rect(rect, None);
                            did_scroll = true;
                        }
                    }
                });
            }
        });
        if did_scroll || scroll_to.is_none() {
            self.scroll_to_current = false;
        } else if let Some(i) = scroll_to {
            // Current row is outside the drawn range: jump the scroll area there.
            let row = i / cols;
            let _ = row;
            self.scroll_to_current = false;
        }

        for p in want {
            self.thumbs.insert(p.clone(), Thumb::Pending);
            self.workers.load_thumb(p, THUMB_DECODE);
        }
        if self.thumbs.len() > MAX_THUMBS {
            let cutoff = frame.saturating_sub(2);
            self.thumbs.retain(|_, t| match t {
                Thumb::Ready(_, seen) => *seen >= cutoff,
                _ => true,
            });
        }

        if let Some((i, mods, double)) = clicked {
            if mods.command {
                if !self.selection.insert(i) {
                    self.selection.remove(&i);
                }
                self.anchor = Some(i);
            } else if mods.shift {
                let a = self.anchor.unwrap_or(i);
                self.selection = (a.min(i)..=a.max(i)).collect();
            } else {
                self.selection.clear();
                self.selection.insert(i);
                if self.current != Some(i) {
                    self.go(i);
                }
                if double {
                    self.s.show_browser = false;
                }
            }
        }
        if let Some((i, action)) = menu_action {
            if !self.selection.contains(&i) {
                self.selection.clear();
                self.selection.insert(i);
            }
            let path = self.files.get(i).cloned();
            match (action, path) {
                ("open", _) => self.go(i),
                ("batch", _) => self.open_batch(false),
                ("rotr", _) => self.quick_rotate(ui.ctx(), Rotate::Cw90),
                ("rotl", _) => self.quick_rotate(ui.ctx(), Rotate::Ccw90),
                ("rename", Some(p)) => {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some((p, stem));
                }
                ("reveal", Some(p)) => crate::assoc::reveal(&p),
                ("delete", _) => self.confirm_delete = Some(self.selected_paths()),
                _ => {}
            }
        }
    }

    fn swatch_card(ui: &mut Ui, solver: &Solver, n: Option<usize>, sw_rgb: [u8; 3], share: Option<f32>, r: &Recipe, active: bool) -> egui::Response {
        let frame = egui::Frame::new()
            .fill(if active { theme::SELECT } else { theme::CARD })
            .stroke(if active { Stroke::new(1.0, theme::ACCENT) } else { Stroke::NONE })
            .corner_radius(4.0)
            .inner_margin(6.0);
        let mut swatch_resp = None;
        let out = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::click());
                let p = ui.painter();
                p.rect_filled(rect, 3.0, color32(sw_rgb));
                // Predicted mix in the lower-right corner for comparison.
                let mix_r = Rect::from_min_max(rect.center(), rect.max);
                p.rect_filled(mix_r, egui::CornerRadius { nw: 3, ne: 0, sw: 0, se: 3 }, color32(r.predicted));
                p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_black_alpha(80)), egui::StrokeKind::Inside);
                if let Some(n) = n {
                    let txt = if lightness(sw_rgb) > 55.0 { Color32::from_gray(20) } else { Color32::from_gray(235) };
                    p.text(rect.min + vec2(4.0, 2.0), Align2::LEFT_TOP, n.to_string(), FontId::proportional(11.0), txt);
                }
                swatch_resp = Some(resp.on_hover_text(
                    "Big square: the colour in the image.\nCorner: what the recipe mixes to.\nClick to isolate this colour on the canvas.",
                ));
                ui.vertical(|ui| {
                    let hsv = rgb_to_hsv(sw_rgb);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(hex(sw_rgb)).monospace().strong());
                        if let Some(s) = share {
                            ui.label(RichText::new(format!("{:.0}%", s * 100.0)).color(theme::DIM));
                        }
                        ui.label(RichText::new(format!("value {:.1}", lightness(sw_rgb) / 10.0)).color(theme::ACCENT));
                    });
                    ui.label(
                        RichText::new(format!("HSV {:.0}° {:.0}% {:.0}%   L* {:.0}", hsv[0], hsv[1], hsv[2], lightness(sw_rgb)))
                            .small()
                            .color(theme::DIM),
                    );
                    ui.label(RichText::new(describe(sw_rgb)).small().italics());
                });
            });
            ui.add_space(2.0);
            for line in r.describe(&solver.palette) {
                ui.label(RichText::new(line).small());
            }
            ui.label(
                RichText::new(format!("ΔE {:.1} · {}", r.delta_e, r.accuracy_word()))
                    .small()
                    .color(if r.delta_e < 5.0 { theme::DIM } else { theme::ACCENT }),
            );
        });
        let card = out.response.interact(Sense::click());
        match swatch_resp {
            Some(s) => s.union(card),
            None => card,
        }
    }

    fn colour_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let solver = self.workers.solver();
            if !self.pinned.is_empty() {
                theme::section_label(ui, "Sampled with pipette");
            }
            let mut remove = None;
            for (i, (rgb, r)) in self.pinned.iter().enumerate() {
                let resp = Self::swatch_card(ui, &solver, None, *rgb, None, r, false);
                resp.context_menu(|ui| {
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                    if ui.button("Copy hex").clicked() {
                        ui.ctx().copy_text(hex(*rgb));
                    }
                });
                ui.add_space(3.0);
            }
            if let Some(i) = remove {
                self.pinned.remove(i);
            }

            theme::section_label(ui, "Colours in this image");
            ui.horizontal(|ui| {
                let mut k = self.s.colours;
                ui.label("Count");
                if ui.add(egui::Slider::new(&mut k, 2..=24)).changed() {
                    self.set_colours(k);
                }
                if self.isolate.is_some() && ui.small_button("Show all").clicked() {
                    self.isolate = None;
                    self.study_dirty = true;
                }
            });
            let mut toggle: Option<usize> = None;
            match &self.analysis {
                Some(a) => {
                    for (i, (sw, r)) in a.swatches.iter().zip(&a.recipes).enumerate() {
                        let resp = Self::swatch_card(ui, &solver, Some(i + 1), sw.rgb, Some(sw.share), r, self.isolate == Some(i));
                        if resp.clicked() {
                            toggle = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copy hex").clicked() {
                                ui.ctx().copy_text(hex(sw.rgb));
                            }
                            if ui.button("Copy recipe").clicked() {
                                ui.ctx().copy_text(r.describe(&solver.palette).join("\n"));
                            }
                        });
                        ui.add_space(3.0);
                    }
                }
                None if self.img.is_some() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Finding colours and mixes…");
                    });
                }
                None => {
                    ui.label(RichText::new("Open an image to analyse it.").color(theme::DIM));
                }
            }
            if self.pinned.is_empty() && self.img.is_some() {
                ui.label(
                    RichText::new("Tip: press I (or Ctrl+click) on the image to sample any spot and get its mix.")
                        .small()
                        .color(theme::DIM),
                );
            }
            if let Some(i) = toggle {
                self.isolate = if self.isolate == Some(i) { None } else { Some(i) };
                self.study_dirty = true;
            }

            theme::section_label(ui, "Study view");
            self.study_controls(ui);

            theme::section_label(ui, "Value scale (0 black – 10 white)");
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
            for v in 0..=10 {
                let w = rect.width() / 11.0;
                let r = Rect::from_min_size(pos2(rect.min.x + v as f32 * w, rect.min.y), vec2(w, rect.height()));
                let g = crate::color::to_srgb(crate::color::lstar_to_y(v as f32 * 10.0));
                ui.painter().rect_filled(r, 0.0, Color32::from_gray(g));
                let tc = if v > 5 { Color32::from_gray(30) } else { Color32::from_gray(220) };
                ui.painter().text(r.center(), Align2::CENTER_CENTER, v.to_string(), FontId::proportional(10.0), tc);
            }
            if let Some(c) = self.hover {
                let v = lightness(c) / 100.0;
                let x = rect.min.x + (v * 10.0 + 0.5) / 11.0 * rect.width();
                ui.painter().line_segment(
                    [pos2(x, rect.min.y - 3.0), pos2(x, rect.max.y + 3.0)],
                    Stroke::new(2.0, theme::ACCENT),
                );
            }

            theme::section_label(ui, "Mixing palette");
            ui.label(RichText::new(&solver.palette.name).strong());
            ui.horizontal_wrapped(|ui| {
                for p in &solver.palette.paints {
                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, color32(p.rgb));
                    resp.on_hover_text(&p.name);
                }
            });
            if let Some(n) = &self.palette_note {
                ui.label(RichText::new(n).small().color(theme::ACCENT));
            }
            ui.horizontal(|ui| {
                if ui.button("Edit palette…").on_hover_text("Opens palette.json — change paints or calibrate colours").clicked() {
                    match crate::settings::ensure_palette_file() {
                        Ok(p) => crate::assoc::open_external(&p),
                        Err(e) => self.toast(&ctx, e),
                    }
                }
                if ui.button("Reload").clicked() {
                    self.reload_palette(&ctx);
                }
            });
            ui.label(
                RichText::new(
                    "Recipes use a Kubelka–Munk mixing model. Treat them as the starting pile, then adjust by eye.",
                )
                .small()
                .color(theme::DIM),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Export palette card").clicked() {
                    self.export_palette(&ctx);
                }
                if ui.button("Export study view").clicked() {
                    self.export_study(&ctx);
                }
            });
            ui.add_space(12.0);
        });
    }

    fn study_controls(&mut self, ui: &mut Ui) {
        let before = self.s.params.clone();
        let p = &mut self.s.params;
        match self.s.mode {
            Mode::Original => {
                ui.label(RichText::new("Pick a view with the View menu or keys 1–8.").small().color(theme::DIM));
            }
            Mode::Grayscale => {
                ui.label(RichText::new("Perceptual lightness (CIE L*), not a plain desaturate.").small().color(theme::DIM));
            }
            Mode::Values => {
                ui.add(egui::Slider::new(&mut p.value_steps, 2..=10).text("value steps"));
            }
            Mode::Notan => {
                ui.add(egui::Slider::new(&mut p.notan_threshold, 10.0..=90.0).text("split at L*"));
            }
            Mode::Grisaille => {
                ui.add(egui::Slider::new(&mut p.grisaille_steps, 0..=9).text("value steps (0 = smooth)"));
                ui.add(egui::Slider::new(&mut p.grisaille_temp, 0.0..=1.0).text("umber ← → ultramarine"));
            }
            Mode::Simplify => {
                ui.add(egui::Slider::new(&mut p.simplify_smooth, 0..=12).text("smoothing"));
                ui.label(RichText::new("Fewer colours = bigger, blockier masses.").small().color(theme::DIM));
            }
            Mode::Mosaic => {
                ui.add(egui::Slider::new(&mut p.mosaic_block, 4..=96).text("block size"));
            }
            Mode::PaintByNumber => {
                ui.add(egui::Slider::new(&mut p.pbn_min_area, 20..=2000).logarithmic(true).text("smallest region"));
                ui.checkbox(&mut p.pbn_fill, "Fill regions with colour");
            }
            Mode::LineArt => {
                ui.add(egui::Slider::new(&mut p.line_detail, 1..=8).text("line scale"));
                ui.add(egui::Slider::new(&mut p.line_weight, 4.0..=60.0).text("ink"));
            }
        }
        ui.add(egui::Slider::new(&mut p.squint, 0..=20).text("squint (blur)"));
        if *p != before {
            self.study_dirty = true;
        }
        ui.horizontal(|ui| {
            ui.label("Grid");
            egui::ComboBox::from_id_salt("grid")
                .selected_text(match self.s.grid {
                    Grid::Off => "Off",
                    Grid::Thirds => "Thirds",
                    Grid::Quarters => "Quarters",
                    Grid::Custom => "Squares",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.s.grid, Grid::Off, "Off");
                    ui.selectable_value(&mut self.s.grid, Grid::Thirds, "Thirds");
                    ui.selectable_value(&mut self.s.grid, Grid::Quarters, "Quarters");
                    ui.selectable_value(&mut self.s.grid, Grid::Custom, "Squares");
                });
            if self.s.grid == Grid::Custom {
                ui.add(egui::DragValue::new(&mut self.s.grid_n).range(2..=40).suffix(" across"));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.mirror, "Mirror (M)");
            ui.checkbox(&mut self.s.split, "Split compare (S)");
        });
    }

    // ---------------------------------------------------------------- canvas

    fn canvas(&mut self, ui: &mut Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        self.hover = None;

        let Some(img) = &self.img else {
            let msg = if let Some(e) = &self.error {
                e.clone()
            } else if self.loading.is_some() {
                "Loading…".into()
            } else {
                "Drop an image or folder here, or press Ctrl+O".into()
            };
            painter.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), theme::DIM);
            resp.context_menu(|ui| self.canvas_menu(ui));
            return;
        };

        // Fit / zoom / pan.
        let fit_zoom = {
            let z = ((rect.width() - 16.0) / img.size.x).min((rect.height() - 16.0) / img.size.y);
            if self.s.fit_upscale { z } else { z.min(1.0) }
        };
        if self.fit {
            self.zoom = fit_zoom.max(0.01);
            self.pan = Vec2::ZERO;
        }
        if resp.hovered() {
            let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = pinch * (scroll * 0.0018).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(ptr) = pointer
            {
                let new_zoom = (self.zoom * factor).clamp(0.02, 40.0);
                let center = rect.center() + self.pan;
                let rel = ptr - center;
                self.pan += rel - rel * (new_zoom / self.zoom);
                self.zoom = new_zoom;
                self.fit = false;
            }
        }
        if resp.dragged() && !self.pick_mode {
            self.pan += resp.drag_delta();
            self.fit = false;
        }
        if resp.double_clicked() {
            if self.fit {
                self.fit = false;
                self.zoom = 1.0;
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let center = rect.center();
                    let rel = ptr - center;
                    self.pan = -rel * (1.0 / fit_zoom.max(0.01) - 1.0);
                }
            } else {
                self.fit = true;
            }
        }

        let size = img.size * self.zoom;
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size);
        let uv = if self.mirror {
            Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0))
        } else {
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
        };

        // Map a screen point to an image pixel (accounting for mirror).
        let to_px = |pos: Pos2| -> Option<(u32, u32)> {
            if !img_rect.contains(pos) {
                return None;
            }
            let mut fx = (pos.x - img_rect.min.x) / img_rect.width();
            let fy = (pos.y - img_rect.min.y) / img_rect.height();
            if self.mirror {
                fx = 1.0 - fx;
            }
            let x = ((fx * img.size.x) as u32).min(img.full.width().saturating_sub(1));
            let y = ((fy * img.size.y) as u32).min(img.full.height().saturating_sub(1));
            Some((x, y))
        };

        // Shadow + image.
        painter.rect_filled(img_rect.translate(vec2(0.0, 3.0)).expand(2.0), 2.0, Color32::from_black_alpha(90));
        let study = self.study.as_ref().filter(|_| self.s.mode != Mode::Original || self.isolate.is_some() || self.s.params.squint > 0);
        match study {
            Some(st) if self.s.split => {
                let split_x = img_rect.min.x + img_rect.width() * self.s.split_pos.clamp(0.0, 1.0);
                let left = Rect::from_min_max(img_rect.min, pos2(split_x, img_rect.max.y)).intersect(rect);
                let right = Rect::from_min_max(pos2(split_x, img_rect.min.y), img_rect.max).intersect(rect);
                painter.with_clip_rect(left).image(img.tex.id(), img_rect, uv, Color32::WHITE);
                painter.with_clip_rect(right).image(st.tex.id(), img_rect, uv, Color32::WHITE);
                // Draggable divider.
                let handle = Rect::from_center_size(pos2(split_x, img_rect.center().y), vec2(14.0, img_rect.height()));
                let hr = ui.interact(handle.intersect(rect), ui.scope_id().with("split"), Sense::drag());
                if hr.dragged()
                    && let Some(ptr) = hr.interact_pointer_pos()
                {
                    self.s.split_pos = ((ptr.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                }
                if hr.hovered() || hr.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                painter.line_segment(
                    [pos2(split_x, img_rect.min.y.max(rect.min.y)), pos2(split_x, img_rect.max.y.min(rect.max.y))],
                    Stroke::new(1.5, theme::ACCENT),
                );
            }
            Some(st) => {
                painter.image(st.tex.id(), img_rect, uv, Color32::WHITE);
            }
            None => {
                painter.image(img.tex.id(), img_rect, uv, Color32::WHITE);
            }
        }

        // Paint-by-number labels drawn crisply at screen resolution.
        if let Some(st) = study
            && self.s.mode == Mode::PaintByNumber
        {
            let sx = img_rect.width() / st.work_size.x.max(1.0);
            let split_x = if self.s.split { img_rect.min.x + img_rect.width() * self.s.split_pos } else { f32::MIN };
            for l in &st.labels {
                let room = l.room * sx;
                let fs = (room * 0.9).min(20.0);
                if fs < 7.0 {
                    continue;
                }
                let mut x = l.x;
                if self.mirror {
                    x = st.work_size.x - x;
                }
                let pos = pos2(img_rect.min.x + x * sx, img_rect.min.y + l.y * sx);
                if pos.x < split_x || !rect.contains(pos) {
                    continue;
                }
                painter.text(pos, Align2::CENTER_CENTER, l.number.to_string(), FontId::proportional(fs), Color32::from_gray(95));
            }
        }

        // Grid overlay.
        let grid_lines: Vec<(u32, u32)> = match self.s.grid {
            Grid::Off => Vec::new(),
            Grid::Thirds => vec![(3, 3)],
            Grid::Quarters => vec![(4, 4)],
            Grid::Custom => {
                let n = self.s.grid_n.max(2);
                let rows = ((n as f32) * img.size.y / img.size.x).round().max(1.0) as u32;
                vec![(n, rows)]
            }
        };
        for (cols, rows) in grid_lines {
            let st = Stroke::new(1.0, Color32::from_rgba_unmultiplied(235, 200, 140, 150));
            for c in 1..cols {
                let x = img_rect.min.x + img_rect.width() * c as f32 / cols as f32;
                painter.line_segment([pos2(x, img_rect.min.y), pos2(x, img_rect.max.y)], st);
            }
            for r in 1..rows {
                let y = img_rect.min.y + img_rect.height() * r as f32 / rows as f32;
                painter.line_segment([pos2(img_rect.min.x, y), pos2(img_rect.max.x, y)], st);
            }
        }

        // Hover readout + pipette.
        if let Some(pos) = resp.hover_pos()
            && let Some((x, y)) = to_px(pos)
        {
            let p = img.full.get_pixel(x, y);
            let c = [p[0], p[1], p[2]];
            self.hover = Some(c);
            if self.pick_mode {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                // Loupe swatch next to the cursor.
                let r = Rect::from_min_size(pos + vec2(16.0, 16.0), vec2(34.0, 34.0));
                painter.rect_filled(r, 3.0, color32(c));
                painter.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
            }
        }
        let ctrl = ui.input(|i| i.modifiers.command);
        if resp.clicked()
            && (self.pick_mode || ctrl)
            && let Some(c) = self.hover
        {
            self.workers.pin(c);
        }

        // Toast.
        let now = ui.input(|i| i.time);
        if let Some((msg, until)) = &self.toast {
            if now < *until {
                let galley = painter.layout(msg.clone(), FontId::proportional(13.0), theme::TEXT, 520.0);
                let r = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 40.0), galley.size() + vec2(24.0, 14.0));
                painter.rect_filled(r, 5.0, Color32::from_rgba_unmultiplied(20, 20, 20, 230));
                painter.galley(r.min + vec2(12.0, 7.0), galley, theme::TEXT);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            } else {
                self.toast = None;
            }
        }

        resp.context_menu(|ui| self.canvas_menu(ui));
    }

    fn canvas_menu(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.menu_button("View", |ui| {
            for m in Mode::ALL {
                if ui.selectable_label(self.s.mode == m, format!("{}   {}", m.key(), m.label())).clicked() {
                    self.set_mode(m);
                    ui.close();
                }
            }
        });
        ui.menu_button("Overlay", |ui| {
            ui.checkbox(&mut self.s.split, "Split compare  (S)");
            ui.checkbox(&mut self.mirror, "Mirror  (M)");
            let mut squint = self.s.params.squint > 0;
            if ui.checkbox(&mut squint, "Squint  (Q)").changed() {
                self.s.params.squint = if squint { 6 } else { 0 };
                self.study_dirty = true;
            }
            ui.separator();
            ui.radio_value(&mut self.s.grid, Grid::Off, "No grid");
            ui.radio_value(&mut self.s.grid, Grid::Thirds, "Thirds");
            ui.radio_value(&mut self.s.grid, Grid::Quarters, "Quarters");
            ui.radio_value(&mut self.s.grid, Grid::Custom, "Square grid");
        });
        if ui.button("Pipette  (I)").clicked() {
            self.pick_mode = !self.pick_mode;
            ui.close();
        }
        ui.separator();
        if ui.button("Fit  (F)").clicked() {
            self.fit = true;
            ui.close();
        }
        if ui.button("Actual size  (Z)").clicked() {
            self.fit = false;
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            ui.close();
        }
        ui.checkbox(&mut self.s.fit_upscale, "Enlarge small images to fit");
        if ui.checkbox(&mut self.on_top, "Always on top  (T)").changed() {
            self.apply_on_top(&ctx);
        }
        ui.separator();
        ui.menu_button("File", |ui| {
            if ui.button("Open…  (Ctrl+O)").clicked() {
                ui.close();
                self.open_dialog();
            }
            if ui.button("Rotate right  (Ctrl+R)").clicked() {
                ui.close();
                self.quick_rotate(&ctx, Rotate::Cw90);
            }
            if ui.button("Rotate left  (Ctrl+L)").clicked() {
                ui.close();
                self.quick_rotate(&ctx, Rotate::Ccw90);
            }
            if ui.button("Rename…  (F2)").clicked() {
                ui.close();
                if let Some(p) = self.current.and_then(|i| self.files.get(i)).cloned() {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some((p, stem));
                }
            }
            if ui.button("Batch edit…  (Ctrl+B)").clicked() {
                ui.close();
                self.open_batch(false);
            }
            if ui.button("Batch edit whole folder…").clicked() {
                ui.close();
                self.open_batch(true);
            }
            if ui.button("Show in folder").clicked() {
                ui.close();
                if let Some(img) = &self.img {
                    crate::assoc::reveal(&img.path);
                }
            }
            if ui.button("Copy file path").clicked() {
                ui.close();
                if let Some(img) = &self.img {
                    ctx.copy_text(img.path.display().to_string());
                }
            }
            ui.separator();
            if ui.button("Move to recycle bin  (Del)").clicked() {
                ui.close();
                self.confirm_delete = Some(self.selected_paths());
            }
        });
        ui.menu_button("Export", |ui| {
            if ui.button("Study view as PNG  (Ctrl+E)").clicked() {
                ui.close();
                self.export_study(&ctx);
            }
            if ui.button("Palette card + recipes").clicked() {
                ui.close();
                self.export_palette(&ctx);
            }
        });
        ui.separator();
        if ui.button("Make Viewer the default image viewer").clicked() {
            ui.close();
            let msg = match crate::assoc::register() {
                Ok(m) | Err(m) => m,
            };
            self.toast(&ctx, msg);
        }
        if ui.button("Shortcuts  (F1)").clicked() {
            ui.close();
            self.show_help = true;
        }
    }

    // ---------------------------------------------------------------- dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((path, mut name)) = self.rename.take() {
            let mut keep = true;
            let mut commit = false;
            egui::Window::new("Rename")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(320.0));
                    r.request_focus();
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        commit = true;
                    }
                    if ui.input(|i| i.key_pressed(Key::Escape)) {
                        keep = false;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            commit = true;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
            if commit {
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
                let clean: String = name.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')).collect();
                let mut dest = path.with_file_name(clean.trim());
                if let Some(e) = ext {
                    dest.set_extension(e);
                }
                if clean.trim().is_empty() {
                    self.toast(ctx, "Name cannot be empty");
                } else if dest.exists() && dest != path {
                    self.toast(ctx, format!("{} already exists", file_name(&dest)));
                } else if let Err(e) = std::fs::rename(&path, &dest) {
                    self.toast(ctx, format!("Rename failed: {e}"));
                } else {
                    if let Some(i) = self.files.iter().position(|f| *f == path) {
                        self.files[i] = dest.clone();
                    }
                    if let Some(img) = &mut self.img
                        && img.path == path
                    {
                        img.path = dest.clone();
                    }
                    self.thumbs.remove(&path);
                    self.cache.retain(|c| c.path != path);
                }
            } else if keep {
                self.rename = Some((path, name));
            }
        }

        if let Some(paths) = self.confirm_delete.take() {
            let mut keep = true;
            egui::Window::new("Move to recycle bin?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if paths.len() == 1 {
                        ui.label(file_name(&paths[0]));
                    } else {
                        ui.label(format!("{} files", paths.len()));
                    }
                    ui.horizontal(|ui| {
                        let yes = ui.button("Move to recycle bin");
                        if yes.clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            let mut failed = 0;
                            for p in &paths {
                                if trash::delete(p).is_err() {
                                    failed += 1;
                                }
                            }
                            keep = false;
                            let next = self.current;
                            self.rescan();
                            if let Some(i) = next
                                && !self.files.is_empty()
                            {
                                self.go(i.min(self.files.len() - 1));
                            }
                            if failed > 0 {
                                self.toast(ctx, format!("{failed} file(s) could not be moved"));
                            }
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                });
            if keep {
                self.confirm_delete = Some(paths);
            }
        }

        if let Some(mut b) = self.batch.take() {
            let mut open = true;
            let mut run = false;
            egui::Window::new(format!("Batch edit — {} file(s)", b.total))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let j = &mut b.job;
                    ui.add_enabled_ui(!b.running, |ui| {
                        egui::Grid::new("batch").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                            ui.label("Rotate");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut j.rotate, Rotate::None, "none");
                                ui.radio_value(&mut j.rotate, Rotate::Cw90, "90° right");
                                ui.radio_value(&mut j.rotate, Rotate::R180, "180°");
                                ui.radio_value(&mut j.rotate, Rotate::Ccw90, "90° left");
                            });
                            ui.end_row();
                            ui.label("Flip");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut j.flip_h, "horizontal");
                                ui.checkbox(&mut j.flip_v, "vertical");
                            });
                            ui.end_row();
                            ui.label("Resize");
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut j.max_edge).range(0..=20000).suffix(" px"));
                                ui.label(RichText::new("long edge (0 = keep)").small().color(theme::DIM));
                            });
                            ui.end_row();
                            ui.label("Format");
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("fmt").selected_text(j.format.label()).show_ui(ui, |ui| {
                                    for f in OutFormat::ALL {
                                        ui.selectable_value(&mut j.format, f, f.label());
                                    }
                                });
                                ui.add(egui::Slider::new(&mut j.jpeg_quality, 40..=100).text("JPEG quality"));
                            });
                            ui.end_row();
                            ui.label("Rename");
                            ui.vertical(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut j.rename).hint_text("e.g. study_{n:3}  (empty = keep)"));
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("{name} {n} {n:3}   start at").small().color(theme::DIM));
                                    ui.add(egui::DragValue::new(&mut j.start_number).range(0..=999_999));
                                });
                                if !j.rename.trim().is_empty()
                                    && let Some(f) = j.files.first()
                                {
                                    let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                    ui.label(
                                        RichText::new(format!("→ {}", crate::batch::expand_name(j.rename.trim(), &stem, "", j.start_number)))
                                            .small()
                                            .color(theme::ACCENT),
                                    );
                                }
                            });
                            ui.end_row();
                            ui.label("Output");
                            ui.vertical(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut j.subfolder).hint_text("subfolder (empty = overwrite in place)"));
                                if j.subfolder.trim().is_empty() && j.changes_pixels() {
                                    ui.label(
                                        RichText::new("Originals will be replaced. JPEGs are re-encoded at the quality above.")
                                            .small()
                                            .color(theme::ACCENT),
                                    );
                                }
                            });
                            ui.end_row();
                        });
                    });
                    ui.separator();
                    if b.running {
                        let done = b.progress.load(Ordering::Relaxed);
                        ui.add(egui::ProgressBar::new(done as f32 / b.total.max(1) as f32).text(format!("{done} / {}", b.total)));
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    } else {
                        if let Some(r) = &b.result {
                            match r {
                                Ok(m) => ui.label(RichText::new(m).color(theme::ACCENT)),
                                Err(e) => ui.label(RichText::new(e).color(Color32::from_rgb(230, 110, 100))),
                            };
                        }
                        if ui.button("Run").clicked() {
                            run = true;
                        }
                    }
                });
            if run {
                b.running = true;
                b.result = None;
                b.progress.store(0, Ordering::Relaxed);
                self.workers.batch(b.job.clone(), b.progress.clone());
            }
            if open || b.running {
                self.batch = Some(b);
            }
        }

        if self.show_help {
            egui::Window::new("Shortcuts")
                .collapsible(false)
                .resizable(false)
                .open(&mut self.show_help)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    egui::Grid::new("keys").num_columns(2).spacing([18.0, 3.0]).show(ui, |ui| {
                        for (k, d) in [
                            ("← → / Space", "previous / next image"),
                            ("Wheel, double-click", "zoom, toggle fit ↔ 100%"),
                            ("Drag", "pan"),
                            ("F / Z", "fit / actual size"),
                            ("0 – 8", "views: original, B&W, values, notan, grisaille, simplify, blocks, paint-by-number, line art"),
                            ("S", "split compare (drag the divider)"),
                            ("M / G / Q", "mirror / grid / squint"),
                            ("T", "always on top (float over your painting app)"),
                            ("I, Ctrl+click", "pipette: sample a colour and get its mix"),
                            ("Tab", "hide all panels (clean canvas)"),
                            ("B / C", "browser / colour panel"),
                            ("F11", "full screen"),
                            ("Ctrl+R / Ctrl+L", "rotate file right / left"),
                            ("F2 / Del", "rename / recycle bin"),
                            ("Ctrl+B", "batch edit selection"),
                            ("Ctrl+A", "select all"),
                            ("Ctrl+E", "export study view"),
                            ("Right-click", "everything else"),
                        ] {
                            ui.label(RichText::new(k).monospace().color(theme::ACCENT));
                            ui.label(d);
                            ui.end_row();
                        }
                    });
                });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        self.max_tex = ctx.input(|i| i.max_texture_side).max(2048);
        self.pump(&ctx);

        // Drag and drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.first() {
            self.open_path(p);
        }

        self.keys(&ctx);
        self.drive_study();

        let title = match &self.img {
            Some(i) => format!("{} — Viewer", file_name(&i.path)),
            None => "Viewer".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let chrome = self.s.show_browser || self.s.show_panel;
        if chrome {
            egui::Panel::top("top")
                .exact_size(30.0)
                .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 3)))
                .show(ui, |ui| self.top_bar(ui));
            egui::Panel::bottom("status")
                .exact_size(24.0)
                .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 2)))
                .show(ui, |ui| self.status_bar(ui));
        }
        if self.s.show_browser {
            egui::Panel::left("browser")
                .resizable(true)
                .default_size(290.0)
                .size_range(140.0..=900.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
                .show(ui, |ui| self.browser(ui));
        }
        if self.s.show_panel {
            egui::Panel::right("colour")
                .resizable(true)
                .default_size(340.0)
                .size_range(260.0..=640.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(8))
                .show(ui, |ui| self.colour_panel(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
        self.dialogs(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
    }
}
