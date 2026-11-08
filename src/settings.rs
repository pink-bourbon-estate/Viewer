//! Persistent settings and the user-editable mixing palette.

use crate::filters::{Mode, Params};
use crate::palette::PaletteDef;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
    Folders,
    Metadata,
    Develop,
    Colours,
    Composition,
    ArtViews,
    Export,
}

impl PanelKind {
    pub const ALL: [PanelKind; 7] = [
        PanelKind::Folders,
        PanelKind::Metadata,
        PanelKind::Develop,
        PanelKind::Colours,
        PanelKind::Composition,
        PanelKind::ArtViews,
        PanelKind::Export,
    ];
    pub fn label(self) -> &'static str {
        match self {
            PanelKind::Folders => "Folders",
            PanelKind::Metadata => "Metadata",
            PanelKind::Develop => "Develop",
            PanelKind::Colours => "Colours",
            PanelKind::Composition => "Composition",
            PanelKind::ArtViews => "Art views",
            PanelKind::Export => "Export",
        }
    }
}

/// Four dock stacks: left-top, left-bottom, right-top, right-bottom.
pub const DOCK_NAMES: [&str; 4] = ["Left top", "Left bottom", "Right top", "Right bottom"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Dock {
    pub stacks: [Vec<PanelKind>; 4],
    pub active: [usize; 4],
}

impl Default for Dock {
    fn default() -> Self {
        Dock {
            stacks: [
                vec![PanelKind::Folders],
                vec![PanelKind::Metadata],
                vec![PanelKind::Develop, PanelKind::Colours, PanelKind::ArtViews],
                vec![PanelKind::Composition, PanelKind::Export],
            ],
            active: [0; 4],
        }
    }
}

impl Dock {
    pub fn location(&self, p: PanelKind) -> Option<usize> {
        self.stacks.iter().position(|s| s.contains(&p))
    }
    pub fn move_to(&mut self, p: PanelKind, stack: Option<usize>) {
        for s in &mut self.stacks {
            s.retain(|x| *x != p);
        }
        if let Some(i) = stack.filter(|i| *i < 4) {
            self.stacks[i].push(p);
            self.active[i] = self.stacks[i].len() - 1;
        }
        for i in 0..4 {
            if self.active[i] >= self.stacks[i].len() {
                self.active[i] = 0;
            }
        }
    }
    /// Bring a panel to the front (showing it in its default spot if hidden).
    pub fn reveal(&mut self, p: PanelKind) {
        let i = match self.location(p) {
            Some(i) => i,
            None => {
                let d = match p {
                    PanelKind::Folders => 0,
                    PanelKind::Metadata => 1,
                    PanelKind::Composition | PanelKind::Export => 3,
                    _ => 2,
                };
                self.stacks[d].push(p);
                d
            }
        };
        if let Some(j) = self.stacks[i].iter().position(|x| *x == p) {
            self.active[i] = j;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    Tiles,
    Thumbs,
    Details,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortBy {
    Name,
    Modified,
    Size,
    Rating,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Center {
    Grid,
    Split,
    Viewer,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    pub folder: Option<PathBuf>,
    pub collection: Option<String>,
    pub recursive: bool,
    #[serde(skip)]
    pub search: String,
    #[serde(skip)]
    pub back: Vec<PathBuf>,
    #[serde(skip)]
    pub forward: Vec<PathBuf>,
}

impl TabState {
    pub fn title(&self) -> String {
        if let Some(c) = &self.collection {
            return format!("♦ {c}");
        }
        match &self.folder {
            Some(f) => f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.display().to_string()),
            None => "New tab".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Overlays {
    pub thirds: bool,
    pub golden: bool,
    pub centre: bool,
    pub diagonals: bool,
    pub armature: bool,
    pub quarters: bool,
    pub squares: bool,
    pub squares_n: u32,
    pub opacity: f32,
    pub colour: [u8; 3],
}

impl Default for Overlays {
    fn default() -> Self {
        Overlays {
            thirds: false,
            golden: false,
            centre: false,
            diagonals: false,
            armature: false,
            quarters: false,
            squares: false,
            squares_n: 6,
            opacity: 0.6,
            colour: [235, 200, 140],
        }
    }
}

impl Overlays {
    pub fn any(&self) -> bool {
        self.thirds || self.golden || self.centre || self.diagonals || self.armature || self.quarters || self.squares
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub dock: Dock,
    pub show_panels: bool,
    pub view_mode: ViewMode,
    pub sort: SortBy,
    pub sort_desc: bool,
    pub min_rating: u8,
    pub thumb_size: f32,
    pub center: Center,
    pub colours: usize,
    pub compact_cards: bool,
    pub mode: Mode,
    pub params: Params,
    pub overlays: Overlays,
    pub split: bool,
    pub split_pos: f32,
    pub fit_upscale: bool,
    pub export_format: crate::batch::OutFormat,
    pub export_quality: u8,
    pub export_max_edge: u32,
    pub export_suffix: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            tabs: vec![TabState::default()],
            active_tab: 0,
            dock: Dock::default(),
            show_panels: true,
            view_mode: ViewMode::Tiles,
            sort: SortBy::Name,
            sort_desc: false,
            min_rating: 0,
            thumb_size: 120.0,
            center: Center::Split,
            colours: 8,
            compact_cards: false,
            mode: Mode::Original,
            params: Params::default(),
            overlays: Overlays::default(),
            split: false,
            split_pos: 0.5,
            fit_upscale: false,
            export_format: crate::batch::OutFormat::Jpeg,
            export_quality: 92,
            export_max_edge: 0,
            export_suffix: "_edit".into(),
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
