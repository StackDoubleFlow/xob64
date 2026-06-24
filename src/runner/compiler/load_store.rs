use iced_x86::{
    Register,
    code_asm::{AsmRegister64, CodeAssembler, gpr32, gpr64},
};

use crate::runner::compiler::register::{
    RegClass, RegTranslation, get_reg_class, load_indirect, reg_operand_gpr, reg_operand_no_mem,
    translate_reg, unwrap_reg,
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
) -> Result<(bad64::Reg, i32, Option<i64>), iced_x86::IcedError> {
    Ok(match operand {
        bad64::Operand::MemOffset { reg, offset, .. } => (reg, any_offset_sign(offset), None),
        bad64::Operand::MemPreIdx { reg, imm } => {
            let imm = any_offset_sign(imm);
            reg_operand_gpr(reg).add_dest_imm(ass, imm)?;
            (reg, 0, None)
        }
        _ => todo!("memory address operand: {:?}", operand),
    })
}

fn finish_post_index(
    ass: &mut CodeAssembler,
    base_reg: bad64::Reg,
    post_index_offset: Option<i64>,
) -> Result<(), iced_x86::IcedError> {
    if let Some(offset) = post_index_offset {
        reg_operand_gpr(base_reg).add_dest_imm(ass, offset as i32)?;
    }
    Ok(())
}

fn make_store(
    ass: &mut CodeAssembler,
    reg: bad64::Reg,
    base_reg: AsmRegister64,
    offset: i32,
) -> Result<(), iced_x86::IcedError> {
    let translated_reg = reg_operand_no_mem(ass, reg)?;
    if let Some(reg) = gpr64::get_gpr64(translated_reg) {
        ass.mov(base_reg + offset, reg)?;
    } else {
        let reg = gpr32::get_gpr32(translated_reg).unwrap();
        ass.mov(base_reg + offset, reg)?;
    }
    Ok(())
}

fn make_load(
    ass: &mut CodeAssembler,
    reg: bad64::Reg,
    base_reg: AsmRegister64,
    offset: i32,
) -> Result<(), iced_x86::IcedError> {
    let translated_reg = reg_operand_no_mem(ass, reg)?;
    if let Some(reg) = gpr64::get_gpr64(translated_reg) {
        ass.mov(reg, base_reg + offset)?;
    } else {
        let reg = gpr32::get_gpr32(translated_reg).unwrap();
        ass.mov(reg, base_reg + offset)?;
    }
    Ok(())
}

fn load_store_pair(
    ass: &mut CodeAssembler,
    arm_instr: &bad64::Instruction,
    gen_fn: fn(
        ass: &mut CodeAssembler,
        reg: bad64::Reg,
        base_reg: AsmRegister64,
        offset: i32,
    ) -> Result<(), iced_x86::IcedError>,
) -> Result<(), iced_x86::IcedError> {
    let (arm_base_reg, offset, post_index_offset) =
        process_addr_mode(ass, arm_instr.operands()[2])?;

    let base_reg_translation = translate_reg(arm_base_reg);
    let reg1 = unwrap_reg(arm_instr.operands()[0]);
    let reg1_translation = translate_reg(reg1);
    let reg2 = unwrap_reg(arm_instr.operands()[1]);
    let reg2_translation = translate_reg(reg2);

    let double_indirect = base_reg_translation.is_indirect()
        && (reg1_translation.is_indirect() || reg2_translation.is_indirect());

    let base_reg = match base_reg_translation {
        RegTranslation::Direct(base_reg) => gpr64::get_gpr64(base_reg).unwrap(),
        RegTranslation::Indirect(indirect_idx) => {
            if double_indirect {
                // We have a situation where both the offset and store value are indirect.
                // We need to load the offset into a register that isn't the scratch, and we need to pick a register that isn't going to be the load/store value of either reg1 or reg2.
                // First, we'll try rdi, then rsi.
                let scratch = if reg1_translation == RegTranslation::Direct(Register::RDI)
                    || reg2_translation == RegTranslation::Direct(Register::RDI)
                {
                    gpr64::rsi
                } else {
                    gpr64::rdi
                };
                ass.push(scratch)?;
                load_indirect(ass, scratch, indirect_idx)?;
                scratch
            } else {
                load_indirect(ass, gpr64::rax, indirect_idx)?;
                gpr64::rax
            }
        }
    };

    gen_fn(ass, reg1, base_reg, offset)?;
    let incr = if get_reg_class(reg1).0 == RegClass::GPR32 {
        4
    } else {
        8
    };
    gen_fn(ass, reg2, base_reg, offset + incr)?;

    if double_indirect {
        ass.pop(base_reg)?;
    }
    finish_post_index(ass, arm_base_reg, post_index_offset)?;
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
