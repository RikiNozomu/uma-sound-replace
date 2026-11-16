//! The few Win32 calls the plugin needs, declared by hand.

#![cfg(windows)]

use std::ffi::{c_char, c_void};

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// Handle of an already loaded DLL, if it is loaded.
pub fn module(name: &str) -> Option<*mut c_void> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe { GetModuleHandleW(wide.as_ptr()) };
    (!h.is_null()).then_some(h)
}

/// Address of an export, if present.
pub fn export(module: *mut c_void, name: &str) -> Option<usize> {
    let name = std::ffi::CString::new(name).ok()?;
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then_some(p as usize)
}
