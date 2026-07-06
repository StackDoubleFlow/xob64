use iced_x86::{
    Code, Instruction, MemoryOperand, Register,
    code_asm::{CodeAssembler, CodeLabel, gpr64, ptr},
};

use crate::runner::{
    CHUNK_SIZE, ExecCtx, ExecPool, callbacks,
    compiler::{
        instr_utils::{IcedResult, label_target, make_call},
        register::{RegClass, translate_reg, unwrap_reg},
    },
    get_exec,
};

pub fn write_jump(at: *const u8, dest: u64) {
    let mut ass = CodeAssembler::new(64).unwrap();
    ass.mov(gpr64::rax, dest).unwrap();
    ass.jmp(gpr64::rax).unwrap();

    let new_code = ass.assemble(at as u64).unwrap();
    assert_eq!(new_code.len(), 12);
    let code_slice = unsafe { std::slice::from_raw_parts_mut(at.cast_mut(), 12) };
    code_slice.copy_from_slice(&new_code);
}

pub fn rewrite_branch(arm_instr: &bad64::Instruction, call_ptr: *const u8) -> *const u8 {
    assert!(arm_instr.op() == bad64::Op::BL || arm_instr.op() == bad64::Op::B);
    let label_target = label_target(arm_instr.operands()[0]);
    let exec_ptr = get_exec(label_target as *const u8);

    write_jump(call_ptr, exec_ptr as u64);

    exec_ptr
}

fn make_jump(
    ass: &mut CodeAssembler,
    label_operand: bad64::Operand,
    chunk_addr: usize,
    exec_pool: &mut ExecPool,
) -> IcedResult<()> {
    let target = label_target(label_operand);
    if target >= chunk_addr && target < chunk_addr + CHUNK_SIZE {
        let arm_instr_idx = (target - chunk_addr) / 4;
        ass.add_instruction(Instruction::with_branch(
            Code::Jmp_rel32_64,
            arm_instr_idx as u64 + 1,
        )?)?;
    } else {
        let chunk_offset = target % CHUNK_SIZE;
        let chunk_addr = target - chunk_offset;
        if let Some(chunk) = exec_pool.executable_map.get(&chunk_addr) {
            let instr_idx = chunk_offset / 4;
            let byte_idx = chunk.instr_map[instr_idx] as usize;
            let code_target = chunk.addr as usize + byte_idx;
            ass.mov(gpr64::rax, code_target as u64)?;
            ass.jmp(gpr64::rax)?;
        } else {
            make_call(
                ass,
                callbacks::rewrite_branch_landing_pad as *const () as u64,
            )?;
        }
    }
    Ok(())
}

