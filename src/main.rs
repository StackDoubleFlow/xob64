macro_rules! debug_print {
    ($($arg:tt)*) => {
        $crate::debug_log(format_args!($($arg)*));
    };
}

macro_rules! debug_println {
    () => {
        debug_print!("\n");
    };
    ($($arg:tt)*) => {
        debug_print!($($arg)*);
        debug_print!("\n");
    };
}

mod loader;
mod runner;
mod wrapped;

use std::{
    env,
    ffi::CString,
    fs::File,
    os::unix::ffi::OsStringExt,
    sync::{LazyLock, Mutex},
};

pub static DEBUG_FILE: LazyLock<Mutex<File>> =
    LazyLock::new(|| Mutex::new(File::create("test.log").unwrap()));

fn debug_log(args: std::fmt::Arguments) {
    use std::io::Write;
    let mut file = DEBUG_FILE.lock().unwrap();
    write!(file, "{}", args).unwrap();
}

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
