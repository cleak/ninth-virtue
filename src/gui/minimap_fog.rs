use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
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
    dirty_scenes: HashSet<FogScene>,
    last_persistence_error: Option<String>,
}

impl FogState {
    pub fn new() -> Self {
        Self {
            enabled: true,
            game_key: None,
            scenes: HashMap::new(),
            dirty_scenes: HashSet::new(),
            last_persistence_error: None,
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

    pub fn set_game_dir(&mut self, game_dir: Option<&Path>) -> bool {
        let next = game_dir.map(game_dir_storage_key);
        if self.game_key != next {
            self.game_key = next;
            self.scenes.clear();
            self.dirty_scenes.clear();
            self.last_persistence_error = None;
            true
        } else {
            false
        }
    }

    pub fn can_reset(&self) -> bool {
        self.game_key.is_some()
    }

    pub fn current_game_key(&self) -> Option<&str> {
        self.game_key.as_deref()
    }

    pub fn persistence_error(&self) -> Option<&str> {
        self.last_persistence_error.as_deref()
    }

    pub fn reset_game_by_key(&mut self, game_key: &str) -> io::Result<()> {
        remove_fog_root(&game_dir_path_for_key(game_key)?)?;
        if self.current_game_key() == Some(game_key) {
            self.scenes.clear();
            self.dirty_scenes.clear();
            self.last_persistence_error = None;
        }
        Ok(())
    }

    pub fn record_visible_tiles(&mut self, scene: FogScene, coords: &[(usize, usize)]) {
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

        if changed || self.dirty_scenes.contains(&scene) {
            match self.save_scene(scene) {
                Ok(()) => {
                    self.dirty_scenes.remove(&scene);
                    self.last_persistence_error = None;
                }
                Err(err) => {
                    self.dirty_scenes.insert(scene);
                    self.last_persistence_error = Some(err.to_string());
                }
            }
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
        let Ok(Some(path)) = self.scene_file_path(scene) else {
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

    fn save_scene(&self, scene: FogScene) -> io::Result<()> {
        let Some(data) = self.scenes.get(&scene) else {
            return Ok(());
        };
        let Some(path) = self.scene_file_path(scene)? else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)?;
        Ok(())
    }

    fn scene_file_path(&self, scene: FogScene) -> io::Result<Option<PathBuf>> {
        let Some(game_key) = self.game_key.as_deref() else {
            return Ok(None);
        };
        let mut path = game_dir_path_for_key(game_key)?;
        path.push(scene.file_name());
        Ok(Some(path))
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
    crate::preferences::appdata_file_path(file_name)
}

fn game_dir_path_for_key(game_key: &str) -> io::Result<PathBuf> {
    let Some(mut path) = crate::preferences::appdata_root() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "LOCALAPPDATA or APPDATA is unavailable",
        ));
    };
    path.push(FOG_ROOT_DIR);
    path.push(game_key);
    Ok(path)
}

fn remove_fog_root(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn remove_fog_root_propagates_non_directory_errors() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ninth-virtue-fog-reset-{unique}.tmp"));
        fs::write(&path, b"not a directory").unwrap();

        let err = remove_fog_root(&path).expect_err("removing a file as a directory should fail");
        assert_ne!(err.kind(), io::ErrorKind::NotFound);

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn set_game_dir_reports_when_key_changes() {
        let mut fog = FogState::new();

        assert!(fog.set_game_dir(Some(Path::new(r"C:\Games\Ultima 5"))));
        assert!(!fog.set_game_dir(Some(Path::new(r"c:\games\ultima 5"))));
        assert!(fog.set_game_dir(Some(Path::new(r"C:\Games\Ultima 5 Test"))));
    }

    #[test]
    fn record_visible_tiles_retries_dirty_scene_until_persisted() {
        let mut localappdata = crate::test_support::EnvVarGuard::new("LOCALAPPDATA");

        let blocked_root = unique_test_path("ninth-virtue-fog-blocked");
        fs::write(&blocked_root, b"blocked").unwrap();

        let valid_root = unique_test_path("ninth-virtue-fog-valid");
        fs::create_dir_all(&valid_root).unwrap();

        let scene = FogScene::Local(7);
        let mut fog = FogState::new();
        fog.set_game_dir(Some(Path::new(r"C:\Games\Ultima 5")));

        localappdata.set(&blocked_root);
        fog.record_visible_tiles(scene, &[(3, 4)]);
        assert!(fog.dirty_scenes.contains(&scene));
        assert!(fog.persistence_error().is_some());

        localappdata.set(&valid_root);
        fog.record_visible_tiles(scene, &[]);

        let scene_path = fog.scene_file_path(scene).unwrap().unwrap();
        assert!(scene_path.exists());
        assert!(!fog.dirty_scenes.contains(&scene));
        assert_eq!(fog.persistence_error(), None);
        assert_eq!(fs::read(scene_path).unwrap()[4 * 32 + 3], 1);

        fs::remove_file(blocked_root).unwrap();
        fs::remove_dir_all(valid_root).unwrap();
    }

    fn unique_test_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{unique}"))
    }
}
