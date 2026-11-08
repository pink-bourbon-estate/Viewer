//! Persistent settings and the user-editable mixing palette.

use crate::filters::{Mode, Params};
use crate::palette::PaletteDef;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grid {
    Off,
    Thirds,
    Quarters,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub last_folder: Option<PathBuf>,
    pub thumb_size: f32,
    pub show_browser: bool,
    pub show_panel: bool,
    pub colours: usize,
    pub mode: Mode,
    pub params: Params,
    pub grid: Grid,
    pub grid_n: u32,
    pub split: bool,
    pub split_pos: f32,
    pub fit_upscale: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_folder: None,
            thumb_size: 120.0,
            show_browser: true,
            show_panel: true,
            colours: 8,
            mode: Mode::Original,
            params: Params::default(),
            grid: Grid::Off,
            grid_n: 6,
            split: false,
            split_pos: 0.5,
            fit_upscale: false,
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Viewer").map(|d| d.config_dir().to_path_buf())
}

fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

pub fn palette_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("palette.json"))
}

impl Settings {
    pub fn load() -> Self {
        settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = settings_path() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}

/// Load the mixing palette: `palette.json` if present and valid, else Patrick's.
pub fn load_palette() -> (PaletteDef, Option<String>) {
    let Some(p) = palette_path() else { return (PaletteDef::patrick(), None) };
    match std::fs::read_to_string(&p) {
        Ok(s) => match serde_json::from_str::<PaletteDef>(&s) {
            Ok(def) if !def.paints.is_empty() => (def, None),
            Ok(_) => (PaletteDef::patrick(), Some("palette.json has no paints; using Patrick's palette".into())),
            Err(e) => (PaletteDef::patrick(), Some(format!("palette.json: {e}; using Patrick's palette"))),
        },
        Err(_) => (PaletteDef::patrick(), None),
    }
}

/// Make sure palette.json exists (writing Patrick's palette) and return its path.
pub fn ensure_palette_file() -> Result<PathBuf, String> {
    let p = palette_path().ok_or("no config directory")?;
    if !p.exists() {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let s = serde_json::to_string_pretty(&PaletteDef::patrick()).map_err(|e| e.to_string())?;
        std::fs::write(&p, s).map_err(|e| e.to_string())?;
    }
    Ok(p)
}
