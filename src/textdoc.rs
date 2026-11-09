//! Text, code and Markdown: load / edit / safe save, syntax colouring,
//! find & replace, Markdown preview with [[wiki links]], linked mentions,
//! and running Python scripts.

use crate::theme;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, Ui};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub const MAX_TEXT: u64 = 20 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum MdMode {
    Preview,
    Edit,
    Split,
}

pub struct TextDoc {
    pub path: PathBuf,
    pub text: String,
    saved: String,
    pub read_only: Option<String>,
    mtime: Option<SystemTime>,
    pub changed_on_disk: bool,
    last_disk_check: f64,
    pub lang: String,
    pub find: String,
    pub replace: String,
    pub show_find: bool,
    find_from: usize,
    pub run: Option<PyRun>,
    pub args: String,
    pub mentions: Option<Vec<Mention>>,
    pub mentions_busy: bool,
}

pub fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Load a text file. Refuses binary files; non-UTF-8 opens read-only.
pub fn load(path: &Path) -> Result<TextDoc, String> {
    let md = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if md.len() > MAX_TEXT {
        return Err(format!("This file is {} MB; quick editing is limited to 20 MB.", md.len() / 1_048_576));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("This looks like a binary file, not text.".into());
    }
    let (text, read_only) = match String::from_utf8(bytes) {
        Ok(s) => (s, None),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), Some("Not UTF-8 text — opened read-only so nothing is corrupted.".to_string())),
    };
    let lang = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    Ok(TextDoc {
        path: path.to_path_buf(),
        saved: text.clone(),
        text,
        read_only,
        mtime: mtime(path),
        changed_on_disk: false,
        last_disk_check: 0.0,
        lang,
        find: String::new(),
        replace: String::new(),
        show_find: false,
        find_from: 0,
        run: None,
        args: String::new(),
        mentions: None,
        mentions_busy: false,
    })
}

/// Write via a temporary file + rename so a crash never leaves half a file.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.viewer-tmp", path.extension().map(|e| e.to_string_lossy()).unwrap_or_default()));
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

impl TextDoc {
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(r) = &self.read_only {
            return Err(r.clone());
        }
        atomic_write(&self.path, self.text.as_bytes())?;
        self.saved = self.text.clone();
        self.mtime = mtime(&self.path);
        self.changed_on_disk = false;
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let fresh = load(&self.path)?;
        self.text = fresh.text.clone();
        self.saved = fresh.text;
        self.mtime = fresh.mtime;
        self.changed_on_disk = false;
        Ok(())
    }

    /// Notice edits made by other programs (checked every 2 s).
    pub fn poll_disk(&mut self, now: f64) {
        if now - self.last_disk_check < 2.0 {
            return;
        }
        self.last_disk_check = now;
        let m = mtime(&self.path);
        if m.is_some() && m != self.mtime {
            if self.dirty() {
                self.changed_on_disk = true;
            } else if self.reload().is_err() {
                self.changed_on_disk = true;
            }
        }
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self.lang.as_str(), "md" | "markdown" | "mdown")
    }

    pub fn is_python(&self) -> bool {
        matches!(self.lang.as_str(), "py" | "pyw")
    }

    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

// ---------------------------------------------------------------- highlighting

fn keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "py" | "pyw" => &[
            "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
            "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or",
            "pass", "raise", "return", "try", "while", "with", "yield", "self", "print",
        ],
        "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if",
            "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
            "super", "trait", "true", "type", "unsafe", "use", "where", "while", "dyn",
        ],
        "js" | "mjs" | "ts" | "tsx" | "jsx" => &[
            "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else",
            "export", "extends", "false", "finally", "for", "function", "if", "import", "in", "instanceof", "let", "new", "null",
            "return", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "yield",
            "interface", "type",
        ],
        "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "java" | "kt" | "go" | "swift" | "glsl" | "hlsl" | "wgsl" => &[
            "auto", "bool", "break", "case", "char", "class", "const", "continue", "default", "do", "double", "else", "enum",
            "extern", "false", "float", "for", "if", "int", "long", "namespace", "new", "null", "nullptr", "private", "protected",
            "public", "return", "short", "static", "struct", "switch", "this", "true", "typedef", "using", "void", "while",
            "func", "package", "import", "var", "let", "fn", "vec2", "vec3", "vec4", "uniform", "in", "out",
        ],
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => &[
            "if", "then", "else", "elif", "fi", "for", "do", "done", "while", "case", "esac", "function", "return", "echo",
            "export", "local", "set", "in",
        ],
        "sql" => &["select", "from", "where", "insert", "into", "values", "update", "delete", "create", "table", "join", "on", "and", "or", "not", "null", "order", "by", "group", "limit", "as"],
        "lua" => &["and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while"],
        _ => &[],
    }
}

