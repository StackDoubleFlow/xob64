mod arith;
pub mod branch;
mod dump;
mod instr_utils;
mod load_store;
mod pauth;
mod register;

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, BlockEncoderResult, Instruction, InstructionBlock,
    code_asm::CodeAssembler,
};

use crate::runner::{
    CHUNK_SIZE, CompiledChunk, EXECUTABLE_ALLOC_SIZE, ExecPool, callbacks,
    compiler::instr_utils::make_call,
};

fn alloc_new_region(exec_pool: &mut ExecPool) {
    if !exec_pool.current_alloc.is_null() {
        exec_pool.fully_used_allocs.push(exec_pool.current_alloc);
    }

    let new_alloc = unsafe {
        nix::libc::mmap(
            std::ptr::null_mut(),
            EXECUTABLE_ALLOC_SIZE,
            nix::libc::PROT_EXEC | nix::libc::PROT_WRITE,
            nix::libc::MAP_ANONYMOUS | nix::libc::MAP_PRIVATE,
            -1,
            0,
        )
    };
    if new_alloc as isize == -1 {
        unsafe {
            nix::libc::perror(c"exec mmap failed".as_ptr());
        }
        panic!("exec mmap failed");
    }
    exec_pool.current_alloc = new_alloc as *const u8;
    exec_pool.current_alloc_utilization = 0;
}

fn assemble_instrs(instrs: &[Instruction], ip: u64) -> BlockEncoderResult {
    let block = InstructionBlock::new(instrs, ip);
    BlockEncoder::encode(
        64,
        block,
        BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS
            | BlockEncoderOptions::DONT_FIX_BRANCHES,
    )
    .unwrap()
}

fn finalize_ass(
    exec_pool: &mut ExecPool,
    mut ass: CodeAssembler,
    x86_idxs: &[usize],
) -> CompiledChunk {
    let instrs = ass.take_instructions();

    if exec_pool.current_alloc.is_null() {
        alloc_new_region(exec_pool);
    }

    let enc_result = assemble_instrs(&instrs, exec_pool.current_alloc as u64);
    // If this block exceeds the current allocation, allocate a new one and re-encode with the new rip.
    let enc_result = if enc_result.code_buffer.len() + exec_pool.current_alloc_utilization
        > EXECUTABLE_ALLOC_SIZE
    {
        alloc_new_region(exec_pool);
        assemble_instrs(&instrs, exec_pool.current_alloc as u64)
    } else {
        enc_result
    };

    // Copy to executable memory
    let target_addr = unsafe {
        exec_pool
            .current_alloc
            .cast_mut()
            .add(exec_pool.current_alloc_utilization)
    };
    let target_slice =
        unsafe { std::slice::from_raw_parts_mut(target_addr, enc_result.code_buffer.len()) };
    target_slice.copy_from_slice(&enc_result.code_buffer);
    exec_pool.current_alloc_utilization += enc_result.code_buffer.len();

    let mut instr_map = Vec::new();
    for arm_idx in 0..CHUNK_SIZE / 4 {
        let x86_idx = x86_idxs[arm_idx];
        let offset = if x86_idx == enc_result.new_instruction_offsets.len() {
            enc_result.code_buffer.len() as u32
        } else {
            enc_result.new_instruction_offsets[x86_idx]
        };
        instr_map.push(offset.try_into().expect("chunk offset overflow"));
    }

    CompiledChunk {
        instr_map,
        addr: target_addr,
        len: enc_result.code_buffer.len(),
    }
}

pub fn compile_instr(
    exec_pool: &mut ExecPool,
    arm_instr: &Result<bad64::Instruction, bad64::DecodeError>,
    ass: &mut CodeAssembler,
    chunk_addr: usize,
) -> Result<(), iced_x86::IcedError> {
    let Ok(arm_instr) = arm_instr else {
        make_call(ass, callbacks::invalid_arm_instr as *const () as u64)?;
        return Ok(());
    };

    // Load and store instructions
    if load_store::compile_instr(arm_instr, ass)? {
        return Ok(());
    }

    // Arithmetic and logic instructions
    if arith::compile_instr(arm_instr, ass)? {
        return Ok(());
    }

    // Branch instructions
    if branch::compile_instr(arm_instr, ass, exec_pool, chunk_addr)? {
        return Ok(());
    }

    // Pointer authentication instructions (FEAT_PAuth)
    if pauth::compile_instr(arm_instr, ass)? {
        return Ok(());
    }

    make_call(
        ass,
        callbacks::unimplemented_arm_instr_landing_pad as *const () as u64,
    )?;

    Ok(())
}

fn get_arm_chunk(
    chunk_addr: usize,
) -> impl Iterator<Item = Result<bad64::Instruction, bad64::DecodeError>> {
    bad64::disasm(
        unsafe { std::slice::from_raw_parts(chunk_addr as *const u8, CHUNK_SIZE) },
        chunk_addr as u64,
    )
}

pub fn compile_chunk(exec_pool: &mut ExecPool, chunk_addr: usize) -> CompiledChunk {
    let arm_instrs = get_arm_chunk(chunk_addr);

    let mut ass = CodeAssembler::new(64).unwrap();
    // Each label should have an id of instr index + 1
    let mut ass_labels: Vec<_> = (0..CHUNK_SIZE / 4).map(|_| ass.create_label()).collect();

    let mut x86_idxs = Vec::new();
    for (instr_idx, arm_instr) in arm_instrs.enumerate() {
        x86_idxs.push(ass.instructions().len());
        ass.set_label(&mut ass_labels[instr_idx]).unwrap();
        // TODO: what's the performance penalty of this?
        ass.zero_bytes().unwrap();

        compile_instr(exec_pool, &arm_instr, &mut ass, chunk_addr).unwrap();
    }
    make_call(&mut ass, callbacks::end_of_chunk as *const () as u64).unwrap();

    let compiled_chunk = finalize_ass(exec_pool, ass, &x86_idxs);
    dump::dump_translation(chunk_addr, &compiled_chunk);
    compiled_chunk
}
