//! The viewer canvas: zoom/pan, live geometry preview (turns, flips,
//! straighten), split compare, composition overlays, crop and level tools,
//! paint-by-number labels, pipette and toasts.

use crate::app::{color32, App, CropDrag, Tool};
use crate::batch::Rotate;
use crate::filters::Mode;
use crate::geom::Geometry;
use crate::settings::{Center, Overlays, PanelKind};
use crate::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextureId, Ui, Vec2};

/// Source UV for a point (x, y in 0..1) of the turned/flipped output frame.
fn src_uv(g: &Geometry, mirror: bool, mut x: f32, mut y: f32) -> Pos2 {
    if g.flip_h ^ mirror {
        x = 1.0 - x;
    }
    if g.flip_v {
        y = 1.0 - y;
    }
    match g.quarter % 4 {
        1 => pos2(y, 1.0 - x),
        2 => pos2(1.0 - x, 1.0 - y),
        3 => pos2(1.0 - y, x),
        _ => pos2(x, y),
    }
}

fn rotate(p: Pos2, c: Pos2, a: f32) -> Pos2 {
    let (s, co) = a.sin_cos();
    let d = p - c;
    c + vec2(d.x * co - d.y * s, d.x * s + d.y * co)
}

/// Draw `tex` into the output frame `frame` (screen rect of the turned image),
/// rotated by the straighten angle around its centre.
fn paint_image(painter: &egui::Painter, tex: TextureId, frame: Rect, g: &Geometry, mirror: bool) {
    let a = g.angle.to_radians();
    let c = frame.center();
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let mut mesh = Mesh::with_texture(tex);
    for (x, y) in corners {
        let p = pos2(frame.min.x + x * frame.width(), frame.min.y + y * frame.height());
        mesh.vertices.push(egui::epaint::Vertex { pos: rotate(p, c, a), uv: src_uv(g, mirror, x, y), color: Color32::WHITE });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

fn ray_to_rect(r: Rect, from: Pos2, dir: Vec2) -> Pos2 {
    let mut t = f32::MAX;
    if dir.x.abs() > 1e-6 {
        for x in [r.min.x, r.max.x] {
            let k = (x - from.x) / dir.x;
            if k > 1e-3 {
                t = t.min(k);
            }
        }
    }
    if dir.y.abs() > 1e-6 {
        for y in [r.min.y, r.max.y] {
            let k = (y - from.y) / dir.y;
            if k > 1e-3 {
                t = t.min(k);
            }
        }
    }
    if t == f32::MAX { from } else { from + dir * t }
}

fn draw_overlays(p: &egui::Painter, r: Rect, o: &Overlays) {
    let c = Color32::from_rgba_unmultiplied(o.colour[0], o.colour[1], o.colour[2], (o.opacity * 255.0) as u8);
    let st = Stroke::new(1.0, c);
    let vline = |f: f32| p.line_segment([pos2(r.min.x + r.width() * f, r.min.y), pos2(r.min.x + r.width() * f, r.max.y)], st);
    let hline = |f: f32| p.line_segment([pos2(r.min.x, r.min.y + r.height() * f), pos2(r.max.x, r.min.y + r.height() * f)], st);
    if o.thirds {
        for f in [1.0 / 3.0, 2.0 / 3.0] {
            vline(f);
            hline(f);
        }
    }
    if o.golden {
        for f in [0.382, 0.618] {
            vline(f);
            hline(f);
        }
    }
    if o.quarters {
        for f in [0.25, 0.5, 0.75] {
            vline(f);
            hline(f);
        }
    }
    if o.centre {
        vline(0.5);
        hline(0.5);
        p.circle_stroke(r.center(), r.width().min(r.height()) * 0.04, st);
    }
    if o.diagonals || o.armature {
        p.line_segment([r.left_top(), r.right_bottom()], st);
        p.line_segment([r.right_top(), r.left_bottom()], st);
    }
    if o.armature {
        // Reciprocals: from each corner, perpendicular to the diagonal it is not on.
        let (w, h) = (r.width(), r.height());
        for (corner, d) in [
            (r.left_top(), vec2(h, w)),
            (r.right_top(), vec2(-h, w)),
            (r.left_bottom(), vec2(h, -w)),
            (r.right_bottom(), vec2(-h, -w)),
        ] {
            let end = ray_to_rect(r, corner, d.normalized());
            p.line_segment([corner, end], Stroke::new(1.0, c.gamma_multiply(0.8)));
        }
    }
    if o.squares {
        let n = o.squares_n.max(2);
        let cell = r.width() / n as f32;
        for i in 1..n {
            vline(i as f32 / n as f32);
        }
        let rows = (r.height() / cell).floor() as u32;
        for j in 1..=rows {
            let y = r.min.y + j as f32 * cell;
            if y < r.max.y - 1.0 {
                p.line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], st);
            }
        }
    }
}