fn line_comment(lang: &str) -> Option<&'static str> {
    match lang {
        "py" | "pyw" | "sh" | "bash" | "zsh" | "ps1" | "toml" | "yaml" | "yml" | "r" | "jl" | "env" | "conf" | "cfg" | "gitignore" | "rb" => Some("#"),
        "ini" => Some(";"),
        "sql" | "lua" => Some("--"),
        "bat" | "cmd" => Some("REM"),
        "md" | "markdown" | "txt" | "log" | "csv" | "json" => None,
        _ => Some("//"),
    }
}

/// A simple, fast highlighter: comments, strings, numbers, keywords.
pub fn highlight(text: &str, lang: &str, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
    let plain = fmt(theme::TEXT);
    if text.len() > 300_000 || lang == "txt" || lang == "log" {
        job.append(text, 0.0, plain);
        return job;
    }
    let (kw_c, str_c, num_c, com_c, head_c) = (
        Color32::from_rgb(214, 160, 92),
        Color32::from_rgb(150, 196, 120),
        Color32::from_rgb(140, 180, 230),
        Color32::from_rgb(110, 110, 110),
        Color32::from_rgb(235, 200, 140),
    );
    let kws = keywords(lang);
    let lc = line_comment(lang);
    let md = matches!(lang, "md" | "markdown" | "mdown");
    let block_comment = !matches!(lang, "py" | "pyw" | "sh" | "bash" | "toml" | "yaml" | "yml" | "ini" | "md" | "markdown" | "txt" | "sql" | "lua");
    let b = text.as_bytes();
    let mut i = 0;
    let mut line_start = true;
    while i < b.len() {
        let rest = &text[i..];
        if md && line_start && (rest.starts_with('#') || rest.starts_with("> ")) {
            let end = rest.find('\n').map(|e| i + e).unwrap_or(b.len());
            job.append(&text[i..end], 0.0, fmt(head_c));
            i = end;
            continue;
        }
        if md && rest.starts_with("[[") {
            if let Some(e) = rest.find("]]") {
                job.append(&text[i..i + e + 2], 0.0, fmt(num_c));
                i += e + 2;
                line_start = false;
                continue;
            }
        }
        if md && rest.starts_with('`') {
            let end = rest[1..].find('`').map(|e| i + e + 2).unwrap_or(i + 1);
            job.append(&text[i..end], 0.0, fmt(str_c));
            i = end;
            line_start = false;
            continue;
        }
        if let Some(c) = lc
            && !md
            && rest.starts_with(c)
        {
            let end = rest.find('\n').map(|e| i + e).unwrap_or(b.len());
            job.append(&text[i..end], 0.0, fmt(com_c));
            i = end;
            continue;
        }
        if block_comment && rest.starts_with("/*") {
            let end = rest.find("*/").map(|e| i + e + 2).unwrap_or(b.len());
            job.append(&text[i..end], 0.0, fmt(com_c));
            i = end;
            continue;
        }
        let ch = b[i];
        if !md && (ch == b'"' || ch == b'\'' || ch == b'`') {
            let triple = matches!(lang, "py" | "pyw") && (rest.starts_with("\"\"\"") || rest.starts_with("'''"));
            let end = if triple {
                let q = &rest[..3];
                rest[3..].find(q).map(|e| i + 3 + e + 3).unwrap_or(b.len())
            } else {
                let mut j = i + 1;
                while j < b.len() && b[j] != ch && b[j] != b'\n' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                (j + 1).min(b.len())
            };
            let end = end.max(i + 1);
            let end = (end..=b.len()).find(|&e| text.is_char_boundary(e)).unwrap_or(b.len());
            job.append(&text[i..end], 0.0, fmt(str_c));
            i = end;
            line_start = false;
            continue;
        }
        if !md && ch.is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            job.append(&text[i..j], 0.0, fmt(num_c));
            i = j;
            line_start = false;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let w = &text[i..j];
            let is_kw = kws.contains(&w) || (lang == "sql" && kws.contains(&w.to_lowercase().as_str()));
            job.append(w, 0.0, if is_kw { fmt(kw_c) } else { plain.clone() });
            i = j;
            line_start = false;
            continue;
        }
        // Everything else up to the next interesting byte.
        let mut j = i + 1;
        while j < b.len() && !text.is_char_boundary(j) {
            j += 1;
        }
        let s = &text[i..j];
        line_start = s == "\n" || (line_start && (s == " " || s == "\t"));
        job.append(s, 0.0, plain.clone());
        i = j;
    }
    job
}

