use iced_x86::{
    Code, Instruction, MemoryOperand, Register,
    code_asm::{CodeAssembler, gpr8, gpr32, gpr64},
};

use crate::runner::compiler::{
    branch::make_jcc,
    instr_utils::{
        IcedResult, OpRICodes, OpRRCodes,
        codes::{
            ADD_RI_CODES, ADD_RR_CODES, AND_RI_CODES, AND_RR_CODES, CMP_RI_CODES, MOV_RI_CODES,
            OR_RI_CODES, SUB_RI_CODES, SUB_RR_CODES,
        },
        get_alt_reg, get_shamt_from_shift, label_target, make_cmp_rr, make_mov_ri64, make_mov_rr,
        make_ri, make_rr,
    },
    register::{
        NativeRegClass, RegClass, RegTranslation, lower_reg_to_class, translate_reg, unwrap_cond,
        unwrap_imm, unwrap_reg, unwrap_unsigned,
    },
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
        src2.set_operand(&mut op, 1);
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
    let native_src_class = match shift {
        Shift::UXTB(_) | Shift::SXTB(_) => NativeRegClass::GPR8,
        Shift::UXTH(_) | Shift::SXTH(_) => NativeRegClass::GPR16,
        Shift::UXTW(_) | Shift::SXTW(_) => NativeRegClass::GPR32,
        Shift::UXTX(_) | Shift::SXTX(_) | Shift::LSL(_) | Shift::LSR(_) | Shift::ASR(_) => {
            src_class.to_native_class()
        }
        _ => unreachable!(),
    };
    let mov_code = match dest_class {
        RegClass::GPR64 => mov_64,
        RegClass::GPR32 => mov_32,
        _ => unreachable!(),
    };
    let mut mov = Instruction::with2(mov_code, dest, Register::None)?;
    src.with_native_class(native_src_class)
        .set_operand(&mut mov, 1);
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
                RegTranslation::Direct(lower_reg_to_class(src2_reg, reg_class)),
                reg_class,
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

fn translate_cmp_cmn(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (src1, reg_class) = translate_reg(unwrap_reg(operands[0]));
    match operands[1] {
        bad64::Operand::Reg { reg: src2, .. } => {
            let (src2, _) = translate_reg(src2);
            if arm_instr.op() == bad64::Op::CMP {
                make_cmp_rr(ass, reg_class, src1, src2)?;
            } else {
                let scratch = reg_class.scratch_translation();
                make_mov_rr(ass, reg_class, scratch, src1)?;
                make_rr(ass, &ADD_RR_CODES, reg_class, scratch, src2)?;
            }
        }
        bad64::Operand::Imm64 { imm, .. } | bad64::Operand::Imm32 { imm, .. } => {
            let imm = unwrap_unsigned(imm);
            if arm_instr.op() == bad64::Op::CMP {
                make_ri(ass, &CMP_RI_CODES, reg_class, src1, imm as i32)?;
            } else {
                let scratch = reg_class.scratch_translation();
                make_mov_rr(ass, reg_class, scratch, src1)?;
                make_ri(ass, &ADD_RI_CODES, reg_class, scratch, imm as i32)?;
            }
        }
        operand => todo!("operand: {:?}", operand),
    }

    Ok(())
}

fn translate_div(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    signed: bool,
) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let (src1, _) = translate_reg(unwrap_reg(operands[1]));
    let (src2, _) = translate_reg(unwrap_reg(operands[2]));
    ass.push(gpr64::rdx)?;
    make_mov_rr(ass, reg_class, reg_class.scratch_translation(), src1)?;
    if signed {
        ass.cdq()?;
    } else {
        ass.xor(gpr32::edx, gpr32::edx)?;
    }
    let (unsigned_code, signed_code) = match reg_class {
        RegClass::GPR32 => (Code::Div_rm32, Code::Idiv_rm32),
        RegClass::GPR64 => (Code::Div_rm64, Code::Idiv_rm64),
        _ => unimplemented!(),
    };
    let mut div = Instruction::with1(
        if signed { signed_code } else { unsigned_code },
        Register::None,
    )?;
    src2.set_operand(&mut div, 0);
    ass.add_instruction(div)?;
    make_mov_rr(ass, reg_class, dest, reg_class.scratch_translation())?;
    ass.pop(gpr64::rdx)?;
    Ok(())
}

