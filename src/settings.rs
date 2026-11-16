//! `settings.txt` in the plugin folder (`hachimi\uma-sound-replace\`):
//! `key = value` lines. Every save writes all keys.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::api;

pub const PLUGIN_DIR: &str = "uma-sound-replace";
const FILE_NAME: &str = "settings.txt";

/// Replacements on (menu toggle).
pub const ENABLED: &str = "enabled";
/// Write every cue the game plays to `cue_log.txt` (menu toggle).
pub const LOG_CUES: &str = "log_cues";
/// Give replaced sounds the categories of the cue they replace (volume sliders).
pub const COPY_CATEGORIES: &str = "copy_categories";
/// Report the replacement's length when the game asks for the cue's length.
pub const PATCH_LENGTH: &str = "patch_length";
/// Add a standard (ADX/HCA) voice pool for replacements.
pub const EXTRA_VOICE_POOL: &str = "extra_voice_pool";

static VALUES: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

/// Hachimi's folder (`...\UmamusumePrettyDerby\hachimi`); "." without the API (tests).
pub fn hachimi_dir() -> PathBuf {
    let Some(api) = api::try_get() else { return PathBuf::from(".") };
    let base = unsafe { (api.hachimi_get_base_dir)() };
    if base.is_null() {
        PathBuf::from(".")
    } else {
        PathBuf::from(unsafe { CStr::from_ptr(base) }.to_string_lossy().into_owned())
    }
}

pub fn plugin_dir() -> PathBuf {
    hachimi_dir().join(PLUGIN_DIR)
}

/// Where the user's sound files go.
pub fn sounds_dir() -> PathBuf {
    plugin_dir().join("sounds")
}

fn path() -> PathBuf {
    plugin_dir().join(FILE_NAME)
}

pub fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty() && !k.starts_with('#'))
        .collect()
}

fn format(map: &BTreeMap<String, String>) -> String {
    map.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()
}

fn save(map: &BTreeMap<String, String>) {
    let path = path();
    let result = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(&path, format(map)));
    if let Err(e) = result {
        if let Some(api) = api::try_get() {
            api.warn(&format!("cannot save {}: {e}", path.display()));
        }
    }
}

/// (Re)reads settings.txt.
pub fn load() {
    let map = parse(&fs::read_to_string(path()).unwrap_or_default());
    *VALUES.lock().unwrap_or_else(|e| e.into_inner()) = Some(map);
}

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    let mut guard = VALUES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| parse(&fs::read_to_string(path()).unwrap_or_default()));
    f(map)
}

pub fn get_bool(key: &str, default: bool) -> bool {
    with(|m| m.get(key).map(|v| v == "true").unwrap_or(default))
}

/// Sets one key and saves the whole file.
pub fn set_value(key: &str, value: impl ToString) {
    let map = with(|m| {
        m.insert(key.to_owned(), value.to_string());
        m.clone()
    });
    save(&map);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_values() {
        let m = parse("a = true\n# c = 1\nb=0.5\nbad line\n");
        assert_eq!(m.get("a").map(String::as_str), Some("true"));
        assert_eq!(m.get("b").map(String::as_str), Some("0.5"));
        assert_eq!(m.len(), 2);
        assert_eq!(format(&m), "a = true\nb = 0.5\n");
    }
}
