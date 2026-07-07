use iced_x86::{Code, Instruction, MemoryOperand, Register, code_asm::CodeAssembler};

use crate::runner::compiler::{
    instr_utils::{IcedResult, get_alt_reg, load_indirect},
    register::{NativeRegClass, RegClass, RegTranslation, translate_reg, unwrap_reg},
};

// Used for offset bits, which can only be up to 12 bits, so we don't have to worry about overflow.
fn any_offset_sign(imm: bad64::Imm) -> i32 {
    match imm {
        bad64::Imm::Signed(imm) => imm as i32,
        bad64::Imm::Unsigned(imm) => imm as i32,
    }
}

#[derive(Debug)]
enum LSWidth {
    RegClass,
    SignedWord,
    Half(bool),
    Byte(bool),
}

struct AddrModeInfo {
    base_reg: Register,
    base_reg_translation: RegTranslation,
    offset: i32,
    post_index_offset: Option<i32>,
    write_back_base_reg: bool,
    pop_base_reg: bool,
}

impl AddrModeInfo {
    fn memory_operand(&self, extra_offset: i64) -> MemoryOperand {
        MemoryOperand::with_base_displ(self.base_reg, self.offset as i64 + extra_offset)
    }
}

// Returns base reg, offset, and a post-indexing offset if it exists
fn process_addr_mode(
    ass: &mut CodeAssembler,
    mem_operand: bad64::Operand,
    alternate_indirect: bool,
    alt_blockers: &[RegTranslation],
) -> IcedResult<AddrModeInfo> {
    let mut reg_offset = None;
    let (base_reg, imm) = match mem_operand {
        bad64::Operand::MemOffset { reg, offset, .. } => (reg, offset),
        bad64::Operand::MemPreIdx { reg, imm } => (reg, imm),
        bad64::Operand::MemPostIdxImm { reg, imm } => (reg, imm),
        bad64::Operand::MemExt { regs, shift, .. } => {
            assert!(shift.is_none());
            reg_offset = Some(regs[1]);
            (regs[0], bad64::Imm::Unsigned(0))
        }
        _ => todo!("memory address operand: {:?}", mem_operand),
    };
    let imm = any_offset_sign(imm);
    let (reg_translation, _) = translate_reg(base_reg);

    let mut pop_base_reg = false;
    let new_base_reg = match reg_translation {
        RegTranslation::Direct(reg) => {
            if reg_offset.is_some() {
                if alternate_indirect {
                    let alt_reg = get_alt_reg(alt_blockers);
                    ass.add_instruction(Instruction::with1(Code::Push_r64, alt_reg)?)?;
                    pop_base_reg = true;
                    ass.add_instruction(Instruction::with2(Code::Mov_r64_rm64, alt_reg, reg)?)?;
                    alt_reg
                } else {
                    ass.add_instruction(Instruction::with2(
                        Code::Mov_r64_rm64,
                        Register::RAX,
                        reg,
                    )?)?;
                    Register::RAX
                }
            } else {
                reg
            }
        }
        RegTranslation::Indirect(indirect_offset) => {
            if alternate_indirect {
                // We have a situation where both the offset and store value are indirect.
                // We need to load the offset into a register that isn't the scratch, and we need to pick a register that isn't going to be the load/store value of either reg1 or reg2.
                let alt_reg = get_alt_reg(alt_blockers);
                ass.add_instruction(Instruction::with1(Code::Push_r64, alt_reg)?)?;
                pop_base_reg = true;
                ass.add_instruction(Instruction::with2(
                    Code::Mov_r64_rm64,
                    alt_reg,
                    MemoryOperand::with_base_displ(Register::R15, indirect_offset as i64),
                )?)?;
                alt_reg
            } else {
                load_indirect(ass, RegClass::GPR64, indirect_offset)?;
                Register::RAX
            }
        }
        RegTranslation::Zero(_) => unreachable!(),
    };

    if let Some(reg_offset) = reg_offset {
        let (reg_offset, _) = translate_reg(reg_offset);
        let mut add = Instruction::with2(Code::Add_r64_rm64, new_base_reg, Register::None)?;
        reg_offset.set_operand(&mut add, 1);
        ass.add_instruction(add)?;
    }

    let mut addr_mode_info = AddrModeInfo {
        base_reg: new_base_reg,
        base_reg_translation: reg_translation,
        write_back_base_reg: false,
        offset: 0,
        post_index_offset: None,
        pop_base_reg,
    };

    match mem_operand {
        bad64::Operand::MemOffset { .. } => {
            addr_mode_info.offset = imm;
        }
        bad64::Operand::MemPreIdx { .. } => {
            ass.add_instruction(Instruction::with2(Code::Add_rm64_imm32, new_base_reg, imm)?)?;
            addr_mode_info.write_back_base_reg = true;
        }
        bad64::Operand::MemPostIdxImm { .. } => {
            addr_mode_info.post_index_offset = Some(imm);
            addr_mode_info.write_back_base_reg = true;
        }
        bad64::Operand::MemExt { .. } => {}
        _ => unreachable!(),
    }

    Ok(addr_mode_info)
}