impl App {
    pub fn canvas(&mut self, ui: &mut Ui) {
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
                "Select an image, or drop one here".into()
            };
            painter.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), theme::DIM);
            resp.context_menu(|ui| self.canvas_menu(ui));
            self.draw_toast(ui, &painter, rect);
            return;
        };
        let full_size = img.size;
        let tex_orig = img.tex.id();
        let full = img.full.clone();
        let g = self.geo;
        let (tw, th) = g.turned(full_size.x, full_size.y);
        let crop = g.crop;
        let cropping = self.tool == Tool::Crop;
        // When not cropping, frame the crop; while cropping, show the whole image.
        let (vw, vh, vx0, vy0) = if cropping || !g.has_crop() {
            (tw, th, 0.0, 0.0)
        } else {
            ((crop[2] - crop[0]) * tw, (crop[3] - crop[1]) * th, crop[0], crop[1])
        };

        let fit_zoom = {
            let z = ((rect.width() - 24.0) / vw).min((rect.height() - 24.0) / vh);
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
        let tool_drag = self.tool != Tool::None && !self.pick_mode;
        if resp.dragged() && !self.pick_mode && !tool_drag {
            self.pan += resp.drag_delta();
            self.fit = false;
        }
        if resp.double_clicked() && self.tool == Tool::None {
            if self.fit {
                self.fit = false;
                self.zoom = 1.0;
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let rel = ptr - rect.center();
                    self.pan = -rel * (1.0 / fit_zoom.max(0.01) - 1.0);
                }
            } else {
                self.fit = true;
            }
        }

        // The whole turned frame on screen (crop region centred when framed).
        let frame_size = vec2(tw, th) * self.zoom;
        let view_center = rect.center() + self.pan;
        let crop_center_off = vec2((vx0 + vw / tw / 2.0 - 0.5) * frame_size.x, (vy0 + vh / th / 2.0 - 0.5) * frame_size.y);
        let frame = Rect::from_center_size(view_center - crop_center_off, frame_size);
        let crop_rect = Rect::from_min_max(
            pos2(frame.min.x + crop[0] * frame.width(), frame.min.y + crop[1] * frame.height()),
            pos2(frame.min.x + crop[2] * frame.width(), frame.min.y + crop[3] * frame.height()),
        );
        let clip = if cropping { rect } else { crop_rect.intersect(rect) };

        // Screen point -> source pixel (inverse of the preview transform).
        let mirror = self.mirror;
        let to_px = move |pos: Pos2| -> Option<(u32, u32)> {
            let p = rotate(pos, frame.center(), -g.angle.to_radians());
            if !frame.contains(p) {
                return None;
            }
            let x = (p.x - frame.min.x) / frame.width();
            let y = (p.y - frame.min.y) / frame.height();
            let uv = src_uv(&g, mirror, x, y);
            let px = ((uv.x * full_size.x) as u32).min(full.width().saturating_sub(1));
            let py = ((uv.y * full_size.y) as u32).min(full.height().saturating_sub(1));
            Some((px, py))
        };

        let shown = painter.with_clip_rect(clip);
        shown.rect_filled(clip.translate(vec2(0.0, 3.0)).expand(2.0), 2.0, Color32::from_black_alpha(70));
        match &self.processed {
            Some(st) if self.s.split => {
                let split_x = clip.min.x + clip.width() * self.s.split_pos.clamp(0.02, 0.98);
                let left = Rect::from_min_max(clip.min, pos2(split_x, clip.max.y));
                let right = Rect::from_min_max(pos2(split_x, clip.min.y), clip.max);
                paint_image(&painter.with_clip_rect(left), tex_orig, frame, &g, mirror);
                paint_image(&painter.with_clip_rect(right), st.tex.id(), frame, &g, mirror);
                let handle = Rect::from_center_size(pos2(split_x, clip.center().y), vec2(16.0, clip.height()));
                let hr = ui.interact(handle, ui.scope_id().with("split"), Sense::drag());
                if hr.dragged()
                    && let Some(ptr) = hr.interact_pointer_pos()
                {
                    self.s.split_pos = ((ptr.x - clip.min.x) / clip.width()).clamp(0.02, 0.98);
                }
                if hr.hovered() || hr.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                painter.line_segment([pos2(split_x, clip.min.y), pos2(split_x, clip.max.y)], Stroke::new(1.5, theme::ACCENT));
                let tag = |x: f32, t: &str, a: Align2| {
                    painter.text(pos2(x, clip.min.y + 12.0), a, t, FontId::proportional(11.0), Color32::from_white_alpha(200));
                };
                tag(split_x - 8.0, "original", Align2::RIGHT_CENTER);
                tag(split_x + 8.0, "edit", Align2::LEFT_CENTER);
            }
            Some(st) => paint_image(&shown, st.tex.id(), frame, &g, mirror),
            None => paint_image(&shown, tex_orig, frame, &g, mirror),
        }

        // Paint-by-number labels at screen resolution.
        if let Some(st) = &self.processed
            && self.s.mode == Mode::PaintByNumber
            && g.quarter == 0
            && g.angle.abs() < 0.01
            && !g.flip_h
            && !g.flip_v
        {
            let sx = frame.width() / st.work_size.x.max(1.0);
            let split_x = if self.s.split { clip.min.x + clip.width() * self.s.split_pos } else { f32::MIN };
            for l in &st.labels {
                let fs = (l.room * sx * 0.9).min(20.0);
                if fs < 7.0 {
                    continue;
                }
                let x = if mirror { st.work_size.x - l.x } else { l.x };
                let pos = pos2(frame.min.x + x * sx, frame.min.y + l.y * sx);
                if pos.x < split_x || !clip.contains(pos) {
                    continue;
                }
                painter.text(pos, Align2::CENTER_CENTER, l.number.to_string(), FontId::proportional(fs), Color32::from_gray(95));
            }
        }

        let overlay_rect = if cropping || g.has_crop() { crop_rect } else { frame };
        if self.s.overlays.any() {
            draw_overlays(&painter.with_clip_rect(overlay_rect.intersect(rect)), overlay_rect, &self.s.overlays);
        }

        // Crop tool.
        if cropping {
            let dim = Color32::from_black_alpha(150);
            let fr = frame.intersect(rect);
            for r in [
                Rect::from_min_max(fr.min, pos2(fr.max.x, crop_rect.min.y)),
                Rect::from_min_max(pos2(fr.min.x, crop_rect.max.y), fr.max),
                Rect::from_min_max(pos2(fr.min.x, crop_rect.min.y), pos2(crop_rect.min.x, crop_rect.max.y)),
                Rect::from_min_max(pos2(crop_rect.max.x, crop_rect.min.y), pos2(fr.max.x, crop_rect.max.y)),
            ] {
                if r.is_positive() {
                    painter.rect_filled(r, 0.0, dim);
                }
            }
            painter.rect_stroke(crop_rect, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
            for c in [crop_rect.left_top(), crop_rect.right_top(), crop_rect.left_bottom(), crop_rect.right_bottom()] {
                painter.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 1.0, Color32::WHITE);
            }
            if !self.s.overlays.any() {
                draw_overlays(&painter, crop_rect, &Overlays { thirds: true, opacity: 0.35, colour: [255, 255, 255], ..Overlays::default() });
            }
            let to_norm = |q: Pos2| (((q.x - frame.min.x) / frame.width()).clamp(0.0, 1.0), ((q.y - frame.min.y) / frame.height()).clamp(0.0, 1.0));
            if resp.drag_started()
                && let Some(q) = resp.interact_pointer_pos()
            {
                let corners = [
                    (crop_rect.left_top(), (crop[2], crop[3])),
                    (crop_rect.right_top(), (crop[0], crop[3])),
                    (crop_rect.left_bottom(), (crop[2], crop[1])),
                    (crop_rect.right_bottom(), (crop[0], crop[1])),
                ];
                self.crop_drag = if let Some((_, fixed)) = corners.iter().find(|(c, _)| c.distance(q) < 14.0) {
                    Some(CropDrag::Corner { fixed: *fixed })
                } else if crop_rect.contains(q) {
                    Some(CropDrag::Move { start: crop, origin: q })
                } else {
                    Some(CropDrag::New { origin: to_norm(q) })
                };
            }
            if resp.dragged()
                && let (Some(d), Some(q)) = (self.crop_drag, resp.interact_pointer_pos())
            {
                let (nx, ny) = to_norm(q);
                let aspect = match self.crop_aspect {
                    None => None,
                    Some(r) if r < 0.0 => Some(1.0),
                    Some(r) => Some(r * th / tw),
                };
                let corner_box = |fx: f32, fy: f32, nx: f32, ny: f32| {
                    let (mut w, mut h) = (nx - fx, ny - fy);
                    if let Some(a) = aspect {
                        // Keep the ratio (normalised units): follow the larger move.
                        let sw = if w == 0.0 { 1.0 } else { w.signum() };
                        let sh = if h == 0.0 { 1.0 } else { h.signum() };
                        if w.abs() / a > h.abs() {
                            h = w.abs() / a * sh;
                        } else {
                            w = h.abs() * a * sw;
                        }
                    }
                    [fx.min(fx + w), fy.min(fy + h), fx.max(fx + w), fy.max(fy + h)]
                };
                match d {
                    CropDrag::Move { start, origin } => {
                        let dx = (q.x - origin.x) / frame.width();
                        let dy = (q.y - origin.y) / frame.height();
                        let (w, h) = (start[2] - start[0], start[3] - start[1]);
                        let x0 = (start[0] + dx).clamp(0.0, 1.0 - w);
                        let y0 = (start[1] + dy).clamp(0.0, 1.0 - h);
                        self.geo.crop = [x0, y0, x0 + w, y0 + h];
                    }
                    CropDrag::Corner { fixed } => self.geo.crop = corner_box(fixed.0, fixed.1, nx, ny),
                    CropDrag::New { origin } => self.geo.crop = corner_box(origin.0, origin.1, nx, ny),
                }
                self.geo.constrain(full_size.x, full_size.y);
            }
            if resp.drag_stopped() {
                self.crop_drag = None;
            }
            painter.text(rect.center_top() + vec2(0.0, 16.0), Align2::CENTER_CENTER, "Crop: drag corners or inside · Enter / C to finish", FontId::proportional(12.0), Color32::from_white_alpha(200));
        }

        // Level tool.
        if self.tool == Tool::Level {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.drag_started()
                && let Some(q) = resp.interact_pointer_pos()
            {
                self.level_line = Some((q, q));
            }
            if resp.dragged()
                && let (Some((a, _)), Some(q)) = (self.level_line, resp.interact_pointer_pos())
            {
                self.level_line = Some((a, q));
            }
            if let Some((a, b)) = self.level_line {
                painter.line_segment([a, b], Stroke::new(2.0, theme::ACCENT));
            }
            if resp.drag_stopped()
                && let Some((a, b)) = self.level_line.take()
                && a.distance(b) > 20.0
            {
                let corr = crate::geom::level_angle((a.x, a.y), (b.x, b.y));
                self.geo.angle = (self.geo.angle + corr).clamp(-45.0, 45.0);
                self.geo.constrain(full_size.x, full_size.y);
                self.tool = Tool::None;
            }
            painter.text(rect.center_top() + vec2(0.0, 16.0), Align2::CENTER_CENTER, "Drag along a line that should be level", FontId::proportional(12.0), Color32::from_white_alpha(200));
        }

        // Hover readout + pipette.
        if let Some(pos) = resp.hover_pos()
            && let Some((x, y)) = to_px(pos)
            && let Some(img) = &self.img
        {
            let p = img.full.get_pixel(x, y);
            let c = [p[0], p[1], p[2]];
            self.hover = Some(c);
            if self.pick_mode {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
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
        self.draw_toast(ui, &painter, rect);
        resp.context_menu(|ui| self.canvas_menu(ui));
    }

    fn draw_toast(&mut self, ui: &mut Ui, painter: &egui::Painter, rect: Rect) {
        let now = ui.input(|i| i.time);
        if let Some((msg, until)) = &self.toast {
            if now < *until {
                let galley = painter.layout(msg.clone(), FontId::proportional(13.0), theme::TEXT, 520.0);
                let r = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 44.0), galley.size() + vec2(24.0, 14.0));
                painter.rect_filled(r, 5.0, Color32::from_rgba_unmultiplied(20, 20, 20, 235));
                painter.galley(r.min + vec2(12.0, 7.0), galley, theme::TEXT);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            } else {
                self.toast = None;
            }
        }
    }

    pub fn canvas_menu(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.menu_button("Art view", |ui| {
            for m in Mode::ALL {
                if ui.selectable_label(self.s.mode == m, format!("{}   {}", m.key(), m.label())).clicked() {
                    self.set_mode(m);
                    ui.close();
                }
            }
        });
        ui.menu_button("Overlay", |ui| {
            let o = &mut self.s.overlays;
            ui.checkbox(&mut o.thirds, "Rule of thirds  (G)");
            ui.checkbox(&mut o.golden, "Golden ratio  (Shift+G)");
            ui.checkbox(&mut o.centre, "Centre lines");
            ui.checkbox(&mut o.diagonals, "Diagonals");
            ui.checkbox(&mut o.armature, "Dynamic symmetry");
            ui.checkbox(&mut o.squares, "Square grid");
            ui.separator();
            ui.checkbox(&mut self.mirror, "Mirror  (M)");
            if ui.checkbox(&mut self.s.split.clone(), "Split compare  (S)").clicked() {
                self.toggle_split(&ctx);
            }
        });
        if ui.button("Pipette  (I)").clicked() {
            self.pick_mode = !self.pick_mode;
            ui.close();
        }
        if ui.button("Crop  (C)").clicked() {
            self.tool = Tool::Crop;
            self.s.dock.reveal(PanelKind::Export);
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
        if ui.button(if self.s.center == Center::Viewer { "Show browser  (V)" } else { "Viewer only  (V)" }).clicked() {
            self.s.center = if self.s.center == Center::Viewer { Center::Split } else { Center::Viewer };
            ui.close();
        }
        ui.separator();
        ui.menu_button("File", |ui| {
            if ui.button("Open…  (Ctrl+O)").clicked() {
                ui.close();
                self.open_dialog();
            }
            if ui.button("Save edited copy  (Ctrl+E)").clicked() {
                ui.close();
                self.export_current(&ctx, crate::panels::ExportTarget::Copy);
            }
            if ui.button("Rotate file right  (Ctrl+R)").clicked() {
                ui.close();
                self.quick_rotate(&ctx, Rotate::Cw90);
            }
            if ui.button("Rotate file left  (Ctrl+L)").clicked() {
                ui.close();
                self.quick_rotate(&ctx, Rotate::Ccw90);
            }
            if ui.button("Rename…  (F2)").clicked() {
                ui.close();
                if let Some(p) = self.img.as_ref().map(|i| i.path.clone()) {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some((p, stem));
                }
            }
            if ui.button("Batch edit…  (Ctrl+B)").clicked() {
                ui.close();
                self.open_batch(false);
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
}