fn translate_madd(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let (src1, _) = translate_reg(unwrap_reg(operands[1]));
    let (src2, _) = translate_reg(unwrap_reg(operands[2]));
    let (src3, _) = translate_reg(unwrap_reg(operands[2]));

    ass.push(gpr64::rdx)?;
    make_mov_rr(ass, reg_class, reg_class.scratch_translation(), src1)?;

    let (mul_code, add_code) = match reg_class {
        RegClass::GPR32 => (Code::Mul_rm32, Code::Add_r32_rm32),
        RegClass::GPR64 => (Code::Mul_rm64, Code::Add_r64_rm64),
        _ => unimplemented!(),
    };

    let mut mul = Instruction::with1(mul_code, Register::None)?;
    src2.set_operand(&mut mul, 0);
    ass.add_instruction(mul)?;

    let mut add = Instruction::with2(add_code, reg_class.scratch(), Register::None)?;
    src3.set_operand(&mut add, 1);
    ass.add_instruction(add)?;

    make_mov_rr(ass, reg_class, dest, reg_class.scratch_translation())?;
    ass.pop(gpr64::rdx)?;

    Ok(())
}

fn translate_csel(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let (src1, _) = translate_reg(unwrap_reg(operands[1]));
    let (src2, _) = translate_reg(unwrap_reg(operands[2]));
    let cond = unwrap_cond(operands[3]);

    make_mov_rr(ass, reg_class, reg_class.scratch_translation(), src2)?;

    let (cmov32, cmov64) = match cond {
        bad64::Condition::EQ => (Code::Cmove_r32_rm32, Code::Cmove_r64_rm64),
        bad64::Condition::NE => (Code::Cmovne_r32_rm32, Code::Cmovne_r64_rm64),
        bad64::Condition::CS => (Code::Cmovae_r32_rm32, Code::Cmovae_r64_rm64),
        bad64::Condition::CC => (Code::Cmovb_r32_rm32, Code::Cmovb_r64_rm64),
        bad64::Condition::MI => (Code::Cmovs_r32_rm32, Code::Cmovs_r64_rm64),
        bad64::Condition::PL => (Code::Cmovns_r32_rm32, Code::Cmovns_r64_rm64),
        bad64::Condition::VS => (Code::Cmovo_r32_rm32, Code::Cmovo_r64_rm64),
        bad64::Condition::VC => (Code::Cmovno_r32_rm32, Code::Cmovno_r64_rm64),
        bad64::Condition::HI => (Code::Cmova_r32_rm32, Code::Cmova_r64_rm64),
        bad64::Condition::LS => (Code::Cmovbe_r32_rm32, Code::Cmovbe_r64_rm64),
        bad64::Condition::GE => (Code::Cmovge_r32_rm32, Code::Cmovge_r64_rm64),
        bad64::Condition::LT => (Code::Cmovl_r32_rm32, Code::Cmovl_r64_rm64),
        bad64::Condition::GT => (Code::Cmovg_r32_rm32, Code::Cmovg_r64_rm64),
        bad64::Condition::LE => (Code::Cmovle_r32_rm32, Code::Cmovle_r64_rm64),
        bad64::Condition::AL | bad64::Condition::NV => (Code::Mov_r32_rm32, Code::Mov_r64_rm64),
    };

    let mut cmov = Instruction::with2(
        if reg_class == RegClass::GPR64 {
            cmov64
        } else {
            cmov32
        },
        Register::RAX,
        Register::None,
    )?;
    src1.set_operand(&mut cmov, 1);
    ass.add_instruction(cmov)?;

    make_mov_rr(ass, reg_class, dest, reg_class.scratch_translation())?;

    Ok(())
}

