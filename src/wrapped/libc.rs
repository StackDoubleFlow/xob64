use crate::{loader::SymbolTable, wrapped::wrapped_landing_pad};

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    symbol_table.insert_global(c"__libc_start_main", __libc_start_main as *const ());
    symbol_table.insert_global(c"abort", abort as *const ());
    symbol_table.insert_global(c"puts", puts as *const ());
}

wrapped_landing_pad!(__libc_start_main, __libc_start_main_impl);
extern "C" fn __libc_start_main_impl(
    main_fn: extern "C" fn(u32, *const *const u8, *const *const u8),
    argc: u32,
    argv: *const *const u8,
) {
    dbg!(argc, argv);
}

wrapped_landing_pad!(abort, abort_impl);
extern "C" fn abort_impl() {
    std::process::abort();
}

wrapped_landing_pad!(puts, puts_impl);
extern "C" fn puts_impl(str: *const u8) {
    dbg!(str);
}
