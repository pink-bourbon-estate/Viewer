//! Previews for everything that is not an image: text / code / Markdown
//! (editable), Python (editable + run), CSV (editable grid), PDF, Word,
//! spreadsheets, audio and 3D models. Loading happens on worker threads; a
//! file that cannot be read shows a message instead of taking the app down.

use crate::app::{color32, file_name, App};
use crate::audio::{fmt_time, Player};
use crate::filetypes::Kind;
use crate::loader::Msg;
use crate::model3d::{Camera, Mesh};
use crate::office::{Block, PdfInfo, Sheet};
use crate::textdoc::{Click, MdMode, TextDoc};
use crate::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, TextureHandle, Ui};
use image::RgbaImage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub enum DocContent {
    Text(TextDoc),
    Audio(crate::audio::Decoded),
    Pdf(PdfInfo),
    Docx(Vec<Block>),
    Sheets(Vec<Sheet>),
    Csv { rows: Vec<Vec<String>>, delim: char },
    Model(Mesh),
}

pub struct PdfView {
    pub info: PdfInfo,
    pub zoom: f32,
    pub pages: HashMap<usize, (TextureHandle, f32)>,
    pub requested: HashSet<(usize, u32)>,
}

pub struct CsvView {
    pub rows: Vec<Vec<String>>,
    pub delim: char,
    pub dirty: bool,
    pub edit: Option<(usize, usize, String)>,
}

pub struct ModelView {
    pub mesh: Arc<Mesh>,
    pub cam: Camera,
    pub tex: Option<TextureHandle>,
    pub rendering: bool,
    pub last: Option<(Camera, u32, u32)>,
    pub rgen: u64,
}

pub enum DocView {
    Loading(PathBuf),
    Failed(PathBuf, String),
    Text(TextDoc),
    Audio(PathBuf, Player),
    Pdf(PathBuf, PdfView),
    Docx(PathBuf, Vec<Block>, Vec<Option<TextureHandle>>),
    Sheets(PathBuf, Vec<Sheet>, usize),
    Csv(PathBuf, CsvView),
    Model(PathBuf, ModelView),
}

impl DocView {
    pub fn path(&self) -> &Path {
        match self {
            DocView::Loading(p) | DocView::Failed(p, _) | DocView::Audio(p, _) | DocView::Pdf(p, _) | DocView::Docx(p, _, _) => p,
            DocView::Sheets(p, _, _) | DocView::Csv(p, _) | DocView::Model(p, _) => p,
            DocView::Text(t) => &t.path,
        }
    }
}

/// Pending decision after clicking a [[link]].
pub enum LinkPopup {
    Choose(String, Vec<PathBuf>),
    Missing(String, PathBuf),
}

fn load_content(path: &Path, kind: Kind, blender: &str) -> Result<DocContent, String> {
    match kind {
        Kind::Markdown | Kind::Text | Kind::Python => crate::textdoc::load(path).map(DocContent::Text),
        Kind::Csv => {
            let t = crate::textdoc::load(path)?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let delim = crate::office::guess_delim(&t.text, &ext);
            Ok(DocContent::Csv { rows: crate::office::csv_parse(&t.text, delim), delim })
        }
        Kind::Audio => crate::audio::decode(path).map(DocContent::Audio),
        Kind::Pdf => crate::office::pdf_open(path).map(DocContent::Pdf),
        Kind::Docx => crate::office::docx_open(path).map(DocContent::Docx),
        Kind::Sheet => crate::office::sheets_open(path).map(DocContent::Sheets),
        Kind::Model => crate::model3d::load(path).map(DocContent::Model),
        Kind::ModelBlender => {
            let b = crate::model3d::find_blender(blender).ok_or(
                "This format is opened through Blender, but Blender wasn't found. Use \"Set Blender location…\" below.",
            )?;
            crate::model3d::load_via_blender(path, &b).map(DocContent::Model)
        }
        Kind::ModelClosed => Err(
            "3ds Max (.max) files can only be opened by 3ds Max itself — not even Blender can read them. \
             In 3ds Max, use File → Export as FBX or OBJ and Viewer will show that."
                .into(),
        ),
        Kind::Image => Err("image".into()),
        Kind::Other => match crate::textdoc::load(path) {
            Ok(t) if !t.text.is_empty() => Ok(DocContent::Text(t)),
            _ => Err("No preview for this kind of file.".into()),
        },
    }
}

fn page_rows(info: &PdfInfo) -> f32 {
    let (w, h) = info.sizes.first().copied().unwrap_or((612.0, 792.0));
    h / w.max(1.0)
}

impl App {
    pub fn open_doc(&mut self, path: PathBuf) {
        self.save_doc_if_dirty();
        self.doc_gen += 1;
        let kind = crate::filetypes::kind(&path);
        let blender = self.s.blender_path.clone();
        let generation = self.doc_gen;
        self.doc = Some(DocView::Loading(path.clone()));
        self.link_popup = None;
        self.workers.run(move || {
            let result = std::panic::catch_unwind(|| load_content(&path, kind, &blender))
                .unwrap_or_else(|_| Err("This file could not be read (the reader stopped unexpectedly).".into()));
            Msg::Doc { generation, result }
        });
    }

