mod loader;
mod runner;
mod wrapped;

use std::{env, ffi::CString, os::unix::ffi::OsStringExt};

fn main() {
    let mut args = env::args_os().skip(1);
    let exec_name = args.next().expect("expected name of executable");
    let exec_name = CString::new(exec_name.into_vec()).unwrap();
    let args: Vec<*const u8> = args
        .map(|str| {
            let mut bytes = str.into_vec();
            // Add null terminator
            bytes.push(0);
            Box::leak(bytes.into_boxed_slice()).as_ptr()
        })
        .collect();

    loader::load_object(&exec_name, &args);
    let start_sym = loader::get_symbol(c"_start").expect("could not find _start symbol");
    runner::call(start_sym, &args);
}
