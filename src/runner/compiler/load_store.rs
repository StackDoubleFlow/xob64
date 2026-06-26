use iced_x86::{Code, Instruction, MemoryOperand, Register, code_asm::CodeAssembler};

use crate::runner::compiler::{
    instr_utils::{codes::ADD_RI_CODES, load_indirect, make_ri},
    register::{RegClass, RegTranslation, translate_reg, unwrap_reg},
};

// Used for offset bits, which can only be up to 12 bits, so we don't have to worry about overflow.
fn any_offset_sign(imm: bad64::Imm) -> i32 {
    match imm {
        bad64::Imm::Signed(imm) => imm as i32,
        bad64::Imm::Unsigned(imm) => imm as i32,
    }
}

// Returns base reg, offset, and a post-indexing offset if it exists
fn process_addr_mode(
    ass: &mut CodeAssembler,
    operand: bad64::Operand,
) -> Result<(bad64::Reg, i32, Option<i32>), iced_x86::IcedError> {
    Ok(match operand {
        bad64::Operand::MemOffset { reg, offset, .. } => (reg, any_offset_sign(offset), None),
        bad64::Operand::MemPreIdx { reg, imm } => {
            let imm = any_offset_sign(imm);
            let (reg_translation, reg_class) = translate_reg(reg);
            make_ri(ass, &ADD_RI_CODES, reg_class, reg_translation, imm)?;
            (reg, 0, None)
        }
        _ => todo!("memory address operand: {:?}", operand),
    })
}

fn finish_post_index(
    ass: &mut CodeAssembler,
    base_reg_translation: RegTranslation,
    post_index_offset: Option<i32>,
) -> Result<(), iced_x86::IcedError> {
    if let Some(offset) = post_index_offset {
        make_ri(
            ass,
            &ADD_RI_CODES,
            RegClass::GPR64,
            base_reg_translation,
            offset,
        )?;
    }
    Ok(())
}

fn make_store(
    ass: &mut CodeAssembler,
    src_translation: RegTranslation,
    reg_class: RegClass,
    base_reg: Register,
    offset: i32,
) -> Result<(), iced_x86::IcedError> {
    src_translation.pre_read(ass, reg_class)?;
    let code = match reg_class {
        RegClass::GPR64 => Code::Mov_rm64_r64,
        RegClass::GPR32 => Code::Mov_rm32_r32,
        _ => todo!(),
    };
    let mem = MemoryOperand::with_base_displ(base_reg, offset as i64);
    let mut instr = Instruction::with2(code, mem, Register::None)?;
    src_translation.set_reg_operand(&mut instr, 1, reg_class);
    ass.add_instruction(instr)?;
    Ok(())
}

fn make_load(
    ass: &mut CodeAssembler,
    dest_translation: RegTranslation,
    reg_class: RegClass,
    base_reg: Register,
    offset: i32,
) -> Result<(), iced_x86::IcedError> {
    let code = match reg_class {
        RegClass::GPR64 => Code::Mov_r64_rm64,
        RegClass::GPR32 => Code::Mov_r32_rm32,
        _ => todo!(),
    };
    let mem = MemoryOperand::with_base_displ(base_reg, offset as i64);
    let mut instr = Instruction::with2(code, Register::None, mem)?;
    dest_translation.set_reg_operand(&mut instr, 0, reg_class);
    ass.add_instruction(instr)?;
    dest_translation.post_write(ass, reg_class)?;
    Ok(())
}

fn load_store_pair(
    ass: &mut CodeAssembler,
    arm_instr: &bad64::Instruction,
    gen_fn: fn(
        ass: &mut CodeAssembler,
        reg_translation: RegTranslation,
        reg_class: RegClass,
        base_reg: Register,
        offset: i32,
    ) -> Result<(), iced_x86::IcedError>,
) -> Result<(), iced_x86::IcedError> {
    let operands = arm_instr.operands();
    let (arm_base_reg, offset, post_index_offset) = process_addr_mode(ass, operands[2])?;

    let (base_reg_translation, _) = translate_reg(arm_base_reg);
    let reg1 = unwrap_reg(operands[0]);
    let (reg1_translation, reg_class) = translate_reg(reg1);
    let reg2 = unwrap_reg(operands[1]);
    let (reg2_translation, _) = translate_reg(reg2);

    let double_indirect = base_reg_translation.is_indirect()
        && (reg1_translation.is_indirect() || reg2_translation.is_indirect());

    let base_reg = match base_reg_translation {
        RegTranslation::Direct(base_reg) => base_reg,
        RegTranslation::Indirect(indirect_offset) => {
            if double_indirect {
                // We have a situation where both the offset and store value are indirect.
                // We need to load the offset into a register that isn't the scratch, and we need to pick a register that isn't going to be the load/store value of either reg1 or reg2.
                // First, we'll try rdi, then rsi.
                let scratch = if reg1_translation == RegTranslation::Direct(Register::RDI)
                    || reg2_translation == RegTranslation::Direct(Register::RDI)
                {
                    Register::RSI
                } else {
                    Register::RDI
                };
                ass.add_instruction(Instruction::with1(Code::Push_r64, scratch)?)?;
                ass.add_instruction(Instruction::with2(
                    Code::Mov_r64_rm64,
                    scratch,
                    MemoryOperand::with_base_displ(Register::R15, indirect_offset as i64),
                )?)?;
                scratch
            } else {
                load_indirect(ass, RegClass::GPR64, indirect_offset)?;
                Register::RAX
            }
        }
    };

    gen_fn(ass, reg1_translation, reg_class, base_reg, offset)?;
    let incr = if reg_class == RegClass::GPR32 { 4 } else { 8 };
    gen_fn(ass, reg2_translation, reg_class, base_reg, offset + incr)?;

    if double_indirect {
        ass.add_instruction(Instruction::with1(Code::Pop_r64, base_reg)?)?;
    }
    finish_post_index(ass, base_reg_translation, post_index_offset)?;
    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    match arm_instr.op() {
        Op::STP => load_store_pair(ass, arm_instr, make_store)?,
        Op::LDP => load_store_pair(ass, arm_instr, make_load)?,
        _ => return Ok(false),
    }

    Ok(true)
}