fn handle_cbz_cbnz(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> IcedResult<()> {
    let operands = arm_instr.operands();

    let (reg_translation, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let cmp_code = match reg_class {
        RegClass::GPR64 => Code::Cmp_rm64_imm8,
        RegClass::GPR32 => Code::Cmp_rm32_imm8,
        _ => unreachable!(),
    };
    let mut cmp = Instruction::with2(cmp_code, Register::None, 0u32)?;
    reg_translation.set_operand(&mut cmp, 0);
    ass.add_instruction(cmp)?;

    let f = if arm_instr.op() == bad64::Op::CBZ {
        |ass: &mut CodeAssembler, label| ass.jnz(label)
    } else {
        |ass: &mut CodeAssembler, label| ass.jz(label)
    };
    reverse_conditional(ass, operands[1], exec_pool, chunk_addr, f)?;

    Ok(())
}

fn reverse_conditional(
    ass: &mut CodeAssembler,
    label_operand: bad64::Operand,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
    mut f: impl FnMut(&mut CodeAssembler, CodeLabel) -> IcedResult<()>,
) -> IcedResult<()> {
    let label = ass.fwd()?;
    f(ass, label)?;
    make_jump(ass, label_operand, chunk_addr, exec_pool)?;
    ass.anonymous_label()?;
    ass.zero_bytes().unwrap();

    Ok(())
}

fn push_shadow_stack(ass: &mut CodeAssembler, label: CodeLabel, reg: Register) -> IcedResult<()> {
    // TODO: avoid spilling register
    ass.push(gpr64::r10)?;
    let shadow_sp = ptr(gpr64::r15 + ExecCtx::SHADOW_SP_OFFSET);
    ass.mov(gpr64::r10, shadow_sp)?;
    ass.sub(gpr64::r10, 16)?;
    ass.add_instruction(Instruction::with2(
        Code::Mov_rm64_r64,
        MemoryOperand::with_base_displ(Register::R10, 8),
        reg,
    )?)?;
    ass.lea(gpr64::rax, ptr(label))?;
    ass.mov(ptr(gpr64::r10), gpr64::rax)?;
    ass.mov(shadow_sp, gpr64::r10)?;
    ass.pop(gpr64::r10)?;
    Ok(())
}

fn make_indirect_jump(ass: &mut CodeAssembler, reg: bad64::Operand) -> IcedResult<()> {
    let (src, _) = translate_reg(unwrap_reg(reg));
    src.pre_read(ass, RegClass::GPR64)?;
    let mut mov = Instruction::with2(
        Code::Mov_rm64_r64,
        MemoryOperand::with_base_displ(Register::R15, std::mem::offset_of!(ExecCtx, param) as i64),
        Register::None,
    )?;
    src.set_reg_operand(&mut mov, 1, RegClass::GPR64);
    ass.add_instruction(mov)?;
    make_call(
        ass,
        callbacks::indirect_jump_landing_pad as *const u8 as u64,
    )
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> IcedResult<bool> {
    use bad64::Op;
    let operands = arm_instr.operands();
    match arm_instr.op() {
        Op::B => {
            make_jump(ass, operands[0], chunk_addr, exec_pool)?;
        }
        Op::BL => {
            // Load link register
            ass.mov(gpr64::r11, arm_instr.address() + 4)?;
            // Push shadow stack
            let ret_label = ass.fwd()?;
            push_shadow_stack(ass, ret_label, Register::R11)?;
            make_jump(ass, operands[0], chunk_addr, exec_pool)?;
            ass.anonymous_label()?;
            ass.zero_bytes()?;
        }
        Op::BR => make_indirect_jump(ass, operands[0])?,
        Op::BLR => {
            // Load link register
            ass.mov(gpr64::r11, arm_instr.address() + 4)?;
            // Push shadow stack
            let ret_label = ass.fwd()?;
            push_shadow_stack(ass, ret_label, Register::R11)?;
            make_indirect_jump(ass, operands[0])?;
            ass.anonymous_label()?;
            ass.zero_bytes()?;
        }
        Op::CBZ | Op::CBNZ => handle_cbz_cbnz(arm_instr, ass, exec_pool, chunk_addr)?,
        Op::RET => {
            let shadow_sp = ptr(gpr64::r15 + ExecCtx::SHADOW_SP_OFFSET);
            ass.push(gpr64::r10)?;
            // Load shadow sp
            ass.mov(gpr64::r10, shadow_sp)?;
            // Check if emulated pointer matches
            ass.mov(gpr64::rax, ptr(gpr64::r10 + 8))?;
            ass.cmp(gpr64::r11, gpr64::rax)?;
            // Load native pointer
            ass.mov(gpr64::rax, ptr(gpr64::r10))?;
            // Pop pointers from stack
            ass.lea(gpr64::r10, gpr64::r10 + 16)?;
            ass.mov(shadow_sp, gpr64::r10)?;
            ass.pop(gpr64::r10)?;
            // If the emulated pointer matches, we jump to the native pointer
            let callback_label = ass.fwd()?;
            ass.jne(callback_label)?;
            ass.jmp(gpr64::rax)?;
            // Otherwise, we do an indirect call
            ass.anonymous_label()?;
            ass.mov(gpr64::r15 + ExecCtx::PARAM_OFFSET, gpr64::r11)?;
            make_call(
                ass,
                callbacks::indirect_jump_landing_pad as *const u8 as u64,
            )?;
        }
        Op::B_EQ => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jne(label)
        })?,
        Op::B_NE => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.je(label)
        })?,
        Op::B_LT => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jge(label)
        })?,
        Op::B_GE => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jl(label)
        })?,
        Op::B_GT => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jle(label)
        })?,
        Op::B_LE => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jg(label)
        })?,
        Op::B_CC => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jae(label)
        })?,
        Op::B_CS => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jb(label)
        })?,
        Op::B_HI => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jbe(label)
        })?,
        Op::B_LS => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.ja(label)
        })?,
        Op::B_VS => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jno(label)
        })?,
        Op::B_VC => reverse_conditional(ass, operands[0], exec_pool, chunk_addr, |ass, label| {
            ass.jo(label)
        })?,
        Op::B_AL | Op::B_NV => make_jump(ass, operands[0], chunk_addr, exec_pool)?,
        // Branch Target Identification
        Op::BTI => {}
        _ => return Ok(false),
    }

    Ok(true)
}
