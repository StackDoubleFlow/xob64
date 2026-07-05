use std::{collections::HashMap, ffi::CStr};

use crate::{
    runner::{ExecCtx, get_exec},
    wrapped::wrapped_landing_pad,
};

pub fn get_overrides() -> HashMap<&'static CStr, *const u8> {
    let mut overrides = HashMap::new();
    overrides.insert(c"__libc_start_main", __libc_start_main as _);
    overrides.insert(c"__stack_chk_guard", &raw mut __STACK_CHK_GUARD as _);
    overrides
}

static mut __STACK_CHK_GUARD: u64 = 0;

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
