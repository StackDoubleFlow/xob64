use iced_x86::code_asm::CodeAssembler;

use crate::runner::compiler::{
    instr_utils::make_mov_rr,
    register::{translate_reg, unwrap_reg},
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

fn move_to_dest(
    ass: &mut CodeAssembler,
    dest: bad64::Reg,
    src: bad64::Reg,
) -> Result<(), iced_x86::IcedError> {
    let (dest_translation, reg_class) = translate_reg(dest);
    let (src_translation, _) = translate_reg(src);
    make_mov_rr(ass, reg_class, dest_translation, src_translation)?;
    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    match arm_instr.op() {
        Op::MOV => {
            let operands = arm_instr.operands();
            let dest = unwrap_reg(operands[0]);
            let src = unwrap_reg(operands[1]);
            move_to_dest(ass, dest, src)?;
        }
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
