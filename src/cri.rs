//! Hooks on the game's audio library (CRI ADX2, `cri_ware_unity.dll`, native
//! exports, which Hachimi does not hook: it hooks the managed wrappers).
//!
//! The game plays a sound by giving a player a cue (`criAtomExPlayer_SetCueId`
//! / `SetCueName` / `SetCueIndex` with an ACB handle) and calling `Start` (or
//! `Prepare`). We remember each player's cue; at `Start`, when the user has a
//! file for that cue, the player gets our ADX data instead (`SetFormat` ADX +
//! `SetData`) and the original `Start` runs: the game's own player plays it,
//! so its volume, pause and stop apply. No file, a broken file or a missing
//! export: nothing is changed.
//!
//! Cue sheet names come from the ACB files the game loads (`LoadAcbFile` /
//! `LoadAcbData`, @UTF header `Name`). ACBs loaded before the plugin are only
//! matched by cue name (cue names carry the sheet name in this game).
//!
//! Optional, checked at run time (`LAYOUT_OK`): the cue's categories are copied
//! onto the player (volume sliders), the cue length the game reads is the
//! replacement's, and looping cues (length -1) loop. These read CRI's
//! `CriAtomExCueInfo`, whose layout is verified on a real cue first.

#![cfg(windows)]

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::library::{self, State};
use crate::{api, settings, sys, utf};

type Hn = usize;

const DLL: &str = "cri_ware_unity.dll";
const FORMAT_ADX: u32 = 1;
const NO_CATEGORY: u16 = 0xFFFF;
const MAX_CATEGORIES: usize = 16;
/// `CriAtomExCueInfo` offsets (x64): id i32 @0, type @4, name ptr @8,
/// user_data @16, length i64 @24, categories u16[16] @32.
const INFO_ID: usize = 0;
const INFO_NAME: usize = 8;
const INFO_LENGTH: usize = 24;
const INFO_CATEGORIES: usize = 32;
/// `CriAtomExCategoryInfo`: group_no u32 @0, id u32 @4.
const CATEGORY_ID: usize = 4;

#[repr(C, align(8))]
struct InfoBuf([u8; 1024]);