// ---------------------------------------------------------------- wiki links

/// "[[Note#Heading|alias]]" -> "Note"
pub fn link_target(raw: &str) -> String {
    let t = raw.split('|').next().unwrap_or(raw);
    let t = t.split('#').next().unwrap_or(t);
    t.trim().to_string()
}

/// Where a link points: same folder first, then anywhere under `root`.
pub fn resolve(name: &str, from: &Path, root: Option<&Path>) -> Vec<PathBuf> {
    let name = link_target(name);
    if name.is_empty() {
        return Vec::new();
    }
    let want = name.replace('\\', "/");
    let file_part = want.rsplit('/').next().unwrap_or(&want).to_lowercase();
    let file_part = file_part.strip_suffix(".md").unwrap_or(&file_part).to_string();
    let dir = from.parent().unwrap_or(Path::new("."));
    // Relative path or same folder.
    for cand in [dir.join(&want), dir.join(format!("{want}.md"))] {
        if cand.is_file() {
            return vec![cand];
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && stem_lower(&p) == file_part {
                return vec![p];
            }
        }
    }
    let mut out = Vec::new();
    if let Some(root) = root {
        walk_md(root, &mut |p| {
            if stem_lower(p) == file_part && (want.contains('/') == false || p.to_string_lossy().replace('\\', "/").to_lowercase().ends_with(&format!("{}.md", want.to_lowercase()))) {
                out.push(p.to_path_buf());
            }
            out.len() < 50
        });
    }
    out
}

fn stem_lower(p: &Path) -> String {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.strip_suffix(".md").map(str::to_string).unwrap_or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default())
}

