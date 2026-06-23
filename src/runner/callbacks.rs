use crate::runner::ExecCtx;

pub extern "C" fn invalid_arm_instr() {
    eprintln!("todo: invalid arm instruction");
    std::process::abort();
}

#[unsafe(naked)]
pub extern "C" fn unimplemented_arm_instr_landing_pad() {
    std::arch::naked_asm!(
        "mov rdi, rax",
        "mov rsi, [rsp]",
        "call {}",
        sym unimplemented_arm_instr
    )
}

extern "C" fn unimplemented_arm_instr(ctx: *mut ExecCtx, ret_ptr: *const u8) {
    eprintln!("todo: unimplemented arm instruction");
    eprintln!("ret ptr: {:?}", ret_ptr);
    std::process::abort();
}

pub extern "C" fn end_of_chunk() {
    eprintln!("todo: end of chunk");
    std::process::abort();
}
