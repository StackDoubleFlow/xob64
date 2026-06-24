use crate::runner::{ExecCtx, from_exec};

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

extern "C" fn unimplemented_arm_instr(_ctx: *mut ExecCtx, ret_ptr: *const u8) {
    eprintln!("todo: unimplemented arm instruction");
    let arm_ptr = from_exec(ret_ptr).wrapping_sub(4);
    let arm_code = unsafe { *(arm_ptr as *const u32) };
    eprintln!(
        "{:?}: {}",
        arm_ptr,
        // We should never fail to decode here since that an invalid instruction would have triggered the invalid_arm_instr callback.
        // If it does, it might indicate that the code has changed/currupted.
        bad64::decode(arm_code, arm_ptr as u64).expect("failed to decode instr")
    );
    std::process::abort();
}

pub extern "C" fn end_of_chunk() {
    eprintln!("todo: end of chunk");
    std::process::abort();
}
