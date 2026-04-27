use std::fs;
use std::path::{Path, PathBuf};

use crate::game::character::PartyLocks;
use crate::game::inventory::InventoryLocks;

const LOCK_PREFS_FILE: &str = "lock_preferences.txt";
const AUDIO_PREFS_FILE: &str = "audio_preferences.txt";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockPreferences {
    pub party: PartyLocks,
    pub inventory: InventoryLocks,
}

impl LockPreferences {
    fn parse(contents: &str) -> Self {
        let mut prefs = Self::default();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(value) = parse_bool(value) else {
                continue;
            };

            match key.trim() {
                "party.mana" => prefs.party.mana = value,
                "party.health" => prefs.party.health = value,
                "inventory.food" => prefs.inventory.food = value,
                "inventory.gold" => prefs.inventory.gold = value,
                "inventory.keys" => prefs.inventory.keys = value,
                "inventory.gems" => prefs.inventory.gems = value,
                "inventory.torches" => prefs.inventory.torches = value,
                "inventory.arrows" => prefs.inventory.arrows = value,
                key => {
                    if let Some(index) = parse_reagent_key(key) {
                        prefs.inventory.reagents[index] = value;
                    }
                }
            }
        }

        prefs
    }

    fn serialize(&self) -> String {
        let mut output = String::from("party.mana=");
        output.push_str(bool_as_str(self.party.mana));
        output.push('\n');
        output.push_str("party.health=");
        output.push_str(bool_as_str(self.party.health));
        output.push('\n');
        output.push_str("inventory.food=");
        output.push_str(bool_as_str(self.inventory.food));
        output.push('\n');
        output.push_str("inventory.gold=");
        output.push_str(bool_as_str(self.inventory.gold));
        output.push('\n');
        output.push_str("inventory.keys=");
        output.push_str(bool_as_str(self.inventory.keys));
        output.push('\n');
        output.push_str("inventory.gems=");
        output.push_str(bool_as_str(self.inventory.gems));
        output.push('\n');
        output.push_str("inventory.torches=");
        output.push_str(bool_as_str(self.inventory.torches));
        output.push('\n');
        output.push_str("inventory.arrows=");
        output.push_str(bool_as_str(self.inventory.arrows));
        output.push('\n');

        for (index, reagent) in self.inventory.reagents.iter().enumerate() {
            output.push_str("inventory.reagents.");
            output.push_str(&index.to_string());
            output.push('=');
            output.push_str(bool_as_str(*reagent));
            output.push('\n');
        }

        output
    }
}

pub fn load_lock_preferences() -> LockPreferences {
    lock_prefs_path()
        .map(|path| load_lock_preferences_from_path(&path))
        .unwrap_or_default()
}

pub fn save_lock_preferences(party: &PartyLocks, inventory: &InventoryLocks) {
    let Some(path) = lock_prefs_path() else {
        return;
    };

    let prefs = LockPreferences {
        party: party.clone(),
        inventory: inventory.clone(),
    };
    save_lock_preferences_to_path(&path, &prefs);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioPreferences {
    pub mute_on_lost_focus: bool,
    /// The user's last explicit mute choice. Acts as the source of truth for
    /// whether the game should be muted, surviving DOSBox restarts and
    /// overriding any per-application mute state Windows persists for the
    /// process. Auto-mute (focus loss) does not modify this field.
    pub user_muted: bool,
}

impl AudioPreferences {
    fn parse(contents: &str) -> Self {
        let mut prefs = Self::default();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(value) = parse_bool(value) else {
                continue;
            };
            match key.trim() {
                "mute_on_lost_focus" => prefs.mute_on_lost_focus = value,
                "user_muted" => prefs.user_muted = value,
                _ => {}
            }
        }
        prefs
    }

    fn serialize(&self) -> String {
        let mut output = String::from("mute_on_lost_focus=");
        output.push_str(bool_as_str(self.mute_on_lost_focus));
        output.push('\n');
        output.push_str("user_muted=");
        output.push_str(bool_as_str(self.user_muted));
        output.push('\n');
        output
    }
}

pub fn load_audio_preferences() -> AudioPreferences {
    audio_prefs_path()
        .map(|path| load_audio_preferences_from_path(&path))
        .unwrap_or_default()
}

