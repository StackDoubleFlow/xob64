pub mod libc;

macro_rules! wrapped_landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            std::arch::naked_asm!(
                "sub rsp, 16",
                "mov [rsp], r10",
                "mov [rsp + 8], r11",
                "call {}",
                "mov r10, [rsp]",
                "mov r11, [rsp + 8]",
                "add rsp, 16",
                "mov rdi, rax", // Return value
                "mov [r15 + {}], r11",
                "call {}",
                sym $func,
                const $crate::runner::ExecCtx::PARAM_OFFSET,
                sym $crate::runner::callbacks::indirect_jump_landing_pad
            )
        }
    };
}

pub(self) use wrapped_landing_pad;
