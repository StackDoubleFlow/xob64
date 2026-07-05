use iced_x86::code_asm::*;

use crate::runner::{self, ExecCtx, callbacks};

pub fn create_lib_proxy(target: u64) -> Result<*const u8, IcedError> {
    let mut ass = CodeAssembler::new(64)?;

    ass.sub(rsp, 16)?;
    ass.mov(ptr(rsp), r10)?; // x23 is callee-saved but r10 is temporary
    ass.mov(ptr(rsp + 8), r11)?; // link register
    ass.mov(rax, target)?;
    ass.call(rax)?;
    ass.mov(r10, ptr(rsp))?;
    ass.mov(r11, ptr(rsp + 8))?;
    ass.add(rsp, 16)?;
    ass.mov(rdi, rax)?; // Return value
    // Shadow stack return requence
    ass.mov(rdx, ptr(r15 + ExecCtx::SHADOW_SP_OFFSET))?;
    ass.mov(rax, ptr(rdx + 8))?;
    ass.cmp(r11, rax)?;
    ass.mov(rax, ptr(rdx))?;
    ass.lea(rdx, ptr(rdx + 16))?;
    ass.mov(ptr(r15 + ExecCtx::SHADOW_SP_OFFSET), rdx)?;
    let shadow_stack_miss = ass.fwd()?;
    ass.jne(shadow_stack_miss)?;
    ass.jmp(rax)?;
    ass.anonymous_label()?;
    ass.mov(ptr(r15 + ExecCtx::PARAM_OFFSET), r11)?;
    ass.mov(
        rax,
        callbacks::indirect_jump_landing_pad as *const () as u64,
    )?;
    ass.call(rax)?;

    Ok(runner::alloc_code(ass))
}
