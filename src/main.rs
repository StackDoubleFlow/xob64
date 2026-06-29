mod loader;
mod runner;
mod wrapped;

use std::{env, ffi::CString, os::unix::ffi::OsStringExt};

fn main() {
    let args: Vec<_> = env::args_os()
        .skip(1)
        .map(|str| CString::new(str.into_vec()).unwrap())
        .collect();
    let exec_name = args.get(0).expect("expected name of executable");

    loader::load_object(&exec_name);
    let start_sym = loader::get_symbol(c"_start").expect("could not find _start symbol");
    runner::call(start_sym);
}
