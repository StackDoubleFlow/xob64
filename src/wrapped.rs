pub mod libc;

macro_rules! wrapped_landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            std::arch::naked_asm!(
                "sub rsp, 16",
                "mov [rsp], r10", // x23 is callee-saved but r10 is temporary
                "mov [rsp + 8], r11", // link register
                "call {target}",
                "mov r10, [rsp]",
                "mov r11, [rsp + 8]",
                "add rsp, 16",
                "mov rdi, rax", // Return value
                // Shadow stack return sequence
                "mov rdx, [r15 + {shadow_sp}]",
                "mov rax, [rdx + 8]",
                "cmp r11, rax",
                "mov rax, [rdx]",
                "lea rdx, [rdx + 16]",
                "mov [r15 + {shadow_sp}], rdx",
                "jne 2f",
                "jmp rax",
                "2: mov [r15 + {param_offset}], r11",
                "call {indirect_jump}",
                target = sym $func,
                param_offset = const $crate::runner::ExecCtx::PARAM_OFFSET,
                shadow_sp = const $crate::runner::ExecCtx::SHADOW_SP_OFFSET,
                indirect_jump = sym $crate::runner::callbacks::indirect_jump_landing_pad
            )
        }
    };
}

pub(self) use wrapped_landing_pad;
