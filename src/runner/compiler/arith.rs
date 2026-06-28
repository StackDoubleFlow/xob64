use iced_x86::{Code, Instruction, MemoryOperand, Register, code_asm::CodeAssembler};

use crate::runner::compiler::{
    instr_utils::{
        IcedResult, codes::MOV_RI_CODES, label_target, make_mov_ri64, make_mov_rr, make_ri,
    },
    register::{RegClass, translate_reg, unwrap_reg},
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
// mov rax, [r15 + dest_offset] ; Load indirect src1
// or rax, [r15 + src2_offset] ; Perform operation on scratch with src2
// mov [r15 + dest_offset], rax; Transfer to indirect dest
// ```

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
        bad64::Operand::Reg { reg, .. } => {
            let (src2_translation, _) = translate_reg(reg);
            if src1_translation.is_indirect() && src2_translation.is_indirect() {
                return Ok(false);
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
            lea.set_memory_displ_size(8);
            lea.set_memory_displacement64(imm as u64);
            src1_translation.pre_read(ass, reg_class)?;
            ass.add_instruction(lea)?;
            dest_translation.post_write(ass, reg_class)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
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
        Op::ADD => return translate_add_sub(arm_instr, ass),
        // Op::ORR => {

        //     let operands = arm_instr.operands();
        //     let dest = unwrap_reg(operands[0]);
        //     let src1 = unwrap_reg(operands[1]);

        //     if dest != src1 {
        //         move_to_dest(ass, dest, src1)?;
        //     }
        //     todo!();
        // }
        _ => return Ok(false),
    }

    Ok(true)
}
