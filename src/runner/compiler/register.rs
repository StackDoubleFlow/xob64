use iced_x86::{
    Register,
    code_asm::{
        AsmMemoryOperand, AsmRegister64, CodeAssembler, asm_traits::*, dword_ptr, gpr32, gpr64,
        qword_ptr,
    },
};
use num_traits::FromPrimitive;

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
    // The register is stored at `idx` in the exec context
    Indirect(u32),
}

impl RegTranslation {
    pub fn is_indirect(&self) -> bool {
        matches!(self, RegTranslation::Indirect(_))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegClass {
    GPR64,
    GPR32,
    FP128,
    FP64,
    FP32,
    FP16,
    FP8,
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

pub fn translate_reg(reg: bad64::Reg) -> RegTranslation {
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

#[derive(Clone, Copy)]
pub enum RegOrMemory64 {
    Register(AsmRegister64),
    Memory(AsmMemoryOperand),
}

impl RegOrMemory64 {
    pub fn add_src<Src>(self, ass: &mut CodeAssembler, src: Src) -> Result<(), iced_x86::IcedError>
    where
        CodeAssembler: CodeAsmAdd<AsmMemoryOperand, Src> + CodeAsmAdd<AsmRegister64, Src>,
    {
        match self {
            RegOrMemory64::Register(reg) => ass.add(reg, src),
            RegOrMemory64::Memory(mem) => ass.add(mem, src),
        }
    }
}

pub fn load_indirect<Dest>(
    ass: &mut CodeAssembler,
    dest: Dest,
    indirect_idx: u32,
) -> Result<(), iced_x86::IcedError>
where
    CodeAssembler: CodeAsmMov<Dest, AsmMemoryOperand>,
{
    let mem = gpr64::r15 + indirect_idx * 8;
    // TODO: does the size hint type matter here?
    ass.mov(dest, qword_ptr(mem))
}

// Returns either a x86_64 register or memory operand
pub fn reg_operand_gpr64(reg: bad64::Reg) -> RegOrMemory64 {
    match translate_reg(reg) {
        RegTranslation::Direct(reg) => RegOrMemory64::Register(gpr64::get_gpr64(reg).unwrap()),
        RegTranslation::Indirect(reg) => RegOrMemory64::Memory(qword_ptr(gpr64::r15 + reg * 8)),
    }
}

pub fn reg_operand_no_mem(
    ass: &mut CodeAssembler,
    reg: bad64::Reg,
) -> Result<Register, iced_x86::IcedError> {
    let (reg_class, top_level_reg) = get_reg_class(reg);
    let translation = translate_reg(top_level_reg);
    let indirect_idx = match translation {
        RegTranslation::Direct(reg) => return Ok(lower_reg_to_class(reg, reg_class)),
        RegTranslation::Indirect(idx) => idx,
    };
    match reg_class {
        RegClass::GPR64 => load_indirect(ass, gpr64::rax, indirect_idx)?,
        RegClass::GPR32 => load_indirect(ass, gpr32::eax, indirect_idx)?,
        _ => todo!("reg_operand_no_mem: {:?}", reg_class),
    }
    Ok(lower_reg_to_class(Register::RAX, reg_class))
}
