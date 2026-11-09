//! What kind of file is this, and which filter group does it belong to?

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Markdown,
    Text,
    Python,
    Csv,
    Audio,
    Pdf,
    Docx,
    Sheet,
    /// OBJ / STL / PLY / glTF / GLB / 3MF, read directly.
    Model,
    /// FBX / Alembic / .blend, converted through Blender.
    ModelBlender,
    /// .max — recognised so we can explain it cannot be opened.
    ModelClosed,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    All,
    Images,
    Documents,
    TextCode,
    Audio,
    Models,
}

impl Filter {
    pub const ALL: [Filter; 6] = [Filter::All, Filter::Images, Filter::Documents, Filter::TextCode, Filter::Audio, Filter::Models];
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Images => "Images",
            Filter::Documents => "Documents",
            Filter::TextCode => "Text & code",
            Filter::Audio => "Audio",
            Filter::Models => "3D",
        }
    }
    pub fn accepts(self, k: Kind) -> bool {
        match self {
            Filter::All => true,
            Filter::Images => k == Kind::Image,
            Filter::Documents => matches!(k, Kind::Markdown | Kind::Pdf | Kind::Docx | Kind::Sheet | Kind::Csv),
            Filter::TextCode => matches!(k, Kind::Markdown | Kind::Text | Kind::Python | Kind::Csv),
            Filter::Audio => k == Kind::Audio,
            Filter::Models => matches!(k, Kind::Model | Kind::ModelBlender | Kind::ModelClosed),
        }
    }
}

const TEXT_EXT: &[&str] = &[
    "txt", "log", "json", "jsonc", "toml", "yaml", "yml", "ini", "cfg", "conf", "xml", "html", "htm", "css", "js", "mjs", "ts",
    "tsx", "jsx", "rs", "c", "h", "cpp", "hpp", "cc", "cs", "java", "kt", "go", "rb", "php", "lua", "sh", "bash", "zsh", "ps1",
    "bat", "cmd", "sql", "r", "jl", "swift", "glsl", "hlsl", "wgsl", "tex", "bib", "srt", "vtt", "env", "gitignore", "lrtemplate",
    "cube", "osl", "mel", "vex",
];

pub fn kind(p: &Path) -> Kind {
    let Some(ext) = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) else {
        return Kind::Other;
    };
    if crate::assoc::is_image(p) {
        return Kind::Image;
    }
    match ext.as_str() {
        "md" | "markdown" | "mdown" => Kind::Markdown,
        "py" | "pyw" => Kind::Python,
        "csv" | "tsv" => Kind::Csv,
        "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "aif" | "aiff" | "caf" | "mka" | "opus" => Kind::Audio,
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        "xlsx" | "xlsm" | "xls" | "ods" => Kind::Sheet,
        "obj" | "stl" | "ply" | "gltf" | "glb" | "3mf" => Kind::Model,
        "fbx" | "abc" | "blend" | "dae" | "usd" | "usda" | "usdc" | "usdz" | "x3d" => Kind::ModelBlender,
        "max" => Kind::ModelClosed,
        e if TEXT_EXT.contains(&e) => Kind::Text,
        _ => Kind::Other,
    }
}

/// Short label and badge colour for file tiles.
pub fn badge(k: Kind, p: &Path) -> (String, [u8; 3]) {
    let ext = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    let c = match k {
        Kind::Markdown => [120, 160, 220],
        Kind::Text => [140, 140, 150],
        Kind::Python => [70, 120, 190],
        Kind::Csv | Kind::Sheet => [60, 150, 90],
        Kind::Audio => [200, 110, 170],
        Kind::Pdf => [210, 70, 60],
        Kind::Docx => [60, 100, 200],
        Kind::Model | Kind::ModelBlender => [230, 150, 60],
        Kind::ModelClosed => [120, 100, 80],
        Kind::Image | Kind::Other => [100, 100, 100],
    };
    (ext, c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new("a/notes.MD")), Kind::Markdown);
        assert_eq!(kind(Path::new("x.py")), Kind::Python);
        assert_eq!(kind(Path::new("x.flac")), Kind::Audio);
        assert_eq!(kind(Path::new("x.fbx")), Kind::ModelBlender);
        assert_eq!(kind(Path::new("x.jpg")), Kind::Image);
        assert!(Filter::Documents.accepts(Kind::Pdf));
        assert!(!Filter::Images.accepts(Kind::Pdf));
    }
}
