mod loader;
mod runner;

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let exec_name = args.next().expect("expected name of executable");
    let exec_args: Vec<String> = args.collect();

    loader::load_object(&exec_name);
}
