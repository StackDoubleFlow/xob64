use iced_x86::{Code, Instruction, MemoryOperand, Register, code_asm::CodeAssembler};

use crate::runner::compiler::{
    instr_utils::{
        IcedResult, OpRICodes, OpRRCodes,
        codes::{ADD_RR_CODES, MOV_RI_CODES, SUB_RI_CODES, SUB_RR_CODES},
        get_alt_reg, get_shamt_from_shift, label_target, make_mov_ri64, make_mov_rr, make_ri,
        make_rr,
    },
    register::{RegClass, RegTranslation, translate_reg, unwrap_reg},
};

// Given `ORR dest, src1, src2`
//
// If src1 and dest are equal, we have this:
// ```
// or dest, src2
// ```
// Either dest/src1 or src2 can be indirect in this case, but if both are indirect:
// ```
// mov rax, [r15 + dest_offset]
// or rax, [r15 + src2_offset]
// mov [r15 + dest_offset], rax
// ```
// The above 2 cases can be handled by `make_rr`
//
// If src1 and dest are not equal, we need to transfer from src1 to dest first, and then perform the operation on dest with src2.
// If dest is not indirect, we get this case. src1 and src2 can become memory operands if they are indirect.
// ```
// mov dest, src1
// or dest, src2
// ```
// But, if dest is indirect, then it's best to operate with scratch as dest, and then store that at the end.
// As before, src1 and src2 can become memory operands.
// ```
// mov rax, src1
// or rax, src2
// mov [r15 + dest_offset], rax
// ```
// The above two cases are essentially
// ```
// // Create mov instruction
// dest_translation.set_reg_operand(&mut mov_instr, 0, reg_class);
// src1_translation.set_operand(&mut mov_instr, 1);
// // Create or instruction
// dest_translation.set_reg_operand(&mut or_instr, 0, reg_class);
// src2_translation.set_operand(&mut or_instr, 1);
// // Create last mov if needed
// dest_translation.post_write(ass, reg_class);
// ```
//
// From there, this is the absolute worst case:
// ```
// mov rax, [r15 + src1_offset] ; Load indirect src1
// or rax, [r15 + src2_offset] ; Perform operation on scratch with src2
// mov [r15 + dest_offset], rax; Transfer to indirect dest
// ```
fn make_rrr(
    ass: &mut CodeAssembler,
    codes: &OpRRCodes,
    dest: RegTranslation,
    src1: RegTranslation,
    src2: RegTranslation,
    reg_class: RegClass,
    shift_src2: Option<Register>,
) -> IcedResult<()> {
    // TODO: eflags preservation?
    if dest == src1 {
        make_rr(ass, codes, reg_class, dest, src2)?;
    } else {
        let (mov_code, op_code) = match reg_class {
            RegClass::GPR64 => (Code::Mov_r64_rm64, codes.r64_rm64),
            RegClass::GPR32 => (Code::Mov_r32_rm32, codes.r32_rm32),
            _ => unimplemented!(),
        };
        let mut mov = Instruction::with2(mov_code, Register::None, Register::None)?;
        dest.set_reg_operand(&mut mov, 0, reg_class);
        src1.set_operand(&mut mov, 1);
        ass.add_instruction(mov)?;
        let mut op = Instruction::with2(op_code, Register::None, Register::None)?;
        dest.set_reg_operand(&mut op, 0, reg_class);
        if let Some(shift_reg) = shift_src2 {
            op.set_op1_register(shift_reg);
        } else {
            src2.set_operand(&mut op, 1);
        }
        ass.add_instruction(op)?;
        dest.post_write(ass, reg_class)?;
    }
    Ok(())
}

