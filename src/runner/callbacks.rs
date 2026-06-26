use crate::runner::{ExecCtx, from_exec};

macro_rules! landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        pub extern "C" fn $name() {
            std::arch::naked_asm!(
                "mov rdi, r15",
                "mov rsi, [rsp]",
                "call {}",
                sym $func
            )
        }
    };
}

macro_rules! resumable_landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        pub extern "C" fn $name() {
            // We need to save all registers except:
            // - %rbx, %rbp, %r12-%r15 (callee-saved, so our callback func will save them for us)
            // - %rax (scratch)
            std::arch::naked_asm!(
                "mov [rsp - 8], rcx",
                "mov [rsp - 16], rdx",
                "mov [rsp - 24], rsi",
                "mov [rsp - 32], rdi",
                "mov [rsp - 40], r8",
                "mov [rsp - 48], r9",
                "mov [rsp - 56], r10",
                "mov [rsp - 64], r11",
                "mov rdi, r15",
                "mov rsi, [rsp]",
                "sub rsp, 64",
                "call {}",
                // Also get rid of the return address
                "add rsp, 72",
                "mov rcx, [rsp - 8]",
                "mov rdx, [rsp - 16]",
                "mov rsi, [rsp - 24]",
                "mov rdi, [rsp - 32]",
                "mov r8, [rsp - 40]",
                "mov r9, [rsp - 48]",
                "mov r10, [rsp - 56]",
                "mov r11, [rsp - 64]",
                "jmp rax",
                sym $func
            )
        }
    };
}

pub extern "C" fn invalid_arm_instr() {
    eprintln!("todo: invalid arm instruction");
    std::process::abort();
}

landing_pad!(unimplemented_arm_instr_landing_pad, unimplemented_arm_instr);
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

resumable_landing_pad!(rewrite_branch_landing_pad, rewrite_branch);
extern "C" fn rewrite_branch(_ctx: *mut ExecCtx, ret_ptr: *const u8) -> u64 {
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
