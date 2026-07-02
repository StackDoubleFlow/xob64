use nix::libc;

use crate::{
    loader::SymbolTable,
    runner::{ExecCtx, get_exec},
    wrapped::{load_proxy, wrapped_landing_pad, wrapped_lib_proxy},
};

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    unsafe {
        let handle = libc::dlopen(c"libc.so".as_ptr(), libc::RTLD_LAZY);
        load_proxy(symbol_table, handle, &abort::INFO);
        load_proxy(symbol_table, handle, &puts::INFO);
    }
    symbol_table.insert_global(c"__libc_start_main", __libc_start_main as *const ());
}

wrapped_lib_proxy!(abort, c"abort");
wrapped_lib_proxy!(puts, c"puts");

wrapped_landing_pad!(__libc_start_main, __libc_start_main_impl);
extern "C" fn __libc_start_main_impl(main_fn: *const u8, argc: u32, argv: *const *const u8) {
    let mut ctx = ExecCtx::new();
    ctx.push_shadow_stack(std::ptr::null(), std::ptr::null());
    let ctx_ptr = &mut ctx as *mut ExecCtx;
    let target = get_exec(main_fn);

    eprintln!("calling main: {:?} -> {:?}", main_fn, target);
    let mut result = argc;
    unsafe {
        std::arch::asm!(
            // Load link register
            "lea r11, [rip + 2f]",
            // Store return address on shadow stack
            "mov {temp}, [r15 + {shadow_sp}]",
            "mov [{temp} + 8], r11",
            "mov [{temp}], r11",
            // Jump to emulation
            "jmp {main_fn}",
            "2:",
            main_fn = in(reg) target,
            shadow_sp = const ExecCtx::SHADOW_SP_OFFSET,
            temp = in(reg) 0u64,
            in("r15") ctx_ptr,
            in("rsi") argv,
            inout("edi") result,
            clobber_abi("C"),
        )
    }
    std::process::exit(result as i32);
}
