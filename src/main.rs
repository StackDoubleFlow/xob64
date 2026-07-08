mod loader;
mod runner;
mod wrapped;

use std::{env, ffi::CString, os::unix::ffi::OsStringExt};

fn main() {
    let exec_name = env::args_os()
        .skip(1)
        .next()
        .expect("expected name of executable");
    let exec_name = CString::new(exec_name.into_vec()).unwrap();

    let args = env::args_os().skip(1);
    let args: Vec<*const u8> = args
        .map(|str| {
            let mut bytes = str.into_vec();
            // Add null terminator
            bytes.push(0);
            Box::leak(bytes.into_boxed_slice()).as_ptr()
        })
        .collect();

    let obj_idx = loader::load_object(&exec_name, &args);
    let entry = loader::get_entry(obj_idx).expect("could not find program entry");
    runner::call(entry, &args);
}
