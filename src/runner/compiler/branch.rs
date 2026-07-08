use iced_x86::{
    Code, Instruction, MemoryOperand, Register,
    code_asm::{CodeAssembler, CodeLabel, gpr64, ptr},
};

use crate::runner::{
    CHUNK_SIZE, ExecCtx, ExecPool, callbacks,
    compiler::{
        CompileResult,
        instr_utils::{label_target, make_call},
        register::{RegClass, translate_reg, unwrap_imm, unwrap_reg, unwrap_unsigned},
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
    let operands = arm_instr.operands();
    use bad64::Op;
    let label_operand = match arm_instr.op() {
        Op::B_AL
        | Op::B_CC
        | Op::B_CS
        | Op::B_EQ
        | Op::B_GE
        | Op::B_GT
        | Op::B_HI
        | Op::B_LE
        | Op::B_LS
        | Op::B_LT
        | Op::B_MI
        | Op::B_NE
        | Op::B_NV
        | Op::B_PL
        | Op::B_VC
        | Op::B_VS
        | Op::B
        | Op::BL => operands[0],
        Op::CBZ | Op::CBNZ => operands[1],
        Op::TBZ | Op::TBNZ => operands[2],
        _ => unimplemented!("rewrite branch: {}", arm_instr),
    };
    let label_target = label_target(label_operand);
    let exec_ptr = get_exec(label_target as *const u8);

    write_jump(call_ptr, exec_ptr as u64);

    exec_ptr
}

fn make_jump(
    ass: &mut CodeAssembler,
    label_operand: bad64::Operand,
    chunk_addr: usize,
    exec_pool: &mut ExecPool,
) -> CompileResult<()> {
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
) -> CompileResult<()> {
    let operands = arm_instr.operands();

    let (reg_translation, reg_class) = translate_reg(unwrap_reg(operands[0]))?;
    let cmp_code = match reg_class {
        RegClass::GPR64 => Code::Cmp_rm64_imm8,
        RegClass::GPR32 => Code::Cmp_rm32_imm8,
        _ => unreachable!(),
    };
    let mut cmp = Instruction::with2(cmp_code, Register::None, 0u32)?;
    reg_translation.set_operand(&mut cmp, 0);
    ass.add_instruction(cmp)?;

    let cond = if arm_instr.op() == bad64::Op::CBZ {
        bad64::Condition::EQ
    } else {
        bad64::Condition::NE
    };
    make_branch_to_label(ass, operands[1], exec_pool, chunk_addr, cond)?;

    Ok(())
}

fn handle_tbz_tbnz(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> CompileResult<()> {
    let operands = arm_instr.operands();

    let (reg_translation, reg_class) = translate_reg(unwrap_reg(operands[0]))?;
    let bt_code = match reg_class {
        RegClass::GPR64 => Code::Bt_rm64_imm8,
        RegClass::GPR32 => Code::Bt_rm32_imm8,
        _ => unreachable!(),
    };
    let imm = unwrap_unsigned(unwrap_imm(operands[1]).0);
    let mut bt = Instruction::with2(bt_code, Register::None, imm as u32)?;
    reg_translation.set_operand(&mut bt, 0);
    ass.add_instruction(bt)?;

    // bt will set the carry flag equal to the bit
    let cond = if arm_instr.op() == bad64::Op::TBZ {
        bad64::Condition::CC
    } else {
        bad64::Condition::CS
    };
    make_branch_to_label(ass, operands[2], exec_pool, chunk_addr, cond)?;

    Ok(())
}

fn inverse_condition(cond: bad64::Condition) -> bad64::Condition {
    use bad64::Condition::*;
    match cond {
        MI => PL,
        PL => MI,
        EQ => NE,
        NE => EQ,
        VS => VC,
        VC => VS,
        CS => CC,
        CC => CS,
        HI => LS,
        LS => HI,
        GE => LT,
        LT => GE,
        GT => LE,
        LE => GT,
        AL => NV,
        NV => AL,
    }
}

pub fn make_jcc(
    ass: &mut CodeAssembler,
    cond: bad64::Condition,
    label: CodeLabel,
) -> CompileResult<()> {
    use bad64::Condition::*;
    match cond {
        EQ => ass.je(label)?,
        NE => ass.jne(label)?,
        LT => ass.jl(label)?,
        GE => ass.jge(label)?,
        GT => ass.jg(label)?,
        LE => ass.jle(label)?,
        CC => ass.jnc(label)?,
        CS => ass.jc(label)?,
        HI => ass.ja(label)?,
        LS => ass.jbe(label)?,
        VC => ass.jno(label)?,
        VS => ass.jo(label)?,
        MI => ass.js(label)?,
        PL => ass.jns(label)?,
        AL | NV => ass.jmp(label)?,
    }
    Ok(())
}

fn make_branch_to_label(
    ass: &mut CodeAssembler,
    label_operand: bad64::Operand,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
    cond: bad64::Condition,
) -> CompileResult<()> {
    let label = ass.fwd()?;
    // Branch over the far jump with the inverse condition
    make_jcc(ass, inverse_condition(cond), label)?;
    make_jump(ass, label_operand, chunk_addr, exec_pool)?;
    ass.anonymous_label()?;
    ass.zero_bytes().unwrap();
    Ok(())
}

fn push_shadow_stack(
    ass: &mut CodeAssembler,
    label: CodeLabel,
    reg: Register,
) -> CompileResult<()> {
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

fn make_indirect_jump(ass: &mut CodeAssembler, reg: bad64::Operand) -> CompileResult<()> {
    let (src, _) = translate_reg(unwrap_reg(reg))?;
    src.pre_read(ass, RegClass::GPR64)?;
    let mut mov = Instruction::with2(
        Code::Mov_rm64_r64,
        MemoryOperand::with_base_displ(Register::R15, std::mem::offset_of!(ExecCtx, param) as i64),
        Register::None,
    )?;
    src.set_reg_operand(&mut mov, 1);
    ass.add_instruction(mov)?;
    make_call(
        ass,
        callbacks::indirect_jump_landing_pad as *const u8 as u64,
    )
}

pub fn handle_b_cc(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> CompileResult<bool> {
    use bad64::Condition::*;
    use bad64::Op;

    let cond = match arm_instr.op() {
        Op::B_AL => AL,
        Op::B_CC => CC,
        Op::B_CS => CS,
        Op::B_EQ => EQ,
        Op::B_GE => GE,
        Op::B_GT => GT,
        Op::B_HI => HI,
        Op::B_LE => LE,
        Op::B_LS => LS,
        Op::B_LT => LT,
        Op::B_MI => MI,
        Op::B_NE => NE,
        Op::B_NV => NV,
        Op::B_PL => PL,
        Op::B_VC => VC,
        Op::B_VS => VS,
        _ => return Ok(false),
    };
    make_branch_to_label(ass, arm_instr.operands()[0], exec_pool, chunk_addr, cond)?;

    Ok(true)
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> CompileResult<bool> {
    use bad64::Op;
    let operands = arm_instr.operands();

    if handle_b_cc(arm_instr, ass, exec_pool, chunk_addr)? {
        return Ok(true);
    }

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
        Op::TBZ | Op::TBNZ => handle_tbz_tbnz(arm_instr, ass, exec_pool, chunk_addr)?,
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
        // Branch Target Identification
        Op::BTI => {}
        _ => return Ok(false),
    }

    Ok(true)
}
