//! Hachimi menu section: on/off, the folder, what was found, reload, cue log.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use crate::library::{self, State};
use crate::{api, settings};

unsafe fn label(ui: *mut c_void, text: &str) {
    let t = api::cstr(text);
    (api::get().gui_ui_label)(ui, t.as_ptr());
}

unsafe fn small(ui: *mut c_void, text: &str) {
    let t = api::cstr(text);
    (api::get().gui_ui_small)(ui, t.as_ptr());
}

unsafe fn checkbox(ui: *mut c_void, text: &str, key: &str, default: bool) -> bool {
    let mut v = settings::get_bool(key, default);
    let t = api::cstr(text);
    if (api::get().gui_ui_checkbox)(ui, t.as_ptr(), &mut v) {
        settings::set_value(key, v);
    }
    v
}

pub fn draw(ui: *mut c_void) {
    let api = api::get();
    unsafe {
        let h = api::cstr("Sound replace");
        (api.gui_ui_heading)(ui, h.as_ptr());
        checkbox(ui, "Use replacement sounds", settings::ENABLED, true);

        #[cfg(windows)]
        if !crate::cri::INSTALLED.load(Ordering::Acquire) {
            label(ui, &format!("Audio hooks: {}", crate::STATUS.lock().map(|s| s.clone()).unwrap_or_default()));
        }
        #[cfg(not(windows))]
        let _ = Ordering::Relaxed;

        small(ui, &format!("Folder: {}", settings::sounds_dir().display()));
        let (pending, ready, broken, dups, broken_list) = library::with(|l| {
            let (p, r, b) = l.counts();
            let mut list: Vec<String> = l
                .entries
                .values()
                .filter_map(|e| match &e.state {
                    State::Broken(err) => Some(format!("{}: {err}", e.path.file_name().unwrap_or_default().to_string_lossy())),
                    _ => None,
                })
                .collect();
            list.sort();
            (p, r, b, l.duplicates.len(), list)
        });
        label(ui, &format!("Files: {} ready, {} loading, {} broken", ready, pending, broken));
        if dups > 0 {
            small(ui, &format!("{dups} duplicate file(s) ignored (same cue, see the log)"));
        }
        for line in broken_list.iter().take(5) {
            small(ui, &format!("  broken: {line}"));
        }
        if let Some(button) = api::extra().gui_ui_button {
            let t = api::cstr("Reload files");
            if button(ui, t.as_ptr()) {
                crate::reload();
                api.notify("Sound replace: files reloaded");
            }
        } else {
            small(ui, "Files are read at game start (this Hachimi has no menu buttons).");
        }

        (api.gui_ui_separator)(ui);
        let log = checkbox(ui, "Log played cues (cue_log.txt)", settings::LOG_CUES, false);
        if log {
            #[cfg(windows)]
            {
                let recent = crate::cri::RECENT.lock().map(|r| r.clone()).unwrap_or_default();
                if recent.is_empty() {
                    small(ui, "Nothing played yet.");
                }
                for (sheet, cue, replaced) in recent {
                    small(ui, &format!("{}{sheet} / {cue}", if replaced { "* " } else { "" }));
                }
            }
        }
    }
}
