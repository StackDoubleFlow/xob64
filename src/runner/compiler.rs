mod dump;

use iced_x86::{
    BlockEncoderOptions, Register,
    code_asm::{CodeAssembler, gpr64},
};
use num_traits::FromPrimitive;

use crate::runner::{CHUNK_SIZE, CompiledChunk, EXECUTABLE_ALLOC_SIZE, ExecPool, callbacks};

// Allocation for all 16 integer x86_64 registers:
// x0 -> %rdi (1st argument)
// x31 (sp) -> %rsp (stack pointer)
// x1 -> %rsi (2nd argument)
// x2 -> %rdx (3rd argument)
// x19 -> %rbx (1st callee-saved)
// x3 -> %rcx (4th argument)
// x20 -> %r12 (2nd callee-saved)
// x21 -> %r13 (3rd callee-saved)
// x4 -> %r8 (5th argument)
// x29 (fp) -> %rbp (frame pointer)
// x22 -> %r14 (4th callee-saved)
// x5 -> %r9 (6th argument)
// x23 -> %r10 (5th callee-saved -> temporary)
// x30 -> %r11 (link register -> temporary)
// %rax (emulation scratch)
// %r15 (emulation context)

// Allocation for all 16 fp registers:
// v0-v7 -> %xmm0-xmm7 (argument/return value)
// v16-v22 -> %xmm8-xmm14 (temporary)
// %xmm15 -> (emulation scratch)

// Translated Aarch64 registers:
// x0-x5, x19-x23, x29-x31
// v0-v7, v16-v23
// Emulated Aarch64 registers:
// x6-x18, x24-x28 (18 64-bit registers)
// v8-v15, v23-v31 (17 128-bit registers)

fn alloc_new_region(exec_pool: &mut ExecPool) {
    if !exec_pool.current_alloc.is_null() {
        exec_pool.fully_used_allocs.push(exec_pool.current_alloc);
    }

    let new_alloc = unsafe {
        nix::libc::mmap(
            std::ptr::null_mut(),
            EXECUTABLE_ALLOC_SIZE,
            nix::libc::PROT_EXEC | nix::libc::PROT_WRITE,
            nix::libc::MAP_ANONYMOUS | nix::libc::MAP_PRIVATE,
            -1,
            0,
        )
    };
    if new_alloc as isize == -1 {
        unsafe {
            nix::libc::perror(c"exec mmap failed".as_ptr());
        }
        panic!("exec mmap failed");
    }
    exec_pool.current_alloc = new_alloc as *const u8;
    exec_pool.current_alloc_utilization = 0;
}

fn finalize_ass(
    exec_pool: &mut ExecPool,
    mut ass: CodeAssembler,
    x86_idxs: &[usize],
    epilogue_offset: u16,
) -> CompiledChunk {
    if exec_pool.current_alloc.is_null() {
        alloc_new_region(exec_pool);
    }

    // FIXME: why does this take in &mut ass
    let enc_result = ass
        .assemble_options(
            exec_pool.current_alloc as u64,
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )
        .unwrap()
        .inner;

    // If this block exceeds the current allocation, allocate a new one and re-encode with the new rip.
    let enc_result = if enc_result.code_buffer.len() + exec_pool.current_alloc_utilization
        > EXECUTABLE_ALLOC_SIZE
    {
        alloc_new_region(exec_pool);
        ass.assemble_options(
            exec_pool.current_alloc as u64,
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )
        .unwrap()
        .inner
    } else {
        enc_result
    };

    // Copy to executable memory
    let target_addr = unsafe {
        exec_pool
            .current_alloc
            .cast_mut()
            .add(exec_pool.current_alloc_utilization)
    };
    let target_slice =
        unsafe { std::slice::from_raw_parts_mut(target_addr, enc_result.code_buffer.len()) };
    target_slice.copy_from_slice(&enc_result.code_buffer);

    let mut instr_map = Vec::new();
    for arm_idx in 0..CHUNK_SIZE / 4 {
        let x86_idx = x86_idxs[arm_idx];
        let offset = if x86_idx == enc_result.new_instruction_offsets.len() {
            enc_result.code_buffer.len() as u32
        } else {
            enc_result.new_instruction_offsets[x86_idx]
        };
        instr_map.push(offset.try_into().expect("chunk offset overflow"));
    }

    CompiledChunk {
        instr_map,
        addr: target_addr,
        len: enc_result.code_buffer.len(),
        epilogue_offset,
    }
}

fn make_call(ass: &mut CodeAssembler, target: u64) -> Result<(), iced_x86::IcedError> {
    ass.mov(gpr64::rax, target)?;
    ass.call(gpr64::rax)?;
    Ok(())
}

