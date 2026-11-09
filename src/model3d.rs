//! 3D preview: OBJ / STL / PLY / glTF / GLB / 3MF loaders, a CPU rasteriser
//! (no GPU driver involvement, so a bad model can't hang the window), and
//! Blender as a converter for FBX / Alembic / .blend / USD / Collada, plus
//! "Export to .blend".

use image::RgbaImage;
use rayon::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_TRIS: usize = 6_000_000;

#[derive(Default)]
pub struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
    /// Per-triangle colour.
    pub col: Vec<[u8; 3]>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub objects: usize,
    pub note: String,
}

const CLAY: [u8; 3] = [196, 186, 170];

impl Mesh {
    fn finish(mut self) -> Result<Mesh, String> {
        let n = self.pos.len() as u32;
        self.tris.retain(|t| t[0] < n && t[1] < n && t[2] < n);
        if self.tris.is_empty() {
            return Err("No triangles found in this file.".into());
        }
        if self.col.len() != self.tris.len() {
            self.col.resize(self.tris.len(), CLAY);
        }
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for p in &self.pos {
            for k in 0..3 {
                if p[k].is_finite() {
                    mn[k] = mn[k].min(p[k]);
                    mx[k] = mx[k].max(p[k]);
                }
            }
        }
        self.min = mn;
        self.max = mx;
        Ok(self)
    }

    pub fn centre(&self) -> [f32; 3] {
        [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0, (self.min[2] + self.max[2]) / 2.0]
    }

    pub fn radius(&self) -> f32 {
        let d = [self.max[0] - self.min[0], self.max[1] - self.min[1], self.max[2] - self.min[2]];
        ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 2.0).max(1e-6)
    }

    pub fn size(&self) -> [f32; 3] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1], self.max[2] - self.min[2]]
    }
}

// ---------------------------------------------------------------- loaders

pub fn load(path: &Path) -> Result<Mesh, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let m = match ext.as_str() {
        "obj" => load_obj(path)?,
        "stl" => load_stl(path)?,
        "ply" => load_ply(path)?,
        "gltf" | "glb" => load_gltf(path)?,
        "3mf" => load_3mf(path)?,
        _ => return Err(format!(".{ext} needs Blender to preview")),
    };
    if m.tris.len() > MAX_TRIS {
        return Err(format!("{} million triangles is more than the preview handles (6 million).", m.tris.len() / 1_000_000));
    }
    m.finish()
}

fn load_obj(path: &Path) -> Result<Mesh, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut m = Mesh::default();
    let mut colour = CLAY;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let v: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if v.len() == 3 {
                    m.pos.push([v[0], v[1], v[2]]);
                }
            }
            Some("f") => {
                let n = m.pos.len() as i64;
                let idx: Vec<u32> = it
                    .filter_map(|s| s.split('/').next()?.parse::<i64>().ok())
                    .map(|i| if i < 0 { n + i } else { i - 1 })
                    .filter(|i| *i >= 0)
                    .map(|i| i as u32)
                    .collect();
                for k in 1..idx.len().saturating_sub(1) {
                    m.tris.push([idx[0], idx[k], idx[k + 1]]);
                    m.col.push(colour);
                }
                if m.tris.len() > MAX_TRIS {
                    break;
                }
            }
            Some("o") | Some("g") => m.objects += 1,
            Some("usemtl") => {
                // A stable pastel per material name, so parts read apart.
                let name = it.next().unwrap_or("");
                let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
                colour = [150 + (h % 90) as u8, 150 + ((h >> 8) % 90) as u8, 150 + ((h >> 16) % 90) as u8];
            }
            _ => {}
        }
    }
    m.objects = m.objects.max(1);
    Ok(m)
}

