use iced_x86::{
    Code, Instruction, MemoryOperand, Register,
    code_asm::{CodeAssembler, gpr64},
};

use crate::runner::{
    CHUNK_SIZE, ExecCtx, ExecPool, callbacks,
    compiler::{
        instr_utils::{IcedResult, label_target, make_call},
        register::{RegClass, translate_reg, unwrap_reg},
    },
    get_exec,
};

pub fn rewrite_branch(arm_instr: &bad64::Instruction, call_ptr: *const u8) -> *const u8 {
    assert!(arm_instr.op() == bad64::Op::BL || arm_instr.op() == bad64::Op::B);
    let label_target = label_target(arm_instr.operands()[0]);
    let exec_ptr = get_exec(label_target as *const u8);

    let mut ass = CodeAssembler::new(64).unwrap();
    ass.mov(gpr64::rax, exec_ptr as u64).unwrap();
    ass.jmp(gpr64::rax).unwrap();

    let new_code = ass.assemble(call_ptr as u64).unwrap();
    assert_eq!(new_code.len(), 12);
    let code_slice = unsafe { std::slice::from_raw_parts_mut(call_ptr.cast_mut(), 12) };
    code_slice.copy_from_slice(&new_code);

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
) -> Result<(), iced_x86::IcedError> {
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
    let label = ass.fwd()?;
    if arm_instr.op() == bad64::Op::CBZ {
        ass.jne(label)?;
    } else {
        ass.jz(label)?;
    }

    make_jump(ass, operands[1], chunk_addr, exec_pool)?;

    ass.anonymous_label()?;
    ass.zero_bytes().unwrap();

    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    let operands = arm_instr.operands();
    match arm_instr.op() {
        Op::B => {
            make_jump(ass, operands[0], chunk_addr, exec_pool)?;
        }
        Op::BL => {
            ass.mov(gpr64::r11, arm_instr.address() + 4)?;
            make_jump(ass, operands[0], chunk_addr, exec_pool)?;
        }
        Op::CBZ | Op::CBNZ => handle_cbz_cbnz(arm_instr, ass, exec_pool, chunk_addr)?,
        Op::BR => {
            let (src, _) = translate_reg(unwrap_reg(operands[0]));
            src.pre_read(ass, RegClass::GPR64)?;
            let mut mov = Instruction::with2(
                Code::Mov_rm64_r64,
                MemoryOperand::with_base_displ(
                    Register::R15,
                    std::mem::offset_of!(ExecCtx, param) as i64,
                ),
                Register::None,
            )?;
            src.set_reg_operand(&mut mov, 1, RegClass::GPR64);
            ass.add_instruction(mov)?;
            make_call(
                ass,
                callbacks::indirect_jump_landing_pad as *const u8 as u64,
            )?;
        }
        Op::RET => {
            ass.mov(
                gpr64::r15 + std::mem::offset_of!(ExecCtx, param),
                gpr64::r11,
            )?;
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
