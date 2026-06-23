use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, Formatter, Instruction,
    InstructionBlock, IntelFormatter,
    code_asm::{self, CodeAssembler},
};

use crate::runner::{CHUNK_SIZE, CompiledChunk, EXECUTABLE_ALLOC_SIZE, ExecPool, callbacks};

// Allocation for all 16 integer x86_64 registers:
// x0 -> %rdi (1st argument)
// x31 (sp) -> %rsp (stack pointer)
// x1 -> %rsi (2nd argument)
// x2 -> %rdx (3rd argument)
// x19 -> %rbx (1st callee-saved)
// x3 -> %rcx (4th argument)
// x20 -> %r12 (2nd callee-saved)
// x21 -> %r13 (3rd callee-saved)
// x4 -> %r8 (5th argument)
// x29 (fp) -> %rbp (frame pointer)
// x22 -> %r14 (4th callee-saved)
// x5 -> %r9 (6th argument)
// x23 -> %r10 (5th callee-saved -> temporary)
// x30 -> %r11 (link register -> temporary)
// %rax (emulation scratch)
// %r15 (emulation context)

// Allocation for all 16 fp registers:
// v0-v7 -> %xmm0-xmm7 (argument/return value)
// v16-v22 -> %xmm8-xmm14 (temporary)
// %xmm15 -> (emulation scratch)

// Translated Aarch64 registers:
// x0-x5, x19-x23, x29-x31
// v0-v7, v16-v23
// Emulated Aarch64 registers:
// x6-x18, x24-x28 (18 64-bit registers)
// v8-v15, v23-v31 (17 128-bit registers)

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

fn finalize_ass(
    exec_pool: &mut ExecPool,
    mut ass: CodeAssembler,
    x86_idxs: &[usize],
) -> CompiledChunk {
    if exec_pool.current_alloc.is_null() {
        alloc_new_region(exec_pool);
    }

    // FIXME: why does this take in &mut ass
    let enc_result = ass
        .assemble_options(
            exec_pool.current_alloc as u64,
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )
        .unwrap()
        .inner;

    // If this block exceeds the current allocation, allocate a new one and re-encode with the new rip.
    let enc_result = if enc_result.code_buffer.len() + exec_pool.current_alloc_utilization
        > EXECUTABLE_ALLOC_SIZE
    {
        alloc_new_region(exec_pool);
        ass.assemble_options(
            exec_pool.current_alloc as u64,
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )
        .unwrap()
        .inner
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
    arm_instr: &Result<bad64::Instruction, bad64::DecodeError>,
    ass: &mut CodeAssembler,
) -> Result<(), iced_x86::IcedError> {
    let Ok(arm_instr) = arm_instr else {
        // ass.call(callbacks::invalid_arm_instr as *const () as u64)?;
        ass.nop()?;
        return Ok(());
    };

    // match arm_instr.op() {
    //     _ => ass.nop()?,
    // }

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

#[derive(Default)]
struct X86Formatter {
    inner: IntelFormatter,
    str: String,
}

impl X86Formatter {
    fn format(&mut self, instr: &Instruction) -> &str {
        self.str.clear();
        self.inner.format(instr, &mut self.str);
        &self.str
    }
}

fn dump_instr_pair(
    arm_instr: Option<(usize, &Result<bad64::Instruction, bad64::DecodeError>)>,
    x86_instr: Option<&Instruction>,
    formatter: &mut X86Formatter,
) {
    if let Some((arm_instr_addr, arm_instr)) = arm_instr {
        let arm_asm = match arm_instr {
            Ok(arm_instr) => format!("{}", arm_instr),
            Err(_) => "invalid instruction".to_string(),
        };
        print!("{:?}: {:<70}", arm_instr_addr as *const u8, arm_asm);
    } else {
        print!("{:<90}", "");
    }

    if let Some(x86_instr) = x86_instr {
        print!(
            "{:?}: {}",
            x86_instr.ip() as *const u8,
            formatter.format(x86_instr)
        );
    }
    println!();
}

fn dump_translation(chunk_addr: usize, compiled_chunk: &CompiledChunk) {
    let mut arm_instrs = get_arm_chunk(chunk_addr);

    let x86_code = unsafe { std::slice::from_raw_parts(compiled_chunk.addr, compiled_chunk.len) };
    let mut x86_decoder = Decoder::with_ip(
        64,
        x86_code,
        compiled_chunk.addr as u64,
        DecoderOptions::NONE,
    );
    let mut formatter = X86Formatter::default();

    let mut decoded_x86_instr = Instruction::new();
    let x86_offset = |instr: &Instruction| instr.ip() as usize - compiled_chunk.addr as usize;

    x86_decoder.decode_out(&mut decoded_x86_instr);
    dump_instr_pair(
        Some((chunk_addr, &arm_instrs.next().unwrap())),
        Some(&decoded_x86_instr),
        &mut formatter,
    );

    for (arm_instr_idx, arm_instr) in arm_instrs.enumerate() {
        let arm_instr_idx = arm_instr_idx + 1;
        let arm_addr = chunk_addr + arm_instr_idx * 4;

        let cur_arm_x86_offset = compiled_chunk.instr_map[arm_instr_idx] as usize;
        if x86_offset(&decoded_x86_instr) == cur_arm_x86_offset {
            // We've already printed the corresponding x86 instruction, so we only print the ARM instruction;
            dump_instr_pair(Some((arm_addr, &arm_instr)), None, &mut formatter);
            continue;
        }

        x86_decoder.decode_out(&mut decoded_x86_instr);
        while x86_offset(&decoded_x86_instr) < cur_arm_x86_offset {
            // We haven't reached the ARM instruction yet, so we print the x86 instruction and decode the next one.
            dump_instr_pair(None, Some(&decoded_x86_instr), &mut formatter);
            x86_decoder.decode_out(&mut decoded_x86_instr);
        }

        dump_instr_pair(
            Some((arm_addr, &arm_instr)),
            Some(&decoded_x86_instr),
            &mut formatter,
        );
    }

    loop {
        x86_decoder.decode_out(&mut decoded_x86_instr);
        if decoded_x86_instr.is_invalid() {
            break;
        } else {
            dump_instr_pair(None, Some(&decoded_x86_instr), &mut formatter);
        }
    }
}

pub fn compile_chunk(exec_pool: &mut ExecPool, chunk_addr: usize) -> CompiledChunk {
    let arm_instrs = get_arm_chunk(chunk_addr);

    let mut ass = CodeAssembler::new(64).unwrap();

    let mut x86_idxs = Vec::new();
    for arm_instr in arm_instrs {
        x86_idxs.push(ass.instructions().len());
        compile_instr(&arm_instr, &mut ass).unwrap();
    }

    let compiled_chunk = finalize_ass(exec_pool, ass, &x86_idxs);
    dump_translation(chunk_addr, &compiled_chunk);
    compiled_chunk
}
