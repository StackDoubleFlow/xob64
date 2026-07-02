use crate::{
    loader::SymbolTable,
    runner::{ExecCtx, get_exec},
    wrapped::wrapped_landing_pad,
};

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    symbol_table.insert_global(c"__libc_start_main", __libc_start_main as *const ());
    symbol_table.insert_global(c"abort", abort as *const ());
    symbol_table.insert_global(c"puts", puts as *const ());
}

wrapped_landing_pad!(__libc_start_main, __libc_start_main_impl);
extern "C" fn __libc_start_main_impl(main_fn: *const u8, argc: u32, argv: *const *const u8) {
    let mut ctx = ExecCtx::new();
    let ctx_ptr = &mut ctx as *mut ExecCtx;
    let target = get_exec(main_fn);

    eprintln!("calling main: {:?} -> {:?}", main_fn, target);
    let result: i32;
    unsafe {
        std::arch::asm!(
            "mov edi, {argc:e}",
            "mov rsi, {argv}",
            "mov r15, {ctx_ptr}",
            // Load link register
            "lea r11, [rip + 2f]",
            // Jump to emulation
            "jmp {main_fn}",
            "2:",
            ctx_ptr = in(reg) ctx_ptr,
            main_fn = in(reg) target,
            argc = in(reg) argc,
            argv = in(reg) argv,
            out("rdi") result,
            clobber_abi("C"),
            out("r15") _
        )
    }
    std::process::exit(result);
}

wrapped_landing_pad!(abort, abort_impl);
extern "C" fn abort_impl() {
    std::process::abort();
}

wrapped_landing_pad!(puts, puts_impl);
extern "C" fn puts_impl(str: *const i8) -> i32 {
    unsafe { nix::libc::puts(str) }
}