fn set_flags(ass: &mut CodeAssembler, nzcv: u64) -> IcedResult<()> {
    ass.pushfq()?;
    ass.pop(gpr64::rax)?;

    // Mask to clear CF, ZF, SF, and OF flags
    let and_mask = !0x08C1u32;
    ass.and(gpr64::rax, and_mask as i32)?;

    let mut set_mask = 0;
    if nzcv & 0b0001 != 0 {
        // V flag corresponds to OF flag
        set_mask |= 1 << 11;
    }
    if nzcv & 0b0010 != 0 {
        // C flag corresponds to CF flag
        set_mask |= 1;
    }
    if nzcv & 0b0100 != 0 {
        // Z flag corresponds to ZF flag
        set_mask |= 1 << 6;
    }
    if nzcv & 0b1000 != 0 {
        // N flag corresponds to SF flag
        set_mask |= 1 << 7;
    }
    if nzcv != 0 {
        ass.or(gpr64::rax, set_mask)?;
    }

    ass.push(gpr64::rax)?;
    ass.popfq()?;
    Ok(())
}

fn translate_ccmp(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (src1, reg_class) = translate_reg(unwrap_reg(operands[0]));
    let cond = unwrap_cond(operands[3]);

    let mut end_label = ass.create_label();
    let mut cond_pass_label = ass.create_label();

    make_jcc(ass, cond, cond_pass_label)?;

    let nzcv = unwrap_unsigned(unwrap_imm(operands[1]).0);
    set_flags(ass, nzcv)?;

    ass.jmp(end_label)?;
    ass.set_label(&mut cond_pass_label)?;

    // `cmp <src1>, <src2|imm>
    match operands[1] {
        bad64::Operand::Reg { reg: src2, .. } => {
            let (src2, _) = translate_reg(src2);
            make_cmp_rr(ass, reg_class, src1, src2)?;
        }
        bad64::Operand::Imm32 { imm, .. } | bad64::Operand::Imm64 { imm, .. } => {
            let imm = unwrap_unsigned(imm);
            make_ri(ass, &CMP_RI_CODES, reg_class, src1, imm as i32)?;
        }
        _ => unreachable!(),
    }

    ass.set_label(&mut end_label)?;
    ass.zero_bytes()?;

    Ok(())
}

