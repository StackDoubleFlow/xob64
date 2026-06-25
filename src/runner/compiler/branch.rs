use iced_x86::{Instruction, code_asm::CodeAssembler};

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    match arm_instr.op() {
        // Op::BL => {
        //     ass.add_instruction(instruction)
        // }
        _ => return Ok(false),
    }

    Ok(true)
}
