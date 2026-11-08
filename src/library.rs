//! The photo library: star ratings, keywords, collections and favourite
//! folders (one JSON file in the config folder), folder listing (optionally
//! recursive) and EXIF reading.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileMeta {
    pub rating: u8,
    pub keywords: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub files: BTreeMap<String, FileMeta>,
    pub collections: BTreeMap<String, BTreeSet<String>>,
    pub favourites: Vec<PathBuf>,
    #[serde(skip)]
    dirty: bool,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn lib_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("library.json"))
}

impl Library {
    pub fn load() -> Self {
        lib_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(p) = lib_path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }

    pub fn rating(&self, p: &Path) -> u8 {
        self.files.get(&key(p)).map(|m| m.rating).unwrap_or(0)
    }

    pub fn set_rating(&mut self, p: &Path, r: u8) {
        self.files.entry(key(p)).or_default().rating = r.min(5);
        self.dirty = true;
        self.tidy(p);
    }

    pub fn keywords(&self, p: &Path) -> Vec<String> {
        self.files.get(&key(p)).map(|m| m.keywords.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn add_keyword(&mut self, p: &Path, k: &str) {
        let k = k.trim();
        if k.is_empty() {
            return;
        }
        self.files.entry(key(p)).or_default().keywords.insert(k.to_string());
        self.dirty = true;
    }

    pub fn remove_keyword(&mut self, p: &Path, k: &str) {
        if let Some(m) = self.files.get_mut(&key(p)) {
            m.keywords.remove(k);
            self.dirty = true;
        }
        self.tidy(p);
    }

    /// Every keyword used anywhere, for suggestions.
    pub fn all_keywords(&self) -> Vec<String> {
        let mut s: BTreeSet<String> = BTreeSet::new();
        for m in self.files.values() {
            s.extend(m.keywords.iter().cloned());
        }
        s.into_iter().collect()
    }

    fn tidy(&mut self, p: &Path) {
        let k = key(p);
        if self.files.get(&k).is_some_and(|m| m.rating == 0 && m.keywords.is_empty()) {
            self.files.remove(&k);
        }
    }

    pub fn in_collection(&self, name: &str, p: &Path) -> bool {
        self.collections.get(name).is_some_and(|c| c.contains(&key(p)))
    }

    pub fn toggle_collection(&mut self, name: &str, p: &Path, on: bool) {
        let c = self.collections.entry(name.to_string()).or_default();
        if on {
            c.insert(key(p));
        } else {
            c.remove(&key(p));
        }
        self.dirty = true;
    }

    pub fn new_collection(&mut self, name: &str) {
        let n = name.trim();
        if !n.is_empty() {
            self.collections.entry(n.to_string()).or_default();
            self.dirty = true;
        }
    }

    pub fn delete_collection(&mut self, name: &str) {
        self.collections.remove(name);
        self.dirty = true;
    }

    pub fn collection_files(&self, name: &str) -> Vec<PathBuf> {
        self.collections.get(name).map(|c| c.iter().map(PathBuf::from).filter(|p| p.exists()).collect()).unwrap_or_default()
    }

    pub fn toggle_favourite(&mut self, p: &Path) {
        if let Some(i) = self.favourites.iter().position(|f| f == p) {
            self.favourites.remove(i);
        } else {
            self.favourites.push(p.to_path_buf());
        }
        self.dirty = true;
    }

    /// Keep metadata attached when a file is renamed or moved.
    pub fn rename(&mut self, from: &Path, to: &Path) {
        if let Some(m) = self.files.remove(&key(from)) {
            self.files.insert(key(to), m);
            self.dirty = true;
        }
        for c in self.collections.values_mut() {
            if c.remove(&key(from)) {
                c.insert(key(to));
                self.dirty = true;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
}

fn entry(path: PathBuf) -> Option<Entry> {
    let md = std::fs::metadata(&path).ok()?;
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(Entry { is_dir: md.is_dir(), size: md.len(), modified, path })
}

fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// List a folder: subfolders + images; with `recursive`, every image below
/// (depth ≤ 12, at most 50 000 files) and no folder entries.
pub fn list(dir: &Path, recursive: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    if !recursive {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if hidden(&p) {
                    continue;
                }
                if p.is_dir() || crate::assoc::is_image(&p) {
                    if let Some(en) = entry(p) {
                        out.push(en);
                    }
                }
            }
        }
        return out;
    }
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if hidden(&p) {
                continue;
            }
            if p.is_dir() {
                if depth < 12 {
                    stack.push((p, depth + 1));
                }
            } else if crate::assoc::is_image(&p) {
                if let Some(en) = entry(p) {
                    out.push(en);
                }
                if out.len() >= 50_000 {
                    return out;
                }
            }
        }
    }
    out
}

/// Immediate subfolders (for the folder tree), naturally sorted.
pub fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !hidden(p)).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| {
        let an = a.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let bn = b.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        crate::loader::natural_cmp(&an, &bn)
    });
    v
}