fn load_stl(path: &Path) -> Result<Mesh, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut m = Mesh { objects: 1, ..Mesh::default() };
    let binary_len = if data.len() >= 84 { 84 + 50 * u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize } else { 0 };
    if data.len() >= 84 && binary_len == data.len() {
        let n = (data.len() - 84) / 50;
        m.pos.reserve(n * 3);
        for t in 0..n {
            let base = 84 + t * 50 + 12;
            for v in 0..3 {
                let o = base + v * 12;
                let f = |k: usize| f32::from_le_bytes([data[o + k], data[o + k + 1], data[o + k + 2], data[o + k + 3]]);
                m.pos.push([f(0), f(4), f(8)]);
            }
            let i = (t * 3) as u32;
            m.tris.push([i, i + 1, i + 2]);
        }
    } else {
        let text = String::from_utf8_lossy(&data);
        for line in text.lines() {
            let l = line.trim_start();
            if let Some(r) = l.strip_prefix("vertex") {
                let v: Vec<f32> = r.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if v.len() == 3 {
                    m.pos.push([v[0], v[1], v[2]]);
                    if m.pos.len() % 3 == 0 {
                        let i = (m.pos.len() - 3) as u32;
                        m.tris.push([i, i + 1, i + 2]);
                    }
                }
            }
        }
    }
    Ok(m)
}

fn load_ply(path: &Path) -> Result<Mesh, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let end = data.windows(10).position(|w| w == b"end_header").ok_or("Not a PLY file")?;
    let header = String::from_utf8_lossy(&data[..end]).into_owned();
    let mut body = end + 10;
    while body < data.len() && (data[body] == b'\n' || data[body] == b'\r') {
        body += 1;
    }
    let fmt = if header.contains("binary_little_endian") {
        1
    } else if header.contains("binary_big_endian") {
        2
    } else {
        0
    };
    // elements: (name, count, props[(name, type, list_count_type)])
    let mut elems: Vec<(String, usize, Vec<(String, String, Option<String>)>)> = Vec::new();
    for line in header.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["element", name, count] => elems.push((name.to_string(), count.parse().unwrap_or(0), Vec::new())),
            ["property", "list", ct, it, name] => {
                if let Some(e) = elems.last_mut() {
                    e.2.push((name.to_string(), it.to_string(), Some(ct.to_string())));
                }
            }
            ["property", t, name] => {
                if let Some(e) = elems.last_mut() {
                    e.2.push((name.to_string(), t.to_string(), None));
                }
            }
            _ => {}
        }
    }
    let size = |t: &str| match t {
        "char" | "uchar" | "int8" | "uint8" => 1,
        "short" | "ushort" | "int16" | "uint16" => 2,
        "int" | "uint" | "float" | "int32" | "uint32" | "float32" => 4,
        "double" | "float64" => 8,
        _ => 4,
    };
    let mut m = Mesh { objects: 1, ..Mesh::default() };
    let mut vcol: Vec<[u8; 3]> = Vec::new();
    let mut at = body;
    let text = if fmt == 0 { String::from_utf8_lossy(&data[body..]).into_owned() } else { String::new() };
    let mut tokens = text.split_ascii_whitespace();
    let read_bin = |at: &mut usize, t: &str| -> f64 {
        let n = size(t);
        if *at + n > data.len() {
            *at = data.len();
            return 0.0;
        }
        let b = &data[*at..*at + n];
        *at += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(b);
        if fmt == 2 {
            a[..n].reverse();
        }
        match t {
            "char" | "int8" => a[0] as i8 as f64,
            "uchar" | "uint8" => a[0] as f64,
            "short" | "int16" => i16::from_le_bytes([a[0], a[1]]) as f64,
            "ushort" | "uint16" => u16::from_le_bytes([a[0], a[1]]) as f64,
            "int" | "int32" => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            "uint" | "uint32" => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            "float" | "float32" => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            "double" | "float64" => f64::from_le_bytes(a),
            _ => 0.0,
        }
    };
    for (name, count, props) in &elems {
        for _ in 0..*count {
            let mut vals: Vec<(String, f64)> = Vec::new();
            let mut list: Vec<u32> = Vec::new();
            for (pn, t, lt) in props {
                if let Some(lt) = lt {
                    let n = if fmt == 0 { tokens.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) } else { read_bin(&mut at, lt) } as usize;
                    for _ in 0..n.min(64) {
                        let v = if fmt == 0 { tokens.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) } else { read_bin(&mut at, t) };
                        list.push(v as u32);
                    }
                } else {
                    let v = if fmt == 0 { tokens.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) } else { read_bin(&mut at, t) };
                    vals.push((pn.clone(), v));
                }
            }
            let get = |k: &str| vals.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
            if name == "vertex" {
                m.pos.push([get("x").unwrap_or(0.0) as f32, get("y").unwrap_or(0.0) as f32, get("z").unwrap_or(0.0) as f32]);
                if let (Some(r), Some(g), Some(b)) = (get("red"), get("green"), get("blue")) {
                    vcol.push([r as u8, g as u8, b as u8]);
                }
            } else if name == "face" {
                for k in 1..list.len().saturating_sub(1) {
                    m.tris.push([list[0], list[k], list[k + 1]]);
                }
            }
            if at >= data.len() && fmt != 0 {
                break;
            }
        }
    }
    if vcol.len() == m.pos.len() {
        m.col = m.tris.iter().map(|t| vcol.get(t[0] as usize).copied().unwrap_or(CLAY)).collect();
    }
    if m.tris.is_empty() && !m.pos.is_empty() {
        m.note = "Point cloud (no faces) — showing nothing to shade.".into();
    }
    Ok(m)
}

fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0; 4]; 4];
    for c in 0..4 {
        for row in 0..4 {
            r[c][row] = (0..4).map(|k| a[k][row] * b[c][k]).sum();
        }
    }
    r
}

fn load_gltf(path: &Path) -> Result<Mesh, String> {
    let (doc, buffers, _) = gltf::import(path).map_err(|e| format!("Cannot read glTF: {e}"))?;
    let mut m = Mesh::default();
    let ident = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    fn visit(node: gltf::Node, parent: [[f32; 4]; 4], buffers: &[gltf::buffer::Data], m: &mut Mesh, depth: usize) {
        if depth > 64 {
            return;
        }
        let world = mat_mul(&parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            m.objects += 1;
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(ps) = reader.read_positions() else { continue };
                let base = m.pos.len() as u32;
                for p in ps {
                    let x = world[0][0] * p[0] + world[1][0] * p[1] + world[2][0] * p[2] + world[3][0];
                    let y = world[0][1] * p[0] + world[1][1] * p[1] + world[2][1] * p[2] + world[3][1];
                    let z = world[0][2] * p[0] + world[1][2] * p[1] + world[2][2] * p[2] + world[3][2];
                    m.pos.push([x, y, z]);
                }
                let f = prim.material().pbr_metallic_roughness().base_color_factor();
                let c = [(f[0].powf(1.0 / 2.2) * 255.0) as u8, (f[1].powf(1.0 / 2.2) * 255.0) as u8, (f[2].powf(1.0 / 2.2) * 255.0) as u8];
                let count = m.pos.len() as u32 - base;
                let idx: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..count).collect(),
                };
                for t in idx.chunks_exact(3) {
                    m.tris.push([base + t[0], base + t[1], base + t[2]]);
                    m.col.push(c);
                }
            }
        }
        for child in node.children() {
            visit(child, world, buffers, m, depth + 1);
        }
    }
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    match scene {
        Some(s) => {
            for n in s.nodes() {
                visit(n, ident, &buffers, &mut m, 0);
            }
        }
        None => {
            for n in doc.nodes() {
                visit(n, ident, &buffers, &mut m, 0);
            }
        }
    }
    // glTF is Y-up; the viewer is Z-up.
    for p in &mut m.pos {
        *p = [p[0], -p[2], p[1]];
    }
    Ok(m)
}

fn load_3mf(path: &Path) -> Result<Mesh, String> {
    use quick_xml::events::Event;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let name = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().and_then(|e| e.name().ok().map(|n| n.to_string())))
        .find(|n| n.to_lowercase().ends_with(".model"))
        .ok_or("No 3D model inside this 3MF")?;
    let mut xml = Vec::new();
    zip.by_name(&name).map_err(|e| e.to_string())?.read_to_end(&mut xml).map_err(|e| e.to_string())?;
    let mut rd = quick_xml::Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut m = Mesh::default();
    let mut base = 0u32;
    let num = |e: &quick_xml::events::BytesStart, k: &[u8]| -> f32 {
        e.attributes().flatten().find(|a| a.key.as_ref() == k).and_then(|a| String::from_utf8_lossy(&a.value).parse().ok()).unwrap_or(0.0)
    };
    loop {
        match rd.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"vertices" => base = m.pos.len() as u32,
                b"object" => m.objects += 1,
                b"vertex" => m.pos.push([num(&e, b"x"), num(&e, b"y"), num(&e, b"z")]),
                b"triangle" => m.tris.push([base + num(&e, b"v1") as u32, base + num(&e, b"v2") as u32, base + num(&e, b"v3") as u32]),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(m)
}

