use iced_x86::code_asm::CodeAssembler;

use crate::runner::compiler::{
    instr_utils::{codes::MOV_RR_CODES, make_rr},
    register::{translate_reg, unwrap_reg},
};

fn move_to_dest(
    ass: &mut CodeAssembler,
    dest: bad64::Reg,
    src: bad64::Reg,
) -> Result<(), iced_x86::IcedError> {
    let (dest_translation, reg_class) = translate_reg(dest);
    let (src_translation, _) = translate_reg(src);
    make_rr(
        ass,
        &MOV_RR_CODES,
        reg_class,
        dest_translation,
        src_translation,
    )?;
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
        //     // Given `ORR dest, src1, src2`
        //     //
        //     // If src1 and dest are not equal, we need to transfer from src1 to dest first, and then perform the operation on dest with src2.
        //     //
        //     // This is the absolute worst case:
        //     // mov rax, [r15 + src1n * 8] ; Load indirect src1
        //     // mov [r15 + destn * 8], rax; Transfer to indirect dest
        //     // mov rax, [r15 + src2n * 8] ; Load indirect src2
        //     // or [r15 + destn * 8], rax ; Perform operand on dest with src2

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
