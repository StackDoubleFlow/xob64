mod loader;

use std::{
    env,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use object::read::elf::ElfFile64;

fn main() {
    let mut args = env::args();
    let exec_name = args.next().expect("expected name of executable");
    let exec_args: Vec<String> = args.collect();
}