/// Filesystem roots: drive letters on Windows, / and home elsewhere.
pub fn roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(b) = directories::UserDirs::new() {
        if let Some(p) = b.picture_dir() {
            v.push(p.to_path_buf());
        }
        v.push(b.home_dir().to_path_buf());
        if let Some(p) = b.desktop_dir() {
            v.push(p.to_path_buf());
        }
        if let Some(p) = b.download_dir() {
            v.push(p.to_path_buf());
        }
    }
    #[cfg(windows)]
    {
        for l in b'A'..=b'Z' {
            let d = PathBuf::from(format!("{}:\\", l as char));
            if d.exists() {
                v.push(d);
            }
        }
    }
    #[cfg(not(windows))]
    {
        v.push(PathBuf::from("/"));
    }
    v.dedup();
    v
}

pub fn format_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

pub fn format_size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).max(1.0))
    }
}

/// Selected EXIF fields as (label, value) pairs.
pub fn exif(path: &Path) -> Vec<(String, String)> {
    let Ok(f) = std::fs::File::open(path) else { return Vec::new() };
    let mut r = std::io::BufReader::new(f);
    let Ok(ex) = exif::Reader::new().read_from_container(&mut r) else { return Vec::new() };
    use exif::{In, Tag};
    let wanted = [
        (Tag::Make, "Camera make"),
        (Tag::Model, "Camera"),
        (Tag::LensModel, "Lens"),
        (Tag::FocalLength, "Focal length"),
        (Tag::FNumber, "Aperture"),
        (Tag::ExposureTime, "Shutter"),
        (Tag::PhotographicSensitivity, "ISO"),
        (Tag::ExposureBiasValue, "Exposure comp."),
        (Tag::Flash, "Flash"),
        (Tag::DateTimeOriginal, "Taken"),
        (Tag::Software, "Software"),
        (Tag::Artist, "Artist"),
        (Tag::Copyright, "Copyright"),
    ];
    let mut out = Vec::new();
    for (tag, label) in wanted {
        if let Some(f) = ex.get_field(tag, In::PRIMARY) {
            let v = f.display_value().with_unit(&ex).to_string().trim_matches('"').trim().to_string();
            if !v.is_empty() {
                out.push((label.to_string(), v));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_and_keywords_roundtrip() {
        let mut l = Library::default();
        let p = Path::new("/x/a.jpg");
        l.set_rating(p, 4);
        l.add_keyword(p, "sky");
        assert_eq!(l.rating(p), 4);
        assert_eq!(l.keywords(p), vec!["sky".to_string()]);
        l.rename(p, Path::new("/x/b.jpg"));
        assert_eq!(l.rating(Path::new("/x/b.jpg")), 4);
        l.set_rating(Path::new("/x/b.jpg"), 0);
        l.remove_keyword(Path::new("/x/b.jpg"), "sky");
        assert!(l.files.is_empty());
    }

    #[test]
    fn recursive_listing() {
        let d = std::env::temp_dir().join(format!("viewer-list-{}", std::process::id()));
        let sub = d.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        let _ = std::fs::write(d.join("a.png"), b"x");
        let _ = std::fs::write(sub.join("b.jpg"), b"x");
        let flat = list(&d, false);
        assert_eq!(flat.len(), 2); // a.png + sub/
        let deep = list(&d, true);
        assert_eq!(deep.len(), 2); // a.png + sub/b.jpg
        assert!(deep.iter().all(|e| !e.is_dir));
        let _ = std::fs::remove_dir_all(&d);
    }
}
