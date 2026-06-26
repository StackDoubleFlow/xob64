use iced_x86::{Instruction, OpKind, Register, code_asm::CodeAssembler};
use num_traits::FromPrimitive;

use crate::runner::compiler::instr_utils::{IcedResult, load_indirect, store_indirect};

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegTranslation {
    Direct(Register),
    // The register is stored at `offset` in the exec context
    Indirect(u32),
}

impl RegTranslation {
    pub fn is_indirect(&self) -> bool {
        matches!(self, RegTranslation::Indirect(_))
    }

    pub fn set_operand(self, instr: &mut Instruction, idx: u32) {
        match self {
            RegTranslation::Direct(reg) => {
                instr.set_op_kind(idx, OpKind::Register);
                instr.set_op_register(idx, reg);
            }
            RegTranslation::Indirect(offset) => {
                instr.set_op_kind(idx, OpKind::Memory);
                instr.set_memory_base(Register::R15);
                instr.set_memory_displacement32(offset);
                instr.set_memory_displ_size(1);
            }
        }
    }

    pub fn pre_read(self, ass: &mut CodeAssembler, reg_class: RegClass) -> IcedResult<()> {
        match self {
            RegTranslation::Direct(_) => Ok(()),
            RegTranslation::Indirect(offset) => load_indirect(ass, reg_class, offset),
        }
    }

    /// Use this when memory operand is not possible, otherwise use `set_operand`.
    /// If `is_write`, then `post_write` must be called, otherwise `pre_read` must be called.
    pub fn set_reg_operand(self, instr: &mut Instruction, idx: u32, reg_class: RegClass) {
        instr.set_op_kind(idx, OpKind::Register);
        let reg = match self {
            RegTranslation::Direct(reg) => reg,
            RegTranslation::Indirect(_) => reg_class.scratch(),
        };
        instr.set_op_register(idx, reg);
    }

    pub fn post_write(self, ass: &mut CodeAssembler, reg_class: RegClass) -> IcedResult<()> {
        match self {
            RegTranslation::Direct(_) => Ok(()),
            RegTranslation::Indirect(offset) => store_indirect(ass, reg_class, offset),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RegClass {
    GPR64,
    GPR32,
    FP128,
    FP64,
    FP32,
    FP16,
    FP8,
}

impl RegClass {
    pub fn scratch(self) -> Register {
        match self {
            RegClass::GPR64 => Register::RAX,
            RegClass::GPR32 => Register::EAX,
            _ => todo!(),
        }
    }
}

// Returns the RegClass and the top-level Reg
pub fn get_reg_class(reg: bad64::Reg) -> (RegClass, bad64::Reg) {
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

pub fn lower_reg_to_class(reg: Register, class: RegClass) -> Register {
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

pub fn translate_reg(reg: bad64::Reg) -> (RegTranslation, RegClass) {
    use bad64::Reg::*;
    let (reg_class, top_level_reg) = get_reg_class(reg);
    let direct_translation = match top_level_reg {
        // These are sorted by frequency. See the comment at the top of the file.
        X0 => Register::RDI,
        SP => Register::RSP,
        X1 => Register::RSI,
        X2 => Register::RDX,
        X19 => Register::RBX,
        X3 => Register::RCX,
        X20 => Register::R12,
        X21 => Register::R13,
        X4 => Register::R8,
        X29 => Register::RBP,
        X22 => Register::R14,
        X5 => Register::R9,
        X23 => Register::R10,
        X30 => Register::R11,

        _ => return (translate_indirect_reg(reg), reg_class),
    };
    (
        RegTranslation::Direct(lower_reg_to_class(direct_translation, reg_class)),
        reg_class,
    )
}

pub fn unwrap_reg(operand: bad64::Operand) -> bad64::Reg {
    match operand {
        bad64::Operand::Reg { reg, .. } => reg,
        _ => panic!("unwrapped reg on non-reg operand: {:?}", operand),
    }
}