fn finalize_addr_mode(ass: &mut CodeAssembler, addr_mode_info: AddrModeInfo) -> IcedResult<()> {
    if let Some(offset) = addr_mode_info.post_index_offset {
        ass.add_instruction(Instruction::with2(
            Code::Add_rm64_imm32,
            addr_mode_info.base_reg,
            offset,
        )?)?;
    }

    if let RegTranslation::Indirect(indirect_offset) = addr_mode_info.base_reg_translation
        && addr_mode_info.write_back_base_reg
    {
        ass.add_instruction(Instruction::with2(
            Code::Mov_rm64_r64,
            addr_mode_info.base_reg,
            MemoryOperand::with_base_displ(Register::R15, indirect_offset as i64),
        )?)?;
    }

    if addr_mode_info.pop_base_reg {
        ass.add_instruction(Instruction::with1(Code::Pop_r64, addr_mode_info.base_reg)?)?;
    }
    Ok(())
}

type GenFn = fn(
    ass: &mut CodeAssembler,
    reg_translation: RegTranslation,
    reg_class: RegClass,
    addr_mode_info: &AddrModeInfo,
    extra_offset: i64,
    ls_width: LSWidth,
) -> IcedResult<()>;

fn make_store(
    ass: &mut CodeAssembler,
    src: RegTranslation,
    reg_class: RegClass,
    addr_mode_info: &AddrModeInfo,
    extra_offset: i64,
    ls_width: LSWidth,
) -> IcedResult<()> {
    src.pre_read(ass, reg_class)?;
    let code = match ls_width {
        LSWidth::RegClass => match reg_class {
            RegClass::GPR64 => Code::Mov_rm64_r64,
            RegClass::GPR32 => Code::Mov_rm32_r32,
            RegClass::FP128 => Code::Movaps_xmmm128_xmm,
            RegClass::FP64 => Code::Movsd_xmmm64_xmm,
            RegClass::FP32 => Code::Movss_xmmm32_xmm,
            _ => todo!(),
        },
        LSWidth::SignedWord => unreachable!(),
        LSWidth::Half(_) => Code::Mov_rm16_r16,
        LSWidth::Byte(_) => Code::Mov_rm8_r8,
    };
    let src = match ls_width {
        LSWidth::Half(_) => src.with_native_class(NativeRegClass::GPR16),
        LSWidth::Byte(_) => src.with_native_class(NativeRegClass::GPR8),
        _ => src,
    };
    let mem = addr_mode_info.memory_operand(extra_offset);
    let mut instr = Instruction::with2(code, mem, Register::None)?;
    src.set_reg_operand(&mut instr, 1, reg_class);
    ass.add_instruction(instr)?;
    Ok(())
}

