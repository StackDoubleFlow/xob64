use nix::libc;

use crate::{
    loader::SymbolTable,
    runner::{ExecCtx, get_exec},
    wrapped::{dlopen, load_direct, load_proxy, wrapped_landing_pad, wrapped_lib_proxy},
};

pub fn register_symbols(symbol_table: &mut SymbolTable) {
    let handle = dlopen(c"libc.so");
    load_proxy(symbol_table, handle, &abort::INFO);
    load_proxy(symbol_table, handle, &puts::INFO);
    load_proxy(symbol_table, handle, &memcpy::INFO);
    load_proxy(symbol_table, handle, &memmove::INFO);
    load_proxy(symbol_table, handle, &_exit::INFO);
    load_proxy(symbol_table, handle, &getcwd::INFO);
    load_proxy(symbol_table, handle, &fwrite_unlocked::INFO);
    load_proxy(symbol_table, handle, &strlen::INFO);
    load_proxy(symbol_table, handle, &__sprintf_chk::INFO);
    load_proxy(symbol_table, handle, &exit::INFO);
    load_proxy(symbol_table, handle, &_setjmp::INFO);
    load_proxy(symbol_table, handle, &raise::INFO);
    load_direct(symbol_table, handle, c"program_invocation_name");
    load_proxy(symbol_table, handle, &sigprocmask::INFO);
    load_proxy(symbol_table, handle, &strnlen::INFO);
    load_proxy(symbol_table, handle, &localtime_r::INFO);
    load_proxy(symbol_table, handle, &setenv::INFO);
    load_proxy(symbol_table, handle, &readlink::INFO);
    load_proxy(symbol_table, handle, &getgrnam::INFO);
    load_proxy(symbol_table, handle, &opendir::INFO);
    load_proxy(symbol_table, handle, &strftime::INFO);
    load_proxy(symbol_table, handle, &iswcntrl::INFO);
    load_proxy(symbol_table, handle, &clock_gettime::INFO);
    load_direct(symbol_table, handle, c"stdin");
    load_direct(symbol_table, handle, c"stdout");
    load_direct(symbol_table, handle, c"stderr");
    load_proxy(symbol_table, handle, &__readlink_chk::INFO);
    load_proxy(symbol_table, handle, &__fpending::INFO);
    load_direct(symbol_table, handle, c"optarg");
    symbol_table.insert_global(c"__libc_start_main", __libc_start_main as *const ());
}

wrapped_lib_proxy!(abort, c"abort");
wrapped_lib_proxy!(puts, c"puts");
wrapped_lib_proxy!(memcpy, c"memcpy");
wrapped_lib_proxy!(memmove, c"memmove");
wrapped_lib_proxy!(_exit, c"_exit");
wrapped_lib_proxy!(getcwd, c"getcwd");
wrapped_lib_proxy!(fwrite_unlocked, c"fwrite_unlocked");
wrapped_lib_proxy!(strlen, c"strlen");
wrapped_lib_proxy!(__sprintf_chk, c"__sprintf_chk");
wrapped_lib_proxy!(exit, c"exit");
wrapped_lib_proxy!(_setjmp, c"_setjmp");
wrapped_lib_proxy!(raise, c"raise");
wrapped_lib_proxy!(sigprocmask, c"sigprocmask");
wrapped_lib_proxy!(strnlen, c"strnlen");
wrapped_lib_proxy!(localtime_r, c"localtime_r");
wrapped_lib_proxy!(setenv, c"setenv");
wrapped_lib_proxy!(readlink, c"readlink");
wrapped_lib_proxy!(getgrnam, c"getgrnam");
wrapped_lib_proxy!(opendir, c"opendir");
wrapped_lib_proxy!(strftime, c"strftime");
wrapped_lib_proxy!(iswcntrl, c"iswcntrl");
wrapped_lib_proxy!(clock_gettime, c"clock_gettime");
wrapped_lib_proxy!(__readlink_chk, c"__readlink_chk");
wrapped_lib_proxy!(__fpending, c"__fpending");

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