// ---------------------------------------------------------------- rendering

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub pan: [f32; 2],
    pub wire: bool,
    pub y_up: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { yaw: 0.7, pitch: 0.45, dist: 2.6, pan: [0.0, 0.0], wire: false, y_up: false }
    }
}

/// Render the mesh to a w×h image.
pub fn render(m: &Mesh, cam: &Camera, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(16), h.max(16));
    let c = m.centre();
    let r = m.radius();
    let (sy, cy) = cam.yaw.sin_cos();
    let (sp, cp) = cam.pitch.sin_cos();
    let f = 1.0 / (22.5f32.to_radians()).tan();
    let aspect = w as f32 / h as f32;
    let dist = cam.dist * r;
    // World -> view: orbit around the centre, Z up (or Y up).
    let to_view = |p: &[f32; 3]| -> [f32; 3] {
        let (mut x, mut y, mut z) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
        if cam.y_up {
            (y, z) = (-z, y);
        }
        let x1 = x * cy - y * sy;
        let y1 = x * sy + y * cy;
        let up = z * cp - y1 * sp;
        let depth = z * sp + y1 * cp;
        x = x1 + cam.pan[0] * r;
        [x, up + cam.pan[1] * r, dist - depth]
    };
    let view: Vec<[f32; 3]> = m.pos.par_iter().map(to_view).collect();
    let proj = |v: &[f32; 3]| -> (f32, f32) {
        let z = v[2].max(1e-4);
        ((v[0] * f / aspect / z * 0.5 + 0.5) * w as f32, (0.5 - v[1] * f / z * 0.5) * h as f32)
    };
    struct T {
        p: [(f32, f32); 3],
        z: [f32; 3],
        col: [u8; 3],
        y0: i32,
        y1: i32,
    }
    let light = [0.35f32, 0.45, 0.82];
    let tris: Vec<T> = m
        .tris
        .par_iter()
        .zip(m.col.par_iter())
        .filter_map(|(t, col)| {
            let a = view.get(t[0] as usize)?;
            let b = view.get(t[1] as usize)?;
            let cc = view.get(t[2] as usize)?;
            if a[2] < 1e-3 || b[2] < 1e-3 || cc[2] < 1e-3 {
                return None;
            }
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
            let mut n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len <= 0.0 || !len.is_finite() {
                return None;
            }
            n = [n[0] / len, n[1] / len, n[2] / len];
            // Two-sided lighting (models often have flipped normals).
            let d = (n[0] * light[0] + n[1] * light[1] - n[2] * light[2]).abs();
            let shade = 0.28 + 0.72 * d;
            let col = [(col[0] as f32 * shade) as u8, (col[1] as f32 * shade) as u8, (col[2] as f32 * shade) as u8];
            let p = [proj(a), proj(b), proj(cc)];
            let y0 = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
            let y1 = p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil().min(h as f32 - 1.0) as i32;
            if y1 < 0 || y0 >= h as i32 {
                return None;
            }
            Some(T { p, z: [a[2], b[2], cc[2]], col, y0, y1 })
        })
        .collect();
    let mut img = RgbaImage::new(w, h);
    let bands = 32usize;
    let band_h = (h as usize).div_ceil(bands).max(1);
    let row_bytes = w as usize * 4;
    img.as_mut().par_chunks_mut(row_bytes * band_h).enumerate().for_each(|(bi, chunk)| {
        let by0 = (bi * band_h) as i32;
        let rows = chunk.len() / row_bytes;
        let by1 = by0 + rows as i32 - 1;
        // Background: soft vertical gradient.
        for y in 0..rows {
            let g = 40.0 - ((by0 + y as i32) as f32 / h as f32) * 14.0;
            for x in 0..w as usize {
                let o = y * row_bytes + x * 4;
                chunk[o..o + 4].copy_from_slice(&[g as u8, g as u8, (g + 2.0) as u8, 255]);
            }
        }
        let mut zb = vec![f32::MAX; rows * w as usize];
        for t in tris.iter().filter(|t| t.y1 >= by0 && t.y0 <= by1) {
            let [(x0, y0), (x1, y1), (x2, y2)] = t.p;
            let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
            if area.abs() < 1e-8 {
                continue;
            }
            let minx = x0.min(x1).min(x2).floor().max(0.0) as i32;
            let maxx = x0.max(x1).max(x2).ceil().min(w as f32 - 1.0) as i32;
            let ys = t.y0.max(by0);
            let ye = t.y1.min(by1);
            let (iz0, iz1, iz2) = (1.0 / t.z[0], 1.0 / t.z[1], 1.0 / t.z[2]);
            for py in ys..=ye {
                let fy = py as f32 + 0.5;
                for px in minx..=maxx {
                    let fx = px as f32 + 0.5;
                    let w0 = ((x1 - fx) * (y2 - fy) - (x2 - fx) * (y1 - fy)) / area;
                    let w1 = ((x2 - fx) * (y0 - fy) - (x0 - fx) * (y2 - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let iz = w0 * iz0 + w1 * iz1 + w2 * iz2;
                    let z = 1.0 / iz;
                    let zi = (py - by0) as usize * w as usize + px as usize;
                    if z < zb[zi] {
                        zb[zi] = z;
                        let edge = cam.wire && (w0 < 0.02 || w1 < 0.02 || w2 < 0.02);
                        let c = if edge { [20, 20, 22] } else { t.col };
                        let o = (py - by0) as usize * row_bytes + px as usize * 4;
                        chunk[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
            }
        }
    });
    img
}

// ---------------------------------------------------------------- Blender

/// Find a Blender executable: env BLENDER, PATH, or the usual install folders.
pub fn find_blender(custom: &str) -> Option<PathBuf> {
    let custom = custom.trim();
    if !custom.is_empty() && Path::new(custom).is_file() {
        return Some(PathBuf::from(custom));
    }
    if let Ok(b) = std::env::var("BLENDER")
        && Path::new(&b).is_file()
    {
        return Some(PathBuf::from(b));
    }
    let exe = if cfg!(windows) { "blender.exe" } else { "blender" };
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for base in ["C:\\Program Files\\Blender Foundation", "C:\\Program Files (x86)\\Blender Foundation"] {
            roots.push(PathBuf::from(base));
        }
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            roots.push(Path::new(&la).join("Programs").join("Blender Foundation"));
        }
        let steam = PathBuf::from("C:\\Program Files (x86)\\Steam\\steamapps\\common\\Blender\\blender.exe");
        if steam.is_file() {
            return Some(steam);
        }
    } else if cfg!(target_os = "macos") {
        let p = PathBuf::from("/Applications/Blender.app/Contents/MacOS/Blender");
        if p.is_file() {
            return Some(p);
        }
    } else {
        for p in ["/usr/bin/blender", "/snap/bin/blender", "/var/lib/flatpak/exports/bin/org.blender.Blender"] {
            if Path::new(p).is_file() {
                return Some(PathBuf::from(p));
            }
        }
    }
    // Newest "Blender x.y" folder wins.
    let mut found: Vec<PathBuf> = Vec::new();
    for r in roots {
        if let Ok(rd) = std::fs::read_dir(&r) {
            for e in rd.flatten() {
                let p = e.path().join(exe);
                if p.is_file() {
                    found.push(p);
                }
            }
        }
    }
    found.sort_by(|a, b| crate::loader::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    found.pop()
}

const SCRIPT: &str = r#"
import bpy, sys, os
argv = sys.argv[sys.argv.index("--") + 1:]
src, out, mode = argv[0], argv[1], argv[2]
ext = os.path.splitext(src)[1].lower()
def op(*names):
    for n in names:
        mod, name = n.split(".")
        f = getattr(getattr(bpy.ops, mod), name)
        try:
            f.get_rna_type()
            return f
        except Exception:
            continue
    raise SystemExit("Blender has no importer for " + ext)
if ext != ".blend":
    bpy.ops.wm.read_factory_settings(use_empty=True)
    if ext == ".fbx": op("import_scene.fbx")(filepath=src)
    elif ext == ".abc": op("wm.alembic_import")(filepath=src, as_background_job=False)
    elif ext == ".obj": op("wm.obj_import", "import_scene.obj")(filepath=src)
    elif ext == ".stl": op("wm.stl_import", "import_mesh.stl")(filepath=src)
    elif ext == ".ply": op("wm.ply_import", "import_mesh.ply")(filepath=src)
    elif ext in (".gltf", ".glb"): op("import_scene.gltf")(filepath=src)
    elif ext == ".dae": op("wm.collada_import")(filepath=src)
    elif ext in (".usd", ".usda", ".usdc", ".usdz"): op("wm.usd_import")(filepath=src)
    elif ext == ".x3d": op("import_scene.x3d")(filepath=src)
    else: raise SystemExit("unsupported " + ext)
if mode == "glb":
    bpy.ops.export_scene.gltf(filepath=out, export_format="GLB", export_apply=True)
else:
    bpy.ops.file.pack_all()
    bpy.ops.wm.save_as_mainfile(filepath=out, compress=True)
"#;

fn run_blender(blender: &Path, src: &Path, out: &Path, mode: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(blender);
    cmd.arg("-b");
    if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("blend")) {
        cmd.arg(src);
    } else {
        cmd.arg("--factory-startup");
    }
    cmd.arg("--python-exit-code").arg("1").arg("--python-expr").arg(SCRIPT).arg("--").arg(src).arg(out).arg(mode);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Could not start Blender: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() && out.is_file() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err);
                }
                if let Some(mut s) = child.stdout.take() {
                    let mut o = String::new();
                    let _ = s.read_to_string(&mut o);
                    err.push_str(&o);
                }
                let tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect();
                return Err(format!("Blender could not convert this file. {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
            }
            Ok(None) => {
                if start.elapsed().as_secs() > 180 {
                    let _ = child.kill();
                    return Err("Blender took longer than 3 minutes; gave up.".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Convert through Blender to a cached GLB and load it.
pub fn load_via_blender(src: &Path, blender: &Path) -> Result<Mesh, String> {
    use std::hash::{Hash, Hasher};
    let md = std::fs::metadata(src).map_err(|e| e.to_string())?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    md.len().hash(&mut h);
    md.modified().ok().hash(&mut h);
    let dir = crate::settings::config_dir().map(|d| d.join("blender-cache")).ok_or("no cache folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{:016x}.glb", h.finish()));
    if !out.is_file() {
        run_blender(blender, src, &out, "glb")?;
    }
    let mut m = load_gltf(&out)?;
    m.note = "Converted with Blender".into();
    m.finish()
}

/// Save a .blend next to the source (never overwriting an existing file).
pub fn export_blend(src: &Path, blender: &Path) -> Result<PathBuf, String> {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
    let dir = src.parent().unwrap_or(Path::new("."));
    let mut out = dir.join(format!("{stem}.blend"));
    let mut n = 1;
    while out.exists() {
        out = dir.join(format!("{stem}_{n}.blend"));
        n += 1;
    }
    run_blender(blender, src, &out, "blend")?;
    Ok(out)
}

pub fn blender_can_import(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "fbx" | "abc" | "obj" | "stl" | "ply" | "gltf" | "glb" | "dae" | "usd" | "usda" | "usdc" | "usdz" | "x3d")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, data: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("viewer-3d-{}-{name}", std::process::id()));
        let _ = std::fs::write(&p, data);
        p
    }

    #[test]
    fn obj_cube_renders() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
                   f 1 2 3 4\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";
        let p = tmp("cube.obj", obj.as_bytes());
        let m = load(&p);
        let _ = std::fs::remove_file(&p);
        let m = m.unwrap_or_default();
        assert_eq!(m.tris.len(), 12);
        let img = render(&m, &Camera::default(), 120, 90);
        let centre = img.get_pixel(60, 45);
        assert!(centre[0] > 60, "cube should cover the centre: {centre:?}");
    }

    #[test]
    fn stl_binary_and_ascii() {
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 12]);
        for v in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
            for c in v {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 0]);
        let p = tmp("t.stl", &b);
        assert_eq!(load(&p).map(|m| m.tris.len()).ok(), Some(1));
        let a = "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid\n";
        let q = tmp("a.stl", a.as_bytes());
        assert_eq!(load(&q).map(|m| m.tris.len()).ok(), Some(1));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&q);
    }

    #[test]
    fn ply_ascii() {
        let s = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                 element face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
        let p = tmp("t.ply", s.as_bytes());
        assert_eq!(load(&p).map(|m| m.tris.len()).ok(), Some(1));
        let _ = std::fs::remove_file(&p);
    }
}