fn load_shifted(
    ass: &mut CodeAssembler,
    dest: Register,
    dest_class: RegClass,
    src: RegTranslation,
    src_class: RegClass,
    shift: bad64::Shift,
) -> IcedResult<bool> {
    use bad64::Shift;
    let (mov_64, mov_32) = match shift {
        Shift::MSL(_) | Shift::ROR(_) => return Ok(false),
        // Moving to a 32-bit register zero-extends it, so most of these are the same between GPR64 and GPR32
        Shift::UXTB(_) => (Code::Movzx_r64_rm8, Code::Movzx_r32_rm8),
        Shift::UXTH(_) => (Code::Movzx_r64_rm16, Code::Movzx_r32_rm16),
        Shift::UXTW(_) => (Code::Mov_r32_rm32, Code::Mov_r32_rm32),
        Shift::UXTX(_) => (Code::Mov_r64_rm64, Code::Mov_r32_rm32),
        Shift::SXTB(_) => (Code::Movsx_r64_rm8, Code::Movsx_r32_rm8),
        Shift::SXTH(_) => (Code::Movsx_r64_rm16, Code::Movsx_r32_rm16),
        Shift::SXTW(_) => (Code::Movsxd_r64_rm32, Code::Mov_r32_rm32),
        Shift::SXTX(_) => (
            if src_class == RegClass::GPR64 {
                Code::Mov_r64_rm64
            } else {
                Code::Movsxd_r64_rm32
            },
            Code::Mov_r32_rm32,
        ),
        // In these cases, src_class and dest_class are always the same
        Shift::LSL(_) | Shift::LSR(_) | Shift::ASR(_) => (Code::Mov_r64_rm64, Code::Mov_r32_rm32),
    };
    let mov_code = match dest_class {
        RegClass::GPR64 => mov_64,
        RegClass::GPR32 => mov_32,
        _ => unreachable!(),
    };
    let mut mov = Instruction::with2(mov_code, dest, Register::None)?;
    src.set_operand(&mut mov, 1);
    ass.add_instruction(mov)?;

    let shamt = get_shamt_from_shift(shift);
    if shamt == 0 {
        return Ok(true);
    }

    let (shift_64, shift_32) = match shift {
        Shift::LSR(_) => (Code::Shr_rm64_imm8, Code::Shr_rm32_imm8),
        Shift::ASR(_) => (Code::Sar_rm64_imm8, Code::Sar_rm32_imm8),
        _ => (Code::Shl_rm64_imm8, Code::Shl_rm32_imm8),
    };
    let shift_code = match dest_class {
        RegClass::GPR64 => shift_64,
        RegClass::GPR32 => shift_32,
        _ => unreachable!(),
    };
    let shift = Instruction::with2(shift_code, dest, shamt)?;
    ass.add_instruction(shift)?;

    Ok(true)
}

fn make_rri(
    ass: &mut CodeAssembler,
    codes: &OpRICodes,
    dest_translation: RegTranslation,
    src_translation: RegTranslation,
    imm: i32,
    reg_class: RegClass,
) -> IcedResult<()> {
    // TODO: eflags preservation?
    if dest_translation == src_translation {
        make_ri(ass, codes, reg_class, dest_translation, imm)?;
    } else {
        let (mov_code, op_code) = match reg_class {
            RegClass::GPR64 => (Code::Mov_r64_rm64, codes.rm64_imm32),
            RegClass::GPR32 => (Code::Mov_r32_rm32, codes.rm32_imm32),
            _ => unimplemented!(),
        };
        let mut mov = Instruction::with2(mov_code, Register::None, Register::None)?;
        dest_translation.set_reg_operand(&mut mov, 0, reg_class);
        src_translation.set_operand(&mut mov, 1);
        ass.add_instruction(mov)?;
        let mut op = Instruction::with2(op_code, Register::None, imm)?;
        dest_translation.set_reg_operand(&mut op, 0, reg_class);
        dest_translation.post_write(ass, reg_class)?;
    }
    Ok(())
}

