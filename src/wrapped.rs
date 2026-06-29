pub mod libc;

macro_rules! wrapped_landing_pad {
    ($name:ident, $func:ident) => {
        #[unsafe(naked)]
        pub extern "C" fn $name() {
            std::arch::naked_asm!(
                "sub rsp, 8",
                "mov [r15 + {}], r10",
                "call {}",
                "mov r10, [r15 + {}]",
                "add rsp, 8",
                const $crate::runner::ExecCtx::PARAM_OFFSET,
                sym $func,
                const $crate::runner::ExecCtx::PARAM_OFFSET,
            )
        }
    };
}

pub(self) use wrapped_landing_pad;
