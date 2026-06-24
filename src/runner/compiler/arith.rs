use iced_x86::code_asm::{CodeAssembler, gpr32, gpr64};

use crate::runner::compiler::register::{
    RegTranslation, get_reg_class, lower_reg_to_class, reg_operand_gpr, reg_operand_no_mem,
    store_indirect, translate_reg, unwrap_reg,
};

fn move_to_dest(
    ass: &mut CodeAssembler,
    dest: bad64::Reg,
    src: bad64::Reg,
) -> Result<(), iced_x86::IcedError> {
    let (dest_class, dest_top_level) = get_reg_class(dest);
    let dest_translation = translate_reg(dest_top_level);

    match dest_translation {
        RegTranslation::Indirect(dest_indirect_idx) => {
            let src_reg = reg_operand_no_mem(ass, src)?;
            if let Some(src_reg) = gpr64::get_gpr64(src_reg) {
                store_indirect(ass, src_reg, dest_indirect_idx)?;
            } else {
                let src_reg = gpr32::get_gpr32(src_reg).unwrap();
                store_indirect(ass, src_reg, dest_indirect_idx)?;
            }
        }
        RegTranslation::Direct(dest_reg) => {
            let dest_reg = lower_reg_to_class(dest_reg, dest_class);
            reg_operand_gpr(src).mov_src_reg(ass, dest_reg)?;
        }
    }
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
