//! PDF pages (hayro), Word documents (text, headings, lists, tables,
//! pictures), spreadsheets (calamine) and CSV (editable grid).

use image::RgbaImage;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------- PDF

pub struct PdfInfo {
    pub bytes: Arc<Vec<u8>>,
    pub pages: usize,
    /// Page sizes in points.
    pub sizes: Vec<(f32, f32)>,
}

pub fn pdf_open(path: &Path) -> Result<PdfInfo, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pdf = hayro::hayro_syntax::Pdf::new(bytes.clone()).map_err(|e| format!("Cannot read this PDF: {e:?}"))?;
    let sizes: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
    if sizes.is_empty() {
        return Err("This PDF has no pages.".into());
    }
    Ok(PdfInfo { bytes: Arc::new(bytes), pages: sizes.len(), sizes })
}

/// Render one page at `scale` (1.0 = 72 dpi) to RGBA.
pub fn pdf_render(bytes: &Arc<Vec<u8>>, page: usize, scale: f32) -> Result<RgbaImage, String> {
    use hayro::vello_cpu::color::palette::css::WHITE;
    let pdf = hayro::hayro_syntax::Pdf::new((**bytes).clone()).map_err(|e| format!("{e:?}"))?;
    let pages = pdf.pages();
    let p = pages.get(page).ok_or("No such page")?;
    let (w, h) = p.render_dimensions();
    // Keep pages under ~40 MP so a huge poster can't exhaust memory.
    let s = scale.min((40_000_000.0 / (w * h).max(1.0)).sqrt()).max(0.05);
    let cache = hayro::RenderCache::new();
    let px = hayro::render(
        p,
        &cache,
        &hayro::hayro_interpret::InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings { x_scale: s, y_scale: s, bg_color: WHITE },
    );
    let (pw, ph) = (px.width() as u32, px.height() as u32);
    RgbaImage::from_raw(pw, ph, px.data_as_u8_slice().to_vec()).ok_or_else(|| "Bad page buffer".to_string())
}

// ---------------------------------------------------------------- DOCX

#[derive(Clone, Debug, Default)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug)]
pub enum Block {
    Para { heading: u8, list: Option<u8>, runs: Vec<Run> },
    Table(Vec<Vec<String>>),
    Image(RgbaImage),
}

fn attr(e: &quick_xml::events::BytesStart, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == name).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

