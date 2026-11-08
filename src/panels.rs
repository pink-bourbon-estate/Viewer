//! Develop (presets + adjustments + curves), Colours, Composition, Art views
//! and Export panels.

use crate::app::{color32, file_name, App, Tool};
use crate::batch::{BatchJob, OutFormat};
use crate::color::{describe, hex, lightness, rgb_to_hsv};
use crate::develop::{curve_lut, Develop, BANDS};
use crate::filters::Mode;
use crate::palette::{Recipe, Solver};
use crate::presets::{category_colors, CATEGORY_ORDER};
use crate::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportTarget {
    Copy,
    Overwrite,
    SaveAs,
}

fn sl(ui: &mut Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str) -> bool {
    let r = ui.add(egui::Slider::new(v, range).text(label).fixed_decimals(if label == "Exposure" { 2 } else { 0 }));
    let mut changed = r.changed();
    if r.double_clicked() {
        *v = 0.0;
        changed = true;
    }
    changed
}

impl App {
    // ---------------------------------------------------------------- Develop

    fn preset_cards(&mut self, ui: &mut Ui) {
        let all = self.all_presets();
        let cats: Vec<&str> = CATEGORY_ORDER.iter().copied().filter(|c| all.iter().any(|p| p.category == *c)).collect();
        let w = ((ui.available_width() - 6.0) / 2.0).max(60.0);
        let h = 58.0;
        let mut open = None;
        egui::Grid::new("cats").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, c) in cats.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                let (bg, fg) = category_colors(c);
                let p = ui.painter();
                p.rect_filled(rect, 8.0, color32(bg));
                // A diagonal band, like a film box.
                let band = [rect.left_bottom(), rect.left_bottom() + vec2(w * 0.28, 0.0), rect.left_top() + vec2(w * 0.5, 0.0), rect.left_top() + vec2(w * 0.22, 0.0)];
                p.add(egui::Shape::convex_polygon(band.to_vec(), Color32::from_white_alpha(28), Stroke::NONE));
                if resp.hovered() {
                    p.rect_stroke(rect, 8.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
                }
                let n = all.iter().filter(|p| p.category == *c).count();
                p.text(rect.center() - vec2(0.0, 7.0), Align2::CENTER_CENTER, *c, FontId::proportional(14.5), color32(fg));
                p.text(rect.center() + vec2(0.0, 11.0), Align2::CENTER_CENTER, format!("{n} presets"), FontId::proportional(10.5), color32(fg).gamma_multiply(0.85));
                if resp.clicked() {
                    open = Some(c.to_string());
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        if let Some(c) = open {
            self.preset_cat = Some(c);
        }
    }

    fn preset_list(&mut self, ui: &mut Ui, cat: &str) {
        self.want_preset_thumbs(cat);
        let list: Vec<crate::presets::Preset> = self.all_presets().into_iter().filter(|p| p.category == cat).collect();
        let w = ((ui.available_width() - 6.0) / 2.0).max(60.0);
        let mut apply = None;
        let mut delete = None;
        egui::Grid::new("plist").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, pr) in list.iter().enumerate() {
                let th = w * 0.66;
                let (rect, resp) = ui.allocate_exact_size(vec2(w, th + 18.0), Sense::click());
                let p = ui.painter();
                let img_r = Rect::from_min_size(rect.min, vec2(w, th));
                match self.preset_thumbs.get(&pr.name) {
                    Some(t) => {
                        let ts = t.size_vec2();
                        let s = (w / ts.x).min(th / ts.y);
                        p.rect_filled(img_r, 4.0, theme::CANVAS);
                        p.image(t.id(), Rect::from_center_size(img_r.center(), ts * s), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        let (bg, _) = category_colors(cat);
                        p.rect_filled(img_r, 4.0, color32(bg).gamma_multiply(0.5));
                    }
                }
                let active = self.active_preset.as_deref() == Some(pr.name.as_str());
                if active || resp.hovered() {
                    p.rect_stroke(img_r, 4.0, Stroke::new(if active { 2.0 } else { 1.0 }, if active { theme::ACCENT } else { Color32::WHITE }), egui::StrokeKind::Inside);
                }
                let g = p.layout_no_wrap(pr.name.clone(), FontId::proportional(11.0), if active { theme::ACCENT } else { theme::TEXT });
                let lr = Rect::from_min_max(pos2(rect.min.x, img_r.max.y + 2.0), rect.max);
                p.with_clip_rect(lr).galley(pos2(lr.min.x + 2.0, lr.min.y), g, theme::TEXT);
                let resp = resp.on_hover_text(if pr.note.is_empty() { pr.name.clone() } else { format!("{}\n{}", pr.name, pr.note) });
                if resp.clicked() {
                    apply = Some(pr.clone());
                }
                let user = self.is_user_preset(&pr.name);
                resp.context_menu(|ui| {
                    if ui.button("Apply").clicked() {
                        apply = Some(pr.clone());
                        ui.close();
                    }
                    if user && ui.button("Delete preset").clicked() {
                        delete = Some(pr.name.clone());
                        ui.close();
                    }
                    if !user {
                        ui.label(RichText::new("Built-in — apply it, adjust, then Save as new preset to fork it.").small().color(theme::DIM));
                    }
                });
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        if let Some(pr) = apply {
            let mut d = pr.dev.clone();
            d.amount = 1.0;
            self.dev = d;
            self.active_preset = Some(pr.name);
            self.dev_changed();
        }
        if let Some(n) = delete {
            self.user_presets.retain(|p| p.name != n);
            let _ = crate::presets::save_user(&self.user_presets);
        }
    }

    fn curve_editor(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (i, (name, c)) in [("RGB", theme::TEXT), ("Red", Color32::from_rgb(230, 90, 90)), ("Green", Color32::from_rgb(110, 200, 110)), ("Blue", Color32::from_rgb(110, 150, 240))].iter().enumerate() {
                if ui.selectable_label(self.curve_channel == i, RichText::new(*name).color(*c)).clicked() {
                    self.curve_channel = i;
                }
            }
            if ui.small_button("Reset").clicked() {
                self.curve_points_mut().clear();
                self.dev_changed();
            }
        });
        let size = ui.available_width().min(260.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 3.0, theme::CANVAS);
        // Histogram behind.
        if let Some(h) = &self.hist {
            let bins = match self.curve_channel {
                1 => &h.r,
                2 => &h.g,
                3 => &h.b,
                _ => &h.l,
            };
            let max = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
            for (i, &v) in bins.iter().enumerate() {
                let x = rect.min.x + i as f32 / 255.0 * rect.width();
                let hgt = (v as f32 / max).sqrt() * rect.height();
                p.line_segment([pos2(x, rect.max.y), pos2(x, rect.max.y - hgt)], Stroke::new(1.5, Color32::from_white_alpha(18)));
            }
        }
        for k in 1..4 {
            let f = k as f32 / 4.0;
            let st = Stroke::new(1.0, Color32::from_white_alpha(18));
            p.line_segment([pos2(rect.min.x + f * rect.width(), rect.min.y), pos2(rect.min.x + f * rect.width(), rect.max.y)], st);
            p.line_segment([pos2(rect.min.x, rect.min.y + f * rect.height()), pos2(rect.max.x, rect.min.y + f * rect.height())], st);
        }
        p.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_white_alpha(30)));
        let colour = [theme::TEXT, Color32::from_rgb(230, 90, 90), Color32::from_rgb(110, 200, 110), Color32::from_rgb(110, 150, 240)][self.curve_channel.min(3)];
        let to_screen = |x: f32, y: f32| pos2(rect.min.x + x / 255.0 * rect.width(), rect.max.y - y / 255.0 * rect.height());
        let to_val = |q: Pos2| (((q.x - rect.min.x) / rect.width() * 255.0).clamp(0.0, 255.0), ((rect.max.y - q.y) / rect.height() * 255.0).clamp(0.0, 255.0));

        let mut pts = self.curve_points_mut().clone();
        if pts.len() < 2 {
            pts = vec![[0.0, 0.0], [255.0, 255.0]];
        }
        if let Some(l) = curve_lut(&pts) {
            let line: Vec<Pos2> = (0..=64).map(|i| {
                let x = i * 4;
                let x = x.min(255);
                to_screen(x as f32, l[x] * 255.0)
            }).collect();
            p.add(egui::Shape::line(line, Stroke::new(1.8, colour)));
        } else {
            p.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.8, colour));
        }
        for q in &pts {
            p.circle(to_screen(q[0], q[1]), 4.0, theme::CANVAS, Stroke::new(1.5, colour));
        }

        // Interaction: drag nearest point; click empty to add; right/double-click removes.
        let id = ui.scope_id().with("curve_drag");
        let mut drag: Option<usize> = ui.data(|d| d.get_temp(id));
        let ptr = resp.interact_pointer_pos().or(resp.hover_pos());
        let nearest = |pts: &[[f32; 2]], q: Pos2| {
            pts.iter().enumerate().map(|(i, v)| (i, to_screen(v[0], v[1]).distance(q))).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
        };
        let mut changed = false;
        if resp.drag_started()
            && let Some(q) = ptr
        {
            drag = nearest(&pts, q).or_else(|| {
                let (x, y) = to_val(q);
                pts.push([x, y]);
                pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                pts.iter().position(|v| v[0] == x && v[1] == y)
            });
            changed = true;
        }
        if resp.dragged()
            && let (Some(i), Some(q)) = (drag, ptr)
        {
            let (mut x, y) = to_val(q);
            let n = pts.len();
            if i == 0 {
                x = x.min(pts.get(1).map(|v| v[0] - 2.0).unwrap_or(255.0));
            } else if i + 1 == n {
                x = x.max(pts[i - 1][0] + 2.0);
            } else {
                x = x.clamp(pts[i - 1][0] + 2.0, pts[i + 1][0] - 2.0);
            }
            if let Some(v) = pts.get_mut(i) {
                *v = [x, y];
            }
            changed = true;
        }
        if resp.drag_stopped() {
            drag = None;
        }
        if resp.clicked()
            && let Some(q) = ptr
            && nearest(&pts, q).is_none()
        {
            let (x, _) = to_val(q);
            let y = curve_lut(&pts).map(|l| l[x as usize] * 255.0).unwrap_or(x);
            pts.push([x, y]);
            pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
            changed = true;
        }
        if (resp.secondary_clicked() || resp.double_clicked())
            && let Some(q) = ptr
            && let Some(i) = nearest(&pts, q)
            && i != 0
            && i + 1 != pts.len()
        {
            pts.remove(i);
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(id, drag));
        if changed {
            *self.curve_points_mut() = pts;
            self.dev_changed();
        }
        ui.label(RichText::new("Drag to shape · click to add a point · right-click removes").small().color(theme::DIM));
    }

    fn curve_points_mut(&mut self) -> &mut Vec<[f32; 2]> {
        match self.curve_channel {
            1 => &mut self.dev.curve_r,
            2 => &mut self.dev.curve_g,
            3 => &mut self.dev.curve_b,
            _ => &mut self.dev.curve,
        }
    }

    pub fn panel_develop(&mut self, ui: &mut Ui) {
        if self.img.is_none() {
            ui.label(RichText::new("Open an image to develop it.").color(theme::DIM));
        }
        ui.horizontal(|ui| {
            match self.preset_cat.clone() {
                Some(c) => {
                    if ui.button("‹ Looks").clicked() {
                        self.preset_cat = None;
                    }
                    ui.label(RichText::new(c).strong());
                }
                None => {
                    ui.label(RichText::new("Looks").strong());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Original").on_hover_text("Remove all develop edits").clicked() {
                    self.dev = Develop::default();
                    self.active_preset = None;
                    self.dev_changed();
                }
            });
        });
        match self.preset_cat.clone() {
            Some(c) => self.preset_list(ui, &c),
            None => self.preset_cards(ui),
        }
        if let Some(n) = &self.active_preset {
            ui.label(RichText::new(format!("Look: {n}")).color(theme::ACCENT));
            let mut a = self.dev.amount * 100.0;
            if ui.add(egui::Slider::new(&mut a, 0.0..=100.0).text("strength %").fixed_decimals(0)).changed() {
                self.dev.amount = a / 100.0;
                self.dev_changed();
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Save as new preset…").on_hover_text("Fork the current settings into your own preset").clicked() {
                let base = self.active_preset.clone().map(|n| format!("{n} (mine)")).unwrap_or_else(|| "My look".into());
                self.save_preset = Some((base, "My presets".into()));
            }
            if ui.button("Copy").on_hover_text("Copy develop settings (Ctrl+Shift+C)").clicked() {
                self.copied_dev = Some(self.dev.clone());
            }
            if ui.add_enabled(self.copied_dev.is_some(), egui::Button::new("Paste")).on_hover_text("Paste develop settings (Ctrl+Shift+V)").clicked()
                && let Some(d) = self.copied_dev.clone()
            {
                self.dev = d;
                self.dev_changed();
            }
        });
        ui.separator();

        let before = self.dev.clone();
        let d = &mut self.dev;
        egui::CollapsingHeader::new("Light").default_open(true).show(ui, |ui| {
            sl(ui, &mut d.exposure, -4.0..=4.0, "Exposure");
            sl(ui, &mut d.contrast, -100.0..=100.0, "Contrast");
            sl(ui, &mut d.highlights, -100.0..=100.0, "Highlights");
            sl(ui, &mut d.shadows, -100.0..=100.0, "Shadows");
            sl(ui, &mut d.whites, -100.0..=100.0, "Whites");
            sl(ui, &mut d.blacks, -100.0..=100.0, "Blacks");
        });
        egui::CollapsingHeader::new("Colour").default_open(true).show(ui, |ui| {
            sl(ui, &mut d.temp, -100.0..=100.0, "Temperature");
            sl(ui, &mut d.tint, -100.0..=100.0, "Tint");
            sl(ui, &mut d.vibrance, -100.0..=100.0, "Vibrance");
            sl(ui, &mut d.saturation, -100.0..=100.0, "Saturation");
        });
        egui::CollapsingHeader::new("Effects").show(ui, |ui| {
            sl(ui, &mut d.clarity, -100.0..=100.0, "Clarity");
            sl(ui, &mut d.dehaze, -100.0..=100.0, "Dehaze");
            sl(ui, &mut d.fade, 0.0..=100.0, "Fade");
            sl(ui, &mut d.vignette, -100.0..=100.0, "Vignette");
            sl(ui, &mut d.grain, 0.0..=100.0, "Grain");
            ui.add(egui::Slider::new(&mut d.grain_size, 5.0..=100.0).text("Grain size"));
        });
        if self.dev != before {
            self.dev_changed();
        }
        egui::CollapsingHeader::new("Curves").default_open(true).show(ui, |ui| self.curve_editor(ui));
        let before = self.dev.clone();
        let tab = self.hsl_tab;
        let d = &mut self.dev;
        let mut new_tab = tab;
        egui::CollapsingHeader::new("Colour mixer (HSL)").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, n) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                    if ui.selectable_label(tab == i, *n).clicked() {
                        new_tab = i;
                    }
                }
            });
            let arr = match tab {
                1 => &mut d.sat,
                2 => &mut d.lum,
                _ => &mut d.hue,
            };
            for (i, b) in BANDS.iter().enumerate() {
                sl(ui, &mut arr[i], -100.0..=100.0, b);
            }
        });
        egui::CollapsingHeader::new("Colour grading").show(ui, |ui| {
            ui.label(RichText::new("Shadows").small().color(theme::DIM));
            ui.add(egui::Slider::new(&mut d.split_sh_hue, 0.0..=360.0).text("hue"));
            sl(ui, &mut d.split_sh_sat, 0.0..=100.0, "amount");
            ui.label(RichText::new("Midtones").small().color(theme::DIM));
            ui.add(egui::Slider::new(&mut d.split_mid_hue, 0.0..=360.0).text("hue"));
            sl(ui, &mut d.split_mid_sat, 0.0..=100.0, "amount");
            ui.label(RichText::new("Highlights").small().color(theme::DIM));
            ui.add(egui::Slider::new(&mut d.split_hi_hue, 0.0..=360.0).text("hue"));
            sl(ui, &mut d.split_hi_sat, 0.0..=100.0, "amount");
            sl(ui, &mut d.split_balance, -100.0..=100.0, "balance");
        });
        egui::CollapsingHeader::new("Black & white").show(ui, |ui| {
            ui.checkbox(&mut d.grayscale, "Convert to black & white");
            if d.grayscale {
                for (i, b) in BANDS.iter().enumerate() {
                    sl(ui, &mut d.gray_mix[i], -100.0..=100.0, b);
                }
            }
        });
        egui::CollapsingHeader::new("Calibration").show(ui, |ui| {
            for (i, n) in ["Red", "Green", "Blue"].iter().enumerate() {
                sl(ui, &mut d.cal_hue[i], -100.0..=100.0, &format!("{n} hue"));
                sl(ui, &mut d.cal_sat[i], -100.0..=100.0, &format!("{n} saturation"));
            }
            sl(ui, &mut d.shadow_tint, -100.0..=100.0, "Shadow tint");
        });
        self.hsl_tab = new_tab;
        if self.dev != before {
            self.dev_changed();
        }
        ui.label(RichText::new("Double-click a slider to reset it.").small().color(theme::DIM));
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let busy = self.importing;
            if ui
                .add_enabled(!busy, egui::Button::new(if busy { "Importing…" } else { "Import preset folder…" }))
                .on_hover_text("Read a folder of Lightroom presets (.xmp, .lrtemplate) and .cube LUTs, group similar looks and keep about 45 averaged presets")
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.importing = true;
                self.workers.import_presets(dir, 45);
            }
        });
    }

    // ---------------------------------------------------------------- Colours

    fn swatch_card(ui: &mut Ui, solver: &Solver, n: Option<usize>, sw_rgb: [u8; 3], share: Option<f32>, r: &Recipe, active: bool, compact: bool) -> egui::Response {
        let frame = egui::Frame::new()
            .fill(if active { theme::SELECT } else { theme::CARD })
            .stroke(if active { Stroke::new(1.0, theme::ACCENT) } else { Stroke::NONE })
            .corner_radius(4.0)
            .inner_margin(5.0);
        let mut swatch_resp = None;
        let out = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let s = if compact { 30.0 } else { 44.0 };
                let (rect, resp) = ui.allocate_exact_size(vec2(s, s), Sense::click());
                let p = ui.painter();
                p.rect_filled(rect, 3.0, color32(sw_rgb));
                let mix_r = Rect::from_min_max(rect.center(), rect.max);
                p.rect_filled(mix_r, egui::CornerRadius { nw: 3, ne: 0, sw: 0, se: 3 }, color32(r.predicted));
                p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_black_alpha(80)), egui::StrokeKind::Inside);
                if let Some(n) = n {
                    let txt = if lightness(sw_rgb) > 55.0 { Color32::from_gray(20) } else { Color32::from_gray(235) };
                    p.text(rect.min + vec2(3.0, 1.0), Align2::LEFT_TOP, n.to_string(), FontId::proportional(10.5), txt);
                }
                swatch_resp = Some(resp.on_hover_text("Square: the colour in the image. Corner: what the recipe mixes to. Click to isolate."));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(hex(sw_rgb)).monospace().strong());
                        if let Some(s) = share {
                            ui.label(RichText::new(format!("{:.0}%", s * 100.0)).color(theme::DIM));
                        }
                        ui.label(RichText::new(format!("value {:.1}", lightness(sw_rgb) / 10.0)).color(theme::ACCENT));
                    });
                    if compact {
                        if let Some(l) = r.describe(&solver.palette).first() {
                            ui.label(RichText::new(l).small());
                        }
                    } else {
                        let hsv = rgb_to_hsv(sw_rgb);
                        ui.label(RichText::new(format!("HSV {:.0}° {:.0}% {:.0}%   L* {:.0}", hsv[0], hsv[1], hsv[2], lightness(sw_rgb))).small().color(theme::DIM));
                        ui.label(RichText::new(describe(sw_rgb)).small().italics());
                    }
                });
            });
            if !compact {
                for line in r.describe(&solver.palette) {
                    ui.label(RichText::new(line).small());
                }
                ui.label(RichText::new(format!("ΔE {:.1} · {}", r.delta_e, r.accuracy_word())).small().color(if r.delta_e < 5.0 { theme::DIM } else { theme::ACCENT }));
            }
        });
        let card = out.response.interact(Sense::click());
        match swatch_resp {
            Some(s) => s.union(card),
            None => card,
        }
    }

    pub fn panel_colours(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let solver = self.workers.solver();
        let compact = self.s.compact_cards;
        ui.horizontal(|ui| {
            if ui.selectable_label(self.pick_mode, "Pipette (I)").clicked() {
                self.pick_mode = !self.pick_mode;
            }
            ui.checkbox(&mut self.s.compact_cards, "Compact");
        });
        if !self.pinned.is_empty() {
            egui::CollapsingHeader::new(format!("Sampled ({})", self.pinned.len())).default_open(true).show(ui, |ui| {
                if ui.small_button("Clear all").clicked() {
                    self.pinned.clear();
                }
                let mut remove = None;
                for (i, (rgb, r)) in self.pinned.iter().enumerate() {
                    let resp = Self::swatch_card(ui, &solver, None, *rgb, None, r, false, compact);
                    resp.context_menu(|ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                            ui.close();
                        }
                        if ui.button("Copy hex").clicked() {
                            ui.ctx().copy_text(hex(*rgb));
                            ui.close();
                        }
                    });
                    if resp.middle_clicked() {
                        remove = Some(i);
                    }
                }
                if let Some(i) = remove {
                    self.pinned.remove(i);
                }
            });
        }
        egui::CollapsingHeader::new("Colours in this image").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut k = self.s.colours;
                ui.label("Count");
                if ui.add(egui::Slider::new(&mut k, 2..=24)).changed() {
                    self.set_colours(k);
                }
                if self.isolate.is_some() && ui.small_button("Show all").clicked() {
                    self.isolate = None;
                    self.proc_dirty = true;
                }
            });
            let mut toggle = None;
            match &self.analysis {
                Some(a) => {
                    for (i, (sw, r)) in a.swatches.iter().zip(&a.recipes).enumerate() {
                        let resp = Self::swatch_card(ui, &solver, Some(i + 1), sw.rgb, Some(sw.share), r, self.isolate == Some(i), compact);
                        if resp.clicked() {
                            toggle = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copy hex").clicked() {
                                ui.ctx().copy_text(hex(sw.rgb));
                                ui.close();
                            }
                            if ui.button("Copy recipe").clicked() {
                                ui.ctx().copy_text(r.describe(&solver.palette).join("\n"));
                                ui.close();
                            }
                        });
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
            if let Some(i) = toggle {
                self.isolate = if self.isolate == Some(i) { None } else { Some(i) };
                self.proc_dirty = true;
            }
        });
        egui::CollapsingHeader::new("Mixing palette").show(ui, |ui| {
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
                if ui.button("Edit palette…").clicked() {
                    match crate::settings::ensure_palette_file() {
                        Ok(p) => crate::assoc::open_external(&p),
                        Err(e) => self.toast(&ctx, e),
                    }
                }
                if ui.button("Reload").clicked() {
                    self.reload_palette(&ctx);
                }
                if ui.button("Export card").on_hover_text("Palette card PNG + recipes .txt").clicked() {
                    self.export_palette(&ctx);
                }
            });
            ui.label(RichText::new("Kubelka–Munk mixing model: treat recipes as the starting pile, then adjust by eye.").small().color(theme::DIM));
        });
    }

    fn export_palette(&mut self, ctx: &egui::Context) {
        let (Some(img), Some(a)) = (&self.img, &self.analysis) else { return };
        let stem = img.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("palette".into());
        let Some(dest) = rfd::FileDialog::new().set_file_name(format!("{stem}_palette.png")).add_filter("PNG", &["png"]).save_file() else { return };
        let solver = self.workers.solver();
        let card = crate::export::palette_card(&a.swatches, &a.recipes);
        let text = crate::export::recipes_text(&a.swatches, &a.recipes, &solver, &file_name(&img.path));
        let r = card.save(&dest).map_err(|e| e.to_string()).and_then(|_| std::fs::write(dest.with_extension("txt"), text).map_err(|e| e.to_string()));
        let msg = match r {
            Ok(()) => format!("Saved {} + recipes .txt", file_name(&dest)),
            Err(e) => format!("Export failed: {e}"),
        };
        self.toast(ctx, msg);
    }

    // ---------------------------------------------------------------- Composition

    pub fn panel_composition(&mut self, ui: &mut Ui) {
        theme::section_label(ui, "Histogram");
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 90.0), Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 3.0, theme::CANVAS);
        if let Some(h) = &self.hist {
            let max = h.r.iter().chain(&h.g).chain(&h.b).copied().max().unwrap_or(1).max(1) as f32;
            for (bins, c) in [(&h.r, Color32::from_rgba_unmultiplied(230, 70, 70, 90)), (&h.g, Color32::from_rgba_unmultiplied(80, 200, 80, 90)), (&h.b, Color32::from_rgba_unmultiplied(80, 120, 240, 90))] {
                let pts: Vec<Pos2> = std::iter::once(rect.left_bottom())
                    .chain(bins.iter().enumerate().map(|(i, &v)| pos2(rect.min.x + i as f32 / 255.0 * w, rect.max.y - (v as f32 / max).sqrt() * rect.height())))
                    .chain(std::iter::once(rect.right_bottom()))
                    .collect();
                p.add(egui::Shape::line(pts, Stroke::new(1.0, c)));
            }
            let lmax = h.l.iter().copied().max().unwrap_or(1).max(1) as f32;
            for (i, &v) in h.l.iter().enumerate() {
                let x = rect.min.x + i as f32 / 255.0 * w;
                p.line_segment([pos2(x, rect.max.y), pos2(x, rect.max.y - (v as f32 / lmax).sqrt() * rect.height())], Stroke::new(1.2, Color32::from_white_alpha(40)));
            }
            let total: u32 = h.l.iter().sum::<u32>().max(1);
            let lo = h.l[..3].iter().sum::<u32>() as f32 / total as f32 * 100.0;
            let hi = h.l[253..].iter().sum::<u32>() as f32 / total as f32 * 100.0;
            ui.label(RichText::new(format!("Clipped: shadows {lo:.1}%  ·  highlights {hi:.1}%")).small().color(if lo > 2.0 || hi > 2.0 { theme::ACCENT } else { theme::DIM }));
        }
        // Value zones 0-10 under the histogram.
        let (zr, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
        for v in 0..=10 {
            let zw = w / 11.0;
            let r = Rect::from_min_size(pos2(zr.min.x + v as f32 * zw, zr.min.y), vec2(zw, zr.height()));
            let g = crate::color::to_srgb(crate::color::lstar_to_y(v as f32 * 10.0));
            ui.painter().rect_filled(r, 0.0, Color32::from_gray(g));
        }

        theme::section_label(ui, "Overlays");
        let o = &mut self.s.overlays;
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut o.thirds, "Thirds (G)");
            ui.toggle_value(&mut o.golden, "Golden ratio");
            ui.toggle_value(&mut o.centre, "Centre");
            ui.toggle_value(&mut o.quarters, "Quarters");
            ui.toggle_value(&mut o.diagonals, "Diagonals");
            ui.toggle_value(&mut o.armature, "Dynamic symmetry");
            ui.toggle_value(&mut o.squares, "Square grid");
        });
        if o.squares {
            ui.add(egui::Slider::new(&mut o.squares_n, 2..=40).text("squares across"));
        }
        ui.add(egui::Slider::new(&mut o.opacity, 0.1..=1.0).text("opacity"));
        ui.horizontal(|ui| {
            ui.label("Colour");
            for c in [[235, 200, 140], [255, 255, 255], [20, 20, 20], [90, 220, 255], [255, 80, 160]] {
                let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                ui.painter().rect_filled(r, 3.0, color32(c));
                if o.colour == c {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Outside);
                }
                if resp.clicked() {
                    o.colour = c;
                }
            }
        });
        ui.label(RichText::new("Overlays follow the crop when one is set.").small().color(theme::DIM));

        theme::section_label(ui, "Seeing");
        let before = self.s.params.squint;
        ui.add(egui::Slider::new(&mut self.s.params.squint, 0..=20).text("squint blur (Q)"));
        if self.s.params.squint != before {
            self.proc_dirty = true;
        }
        ui.checkbox(&mut self.mirror, "Mirror (M) — fresh eyes on your drawing");
        let ctx = ui.ctx().clone();
        let mut split = self.s.split;
        if ui.checkbox(&mut split, "Split compare (S)").changed() {
            self.toggle_split(&ctx);
        }
    }

    // ---------------------------------------------------------------- Art views

    pub fn panel_art(&mut self, ui: &mut Ui) {
        for m in Mode::ALL {
            if ui.radio(self.s.mode == m, format!("{}  {}", m.key(), m.label())).clicked() {
                self.set_mode(m);
            }
        }
        ui.separator();
        let before = self.s.params.clone();
        let p = &mut self.s.params;
        match self.s.mode {
            Mode::Original => {
                ui.label(RichText::new("Pick a view above or press 1–8.").small().color(theme::DIM));
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
                ui.label(RichText::new("Fewer colours (Colours panel) = bigger, blockier masses.").small().color(theme::DIM));
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
        if self.s.params != before {
            self.proc_dirty = true;
        }
        if matches!(self.s.mode, Mode::Simplify | Mode::PaintByNumber) {
            let mut k = self.s.colours;
            if ui.add(egui::Slider::new(&mut k, 2..=24).text("colours")).changed() {
                self.set_colours(k);
            }
        }
        ui.label(RichText::new("Art views apply on top of any Develop look.").small().color(theme::DIM));
        if ui.button("Export this view as PNG").clicked() {
            let ctx = ui.ctx().clone();
            self.export_study(&ctx);
        }
    }

    fn export_study(&mut self, ctx: &egui::Context) {
        let Some(img) = &self.img else { return };
        let stem = img.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("study".into());
        let mode = self.s.mode.label().split_whitespace().next().unwrap_or("view").to_lowercase();
        let Some(dest) = rfd::FileDialog::new().set_file_name(format!("{stem}_{mode}.png")).add_filter("PNG", &["png"]).save_file() else { return };
        let (w, h) = img.full.dimensions();
        let out = match &self.processed {
            Some(st) => crate::export::study_full_size(&st.image, &st.labels, w, h),
            None => (*img.full).clone(),
        };
        let msg = match out.save(&dest) {
            Ok(()) => format!("Saved {}", file_name(&dest)),
            Err(e) => format!("Export failed: {e}"),
        };
        self.toast(ctx, msg);
    }

    // ---------------------------------------------------------------- Export

    pub fn panel_export(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some((iw, ih)) = self.img.as_ref().map(|i| (i.size.x, i.size.y)) else {
            ui.label(RichText::new("Open an image to crop and export it.").color(theme::DIM));
            return;
        };
        theme::section_label(ui, "Crop & straighten");
        ui.horizontal(|ui| {
            if ui.selectable_label(self.tool == Tool::Crop, "✂ Crop (C)").clicked() {
                self.tool = if self.tool == Tool::Crop { Tool::None } else { Tool::Crop };
            }
            if ui.selectable_label(self.tool == Tool::Level, "📐 Level by line").on_hover_text("Drag along something that should be horizontal (or vertical)").clicked() {
                self.tool = if self.tool == Tool::Level { Tool::None } else { Tool::Level };
            }
        });
        let ratios: [(&str, Option<f32>); 9] = [
            ("Free", None),
            ("Original", Some(-1.0)),
            ("1:1", Some(1.0)),
            ("4:5", Some(0.8)),
            ("5:4", Some(1.25)),
            ("2:3", Some(2.0 / 3.0)),
            ("3:2", Some(1.5)),
            ("16:9", Some(16.0 / 9.0)),
            ("9:16", Some(9.0 / 16.0)),
        ];
        let cur = ratios.iter().find(|(_, r)| *r == self.crop_aspect).map(|(n, _)| *n).unwrap_or("Free");
        let mut new_aspect = None;
        egui::ComboBox::from_label("aspect").selected_text(cur).show_ui(ui, |ui| {
            for (n, r) in ratios {
                if ui.selectable_label(cur == n, n).clicked() {
                    new_aspect = Some(r);
                }
            }
        });
        if let Some(r) = new_aspect {
            self.crop_aspect = r;
            self.fit_crop_to_aspect(iw, ih);
        }
        let before = self.geo;
        let mut ang = self.geo.angle;
        if ui.add(egui::Slider::new(&mut ang, -45.0..=45.0).text("straighten °").fixed_decimals(1)).changed() {
            self.geo.angle = ang;
        }
        ui.horizontal(|ui| {
            if ui.button("⟲ 90°").clicked() {
                self.geo.quarter = (self.geo.quarter + 3) % 4;
                self.geo.crop = [0.0, 0.0, 1.0, 1.0];
            }
            if ui.button("⟳ 90°").clicked() {
                self.geo.quarter = (self.geo.quarter + 1) % 4;
                self.geo.crop = [0.0, 0.0, 1.0, 1.0];
            }
            if ui.button("Flip ↔").clicked() {
                self.geo.flip_h = !self.geo.flip_h;
            }
            if ui.button("Flip ↕").clicked() {
                self.geo.flip_v = !self.geo.flip_v;
            }
            if ui.button("Reset").clicked() {
                self.geo = crate::geom::Geometry::default();
            }
        });
        if self.geo != before {
            self.geo.constrain(iw, ih);
        }
        let (tw, th) = self.geo.turned(iw, ih);
        let c = self.geo.crop;
        ui.label(RichText::new(format!("Output {} × {} px", ((c[2] - c[0]) * tw).round(), ((c[3] - c[1]) * th).round())).small().color(theme::DIM));

        theme::section_label(ui, "Save");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("efmt").selected_text(self.s.export_format.label()).show_ui(ui, |ui| {
                for f in OutFormat::ALL {
                    ui.selectable_value(&mut self.s.export_format, f, f.label());
                }
            });
        });
        if matches!(self.s.export_format, OutFormat::Jpeg | OutFormat::Keep) {
            ui.add(egui::Slider::new(&mut self.s.export_quality, 40..=100).text("JPEG quality"));
        }
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.s.export_max_edge).range(0..=20000).suffix(" px"));
            ui.label(RichText::new("long edge (0 = full size)").small().color(theme::DIM));
        });
        ui.horizontal(|ui| {
            ui.label("Copy suffix");
            ui.add(egui::TextEdit::singleline(&mut self.s.export_suffix).desired_width(80.0));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Save copy").on_hover_text("Ctrl+E — writes name + suffix next to the original").clicked() {
                self.export_current(&ctx, ExportTarget::Copy);
            }
            if ui.button("Save as…").clicked() {
                self.export_current(&ctx, ExportTarget::SaveAs);
            }
            if ui.button("Overwrite original").on_hover_text("Replaces the file (the old one goes to the recycle bin if the format changes)").clicked() {
                self.export_current(&ctx, ExportTarget::Overwrite);
            }
        });
        let n = self.selected_paths().len();
        if n > 1 {
            ui.separator();
            if ui.button(format!("Apply this look to {n} selected & save copies")).clicked() {
                let job = BatchJob {
                    files: self.selected_paths(),
                    develop: Some(self.dev.clone()),
                    format: self.s.export_format,
                    jpeg_quality: self.s.export_quality,
                    max_edge: self.s.export_max_edge,
                    suffix: if self.s.export_suffix.trim().is_empty() { "_edit".into() } else { self.s.export_suffix.clone() },
                    ..BatchJob::default()
                };
                self.run_job(&ctx, job, "Saving");
            }
        }
        ui.add_space(4.0);
        if ui.button("Batch rename / convert…").clicked() {
            self.open_batch(false);
        }
    }

    pub fn fit_crop_to_aspect(&mut self, iw: f32, ih: f32) {
        let (tw, th) = self.geo.turned(iw, ih);
        let target = match self.crop_aspect {
            None => return,
            Some(r) if r < 0.0 => tw / th,
            Some(r) => r,
        };
        let s = self.geo.safe_scale(tw, th);
        // Largest box of the target ratio inside the safe area, centred.
        let (mut cw, mut ch) = (s, s * tw / th / target);
        if ch > s {
            ch = s;
            cw = s * target * th / tw;
        }
        self.geo.crop = [0.5 - cw / 2.0, 0.5 - ch / 2.0, 0.5 + cw / 2.0, 0.5 + ch / 2.0];
    }

    pub fn export_current(&mut self, ctx: &egui::Context, target: ExportTarget) {
        let Some(img) = &self.img else { return };
        let src = img.path.clone();
        let format = self.s.export_format;
        let mut job = BatchJob {
            files: vec![src.clone()],
            develop: Some(self.dev.clone()),
            geometry: Some(self.geo),
            format,
            jpeg_quality: self.s.export_quality,
            max_edge: self.s.export_max_edge,
            ..BatchJob::default()
        };
        match target {
            ExportTarget::Copy => {
                job.suffix = if self.s.export_suffix.trim().is_empty() { "_edit".into() } else { self.s.export_suffix.clone() };
            }
            ExportTarget::Overwrite => {
                if format == OutFormat::Keep || src.extension().is_some_and(|e| matches!((e.to_string_lossy().to_lowercase().as_str(), format), ("jpg" | "jpeg", OutFormat::Jpeg) | ("png", OutFormat::Png) | ("webp", OutFormat::Webp) | ("bmp", OutFormat::Bmp) | ("tif" | "tiff", OutFormat::Tiff))) {
                    job.format = OutFormat::Keep;
                    job.dest = Some(src.clone());
                }
                // Edits are now baked into the file.
                self.dev = Develop::default();
                self.geo = crate::geom::Geometry::default();
                self.active_preset = None;
            }
            ExportTarget::SaveAs => {
                let ext = match format {
                    OutFormat::Keep => src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or("png".into()),
                    OutFormat::Png => "png".into(),
                    OutFormat::Jpeg => "jpg".into(),
                    OutFormat::Webp => "webp".into(),
                    OutFormat::Bmp => "bmp".into(),
                    OutFormat::Tiff => "tif".into(),
                };
                let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let Some(dest) = rfd::FileDialog::new()
                    .set_file_name(format!("{stem}_edit.{ext}"))
                    .set_directory(src.parent().unwrap_or(std::path::Path::new(".")))
                    .save_file()
                else {
                    return;
                };
                job.format = OutFormat::Keep;
                job.dest = Some(dest);
            }
        }
        self.run_job(ctx, job, "Saving");
    }
}