fn translate_add_sub(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<bool> {
    let operands = arm_instr.operands();
    let (dest_translation, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let (src1_translation, _) = translate_reg(unwrap_reg(operands[1]));
    let lea_code = match reg_class {
        RegClass::GPR64 => Code::Lea_r64_m,
        RegClass::GPR32 => Code::Lea_r32_m,
        _ => todo!(),
    };
    let mut lea = Instruction::with2(
        lea_code,
        Register::None,
        MemoryOperand::with_base(Register::None),
    )?;
    dest_translation.set_reg_operand(&mut lea, 0, reg_class);
    src1_translation.set_memory_base(&mut lea, reg_class);
    match operands[2] {
        bad64::Operand::ShiftReg { reg, shift } => {
            let (src2_translation, src2_class) = translate_reg(reg);
            let using_alt_reg = dest_translation.is_indirect() || src1_translation.is_indirect();
            let src2_reg = if using_alt_reg {
                let alt_reg = get_alt_reg(&[src1_translation, src1_translation]);
                ass.add_instruction(Instruction::with1(Code::Push_r64, alt_reg)?)?;
                alt_reg
            } else {
                Register::RAX
            };
            load_shifted(
                ass,
                src2_reg,
                reg_class,
                src2_translation,
                src2_class,
                shift,
            )?;
            let codes = if arm_instr.op() == bad64::Op::SUB {
                &SUB_RR_CODES
            } else {
                &ADD_RR_CODES
            };
            make_rrr(
                ass,
                codes,
                dest_translation,
                src1_translation,
                src2_translation,
                reg_class,
                Some(src2_reg),
            )?;
            if using_alt_reg {
                ass.add_instruction(Instruction::with1(Code::Pop_r64, src2_reg)?)?;
            }
        }
        bad64::Operand::Reg { reg, .. } => {
            let (src2_translation, _) = translate_reg(reg);
            if arm_instr.op() == bad64::Op::SUB {
                // We can't use lea for sub
                make_rrr(
                    ass,
                    &SUB_RR_CODES,
                    dest_translation,
                    src1_translation,
                    src2_translation,
                    reg_class,
                    None,
                )?;
            } else if src1_translation.is_indirect() && src2_translation.is_indirect() {
                // Since both operands need to be registers
                make_rrr(
                    ass,
                    &ADD_RR_CODES,
                    dest_translation,
                    src1_translation,
                    src2_translation,
                    reg_class,
                    None,
                )?;
            } else {
                src2_translation.set_memory_index(&mut lea, reg_class);
                src1_translation.pre_read(ass, reg_class)?;
                src2_translation.pre_read(ass, reg_class)?;
                ass.add_instruction(lea)?;
                dest_translation.post_write(ass, reg_class)?;
            }
        }
        bad64::Operand::Imm64 {
            imm: bad64::Imm::Unsigned(imm),
            shift,
        } => {
            if shift.is_some() {
                return Ok(false);
            }
            if arm_instr.op() == bad64::Op::SUB {
                // We can't use lea for sub
                make_rri(
                    ass,
                    &SUB_RI_CODES,
                    dest_translation,
                    src1_translation,
                    imm as i32,
                    reg_class,
                )?;
            } else {
                lea.set_memory_displ_size(8);
                lea.set_memory_displacement64(imm as u64);
                src1_translation.pre_read(ass, reg_class)?;
                ass.add_instruction(lea)?;
                dest_translation.post_write(ass, reg_class)?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn translate_shift(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (dest_translation, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let (src1_translation, _) = translate_reg(unwrap_reg(operands[1]));

    let (r32_rm32_r32, r64_rm64_r32) = match arm_instr.op() {
        bad64::Op::ASR => (Code::VEX_Sarx_r32_rm32_r32, Code::VEX_Sarx_r64_rm64_r64),
        bad64::Op::LSL => (Code::VEX_Shlx_r32_rm32_r32, Code::VEX_Shlx_r64_rm64_r64),
        bad64::Op::LSR => (Code::VEX_Shrx_r32_rm32_r32, Code::VEX_Shrx_r64_rm64_r64),
        _ => unreachable!(),
    };

    // FIXME: these affect flags
    let (rm32_i, rm64_i) = match arm_instr.op() {
        bad64::Op::ASR => (Code::Sar_rm32_imm8, Code::Sar_rm64_imm8),
        bad64::Op::LSL => (Code::Shl_rm32_imm8, Code::Shl_rm64_imm8),
        bad64::Op::LSR => (Code::Shr_rm32_imm8, Code::Shr_rm64_imm8),
        _ => unreachable!(),
    };

    let (r_rm_r, rm_i) = match reg_class {
        RegClass::GPR64 => (r64_rm64_r32, rm64_i),
        RegClass::GPR32 => (r32_rm32_r32, rm32_i),
        _ => unreachable!(),
    };

    match operands[2] {
        bad64::Operand::Reg { reg, .. } => {
            let (src2_translation, _) = translate_reg(reg);
            src2_translation.pre_read(ass, reg_class)?;
            let mut shift_inst =
                Instruction::with3(r_rm_r, Register::None, Register::None, Register::None)?;
            dest_translation.set_reg_operand(&mut shift_inst, 0, reg_class);
            src1_translation.set_operand(&mut shift_inst, 1);
            src2_translation.set_reg_operand(&mut shift_inst, 2, reg_class);
            ass.add_instruction(shift_inst)?;
            dest_translation.post_write(ass, reg_class)?;
        }
        bad64::Operand::Imm64 {
            imm: bad64::Imm::Unsigned(imm),
            ..
        } => {
            let mut shift_inst = Instruction::with2(rm_i, Register::None, imm as i32)?;
            dest_translation.set_operand(&mut shift_inst, 0);
            ass.add_instruction(shift_inst)?;
        }
        _ => unreachable!(),
    }

    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<bool> {
    use bad64::Op;
    let operands = arm_instr.operands();
    match arm_instr.op() {
        Op::NOP => ass.nop()?,
        Op::MOV => {
            let dest = unwrap_reg(operands[0]);
            let (dest_translation, reg_class) = translate_reg(dest);

            match operands[1] {
                bad64::Operand::Reg { reg: src, .. } => {
                    let (src_translation, _) = translate_reg(src);
                    make_mov_rr(ass, reg_class, dest_translation, src_translation)?;
                }
                bad64::Operand::Imm64 { imm, .. } | bad64::Operand::Imm32 { imm, .. } => {
                    let bad64::Imm::Unsigned(imm) = imm else {
                        unreachable!()
                    };
                    // The immediate is really encoded in 16 bits, so this cast is ok
                    let imm = imm as i32;
                    make_ri(ass, &MOV_RI_CODES, reg_class, dest_translation, imm)?;
                }
                operand => todo!("operand: {:?}", operand),
            }
        }
        Op::ADRP => {
            let dest = unwrap_reg(operands[0]);
            let (dest_translation, reg_class) = translate_reg(dest);
            assert_eq!(reg_class, RegClass::GPR64);
            let addr = label_target(operands[1]);
            make_mov_ri64(ass, dest_translation, addr as i64)?;
        }
        Op::ADD | Op::SUB => return translate_add_sub(arm_instr, ass),
        Op::ASR | Op::LSR | Op::LSL => translate_shift(arm_instr, ass)?,
        _ => return Ok(false),
    }

    Ok(true)
}
