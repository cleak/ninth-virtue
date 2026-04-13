use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::game::map::{LocationType, MapState};

const FOG_PREF_FILE: &str = "minimap_fog_enabled.txt";
const FOG_ROOT_DIR: &str = "fog-v1";

pub const FOG_VISIBILITY_UNSEEN: u8 = 0;
pub const FOG_VISIBILITY_EXPLORED: u8 = 112;
pub const FOG_VISIBILITY_VISIBLE: u8 = 255;
pub const FOG_HIDDEN_TILE: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FogScene {
    Britannia,
    Underworld,
    Local(u8),
}

impl FogScene {
    pub fn from_map(map: &MapState) -> Option<Self> {
        match map.location {
            LocationType::Overworld => Some(if map.is_underworld() {
                Self::Underworld
            } else {
                Self::Britannia
            }),
            LocationType::Town(id)
            | LocationType::Dwelling(id)
            | LocationType::Castle(id)
            | LocationType::Keep(id) => Some(Self::Local(id)),
            LocationType::Dungeon(_) | LocationType::Combat(_) => None,
        }
    }

    pub fn dimensions(self) -> (usize, usize) {
        match self {
            Self::Britannia | Self::Underworld => (256, 256),
            Self::Local(_) => (32, 32),
        }
    }

    fn file_name(self) -> String {
        match self {
            Self::Britannia => "britannia.bin".to_string(),
            Self::Underworld => "underworld.bin".to_string(),
            Self::Local(id) => format!("local-{id:03}.bin"),
        }
    }
}

#[derive(Default)]
pub struct FogState {
    enabled: bool,
    game_key: Option<String>,
    scenes: HashMap<FogScene, Vec<u8>>,
}

impl FogState {
    pub fn new() -> Self {
        Self {
            enabled: true,
            game_key: None,
            scenes: HashMap::new(),
        }
    }

    pub fn load_preferences(&mut self) {
        self.enabled = load_fog_enabled();
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        save_fog_enabled(enabled);
    }

    pub fn set_game_dir(&mut self, game_dir: Option<&Path>) {
        let next = game_dir.map(game_dir_storage_key);
        if self.game_key != next {
            self.game_key = next;
            self.scenes.clear();
        }
    }

    pub fn can_reset(&self) -> bool {
        self.game_key.is_some()
    }

    pub fn reset_current_game(&mut self) {
        if let Some(path) = self.current_game_dir_path() {
            let _ = fs::remove_dir_all(path);
        }
        self.scenes.clear();
    }

    pub fn record_visible_tiles(&mut self, scene: FogScene, coords: &[(usize, usize)]) {
        if coords.is_empty() {
            return;
        }

        let (width, height) = scene.dimensions();
        let mut changed = false;
        {
            let scene_data = self.ensure_scene_loaded(scene);
            for &(x, y) in coords {
                if x >= width || y >= height {
                    continue;
                }
                let idx = y * width + x;
                if scene_data[idx] == 0 {
                    scene_data[idx] = 1;
                    changed = true;
                }
            }
        }

        if changed {
            self.save_scene(scene);
        }
    }

    pub fn scene_data(&mut self, scene: FogScene) -> &[u8] {
        self.ensure_scene_loaded(scene).as_slice()
    }

    fn ensure_scene_loaded(&mut self, scene: FogScene) -> &mut Vec<u8> {
        if !self.scenes.contains_key(&scene) {
            let loaded = self.load_scene(scene);
            self.scenes.insert(scene, loaded);
        }
        self.scenes.get_mut(&scene).unwrap()
    }

    fn load_scene(&self, scene: FogScene) -> Vec<u8> {
        let (width, height) = scene.dimensions();
        let expected_len = width * height;
        let Some(path) = self.scene_file_path(scene) else {
            return vec![0; expected_len];
        };
        let Ok(bytes) = fs::read(path) else {
            return vec![0; expected_len];
        };
        if bytes.len() == expected_len {
            bytes
        } else {
            vec![0; expected_len]
        }
    }

    fn save_scene(&self, scene: FogScene) {
        let Some(path) = self.scene_file_path(scene) else {
            return;
        };
        let Some(data) = self.scenes.get(&scene) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, data);
    }

    fn scene_file_path(&self, scene: FogScene) -> Option<PathBuf> {
        let mut path = self.current_game_dir_path()?;
        path.push(scene.file_name());
        Some(path)
    }

    fn current_game_dir_path(&self) -> Option<PathBuf> {
        let mut path = appdata_root()?;
        path.push(FOG_ROOT_DIR);
        path.push(self.game_key.as_deref()?);
        Some(path)
    }
}

fn load_fog_enabled() -> bool {
    bool_pref_path(FOG_PREF_FILE)
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|value| value.trim() == "true")
        .unwrap_or(true)
}

fn save_fog_enabled(enabled: bool) {
    let Some(path) = bool_pref_path(FOG_PREF_FILE) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, if enabled { "true" } else { "false" });
}

fn bool_pref_path(file_name: &str) -> Option<PathBuf> {
    let mut path = appdata_root()?;
    path.push(file_name);
    Some(path)
}

fn appdata_root() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"))?;
    let mut path = PathBuf::from(base);
    path.push("The Ninth Virtue");
    Some(path)
}

fn game_dir_storage_key(path: &Path) -> String {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let normalized = normalized
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();

    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_scene_dimensions_match_runtime_maps() {
        assert_eq!(FogScene::Britannia.dimensions(), (256, 256));
        assert_eq!(FogScene::Underworld.dimensions(), (256, 256));
        assert_eq!(FogScene::Local(2).dimensions(), (32, 32));
    }

    #[test]
    fn game_dir_storage_key_normalizes_case_and_separators() {
        let a = game_dir_storage_key(Path::new(r"C:\Games\Ultima 5"));
        let b = game_dir_storage_key(Path::new("c:/games/ultima 5"));
        assert_eq!(a, b);
    }
}
