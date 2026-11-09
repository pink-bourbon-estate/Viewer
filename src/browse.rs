//! Bridge-style browsing: toolbar (back / forward / up / path / search /
//! subfolders / sort / view / zoom), folder tabs, the Folders panel
//! (favourites, collections, tree), the Metadata panel and the content view.

use crate::app::{file_name, App, Thumb, MAX_THUMBS, THUMB_DECODE};
use crate::batch::{OutFormat, Rotate};
use crate::library::{format_size, format_time};
use crate::settings::{SortBy, ViewMode};
use crate::theme;
use egui::{pos2, vec2, Align, Align2, Color32, FontId, Key, Layout, Modifiers, Rect, RichText, Sense, Stroke, Ui};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

fn star_row(ui: &mut Ui, rating: u8, size: f32) -> Option<u8> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for i in 1..=5u8 {
            let filled = i <= rating;
            let r = ui.add(
                egui::Label::new(RichText::new(if filled { "★" } else { "☆" }).size(size).color(if filled {
                    theme::ACCENT
                } else {
                    theme::DIM
                }))
                .sense(Sense::click()),
            );
            if r.clicked() {
                out = Some(if rating == i { 0 } else { i });
            }
        }
    });
    out
}

fn draw_stars(p: &egui::Painter, at: egui::Pos2, rating: u8) {
    if rating == 0 {
        return;
    }
    let s: String = "★".repeat(rating as usize);
    p.text(at, Align2::CENTER_CENTER, s, FontId::proportional(10.0), theme::ACCENT);
}

fn folder_icon(p: &egui::Painter, r: Rect) {
    let w = r.width().min(r.height()) * 0.7;
    let body = Rect::from_center_size(r.center() + vec2(0.0, w * 0.06), vec2(w, w * 0.72));
    let tab = Rect::from_min_size(body.min - vec2(0.0, w * 0.12), vec2(w * 0.42, w * 0.16));
    let c = Color32::from_rgb(196, 160, 96);
    p.rect_filled(tab, 2.0, c.gamma_multiply(0.85));
    p.rect_filled(body, 3.0, c);
}