/// Visit Markdown files under `root` (depth ≤ 12, skips hidden folders).
/// The callback returns false to stop.
pub fn walk_md(root: &Path, f: &mut dyn FnMut(&Path) -> bool) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                if depth < 12 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                seen += 1;
                if !f(&p) || seen > 200_000 {
                    return;
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mention {
    pub path: PathBuf,
    pub line: usize,
    pub snippet: String,
}

/// Notes under `root` that link to `target` ([[stem]]).
pub fn linked_mentions(target: &Path, root: &Path) -> Vec<Mention> {
    let stem = stem_lower(target);
    let mut out = Vec::new();
    walk_md(root, &mut |p| {
        if p == target {
            return true;
        }
        if std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > 2 * 1024 * 1024 {
            return true;
        }
        if let Ok(s) = std::fs::read_to_string(p) {
            let lower = s.to_lowercase();
            if !lower.contains("[[") {
                return true;
            }
            for (ln, line) in lower.lines().enumerate() {
                let mut rest = line;
                while let Some(i) = rest.find("[[") {
                    let after = &rest[i + 2..];
                    let end = after.find("]]").unwrap_or(after.len());
                    let t = link_target(&after[..end]);
                    let t = t.rsplit('/').next().unwrap_or(&t).trim_end_matches(".md");
                    if t == stem {
                        let orig = s.lines().nth(ln).unwrap_or("").trim();
                        let snippet: String = orig.chars().take(140).collect();
                        out.push(Mention { path: p.to_path_buf(), line: ln + 1, snippet });
                        break;
                    }
                    rest = &after[end.min(after.len())..];
                }
            }
        }
        out.len() < 500
    });
    out
}

// ---------------------------------------------------------------- markdown preview

#[derive(Clone, Debug)]
pub enum Click {
    Wiki(String),
    Url(String),
}

/// Render Markdown; returns a link if one was clicked.
pub fn markdown_preview(ui: &mut Ui, src: &str, base: &Path) -> Option<Click> {
    use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_WIKILINKS;
    let mut clicked = None;
    let (mut bold, mut italic, mut strike, mut code_block) = (false, false, false, false);
    let mut heading: Option<HeadingLevel> = None;
    let mut link: Option<Click> = None;
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut quote = 0usize;
    let mut code_buf = String::new();
    let mut words: Vec<(RichText, Option<Click>)> = Vec::new();
    let mut in_table = false;
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut table: Vec<Vec<String>> = Vec::new();

    let flush = |ui: &mut Ui, words: &mut Vec<(RichText, Option<Click>)>, quote: usize, indent: usize, clicked: &mut Option<Click>| {
        if words.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space((indent * 18 + quote * 12) as f32);
            if quote > 0 {
                ui.label(RichText::new("» ").color(theme::DIM));
            }
            for (w, l) in words.drain(..) {
                match l {
                    Some(c) => {
                        if ui.link(w).clicked() {
                            *clicked = Some(c);
                        }
                    }
                    None => {
                        ui.label(w);
                    }
                }
            }
        });
    };

    for ev in Parser::new_ext(src, opts) {
        let indent = list_stack.len().saturating_sub(1);
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                heading = Some(level);
                ui.add_space(6.0);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                heading = None;
                ui.add_space(2.0);
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                ui.add_space(4.0);
            }
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,
            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,
            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,
            Event::Start(Tag::BlockQuote(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                let marker = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "•  ".to_string(),
                };
                words.push((RichText::new(marker).color(theme::DIM), None));
            }
            Event::End(TagEnd::Item) => flush(ui, &mut words, quote, list_stack.len().saturating_sub(1), &mut clicked),
            Event::TaskListMarker(done) => words.push((RichText::new(if done { "☑ " } else { "☐ " }).color(theme::ACCENT), None)),
            Event::Start(Tag::CodeBlock(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                code_block = true;
                code_buf.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                code_block = false;
                egui::Frame::new().fill(theme::CANVAS).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(code_buf.trim_end()).monospace());
                });
                ui.add_space(4.0);
            }
            Event::Start(Tag::Link { link_type, dest_url, .. }) => {
                link = Some(match link_type {
                    LinkType::WikiLink { .. } => Click::Wiki(dest_url.to_string()),
                    _ => {
                        let u = dest_url.to_string();
                        if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:") {
                            Click::Url(u)
                        } else {
                            Click::Wiki(base.parent().unwrap_or(Path::new(".")).join(u.replace("%20", " ")).to_string_lossy().into_owned())
                        }
                    }
                });
            }
            Event::End(TagEnd::Link) => link = None,
            Event::Start(Tag::Table(_)) => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                in_table = true;
                table.clear();
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                egui::Grid::new(ui.next_auto_id()).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (ri, r) in table.iter().enumerate() {
                        for c in r {
                            let t = RichText::new(c);
                            ui.label(if ri == 0 { t.strong() } else { t });
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(4.0);
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => row.clear(),
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => table.push(std::mem::take(&mut row)),
            Event::Start(Tag::TableCell) => cell.clear(),
            Event::End(TagEnd::TableCell) => row.push(std::mem::take(&mut cell)),
            Event::Rule => {
                flush(ui, &mut words, quote, indent, &mut clicked);
                ui.separator();
            }
            Event::SoftBreak => words.push((RichText::new(" "), None)),
            Event::HardBreak => flush(ui, &mut words, quote, indent, &mut clicked),
            Event::Code(t) => {
                if in_table {
                    cell.push_str(&t);
                } else {
                    words.push((RichText::new(t.to_string()).monospace().background_color(theme::CARD), link.clone()));
                }
            }
            Event::Text(t) => {
                if code_block {
                    code_buf.push_str(&t);
                    continue;
                }
                if in_table {
                    cell.push_str(&t);
                    continue;
                }
                let size = match heading {
                    Some(HeadingLevel::H1) => Some(22.0),
                    Some(HeadingLevel::H2) => Some(18.0),
                    Some(HeadingLevel::H3) => Some(15.5),
                    Some(_) => Some(13.5),
                    None => None,
                };
                // One widget per word so long lines wrap naturally.
                let text = t.to_string();
                let mut start = 0;
                for (i, c) in text.char_indices() {
                    if c == ' ' {
                        let w = &text[start..=i];
                        words.push((style(w, bold || heading.is_some(), italic, strike, size, link.is_some()), link.clone()));
                        start = i + 1;
                    }
                }
                if start < text.len() {
                    words.push((style(&text[start..], bold || heading.is_some(), italic, strike, size, link.is_some()), link.clone()));
                }
            }
            Event::Html(h) | Event::InlineHtml(h) => words.push((RichText::new(h.to_string()).color(theme::DIM).monospace(), None)),
            _ => {}
        }
    }
    flush(ui, &mut words, quote, 0, &mut clicked);
    clicked
}

