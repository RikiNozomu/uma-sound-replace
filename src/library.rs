//! The user's replacement files and their decoded (ADX) form.
//!
//! Layout under the plugin's `sounds\` folder (case-insensitive):
//! - `sounds\<cue sheet>\<cue name>.<ext>`: only for that cue sheet. A folder
//!   counts as a cue sheet when its name starts with `snd_` (every cue sheet of
//!   the game does), e.g. `sounds\snd_voi_race_100100\snd_voi_race_100100_0003.wav`.
//! - `<cue name>.<ext>` anywhere else (root or any other folder): for that cue
//!   name in any cue sheet. A cue-sheet file wins over this.
//! Extensions: wav, mp3, flac, ogg (`EXTENSIONS`, in order of preference when
//! the same cue has several files).
//!
//! Files are decoded in the background after a scan; a file that fails to
//! decode is marked broken and the game's own sound plays.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::{adx, decode};

pub const EXTENSIONS: [&str; 4] = ["flac", "wav", "ogg", "mp3"];

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    /// Lower-case cue sheet name, None = any sheet.
    pub sheet: Option<String>,
    /// Lower-case cue name.
    pub cue: String,
}

/// A decoded replacement: ADX bytes in two variants (looping or not, only the
/// header differs). Buffers are never freed while the game may play them.
pub struct Sound {
    pub once: &'static [u8],
    pub looped: &'static [u8],
    pub millis: i64,
    pub channels: usize,
    pub rate: u32,
}

#[derive(Clone)]
pub enum State {
    Pending,
    Ready(Arc<Sound>),
    Broken(String),
}

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub stamp: (u64, Option<SystemTime>),
    pub state: State,
}

/// Classifies one file below `root`; None when it is not a sound file.
pub fn key_for(root: &Path, path: &Path) -> Option<(Key, usize)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let rank = EXTENSIONS.iter().position(|e| *e == ext)?;
    let cue = path.file_stem()?.to_str()?.to_ascii_lowercase();
    if cue.is_empty() {
        return None;
    }
    let parent = path.parent()?;
    let sheet = if parent != root {
        parent
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase())
            .filter(|n| n.starts_with("snd_"))
    } else {
        None
    };
    Some((Key { sheet, cue }, rank))
}

/// Every sound file below `root` (depth-limited), keyed; duplicates resolved by
/// extension preference then path. Returns (index, ignored duplicates).
pub fn scan(root: &Path) -> (HashMap<Key, PathBuf>, Vec<PathBuf>) {
    let mut found: Vec<(Key, usize, PathBuf)> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 8 {
                    stack.push((p, depth + 1));
                }
            } else if let Some((k, rank)) = key_for(root, &p) {
                found.push((k, rank, p));
            }
        }
    }
    found.sort();
    let mut index = HashMap::new();
    let mut dups = Vec::new();
    for (k, _, p) in found {
        if index.contains_key(&k) {
            dups.push(p);
        } else {
            index.insert(k, p);
        }
    }
    (index, dups)
}

fn stamp(p: &Path) -> (u64, Option<SystemTime>) {
    match std::fs::metadata(p) {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    }
}

/// Decodes and encodes one file.
pub fn build(path: &Path) -> Result<Sound, String> {
    let pcm = decode::load(path)?;
    let once = adx::encode(&pcm.samples, pcm.channels, pcm.rate, false);
    let looped = adx::encode(&pcm.samples, pcm.channels, pcm.rate, true);
    Ok(Sound {
        once: Box::leak(once.into_boxed_slice()),
        looped: Box::leak(looped.into_boxed_slice()),
        millis: pcm.millis(),
        channels: pcm.channels,
        rate: pcm.rate,
    })
}

#[derive(Default)]
pub struct Library {
    pub root: PathBuf,
    pub entries: HashMap<Key, Entry>,
    pub duplicates: Vec<PathBuf>,
}

impl Library {
    /// Rescans `root`, keeping decoded entries whose file did not change.
    pub fn rescan(&mut self, root: &Path) {
        let (index, dups) = scan(root);
        let mut entries = HashMap::new();
        for (k, path) in index {
            let st = stamp(&path);
            let entry = match self.entries.remove(&k) {
                Some(old) if old.path == path && old.stamp == st => old,
                _ => Entry { path, stamp: st, state: State::Pending },
            };
            entries.insert(k, entry);
        }
        self.root = root.to_path_buf();
        self.entries = entries;
        self.duplicates = dups;
    }

