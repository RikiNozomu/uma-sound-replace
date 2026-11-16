//! Bindings to the Hachimi Edge plugin API (v3, resolved by name through `get_api`).
//! Signatures mirror `src/core/plugin_api.rs` in kairusds/Hachimi-Edge.

use std::ffi::{c_char, c_void, CString};
use std::sync::OnceLock;

pub type GetApiFn = extern "C" fn(name: *const c_char) -> *mut c_void;
pub type UiCallback = extern "C" fn(ui: *mut c_void, userdata: *mut c_void);
pub type GameInitializedCallback = unsafe extern "C" fn(userdata: *mut c_void);

macro_rules! api_struct {
    ($( $name:ident : $ty:ty ),* $(,)?) => {
        #[allow(dead_code)] // some entries are Windows-only
        pub struct Api { $( pub $name: $ty, )* }

        impl Api {
            /// Resolves every function we use. Returns None if the host lacks one.
            pub fn load(get_api: GetApiFn) -> Option<Api> {
                Some(Api { $(
                    $name: {
                        let p = get_api(concat!(stringify!($name), "\0").as_ptr() as *const c_char);
                        if p.is_null() { return None; }
                        unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
                    },
                )* })
            }
        }
    };
}

api_struct! {
    hachimi_instance: unsafe extern "C" fn() -> *const c_void,
    hachimi_get_interceptor: unsafe extern "C" fn(this: *const c_void) -> *const c_void,
    hachimi_register_on_game_initialized:
        unsafe extern "C" fn(cb: Option<GameInitializedCallback>, userdata: *mut c_void) -> bool,
    hachimi_get_base_dir: unsafe extern "C" fn() -> *const c_char,

    // Returns the trampoline (call it to run the original), or null on failure.
    interceptor_hook:
        unsafe extern "C" fn(this: *const c_void, orig: *mut c_void, hook: *mut c_void) -> *mut c_void,

    il2cpp_get_assembly_image: unsafe extern "C" fn(name: *const c_char) -> *const c_void,

    log: unsafe extern "C" fn(level: i32, target: *const c_char, message: *const c_char),

    gui_register_menu_section:
        unsafe extern "C" fn(cb: Option<UiCallback>, userdata: *mut c_void) -> bool,
    gui_show_notification: unsafe extern "C" fn(message: *const c_char) -> bool,
    gui_ui_heading: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_label: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_small: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_separator: unsafe extern "C" fn(ui: *mut c_void) -> bool,
    gui_ui_checkbox:
        unsafe extern "C" fn(ui: *mut c_void, text: *const c_char, value: *mut bool) -> bool,
}

static API: OnceLock<Api> = OnceLock::new();
static GET_API: OnceLock<GetApiFn> = OnceLock::new();

/// Newer menu widgets, absent from older Hachimi builds: resolved one by one,
/// None when missing (unlike `Api`, whose every entry is required).
pub struct Extra {
    pub gui_ui_button: Option<unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool>,
}

/// Remembers `get_api` for `extra()`.
pub fn set_get_api(get_api: GetApiFn) {
    let _ = GET_API.set(get_api);
}

pub fn extra() -> &'static Extra {
    static EXTRA: OnceLock<Extra> = OnceLock::new();
    EXTRA.get_or_init(|| {
        let lookup = |name: &str| -> *mut c_void {
            match GET_API.get() {
                Some(get) => get(cstr(name).as_ptr()),
                None => std::ptr::null_mut(),
            }
        };
        unsafe {
            let button = lookup("gui_ui_button");
            Extra { gui_ui_button: (!button.is_null()).then(|| std::mem::transmute(button)) }
        }
    })
}

pub fn set(api: Api) -> &'static Api {
    API.get_or_init(|| api)
}

/// Panics only if called before `hachimi_init_v3`, which cannot happen:
/// every caller is a callback registered from there.
pub fn try_get() -> Option<&'static Api> {
    API.get()
}

pub fn get() -> &'static Api {
    API.get().expect("api not initialized")
}

/// Lossy CString: interior NULs are dropped instead of failing.
pub fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Api {
    fn log_at(&self, level: i32, msg: &str) {
        let tag = cstr(crate::TAG);
        let msg = cstr(msg);
        unsafe { (self.log)(level, tag.as_ptr(), msg.as_ptr()) }
    }
    pub fn error(&self, msg: &str) { self.log_at(1, msg) }
    pub fn warn(&self, msg: &str) { self.log_at(2, msg) }
    pub fn info(&self, msg: &str) { self.log_at(3, msg) }

    pub fn notify(&self, msg: &str) {
        let msg = cstr(msg);
        unsafe { (self.gui_show_notification)(msg.as_ptr()); }
    }

}