impl InfoBuf {
    fn new() -> Box<InfoBuf> {
        Box::new(InfoBuf([0; 1024]))
    }
    fn i32_at(&self, o: usize) -> i32 {
        i32::from_ne_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn u32_at(&self, o: usize) -> u32 {
        u32::from_ne_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn usize_at(&self, o: usize) -> usize {
        usize::from_ne_bytes(self.0[o..o + 8].try_into().unwrap())
    }
    fn i64_at(&self, o: usize) -> i64 {
        i64::from_ne_bytes(self.0[o..o + 8].try_into().unwrap())
    }
    fn categories(&self) -> Vec<u16> {
        (0..MAX_CATEGORIES)
            .map(|i| u16::from_ne_bytes(self.0[INFO_CATEGORIES + 2 * i..INFO_CATEGORIES + 2 * i + 2].try_into().unwrap()))
            .filter(|&c| c != NO_CATEGORY)
            .collect()
    }
}

// ---- function table -------------------------------------------------------

/// One native function: the address to call (trampoline when hooked).
struct Native(AtomicUsize);

impl Native {
    const fn new() -> Native {
        Native(AtomicUsize::new(0))
    }
    fn get(&self) -> Option<usize> {
        match self.0.load(Ordering::Acquire) {
            0 => None,
            p => Some(p),
        }
    }
    fn set(&self, p: usize) {
        self.0.store(p, Ordering::Release)
    }
}

macro_rules! call {
    ($native:expr, fn($($t:ty),*) -> $r:ty, $($a:expr),*) => {
        $native.get().map(|p| {
            #[allow(unused_unsafe)]
            unsafe { std::mem::transmute::<usize, extern "C" fn($($t),*) -> $r>(p)($($a),*) }
        })
    };
}

// Hooked (address = trampoline).
static START: Native = Native::new();
static PREPARE: Native = Native::new();
static SET_CUE_ID: Native = Native::new();
static SET_CUE_NAME: Native = Native::new();
static SET_CUE_INDEX: Native = Native::new();
static SET_DATA: Native = Native::new();
static SET_FILE: Native = Native::new();
static SET_WAVE_ID: Native = Native::new();
static SET_CONTENT_ID: Native = Native::new();
static SET_CATEGORY_BY_ID: Native = Native::new();
static SET_CATEGORY_BY_NAME: Native = Native::new();
static UNSET_CATEGORY: Native = Native::new();
static DESTROY: Native = Native::new();
static LOAD_ACB_FILE: Native = Native::new();
static LOAD_ACB_DATA: Native = Native::new();
static RELEASE_ACB: Native = Native::new();
static CUE_INFO_BY_ID: Native = Native::new();
static CUE_INFO_BY_NAME: Native = Native::new();
static CUE_INFO_BY_INDEX: Native = Native::new();
// Called only.
static SET_FORMAT: Native = Native::new();
static SET_NUM_CHANNELS: Native = Native::new();
static SET_SAMPLING_RATE: Native = Native::new();
static CUE_NAME_BY_ID: Native = Native::new();
static CUE_NAME_BY_INDEX: Native = Native::new();
static CATEGORY_INFO: Native = Native::new();
static ALLOC_STANDARD_POOL: Native = Native::new();

// ---- state ----------------------------------------------------------------

#[derive(Clone, Debug)]
enum Sel {
    Id(i32),
    Name(String),
    Index(i32),
}

#[derive(Default)]
struct Player {
    cue: Option<(Hn, Sel)>,
    /// We put categories on this player (to take off again).
    our_categories: bool,
    /// The game sets categories on this player itself: leave them alone.
    game_categories: bool,
}

#[derive(Default)]
struct Acb {
    path: Option<String>,
    /// Resolved lazily from `path`: Some(None) = unreadable.
    name: Option<Option<String>>,
}

#[derive(Default)]
struct Tracker {
    players: HashMap<Hn, Player>,
    acbs: HashMap<Hn, Acb>,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
/// Inside our own calls into hooked setters (they must not reset the player).
static OURS: AtomicBool = AtomicBool::new(false);
/// 0 = unknown, 1 = CriAtomExCueInfo layout verified, 2 = does not match.
static LAYOUT: AtomicU8 = AtomicU8::new(0);
static POOL_TRIED: AtomicBool = AtomicBool::new(false);
pub static INSTALLED: AtomicBool = AtomicBool::new(false);

fn tracker<R>(f: impl FnOnce(&mut Tracker) -> R) -> R {
    let mut g = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Tracker::default))
}

/// A cue the game played: (sheet, cue, replaced) for the menu and cue_log.txt.
pub static RECENT: Mutex<VecDeque<(String, String, bool)>> = Mutex::new(VecDeque::new());
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
pub const RECENT_MAX: usize = 12;

fn note_played(sheet: &str, cue: &str, replaced: bool) {
    if !settings::get_bool(settings::LOG_CUES, false) {
        return;
    }
    {
        let mut r = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        if r.front().is_some_and(|f| f.0 == sheet && f.1 == cue) {
            return; // the same cue started again
        }
        r.push_front((sheet.to_owned(), cue.to_owned(), replaced));
        r.truncate(RECENT_MAX);
    }
    let mut f = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
    if f.is_none() {
        let path = settings::plugin_dir().join("cue_log.txt");
        *f = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    }
    if let Some(file) = f.as_mut() {
        let mark = if replaced { "  [replaced]" } else { "" };
        let _ = writeln!(file, "{sheet}\t{cue}{mark}");
    }
}

// ---- lookups --------------------------------------------------------------

unsafe fn c_string(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

fn sheet_name(acb: Hn) -> Option<String> {
    let path = tracker(|t| {
        let a = t.acbs.get(&acb)?;
        match &a.name {
            Some(n) => Some(Err(n.clone())),
            None => a.path.clone().map(Ok),
        }
    })?;
    match path {
        Err(name) => name,
        Ok(path) => {
            let name = std::fs::read(&path).ok().and_then(|b| utf::acb_name(&b));
            tracker(|t| {
                if let Some(a) = t.acbs.get_mut(&acb) {
                    a.name = Some(name.clone());
                }
            });
            name
        }
    }
}

fn cue_name(acb: Hn, sel: &Sel) -> Option<String> {
    match sel {
        Sel::Name(n) => Some(n.clone()),
        Sel::Id(id) => unsafe { c_string(call!(CUE_NAME_BY_ID, fn(Hn, i32) -> *const c_char, acb, *id)?) },
        Sel::Index(i) => unsafe { c_string(call!(CUE_NAME_BY_INDEX, fn(Hn, i32) -> *const c_char, acb, *i)?) },
    }
}

/// The cue's info, read through the original functions.
fn cue_info(acb: Hn, sel: &Sel) -> Option<Box<InfoBuf>> {
    let mut b = InfoBuf::new();
    let p = b.0.as_mut_ptr() as *mut c_void;
    let ok = match sel {
        Sel::Id(id) => call!(CUE_INFO_BY_ID, fn(Hn, i32, *mut c_void) -> i32, acb, *id, p)?,
        Sel::Index(i) => call!(CUE_INFO_BY_INDEX, fn(Hn, i32, *mut c_void) -> i32, acb, *i, p)?,
        Sel::Name(n) => {
            let n = api::cstr(n);
            call!(CUE_INFO_BY_NAME, fn(Hn, *const c_char, *mut c_void) -> i32, acb, n.as_ptr(), p)?
        }
    };
    (ok != 0).then_some(b)
}

/// Verifies the CriAtomExCueInfo offsets once on a real cue: the id at @0
/// must name the same string (same pointer) as `GetCueNameById(id)` at @8.
fn layout_ok(acb: Hn, info: &InfoBuf) -> bool {
    match LAYOUT.load(Ordering::Acquire) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let id = info.i32_at(INFO_ID);
    let Some(name_ptr) = call!(CUE_NAME_BY_ID, fn(Hn, i32) -> *const c_char, acb, id) else { return false };
    let ok = !name_ptr.is_null() && info.usize_at(INFO_NAME) == name_ptr as usize && info.i64_at(INFO_LENGTH) >= -1;
    LAYOUT.store(if ok { 1 } else { 2 }, Ordering::Release);
    if let Some(api) = api::try_get() {
        if ok {
            api.info("CriAtomExCueInfo layout verified: categories, lengths and loops are copied");
        } else {
            api.warn("CriAtomExCueInfo layout differs: replacements play without the cue's categories/length/loop");
        }
    }
    ok
}

fn ensure_pool() {
    if POOL_TRIED.swap(true, Ordering::AcqRel) || !settings::get_bool(settings::EXTRA_VOICE_POOL, true) {
        return;
    }
    let pool = call!(ALLOC_STANDARD_POOL, fn(*const c_void, *mut c_void, i32) -> Hn, std::ptr::null(), std::ptr::null_mut(), 0);
    if let Some(api) = api::try_get() {
        match pool {
            Some(0) | None => api.warn("no extra voice pool (replacements use the game's pools)"),
            Some(_) => api.info("extra standard voice pool allocated for replacements"),
        }
    }
}

// ---- the decision at Start ------------------------------------------------

fn take_off_categories(player: Hn) {
    let ours = tracker(|t| t.players.get_mut(&player).map(|p| std::mem::take(&mut p.our_categories)).unwrap_or(false));
    if ours {
        OURS.store(true, Ordering::Release);
        call!(UNSET_CATEGORY, fn(Hn) -> (), player);
        OURS.store(false, Ordering::Release);
    }
}

fn before_start(player: Hn) {
    let enabled = settings::get_bool(settings::ENABLED, true);
    let logging = settings::get_bool(settings::LOG_CUES, false);
    let (any_files, sheet_files) = library::with(|l| {
        (!l.entries.is_empty(), l.entries.keys().any(|k| k.sheet.is_some()))
    });
    let cue = tracker(|t| t.players.get(&player).and_then(|p| p.cue.clone()));
    let wanted = (enabled && any_files) || logging;
    let Some((acb, sel)) = cue.filter(|_| wanted) else {
        take_off_categories(player);
        return;
    };
    let Some(name) = cue_name(acb, &sel) else {
        take_off_categories(player);
        return;
    };
    // Reading the ACB for its name is only needed for sheet folders or the log.
    let sheet = if (enabled && sheet_files) || logging { sheet_name(acb) } else { None };
    let entry = if enabled && any_files {
        library::with(|l| l.find(sheet.as_deref(), &name).map(|(k, e)| (k.clone(), e.state.clone())))
    } else {
        None
    };
    let sound = match entry {
        Some((_, State::Ready(s))) => Some(s),
        Some((key, State::Pending)) => match library::resolve(&key) {
            State::Ready(s) => Some(s),
            State::Broken(e) => {
                api::get().warn(&format!("{name}: cannot use the replacement ({e}), playing the game's sound"));
                None
            }
            State::Pending => None,
        },
        _ => None,
    };
    let replaced = match sound {
        Some(s) => apply(player, acb, &sel, sheet.as_deref(), &s),
        None => false,
    };
    if !replaced {
        take_off_categories(player);
    }
    note_played(sheet.as_deref().unwrap_or("?"), &name, replaced);
}

/// Points the player at our data. False (nothing changed) if an export is missing.
fn apply(player: Hn, acb: Hn, sel: &Sel, sheet: Option<&str>, s: &Arc<library::Sound>) -> bool {
    if SET_DATA.get().is_none() || SET_FORMAT.get().is_none() {
        return false;
    }
    let info = cue_info(acb, sel).filter(|i| layout_ok(acb, i));
    let looped = match &info {
        Some(i) => i.i64_at(INFO_LENGTH) < 0,
        None => sheet.map(str::to_owned).or_else(|| sheet_name(acb)).is_some_and(|n| n.to_ascii_lowercase().starts_with("snd_bgm")),
    };
    let data = if looped { s.looped } else { s.once };
    ensure_pool();

    OURS.store(true, Ordering::Release);
    call!(SET_FORMAT, fn(Hn, u32) -> (), player, FORMAT_ADX);
    call!(SET_NUM_CHANNELS, fn(Hn, i32) -> (), player, s.channels as i32);
    call!(SET_SAMPLING_RATE, fn(Hn, i32) -> (), player, s.rate as i32);
    call!(SET_DATA, fn(Hn, *const c_void, i32) -> (), player, data.as_ptr() as *const c_void, data.len() as i32);

    let game_categories = tracker(|t| t.players.get(&player).is_some_and(|p| p.game_categories));
    if let (Some(info), false) = (&info, game_categories) {
        if settings::get_bool(settings::COPY_CATEGORIES, true) && CATEGORY_INFO.get().is_some() && SET_CATEGORY_BY_ID.get().is_some() {
            call!(UNSET_CATEGORY, fn(Hn) -> (), player);
            let mut any = false;
            for idx in info.categories() {
                let mut c = InfoBuf::new();
                let ok = call!(CATEGORY_INFO, fn(u16, *mut c_void) -> i32, idx, c.0.as_mut_ptr() as *mut c_void);
                if ok.is_some_and(|v| v != 0) {
                    call!(SET_CATEGORY_BY_ID, fn(Hn, u32) -> (), player, c.u32_at(CATEGORY_ID));
                    any = true;
                }
            }
            tracker(|t| t.players.entry(player).or_default().our_categories = any);
        }
    }
    OURS.store(false, Ordering::Release);
    true
}

// ---- hooks ----------------------------------------------------------------

fn guarded(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        if let Some(api) = api::try_get() {
            api.error(&format!("{what} panicked"));
        }
    }
}

fn set_cue(player: Hn, cue: Option<(Hn, Sel)>) {
    if OURS.load(Ordering::Acquire) {
        return;
    }
    guarded("set cue", || tracker(|t| t.players.entry(player).or_default().cue = cue));
}

extern "C" fn start_hook(player: Hn) -> u32 {
    guarded("start", || before_start(player));
    call!(START, fn(Hn) -> u32, player).unwrap_or(0)
}

extern "C" fn prepare_hook(player: Hn) -> u32 {
    guarded("prepare", || before_start(player));
    call!(PREPARE, fn(Hn) -> u32, player).unwrap_or(0)
}

extern "C" fn set_cue_id_hook(player: Hn, acb: Hn, id: i32) {
    set_cue(player, Some((acb, Sel::Id(id))));
    call!(SET_CUE_ID, fn(Hn, Hn, i32) -> (), player, acb, id);
}

extern "C" fn set_cue_name_hook(player: Hn, acb: Hn, name: *const c_char) {
    let n = unsafe { c_string(name) };
    set_cue(player, n.map(|n| (acb, Sel::Name(n))));
    call!(SET_CUE_NAME, fn(Hn, Hn, *const c_char) -> (), player, acb, name);
}

extern "C" fn set_cue_index_hook(player: Hn, acb: Hn, index: i32) {
    set_cue(player, Some((acb, Sel::Index(index))));
    call!(SET_CUE_INDEX, fn(Hn, Hn, i32) -> (), player, acb, index);
}

extern "C" fn set_data_hook(player: Hn, buf: *const c_void, size: i32) {
    set_cue(player, None);
    call!(SET_DATA, fn(Hn, *const c_void, i32) -> (), player, buf, size);
}

extern "C" fn set_file_hook(player: Hn, binder: Hn, path: *const c_char) {
    set_cue(player, None);
    call!(SET_FILE, fn(Hn, Hn, *const c_char) -> (), player, binder, path);
}

extern "C" fn set_wave_id_hook(player: Hn, awb: Hn, id: i32) {
    set_cue(player, None);
    call!(SET_WAVE_ID, fn(Hn, Hn, i32) -> (), player, awb, id);
}

extern "C" fn set_content_id_hook(player: Hn, binder: Hn, id: i32) {
    set_cue(player, None);
    call!(SET_CONTENT_ID, fn(Hn, Hn, i32) -> (), player, binder, id);
}

fn game_sets_categories(player: Hn) {
    if OURS.load(Ordering::Acquire) {
        return;
    }
    guarded("category", || tracker(|t| {
        let p = t.players.entry(player).or_default();
        p.game_categories = true;
        p.our_categories = false;
    }));
}

extern "C" fn set_category_by_id_hook(player: Hn, id: u32) {
    game_sets_categories(player);
    call!(SET_CATEGORY_BY_ID, fn(Hn, u32) -> (), player, id);
}

extern "C" fn set_category_by_name_hook(player: Hn, name: *const c_char) {
    game_sets_categories(player);
    call!(SET_CATEGORY_BY_NAME, fn(Hn, *const c_char) -> (), player, name);
}

extern "C" fn unset_category_hook(player: Hn) {
    game_sets_categories(player);
    call!(UNSET_CATEGORY, fn(Hn) -> (), player);
}

extern "C" fn destroy_hook(player: Hn) {
    guarded("destroy", || tracker(|t| { t.players.remove(&player); }));
    call!(DESTROY, fn(Hn) -> (), player);
}

extern "C" fn load_acb_file_hook(
    acb_binder: Hn, acb_path: *const c_char, awb_binder: Hn, awb_path: *const c_char, work: *mut c_void, size: i32,
) -> Hn {
    let acb = call!(LOAD_ACB_FILE, fn(Hn, *const c_char, Hn, *const c_char, *mut c_void, i32) -> Hn,
        acb_binder, acb_path, awb_binder, awb_path, work, size).unwrap_or(0);
    if acb != 0 {
        guarded("load acb", || {
            let path = unsafe { c_string(acb_path) };
            tracker(|t| { t.acbs.insert(acb, Acb { path, name: None }); });
        });
    }
    acb
}

extern "C" fn load_acb_data_hook(
    data: *const u8, data_size: i32, awb_binder: Hn, awb_path: *const c_char, work: *mut c_void, size: i32,
) -> Hn {
    let acb = call!(LOAD_ACB_DATA, fn(*const u8, i32, Hn, *const c_char, *mut c_void, i32) -> Hn,
        data, data_size, awb_binder, awb_path, work, size).unwrap_or(0);
    if acb != 0 {
        guarded("load acb data", || {
            let name = (!data.is_null() && data_size > 0)
                .then(|| utf::acb_name(unsafe { std::slice::from_raw_parts(data, data_size as usize) }))
                .flatten();
            tracker(|t| { t.acbs.insert(acb, Acb { path: None, name: Some(name) }); });
        });
    }
    acb
}

extern "C" fn release_acb_hook(acb: Hn) {
    guarded("release acb", || tracker(|t| {
        t.acbs.remove(&acb);
        t.players.values_mut().for_each(|p| {
            if p.cue.as_ref().is_some_and(|c| c.0 == acb) {
                p.cue = None;
            }
        });
    }));
    call!(RELEASE_ACB, fn(Hn) -> (), acb);
}

/// After the original fills `info`: report a ready replacement's length.
fn patch_length(acb: Hn, info: *mut c_void, ok: i32) {
    if ok == 0 || info.is_null() || LAYOUT.load(Ordering::Acquire) != 1 {
        return;
    }
    if !settings::get_bool(settings::ENABLED, true) || !settings::get_bool(settings::PATCH_LENGTH, true) {
        return;
    }
    guarded("cue info", || unsafe {
        let base = info as *mut u8;
        let name = c_string(*(base.add(INFO_NAME) as *const *const c_char));
        let Some(name) = name else { return };
        let len = base.add(INFO_LENGTH) as *mut i64;
        if *len < 0 {
            return; // looping cue: stays endless
        }
        let sheet = sheet_name(acb);
        let ms = library::with(|l| match l.find(sheet.as_deref(), &name).map(|(_, e)| e.state.clone()) {
            Some(State::Ready(s)) => Some(s.millis),
            _ => None,
        });
        if let Some(ms) = ms {
            *len = ms;
        }
    });
}

extern "C" fn cue_info_by_id_hook(acb: Hn, id: i32, info: *mut c_void) -> i32 {
    let ok = call!(CUE_INFO_BY_ID, fn(Hn, i32, *mut c_void) -> i32, acb, id, info).unwrap_or(0);
    patch_length(acb, info, ok);
    ok
}

extern "C" fn cue_info_by_name_hook(acb: Hn, name: *const c_char, info: *mut c_void) -> i32 {
    let ok = call!(CUE_INFO_BY_NAME, fn(Hn, *const c_char, *mut c_void) -> i32, acb, name, info).unwrap_or(0);
    patch_length(acb, info, ok);
    ok
}

extern "C" fn cue_info_by_index_hook(acb: Hn, index: i32, info: *mut c_void) -> i32 {
    let ok = call!(CUE_INFO_BY_INDEX, fn(Hn, i32, *mut c_void) -> i32, acb, index, info).unwrap_or(0);
    patch_length(acb, info, ok);
    ok
}

// ---- install --------------------------------------------------------------

/// Hooks and resolves everything; returns a one-line report.
pub fn install() -> Result<String, String> {
    if INSTALLED.load(Ordering::Acquire) {
        return Ok("already installed".into());
    }
    let api = api::get();
    let module = sys::module(DLL).ok_or_else(|| format!("{DLL} is not loaded"))?;
    let interceptor = unsafe { (api.hachimi_get_interceptor)((api.hachimi_instance)()) };
    let mut missing: Vec<&str> = Vec::new();

    let mut hook = |name: &'static str, slot: &Native, detour: usize, required: bool| -> bool {
        let Some(addr) = sys::export(module, name) else {
            missing.push(name);
            return !required;
        };
        let tramp = unsafe { (api.interceptor_hook)(interceptor, addr as *mut c_void, detour as *mut c_void) };
        if tramp.is_null() {
            api.warn(&format!("cannot hook {name}"));
            missing.push(name);
            return !required;
        }
        slot.set(tramp as usize);
        true
    };

