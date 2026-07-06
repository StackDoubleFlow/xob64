use crate::runner::compiler::branch;
use crate::runner::{ExecCtx, from_exec, get_exec};

macro_rules! landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        pub extern "C" fn $name() {
            std::arch::naked_asm!(
                "mov rdi, r15",
                "mov rsi, [rsp]",
                "sub rsp, 8",
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
                // The extra 8 bytes are to align the stack to 16 bytes
                "sub rsp, 72",
                "call {}",
                // Also get rid of the return address
                "add rsp, 80",
                "mov rcx, [rsp - 16]",
                "mov rdx, [rsp - 24]",
                "mov rsi, [rsp - 32]",
                "mov rdi, [rsp - 40]",
                "mov r8, [rsp - 48]",
                "mov r9, [rsp - 56]",
                "mov r10, [rsp - 64]",
                "mov r11, [rsp - 72]",
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

    // 12 is the length of the mov + call
    let arm_ptr = from_exec(ret_ptr.wrapping_sub(12));
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
    // 12 is the length of the mov + call
    let call_ptr = ret_ptr.wrapping_sub(12);
    let arm_ptr = from_exec(call_ptr);
    let arm_code = unsafe { *(arm_ptr as *const u32) };
    let arm_instr = bad64::decode(arm_code, arm_ptr as u64).expect("failed to decode instr");

    eprintln!(
        "rewriting branch at {:?}: {}",
        arm_ptr,
        // We should never fail to decode here since that an invalid instruction would have triggered the invalid_arm_instr callback.
        // If it does, it might indicate that the code has changed/currupted.
        arm_instr
    );

    branch::rewrite_branch(&arm_instr, call_ptr) as u64
}

resumable_landing_pad!(indirect_jump_landing_pad, indirect_jump);
extern "C" fn indirect_jump(ctx: *mut ExecCtx, ret_ptr: *const u8) -> u64 {
    let ctx = unsafe { &*ctx };

    let target_addr = get_exec(ctx.param as *const u8);

    // 12 is the length of the mov + call
    let call_ptr = ret_ptr.wrapping_sub(12);
    eprintln!(
        "indirect jump at {:?} to {:#x} -> {:#x}",
        call_ptr, ctx.param, target_addr as u64
    );
    target_addr as u64
}

resumable_landing_pad!(end_of_chunk_landing_pad, end_of_chunk);
extern "C" fn end_of_chunk(_ctx: *mut ExecCtx, ret_ptr: *const u8) -> u64 {
    // 12 is the length of the mov + call
    let call_ptr = ret_ptr.wrapping_sub(12);
    let arm_ptr = from_exec(call_ptr);
    let next_arm_ptr = arm_ptr.wrapping_byte_add(4);
    let next_chunk_start = get_exec(next_arm_ptr);
    branch::write_jump(call_ptr, next_chunk_start as u64);
    eprintln!(
        "rewrite end of chunk {:?}, jump to {:?}",
        call_ptr, next_chunk_start
    );
    next_chunk_start as u64
}