enum RegTranslation {
    Direct(Register),
    // The register is stored at `idx` in the exec context
    Indirect(u32),
}

enum RegClass {
    GPR64,
    GPR32,
    FP128,
    FP64,
    FP32,
    FP16,
    FP8,
}

// Returns the RegClass and the top-level Reg
fn get_reg_class(reg: bad64::Reg) -> (RegClass, bad64::Reg) {
    use bad64::Reg;
    let rn = reg as u32;
    if reg == Reg::SP || rn >= Reg::X0 as u32 && rn <= Reg::X30 as u32 {
        (RegClass::GPR64, reg)
    } else if reg == Reg::WSP {
        (RegClass::GPR32, Reg::SP)
    } else if rn >= Reg::W0 as u32 && rn <= Reg::W30 as u32 {
        (
            RegClass::GPR32,
            Reg::from_u32(rn - Reg::W0 as u32 + Reg::X0 as u32).unwrap(),
        )
    } else {
        todo!("get_reg_class: {:?}", reg)
    }
}

fn lower_reg_to_class(reg: Register, class: RegClass) -> Register {
    let rn = reg as u32;
    match class {
        RegClass::GPR64 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                reg
            } else {
                panic!("Tried to lower {:?} to GPR64", reg);
            }
        }
        RegClass::GPR32 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                reg.full_register32()
            } else {
                panic!("Tried to lower {:?} to GPR64", reg);
            }
        }
        _ => todo!(),
    }
}

fn translate_indirect_reg(reg: bad64::Reg) -> RegTranslation {
    use RegTranslation::Indirect;
    use bad64::Reg::*;
    let rn = reg as u32;
    if rn >= X0 as u32 && rn <= X18 as u32 {
        Indirect(rn - X0 as u32)
    } else if rn >= X24 as u32 && rn <= X28 as u32 {
        Indirect(rn - X24 as u32 + 13)
    } else {
        unimplemented!("translating reg: {:?}", reg)
    }
}

fn translate_reg(reg: bad64::Reg) -> RegTranslation {
    use RegTranslation::Direct;
    use bad64::Reg::*;
    match reg {
        // These are sorted by frequency. See the comment at the top of the file.
        X0 => Direct(Register::RDI),
        SP => Direct(Register::RSP),
        X1 => Direct(Register::RSI),
        X2 => Direct(Register::RDX),
        X19 => Direct(Register::RBX),
        X3 => Direct(Register::RCX),
        X20 => Direct(Register::R12),
        X21 => Direct(Register::R13),
        X4 => Direct(Register::R8),
        X29 => Direct(Register::RBP),
        X22 => Direct(Register::R14),
        X5 => Direct(Register::R9),
        X23 => Direct(Register::R10),
        X30 => Direct(Register::R11),
        _ => translate_indirect_reg(reg),
    }
}

pub fn compile_instr(
    arm_instr: &Result<bad64::Instruction, bad64::DecodeError>,
    ass: &mut CodeAssembler,
) -> Result<(), iced_x86::IcedError> {
    let Ok(arm_instr) = arm_instr else {
        make_call(ass, callbacks::invalid_arm_instr as *const () as u64)?;
        return Ok(());
    };

    match arm_instr.op() {
        // bad64::Op::PACIASP
        _ => {
            make_call(
                ass,
                callbacks::unimplemented_arm_instr_landing_pad as *const () as u64,
            )?;
        }
    }

    Ok(())
}

fn get_arm_chunk(
    chunk_addr: usize,
) -> impl Iterator<Item = Result<bad64::Instruction, bad64::DecodeError>> {
    bad64::disasm(
        unsafe { std::slice::from_raw_parts(chunk_addr as *const u8, CHUNK_SIZE) },
        chunk_addr as u64,
    )
}

pub fn compile_chunk(exec_pool: &mut ExecPool, chunk_addr: usize) -> CompiledChunk {
    let arm_instrs = get_arm_chunk(chunk_addr);

    let mut ass = CodeAssembler::new(64).unwrap();

    let mut x86_idxs = Vec::new();
    for arm_instr in arm_instrs {
        x86_idxs.push(ass.instructions().len());
        compile_instr(&arm_instr, &mut ass).unwrap();
    }
    let epilogue_offset = ass
        .instructions()
        .len()
        .try_into()
        .expect("epilogue offset overflow");
    make_call(&mut ass, callbacks::end_of_chunk as *const () as u64).unwrap();

    let compiled_chunk = finalize_ass(exec_pool, ass, &x86_idxs, epilogue_offset);
    dump::dump_translation(chunk_addr, &compiled_chunk);
    compiled_chunk
}
