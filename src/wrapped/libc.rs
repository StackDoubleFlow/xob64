use crate::{loader::SymbolTable, wrapped::wrapped_landing_pad};

wrapped_landing_pad!(__libc_start_main, __libc_start_main_impl);

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    symbol_table.insert_global(c"__libc_start_main", __libc_start_main as *const ());
}

extern "C" fn __libc_start_main_impl() {}
