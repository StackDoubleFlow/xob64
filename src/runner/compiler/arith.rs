use iced_x86::code_asm::CodeAssembler;

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    match arm_instr.op() {
        Op::ORR {
            // Given `ORR dest, src1, src2`
            //
            // If src1 and dest are not equal, we need to transfer from src1 to dest first, and then perform the operation on dest with src2.
            //
            // This is the absolute worst case:
            // mov rax, [r15 + src1n * 8] ; Load indirect src1
            // mov [r15 + destn * 8], rax; Transfer to indirect dest
            // mov rax, [r15 + src2n * 8] ; Load indirect src2
            // or [r15 + destn * 8], rax ; Perform operand on dest with src2
        }
        _ => return Ok(false),
    }

    Ok(true)
}
