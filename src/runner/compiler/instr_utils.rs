use iced_x86::{
    Code, Instruction, Register,
    code_asm::{CodeAssembler, gpr32, gpr64},
};

use crate::runner::compiler::register::{RegClass, RegTranslation};

pub type IcedResult<T> = Result<T, iced_x86::IcedError>;

pub mod codes {
    use super::{OpRICodes, OpRRCodes};
    use iced_x86::Code::*;

    pub const MOV_RR_CODES: OpRRCodes =
        OpRRCodes::new(Mov_r32_rm32, Mov_rm32_r32, Mov_r64_rm64, Mov_rm64_r64);

    pub const ADD_RR_CODES: OpRRCodes =
        OpRRCodes::new(Add_r32_rm32, Add_rm32_r32, Add_r64_rm64, Add_rm64_r64);
    pub const ADD_RI_CODES: OpRICodes = OpRICodes::new(Add_rm32_imm32, Add_rm64_imm32);
}

pub struct OpRRCodes {
    r32_rm32: Code,
    rm32_r32: Code,
    r64_rm64: Code,
    rm64_r64: Code,
}

impl OpRRCodes {
    const fn new(r32_rm32: Code, rm32_r32: Code, r64_rm64: Code, rm64_r64: Code) -> Self {
        Self {
            r32_rm32,
            rm32_r32,
            r64_rm64,
            rm64_r64,
        }
    }
}

pub struct OpRICodes {
    rm32_imm32: Code,
    rm64_imm32: Code,
}

impl OpRICodes {
    const fn new(rm32_imm32: Code, rm64_imm32: Code) -> Self {
        Self {
            rm32_imm32,
            rm64_imm32,
        }
    }
}

pub fn load_indirect(
    ass: &mut CodeAssembler,
    reg_class: RegClass,
    indirect_offset: u32,
) -> IcedResult<()> {
    let mem = gpr64::r15 + indirect_offset;
    match reg_class {
        RegClass::GPR32 => ass.mov(gpr32::eax, mem),
        RegClass::GPR64 => ass.mov(gpr64::rax, mem),
        _ => todo!(),
    }
}

pub fn store_indirect(
    ass: &mut CodeAssembler,
    reg_class: RegClass,
    indirect_offset: u32,
) -> IcedResult<()> {
    let mem = gpr64::r15 + indirect_offset;
    match reg_class {
        RegClass::GPR32 => ass.mov(mem, gpr32::eax),
        RegClass::GPR64 => ass.mov(mem, gpr64::rax),
        _ => todo!(),
    }
}

/// Note that this function does not yet handle the case where dest is both an input and an output
fn make_rr_impl(
    ass: &mut CodeAssembler,
    codes: &OpRRCodes,
    reg_class: RegClass,
    dest: RegTranslation,
    src: RegTranslation,
    reads_dest: bool,
) -> IcedResult<()> {
    let (r_rm, rm_r) = match reg_class {
        RegClass::GPR32 => (codes.r32_rm32, codes.rm32_r32),
        RegClass::GPR64 => (codes.r64_rm64, codes.rm64_r64),
        _ => unimplemented!(),
    };
    match (dest, src) {
        (RegTranslation::Indirect(dest_indirect_idx), RegTranslation::Indirect(_)) => {
            if reads_dest {
                load_indirect(ass, reg_class, dest_indirect_idx);
            }
            let mut instr = Instruction::with1(r_rm, reg_class.scratch())?;
            src.set_operand(&mut instr, 1);
            ass.add_instruction(instr)?;
            store_indirect(ass, reg_class, dest_indirect_idx)?;
        }
        _ => {
            let code = if src.is_indirect() { r_rm } else { rm_r };
            let mut instr = Instruction::with2(code, Register::None, Register::None)?;
            dest.set_operand(&mut instr, 0);
            src.set_operand(&mut instr, 1);
            ass.add_instruction(instr)?;
        }
    }

    Ok(())
}

/// If you are trying to make a mov, use `make_mov_rr` instead.
pub fn make_rr(
    ass: &mut CodeAssembler,
    codes: &OpRRCodes,
    reg_class: RegClass,
    dest: RegTranslation,
    src: RegTranslation,
) -> IcedResult<()> {
    make_rr_impl(ass, codes, reg_class, dest, src, true)
}

pub fn make_mov_rr(
    ass: &mut CodeAssembler,
    reg_class: RegClass,
    dest: RegTranslation,
    src: RegTranslation,
) -> IcedResult<()> {
    make_rr_impl(ass, &codes::MOV_RR_CODES, reg_class, dest, src, false)
}

pub fn make_ri(
    ass: &mut CodeAssembler,
    codes: &OpRICodes,
    reg_class: RegClass,
    reg: RegTranslation,
    imm: i32,
) -> IcedResult<()> {
    let code = match reg_class {
        RegClass::GPR32 => codes.rm32_imm32,
        RegClass::GPR64 => codes.rm64_imm32,
        _ => unimplemented!(),
    };
    let mut instr = Instruction::with2(code, Register::None, imm)?;
    reg.set_operand(&mut instr, 0);
    ass.add_instruction(instr)?;
    Ok(())
}
