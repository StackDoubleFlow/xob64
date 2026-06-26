use iced_x86::{
    Code, Instruction,
    code_asm::{CodeAssembler, gpr64, ptr},
};

use crate::runner::{
    CHUNK_SIZE, ExecPool, callbacks,
    compiler::instr_utils::{IcedResult, label_target, make_call},
    get_exec,
};

pub fn rewrite_branch(arm_instr: &bad64::Instruction, call_ptr: *const u8) -> *const u8 {
    assert!(arm_instr.op() == bad64::Op::BL || arm_instr.op() == bad64::Op::B);
    let label_target = label_target(arm_instr.operands()[0]);
    let exec_ptr = get_exec(label_target as *const u8);

    let mut ass = CodeAssembler::new(64).unwrap();
    ass.mov(gpr64::rax, exec_ptr as u64).unwrap();
    ass.jmp(gpr64::rax).unwrap();

    let new_code = ass.assemble(call_ptr as u64).unwrap();
    assert_eq!(new_code.len(), 12);
    let code_slice = unsafe { std::slice::from_raw_parts_mut(call_ptr.cast_mut(), 12) };
    code_slice.copy_from_slice(&new_code);

    exec_ptr
}

fn make_jump(
    ass: &mut CodeAssembler,
    label_operand: bad64::Operand,
    chunk_addr: usize,
    exec_pool: &mut ExecPool,
) -> IcedResult<()> {
    let target = label_target(label_operand);
    if target >= chunk_addr && target < chunk_addr + CHUNK_SIZE {
        let arm_instr_idx = (target - chunk_addr) / 4;
        ass.add_instruction(Instruction::with_branch(
            Code::Jmp_rel32_64,
            arm_instr_idx as u64 + 1,
        )?)?;
    } else {
        let chunk_offset = target % CHUNK_SIZE;
        let chunk_addr = target - chunk_offset;
        if let Some(chunk) = exec_pool.executable_map.get(&chunk_addr) {
            let instr_idx = chunk_offset / 4;
            let byte_idx = chunk.instr_map[instr_idx] as usize;
            let code_target = chunk.addr as usize + byte_idx;
            ass.mov(gpr64::rax, code_target as u64)?;
            ass.jmp(gpr64::rax)?;
        } else {
            make_call(
                ass,
                callbacks::rewrite_branch_landing_pad as *const () as u64,
            )?;
        }
    }
    Ok(())
}

// Returns true if the instruction was successfully translated.
pub fn compile_instr(
    arm_instr: &bad64::Instruction,
    ass: &mut CodeAssembler,
    exec_pool: &mut ExecPool,
    chunk_addr: usize,
) -> Result<bool, iced_x86::IcedError> {
    use bad64::Op;
    match arm_instr.op() {
        Op::B => {
            make_jump(ass, arm_instr.operands()[0], chunk_addr, exec_pool)?;
        }
        Op::BL => {
            let label = ass.fwd()?;
            ass.lea(gpr64::r11, ptr(label))?;

            make_jump(ass, arm_instr.operands()[0], chunk_addr, exec_pool)?;

            // TODO: Maybe just re-use the label the next instruction should already have
            ass.anonymous_label()?;
            ass.zero_bytes().unwrap();
        }
        _ => return Ok(false),
    }

    Ok(true)
}