fn make_load(
    ass: &mut CodeAssembler,
    dest_translation: RegTranslation,
    reg_class: RegClass,
    addr_mode_info: &AddrModeInfo,
    extra_offset: i64,
    ls_width: LSWidth,
) -> IcedResult<()> {
    let code = match ls_width {
        LSWidth::RegClass => match reg_class {
            RegClass::GPR64 => Code::Mov_r64_rm64,
            RegClass::GPR32 => Code::Mov_r32_rm32,
            RegClass::FP128 => Code::Movaps_xmm_xmmm128,
            RegClass::FP64 => Code::Movsd_xmm_xmmm64,
            RegClass::FP32 => Code::Movss_xmm_xmmm32,
            _ => todo!(),
        },
        LSWidth::SignedWord => Code::Movsxd_r64_rm32,
        LSWidth::Half(false) => Code::Movzx_r32_rm16,
        LSWidth::Half(true) => Code::Movsx_r32_rm16,
        LSWidth::Byte(false) => Code::Movzx_r32_rm8,
        LSWidth::Byte(true) => Code::Movsx_r32_rm8,
    };
    let mem = addr_mode_info.memory_operand(extra_offset);
    let mut instr = Instruction::with2(code, Register::None, mem)?;
    dest_translation.set_reg_operand(&mut instr, 0, reg_class);
    ass.add_instruction(instr)?;
    dest_translation.post_write(ass, reg_class)?;
    Ok(())
}

fn load_store_pair(
    ass: &mut CodeAssembler,
    arm_instr: &bad64::Instruction,
    gen_fn: GenFn,
) -> IcedResult<()> {
    let operands = arm_instr.operands();

    let reg1 = unwrap_reg(operands[0]);
    let (reg1_translation, reg_class) = translate_reg(reg1);
    let reg2 = unwrap_reg(operands[1]);
    let (reg2_translation, _) = translate_reg(reg2);

    let addr_mode_info = process_addr_mode(
        ass,
        operands[2],
        reg1_translation.is_indirect() || reg2_translation.is_indirect(),
        &[reg1_translation, reg2_translation],
    )?;

    gen_fn(
        ass,
        reg1_translation,
        reg_class,
        &addr_mode_info,
        0,
        LSWidth::RegClass,
    )?;
    let incr = if reg_class == RegClass::GPR32 { 4 } else { 8 };
    gen_fn(
        ass,
        reg2_translation,
        reg_class,
        &addr_mode_info,
        incr,
        LSWidth::RegClass,
    )?;

    finalize_addr_mode(ass, addr_mode_info)?;
    Ok(())
}

fn load_store(
    ass: &mut CodeAssembler,
    arm_instr: &bad64::Instruction,
    gen_fn: GenFn,
    ls_width: LSWidth,
) -> IcedResult<()> {
    let operands = arm_instr.operands();

    let reg = unwrap_reg(operands[0]);
    let (reg_translation, reg_class) = translate_reg(reg);

    let addr_mode_info = process_addr_mode(
        ass,
        operands[1],
        reg_translation.is_indirect(),
        &[reg_translation],
    )?;

    gen_fn(
        ass,
        reg_translation,
        reg_class,
        &addr_mode_info,
        0,
        ls_width,
    )?;

    finalize_addr_mode(ass, addr_mode_info)?;
    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<bool> {
    use bad64::Op;
    match arm_instr.op() {
        Op::STP => load_store_pair(ass, arm_instr, make_store)?,
        Op::LDP => load_store_pair(ass, arm_instr, make_load)?,
        Op::STR | Op::STUR => load_store(ass, arm_instr, make_store, LSWidth::RegClass)?,
        Op::LDR | Op::LDUR => load_store(ass, arm_instr, make_load, LSWidth::RegClass)?,
        Op::LDRB | Op::LDURB => load_store(ass, arm_instr, make_load, LSWidth::Byte(false))?,
        Op::LDRSB | Op::LDURSB => load_store(ass, arm_instr, make_load, LSWidth::Byte(true))?,
        Op::LDRH | Op::LDURH => load_store(ass, arm_instr, make_load, LSWidth::Half(false))?,
        Op::LDRSH | Op::LDURSH => load_store(ass, arm_instr, make_load, LSWidth::Half(true))?,
        Op::LDRSW | Op::LDURSW => load_store(ass, arm_instr, make_load, LSWidth::SignedWord)?,
        Op::STRB | Op::STURB => load_store(ass, arm_instr, make_store, LSWidth::Byte(false))?,
        Op::STRH | Op::STURH => load_store(ass, arm_instr, make_store, LSWidth::Half(false))?,
        _ => return Ok(false),
    }

    Ok(true)
}