    // The cue tracking needs all of these; without them nothing is hooked further.
    let core = hook("criAtomExPlayer_SetCueId", &SET_CUE_ID, set_cue_id_hook as *const () as usize, true)
        && hook("criAtomExPlayer_SetCueName", &SET_CUE_NAME, set_cue_name_hook as *const () as usize, true)
        && hook("criAtomExPlayer_SetCueIndex", &SET_CUE_INDEX, set_cue_index_hook as *const () as usize, true)
        && hook("criAtomExPlayer_Start", &START, start_hook as *const () as usize, true);
    if !core {
        return Err(format!("missing in {DLL}: {}", missing.join(", ")));
    }
    hook("criAtomExPlayer_Prepare", &PREPARE, prepare_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetData", &SET_DATA, set_data_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetFile", &SET_FILE, set_file_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetWaveId", &SET_WAVE_ID, set_wave_id_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetContentId", &SET_CONTENT_ID, set_content_id_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetCategoryById", &SET_CATEGORY_BY_ID, set_category_by_id_hook as *const () as usize, false);
    hook("criAtomExPlayer_SetCategoryByName", &SET_CATEGORY_BY_NAME, set_category_by_name_hook as *const () as usize, false);
    hook("criAtomExPlayer_UnsetCategory", &UNSET_CATEGORY, unset_category_hook as *const () as usize, false);
    hook("criAtomExPlayer_Destroy", &DESTROY, destroy_hook as *const () as usize, false);
    hook("criAtomExAcb_LoadAcbFile", &LOAD_ACB_FILE, load_acb_file_hook as *const () as usize, false);
    hook("criAtomExAcb_LoadAcbData", &LOAD_ACB_DATA, load_acb_data_hook as *const () as usize, false);
    hook("criAtomExAcb_Release", &RELEASE_ACB, release_acb_hook as *const () as usize, false);
    hook("criAtomExAcb_GetCueInfoById", &CUE_INFO_BY_ID, cue_info_by_id_hook as *const () as usize, false);
    hook("criAtomExAcb_GetCueInfoByName", &CUE_INFO_BY_NAME, cue_info_by_name_hook as *const () as usize, false);
    hook("criAtomExAcb_GetCueInfoByIndex", &CUE_INFO_BY_INDEX, cue_info_by_index_hook as *const () as usize, false);

    let mut plain = |name: &'static str, slot: &Native| match sys::export(module, name) {
        Some(a) => slot.set(a),
        None => missing.push(name),
    };
    plain("criAtomExPlayer_SetFormat", &SET_FORMAT);
    plain("criAtomExPlayer_SetNumChannels", &SET_NUM_CHANNELS);
    plain("criAtomExPlayer_SetSamplingRate", &SET_SAMPLING_RATE);
    plain("criAtomExAcb_GetCueNameById", &CUE_NAME_BY_ID);
    plain("criAtomExAcb_GetCueNameByIndex", &CUE_NAME_BY_INDEX);
    plain("criAtomExAcf_GetCategoryInfo", &CATEGORY_INFO);
    plain("criAtomExVoicePool_AllocateStandardVoicePool", &ALLOC_STANDARD_POOL);

    INSTALLED.store(true, Ordering::Release);
    if SET_DATA.get().is_none() || SET_FORMAT.get().is_none() {
        return Err(format!("hooks in place but cannot play replacements; missing: {}", missing.join(", ")));
    }
    Ok(if missing.is_empty() {
        "audio hooks installed".into()
    } else {
        format!("audio hooks installed; missing (features off): {}", missing.join(", "))
    })
}