fn style(w: &str, bold: bool, italic: bool, strike: bool, size: Option<f32>, link: bool) -> RichText {
    let mut r = RichText::new(w);
    if let Some(s) = size {
        r = r.size(s).color(Color32::from_rgb(235, 225, 210));
    }
    if bold {
        r = r.strong();
    }
    if italic {
        r = r.italics();
    }
    if strike {
        r = r.strikethrough();
    }
    if link {
        r = r.color(theme::ACCENT);
    }
    r
}

// ---------------------------------------------------------------- find & replace

/// Select the next match of `find` after the cursor. Returns false if none.
pub fn find_next(ctx: &egui::Context, id: egui::Id, doc: &mut TextDoc, backwards: bool) -> bool {
    if doc.find.is_empty() {
        return false;
    }
    let hay = doc.text.to_lowercase();
    let needle = doc.find.to_lowercase();
    let from = doc.find_from.min(hay.len());
    let pos = if backwards {
        hay[..from.saturating_sub(1).min(hay.len())].rfind(&needle).or_else(|| hay.rfind(&needle))
    } else {
        hay[from..].find(&needle).map(|p| p + from).or_else(|| hay.find(&needle))
    };
    let Some(p) = pos else { return false };
    doc.find_from = p + needle.len();
    let start = doc.text[..p.min(doc.text.len())].chars().count();
    let len = doc.find.chars().count();
    if let Some(mut st) = egui::TextEdit::load_state(ctx, id) {
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(start), CCursor::new(start + len))));
        st.store(ctx, id);
    }
    ctx.memory_mut(|m| m.request_focus(id));
    true
}

pub fn replace_all(doc: &mut TextDoc) -> usize {
    if doc.find.is_empty() {
        return 0;
    }
    let n = doc.text.matches(&doc.find).count();
    doc.text = doc.text.replace(&doc.find, &doc.replace);
    n
}

// ---------------------------------------------------------------- python

pub struct PyRun {
    pub child: Option<Child>,
    pub output: Arc<Mutex<Vec<(bool, String)>>>,
    pub done: Arc<Mutex<Option<String>>>,
    pub started: f64,
}

fn python_candidates() -> Vec<Vec<&'static str>> {
    if cfg!(windows) { vec![vec!["py", "-3"], vec!["python"], vec!["python3"]] } else { vec![vec!["python3"], vec!["python"]] }
}