pub fn docx_open(path: &Path) -> Result<Vec<Block>, String> {
    use quick_xml::events::Event;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("Not a valid .docx: {e}"))?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut e = zip.by_name(name).ok()?;
        if e.size() > 200 * 1024 * 1024 {
            return None;
        }
        let mut v = Vec::new();
        e.read_to_end(&mut v).ok()?;
        Some(v)
    };
    let doc = read("word/document.xml").ok_or("This .docx has no document body.")?;
    // Relationship id -> media path, for pictures.
    let mut rels = std::collections::HashMap::new();
    if let Some(r) = read("word/_rels/document.xml.rels") {
        let mut rd = quick_xml::Reader::from_reader(r.as_slice());
        let mut buf = Vec::new();
        loop {
            match rd.read_event_into(&mut buf) {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.local_name().as_ref() == b"Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                        rels.insert(id, format!("word/{}", t.trim_start_matches("./")));
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let mut blocks = Vec::new();
    let mut rd = quick_xml::Reader::from_reader(doc.as_slice());
    let mut buf = Vec::new();
    let (mut in_t, mut bold, mut italic, mut underline) = (false, false, false, false);
    let mut heading = 0u8;
    let mut list: Option<u8> = None;
    let mut runs: Vec<Run> = Vec::new();
    let mut table_depth = 0usize;
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut cell = String::new();
    let mut images: Vec<String> = Vec::new();
    loop {
        let ev = rd.read_event_into(&mut buf);
        match ev {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"p" => {
                        heading = 0;
                        list = None;
                        runs.clear();
                    }
                    b"pStyle" => {
                        let v = attr(&e, b"w:val").unwrap_or_default().to_lowercase();
                        if v.contains("list") {
                            list = Some(list.unwrap_or(0));
                        }
                        heading = if v == "title" {
                            1
                        } else if let Some(n) = v.strip_prefix("heading") {
                            n.trim().parse().unwrap_or(2).clamp(1, 6)
                        } else {
                            0
                        };
                    }
                    b"ilvl" => list = Some(attr(&e, b"w:val").and_then(|v| v.parse().ok()).unwrap_or(0)),
                    b"numPr" => list = Some(list.unwrap_or(0)),
                    b"r" => {
                        bold = false;
                        italic = false;
                        underline = false;
                    }
                    b"b" => bold = attr(&e, b"w:val").is_none_or(|v| v != "0" && v != "false"),
                    b"i" => italic = attr(&e, b"w:val").is_none_or(|v| v != "0" && v != "false"),
                    b"u" => underline = attr(&e, b"w:val").is_none_or(|v| v != "none"),
                    b"t" => in_t = true,
                    b"tab" => runs.push(Run { text: "    ".into(), ..Run::default() }),
                    b"br" | b"cr" => runs.push(Run { text: "\n".into(), ..Run::default() }),
                    b"tbl" => {
                        table_depth += 1;
                        if table_depth == 1 {
                            table.clear();
                        }
                    }
                    b"tr" if table_depth == 1 => table.push(Vec::new()),
                    b"tc" if table_depth == 1 => cell.clear(),
                    b"blip" => {
                        if let Some(id) = attr(&e, b"r:embed")
                            && let Some(t) = rels.get(&id)
                        {
                            images.push(t.clone());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) if in_t => {
                let s = t.decode().map(|c| c.into_owned()).unwrap_or_default();
                if table_depth > 0 {
                    cell.push_str(&s);
                } else {
                    runs.push(Run { text: s, bold, italic, underline });
                }
            }
            Ok(Event::GeneralRef(r)) if in_t => {
                let name = String::from_utf8_lossy(r.as_ref()).into_owned();
                let ch = match name.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    n => n
                        .strip_prefix("#x")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                if let Some(c) = ch {
                    if table_depth > 0 {
                        cell.push(c);
                    } else if let Some(last) = runs.last_mut().filter(|l| l.bold == bold && l.italic == italic) {
                        last.text.push(c);
                    } else {
                        runs.push(Run { text: c.to_string(), bold, italic, underline });
                    }
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"t" => in_t = false,
                b"p" => {
                    if table_depth > 0 {
                        cell.push(' ');
                    } else {
                        blocks.push(Block::Para { heading, list, runs: std::mem::take(&mut runs) });
                    }
                    for img in images.drain(..) {
                        if let Some(b) = read(&img)
                            && let Ok(i) = image::load_from_memory(&b)
                        {
                            let i = i.thumbnail(1400, 1400).to_rgba8();
                            blocks.push(Block::Image(i));
                        }
                    }
                }
                b"tc" if table_depth == 1 => {
                    if let Some(r) = table.last_mut() {
                        r.push(cell.trim().to_string());
                    }
                }
                b"tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        blocks.push(Block::Table(std::mem::take(&mut table)));
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("Damaged document: {e}")),
            _ => {}
        }
        buf.clear();
        if blocks.len() > 100_000 {
            break;
        }
    }
    Ok(blocks)
}

// ---------------------------------------------------------------- sheets

pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

const MAX_ROWS: usize = 200_000;
const MAX_COLS: usize = 300;

pub fn sheets_open(path: &Path) -> Result<Vec<Sheet>, String> {
    use calamine::{open_workbook_auto, Data, Reader};
    let mut wb = open_workbook_auto(path).map_err(|e| format!("Cannot read this workbook: {e}"))?;
    let names = wb.sheet_names().to_vec();
    let mut out = Vec::new();
    for n in names {
        let Ok(range) = wb.worksheet_range(&n) else { continue };
        let rows = range
            .rows()
            .take(MAX_ROWS)
            .map(|r| {
                r.iter()
                    .take(MAX_COLS)
                    .map(|c| match c {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => {
                            if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", *f as i64) } else { format!("{f}") }
                        }
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string().to_uppercase(),
                        Data::DateTime(d) => d.to_string(),
                        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
                        Data::Error(e) => format!("#{e:?}"),
                    })
                    .collect()
            })
            .collect();
        out.push(Sheet { name: n, rows });
    }
    if out.is_empty() {
        return Err("No readable sheets.".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- CSV

pub fn guess_delim(text: &str, ext: &str) -> char {
    if ext == "tsv" {
        return '\t';
    }
    let line = text.lines().next().unwrap_or("");
    let counts = [(',', line.matches(',').count()), (';', line.matches(';').count()), ('\t', line.matches('\t').count())];
    counts.iter().max_by_key(|c| c.1).filter(|c| c.1 > 0).map(|c| c.0).unwrap_or(',')
}

pub fn csv_parse(text: &str, d: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    q = false;
                }
            } else {
                cell.push(c);
            }
        } else if c == '"' && cell.is_empty() {
            q = true;
        } else if c == d {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' {
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else if c != '\r' {
            cell.push(c);
        }
        if rows.len() > MAX_ROWS {
            break;
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

pub fn csv_write(rows: &[Vec<String>], d: char) -> String {
    let mut s = String::new();
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .map(|c| {
                if c.contains(d) || c.contains('"') || c.contains('\n') {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            })
            .collect();
        s.push_str(&line.join(&d.to_string()));
        s.push('\n');
    }
    s
}

pub fn col_name(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_roundtrip() {
        let t = "a,b,c\n1,\"x, y\",\"say \"\"hi\"\"\"\n";
        let rows = csv_parse(t, ',');
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["1", "x, y", "say \"hi\""]]);
        assert_eq!(csv_parse(&csv_write(&rows, ','), ','), rows);
        assert_eq!(guess_delim("a;b;c\n", "csv"), ';');
    }

    #[test]
    fn columns() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
    }

    #[test]
    fn docx_minimal() {
        let p = std::env::temp_dir().join(format!("viewer-d-{}.docx", std::process::id()));
        let f = std::fs::File::create(&p);
        if let Ok(f) = f {
            let mut z = zip::ZipWriter::new(f);
            let opt = zip::write::SimpleFileOptions::default();
            let _ = z.start_file("word/document.xml", opt);
            let body = r#"<w:document xmlns:w="w"><w:body>
              <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title here</w:t></w:r></w:p>
              <w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r><w:r><w:t xml:space="preserve"> text &amp; more</w:t></w:r></w:p>
              <w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
              </w:body></w:document>"#;
            let _ = std::io::Write::write_all(&mut z, body.as_bytes());
            let _ = z.finish();
        }
        let b = docx_open(&p).unwrap_or_default();
        let _ = std::fs::remove_file(&p);
        assert!(matches!(&b[0], Block::Para { heading: 1, .. }));
        match &b[1] {
            Block::Para { runs, .. } => {
                assert!(runs[0].bold);
                assert_eq!(runs.iter().map(|r| r.text.as_str()).collect::<String>(), "Bold text & more");
            }
            other => panic!("expected paragraph, got {other:?} in {b:?}"),
        }
        assert!(matches!(&b[2], Block::Table(t) if t[0] == vec!["A1".to_string(), "B1".to_string()]));
    }
}
