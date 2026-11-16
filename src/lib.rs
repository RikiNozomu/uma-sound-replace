//! uma-sound-replace — Hachimi Edge plugin (API v3) for Umamusume (Windows).
//!
//! Plays the user's own files (wav, mp3, flac, ogg) instead of the game's
//! sounds: music, voices, effects. A file is named after the cue it replaces
//! and put in `hachimi\uma-sound-replace\sounds\` (see `library`). No file or a
//! broken file: the game's own sound plays. See `cri` for the game side.

// The game side is Windows only; elsewhere (tests) its users are absent.
#![cfg_attr(not(windows), allow(dead_code))]

mod adx;
mod api;
#[cfg(windows)]
mod cri;
mod decode;
mod library;
mod menu;
mod settings;
#[cfg(windows)]
mod sys;
mod utf;

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use api::{Api, GetApiFn};

pub const TAG: &str = "uma-sound-replace";
const REQUIRED_API_VERSION: i32 = 3;

static READY: AtomicBool = AtomicBool::new(false);
/// Outcome of the hook install, shown in the menu when it failed.
pub static STATUS: Mutex<String> = Mutex::new(String::new());

/// Entry point. Hachimi looks this symbol up after LoadLibrary.
#[no_mangle]
pub extern "C" fn hachimi_init_v3(get_api: Option<GetApiFn>, version: i32) -> i32 {
    let Some(get_api) = get_api else { return 0 };
    if version < REQUIRED_API_VERSION {
        return 0;
    }
    let Some(api) = Api::load(get_api) else { return 0 };
    api::set_get_api(get_api);
    let api = api::set(api);

    let result = catch_unwind(|| unsafe {
        settings::load();
        reload();
        // On Windows, Hachimi runs plugin init *after* it has fired the
        // game-initialized callbacks, so check whether the game is already up.
        if !(api.il2cpp_get_assembly_image)(c"umamusume.dll".as_ptr()).is_null() {
            mark_ready();
        } else {
            (api.hachimi_register_on_game_initialized)(Some(on_game_initialized), std::ptr::null_mut());
        }
        (api.gui_register_menu_section)(Some(menu_section), std::ptr::null_mut());
    });
    if result.is_err() {
        api.error("init panicked");
        return 0;
    }
    api.info("plugin loaded");
    1
}

/// Hooks the audio library (it is loaded once the game is initialized).
fn mark_ready() {
    if READY.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(windows)]
    {
        let api = api::get();
        let status = match catch_unwind(cri::install) {
            Ok(Ok(msg)) => {
                api.info(&msg);
                msg
            }
            Ok(Err(msg)) => {
                api.error(&msg);
                msg
            }
            Err(_) => {
                api.error("hook install panicked");
                "hook install panicked".into()
            }
        };
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status;
    }
}

/// Rescans the sounds folder and decodes new or changed files in the background.
pub fn reload() {
    let dir = settings::sounds_dir();
    let _ = std::fs::create_dir_all(&dir);
    let dups = library::with(|l| {
        l.rescan(&dir);
        l.duplicates.clone()
    });
    if let Some(api) = api::try_get() {
        let n = library::with(|l| l.entries.len());
        api.info(&format!("{n} replacement file(s) in {}", dir.display()));
        for d in dups {
            api.warn(&format!("ignored (another file already replaces this cue): {}", d.display()));
        }
    }
    std::thread::spawn(|| {
        if catch_unwind(library::prewarm).is_err() {
            if let Some(api) = api::try_get() {
                api.error("decoding panicked");
            }
        }
        if let Some(api) = api::try_get() {
            library::with(|l| {
                for e in l.entries.values() {
                    if let library::State::Broken(err) = &e.state {
                        api.warn(&format!("cannot use {}: {err}", e.path.display()));
                    }
                }
            });
        }
    });
}

unsafe extern "C" fn on_game_initialized(_userdata: *mut c_void) {
    mark_ready();
}

extern "C" fn menu_section(ui: *mut c_void, _userdata: *mut c_void) {
    if catch_unwind(AssertUnwindSafe(|| menu::draw(ui))).is_err() {
        api::get().error("menu panicked");
    }
}
