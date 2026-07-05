use std::{collections::HashMap, ffi::CStr};

mod libc;

pub fn get_overrides(name: &CStr) -> HashMap<&'static CStr, *const u8> {
    if let Ok(name) = name.to_str() {
        if name.starts_with("libc.so") {
            return libc::get_overrides();
        }
    }

    HashMap::new()
}
