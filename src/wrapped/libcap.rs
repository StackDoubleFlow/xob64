use crate::{
    loader::SymbolTable,
    wrapped::{dlopen, load_proxy, wrapped_lib_proxy},
};

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    let handle = dlopen(c"libcap.so");
    load_proxy(symbol_table, handle, &cap_to_text::INFO);
}

wrapped_lib_proxy!(cap_to_text, c"cap_to_text");