/// Sets AL register to condition
pub fn make_setcc(
    ass: &mut CodeAssembler,
    cond: bad64::Condition,
    reg: RegTranslation,
) -> IcedResult<()> {
    use bad64::Condition::*;
    match cond {
        EQ => ass.sete(gpr8::al),
        NE => ass.setne(gpr8::al),
        LT => ass.setl(gpr8::al),
        GE => ass.setge(gpr8::al),
        GT => ass.setg(gpr8::al),
        LE => ass.setle(gpr8::al),
        CC => ass.setnc(gpr8::al),
        CS => ass.setc(gpr8::al),
        HI => ass.seta(gpr8::al),
        LS => ass.setbe(gpr8::al),
        VC => ass.setno(gpr8::al),
        VS => ass.seto(gpr8::al),
        MI => ass.sets(gpr8::al),
        PL => ass.setns(gpr8::al),
        AL | NV => ass.mov(gpr8::al, 1),
    }?;
    let reg = reg.with_native_class(NativeRegClass::GPR32);
    match reg {
        RegTranslation::Direct(reg) => {
            ass.add_instruction(Instruction::with2(Code::Movzx_r32_rm8, reg, Register::AL)?)?
        }
        RegTranslation::Indirect(_) => {
            ass.movzx(gpr32::eax, gpr8::al)?;
            let mut mov = Instruction::with2(Code::Mov_rm32_r32, Register::None, Register::RAX)?;
            reg.set_operand(&mut mov, 0);
            ass.add_instruction(mov)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn translate_cset(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<()> {
    let operands = arm_instr.operands();
    let (dest, _) = translate_reg(unwrap_reg(operands[0]));
    let cond = unwrap_cond(operands[1]);
    make_setcc(ass, cond, dest)?;
    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(arm_instr: &bad64::Instruction, ass: &mut CodeAssembler) -> IcedResult<bool> {
    use bad64::Op;
    let operands = arm_instr.operands();
    match arm_instr.op() {
        Op::NOP => ass.nop()?,
        Op::CMP | Op::CMN => translate_cmp_cmn(arm_instr, ass)?,
        Op::MOV => {
            let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));

            match operands[1] {
                bad64::Operand::Reg { reg: src, .. } => {
                    let (src_translation, _) = translate_reg(src);
                    make_mov_rr(ass, reg_class, dest, src_translation)?;
                }
                bad64::Operand::Imm64 { imm, .. } | bad64::Operand::Imm32 { imm, .. } => {
                    // The immediate is really encoded in 16 bits, so this cast is ok
                    let imm = unwrap_unsigned(imm) as i32;
                    make_ri(ass, &MOV_RI_CODES, reg_class, dest, imm)?;
                }
                operand => todo!("operand: {:?}", operand),
            }
        }
        Op::AND => {
            let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));
            let (src1, _) = translate_reg(unwrap_reg(operands[1]));

            match operands[2] {
                bad64::Operand::Reg { reg: src2, .. } => {
                    let (src2, _) = translate_reg(src2);
                    make_rrr(ass, &AND_RR_CODES, dest, src1, src2, reg_class)?;
                }
                bad64::Operand::Imm64 { imm, .. } | bad64::Operand::Imm32 { imm, .. } => {
                    // Full width bit patterns can be encoded in logical immediate instruction
                    let imm = unwrap_unsigned(imm);
                    if dest != src1 {
                        make_mov_rr(ass, reg_class, dest, src1)?;
                    }
                    ass.mov(gpr64::rax, imm)?;
                    let scratch = reg_class.scratch_translation();
                    make_rr(ass, &AND_RR_CODES, reg_class, dest, scratch)?;
                }
                _ => unreachable!(),
            }
        }
        Op::MOVK => {
            let (dest, reg_class) = translate_reg(unwrap_reg(operands[0]));
            let (imm, shift) = unwrap_imm(operands[1]);
            let imm = unwrap_unsigned(imm);
            let shift = match shift {
                None => 0,
                Some(bad64::Shift::LSL(shift)) => shift,
                _ => unreachable!(),
            };
            let imm = imm << shift;
            let mask = !(0xFFFFu64 << shift);

            if shift > 24 {
                ass.mov(gpr64::rax, mask)?;
                let mut and =
                    Instruction::with2(Code::And_rm64_r64, Register::None, Register::RAX)?;
                dest.set_operand(&mut and, 0);
                ass.add_instruction(and)?;
                ass.mov(gpr64::rax, imm)?;
                let mut or = Instruction::with2(Code::Or_rm64_r64, Register::None, Register::RAX)?;
                dest.set_operand(&mut or, 0);
                ass.add_instruction(or)?;
            } else {
                make_ri(ass, &AND_RI_CODES, reg_class, dest, mask as i32)?;
                make_ri(ass, &OR_RI_CODES, reg_class, dest, imm as i32)?;
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
        Op::SXTW | Op::SXTH | Op::SXTB | Op::UXTH | Op::UXTB => {
            let (dest, dest_class) = translate_reg(unwrap_reg(operands[0]));
            let (src, src_class) = translate_reg(unwrap_reg(operands[1]));
            let shift = match arm_instr.op() {
                Op::SXTW => bad64::Shift::SXTW(0),
                Op::SXTH => bad64::Shift::SXTH(0),
                Op::SXTB => bad64::Shift::SXTB(0),
                Op::UXTH => bad64::Shift::UXTH(0),
                Op::UXTB => bad64::Shift::UXTB(0),
                _ => unreachable!(),
            };
            match dest {
                RegTranslation::Direct(dest) => {
                    load_shifted(ass, dest, dest_class, src, src_class, shift)?;
                }
                RegTranslation::Indirect(_) => {
                    load_shifted(ass, dest_class.scratch(), dest_class, src, src_class, shift)?;
                    make_mov_rr(ass, dest_class, dest, dest_class.scratch_translation())?;
                }
                _ => todo!(),
            }
        }
        Op::UDIV => translate_div(arm_instr, ass, false)?,
        Op::SDIV => translate_div(arm_instr, ass, true)?,
        Op::MADD => translate_madd(arm_instr, ass)?,
        Op::CSEL => translate_csel(arm_instr, ass)?,
        Op::CCMP => translate_ccmp(arm_instr, ass)?,
        Op::CSET => translate_cset(arm_instr, ass)?,
        _ => return Ok(false),
    }

    Ok(true)
}
