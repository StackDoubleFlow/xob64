use iced_x86::{
    Instruction, OpKind, Register,
    code_asm::{CodeAssembler, gpr32},
};
use num_traits::FromPrimitive;

use crate::runner::{
    ExecCtx,
    compiler::instr_utils::{IcedResult, load_indirect, store_indirect},
};

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
// v0-v7, v16-v22
// Emulated Aarch64 registers:
// x6-x18, x24-x28 (18 64-bit registers)
// v8-v15, v23-v31 (17 128-bit registers)

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegTranslation {
    Direct(Register),
    // The register is stored at `offset` in the exec context
    Indirect(u32),
    Zero(NativeRegClass),
}

impl RegTranslation {
    pub fn is_indirect(&self) -> bool {
        matches!(self, RegTranslation::Indirect(_) | RegTranslation::Zero(_))
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
            RegTranslation::Zero(_) => unimplemented!(),
        }
    }

    pub fn pre_read(self, ass: &mut CodeAssembler, reg_class: RegClass) -> IcedResult<()> {
        match self {
            RegTranslation::Direct(_) => Ok(()),
            RegTranslation::Indirect(offset) => load_indirect(ass, reg_class, offset),
            RegTranslation::Zero(_) => ass.xor(gpr32::eax, gpr32::eax),
        }
    }

    pub fn reg_operand(&self, reg_class: RegClass) -> Register {
        match self {
            RegTranslation::Direct(reg) => *reg,
            RegTranslation::Zero(native_class) => {
                lower_reg_to_native_class(Register::RAX, *native_class)
            }
            RegTranslation::Indirect(_) => reg_class.scratch(),
        }
    }

    /// Use this when memory operand is not possible, otherwise use `set_operand`.
    /// If `is_write`, then `post_write` must be called, otherwise `pre_read` must be called.
    pub fn set_reg_operand(self, instr: &mut Instruction, idx: u32, reg_class: RegClass) {
        instr.set_op_kind(idx, OpKind::Register);
        let reg = self.reg_operand(reg_class);
        instr.set_op_register(idx, reg);
    }

    /// Make sure to use `pre_read`
    pub fn set_memory_base(self, instr: &mut Instruction, reg_class: RegClass) {
        instr.set_memory_base(self.reg_operand(reg_class));
    }

    /// Make sure to use `pre_read`
    pub fn set_memory_index(self, instr: &mut Instruction, reg_class: RegClass) {
        instr.set_memory_index(self.reg_operand(reg_class));
    }

    pub fn post_write(self, ass: &mut CodeAssembler, reg_class: RegClass) -> IcedResult<()> {
        match self {
            RegTranslation::Direct(_) | RegTranslation::Zero(_) => Ok(()),
            RegTranslation::Indirect(offset) => store_indirect(ass, reg_class, offset),
        }
    }

    pub fn with_native_class(self, native_class: NativeRegClass) -> Self {
        match self {
            RegTranslation::Direct(reg) => {
                RegTranslation::Direct(lower_reg_to_native_class(reg.full_register(), native_class))
            }
            RegTranslation::Zero(_) => RegTranslation::Zero(native_class),
            _ => self,
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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum NativeRegClass {
    GPR64,
    GPR32,
    GPR16,
    GPR8,
}

impl RegClass {
    pub fn scratch(self) -> Register {
        match self {
            RegClass::GPR64 => Register::RAX,
            RegClass::GPR32 => Register::EAX,
            RegClass::FP128 | RegClass::FP64 | RegClass::FP32 | RegClass::FP16 | RegClass::FP8 => {
                Register::XMM15
            }
        }
    }

    pub fn scratch_translation(self) -> RegTranslation {
        RegTranslation::Direct(self.scratch())
    }

    pub fn to_native_class(self) -> NativeRegClass {
        match self {
            RegClass::GPR64 => NativeRegClass::GPR64,
            RegClass::GPR32 => NativeRegClass::GPR32,
            _ => todo!(),
        }
    }
}

// Returns the RegClass and the top-level Reg
pub fn get_reg_class(reg: bad64::Reg) -> (RegClass, bad64::Reg) {
    use bad64::Reg;
    let rn = reg as u32;
    if reg == Reg::SP || rn >= Reg::X0 as u32 && rn <= Reg::XZR as u32 {
        (RegClass::GPR64, reg)
    } else if reg == Reg::WSP {
        (RegClass::GPR32, Reg::SP)
    } else if rn >= Reg::W0 as u32 && rn <= Reg::WZR as u32 {
        (
            RegClass::GPR32,
            Reg::from_u32(rn - Reg::W0 as u32 + Reg::X0 as u32).unwrap(),
        )
    } else if rn >= Reg::Q0 as u32 && rn <= Reg::Q31 as u32 {
        (RegClass::FP128, reg)
    } else if rn >= Reg::D0 as u32 && rn <= Reg::D31 as u32 {
        (
            RegClass::FP64,
            Reg::from_u32(rn - Reg::D0 as u32 + Reg::Q0 as u32).unwrap(),
        )
    } else if rn >= Reg::S0 as u32 && rn <= Reg::S31 as u32 {
        (
            RegClass::FP32,
            Reg::from_u32(rn - Reg::S0 as u32 + Reg::Q0 as u32).unwrap(),
        )
    } else if rn >= Reg::H0 as u32 && rn <= Reg::H31 as u32 {
        (
            RegClass::FP16,
            Reg::from_u32(rn - Reg::H0 as u32 + Reg::Q0 as u32).unwrap(),
        )
    } else if rn >= Reg::B0 as u32 && rn <= Reg::B31 as u32 {
        (
            RegClass::FP8,
            Reg::from_u32(rn - Reg::B0 as u32 + Reg::Q0 as u32).unwrap(),
        )
    } else {
        todo!("get_reg_class: {:?}", reg)
    }
}

pub fn lower_reg_to_native_class(reg: Register, class: NativeRegClass) -> Register {
    let rn = reg as u32;
    match class {
        NativeRegClass::GPR64 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                reg
            } else {
                panic!("Tried to lower {:?} to GPR64", reg);
            }
        }
        NativeRegClass::GPR32 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                reg.full_register32()
            } else {
                panic!("Tried to lower {:?} to GPR32", reg);
            }
        }
        NativeRegClass::GPR16 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                reg - Register::RAX as u32 + Register::AX as u32
            } else {
                panic!("Tried to lower {:?} to GPR16", reg);
            }
        }
        NativeRegClass::GPR8 => {
            if rn >= Register::RAX as u32 && rn <= Register::R15 as u32 {
                if rn <= Register::BL as u32 {
                    reg - Register::RAX as u32 + Register::AL as u32
                } else {
                    reg - Register::RAX as u32 + Register::SPL as u32
                }
            } else {
                panic!("Tried to lower {:?} to GPR8", reg);
            }
        }
    }
}