pub fn save_audio_preferences(prefs: &AudioPreferences) {
    let Some(path) = audio_prefs_path() else {
        return;
    };
    save_audio_preferences_to_path(&path, prefs);
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn bool_as_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn parse_reagent_key(key: &str) -> Option<usize> {
    let index = key.strip_prefix("inventory.reagents.")?.parse().ok()?;
    (index < 8).then_some(index)
}

fn lock_prefs_path() -> Option<PathBuf> {
    appdata_file_path(LOCK_PREFS_FILE)
}

fn audio_prefs_path() -> Option<PathBuf> {
    appdata_file_path(AUDIO_PREFS_FILE)
}

fn load_lock_preferences_from_path(path: &Path) -> LockPreferences {
    fs::read_to_string(path)
        .ok()
        .map(|contents| LockPreferences::parse(&contents))
        .unwrap_or_default()
}

fn save_lock_preferences_to_path(path: &Path, prefs: &LockPreferences) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, prefs.serialize());
}

fn load_audio_preferences_from_path(path: &Path) -> AudioPreferences {
    fs::read_to_string(path)
        .ok()
        .map(|contents| AudioPreferences::parse(&contents))
        .unwrap_or_default()
}

fn save_audio_preferences_to_path(path: &Path, prefs: &AudioPreferences) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, prefs.serialize());
}

pub(crate) fn appdata_file_path(file_name: &str) -> Option<PathBuf> {
    let mut path = appdata_root()?;
    path.push(file_name);
    Some(path)
}

pub(crate) fn appdata_root() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"))?;
    let mut path = PathBuf::from(base);
    path.push("The Ninth Virtue");
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lock_preferences_round_trip_through_disk() {
        let path = temp_pref_path();

        let party = PartyLocks {
            mana: true,
            health: false,
        };
        let inventory = InventoryLocks {
            food: true,
            gold: false,
            keys: true,
            gems: false,
            torches: true,
            arrows: false,
            reagents: [true, false, true, false, true, false, true, true],
        };

        let prefs = LockPreferences {
            party: party.clone(),
            inventory: inventory.clone(),
        };
        save_lock_preferences_to_path(&path, &prefs);

        let prefs = load_lock_preferences_from_path(&path);
        assert_eq!(prefs.party, party);
        assert_eq!(prefs.inventory, inventory);

        let stored = fs::read_to_string(&path).unwrap();
        assert!(stored.contains("party.mana=true"));
        assert!(stored.contains("inventory.reagents.7=true"));

        cleanup_temp_pref_path(&path);
    }

    #[test]
    fn invalid_entries_fall_back_to_defaults() {
        let path = temp_pref_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "party.mana=maybe\nparty.health=true\ninventory.reagents.99=true\n",
        )
        .unwrap();

        let prefs = load_lock_preferences_from_path(&path);
        assert!(!prefs.party.mana);
        assert!(prefs.party.health);
        assert_eq!(prefs.inventory, InventoryLocks::default());

        cleanup_temp_pref_path(&path);
    }

    #[test]
    fn audio_preferences_round_trip_through_disk() {
        let path = temp_audio_pref_path();

        let prefs = AudioPreferences {
            mute_on_lost_focus: true,
            user_muted: true,
        };
        save_audio_preferences_to_path(&path, &prefs);

        let loaded = load_audio_preferences_from_path(&path);
        assert_eq!(loaded, prefs);

        cleanup_temp_pref_path(&path);
    }

    #[test]
    fn audio_invalid_entries_fall_back_to_defaults() {
        let path = temp_audio_pref_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "mute_on_lost_focus=maybe\nuser_muted=yes\nunknown_key=true\nno_equals\n",
        )
        .unwrap();

        let prefs = load_audio_preferences_from_path(&path);
        assert_eq!(prefs, AudioPreferences::default());

        cleanup_temp_pref_path(&path);
    }

    fn temp_pref_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("ninth-virtue-lock-prefs-{unique}"))
            .join(LOCK_PREFS_FILE)
    }

    fn temp_audio_pref_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("ninth-virtue-audio-prefs-{unique}"))
            .join(AUDIO_PREFS_FILE)
    }

    fn cleanup_temp_pref_path(path: &Path) {
        let _ = fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}