    /// The entry for a cue: the sheet's own file first, then the any-sheet one.
    pub fn find(&self, sheet: Option<&str>, cue: &str) -> Option<(&Key, &Entry)> {
        let cue = cue.to_ascii_lowercase();
        if let Some(s) = sheet {
            let k = Key { sheet: Some(s.to_ascii_lowercase()), cue: cue.clone() };
            if let Some(e) = self.entries.get_key_value(&k) {
                return Some(e);
            }
        }
        self.entries.get_key_value(&Key { sheet: None, cue })
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for e in self.entries.values() {
            match e.state {
                State::Pending => c.0 += 1,
                State::Ready(_) => c.1 += 1,
                State::Broken(_) => c.2 += 1,
            }
        }
        c
    }
}

pub static LIBRARY: Mutex<Option<Library>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Library) -> R) -> R {
    let mut g = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Library::default))
}

/// Decodes one pending entry (if it still is) outside the lock and stores the result.
pub fn resolve(key: &Key) -> State {
    let path = match with(|l| l.entries.get(key).map(|e| (e.state.clone(), e.path.clone()))) {
        Some((State::Pending, p)) => p,
        Some((state, _)) => return state,
        None => return State::Broken("gone".into()),
    };
    let state = match build(&path) {
        Ok(s) => State::Ready(Arc::new(s)),
        Err(e) => State::Broken(e),
    };
    with(|l| {
        if let Some(e) = l.entries.get_mut(key) {
            if e.path == path && matches!(e.state, State::Pending) {
                e.state = state.clone();
            }
        }
    });
    state
}

/// Decodes every pending entry (background thread).
pub fn prewarm() {
    loop {
        let next = with(|l| {
            l.entries.iter().find(|(_, e)| matches!(e.state, State::Pending)).map(|(k, _)| k.clone())
        });
        let Some(k) = next else { return };
        let _ = resolve(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("usr_lib_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keys() {
        let root = Path::new("/r");
        let k = |p: &str| key_for(root, Path::new(p)).map(|(k, r)| (k.sheet, k.cue, r));
        assert_eq!(k("/r/A.WAV"), Some((None, "a".into(), 1)));
        assert_eq!(
            k("/r/snd_voi_race_100100/snd_voi_race_100100_0003.mp3"),
            Some((Some("snd_voi_race_100100".into()), "snd_voi_race_100100_0003".into(), 3))
        );
        assert_eq!(k("/r/my voices/snd_bgm_AM003.flac"), Some((None, "snd_bgm_am003".into(), 0)));
        assert_eq!(k("/r/readme.txt"), None);
        assert_eq!(k("/r/x/.wav"), None);
    }

    #[test]
    fn scan_find_and_duplicates() {
        let d = dir("scan");
        std::fs::create_dir_all(d.join("snd_voi_jky_02_1000000")).unwrap();
        std::fs::create_dir_all(d.join("misc/deep")).unwrap();
        for f in [
            "snd_voi_race_jky_1000101.mp3",
            "misc/deep/snd_voi_race_jky_1000101.flac",
            "snd_voi_jky_02_1000000/snd_voi_race_jky_1000101.wav",
            "notes.txt",
        ] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let mut lib = Library::default();
        lib.rescan(&d);
        assert_eq!(lib.entries.len(), 2);
        assert_eq!(lib.duplicates.len(), 1);
        let (_, e) = lib.find(Some("snd_voi_jky_02_1000000"), "SND_VOI_RACE_JKY_1000101").unwrap();
        assert!(e.path.ends_with("snd_voi_race_jky_1000101.wav"));
        let (_, e) = lib.find(Some("snd_voi_jky_01_1000000"), "snd_voi_race_jky_1000101").unwrap();
        assert!(e.path.ends_with("deep/snd_voi_race_jky_1000101.flac"), "{:?}", e.path);
        let (_, e) = lib.find(None, "snd_voi_race_jky_1000101").unwrap();
        assert_eq!(e.path.extension().unwrap(), "flac");
        assert!(lib.find(None, "other").is_none());
    }

    #[test]
    fn broken_file_is_marked_and_kept_until_changed() {
        let d = dir("broken");
        std::fs::write(d.join("a.wav"), b"not a wav").unwrap();
        let mut lib = Library::default();
        lib.rescan(&d);
        let key = lib.entries.keys().next().unwrap().clone();
        *LIBRARY.lock().unwrap() = Some(lib);
        assert!(matches!(resolve(&key), State::Broken(_)));
        with(|l| {
            let root = l.root.clone();
            l.rescan(&root);
            assert!(matches!(l.entries[&key].state, State::Broken(_)));
        });
    }
}