pub fn lower_reg_to_class(reg: Register, class: RegClass) -> Register {
    match class {
        RegClass::GPR64 => lower_reg_to_native_class(reg, NativeRegClass::GPR64),
        RegClass::GPR32 => lower_reg_to_native_class(reg, NativeRegClass::GPR32),
        // These all have the same register names in x86_64, just the instruction changes
        RegClass::FP128 | RegClass::FP64 | RegClass::FP32 | RegClass::FP16 | RegClass::FP8 => reg,
    }
}

fn translate_indirect_reg(reg: bad64::Reg, reg_class: RegClass) -> RegTranslation {
    use RegTranslation::Indirect;
    use bad64::Reg::*;
    let fp_offset = std::mem::offset_of!(ExecCtx, indirect_fp_regs) as u32;
    let rn = reg as u32;
    if rn >= X0 as u32 && rn <= X18 as u32 {
        Indirect((rn - X0 as u32) * 8)
    } else if rn >= X24 as u32 && rn <= X28 as u32 {
        Indirect((rn - X24 as u32 + 13) * 8)
    } else if rn >= Q8 as u32 && rn <= Q15 as u32 {
        Indirect(fp_offset + (rn - X8 as u32) * 16)
    } else if rn >= Q23 as u32 && rn <= Q31 as u32 {
        Indirect(fp_offset + (rn - X8 as u32 + 8) * 16)
    } else if reg == bad64::Reg::XZR {
        RegTranslation::Zero(reg_class.to_native_class())
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

        Q0 => Register::XMM0,
        Q1 => Register::XMM1,
        Q2 => Register::XMM2,
        Q3 => Register::XMM3,
        Q4 => Register::XMM4,
        Q5 => Register::XMM5,
        Q6 => Register::XMM6,
        Q7 => Register::XMM7,
        Q16 => Register::XMM8,
        Q17 => Register::XMM9,
        Q18 => Register::XMM10,
        Q19 => Register::XMM11,
        Q20 => Register::XMM12,
        Q21 => Register::XMM13,
        Q22 => Register::XMM14,

        _ => return (translate_indirect_reg(top_level_reg, reg_class), reg_class),
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

pub fn unwrap_imm(operand: bad64::Operand) -> (bad64::Imm, Option<bad64::Shift>) {
    match operand {
        bad64::Operand::Imm64 { imm, shift } => (imm, shift),
        bad64::Operand::Imm32 { imm, shift } => (imm, shift),
        _ => panic!("unwrapped imm on non-imm operand: {:?}", operand),
    }
}

pub fn unwrap_unsigned(imm: bad64::Imm) -> u64 {
    match imm {
        bad64::Imm::Unsigned(imm) => imm,
        _ => panic!("unwrapped unsigned on operand: {:?}", imm),
    }
}
pub fn unwrap_cond(operand: bad64::Operand) -> bad64::Condition {
    match operand {
        bad64::Operand::Cond(cond) => cond,
        _ => panic!("unwrapped cond on operand: {:?}", operand),
    }
}
