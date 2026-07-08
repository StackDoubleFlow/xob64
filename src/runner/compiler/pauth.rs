use iced_x86::code_asm::CodeAssembler;

use crate::runner::compiler::CompileResult;

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    _ass: &mut CodeAssembler,
) -> CompileResult<bool> {
    use bad64::Op;
    match arm_instr.op() {
        // I don't think we have to emulate pointer authentication. These instructions modify the pointer in-place to add the PAC, so it shouldn't be a problem to have these be no-op.
        Op::PACIASP
        | Op::PACIAZ
        | Op::PACIA1716
        | Op::PACIBSP
        | Op::PACIBZ
        | Op::PACIB1716
        | Op::PACIA
        | Op::PACDA
        | Op::PACIB
        | Op::PACDB
        | Op::PACIZA
        | Op::PACDZA
        | Op::PACIZB
        | Op::PACDZB => {}
        // TODO: PACGA, should be a simple mov
        // Authenticate PAC instructions
        Op::AUTIASP
        | Op::AUTIAZ
        | Op::AUTIA1716
        | Op::AUTIBSP
        | Op::AUTIBZ
        | Op::AUTIB1716
        | Op::AUTIA
        | Op::AUTDA
        | Op::AUTIB
        | Op::AUTDB
        | Op::AUTIZA
        | Op::AUTDZA
        | Op::AUTIZB
        | Op::AUTDZB => {}
        // Since we don't add a PAC, stripping it also becomes a no-op
        Op::XPACD | Op::XPACI | Op::XPACLRI => {}

        _ => return Ok(false),
    }

    Ok(true)
}