    pub fn doc_loaded(&mut self, generation: u64, result: Result<DocContent, String>) {
        if generation != self.doc_gen {
            return;
        }
        let Some(path) = self.doc.as_ref().map(|d| d.path().to_path_buf()) else { return };
        self.doc = Some(match result {
            Ok(DocContent::Text(t)) => DocView::Text(t),
            Ok(DocContent::Audio(a)) => DocView::Audio(path, Player::new(a)),
            Ok(DocContent::Pdf(info)) => DocView::Pdf(path, PdfView { info, zoom: 1.0, pages: HashMap::new(), requested: HashSet::new() }),
            Ok(DocContent::Docx(b)) => {
                let n = b.len();
                DocView::Docx(path, b, (0..n).map(|_| None).collect())
            }
            Ok(DocContent::Sheets(s)) => DocView::Sheets(path, s, 0),
            Ok(DocContent::Csv { rows, delim }) => DocView::Csv(path, CsvView { rows, delim, dirty: false, edit: None }),
            Ok(DocContent::Model(m)) => {
                let y_up = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("obj"));
                DocView::Model(path, ModelView { mesh: Arc::new(m), cam: Camera { y_up, ..Camera::default() }, tex: None, rendering: false, last: None, rgen: 0 })
            }
            Err(e) => DocView::Failed(path, e),
        });
    }

    pub fn save_doc_if_dirty(&mut self) {
        let mut msg = None;
        match &mut self.doc {
            Some(DocView::Text(t)) if t.dirty() && t.read_only.is_none() => {
                if let Err(e) = t.save() {
                    msg = Some(format!("Could not save {}: {e}", file_name(&t.path)));
                }
            }
            Some(DocView::Csv(p, c)) if c.dirty => {
                let s = crate::office::csv_write(&c.rows, c.delim);
                match crate::textdoc::atomic_write(p, s.as_bytes()) {
                    Ok(()) => c.dirty = false,
                    Err(e) => msg = Some(format!("Could not save: {e}")),
                }
            }
            _ => {}
        }
        if let Some(m) = msg {
            let ctx = self.ctx_handle.clone();
            self.toast(&ctx, m);
        }
    }

    /// Open a file found through a link: in place if it is in the current
    /// listing, otherwise switch this tab to its folder.
    pub fn open_found(&mut self, p: &Path) {
        if let Some(i) = self.files.iter().position(|e| e.path == p) {
            self.go(i);
        } else {
            self.open_path(p);
        }
    }

    fn follow_link(&mut self, c: Click) {
        let Some(from) = self.doc.as_ref().map(|d| d.path().to_path_buf()) else { return };
        match c {
            Click::Url(u) => crate::assoc::open_external(Path::new(&u)),
            Click::Wiki(name) => {
                let direct = PathBuf::from(&name);
                if direct.is_absolute() && direct.is_file() {
                    self.open_found(&direct);
                    return;
                }
                let root = self.tab().folder.clone();
                let found = crate::textdoc::resolve(&name, &from, root.as_deref());
                let same_dir = |p: &Path| p.parent() == from.parent();
                match found.as_slice() {
                    [one] if same_dir(one) => {
                        let one = one.clone();
                        self.open_found(&one);
                    }
                    [] => {
                        let target = crate::textdoc::link_target(&name);
                        let dir = from.parent().unwrap_or(Path::new(".")).to_path_buf();
                        let file = target.trim_end_matches(".md").to_string() + ".md";
                        self.link_popup = Some(LinkPopup::Missing(target, dir.join(file)));
                    }
                    many => self.link_popup = Some(LinkPopup::Choose(crate::textdoc::link_target(&name), many.to_vec())),
                }
            }
        }
    }

    fn link_popup_ui(&mut self, ctx: &egui::Context) {
        let Some(pop) = self.link_popup.take() else { return };
        let mut keep = true;
        let mut open: Option<PathBuf> = None;
        egui::Window::new("Link").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match &pop {
                LinkPopup::Choose(name, list) => {
                    ui.label(RichText::new(format!("[[{name}]] is in another folder:")).strong());
                    if list.len() > 1 {
                        ui.label(RichText::new(format!("{} notes have this name.", list.len())).color(theme::DIM));
                    }
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for p in list {
                            ui.horizontal(|ui| {
                                if ui.button("Open").clicked() {
                                    open = Some(p.clone());
                                    keep = false;
                                }
                                if ui.small_button("Show in folder").clicked() {
                                    crate::assoc::reveal(p);
                                }
                                ui.label(RichText::new(p.display().to_string()).small().color(theme::DIM));
                            });
                        }
                    });
                }
                LinkPopup::Missing(name, file) => {
                    ui.label(RichText::new(format!("No note called \"{name}\" was found here or in its subfolders.")).strong());
                    ui.label(RichText::new(file.display().to_string()).small().color(theme::DIM));
                    if ui.button("Create it in this folder").clicked() {
                        match std::fs::OpenOptions::new().write(true).create_new(true).open(file) {
                            Ok(mut f) => {
                                let _ = std::io::Write::write_all(&mut f, format!("# {name}\n\n").as_bytes());
                                open = Some(file.clone());
                            }
                            Err(e) => self.toast(ctx, format!("Could not create note: {e}")),
                        }
                        keep = false;
                    }
                }
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
        });
        if let Some(p) = open {
            if self.files.iter().all(|e| e.path != p) && p.parent() == self.tab().folder.as_deref() {
                self.pending_open = Some(p.clone());
                self.refresh_listing();
            } else {
                self.open_found(&p);
            }
        } else if keep {
            self.link_popup = Some(pop);
        }
    }

    // ---------------------------------------------------------------- dispatcher

    pub fn doc_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, theme::CANVAS);
        let Some(path) = self.doc.as_ref().map(|d| d.path().to_path_buf()) else { return };
        // Header: name + actions common to every type.
        egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let dirty = match &self.doc {
                    Some(DocView::Text(t)) => t.dirty(),
                    Some(DocView::Csv(_, c)) => c.dirty,
                    _ => false,
                };
                ui.label(RichText::new(format!("{}{}", file_name(&path), if dirty { "  • unsaved" } else { "" })).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Open in…").on_hover_text("Open with your default app for this type").clicked() {
                        self.save_doc_if_dirty();
                        crate::assoc::open_external(&path);
                    }
                    if ui.button("Show in folder").clicked() {
                        crate::assoc::reveal(&path);
                    }
                });
            });
        });
        match &mut self.doc {
            Some(DocView::Loading(_)) => {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    let msg = if crate::filetypes::kind(&path) == Kind::ModelBlender { "Converting with Blender…" } else { "Loading…" };
                    ui.label(RichText::new(msg).color(theme::DIM));
                });
            }
            Some(DocView::Failed(_, e)) => {
                let e = e.clone();
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(460.0);
                    ui.label(RichText::new(e).color(theme::DIM));
                    if crate::filetypes::kind(&path) == Kind::ModelBlender && ui.button("Set Blender location…").clicked() {
                        let exe = if cfg!(windows) { &["exe"][..] } else { &[""][..] };
                        if let Some(p) = rfd::FileDialog::new().add_filter("Blender", exe).pick_file() {
                            self.s.blender_path = p.display().to_string();
                            self.open_doc(path.clone());
                        }
                    }
                });
            }
            Some(DocView::Text(_)) => self.text_ui(ui, now),
            Some(DocView::Audio(_, _)) => self.audio_ui(ui),
            Some(DocView::Pdf(_, _)) => self.pdf_ui(ui),
            Some(DocView::Docx(_, _, _)) => self.docx_ui(ui),
            Some(DocView::Sheets(_, _, _)) => self.sheets_ui(ui),
            Some(DocView::Csv(_, _)) => self.csv_ui(ui),
            Some(DocView::Model(_, _)) => self.model_ui(ui),
            None => {}
        }
        self.link_popup_ui(&ctx);
    }

    // ---------------------------------------------------------------- text / markdown / python

    fn text_ui(&mut self, ui: &mut Ui, now: f64) {
        let ctx = ui.ctx().clone();
        let autosave = self.s.autosave;
        let mut md_mode = self.s.md_mode;
        let root = self.tab().folder.clone();
        let mut click: Option<Click> = None;
        let mut want_mentions = false;
        let mut toast: Option<String> = None;
        let mut open_mention: Option<PathBuf> = None;
        let Some(DocView::Text(doc)) = &mut self.doc else { return };
        doc.poll_disk(now);
        if let Some(r) = &mut doc.run {
            r.poll();
            if r.running() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let editor_id = ui.make_persistent_id(("editor", doc.path.clone()));
        let is_md = doc.is_markdown();
        let is_py = doc.is_python();

        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            if is_md {
                for (m, l) in [(MdMode::Preview, "Preview"), (MdMode::Split, "Split"), (MdMode::Edit, "Edit")] {
                    if ui.selectable_label(md_mode == m, l).clicked() {
                        md_mode = m;
                    }
                }
                ui.separator();
            }
            let save = ui.add_enabled(doc.dirty() && doc.read_only.is_none(), egui::Button::new("Save")).on_hover_text("Ctrl+S");
            if save.clicked() || (ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S))) {
                if let Err(e) = doc.save() {
                    toast = Some(e);
                }
            }
            if ui.selectable_label(doc.show_find, "Find").on_hover_text("Ctrl+F").clicked() || ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
                doc.show_find = !doc.show_find;
            }
            if is_py {
                ui.separator();
                let running = doc.run.as_ref().is_some_and(|r| r.running());
                if running {
                    if ui.button("■ Stop").clicked() {
                        if let Some(r) = &mut doc.run {
                            r.stop();
                        }
                    }
                } else if ui.button("▶ Run").on_hover_text("Ctrl+Enter — saves, then runs with your installed Python").clicked()
                    || ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter))
                {
                    if doc.dirty() {
                        let _ = doc.save();
                    }
                    match crate::textdoc::run_python(&doc.path, &doc.args, ctx.clone(), now) {
                        Ok(r) => doc.run = Some(r),
                        Err(e) => toast = Some(e),
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut doc.args).hint_text("arguments").desired_width(140.0));
            }
            if is_md && ui.button("Linked mentions").on_hover_text("Notes in this folder tree that link here").clicked() {
                want_mentions = true;
            }
            ui.label(RichText::new(format!("{} words", doc.word_count())).small().color(theme::DIM));
            if let Some(r) = &doc.read_only {
                ui.label(RichText::new(r).small().color(theme::ACCENT));
            }
        });
        if doc.changed_on_disk {
            ui.horizontal(|ui| {
                ui.label(RichText::new("This file was changed by another program.").color(theme::ACCENT));
                if ui.button("Load their version").clicked() {
                    let _ = doc.reload();
                }
                if ui.button("Keep mine").clicked() {
                    doc.changed_on_disk = false;
                    let _ = doc.save();
                }
            });
        }
        if doc.show_find {
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut doc.find).hint_text("find").desired_width(160.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.small_button("▲").clicked() {
                    crate::textdoc::find_next(&ctx, editor_id, doc, true);
                }
                if ui.small_button("▼").clicked() || enter {
                    if !crate::textdoc::find_next(&ctx, editor_id, doc, false) {
                        toast = Some("Not found".into());
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut doc.replace).hint_text("replace with").desired_width(140.0));
                if ui.button("Replace all").clicked() && doc.read_only.is_none() {
                    let n = crate::textdoc::replace_all(doc);
                    toast = Some(format!("Replaced {n}"));
                }
            });
        }

        // Python output pane.
        if let Some(run) = &doc.run {
            let output = run.output.clone();
            let status = run.done.lock().ok().and_then(|d| d.clone());
            let running = run.running();
            let elapsed = now - run.started;
            egui::Panel::bottom(("pyout", doc.path.clone())).resizable(true).default_size(180.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Output").strong());
                    let st = if running { format!("running… {:.0}s", elapsed) } else { status.unwrap_or_default() };
                    ui.label(RichText::new(st).small().color(theme::DIM));
                    if ui.small_button("Copy").clicked()
                        && let Ok(o) = output.lock()
                    {
                        ctx.copy_text(o.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"));
                    }
                });
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    if let Ok(o) = output.lock() {
                        for (err, l) in o.iter() {
                            ui.label(RichText::new(l).monospace().color(if *err { Color32::from_rgb(235, 120, 110) } else { theme::TEXT }));
                        }
                    }
                });
            });
        }
        if let Some(list) = &doc.mentions {
            egui::Panel::bottom(("mentions", doc.path.clone())).resizable(true).default_size(140.0).show(ui, |ui| {
                ui.label(RichText::new(format!("Linked mentions ({})", list.len())).strong());
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if list.is_empty() {
                        ui.label(RichText::new("No other notes link here.").color(theme::DIM));
                    }
                    for m in list {
                        ui.horizontal(|ui| {
                            if ui.link(file_name(&m.path)).on_hover_text(m.path.display().to_string()).clicked() {
                                open_mention = Some(m.path.clone());
                            }
                            ui.label(RichText::new(format!("line {}: {}", m.line, m.snippet)).small().color(theme::DIM));
                        });
                    }
                });
            });
        } else if doc.mentions_busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Searching for linked mentions…").small().color(theme::DIM));
            });
        }

        let lang = doc.lang.clone();
        let editor = |ui: &mut Ui, doc: &mut TextDoc| {
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                let mut job = crate::textdoc::highlight(buf.as_str(), &lang, FontId::monospace(12.5));
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let read_only = doc.read_only.is_some();
            egui::ScrollArea::vertical().id_salt("edscroll").auto_shrink([false, false]).show(ui, |ui| {
                let te = egui::TextEdit::multiline(&mut doc.text)
                    .id(editor_id)
                    .code_editor()
                    .interactive(!read_only)
                    .desired_width(f32::INFINITY)
                    .desired_rows(30)
                    .lock_focus(true)
                    .layouter(&mut layouter);
                ui.add(te)
            })
            .inner
        };
        let mut edited = false;
        match (is_md, md_mode) {
            (true, MdMode::Preview) => {
                let base = doc.path.clone();
                egui::ScrollArea::vertical().id_salt("mdprev").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 10)).show(ui, |ui| {
                        ui.set_max_width(780.0);
                        click = crate::textdoc::markdown_preview(ui, &doc.text, &base);
                    });
                });
            }
            (true, MdMode::Split) => {
                let w = ui.available_width();
                egui::Panel::left(("mdsplit", doc.path.clone())).resizable(true).default_size(w * 0.5).show(ui, |ui| {
                    edited = editor(ui, doc).changed();
                });
                let base = doc.path.clone();
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("mdprev2").auto_shrink([false, false]).show(ui, |ui| {
                        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                            click = crate::textdoc::markdown_preview(ui, &doc.text, &base);
                        });
                    });
                });
            }
            _ => {
                edited = editor(ui, doc).changed();
            }
        }
        if edited {
            self.last_edit = now;
        }
        if autosave && doc.dirty() && doc.read_only.is_none() && now - self.last_edit > 1.5 && self.last_edit > 0.0 && doc.save().is_err() {
            toast = Some("Autosave failed".into());
        }
        if autosave && doc.dirty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        let mention_req = if want_mentions && !doc.mentions_busy {
            doc.mentions_busy = true;
            Some(doc.path.clone())
        } else {
            None
        };
        self.s.md_mode = md_mode;
        if let Some(t) = toast {
            self.toast(&ctx, t);
        }
        if let Some(p) = mention_req {
            let root = root.unwrap_or_else(|| p.parent().map(Path::to_path_buf).unwrap_or_default());
            self.workers.run(move || {
                let list = crate::textdoc::linked_mentions(&p, &root);
                Msg::Mentions { path: p, list }
            });
        }
        if let Some(p) = open_mention {
            self.open_found(&p);
        } else if let Some(c) = click {
            self.follow_link(c);
        }
    }

    // ---------------------------------------------------------------- audio

    fn audio_ui(&mut self, ui: &mut Ui) {
        let Some(DocView::Audio(_, player)) = &mut self.doc else { return };
        let a = player.audio.clone();
        let total = a.frames() as f64 / a.rate.max(1) as f64;
        if player.playing() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        if !ui.ctx().egui_wants_keyboard_input() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space)) {
            player.toggle();
        }
        ui.add_space(10.0);
        let w = ui.available_width() - 20.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(w.max(50.0), 160.0), Sense::click_and_drag());
        let rect = rect.translate(vec2(10.0, 0.0));
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 6.0, theme::PANEL);
        let pos = player.position();
        let frac = (pos / total.max(1e-6)) as f32;
        let mid = rect.center().y;
        let n = a.peaks.len().max(1);
        for (i, (lo, hi)) in a.peaks.iter().enumerate() {
            let x = rect.min.x + i as f32 / n as f32 * rect.width();
            let played = (i as f32 / n as f32) < frac;
            let c = if played { theme::ACCENT } else { Color32::from_gray(120) };
            p.line_segment([pos2(x, mid - hi * rect.height() * 0.46), pos2(x, mid - lo * rect.height() * 0.46)], Stroke::new(1.0, c));
        }
        let px = rect.min.x + frac * rect.width();
        p.line_segment([pos2(px, rect.min.y), pos2(px, rect.max.y)], Stroke::new(2.0, Color32::WHITE));
        if (resp.clicked() || resp.dragged())
            && let Some(q) = resp.interact_pointer_pos()
        {
            player.seek(((q.x - rect.min.x) / rect.width()).clamp(0.0, 1.0) as f64 * total);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let label = if player.playing() { "⏸ Pause" } else { "▶ Play" };
            if ui.add(egui::Button::new(RichText::new(label).size(15.0))).on_hover_text("Space").clicked() {
                player.toggle();
            }
            if ui.button("⏮").on_hover_text("Back to start").clicked() {
                player.seek(0.0);
            }
            ui.label(RichText::new(format!("{} / {}", fmt_time(pos), fmt_time(total))).monospace());
            let mut lp = player.t.looping.load(std::sync::atomic::Ordering::Relaxed);
            if ui.checkbox(&mut lp, "Loop").changed() {
                player.t.looping.store(lp, std::sync::atomic::Ordering::Relaxed);
            }
            let mut v = player.t.volume.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10.0;
            if ui.add(egui::Slider::new(&mut v, 0.0..=150.0).text("volume %").fixed_decimals(0)).changed() {
                player.t.volume.store((v * 10.0) as u32, std::sync::atomic::Ordering::Relaxed);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{} · {} Hz · {} channel{} · {}",
                    a.codec,
                    a.rate,
                    a.channels_in_file,
                    if a.channels_in_file == 1 { "" } else { "s" },
                    fmt_time(total)
                ))
                .small()
                .color(theme::DIM),
            );
        });
        if a.truncated {
            ui.label(RichText::new("Only the first 30 minutes are loaded for preview.").small().color(theme::ACCENT));
        }
        if let Some(e) = &player.device_error {
            ui.label(RichText::new(e).color(theme::ACCENT));
        }
    }

    // ---------------------------------------------------------------- PDF

    fn pdf_ui(&mut self, ui: &mut Ui) {
        let ppp = ui.ctx().pixels_per_point();
        let pgen = self.doc_gen;
        let mut requests: Vec<(usize, f32)> = Vec::new();
        let Some(DocView::Pdf(_, v)) = &mut self.doc else { return };
        let bytes = v.info.bytes.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} pages", v.info.pages)).color(theme::DIM));
            if ui.button("−").clicked() {
                v.zoom = (v.zoom / 1.2).max(0.3);
            }
            ui.label(format!("{:.0}%", v.zoom * 100.0));
            if ui.button("+").clicked() {
                v.zoom = (v.zoom * 1.2).min(5.0);
            }
            if ui.button("Fit width").clicked() {
                v.zoom = 1.0;
            }
        });
        let z = ui.input(|i| i.zoom_delta());
        if (z - 1.0).abs() > 1e-3 {
            v.zoom = (v.zoom * z).clamp(0.3, 5.0);
        }
        let avail = ui.available_width() - 24.0;
        let page_w = (avail * v.zoom).max(100.0);
        let page_h = page_w * page_rows(&v.info);
        let row_h = page_h + 14.0;
        let n = v.info.pages;
        egui::ScrollArea::both().id_salt("pdf").auto_shrink([false, false]).show_rows(ui, row_h, n, |ui, range| {
            for i in range {
                let (rect, _) = ui.allocate_exact_size(vec2(page_w.max(avail), row_h), Sense::hover());
                let pr = Rect::from_min_size(pos2(rect.center().x - page_w / 2.0, rect.min.y + 6.0), vec2(page_w, page_h));
                let p = ui.painter();
                p.rect_filled(pr.translate(vec2(0.0, 3.0)), 2.0, Color32::from_black_alpha(80));
                p.rect_filled(pr, 0.0, Color32::WHITE);
                let pts_w = v.info.sizes.get(i).map(|s| s.0).unwrap_or(612.0);
                let want = page_w * ppp / pts_w.max(1.0);
                match v.pages.get(&i) {
                    Some((t, s)) => {
                        p.image(t.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        if (s / want - 1.0).abs() > 0.3 {
                            requests.push((i, want));
                        }
                    }
                    None => {
                        p.text(pr.center(), Align2::CENTER_CENTER, format!("page {}", i + 1), FontId::proportional(13.0), Color32::from_gray(150));
                        requests.push((i, want));
                    }
                }
            }
        });
        for (i, s) in requests {
            let key = (i, (s * 4.0) as u32);
            if v.requested.insert(key) {
                let b = bytes.clone();
                self.workers.run(move || {
                    let result = std::panic::catch_unwind(|| crate::office::pdf_render(&b, i, s))
                        .unwrap_or_else(|_| Err("render failed".into()));
                    Msg::PdfPage { generation: pgen, page: i, scale: s, result }
                });
            }
        }
    }

    pub fn pdf_page_done(&mut self, ctx: &egui::Context, generation: u64, page: usize, scale: f32, img: RgbaImage) {
        if generation != self.doc_gen {
            return;
        }
        if let Some(DocView::Pdf(_, v)) = &mut self.doc {
            let tex = ctx.load_texture(format!("pdf{page}"), crate::app::to_color_image(&img), crate::app::tex_options());
            v.pages.insert(page, (tex, scale));
            if v.pages.len() > 40 {
                let keep: Vec<usize> = v.pages.keys().copied().collect();
                if let Some(far) = keep.iter().max_by_key(|k| (**k as i64 - page as i64).abs()) {
                    let far = *far;
                    v.pages.remove(&far);
                    v.requested.retain(|r| r.0 != far);
                }
            }
        }
    }

    // ---------------------------------------------------------------- Word

    fn docx_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(DocView::Docx(_, blocks, texs)) = &mut self.doc else { return };
        egui::ScrollArea::vertical().id_salt("docx").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().fill(Color32::from_rgb(250, 249, 246)).inner_margin(egui::Margin::symmetric(40, 30)).show(ui, |ui| {
                ui.set_max_width(760.0);
                let ink = Color32::from_gray(28);
                ui.style_mut().visuals.override_text_color = Some(ink);
                for (bi, b) in blocks.iter().enumerate() {
                    match b {
                        Block::Para { heading, list, runs } => {
                            if runs.iter().all(|r| r.text.trim().is_empty()) {
                                ui.add_space(6.0);
                                continue;
                            }
                            let size = match heading {
                                1 => 22.0,
                                2 => 18.0,
                                3 => 15.5,
                                4..=6 => 14.0,
                                _ => 13.0,
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                if let Some(l) = list {
                                    ui.add_space(*l as f32 * 18.0 + 6.0);
                                    ui.label(RichText::new("•  ").size(size).color(ink));
                                }
                                for r in runs {
                                    for (k, w) in r.text.split_inclusive(' ').enumerate() {
                                        let _ = k;
                                        let mut t = RichText::new(w).size(size).color(ink);
                                        if r.bold || *heading > 0 {
                                            t = t.strong();
                                        }
                                        if r.italic {
                                            t = t.italics();
                                        }
                                        if r.underline {
                                            t = t.underline();
                                        }
                                        ui.label(t);
                                    }
                                }
                            });
                            ui.add_space(if *heading > 0 { 6.0 } else { 3.0 });
                        }
                        Block::Table(rows) => {
                            egui::Grid::new(("dtbl", bi)).spacing([14.0, 4.0]).show(ui, |ui| {
                                for r in rows {
                                    for c in r {
                                        ui.label(RichText::new(c).color(ink));
                                    }
                                    ui.end_row();
                                }
                            });
                            ui.add_space(8.0);
                        }
                        Block::Image(img) => {
                            if texs.get(bi).is_some_and(|t| t.is_none()) {
                                texs[bi] = Some(ctx.load_texture(format!("docimg{bi}"), crate::app::to_color_image(img), crate::app::tex_options()));
                            }
                            if let Some(Some(t)) = texs.get(bi) {
                                let s = t.size_vec2();
                                let w = s.x.min(ui.available_width());
                                ui.image((t.id(), vec2(w, w * s.y / s.x.max(1.0))));
                            }
                        }
                    }
                }
            });
        });
    }

    // ---------------------------------------------------------------- sheets & CSV

    fn grid(ui: &mut Ui, rows: &[Vec<String>], mut edit: Option<&mut Option<(usize, usize, String)>>) -> Option<(usize, usize)> {
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
        let (cw, rh, hw) = (120.0f32, 20.0f32, 46.0f32);
        let mut clicked = None;
        let font = FontId::proportional(11.5);
        egui::ScrollArea::both().id_salt("grid").auto_shrink([false, false]).show_rows(ui, rh, rows.len() + 1, |ui, range| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for r in range {
                let (rect, _) = ui.allocate_exact_size(vec2(hw + cw * cols as f32, rh), Sense::hover());
                let p = ui.painter().clone();
                if r == 0 {
                    p.rect_filled(rect, 0.0, theme::CHROME);
                    for c in 0..cols {
                        let cr = Rect::from_min_size(pos2(rect.min.x + hw + c as f32 * cw, rect.min.y), vec2(cw, rh));
                        p.text(cr.center(), Align2::CENTER_CENTER, crate::office::col_name(c), font.clone(), theme::DIM);
                    }
                    continue;
                }
                let ri = r - 1;
                p.text(pos2(rect.min.x + hw - 6.0, rect.center().y), Align2::RIGHT_CENTER, (ri + 1).to_string(), font.clone(), theme::DIM);
                let row = &rows[ri];
                for c in 0..cols {
                    let cr = Rect::from_min_size(pos2(rect.min.x + hw + c as f32 * cw, rect.min.y), vec2(cw, rh));
                    p.rect_stroke(cr, 0.0, Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
                    let editing = edit.as_ref().and_then(|e| e.as_ref()).is_some_and(|(er, ec, _)| *er == ri && *ec == c);
                    if editing {
                        if let Some(Some((_, _, buf))) = edit.as_deref_mut() {
                            let r = ui.put(cr.shrink(1.0), egui::TextEdit::singleline(buf).frame(egui::Frame::NONE));
                            r.request_focus();
                        }
                        continue;
                    }
                    if let Some(v) = row.get(c) {
                        let numeric = v.parse::<f64>().is_ok();
                        let g = p.layout_no_wrap(v.clone(), font.clone(), theme::TEXT);
                        let x = if numeric { cr.max.x - 4.0 - g.size().x } else { cr.min.x + 4.0 };
                        p.with_clip_rect(cr.shrink(1.0)).galley(pos2(x, cr.center().y - g.size().y / 2.0), g, theme::TEXT);
                    }
                    if edit.is_some() {
                        let resp = ui.interact(cr, ui.scope_id().with(("cell", ri, c)), Sense::click());
                        if resp.clicked() {
                            clicked = Some((ri, c));
                        }
                    }
                }
            }
        });
        clicked
    }

    fn sheets_ui(&mut self, ui: &mut Ui) {
        let Some(DocView::Sheets(_, sheets, active)) = &mut self.doc else { return };
        ui.horizontal(|ui| {
            for (i, s) in sheets.iter().enumerate() {
                if ui.selectable_label(*active == i, &s.name).clicked() {
                    *active = i;
                }
            }
            ui.label(RichText::new("read-only").small().color(theme::DIM));
        });
        if let Some(s) = sheets.get(*active) {
            Self::grid(ui, &s.rows, None);
        }
    }

    fn csv_ui(&mut self, ui: &mut Ui) {
        let mut save = false;
        let Some(DocView::Csv(_, v)) = &mut self.doc else { return };
        ui.horizontal(|ui| {
            if ui.add_enabled(v.dirty, egui::Button::new("Save")).on_hover_text("Ctrl+S").clicked() || ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
                save = true;
            }
            if ui.button("+ Row").clicked() {
                let cols = v.rows.iter().map(|r| r.len()).max().unwrap_or(1);
                v.rows.push(vec![String::new(); cols]);
                v.dirty = true;
            }
            if ui.button("+ Column").clicked() {
                for r in &mut v.rows {
                    r.push(String::new());
                }
                v.dirty = true;
            }
            ui.label(RichText::new(format!("{} rows · click a cell to edit · Enter to confirm", v.rows.len())).small().color(theme::DIM));
        });
        // Commit or cancel an edit.
        if v.edit.is_some() {
            let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter) || i.key_pressed(Key::Tab), i.key_pressed(Key::Escape)));
            if esc {
                v.edit = None;
            } else if enter
                && let Some((r, c, s)) = v.edit.take()
            {
                if let Some(row) = v.rows.get_mut(r) {
                    while row.len() <= c {
                        row.push(String::new());
                    }
                    if row[c] != s {
                        row[c] = s;
                        v.dirty = true;
                    }
                }
            }
        }
        let rows = std::mem::take(&mut v.rows);
        let clicked = Self::grid(ui, &rows, Some(&mut v.edit));
        v.rows = rows;
        if let Some((r, c)) = clicked {
            // Commit any open edit first.
            if let Some((er, ec, s)) = v.edit.take()
                && let Some(row) = v.rows.get_mut(er)
            {
                while row.len() <= ec {
                    row.push(String::new());
                }
                if row[ec] != s {
                    row[ec] = s;
                    v.dirty = true;
                }
            }
            let cur = v.rows.get(r).and_then(|row| row.get(c)).cloned().unwrap_or_default();
            v.edit = Some((r, c, cur));
        }
        if save {
            self.save_doc_if_dirty();
        }
    }

    // ---------------------------------------------------------------- 3D

    fn model_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let blender = crate::model3d::find_blender(&self.s.blender_path);
        let mut export = false;
        let gen_now = self.doc_gen;
        let Some(DocView::Model(path, v)) = &mut self.doc else { return };
        let path = path.clone();
        let m = v.mesh.clone();
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut v.cam.wire, "Wireframe");
            ui.checkbox(&mut v.cam.y_up, "Y is up");
            if ui.button("Reset view").clicked() {
                let y = v.cam.y_up;
                v.cam = Camera { y_up: y, ..Camera::default() };
            }
            let sz = m.size();
            ui.label(
                RichText::new(format!(
                    "{} triangles · {} vertices · {} object{} · {:.3} × {:.3} × {:.3}",
                    m.tris.len(),
                    m.pos.len(),
                    m.objects.max(1),
                    if m.objects == 1 { "" } else { "s" },
                    sz[0],
                    sz[1],
                    sz[2]
                ))
                .small()
                .color(theme::DIM),
            );
            if !m.note.is_empty() {
                ui.label(RichText::new(&m.note).small().color(theme::ACCENT));
            }
        });
        ui.horizontal(|ui| {
            let can = crate::model3d::blender_can_import(&path);
            match (&blender, can) {
                (Some(_), true) => {
                    if ui.button("Export to .blend").on_hover_text("Imports into Blender and saves a .blend next to this file").clicked() {
                        export = true;
                    }
                }
                (None, true) => {
                    ui.label(RichText::new("Install Blender (or set its location) to export .blend").small().color(theme::DIM));
                }
                _ => {}
            }
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("blend"))
                && let Some(b) = &blender
                && ui.button("Open in Blender").clicked()
            {
                let _ = std::process::Command::new(b).arg(&path).spawn();
            }
            ui.label(RichText::new("Drag to orbit · right-drag or Shift-drag to pan · wheel to zoom · double-click resets").small().color(theme::DIM));
        });
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let pan = resp.dragged_by(egui::PointerButton::Secondary) || (resp.dragged() && ui.input(|i| i.modifiers.shift));
        if resp.dragged() {
            let d = resp.drag_delta();
            if pan {
                v.cam.pan[0] += d.x / rect.width() * 2.0 * v.cam.dist * 0.4;
                v.cam.pan[1] -= d.y / rect.height() * 2.0 * v.cam.dist * 0.4;
            } else {
                v.cam.yaw -= d.x * 0.01;
                v.cam.pitch = (v.cam.pitch + d.y * 0.01).clamp(-1.55, 1.55);
            }
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s.abs() > 0.1 {
                v.cam.dist = (v.cam.dist * (-s * 0.002).exp()).clamp(0.3, 50.0);
            }
        }
        if resp.double_clicked() {
            let y = v.cam.y_up;
            v.cam = Camera { y_up: y, ..Camera::default() };
        }
        // Lower resolution while moving, full when still.
        let moving = resp.dragged() || ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.1);
        let ppp = ctx.pixels_per_point();
        let q = if moving { 0.5 } else { 1.0 };
        let (w, h) = (((rect.width() * ppp * q) as u32).max(32), ((rect.height() * ppp * q) as u32).max(32));
        let want = (v.cam, w, h);
        if v.last != Some(want) && !v.rendering {
            v.rendering = true;
            v.last = Some(want);
            v.rgen += 1;
            let (cam, mesh, g) = (v.cam, v.mesh.clone(), gen_now);
            self.workers.run(move || {
                let img = crate::model3d::render(&mesh, &cam, w, h);
                Msg::ModelFrame { generation: g, img }
            });
        }
        let p = ui.painter_at(rect);
        match &v.tex {
            Some(t) => {
                p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                p.rect_filled(rect, 0.0, Color32::from_gray(36));
                p.text(rect.center(), Align2::CENTER_CENTER, "Rendering…", FontId::proportional(13.0), theme::DIM);
            }
        }
        if v.rendering || v.last != Some(want) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if export
            && let Some(b) = blender
        {
            self.toast(&ctx, "Exporting to .blend with Blender…");
            self.workers.run(move || Msg::Blender(crate::model3d::export_blend(&path, &b)));
        }
    }

    pub fn model_frame(&mut self, ctx: &egui::Context, generation: u64, img: RgbaImage) {
        if generation != self.doc_gen {
            return;
        }
        if let Some(DocView::Model(_, v)) = &mut self.doc {
            v.rendering = false;
            let ci = crate::app::to_color_image(&img);
            match &mut v.tex {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => v.tex = Some(ctx.load_texture("model", ci, egui::TextureOptions::LINEAR)),
            }
        }
    }

    pub fn mentions_done(&mut self, path: PathBuf, list: Vec<crate::textdoc::Mention>) {
        if let Some(DocView::Text(t)) = &mut self.doc
            && t.path == path
        {
            t.mentions_busy = false;
            t.mentions = Some(list);
        }
    }

    /// Small coloured badge for non-image files in the grid.
    pub fn file_badge(p: &egui::Painter, r: Rect, path: &Path) {
        let k = crate::filetypes::kind(path);
        let (ext, c) = crate::filetypes::badge(k, path);
        let w = r.width().min(r.height()) * 0.62;
        let page = Rect::from_center_size(r.center(), vec2(w * 0.78, w));
        p.rect_filled(page, 4.0, Color32::from_rgb(58, 58, 60));
        let fold = w * 0.22;
        p.add(egui::Shape::convex_polygon(
            vec![page.right_top() - vec2(fold, 0.0), page.right_top() + vec2(0.0, fold), page.right_top() - vec2(fold, -fold)],
            Color32::from_rgb(80, 80, 84),
            Stroke::NONE,
        ));
        let band = Rect::from_center_size(page.center() + vec2(0.0, w * 0.12), vec2(page.width() * 1.08, (w * 0.26).max(12.0)));
        p.rect_filled(band, 3.0, color32(c));
        let ext: String = ext.chars().take(5).collect();
        p.text(band.center(), Align2::CENTER_CENTER, ext, FontId::proportional((w * 0.16).clamp(8.0, 15.0)), Color32::WHITE);
    }
}