pub fn run_python(script: &Path, args: &str, ctx: egui::Context, now: f64) -> Result<PyRun, String> {
    let dir = script.parent().unwrap_or(Path::new("."));
    let extra: Vec<String> = shell_words(args);
    let mut last_err = String::new();
    for cand in python_candidates() {
        let mut cmd = Command::new(cand[0]);
        cmd.args(&cand[1..])
            .arg("-u")
            .arg(script)
            .args(&extra)
            .current_dir(dir)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(mut child) => {
                let output = Arc::new(Mutex::new(Vec::new()));
                let done = Arc::new(Mutex::new(None));
                let readers: Vec<(Box<dyn std::io::Read + Send>, bool)> = [
                    child.stdout.take().map(|s| (Box::new(s) as Box<dyn std::io::Read + Send>, false)),
                    child.stderr.take().map(|s| (Box::new(s) as Box<dyn std::io::Read + Send>, true)),
                ]
                .into_iter()
                .flatten()
                .collect();
                for (r, is_err) in readers {
                    let out = output.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(r).lines() {
                            let Ok(l) = line else { break };
                            if let Ok(mut o) = out.lock() {
                                o.push((is_err, l));
                                if o.len() > 20_000 {
                                    o.drain(..5_000);
                                }
                            }
                            ctx.request_repaint();
                        }
                    });
                }
                return Ok(PyRun { child: Some(child), output, done, started: now });
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(format!("Could not find Python ({last_err}). Install it from python.org and tick \"Add to PATH\"."))
}

impl PyRun {
    pub fn poll(&mut self) {
        if let Some(c) = &mut self.child
            && let Ok(Some(status)) = c.try_wait()
        {
            if let Ok(mut d) = self.done.lock() {
                *d = Some(match status.code() {
                    Some(0) => "Finished".into(),
                    Some(n) => format!("Exited with code {n}"),
                    None => "Stopped".into(),
                });
            }
            self.child = None;
        }
    }
    pub fn stop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
        }
    }
    pub fn running(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for PyRun {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Split an argument string, honouring "double quotes".
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            c if c.is_whitespace() && !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_targets() {
        assert_eq!(link_target("Note#Head|alias"), "Note");
        assert_eq!(link_target(" folder/Note "), "folder/Note");
    }

    #[test]
    fn words() {
        assert_eq!(shell_words(r#"a "b c" d"#), vec!["a", "b c", "d"]);
    }

    #[test]
    fn resolve_and_mentions() {
        let d = std::env::temp_dir().join(format!("viewer-md-{}", std::process::id()));
        let sub = d.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        let a = d.join("Alpha.md");
        let b = sub.join("Beta.md");
        let _ = std::fs::write(&a, "see [[Beta]] and [[Missing]]");
        let _ = std::fs::write(&b, "back to [[alpha|home]]");
        assert_eq!(resolve("alpha", &b, Some(&d)), vec![a.clone()]);
        assert_eq!(resolve("Beta", &a, Some(&d)), vec![b.clone()]);
        assert!(resolve("Missing", &a, Some(&d)).is_empty());
        let m = linked_mentions(&a, &d);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, b);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn highlight_does_not_panic_on_unicode() {
        let job = highlight("x = \"héllo ✓\" # coment ü\nprint(1.5)", "py", FontId::monospace(12.0));
        assert!(!job.sections.is_empty());
        let _ = highlight("# Title ✓\n[[link]] `code` ü", "md", FontId::monospace(12.0));
    }

    #[test]
    fn atomic_save_roundtrip() {
        let p = std::env::temp_dir().join(format!("viewer-t-{}.txt", std::process::id()));
        let _ = std::fs::write(&p, "one");
        let mut d = load(&p).unwrap_or_else(|_| load(Path::new("/dev/null")).unwrap_or_else(|_| unreachable_doc()));
        d.text.push_str(" two");
        assert!(d.dirty());
        assert!(d.save().is_ok());
        assert_eq!(std::fs::read_to_string(&p).unwrap_or_default(), "one two");
        let _ = std::fs::remove_file(&p);
    }

    fn unreachable_doc() -> TextDoc {
        TextDoc {
            path: PathBuf::new(),
            text: String::new(),
            saved: String::new(),
            read_only: None,
            mtime: None,
            changed_on_disk: false,
            last_disk_check: 0.0,
            lang: String::new(),
            find: String::new(),
            replace: String::new(),
            show_find: false,
            find_from: 0,
            run: None,
            args: String::new(),
            mentions: None,
            mentions_busy: false,
        }
    }
}