impl App {
    pub fn toolbar(&mut self, ui: &mut Ui) {
        let t = self.tab().clone();
        if ui.add_enabled(!t.back.is_empty(), egui::Button::new("◀")).on_hover_text("Back (Alt+←)").clicked() {
            self.go_back();
        }
        if ui.add_enabled(!t.forward.is_empty(), egui::Button::new("▶")).on_hover_text("Forward (Alt+→)").clicked() {
            self.go_forward();
        }
        if ui.add_enabled(t.folder.as_ref().is_some_and(|f| f.parent().is_some()), egui::Button::new("⏶"))
            .on_hover_text("Up a folder (Backspace / Alt+↑)")
            .clicked()
        {
            self.go_up();
        }
        // Breadcrumb.
        if let Some(c) = &t.collection {
            ui.label(RichText::new(format!("Collection: {c}")).color(theme::ACCENT));
        } else if let Some(f) = &t.folder {
            let parts: Vec<PathBuf> = f.ancestors().map(Path::to_path_buf).collect::<Vec<_>>().into_iter().rev().collect();
            let skip = parts.len().saturating_sub(4);
            if skip > 0 {
                ui.label(RichText::new("…").color(theme::DIM));
            }
            let mut go = None;
            for (i, p) in parts.iter().enumerate().skip(skip) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
                if i > skip || skip > 0 {
                    ui.label(RichText::new("›").color(theme::DIM));
                }
                let last = i + 1 == parts.len();
                if ui.add(egui::Label::new(RichText::new(name).color(if last { theme::TEXT } else { theme::DIM })).sense(Sense::click())).clicked() && !last {
                    go = Some(p.clone());
                }
            }
            if let Some(p) = go {
                self.navigate(&p, true);
            }
            let fav = self.lib.favourites.contains(f);
            if ui.small_button(if fav { "★" } else { "☆" }).on_hover_text("Favourite this folder").clicked() {
                self.lib.toggle_favourite(f);
            }
        } else if ui.button("Open folder…").clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            self.navigate(&d, true);
        }
        ui.separator();
        let mut rec = t.recursive;
        if ui.toggle_value(&mut rec, "Subfolders").on_hover_text("Show images in all subfolders too").changed() {
            self.tab_mut().recursive = rec;
            self.refresh_listing();
        }
        let mut q = t.search.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text("Search names & keywords").desired_width(170.0));
        if r.changed() {
            self.tab_mut().search = q;
            self.apply_filter();
        }
    }

    pub fn tab_strip(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let mut switch = None;
            let mut close = None;
            for (i, t) in self.s.tabs.iter().enumerate() {
                let active = i == self.s.active_tab;
                let frame = egui::Frame::new()
                    .fill(if active { theme::CARD } else { theme::PANEL })
                    .stroke(if active { Stroke::new(1.0, theme::LINE) } else { Stroke::NONE })
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 1));
                let r = frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let title = t.title();
                            let short: String = if title.chars().count() > 22 { title.chars().take(21).collect::<String>() + "…" } else { title };
                            let l = ui.add(egui::Label::new(RichText::new(short).color(if active { theme::TEXT } else { theme::DIM })).sense(Sense::click()));
                            if l.clicked() {
                                switch = Some(i);
                            }
                            if l.middle_clicked() {
                                close = Some(i);
                            }
                            if self.s.tabs.len() > 1 && ui.add(egui::Label::new(RichText::new("×").color(theme::DIM)).sense(Sense::click())).clicked() {
                                close = Some(i);
                            }
                        });
                    })
                    .response;
                if r.middle_clicked() {
                    close = Some(i);
                }
            }
            if ui.small_button("+").on_hover_text("New tab (Ctrl+T)").clicked() {
                self.new_tab();
            }
            if let Some(i) = close {
                self.close_tab(i);
            } else if let Some(i) = switch
                && i != self.s.active_tab
            {
                self.switch_tab(i);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // View options for the content area.
                if ui.small_button("+").on_hover_text("Bigger thumbnails").clicked() {
                    self.s.thumb_size = (self.s.thumb_size * 1.2).min(320.0);
                }
                ui.add(egui::Slider::new(&mut self.s.thumb_size, 48.0..=320.0).show_value(false)).on_hover_text("Thumbnail size (Ctrl+wheel)");
                if ui.small_button("−").on_hover_text("Smaller thumbnails").clicked() {
                    self.s.thumb_size = (self.s.thumb_size / 1.2).max(48.0);
                }
                ui.separator();
                for (m, label, tip) in [(ViewMode::Details, "☰", "Details list"), (ViewMode::Thumbs, "⊞", "Thumbnails only"), (ViewMode::Tiles, "▣", "Tiles with names")] {
                    if ui.selectable_label(self.s.view_mode == m, label).on_hover_text(tip).clicked() {
                        self.s.view_mode = m;
                    }
                }
                ui.separator();
                let mut changed = false;
                let mut filt = self.s.filter;
                egui::ComboBox::from_id_salt("kindfilter")
                    .width(96.0)
                    .selected_text(filt.label())
                    .show_ui(ui, |ui| {
                        for f in crate::filetypes::Filter::ALL {
                            changed |= ui.selectable_value(&mut filt, f, f.label()).changed();
                        }
                    });
                self.s.filter = filt;
                let mut min = self.s.min_rating;
                egui::ComboBox::from_id_salt("minrating")
                    .width(56.0)
                    .selected_text(if min == 0 { "All".to_string() } else { format!("{min}★+") })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut min, 0, "All").changed();
                        for r in 1..=5u8 {
                            changed |= ui.selectable_value(&mut min, r, format!("{} and up", "★".repeat(r as usize))).changed();
                        }
                    });
                if ui.small_button(if self.s.sort_desc { "⏷" } else { "⏶" }).on_hover_text("Sort direction").clicked() {
                    self.s.sort_desc = !self.s.sort_desc;
                    changed = true;
                }
                let mut sort = self.s.sort;
                egui::ComboBox::from_id_salt("sort")
                    .width(84.0)
                    .selected_text(match sort {
                        SortBy::Name => "Name",
                        SortBy::Modified => "Date",
                        SortBy::Size => "Size",
                        SortBy::Rating => "Rating",
                        SortBy::Type => "Type",
                    })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut sort, SortBy::Name, "Name").changed();
                        changed |= ui.selectable_value(&mut sort, SortBy::Modified, "Date modified").changed();
                        changed |= ui.selectable_value(&mut sort, SortBy::Size, "Size").changed();
                        changed |= ui.selectable_value(&mut sort, SortBy::Rating, "Rating").changed();
                        changed |= ui.selectable_value(&mut sort, SortBy::Type, "Type").changed();
                    });
                if changed {
                    self.s.sort = sort;
                    self.s.min_rating = min;
                    self.apply_filter();
                }
            });
        });
    }

    // ---------------------------------------------------------------- Folders panel

    fn tree_node(&mut self, ui: &mut Ui, p: &Path, label: Option<String>, depth: usize, go: &mut Option<PathBuf>) {
        if depth > 24 {
            return;
        }
        let open = self.expanded.contains(p);
        let current = self.tab().folder.as_deref() == Some(p) && self.tab().collection.is_none();
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            let arrow = if open { "⏷" } else { "⏵" };
            if ui.add(egui::Label::new(RichText::new(arrow).color(theme::DIM)).sense(Sense::click())).clicked() {
                if open {
                    self.expanded.remove(p);
                } else {
                    self.expanded.insert(p.to_path_buf());
                }
            }
            let name = label.unwrap_or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()));
            let r = ui.selectable_label(current, name);
            if r.clicked() {
                *go = Some(p.to_path_buf());
            }
            if r.double_clicked() {
                self.expanded.insert(p.to_path_buf());
            }
            r.context_menu(|ui| {
                if ui.button("Open in new tab").clicked() {
                    self.new_tab();
                    *go = Some(p.to_path_buf());
                    ui.close();
                }
                let fav = self.lib.favourites.iter().any(|f| f == p);
                if ui.button(if fav { "Remove from favourites" } else { "Add to favourites" }).clicked() {
                    self.lib.toggle_favourite(p);
                    ui.close();
                }
                if ui.button("Show in file manager").clicked() {
                    crate::assoc::reveal(&p.join("."));
                    ui.close();
                }
                if ui.button("Refresh").clicked() {
                    self.tree_children.remove(p);
                    ui.close();
                }
            });
        });
        if open {
            if !self.tree_children.contains_key(p) {
                self.tree_children.insert(p.to_path_buf(), crate::library::subfolders(p));
            }
            let kids = self.tree_children.get(p).cloned().unwrap_or_default();
            for k in kids {
                self.tree_node(ui, &k, None, depth + 1, go);
            }
        }
    }

    pub fn panel_folders(&mut self, ui: &mut Ui) {
        let mut go: Option<PathBuf> = None;
        theme::section_label(ui, "Favourites");
        let favs = self.lib.favourites.clone();
        if favs.is_empty() {
            ui.label(RichText::new("Click ☆ next to the folder path to pin a folder here.").small().color(theme::DIM));
        }
        let mut remove = None;
        for f in &favs {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.display().to_string());
            let cur = self.tab().folder.as_ref() == Some(f);
            let r = ui.selectable_label(cur, format!("★ {name}")).on_hover_text(f.display().to_string());
            if r.clicked() {
                go = Some(f.clone());
            }
            r.context_menu(|ui| {
                if ui.button("Open in new tab").clicked() {
                    self.new_tab();
                    go = Some(f.clone());
                    ui.close();
                }
                if ui.button("Remove favourite").clicked() {
                    remove = Some(f.clone());
                    ui.close();
                }
            });
        }
        if let Some(f) = remove {
            self.lib.toggle_favourite(&f);
        }

        theme::section_label(ui, "Collections");
        let names: Vec<String> = self.lib.collections.keys().cloned().collect();
        let mut open_coll = None;
        let mut del = None;
        for n in &names {
            let count = self.lib.collections.get(n).map(|c| c.len()).unwrap_or(0);
            let cur = self.tab().collection.as_deref() == Some(n.as_str());
            let r = ui.selectable_label(cur, format!("♦ {n}  ({count})"));
            if r.clicked() {
                open_coll = Some(n.clone());
            }
            r.context_menu(|ui| {
                if ui.button("Delete collection").clicked() {
                    del = Some(n.clone());
                    ui.close();
                }
            });
        }
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.collection_input).hint_text("New collection").desired_width(140.0));
            if (ui.small_button("+").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
                && !self.collection_input.trim().is_empty()
            {
                let n = self.collection_input.trim().to_string();
                self.lib.new_collection(&n);
                for p in self.selected_paths() {
                    self.lib.toggle_collection(&n, &p, true);
                }
                self.collection_input.clear();
            }
        });
        if let Some(n) = del {
            self.lib.delete_collection(&n);
        }
        if let Some(n) = open_coll {
            self.open_collection(&n);
        }

        theme::section_label(ui, "Folders");
        let roots = self.roots.clone();
        for r in &roots {
            let label = if cfg!(windows) && r.parent().is_none() { r.display().to_string() } else { file_name(r) };
            self.tree_node(ui, r, Some(label), 0, &mut go);
        }
        if let Some(p) = go {
            self.navigate(&p, true);
        }
    }

    // ---------------------------------------------------------------- Metadata panel

    pub fn panel_metadata(&mut self, ui: &mut Ui) {
        let sel = self.selected_paths();
        let Some(first) = sel.first().cloned() else {
            ui.label(RichText::new("Select an image to see its details.").color(theme::DIM));
            return;
        };
        if sel.len() > 1 {
            ui.label(RichText::new(format!("{} images selected — changes apply to all", sel.len())).color(theme::ACCENT));
        } else {
            ui.label(RichText::new(file_name(&first)).strong());
        }
        ui.horizontal(|ui| {
            ui.label("Rating");
            if let Some(r) = star_row(ui, self.lib.rating(&first), 16.0) {
                self.set_rating(r);
            }
        });

        theme::section_label(ui, "Keywords");
        let kws = self.lib.keywords(&first);
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for k in &kws {
                let r = egui::Frame::new().fill(theme::CARD).corner_radius(8.0).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(k).small());
                        ui.add(egui::Label::new(RichText::new("×").small().color(theme::DIM)).sense(Sense::click())).clicked()
                    })
                    .inner
                });
                if r.inner {
                    remove = Some(k.clone());
                }
            }
        });
        if let Some(k) = remove {
            for p in &sel {
                self.lib.remove_keyword(p, &k);
            }
        }
        let r = ui.add(egui::TextEdit::singleline(&mut self.keyword_input).hint_text("Add keyword, Enter").desired_width(f32::INFINITY));
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.keyword_input.trim().is_empty() {
            for part in self.keyword_input.clone().split(',') {
                for p in &sel {
                    self.lib.add_keyword(p, part);
                }
            }
            self.keyword_input.clear();
            r.request_focus();
        }
        let q = self.keyword_input.trim().to_lowercase();
        let suggestions: Vec<String> = self
            .lib
            .all_keywords()
            .into_iter()
            .filter(|k| !kws.contains(k) && (q.is_empty() || k.to_lowercase().contains(&q)))
            .take(14)
            .collect();
        if !suggestions.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for s in suggestions {
                    if ui.small_button(&s).clicked() {
                        for p in &sel {
                            self.lib.add_keyword(p, &s);
                        }
                        self.keyword_input.clear();
                    }
                }
            });
        }

        theme::section_label(ui, "Collections");
        let names: Vec<String> = self.lib.collections.keys().cloned().collect();
        if names.is_empty() {
            ui.label(RichText::new("Create one in the Folders panel.").small().color(theme::DIM));
        }
        for n in names {
            let mut on = self.lib.in_collection(&n, &first);
            if ui.checkbox(&mut on, &n).changed() {
                for p in &sel {
                    self.lib.toggle_collection(&n, p, on);
                }
            }
        }

        theme::section_label(ui, "File");
        let entry = self.files.iter().find(|e| e.path == first).cloned();
        egui::Grid::new("fileprops").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(RichText::new(k).color(theme::DIM));
                ui.label(v);
                ui.end_row();
            };
            row("Name", file_name(&first));
            row("Folder", first.parent().map(|p| p.display().to_string()).unwrap_or_default());
            row("Type", first.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default());
            if let Some((w, h)) = self.dims.get(&first) {
                row("Dimensions", format!("{w} × {h} px  ({:.1} MP)", (*w as f64 * *h as f64) / 1e6));
            }
            if let Some(e) = &entry {
                row("Size", format_size(e.size));
                row("Modified", format_time(e.modified));
            }
        });

        if self.exif_cache.as_ref().is_none_or(|(p, _)| *p != first) {
            self.exif_cache = Some((first.clone(), crate::library::exif(&first)));
        }
        if let Some((_, ex)) = &self.exif_cache
            && !ex.is_empty()
        {
            theme::section_label(ui, "Camera (EXIF)");
            egui::Grid::new("exif").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
                for (k, v) in ex {
                    ui.label(RichText::new(k).color(theme::DIM));
                    ui.label(v);
                    ui.end_row();
                }
            });
        }
    }

    // ---------------------------------------------------------------- content view

    fn request_thumb(&mut self, p: &Path, want: &mut Vec<PathBuf>) -> Option<egui::TextureHandle> {
        match self.thumbs.get_mut(p) {
            Some(Thumb::Ready(t, seen)) => {
                *seen = self.frame_no;
                Some(t.clone())
            }
            Some(_) => None,
            None => {
                want.push(p.to_path_buf());
                None
            }
        }
    }

    fn entry_menu(&mut self, ui: &mut Ui, i: usize, action: &mut Option<(usize, &'static str)>) {
        if ui.button("Open").clicked() {
            *action = Some((i, "open"));
            ui.close();
        }
        if self.files.get(i).is_some_and(|e| e.is_dir) {
            if ui.button("Open in new tab").clicked() {
                *action = Some((i, "newtab"));
                ui.close();
            }
            return;
        }
        ui.menu_button("Rating", |ui| {
            for r in 0..=5u8 {
                if ui.button(if r == 0 { "No rating".to_string() } else { "★".repeat(r as usize) }).clicked() {
                    *action = Some((i, ["r0", "r1", "r2", "r3", "r4", "r5"][r as usize]));
                    ui.close();
                }
            }
        });
        if ui.button("Batch edit selection…").clicked() {
            *action = Some((i, "batch"));
            ui.close();
        }
        if ui.button("Rotate right").clicked() {
            *action = Some((i, "rotr"));
            ui.close();
        }
        if ui.button("Rotate left").clicked() {
            *action = Some((i, "rotl"));
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            *action = Some((i, "rename"));
            ui.close();
        }
        if ui.button("Show in folder").clicked() {
            *action = Some((i, "reveal"));
            ui.close();
        }
        ui.separator();
        if ui.button("Move to recycle bin").clicked() {
            *action = Some((i, "delete"));
            ui.close();
        }
    }

    pub fn content(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, theme::CANVAS);
        if self.files.is_empty() {
            let msg = if self.listing {
                "Reading folder…"
            } else if self.tab().folder.is_none() && self.tab().collection.is_none() {
                "Pick a folder in the Folders panel, drop one here, or press Ctrl+Shift+O"
            } else {
                "No images here"
            };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), theme::DIM);
            return;
        }
        // Ctrl+wheel zooms thumbnails.
        if ui.rect_contains_pointer(rect) {
            let z = ui.input(|i| i.zoom_delta());
            if (z - 1.0).abs() > 1e-3 {
                self.s.thumb_size = (self.s.thumb_size * z).clamp(48.0, 320.0);
            }
        }
        let mode = self.s.view_mode;
        let t = self.s.thumb_size;
        let mut clicked: Option<(usize, Modifiers, bool)> = None;
        let mut action: Option<(usize, &'static str)> = None;
        let mut want: Vec<PathBuf> = Vec::new();
        let scroll_to = if self.scroll_to_current { self.current } else { None };
        let mut did_scroll = false;
        let n = self.files.len();

        match mode {
            ViewMode::Tiles | ViewMode::Thumbs => {
                let label_h = if mode == ViewMode::Tiles { 30.0 } else { 0.0 };
                let cell = vec2(t + 10.0, t + 10.0 + label_h);
                let cols = ((ui.available_width() / cell.x).floor() as usize).max(1);
                self.grid_cols = cols;
                let rows = n.div_ceil(cols);
                egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false, false]).show_rows(ui, cell.y, rows, |ui, range| {
                    for r in range {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for c in 0..cols {
                                let i = r * cols + c;
                                let Some(e) = self.files.get(i).cloned() else { break };
                                let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                                let selected = self.selection.contains(&i);
                                let is_cur = self.current == Some(i);
                                let p = ui.painter().clone();
                                if selected || resp.hovered() {
                                    p.rect_filled(rect.shrink(2.0), 4.0, if selected { theme::SELECT } else { theme::CARD });
                                }
                                if is_cur {
                                    p.rect_stroke(rect.shrink(2.0), 4.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
                                }
                                let img_box = Rect::from_min_size(rect.min + vec2(5.0, 5.0), vec2(t, t));
                                if e.is_dir {
                                    folder_icon(&p, img_box);
                                } else if crate::filetypes::kind(&e.path) != crate::filetypes::Kind::Image {
                                    App::file_badge(&p, img_box, &e.path);
                                } else if let Some(tex) = self.request_thumb(&e.path, &mut want) {
                                    let ts = tex.size_vec2();
                                    let s = t / ts.x.max(ts.y);
                                    let r2 = Rect::from_center_size(img_box.center(), ts * s);
                                    p.image(tex.id(), r2, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                    if self.has_edits(&e.path) {
                                        p.circle_filled(r2.right_top() + vec2(-6.0, 6.0), 3.5, theme::ACCENT);
                                    }
                                } else if matches!(self.thumbs.get(&e.path), Some(Thumb::Failed)) {
                                    p.text(img_box.center(), Align2::CENTER_CENTER, "✕", FontId::proportional(18.0), theme::DIM);
                                }
                                if mode == ViewMode::Tiles {
                                    let name = file_name(&e.path);
                                    let lr = Rect::from_min_max(pos2(rect.min.x + 4.0, img_box.max.y + 2.0), pos2(rect.max.x - 4.0, img_box.max.y + 16.0));
                                    let g = p.layout_no_wrap(name, FontId::proportional(10.5), theme::DIM);
                                    let x = if g.size().x < lr.width() { lr.center().x - g.size().x / 2.0 } else { lr.min.x };
                                    p.with_clip_rect(lr).galley(pos2(x, lr.min.y), g, theme::DIM);
                                    if !e.is_dir {
                                        draw_stars(&p, pos2(rect.center().x, lr.max.y + 6.0), self.lib.rating(&e.path));
                                    }
                                }
                                let resp = resp.on_hover_text(file_name(&e.path));
                                if resp.clicked() {
                                    clicked = Some((i, ui.input(|inp| inp.modifiers), false));
                                }
                                if resp.double_clicked() {
                                    clicked = Some((i, Modifiers::NONE, true));
                                }
                                resp.context_menu(|ui| self.entry_menu(ui, i, &mut action));
                                if scroll_to == Some(i) {
                                    ui.scroll_to_rect(rect, None);
                                    did_scroll = true;
                                }
                            }
                        });
                    }
                });
            }
            ViewMode::Details => {
                self.grid_cols = 1;
                let row_h = (t * 0.32).clamp(22.0, 64.0);
                // Header with sortable columns.
                let mut sort_click = None;
                ui.horizontal(|ui| {
                    ui.add_space(row_h + 8.0);
                    for (label, sb, w) in [("Name", SortBy::Name, 0.0), ("Rating", SortBy::Rating, 70.0), ("Size", SortBy::Size, 70.0), ("Modified", SortBy::Modified, 120.0), ("Type", SortBy::Type, 46.0)] {
                        let txt = if self.s.sort == sb { format!("{label} {}", if self.s.sort_desc { "⏷" } else { "⏶" }) } else { label.to_string() };
                        let width = if w == 0.0 { (ui.available_width() - 330.0).max(80.0) } else { w };
                        if ui.add_sized([width, 18.0], egui::Button::new(RichText::new(txt).small()).frame(false)).clicked() {
                            sort_click = Some(sb);
                        }
                    }
                    ui.label(RichText::new("Dimensions").small().color(theme::DIM));
                });
                if let Some(sb) = sort_click {
                    if self.s.sort == sb {
                        self.s.sort_desc = !self.s.sort_desc;
                    } else {
                        self.s.sort = sb;
                    }
                    self.apply_filter();
                }
                egui::ScrollArea::vertical().id_salt("details").auto_shrink([false, false]).show_rows(ui, row_h, n, |ui, range| {
                    for i in range {
                        let Some(e) = self.files.get(i).cloned() else { break };
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                        let p = ui.painter().clone();
                        let selected = self.selection.contains(&i);
                        if selected || resp.hovered() {
                            p.rect_filled(rect, 2.0, if selected { theme::SELECT } else { theme::CARD });
                        }
                        if self.current == Some(i) {
                            p.rect_stroke(rect, 2.0, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
                        }
                        let ib = Rect::from_min_size(rect.min + vec2(4.0, 2.0), vec2(row_h - 4.0, row_h - 4.0));
                        if e.is_dir {
                            folder_icon(&p, ib);
                        } else if crate::filetypes::kind(&e.path) != crate::filetypes::Kind::Image {
                            App::file_badge(&p, ib, &e.path);
                        } else if let Some(tex) = self.request_thumb(&e.path, &mut want) {
                            let ts = tex.size_vec2();
                            let s = ib.width() / ts.x.max(ts.y);
                            p.image(tex.id(), Rect::from_center_size(ib.center(), ts * s), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        let y = rect.center().y;
                        let name_w = (rect.width() - row_h - 340.0).max(80.0);
                        let mut x = rect.min.x + row_h + 8.0;
                        let font = FontId::proportional(11.5);
                        let col = |p: &egui::Painter, x: f32, w: f32, s: String, c: Color32| {
                            let g = p.layout_no_wrap(s, font.clone(), c);
                            p.with_clip_rect(Rect::from_min_max(pos2(x, rect.min.y), pos2(x + w - 6.0, rect.max.y))).galley(pos2(x, y - g.size().y / 2.0), g, c);
                        };
                        col(&p, x, name_w, file_name(&e.path), theme::TEXT);
                        x += name_w;
                        if !e.is_dir {
                            let r = self.lib.rating(&e.path);
                            col(&p, x, 70.0, "★".repeat(r as usize), theme::ACCENT);
                            x += 70.0;
                            col(&p, x, 70.0, format_size(e.size), theme::DIM);
                            x += 70.0;
                            col(&p, x, 120.0, format_time(e.modified), theme::DIM);
                            x += 120.0;
                            col(&p, x, 46.0, e.path.extension().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default(), theme::DIM);
                            x += 46.0;
                            if let Some((w, h)) = self.dims.get(&e.path) {
                                col(&p, x, 100.0, format!("{w}×{h}"), theme::DIM);
                            }
                        } else {
                            col(&p, x, 120.0, "Folder".into(), theme::DIM);
                        }
                        if resp.clicked() {
                            clicked = Some((i, ui.input(|inp| inp.modifiers), false));
                        }
                        if resp.double_clicked() {
                            clicked = Some((i, Modifiers::NONE, true));
                        }
                        resp.context_menu(|ui| self.entry_menu(ui, i, &mut action));
                        if scroll_to == Some(i) {
                            ui.scroll_to_rect(rect, None);
                            did_scroll = true;
                        }
                    }
                });
            }
        }
        let _ = did_scroll;
        self.scroll_to_current = false;

        for p in want {
            self.thumbs.insert(p.clone(), Thumb::Pending);
            self.workers.load_thumb(p, THUMB_DECODE);
        }
        if self.thumbs.len() > MAX_THUMBS {
            let cutoff = self.frame_no.saturating_sub(2);
            self.thumbs.retain(|_, t| match t {
                Thumb::Ready(_, seen) => *seen >= cutoff,
                _ => true,
            });
        }

        if let Some((i, mods, double)) = clicked {
            let is_dir = self.files.get(i).is_some_and(|e| e.is_dir);
            if mods.command {
                if !self.selection.insert(i) {
                    self.selection.remove(&i);
                }
                self.anchor = Some(i);
            } else if mods.shift {
                let a = self.anchor.unwrap_or(i);
                self.selection = (a.min(i)..=a.max(i)).filter(|j| self.files.get(*j).is_some_and(|e| !e.is_dir)).collect();
            } else if is_dir {
                self.selection.clear();
                self.selection.insert(i);
                if double {
                    self.go(i);
                }
            } else {
                self.selection.clear();
                self.selection.insert(i);
                if self.current != Some(i) {
                    self.go(i);
                }
                if double {
                    self.s.center = crate::settings::Center::Viewer;
                }
            }
        }
        if let Some((i, a)) = action {
            if !self.selection.contains(&i) {
                self.selection.clear();
                self.selection.insert(i);
            }
            let path = self.files.get(i).map(|e| e.path.clone());
            let ctx = ui.ctx().clone();
            match (a, path) {
                ("open", _) => self.go(i),
                ("newtab", Some(p)) => {
                    self.new_tab();
                    self.navigate(&p, false);
                }
                ("batch", _) => self.open_batch(false),
                ("rotr", _) => self.quick_rotate(&ctx, Rotate::Cw90),
                ("rotl", _) => self.quick_rotate(&ctx, Rotate::Ccw90),
                ("rename", Some(p)) => {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some((p, stem));
                }
                ("reveal", Some(p)) => crate::assoc::reveal(&p),
                ("delete", _) => self.confirm_delete = Some(self.selected_paths()),
                (r, _) if r.starts_with('r') => {
                    let v: u8 = r[1..].parse().unwrap_or(0);
                    self.set_rating(v);
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- dialogs

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((path, mut name)) = self.rename.take() {
            let mut keep = true;
            let mut commit = false;
            egui::Window::new("Rename").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
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
                    self.lib.rename(&path, &dest);
                    for e in self.files.iter_mut().chain(self.all.iter_mut()) {
                        if e.path == path {
                            e.path = dest.clone();
                        }
                    }
                    if let Some(img) = &mut self.img
                        && img.path == path
                    {
                        img.path = dest.clone();
                    }
                    self.thumbs.remove(&path);
                }
            } else if keep {
                self.rename = Some((path, name));
            }
        }

        if let Some(paths) = self.confirm_delete.take() {
            let mut keep = true;
            egui::Window::new("Move to recycle bin?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                if paths.len() == 1 {
                    ui.label(file_name(&paths[0]));
                } else {
                    ui.label(format!("{} files", paths.len()));
                }
                ui.horizontal(|ui| {
                    if ui.button("Move to recycle bin").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        let mut failed = 0;
                        for p in &paths {
                            if trash::delete(p).is_err() {
                                failed += 1;
                            }
                        }
                        keep = false;
                        let next = self.current;
                        let removed: Vec<PathBuf> = paths.clone();
                        self.all.retain(|e| !removed.contains(&e.path));
                        self.apply_filter();
                        if let Some(i) = next {
                            let imgs = self.image_indices();
                            if let Some(&j) = imgs.iter().find(|&&j| j >= i).or(imgs.last()) {
                                self.go(j);
                            } else {
                                self.img = None;
                            }
                        }
                        if failed > 0 {
                            self.toast(ctx, format!("{failed} file(s) could not be moved"));
                        }
                    }
                    if ui.button("Cancel").clicked() {
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
                    let has_look = !self.dev.is_identity();
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
                            ui.label("Look");
                            let mut use_look = j.develop.is_some();
                            ui.add_enabled_ui(has_look, |ui| {
                                if ui.checkbox(&mut use_look, "Apply the current Develop settings").changed() {
                                    j.develop = if use_look { Some(self.dev.clone()) } else { None };
                                }
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
                                    ui.label(RichText::new(format!("→ {}", crate::batch::expand_name(j.rename.trim(), &stem, "", j.start_number))).small().color(theme::ACCENT));
                                }
                            });
                            ui.end_row();
                            ui.label("Output");
                            ui.vertical(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut j.subfolder).hint_text("subfolder (empty = overwrite in place)"));
                                if j.subfolder.trim().is_empty() && j.changes_pixels() {
                                    ui.label(RichText::new("Originals will be replaced. JPEGs are re-encoded at the quality above.").small().color(theme::ACCENT));
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

        if let Some((mut name, mut cat)) = self.save_preset.take() {
            let mut keep = true;
            let mut save = false;
            egui::Window::new("Save as new preset").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("Name");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                r.request_focus();
                ui.label("Category");
                egui::ComboBox::from_id_salt("pcat").selected_text(cat.clone()).show_ui(ui, |ui| {
                    for c in crate::presets::CATEGORY_ORDER {
                        ui.selectable_value(&mut cat, c.to_string(), *c);
                    }
                });
                if let Some(from) = &self.active_preset {
                    ui.label(RichText::new(format!("Forked from \"{from}\"")).small().color(theme::DIM));
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        keep = false;
                    }
                });
            });
            if save && !name.trim().is_empty() {
                let note = match &self.active_preset {
                    Some(f) => format!("Forked from \"{f}\""),
                    None => "Your preset".into(),
                };
                let mut dev = self.dev.clone();
                dev.amount = 1.0;
                let p = crate::presets::Preset { name: name.trim().to_string(), category: cat.clone(), note, dev };
                self.user_presets.retain(|q| q.name != p.name);
                self.user_presets.insert(0, p);
                let msg = match crate::presets::save_user(&self.user_presets) {
                    Ok(()) => format!("Saved preset \"{}\"", name.trim()),
                    Err(e) => format!("Could not save preset: {e}"),
                };
                self.active_preset = Some(name.trim().to_string());
                self.preset_cat = Some(cat);
                self.preset_thumb_req_clear();
                self.toast(ctx, msg);
            } else if keep && !save {
                self.save_preset = Some((name, cat));
            }
        }

        if self.show_help {
            egui::Window::new("Shortcuts").collapsible(false).resizable(false).open(&mut self.show_help).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                egui::Grid::new("keys").num_columns(2).spacing([18.0, 3.0]).show(ui, |ui| {
                    for (k, d) in [
                        ("← → / Space", "previous / next image"),
                        ("↑ ↓", "move by row in the grid"),
                        ("Alt+← → ↑ / Backspace", "back / forward / up a folder"),
                        ("Enter, double-click", "open folder / switch to the viewer"),
                        ("V", "grid → split → viewer"),
                        ("Ctrl+T / Ctrl+W", "new tab / close tab"),
                        ("Ctrl+0…5", "star rating"),
                        ("Wheel, double-click", "zoom, fit ↔ 100%  (Ctrl+wheel resizes thumbnails)"),
                        ("F / Z", "fit / actual size"),
                        ("0 – 8", "art views: original, B&W, values, notan, grisaille, simplify, blocks, paint-by-number, line art"),
                        ("S", "split compare original ↔ edit (drag the divider)"),
                        ("G / Shift+G", "rule-of-thirds / golden-ratio overlay"),
                        ("M / Q / T", "mirror / squint / always on top"),
                        ("C", "crop tool (drag on the image)"),
                        ("I, Ctrl+click", "pipette: sample a colour and get its mix"),
                        ("Ctrl+Shift+C / V", "copy / paste develop settings"),
                        ("Tab", "hide all panels"),
                        ("F11", "full screen"),
                        ("Ctrl+R / Ctrl+L", "rotate file(s) right / left"),
                        ("F2 / Del / F5", "rename / recycle bin / refresh"),
                        ("Ctrl+B", "batch edit selection"),
                        ("Ctrl+E", "save an edited copy"),
                        ("Right-click", "panel tabs move panels; everything else"),
                    ] {
                        ui.label(RichText::new(k).monospace().color(theme::ACCENT));
                        ui.label(d);
                        ui.end_row();
                    }
                });
            });
        }
    }

    pub fn preset_thumb_req_clear(&mut self) {
        self.preset_thumbs.clear();
        self.reset_preset_req();
    }
}

